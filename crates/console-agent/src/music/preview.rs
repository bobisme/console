//! Source-file preview through the console's real six-channel synthesizer.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use console_core::{CHANNEL_COUNT, PreviewSynth, SAMPLE_RATE, SAMPLES_PER_FRAME};
use cpal::{
    Device, FromSample, I24, OutputCallbackInfo, SampleFormat, SizedSample, StreamConfig, U24,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use super::abc::{AbcEvent, AbcTune, Frac, parse_abc, parse_abc_segments};
use super::midi::{MidiSong, parse_midi};

const MAX_PREVIEW_SECONDS: f64 = 10.0 * 60.0;
const MAX_ABC_HEADER_BYTES: usize = 64 * 1024;
const MAX_ABC_VOICES: usize = 64;
const MAX_ABC_EVENTS: usize = 250_000;
const DEFAULT_PLAYBACK_VOLUME: f32 = 0.5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedNote {
    id: u64,
    start_frame: u64,
    end_frame: u64,
    note: u8,
    wave: u8,
    volume: u8,
    source: usize,
    priority: u8,
}

#[derive(Debug, Clone)]
pub struct TimedScore {
    pub title: String,
    pub notes: Vec<TimedNote>,
    pub duration_frames: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub notes_started: usize,
    pub channel_steals: usize,
    pub release_frames: u64,
}

pub fn score_from_midi(song: &MidiSong) -> TimedScore {
    let mut warnings = song.warnings.clone();
    let mut folded = 0usize;
    let mut notes = Vec::with_capacity(song.notes.len());
    for source in &song.notes {
        let (note, did_fold) = fold_midi_key(source.key);
        folded += usize::from(did_fold);
        let start_frame = song.tick_to_frame(source.start_tick);
        let end_frame = song
            .tick_to_frame(source.end_tick)
            .max(start_frame.saturating_add(1));
        notes.push(TimedNote {
            id: source.id,
            start_frame,
            end_frame,
            note,
            wave: midi_wave(source.channel, source.program),
            volume: velocity_volume(source.velocity),
            source: source.track * 16 + usize::from(source.channel),
            priority: source.velocity,
        });
    }
    if folded != 0 {
        warnings.push(format!(
            "octave-folded {folded} MIDI note(s) into the console's C0-B7 range"
        ));
    }
    let duration_frames = notes.iter().map(|note| note.end_frame).max().unwrap_or(0);
    TimedScore {
        title: song
            .title
            .clone()
            .unwrap_or_else(|| "MIDI preview".to_string()),
        notes,
        duration_frames,
        warnings,
    }
}

pub fn score_from_abc(text: &str) -> Result<TimedScore, String> {
    let tunes = parse_abc_voices(text)?;
    let title = tunes
        .first()
        .and_then(|tune| tune.title.clone())
        .unwrap_or_else(|| "ABC preview".to_string());
    let mut notes = Vec::new();
    let mut warnings = Vec::new();
    let mut next_id = 0u64;
    let mut duration_frames = 0u64;
    for (source, tune) in tunes.iter().enumerate() {
        warnings.extend(tune.warnings.iter().cloned());
        let (beat, bpm) = tune.tempo.unwrap_or((Frac::new(1, 4), 120));
        let mut elapsed = Frac::new(0, 1);
        for event in &tune.events {
            let start_frame = whole_notes_to_frames(elapsed, beat, bpm)?;
            elapsed = elapsed
                .checked_add(event.dur())
                .ok_or("ABC cumulative duration is too complex")?;
            let end_frame =
                whole_notes_to_frames(elapsed, beat, bpm)?.max(start_frame.saturating_add(1));
            if let AbcEvent::Note { note, .. } = event {
                let (note, did_fold) = fold_console_note(*note);
                if did_fold {
                    warnings.push(format!(
                        "voice {} octave-folded an out-of-range ABC note into C0-B7",
                        source + 1
                    ));
                }
                notes.push(TimedNote {
                    id: next_id,
                    start_frame,
                    end_frame,
                    note,
                    wave: abc_wave(source),
                    volume: 6,
                    source,
                    priority: 96,
                });
                next_id += 1;
            }
            duration_frames = duration_frames.max(end_frame);
        }
    }
    warnings.sort();
    warnings.dedup();
    Ok(TimedScore {
        title,
        notes,
        duration_frames,
        warnings,
    })
}

fn whole_notes_to_frames(duration: Frac, beat: Frac, bpm: u32) -> Result<u64, String> {
    if bpm == 0 || beat.num == 0 {
        return Ok(0);
    }
    let numerator = i128::from(duration.num)
        .checked_mul(i128::from(beat.den))
        .and_then(|value| value.checked_mul(60 * 60))
        .ok_or("ABC duration is too large to schedule")?;
    let denominator = i128::from(duration.den)
        .checked_mul(i128::from(beat.num))
        .and_then(|value| value.checked_mul(i128::from(bpm)))
        .filter(|value| *value > 0)
        .ok_or("ABC tempo is invalid or too large to schedule")?;
    let frames = numerator
        .checked_add(denominator / 2)
        .ok_or("ABC duration is too large to schedule")?
        / denominator;
    u64::try_from(frames).map_err(|_| "ABC duration is too large to schedule".to_string())
}

fn parse_abc_voices(text: &str) -> Result<Vec<AbcTune>, String> {
    let mut global = String::new();
    let mut order = Vec::<String>::new();
    let mut bodies = std::collections::BTreeMap::<String, String>::new();
    let mut active = None::<String>;
    let mut body_started = false;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if let Some(value) = trimmed.strip_prefix("V:") {
            let voice = value.split_whitespace().next().unwrap_or("1").to_string();
            if voice.len() > 128 {
                return Err("ABC voice identifier exceeds 128 bytes".to_string());
            }
            if !bodies.contains_key(&voice) {
                if order.len() >= MAX_ABC_VOICES {
                    return Err(format!("ABC declares more than {MAX_ABC_VOICES} voices"));
                }
                order.push(voice.clone());
                bodies.insert(voice.clone(), String::new());
            }
            active = Some(voice);
            continue;
        }
        let field = trimmed.as_bytes().get(1).copied() == Some(b':')
            && trimmed
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic);
        if field && !body_started {
            if global.len() + raw.len() + 1 > MAX_ABC_HEADER_BYTES {
                return Err(format!(
                    "ABC shared header exceeds the {MAX_ABC_HEADER_BYTES}-byte limit"
                ));
            }
            global.push_str(raw);
            global.push('\n');
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('%') {
            continue;
        }
        body_started = true;
        let voice = active.clone().unwrap_or_else(|| "1".to_string());
        if !bodies.contains_key(&voice) {
            if order.len() >= MAX_ABC_VOICES {
                return Err(format!("ABC declares more than {MAX_ABC_VOICES} voices"));
            }
            order.push(voice.clone());
            bodies.insert(voice.clone(), String::new());
        }
        let body = bodies.get_mut(&voice).unwrap();
        body.push_str(raw);
        body.push('\n');
    }

    if order.is_empty() {
        return parse_abc(text).map(|tune| vec![tune]);
    }
    let mut tunes = Vec::new();
    let mut total_events = 0usize;
    for voice in order {
        let body = &bodies[&voice];
        if body.is_empty() {
            continue;
        }
        let voice_field = format!("V:{voice}\n");
        let tune = parse_abc_segments(&[&global, &voice_field, body])?;
        total_events = total_events
            .checked_add(tune.events.len())
            .filter(|count| *count <= MAX_ABC_EVENTS)
            .ok_or_else(|| format!("ABC preview exceeds {MAX_ABC_EVENTS} total events"))?;
        tunes.push(tune);
    }
    if tunes.is_empty() {
        return Err("the ABC input has no voice bodies".to_string());
    }
    Ok(tunes)
}

fn fold_midi_key(key: u8) -> (u8, bool) {
    let mut key = i32::from(key);
    let original = key;
    while key < 12 {
        key += 12;
    }
    while key > 107 {
        key -= 12;
    }
    ((key - 12) as u8, key != original)
}

fn fold_console_note(note: i32) -> (u8, bool) {
    let mut folded = note;
    while folded < 0 {
        folded += 12;
    }
    while folded > 95 {
        folded -= 12;
    }
    (folded as u8, folded != note)
}

fn velocity_volume(velocity: u8) -> u8 {
    (u16::from(velocity) * 7).div_ceil(127).clamp(1, 7) as u8
}

fn midi_wave(channel: u8, program: u8) -> u8 {
    if channel == 9 {
        return 5;
    }
    match program {
        32..=39 => 3,  // bass
        40..=55 => 4,  // strings, ensemble, brass
        80..=87 => 1,  // synth leads
        88..=103 => 0, // pads and effects
        _ => 2,
    }
}

fn abc_wave(source: usize) -> u8 {
    [2, 3, 4, 1, 0, 2][source % CHANNEL_COUNT]
}

#[derive(Debug, Clone, Copy)]
struct Active {
    id: u64,
    source: usize,
    priority: u8,
    start_frame: u64,
}

pub fn render_score(
    score: &TimedScore,
    max_frames: Option<u64>,
) -> Result<(Vec<f32>, RenderStats), String> {
    let hard_limit = (MAX_PREVIEW_SECONDS * 60.0) as u64;
    let frames = max_frames
        .unwrap_or(score.duration_frames)
        .min(score.duration_frames);
    if frames > hard_limit {
        return Err(format!(
            "preview is {:.1} minutes; the safety limit is {:.0} minutes (use --seconds to audition a shorter excerpt)",
            frames as f64 / 3600.0,
            MAX_PREVIEW_SECONDS / 60.0
        ));
    }
    let sample_count = frames
        .checked_mul(SAMPLES_PER_FRAME as u64)
        .and_then(|count| usize::try_from(count).ok())
        .ok_or("preview is too large to render on this host")?;
    let mut samples = Vec::with_capacity(sample_count);
    let mut starts: Vec<&TimedNote> = score
        .notes
        .iter()
        .filter(|note| note.start_frame < frames)
        .collect();
    starts.sort_by_key(|note| (note.start_frame, note.source, note.id));
    let mut ends = starts.clone();
    ends.sort_by_key(|note| (note.end_frame, note.source, note.id));
    let mut start_index = 0usize;
    let mut end_index = 0usize;
    let mut active = [None::<Active>; CHANNEL_COUNT];
    let mut synth = PreviewSynth::new();
    let mut stats = RenderStats::default();

    for frame in 0..frames {
        while end_index < ends.len() && ends[end_index].end_frame <= frame {
            let note = ends[end_index];
            if let Some(channel) = active
                .iter()
                .position(|voice| voice.is_some_and(|voice| voice.id == note.id))
            {
                synth.note_off(channel)?;
                active[channel] = None;
            }
            end_index += 1;
        }
        while start_index < starts.len() && starts[start_index].start_frame <= frame {
            let note = starts[start_index];
            let channel = choose_channel(&active, note);
            if active[channel].is_some() {
                synth.note_off(channel)?;
                stats.channel_steals += 1;
            }
            synth.note_on(channel, note.note, note.wave, note.volume)?;
            active[channel] = Some(Active {
                id: note.id,
                source: note.source,
                priority: note.priority,
                start_frame: note.start_frame,
            });
            stats.notes_started += 1;
            start_index += 1;
        }
        samples.extend_from_slice(synth.render_frame());
    }
    if active.iter().any(Option::is_some) {
        for (channel, voice) in active.iter_mut().enumerate() {
            if voice.take().is_some() {
                synth.note_off(channel)?;
            }
        }
        // One console frame is longer than the 64-sample click ramp, so its
        // suffix is guaranteed silent and CPAL never sees a hard final edge.
        samples.extend_from_slice(synth.render_frame());
        stats.release_frames = 1;
    }
    Ok((samples, stats))
}

fn choose_channel(active: &[Option<Active>; CHANNEL_COUNT], note: &TimedNote) -> usize {
    if let Some(channel) = active.iter().position(Option::is_none) {
        return channel;
    }
    if let Some(channel) = active
        .iter()
        .position(|voice| voice.is_some_and(|voice| voice.source == note.source))
    {
        return channel;
    }
    active
        .iter()
        .enumerate()
        .min_by_key(|(channel, voice)| {
            let voice = voice.expect("all channels are active");
            (voice.priority, voice.start_frame, *channel)
        })
        .map(|(channel, _)| channel)
        .unwrap_or(0)
}

fn read_score(path: &Path) -> Result<TimedScore, String> {
    let bytes = super::read_bounded(path, "music source")?;
    let midi = bytes.starts_with(b"MThd")
        || path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"));
    if midi {
        return parse_midi(&bytes).map(|song| score_from_midi(&song));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| format!("ABC source {} is not UTF-8", path.display()))?;
    score_from_abc(text)
}

#[derive(Debug)]
struct PlayArgs {
    input: PathBuf,
    seconds: Option<f64>,
    volume: f32,
    repeat: bool,
    dry_run: bool,
}

impl PlayArgs {
    fn parse(args: &[String]) -> Result<PlayArgs, String> {
        let mut input = None;
        let mut seconds = None;
        let mut volume = DEFAULT_PLAYBACK_VOLUME;
        let mut repeat = false;
        let mut dry_run = false;
        let mut index = 0usize;
        while index < args.len() {
            match args[index].as_str() {
                "--dry-run" => dry_run = true,
                "--repeat" => repeat = true,
                "--seconds" => {
                    index += 1;
                    let value = args.get(index).ok_or("--seconds requires a value")?;
                    let parsed = value.parse::<f64>().map_err(|_| {
                        format!("invalid --seconds value {value:?} (want a positive number)")
                    })?;
                    if !parsed.is_finite() || parsed <= 0.0 {
                        return Err("--seconds must be a finite positive number".to_string());
                    }
                    seconds = Some(parsed);
                }
                "--volume" => {
                    index += 1;
                    let value = args.get(index).ok_or("--volume requires a value")?;
                    let parsed = value.parse::<f32>().map_err(|_| {
                        format!("invalid --volume value {value:?} (want a number from 0 to 1)")
                    })?;
                    if !parsed.is_finite() || !(0.0..=1.0).contains(&parsed) {
                        return Err("--volume must be a finite number from 0 to 1".to_string());
                    }
                    volume = parsed;
                }
                value if value.starts_with('-') => return Err(format!("unknown flag {value:?}")),
                value => {
                    if input.replace(PathBuf::from(value)).is_some() {
                        return Err("expected exactly one ABC or MIDI file".to_string());
                    }
                }
            }
            index += 1;
        }
        Ok(PlayArgs {
            input: input.ok_or("missing ABC or MIDI file")?,
            seconds,
            volume,
            repeat,
            dry_run,
        })
    }
}

const PLAY_USAGE: &str = "usage: console music play <file.abc|file.mid|file.midi> \
    [--seconds N] [--volume 0..1] [--repeat] [--dry-run]";

pub fn cli_play(args: &[String]) -> i32 {
    if crate::help_requested(args) {
        println!("{PLAY_USAGE}");
        return 0;
    }
    let args = match PlayArgs::parse(args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("{PLAY_USAGE}");
            return 2;
        }
    };
    match run_play(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn run_play(args: PlayArgs) -> Result<(), String> {
    let score = read_score(&args.input)?;
    let max_frames = args.seconds.map(|seconds| (seconds * 60.0).ceil() as u64);
    let (mut samples, stats) = render_score(&score, max_frames)?;
    let program_samples = samples
        .len()
        .saturating_sub(stats.release_frames as usize * SAMPLES_PER_FRAME);
    let rendered_seconds = program_samples as f64 / f64::from(SAMPLE_RATE);
    let mode = match (args.dry_run, args.repeat) {
        (true, true) => " (dry run, repeat)",
        (true, false) => " (dry run)",
        (false, true) => " (repeat; Ctrl-C to stop)",
        (false, false) => "",
    };
    eprintln!(
        "{}: {:.2}s + {} release frame(s), {} note(s), {} channel steal(s), volume {:.2}{}",
        score.title,
        rendered_seconds,
        stats.release_frames,
        stats.notes_started,
        stats.channel_steals,
        args.volume,
        mode
    );
    for warning in &score.warnings {
        eprintln!("warning: {warning}");
    }
    if !args.dry_run {
        apply_playback_volume(&mut samples, args.volume);
        play_samples(Arc::from(samples), args.repeat)?;
    }
    Ok(())
}

fn apply_playback_volume(samples: &mut [f32], volume: f32) {
    for sample in samples {
        *sample *= volume;
    }
}

fn play_samples(samples: Arc<[f32]>, repeat: bool) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no default audio output device is available")?;
    let supported = device
        .default_output_config()
        .map_err(|error| format!("cannot query the default audio output: {error}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    match format {
        SampleFormat::I8 => play_typed::<i8>(&device, config, samples, repeat),
        SampleFormat::I16 => play_typed::<i16>(&device, config, samples, repeat),
        SampleFormat::I24 => play_typed::<I24>(&device, config, samples, repeat),
        SampleFormat::I32 => play_typed::<i32>(&device, config, samples, repeat),
        SampleFormat::I64 => play_typed::<i64>(&device, config, samples, repeat),
        SampleFormat::U8 => play_typed::<u8>(&device, config, samples, repeat),
        SampleFormat::U16 => play_typed::<u16>(&device, config, samples, repeat),
        SampleFormat::U24 => play_typed::<U24>(&device, config, samples, repeat),
        SampleFormat::U32 => play_typed::<u32>(&device, config, samples, repeat),
        SampleFormat::U64 => play_typed::<u64>(&device, config, samples, repeat),
        SampleFormat::F32 => play_typed::<f32>(&device, config, samples, repeat),
        SampleFormat::F64 => play_typed::<f64>(&device, config, samples, repeat),
        other => Err(format!("unsupported output sample format {other}")),
    }
}

#[derive(Debug)]
struct SampleCursor {
    position: f64,
    step: f64,
    repeat: bool,
}

impl SampleCursor {
    fn new(step: f64, repeat: bool) -> SampleCursor {
        SampleCursor {
            position: 0.0,
            step,
            repeat,
        }
    }

    fn next(&mut self, samples: &[f32]) -> Option<f32> {
        if samples.is_empty() {
            return None;
        }
        let len = samples.len() as f64;
        if self.repeat && self.position >= len {
            self.position %= len;
        }
        let index = self.position.floor() as usize;
        let a = *samples.get(index)?;
        let b = samples
            .get(index + 1)
            .copied()
            .unwrap_or_else(|| if self.repeat { samples[0] } else { a });
        let fraction = (self.position - index as f64) as f32;
        self.position += self.step;
        Some(a + (b - a) * fraction)
    }
}

fn play_typed<T>(
    device: &Device,
    config: StreamConfig,
    samples: Arc<[f32]>,
    repeat: bool,
) -> Result<(), String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    if channels == 0 || config.sample_rate == 0 {
        return Err(format!(
            "default audio output reported an invalid {}-channel / {} Hz configuration",
            channels, config.sample_rate
        ));
    }
    let step = f64::from(SAMPLE_RATE) / f64::from(config.sample_rate);
    let deadline = (!repeat).then(|| {
        Instant::now()
            + Duration::from_secs_f64(samples.len() as f64 / f64::from(SAMPLE_RATE) + 5.0)
    });
    let done = Arc::new(AtomicBool::new(false));
    let callback_done = Arc::clone(&done);
    let error = Arc::new(Mutex::new(None::<String>));
    let callback_error = Arc::clone(&error);
    let mut cursor = SampleCursor::new(step, repeat);
    let stream = device
        .build_output_stream(
            config,
            move |output: &mut [T], _: &OutputCallbackInfo| {
                for frame in output.chunks_mut(channels) {
                    let value = cursor.next(&samples).unwrap_or_else(|| {
                        callback_done.store(true, Ordering::Release);
                        0.0
                    });
                    let value = T::from_sample(value);
                    for sample in frame {
                        *sample = value;
                    }
                }
            },
            move |stream_error| {
                if let Ok(mut slot) = callback_error.lock() {
                    *slot = Some(stream_error.to_string());
                }
            },
            None,
        )
        .map_err(|error| format!("cannot open the default audio output: {error}"))?;
    stream
        .play()
        .map_err(|error| format!("cannot start audio playback: {error}"))?;
    loop {
        if playback_status(&done, &error)? {
            break;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err("audio output did not consume samples before its deadline".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Give the host buffer submitted by the final callback time to reach the
    // device before dropping the stream.
    std::thread::sleep(Duration::from_millis(150));
    playback_status(&done, &error)?;
    Ok(())
}

/// Check the error slot before completion so simultaneous data/error callbacks
/// cannot turn a stream failure into success. Called again after the drain
/// window to catch errors delivered while the final host buffer was playing.
fn playback_status(done: &AtomicBool, error: &Mutex<Option<String>>) -> Result<bool, String> {
    let mut error = error
        .lock()
        .map_err(|_| "audio stream error state was poisoned".to_string())?;
    if let Some(error) = error.take() {
        return Err(format!("audio stream failed: {error}"));
    }
    Ok(done.load(Ordering::Acquire))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn playback_volume_defaults_to_half_and_accepts_the_unit_range() {
        assert_eq!(
            PlayArgs::parse(&args(&["tune.mid"])).unwrap().volume,
            DEFAULT_PLAYBACK_VOLUME
        );
        assert_eq!(
            PlayArgs::parse(&args(&["tune.mid", "--volume", "0"]))
                .unwrap()
                .volume,
            0.0
        );
        assert_eq!(
            PlayArgs::parse(&args(&["--volume", "1", "tune.mid"]))
                .unwrap()
                .volume,
            1.0
        );
        for value in ["-0.01", "1.01", "NaN", "inf", "loud"] {
            let error = PlayArgs::parse(&args(&["tune.mid", "--volume", value])).unwrap_err();
            assert!(error.contains("--volume"), "{value}: {error}");
        }
    }

    #[test]
    fn repeat_is_opt_in_and_combines_with_other_playback_flags() {
        assert!(!PlayArgs::parse(&args(&["tune.mid"])).unwrap().repeat);
        let parsed = PlayArgs::parse(&args(&[
            "--repeat",
            "tune.mid",
            "--seconds",
            "2",
            "--volume",
            "0.25",
        ]))
        .unwrap();
        assert!(parsed.repeat);
        assert_eq!(parsed.seconds, Some(2.0));
        assert_eq!(parsed.volume, 0.25);
    }

    #[test]
    fn repeating_sample_cursor_wraps_and_interpolates_across_the_seam() {
        let samples = [1.0, -1.0];
        let mut once = SampleCursor::new(1.0, false);
        assert_eq!(once.next(&samples), Some(1.0));
        assert_eq!(once.next(&samples), Some(-1.0));
        assert_eq!(once.next(&samples), None);

        let mut repeat = SampleCursor::new(0.5, true);
        assert_eq!(repeat.next(&samples), Some(1.0));
        assert_eq!(repeat.next(&samples), Some(0.0));
        assert_eq!(repeat.next(&samples), Some(-1.0));
        assert_eq!(repeat.next(&samples), Some(0.0));
        assert_eq!(repeat.next(&samples), Some(1.0));
        assert_eq!(repeat.next(&[]), None);
    }

    #[test]
    fn playback_volume_is_linear_output_gain() {
        let mut samples = [0.8, -0.8, 0.0];
        apply_playback_volume(&mut samples, 0.5);
        assert_eq!(samples, [0.4, -0.4, 0.0]);
        apply_playback_volume(&mut samples, 0.0);
        assert_eq!(samples, [0.0; 3]);
    }

    #[test]
    fn abc_polyphony_uses_multiple_source_voices() {
        let score =
            score_from_abc("X:1\nT:Two\nM:4/4\nL:1/4\nQ:1/4=120\nK:C\nV:1\nC D\nV:2\nG, A,\n")
                .unwrap();
        assert_eq!(score.notes.len(), 4);
        assert_eq!(score.duration_frames, 60);
        assert_eq!(score.notes.iter().map(|n| n.source).max(), Some(1));
    }

    #[test]
    fn renderer_uses_console_frame_size_and_steals_deterministically() {
        let notes = (0..7)
            .map(|id| TimedNote {
                id,
                start_frame: 0,
                end_frame: 2,
                note: 48 + id as u8,
                wave: 2,
                volume: 6,
                source: id as usize,
                priority: id as u8,
            })
            .collect();
        let score = TimedScore {
            title: "seven".to_string(),
            notes,
            duration_frames: 2,
            warnings: Vec::new(),
        };
        let (samples, stats) = render_score(&score, None).unwrap();
        assert_eq!(samples.len(), SAMPLES_PER_FRAME * 3);
        assert_eq!(stats.notes_started, 7);
        assert_eq!(stats.channel_steals, 1);
        assert_eq!(stats.release_frames, 1);
        assert!(samples.iter().any(|sample| *sample != 0.0));
        assert!(
            samples[samples.len() - 64..]
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }

    #[test]
    fn clipped_preview_also_ends_with_a_silent_release_tail() {
        let score = score_from_abc("X:1\nL:1/1\nQ:1/4=60\nK:C\nC\n").unwrap();
        let (samples, stats) = render_score(&score, Some(1)).unwrap();
        assert_eq!(stats.release_frames, 1);
        assert_eq!(samples.len(), SAMPLES_PER_FRAME * 2);
        assert!(
            samples[samples.len() - 64..]
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }

    #[test]
    fn stream_error_wins_even_when_completion_is_already_visible() {
        let done = AtomicBool::new(true);
        let error = Mutex::new(Some("device vanished".to_string()));
        assert_eq!(
            playback_status(&done, &error).unwrap_err(),
            "audio stream failed: device vanished"
        );
    }

    #[test]
    fn later_abc_tempo_is_ignored_with_an_explicit_warning() {
        let score = score_from_abc("X:1\nL:1/4\nQ:1/4=120\nK:C\nC\nQ:1/4=60\nD\n").unwrap();
        assert_eq!(score.duration_frames, 60);
        assert!(
            score
                .warnings
                .iter()
                .any(|warning| warning.contains("tempo changes after the first"))
        );
    }

    #[test]
    fn complex_cumulative_abc_duration_returns_an_error() {
        let error = score_from_abc("X:1\nL:1/999983\nQ:1/4=120\nK:C\nC/999979 D/999961 E/999953\n")
            .unwrap_err();
        assert!(error.contains("too complex"), "{error}");
    }

    #[test]
    fn large_header_and_excessive_voice_count_are_bounded_before_reparse() {
        let mut abc = format!("X:1\nT:{}\nL:1/4\nK:C\n", "x".repeat(60_000));
        for voice in 0..=MAX_ABC_VOICES {
            abc.push_str(&format!("V:{voice}\nC\n"));
        }
        let error = score_from_abc(&abc).unwrap_err();
        assert!(error.contains("more than 64 voices"), "{error}");
    }
}

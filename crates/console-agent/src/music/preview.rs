//! ABC/MIDI preview and exact native-music playback through the console's
//! real six-channel synthesizer.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use console_core::{CHANNEL_COUNT, Cart, Console, PreviewSynth, SAMPLE_RATE, SAMPLES_PER_FRAME};
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
const NATIVE_BUFFER_FRAMES: usize = 120;
const NATIVE_PREFILL_FRAMES: usize = 4;
const CLICK_RAMP_SAMPLES: usize = 64;

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

enum PlayInput {
    Source(TimedScore),
    Native { label: String, cart: Box<Cart> },
}

fn read_play_input(path: &Path) -> Result<PlayInput, String> {
    if path.is_dir() || path.file_name().and_then(|name| name.to_str()) == Some("console.toml") {
        let cart_text = crate::project::load_cart_text(path).map_err(|error| error.to_string())?;
        return native_cart(path, cart_text);
    }

    let bytes = super::read_bounded(path, "music source")?;
    let midi = bytes.starts_with(b"MThd") || has_extension(path, &["mid", "midi"]);
    if midi {
        return parse_midi(&bytes).map(|song| PlayInput::Source(score_from_midi(&song)));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| format!("music source {} is not UTF-8", path.display()))?;
    if has_extension(path, &["cmusic"]) || super::native::has_magic(text) {
        let bundle = super::native::NativeMusic::parse(text)
            .map_err(|error| format!("invalid {}: {error}", path.display()))?;
        return native_cart(path, bundle.cart_text());
    }
    if has_extension(path, &["cart"]) {
        return native_cart(path, text.to_string());
    }
    score_from_abc(text).map(PlayInput::Source)
}

fn native_cart(path: &Path, cart_text: String) -> Result<PlayInput, String> {
    let cart = Cart::parse(&cart_text)
        .map_err(|error| format!("invalid native music input {}: {error}", path.display()))?;
    Ok(PlayInput::Native {
        label: path.display().to_string(),
        cart: Box::new(cart),
    })
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

#[derive(Debug)]
struct PlayArgs {
    input: PathBuf,
    song: Option<u8>,
    seconds: Option<f64>,
    volume: f32,
    repeat: bool,
    dry_run: bool,
}

impl PlayArgs {
    fn parse(args: &[String]) -> Result<PlayArgs, String> {
        let mut input = None;
        let mut song = None;
        let mut seconds = None;
        let mut volume = DEFAULT_PLAYBACK_VOLUME;
        let mut repeat = false;
        let mut dry_run = false;
        let mut index = 0usize;
        while index < args.len() {
            match args[index].as_str() {
                "--dry-run" => dry_run = true,
                "--repeat" => repeat = true,
                "--song" => {
                    index += 1;
                    let value = args.get(index).ok_or("--song requires a value")?;
                    let parsed = value
                        .parse::<u8>()
                        .map_err(|_| format!("invalid --song value {value:?} (want 0-63)"))?;
                    if parsed > 63 {
                        return Err("--song must be from 0 to 63".to_string());
                    }
                    song = Some(parsed);
                }
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
                        return Err("expected exactly one music input".to_string());
                    }
                }
            }
            index += 1;
        }
        Ok(PlayArgs {
            input: input.ok_or("missing music input")?,
            song,
            seconds,
            volume,
            repeat,
            dry_run,
        })
    }
}

const PLAY_USAGE: &str = "usage: console music play <file.abc|file.mid|file.cmusic|file.cart|project> \
    [--song N] [--seconds N] [--volume 0..1] [--repeat] [--dry-run]";

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
    match read_play_input(&args.input)? {
        PlayInput::Source(score) => run_source_play(score, &args),
        PlayInput::Native { label, cart } => run_native_play(&label, &cart, &args),
    }
}

fn run_source_play(score: TimedScore, args: &PlayArgs) -> Result<(), String> {
    if args.song.is_some() {
        return Err("--song is only valid for .cmusic, .cart, or project inputs".to_string());
    }
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

fn run_native_play(label: &str, cart: &Cart, args: &PlayArgs) -> Result<(), String> {
    let song = match args.song {
        Some(song) => song,
        None => super::default_song(cart)?,
    };
    let plan = super::plan_song(cart, song)?;
    let frames = args
        .seconds
        .map(|seconds| (seconds * super::FPS).ceil() as u64);
    // The plan gives us the exact duration to report. Do not render a whole
    // pass merely to learn it: device playback below is deliberately streamed
    // through a bounded queue, and a legal large song or --seconds value must
    // not first allocate an unbounded PCM cache.
    let report_frames = frames.unwrap_or_else(|| plan.frames_for(1));

    let authored_loop = args.seconds.is_none() && plan.loop_index.is_some();
    let mode = match (args.dry_run, args.repeat, authored_loop) {
        (true, true, _) => " (native, dry run, repeat)",
        (true, false, true) => " (native, dry run, authored loop)",
        (true, false, false) => " (native, dry run)",
        (false, _, true) => " (native authored loop; Ctrl-C to stop)",
        (false, true, false) => " (native repeat; Ctrl-C to stop)",
        (false, false, false) => " (native)",
    };
    eprintln!(
        "{label}: {:.2}s, song {song}, {}, volume {:.2}{mode}",
        super::seconds(report_frames),
        plan.chain_text(),
        args.volume
    );
    if !args.dry_run {
        let runtime = super::audio_only_cart(cart, &format!("function _init() music({song}) end"));
        let infinite = args.repeat || authored_loop;
        let deadline_seconds =
            (!infinite).then_some(report_frames as f64 / super::FPS + 1.0 / super::FPS + 5.0);
        play_native_runtime(runtime, frames, args.repeat, args.volume, deadline_seconds)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeRenderPhase {
    Playing,
    Release,
    Restart,
    Finished,
}

/// Stateful native renderer used by host playback. It deliberately retains
/// one `Console` across authored pattern loops: oscillator, filter, hiss, and
/// echo state must evolve exactly as they do in the game.
struct NativeFrameRenderer {
    runtime: String,
    console: Console,
    clip_frames: Option<u64>,
    repeat: bool,
    frames: u64,
    phase: NativeRenderPhase,
}

impl NativeFrameRenderer {
    fn new(runtime: String, clip_frames: Option<u64>, repeat: bool) -> Result<Self, String> {
        let console = Console::new(&runtime, 0).map_err(|error| error.to_string())?;
        Ok(Self {
            runtime,
            console,
            clip_frames,
            repeat,
            frames: 0,
            phase: NativeRenderPhase::Playing,
        })
    }

    fn next_frame(&mut self) -> Result<Option<Box<[f32]>>, String> {
        loop {
            match self.phase {
                NativeRenderPhase::Finished => return Ok(None),
                NativeRenderPhase::Restart => {
                    self.console =
                        Console::new(&self.runtime, 0).map_err(|error| error.to_string())?;
                    self.frames = 0;
                    self.phase = NativeRenderPhase::Playing;
                }
                NativeRenderPhase::Release => {
                    self.console.step(0).map_err(|error| error.to_string())?;
                    let mut frame = self.console.audio_frame().to_vec().into_boxed_slice();
                    // The core click guard silences dry voices, but post-voice
                    // effects such as echo can still leave a non-zero final
                    // sample. Taper the end of the drained frame so stopping
                    // or restarting always has a genuinely silent seam.
                    fade_tail(&mut frame);
                    self.phase = if self.repeat {
                        NativeRenderPhase::Restart
                    } else {
                        NativeRenderPhase::Finished
                    };
                    return Ok(Some(frame));
                }
                NativeRenderPhase::Playing => {
                    self.console.step(0).map_err(|error| error.to_string())?;
                    self.frames += 1;
                    let mut frame = self.console.audio_frame().to_vec().into_boxed_slice();
                    if self.clip_frames.is_some_and(|limit| self.frames >= limit) {
                        fade_tail(&mut frame);
                        self.phase = if self.repeat {
                            NativeRenderPhase::Restart
                        } else {
                            NativeRenderPhase::Finished
                        };
                    } else if self.console.music_pattern().is_none() {
                        // Sequencer stop arms the core's 64-sample click guard
                        // after this frame was rendered. Drain one more frame
                        // before ending or restarting the song.
                        self.phase = NativeRenderPhase::Release;
                    }
                    return Ok(Some(frame));
                }
            }
        }
    }
}

fn fade_tail(samples: &mut [f32]) {
    let count = samples.len().min(CLICK_RAMP_SAMPLES);
    if count < 2 {
        samples.fill(0.0);
        return;
    }
    let start = samples.len() - count;
    let denominator = (count - 1) as f32;
    for (index, sample) in samples[start..].iter_mut().enumerate() {
        *sample *= (count - 1 - index) as f32 / denominator;
    }
}

#[derive(Default)]
struct NativeBuffer {
    frames: VecDeque<Box<[f32]>>,
    finished: bool,
    stop: bool,
    error: Option<String>,
}

type SharedNativeBuffer = Arc<(Mutex<NativeBuffer>, Condvar)>;

fn play_native_runtime(
    runtime: String,
    clip_frames: Option<u64>,
    repeat: bool,
    volume: f32,
    deadline_seconds: Option<f64>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no default audio output device is available")?;
    let supported = device
        .default_output_config()
        .map_err(|error| format!("cannot query the default audio output: {error}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let shared = Arc::new((Mutex::new(NativeBuffer::default()), Condvar::new()));
    let worker = spawn_native_renderer(runtime, clip_frames, repeat, volume, Arc::clone(&shared));

    let result = wait_for_native_prefill(&shared).and_then(|()| match format {
        SampleFormat::I8 => {
            play_native_typed::<i8>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::I16 => {
            play_native_typed::<i16>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::I24 => {
            play_native_typed::<I24>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::I32 => {
            play_native_typed::<i32>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::I64 => {
            play_native_typed::<i64>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::U8 => {
            play_native_typed::<u8>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::U16 => {
            play_native_typed::<u16>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::U24 => {
            play_native_typed::<U24>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::U32 => {
            play_native_typed::<u32>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::U64 => {
            play_native_typed::<u64>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::F32 => {
            play_native_typed::<f32>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        SampleFormat::F64 => {
            play_native_typed::<f64>(&device, config, Arc::clone(&shared), deadline_seconds)
        }
        other => Err(format!("unsupported output sample format {other}")),
    });
    stop_native_renderer(&shared);
    let joined = worker
        .join()
        .map_err(|_| "native audio renderer panicked".to_string());
    result.and(joined)
}

fn spawn_native_renderer(
    runtime: String,
    clip_frames: Option<u64>,
    repeat: bool,
    volume: f32,
    shared: SharedNativeBuffer,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let mut renderer = NativeFrameRenderer::new(runtime, clip_frames, repeat)?;
            while let Some(mut frame) = renderer.next_frame()? {
                apply_playback_volume(&mut frame, volume);
                let (lock, ready) = &*shared;
                let mut state = lock
                    .lock()
                    .map_err(|_| "native audio buffer was poisoned".to_string())?;
                while state.frames.len() >= NATIVE_BUFFER_FRAMES && !state.stop {
                    state = ready
                        .wait(state)
                        .map_err(|_| "native audio buffer was poisoned".to_string())?;
                }
                if state.stop {
                    return Ok(());
                }
                state.frames.push_back(frame);
                ready.notify_all();
            }
            Ok(())
        })();

        let (lock, ready) = &*shared;
        if let Ok(mut state) = lock.lock() {
            if let Err(error) = result {
                state.error = Some(error);
            }
            state.finished = true;
            ready.notify_all();
        }
    })
}

fn stop_native_renderer(shared: &SharedNativeBuffer) {
    let (lock, ready) = &**shared;
    if let Ok(mut state) = lock.lock() {
        state.stop = true;
        ready.notify_all();
    }
}

fn wait_for_native_prefill(shared: &SharedNativeBuffer) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let (lock, ready) = &**shared;
    let mut state = lock
        .lock()
        .map_err(|_| "native audio buffer was poisoned".to_string())?;
    while state.frames.len() < NATIVE_PREFILL_FRAMES && !state.finished && state.error.is_none() {
        let now = Instant::now();
        if now >= deadline {
            return Err("native audio renderer did not produce samples before its deadline".into());
        }
        let (next, timeout) = ready
            .wait_timeout(state, deadline - now)
            .map_err(|_| "native audio buffer was poisoned".to_string())?;
        state = next;
        if timeout.timed_out() && state.frames.len() < NATIVE_PREFILL_FRAMES && !state.finished {
            return Err("native audio renderer did not produce samples before its deadline".into());
        }
    }
    if let Some(error) = state.error.take() {
        return Err(error);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum StreamSample {
    Sample(f32),
    Pending,
    Finished,
}

struct NativeSampleCursor {
    shared: SharedNativeBuffer,
    frame: Option<Box<[f32]>>,
    frame_index: usize,
    a: Option<f32>,
    b: Option<f32>,
    fraction: f64,
    step: f64,
    end_after_b: bool,
    finished: bool,
}

impl NativeSampleCursor {
    fn new(step: f64, shared: SharedNativeBuffer) -> Self {
        Self {
            shared,
            frame: None,
            frame_index: 0,
            a: None,
            b: None,
            fraction: 0.0,
            step,
            end_after_b: false,
            finished: false,
        }
    }

    fn pull(&mut self) -> StreamSample {
        loop {
            if let Some(frame) = &self.frame {
                if let Some(sample) = frame.get(self.frame_index) {
                    self.frame_index += 1;
                    return StreamSample::Sample(*sample);
                }
            }
            let (lock, ready) = &*self.shared;
            let mut state = match lock.lock() {
                Ok(state) => state,
                Err(_) => return StreamSample::Finished,
            };
            if let Some(frame) = state.frames.pop_front() {
                self.frame = Some(frame);
                self.frame_index = 0;
                ready.notify_all();
                continue;
            }
            return if state.finished {
                StreamSample::Finished
            } else {
                StreamSample::Pending
            };
        }
    }

    fn next(&mut self) -> StreamSample {
        if self.finished {
            return StreamSample::Finished;
        }
        if self.a.is_none() {
            match self.pull() {
                StreamSample::Sample(sample) => self.a = Some(sample),
                other => return other,
            }
        }
        if self.b.is_none() {
            match self.pull() {
                StreamSample::Sample(sample) => self.b = Some(sample),
                StreamSample::Pending => return StreamSample::Pending,
                StreamSample::Finished => {
                    self.b = self.a;
                    self.end_after_b = true;
                }
            }
        }

        let a = self.a.expect("filled above");
        let b = self.b.expect("filled above");
        let value = a + (b - a) * self.fraction as f32;
        self.fraction += self.step;
        while self.fraction >= 1.0 {
            self.fraction -= 1.0;
            if self.end_after_b {
                self.finished = true;
                break;
            }
            self.a = self.b.take();
            match self.pull() {
                StreamSample::Sample(sample) => self.b = Some(sample),
                StreamSample::Pending => break,
                StreamSample::Finished => {
                    self.b = self.a;
                    self.end_after_b = true;
                }
            }
        }
        StreamSample::Sample(value)
    }
}

fn play_native_typed<T>(
    device: &Device,
    config: StreamConfig,
    shared: SharedNativeBuffer,
    deadline_seconds: Option<f64>,
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
    let deadline =
        deadline_seconds.map(|seconds| Instant::now() + Duration::from_secs_f64(seconds));
    let done = Arc::new(AtomicBool::new(false));
    let callback_done = Arc::clone(&done);
    let error = Arc::new(Mutex::new(None::<String>));
    let callback_error = Arc::clone(&error);
    let mut cursor = NativeSampleCursor::new(step, Arc::clone(&shared));
    let stream = device
        .build_output_stream(
            config,
            move |output: &mut [T], _: &OutputCallbackInfo| {
                for frame in output.chunks_mut(channels) {
                    let value = match cursor.next() {
                        StreamSample::Sample(value) => value,
                        StreamSample::Pending => 0.0,
                        StreamSample::Finished => {
                            callback_done.store(true, Ordering::Release);
                            0.0
                        }
                    };
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
        native_render_status(&shared)?;
        if playback_status(&done, &error)? {
            break;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err("audio output did not consume native samples before its deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(150));
    native_render_status(&shared)?;
    playback_status(&done, &error)?;
    Ok(())
}

fn native_render_status(shared: &SharedNativeBuffer) -> Result<(), String> {
    let state = shared
        .0
        .lock()
        .map_err(|_| "native audio buffer was poisoned".to_string())?;
    match &state.error {
        Some(error) => Err(format!("native audio renderer failed: {error}")),
        None => Ok(()),
    }
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

    fn native_example() -> (Cart, String) {
        let bundle = super::super::native::NativeMusic::parse(include_str!(
            "../../../../examples/native-music/audio/game.cmusic"
        ))
        .unwrap();
        let text = bundle.cart_text();
        let cart = Cart::parse(&text).unwrap();
        (cart, text)
    }

    #[test]
    fn native_renderer_matches_two_continuous_runtime_loop_passes() {
        let (cart, _) = native_example();
        let plan = super::super::plan_song(&cart, 0).unwrap();
        let runtime = super::super::audio_only_cart(&cart, "function _init() music(0) end");
        let mut renderer = NativeFrameRenderer::new(runtime, None, false).unwrap();
        let frame_count = plan.frames_for(2);
        let mut streamed = Vec::new();
        for _ in 0..frame_count {
            streamed.extend_from_slice(&renderer.next_frame().unwrap().unwrap());
        }

        let isolated = super::super::audio_only_cart(&cart, "");
        let (expected, report) = super::super::render::render_song(
            &isolated,
            &super::super::render::RenderOpts {
                song: Some(0),
                loops: 2,
                frames: None,
                seed: 0,
            },
        )
        .unwrap();
        assert_eq!(report.frames, frame_count);
        assert_eq!(streamed, expected);

        let intro = plan.intro_frames() as usize * SAMPLES_PER_FRAME;
        let loop_samples = plan.loop_frames() as usize * SAMPLES_PER_FRAME;
        assert_ne!(
            &streamed[intro..intro + loop_samples],
            &streamed[intro + loop_samples..intro + loop_samples * 2],
            "stateful echo/filter/oscillator state should make real loop passes differ"
        );
    }

    #[test]
    fn native_one_shots_drain_release_and_clipped_prefixes_fade_to_zero() {
        let bundle = super::super::native::NativeMusic::parse(
            "console-music 1\n\
             __instruments__\n\
             echo delay=1 feedback=8 level=8\n\
             inst wet wave=2 echo=8\n\
             __sfx__\n\
             sfx 0 speed=1\n\
             C4 wet 7\n\
             __music__\n\
             bpm=120 rows_per_beat=4\n\
             pat 0 stop : 0 - - -\n",
        )
        .unwrap();
        let cart = Cart::parse(&bundle.cart_text()).unwrap();
        let runtime = super::super::audio_only_cart(&cart, "function _init() music(0) end");

        let mut once = NativeFrameRenderer::new(runtime.clone(), None, false).unwrap();
        let body = once.next_frame().unwrap().unwrap();
        let release = once.next_frame().unwrap().unwrap();
        assert!(body.iter().any(|sample| *sample != 0.0));
        assert!(
            release[..CLICK_RAMP_SAMPLES]
                .iter()
                .any(|sample| *sample != 0.0)
        );
        assert!(
            release[CLICK_RAMP_SAMPLES..release.len() - CLICK_RAMP_SAMPLES]
                .iter()
                .any(|sample| *sample != 0.0),
            "echo should outlive the core voice release"
        );
        assert_eq!(release.last(), Some(&0.0));
        assert!(once.next_frame().unwrap().is_none());

        let mut repeated = NativeFrameRenderer::new(runtime.clone(), None, true).unwrap();
        let first = repeated.next_frame().unwrap().unwrap();
        let seam = repeated.next_frame().unwrap().unwrap();
        let restarted = repeated.next_frame().unwrap().unwrap();
        assert_eq!(first, restarted);
        assert_eq!(seam.last(), Some(&0.0));

        let mut clipped = NativeFrameRenderer::new(runtime, Some(1), true).unwrap();
        let first = clipped.next_frame().unwrap().unwrap();
        let restarted = clipped.next_frame().unwrap().unwrap();
        assert_eq!(first, restarted);
        assert_eq!(first.last(), Some(&0.0));
    }

    #[test]
    fn native_stream_cursor_consumes_frame_queue_without_pcm_looping() {
        let shared = Arc::new((Mutex::new(NativeBuffer::default()), Condvar::new()));
        {
            let mut state = shared.0.lock().unwrap();
            state.frames.push_back(Box::from([1.0, 2.0]));
            state.frames.push_back(Box::from([3.0]));
            state.finished = true;
        }
        let mut cursor = NativeSampleCursor::new(1.0, shared);
        assert_eq!(cursor.next(), StreamSample::Sample(1.0));
        assert_eq!(cursor.next(), StreamSample::Sample(2.0));
        assert_eq!(cursor.next(), StreamSample::Sample(3.0));
        assert_eq!(cursor.next(), StreamSample::Finished);
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

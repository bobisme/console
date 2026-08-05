//! Bounded Standard MIDI parsing and `console music midi-to-abc`.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use midly::{Format, MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};

pub const MAX_MIDI_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_MIDI_EVENTS: usize = 1_000_000;
pub const MAX_MIDI_NOTES: usize = 250_000;
pub const MAX_MIDI_TICKS: u64 = 1u64 << 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TempoChange {
    pub tick: u64,
    pub micros_per_quarter: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiNote {
    pub id: u64,
    pub start_tick: u64,
    pub end_tick: u64,
    pub key: u8,
    pub velocity: u8,
    pub program: u8,
    pub channel: u8,
    pub track: usize,
}

#[derive(Debug, Clone)]
pub struct MidiSong {
    pub title: Option<String>,
    pub ticks_per_quarter: u16,
    pub meter: (u8, u8),
    pub tempos: Vec<TempoChange>,
    pub notes: Vec<MidiNote>,
    pub track_names: Vec<Option<String>>,
    pub duration_ticks: u64,
    pub warnings: Vec<String>,
    /// Elapsed `(ticks * microseconds-per-quarter)` at each matching tempo
    /// point. Timing lookups binary-search `tempos` and add one final segment.
    tempo_microticks: Vec<u128>,
}

impl MidiSong {
    /// Convert an absolute MIDI tick to the nearest 60 Hz console frame while
    /// preserving every tempo segment encountered before it.
    pub fn tick_to_frame(&self, tick: u64) -> u64 {
        let index = self
            .tempos
            .partition_point(|change| change.tick <= tick)
            .saturating_sub(1);
        let change = self.tempos[index];
        let microtick_numerator = self.tempo_microticks[index]
            + u128::from(tick - change.tick) * u128::from(change.micros_per_quarter);
        let denominator = u128::from(self.ticks_per_quarter) * 1_000_000;
        ((microtick_numerator * 60 + denominator / 2) / denominator).min(u128::from(u64::MAX))
            as u64
    }
}

#[derive(Debug, Clone)]
struct RawEvent<'a> {
    tick: u64,
    track: usize,
    order: usize,
    kind: TrackEventKind<'a>,
}

#[derive(Debug, Clone, Copy)]
struct ActiveNote {
    id: u64,
    start_tick: u64,
    velocity: u8,
    program: u8,
    track: usize,
}

pub fn read_midi(path: &Path) -> Result<MidiSong, String> {
    let bytes = super::read_bounded(path, "MIDI")?;
    parse_midi(&bytes)
}

pub fn parse_midi(bytes: &[u8]) -> Result<MidiSong, String> {
    if bytes.len() > MAX_MIDI_BYTES {
        return Err(format!(
            "MIDI input is {} bytes; limit is {} bytes",
            bytes.len(),
            MAX_MIDI_BYTES
        ));
    }
    let smf = Smf::parse(bytes).map_err(|error| format!("invalid Standard MIDI file: {error}"))?;
    if smf.header.format == Format::Sequential {
        return Err(
            "MIDI format 2 contains independent songs; split it into format 0 or 1 files first"
                .to_string(),
        );
    }
    let ticks_per_quarter = match smf.header.timing {
        Timing::Metrical(ticks) => ticks.as_int(),
        Timing::Timecode(_, _) => {
            return Err(
                "SMPTE/timecode MIDI timing is not supported; use metrical PPQ timing".to_string(),
            );
        }
    };
    if ticks_per_quarter == 0 {
        return Err(
            "MIDI PPQ division is zero; expected at least one tick per quarter".to_string(),
        );
    }

    let event_count: usize = smf.tracks.iter().map(Vec::len).sum();
    if event_count > MAX_MIDI_EVENTS {
        return Err(format!(
            "MIDI contains {event_count} events; limit is {MAX_MIDI_EVENTS}"
        ));
    }

    let mut raw = Vec::with_capacity(event_count);
    let mut track_names = vec![None; smf.tracks.len()];
    let mut title = None;
    let mut duration_ticks = 0u64;
    for (track_index, track) in smf.tracks.iter().enumerate() {
        let mut tick = 0u64;
        for (order, event) in track.iter().enumerate() {
            tick = tick
                .checked_add(u64::from(event.delta.as_int()))
                .ok_or_else(|| "MIDI absolute tick overflow".to_string())?;
            if tick > MAX_MIDI_TICKS {
                return Err(format!(
                    "MIDI duration exceeds the {MAX_MIDI_TICKS}-tick safety limit"
                ));
            }
            if let TrackEventKind::Meta(MetaMessage::TrackName(name)) = event.kind {
                let name = String::from_utf8_lossy(name).trim().to_string();
                if !name.is_empty() {
                    if title.is_none() {
                        title = Some(name.clone());
                    }
                    track_names[track_index] = Some(name);
                }
            }
            raw.push(RawEvent {
                tick,
                track: track_index,
                order,
                kind: event.kind,
            });
        }
        duration_ticks = duration_ticks.max(tick);
    }
    raw.sort_by_key(|event| (event.tick, event.track, event.order));

    let mut programs = [0u8; 16];
    let mut active: HashMap<(u8, u8), VecDeque<ActiveNote>> = HashMap::new();
    let mut notes = Vec::new();
    let mut tempos = vec![TempoChange {
        tick: 0,
        micros_per_quarter: 500_000,
    }];
    let mut meter = (4, 4);
    let mut warnings = Vec::new();
    let mut next_id = 0u64;
    let mut sustain_seen = false;

    for event in raw {
        match event.kind {
            TrackEventKind::Meta(MetaMessage::Tempo(value)) => {
                let value = value.as_int();
                if value == 0 {
                    warnings.push(format!(
                        "ignored zero-length tempo event at tick {}",
                        event.tick
                    ));
                } else if let Some(last) = tempos.last_mut().filter(|t| t.tick == event.tick) {
                    last.micros_per_quarter = value;
                } else {
                    tempos.push(TempoChange {
                        tick: event.tick,
                        micros_per_quarter: value,
                    });
                }
            }
            TrackEventKind::Meta(MetaMessage::TimeSignature(num, pow, _, _)) if event.tick == 0 => {
                meter = (num, 1u8.checked_shl(u32::from(pow)).unwrap_or(4));
            }
            TrackEventKind::Midi { channel, message } => {
                let channel = channel.as_int();
                match message {
                    MidiMessage::ProgramChange { program } => {
                        programs[usize::from(channel)] = program.as_int();
                    }
                    MidiMessage::Controller { controller, value }
                        if controller.as_int() == 64 && value.as_int() >= 64 =>
                    {
                        sustain_seen = true;
                    }
                    MidiMessage::NoteOn { key, vel } if vel.as_int() != 0 => {
                        if next_id >= MAX_MIDI_NOTES as u64 {
                            return Err(format!("MIDI contains more than {MAX_MIDI_NOTES} notes"));
                        }
                        let key = key.as_int();
                        active
                            .entry((channel, key))
                            .or_default()
                            .push_back(ActiveNote {
                                id: next_id,
                                start_tick: event.tick,
                                velocity: vel.as_int(),
                                program: programs[usize::from(channel)],
                                track: event.track,
                            });
                        next_id += 1;
                    }
                    MidiMessage::NoteOff { key, .. } | MidiMessage::NoteOn { key, vel: _ } => {
                        close_note(&mut active, &mut notes, channel, key.as_int(), event.tick);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let mut hanging = 0usize;
    for ((channel, key), queue) in active {
        for note in queue {
            hanging += 1;
            notes.push(MidiNote {
                id: note.id,
                start_tick: note.start_tick,
                end_tick: duration_ticks.max(note.start_tick + 1),
                key,
                velocity: note.velocity,
                program: note.program,
                channel,
                track: note.track,
            });
        }
    }
    if hanging != 0 {
        warnings.push(format!(
            "closed {hanging} note(s) without note-off at the end of the file"
        ));
    }
    if sustain_seen {
        warnings.push(
            "sustain-pedal events are not modeled; note-off timing is used as written".to_string(),
        );
    }
    notes.sort_by_key(|note| (note.start_tick, note.track, note.channel, note.key, note.id));
    if notes.is_empty() {
        return Err("the MIDI file contains no notes".to_string());
    }

    let tempo_microticks = tempo_prefix(&tempos);
    Ok(MidiSong {
        title,
        ticks_per_quarter,
        meter,
        tempos,
        notes,
        track_names,
        duration_ticks,
        warnings,
        tempo_microticks,
    })
}

fn tempo_prefix(tempos: &[TempoChange]) -> Vec<u128> {
    let mut prefix = Vec::with_capacity(tempos.len());
    let mut elapsed = 0u128;
    let mut cursor = 0u64;
    let mut tempo = tempos[0].micros_per_quarter;
    for change in tempos {
        elapsed += u128::from(change.tick - cursor) * u128::from(tempo);
        prefix.push(elapsed);
        cursor = change.tick;
        tempo = change.micros_per_quarter;
    }
    prefix
}

fn close_note(
    active: &mut HashMap<(u8, u8), VecDeque<ActiveNote>>,
    notes: &mut Vec<MidiNote>,
    channel: u8,
    key: u8,
    tick: u64,
) {
    let Some(queue) = active.get_mut(&(channel, key)) else {
        return;
    };
    let Some(note) = queue.pop_front() else {
        return;
    };
    notes.push(MidiNote {
        id: note.id,
        start_tick: note.start_tick,
        end_tick: tick.max(note.start_tick + 1),
        key,
        velocity: note.velocity,
        program: note.program,
        channel,
        track: note.track,
    });
}

#[derive(Debug)]
struct Lane<'a> {
    track: usize,
    channel: u8,
    notes: Vec<&'a MidiNote>,
    end_tick: u64,
}

pub fn midi_to_abc(song: &MidiSong) -> (String, Vec<String>) {
    let lanes = split_lanes(song);
    let mut warnings = song.warnings.clone();
    if song.tempos.len() > 1 {
        warnings.push(format!(
            "ABC output uses the initial tempo; {} later tempo change(s) remain exact only when playing the MIDI source",
            song.tempos.len() - 1
        ));
    }
    let initial_tempo = song.tempos[0].micros_per_quarter.max(1);
    let bpm = (60_000_000u32 + initial_tempo / 2) / initial_tempo;
    if 60_000_000u32 % initial_tempo != 0 {
        warnings.push(format!(
            "initial MIDI tempo {initial_tempo} us/quarter is not an integer BPM; ABC Q: rounds it to {bpm} BPM (direct MIDI playback remains exact)"
        ));
    }
    let title = song
        .title
        .as_deref()
        .map(clean_header)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "MIDI import".to_string());
    let mut out = format!(
        "X:1\nT:{title}\nM:{}/{}\nL:1/{}\nQ:1/4={bpm}\nK:C\n",
        song.meter.0,
        song.meter.1,
        u32::from(song.ticks_per_quarter) * 4
    );

    for (index, lane) in lanes.iter().enumerate() {
        let track_name = song
            .track_names
            .get(lane.track)
            .and_then(|name| name.as_deref())
            .unwrap_or("track");
        out.push_str(&format!(
            "V:{} name=\"{} ch{} lane{}\"\n",
            index + 1,
            clean_header(track_name).replace('"', "'"),
            lane.channel + 1,
            index + 1
        ));
        let mut cursor = 0u64;
        let mut column = 0usize;
        for note in &lane.notes {
            if note.start_tick > cursor {
                push_token(
                    &mut out,
                    &format!("z{}", note.start_tick - cursor),
                    &mut column,
                );
            }
            let token = format!("{}{}", abc_pitch(note.key), note.end_tick - note.start_tick);
            push_token(&mut out, &token, &mut column);
            cursor = note.end_tick;
        }
        if column != 0 {
            out.push('\n');
        }
    }
    (out, warnings)
}

fn split_lanes(song: &MidiSong) -> Vec<Lane<'_>> {
    let mut lanes = Vec::<Lane<'_>>::new();
    let mut grouped = BTreeMap::<(usize, u8), Vec<&MidiNote>>::new();
    for note in &song.notes {
        grouped
            .entry((note.track, note.channel))
            .or_default()
            .push(note);
    }
    for ((track, channel), notes) in grouped {
        let mut group = Vec::<Lane<'_>>::new();
        let mut busy = BinaryHeap::<Reverse<(u64, usize)>>::new();
        let mut available = BinaryHeap::<Reverse<usize>>::new();
        for note in notes {
            while let Some(Reverse((end_tick, lane))) = busy.peek().copied() {
                if end_tick > note.start_tick {
                    break;
                }
                busy.pop();
                available.push(Reverse(lane));
            }
            let lane = if let Some(Reverse(lane)) = available.pop() {
                group[lane].end_tick = note.end_tick;
                group[lane].notes.push(note);
                lane
            } else {
                group.push(Lane {
                    track,
                    channel,
                    notes: vec![note],
                    end_tick: note.end_tick,
                });
                group.len() - 1
            };
            busy.push(Reverse((note.end_tick, lane)));
        }
        lanes.extend(group);
    }
    lanes
}

fn clean_header(text: &str) -> String {
    text.chars()
        .filter(|ch| !matches!(ch, '\r' | '\n' | '\0'))
        .collect::<String>()
        .trim()
        .to_string()
}

fn push_token(out: &mut String, token: &str, column: &mut usize) {
    if *column != 0 && *column + token.len() + 1 > 96 {
        out.push('\n');
        *column = 0;
    }
    if *column != 0 {
        out.push(' ');
        *column += 1;
    }
    out.push_str(token);
    *column += token.len();
}

fn abc_pitch(key: u8) -> String {
    const PITCH: [(&str, char); 12] = [
        ("=", 'C'),
        ("^", 'C'),
        ("=", 'D'),
        ("^", 'D'),
        ("=", 'E'),
        ("=", 'F'),
        ("^", 'F'),
        ("=", 'G'),
        ("^", 'G'),
        ("=", 'A'),
        ("^", 'A'),
        ("=", 'B'),
    ];
    let octave = i32::from(key) / 12 - 1;
    let (accidental, letter) = PITCH[usize::from(key % 12)];
    let mut out = accidental.to_string();
    if octave >= 5 {
        out.push(letter.to_ascii_lowercase());
        for _ in 0..octave - 5 {
            out.push('\'');
        }
    } else {
        out.push(letter);
        for _ in 0..4 - octave {
            out.push(',');
        }
    }
    out
}

const MIDI_TO_ABC_USAGE: &str =
    "usage: console music midi-to-abc <in-file.mid> [--out <file.abc> | -o <file.abc>]";

pub fn cli_midi_to_abc(args: &[String]) -> i32 {
    if crate::help_requested(args) {
        println!("{MIDI_TO_ABC_USAGE}");
        return 0;
    }
    let (input, output) = match parse_midi_to_abc_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("{MIDI_TO_ABC_USAGE}");
            return 2;
        }
    };
    match run_midi_to_abc(&input, output.as_deref()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn parse_midi_to_abc_args(args: &[String]) -> Result<(PathBuf, Option<PathBuf>), String> {
    let mut input = None::<PathBuf>;
    let mut output = None::<PathBuf>;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--out" => {
                index += 1;
                let value = args.get(index).ok_or("--out requires a path")?;
                if output.replace(PathBuf::from(value)).is_some() {
                    return Err("--out may only be specified once".to_string());
                }
            }
            value if value.starts_with('-') => return Err(format!("unknown flag {value:?}")),
            value => {
                if input.replace(PathBuf::from(value)).is_some() {
                    return Err("expected exactly one MIDI input file".to_string());
                }
            }
        }
        index += 1;
    }
    Ok((input.ok_or("missing MIDI input file")?, output))
}

fn run_midi_to_abc(input: &Path, output: Option<&Path>) -> Result<(), String> {
    let song = read_midi(input)?;
    let (abc, warnings) = midi_to_abc(&song);
    if let Some(output) = output {
        atomic_write(output, abc.as_bytes())?;
        eprintln!(
            "wrote {} voices / {} notes to {}",
            abc.lines().filter(|line| line.starts_with("V:")).count(),
            song.notes.len(),
            output.display()
        );
    } else {
        print!("{abc}");
    }
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("output {} has no UTF-8 file name", path.display()))?;
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let mut collision = None;
    for _ in 0..100 {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(".{name}.{}.{serial}.tmp", std::process::id()));
        let mut file = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                collision = Some(error);
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "cannot create temporary {}: {error}",
                    temp.display()
                ));
            }
        };
        let result = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot write temporary {}: {error}", temp.display()))
            .and_then(|()| {
                drop(file);
                std::fs::rename(&temp, path)
                    .map_err(|error| format!("cannot replace {}: {error}", path.display()))
            });
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        return result;
    }
    Err(format!(
        "cannot allocate a temporary file beside {}: {}",
        path.display(),
        collision.map_or_else(|| "unknown error".to_string(), |error| error.to_string())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_midi() -> Vec<u8> {
        vec![
            b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0, 96, b'M', b'T', b'r', b'k', 0, 0, 0,
            20, 0, 0x90, 60, 100, 96, 0x80, 60, 0, 0, 0x90, 64, 80, 96, 0x80, 64, 0, 0, 0xff, 0x2f,
            0,
        ]
    }

    #[test]
    fn parses_notes_and_converts_pipeable_abc() {
        let song = parse_midi(&simple_midi()).unwrap();
        assert_eq!(song.ticks_per_quarter, 96);
        assert_eq!(song.notes.len(), 2);
        assert_eq!(song.notes[0].start_tick, 0);
        assert_eq!(song.notes[0].end_tick, 96);
        assert_eq!(song.tick_to_frame(96), 30);

        let (abc, warnings) = midi_to_abc(&song);
        assert!(warnings.is_empty());
        assert!(abc.contains("L:1/384"));
        assert!(abc.contains("=C96 =E96"));
        assert_eq!(abc.lines().filter(|line| line.starts_with("V:")).count(), 1);
    }

    #[test]
    fn overlapping_notes_get_independent_voices() {
        let mut bytes = simple_midi();
        bytes.splice(
            22..,
            [
                0, 0x90, 60, 100, 0, 0x90, 64, 100, 96, 0x80, 60, 0, 0, 0x80, 64, 0, 0, 0xff, 0x2f,
                0,
            ],
        );
        bytes[18..22].copy_from_slice(&20u32.to_be_bytes());
        let song = parse_midi(&bytes).unwrap();
        let (abc, _) = midi_to_abc(&song);
        assert_eq!(abc.lines().filter(|line| line.starts_with("V:")).count(), 2);
    }

    #[test]
    fn tempo_map_integrates_each_segment() {
        let bytes = vec![
            b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0, 96, b'M', b'T', b'r', b'k', 0, 0, 0,
            26, 0, 0xff, 0x51, 3, 0x07, 0xa1, 0x20, 0, 0x90, 60, 100, 96, 0xff, 0x51, 3, 0x0f,
            0x42, 0x40, 96, 0x80, 60, 0, 0, 0xff, 0x2f, 0,
        ];
        let song = parse_midi(&bytes).unwrap();
        assert_eq!(song.tempos.len(), 2);
        assert_eq!(song.tick_to_frame(96), 30);
        assert_eq!(song.tick_to_frame(192), 90);
        assert_eq!(song.notes[0].end_tick, 192);
    }

    #[test]
    fn zero_ppq_is_rejected_before_timing_math() {
        let mut bytes = simple_midi();
        bytes[12] = 0;
        bytes[13] = 0;
        assert!(
            parse_midi(&bytes)
                .unwrap_err()
                .contains("PPQ division is zero")
        );
    }

    #[test]
    fn large_tempo_index_and_polyphony_stay_subquadratic() {
        let tempos: Vec<TempoChange> = (0..10_000)
            .map(|tick| TempoChange {
                tick,
                micros_per_quarter: 400_000 + tick as u32 % 200_000,
            })
            .collect();
        let notes: Vec<MidiNote> = (0..20_000)
            .map(|id| MidiNote {
                id,
                start_tick: 0,
                end_tick: 10_000,
                key: 60,
                velocity: 100,
                program: 0,
                channel: 0,
                track: 0,
            })
            .collect();
        let song = MidiSong {
            title: None,
            ticks_per_quarter: 96,
            meter: (4, 4),
            tempo_microticks: tempo_prefix(&tempos),
            tempos,
            notes,
            track_names: vec![None],
            duration_ticks: 10_000,
            warnings: Vec::new(),
        };

        let mut previous = 0;
        for tick in 0..10_000 {
            let frame = song.tick_to_frame(tick);
            assert!(frame >= previous);
            previous = frame;
        }
        let lanes = split_lanes(&song);
        assert_eq!(lanes.len(), 20_000);
        assert!(lanes.iter().all(|lane| lane.notes.len() == 1));
    }

    #[test]
    fn abc_warns_when_initial_tempo_needs_rounding() {
        let bytes = vec![
            b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0, 96, b'M', b'T', b'r', b'k', 0, 0, 0,
            19, 0, 0xff, 0x51, 3, 0x06, 0x1a, 0x81, 0, 0x90, 60, 100, 96, 0x80, 60, 0, 0, 0xff,
            0x2f, 0,
        ];
        let song = parse_midi(&bytes).unwrap();
        assert_eq!(song.tempos[0].micros_per_quarter, 400_001);
        let (abc, warnings) = midi_to_abc(&song);
        assert!(abc.contains("Q:1/4=150"));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("not an integer BPM"))
        );
    }
}

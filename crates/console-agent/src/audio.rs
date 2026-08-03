//! Audio inspection tooling on top of `console-core`'s deterministic synth.
//!
//! Agents can't hear, so this module builds the three non-audible layers
//! described in SPEC.md on top of the raw sample stream a [`Session`]
//! collects every step:
//!
//! 1. **Ground truth as data** — [`AudioEvent`] / [`record_events`]: an
//!    exact diff of `Console::audio_channels()` frame over frame (note_on /
//!    row_change / note_off / music pattern changes), each resolved back to
//!    a human-readable note name via `Cart::sfx`.
//! 2. **Numeric signal stats** — [`StatsWindow`] / [`compute_stats`]: plain
//!    RMS/peak/clip-count over fixed-size windows of the raw samples.
//! 3. **A semitone-grid spectrogram** — [`render_spectrogram`]: a PNG for
//!    vision models, one column per ~50ms window and one row per semitone,
//!    built from a hand-rolled Goertzel filter per `NOTE_FREQ` bin.
//!
//! Plus [`encode_wav`], a hand-rolled 16-bit PCM WAV encoder for humans.
//!
//! [`Session`]: crate::session::Session

use console_core::{
    CHANNEL_COUNT, Cart, ChannelInfo, Console, NOTE_FREQ, SAMPLE_RATE, SAMPLES_PER_FRAME, SfxRow,
};
use serde::Serialize;

// ---------------------------------------------------------------------------
// Note names
// ---------------------------------------------------------------------------

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Inverse of `console_core::parse_note`: turn a semitone index (0 = C0 ..
/// 95 = B7) into a name like `"C#4"`.
pub fn note_name(note: u8) -> String {
    let note = note as usize % 96;
    format!("{}{}", NOTE_NAMES[note % 12], note / 12)
}

/// Resolve a channel's current `(sfx, row)` to the row's note name (or
/// `"rest"`), waveform and volume, straight from the cart's `__sfx__` data
/// — the "ground truth" for what the sequencer is doing right now.
fn resolve_row(cart: &Cart, sfx: Option<u8>, row: u16) -> (Option<String>, Option<u8>, Option<u8>) {
    let Some(id) = sfx else {
        return (None, None, None);
    };
    let Some(s) = cart.sfx(id) else {
        return (None, None, None);
    };
    match s.rows.get(row as usize) {
        Some(SfxRow::Rest) => (Some("rest".to_string()), None, Some(0)),
        Some(SfxRow::Note { note, wave, vol }) => (Some(note_name(*note)), Some(*wave), Some(*vol)),
        None => (None, None, None),
    }
}

// ---------------------------------------------------------------------------
// Layer 1: exact sequencer events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioEventKind {
    NoteOn,
    RowChange,
    NoteOff,
    MusicPatternChange,
}

/// One sequencer event, derived by diffing `Console::audio_channels()` (and
/// `music_pattern()`) frame over frame.
#[derive(Debug, Clone, Serialize)]
pub struct AudioEvent {
    pub frame: u64,
    pub channel: Option<u8>,
    pub kind: AudioEventKind,
    pub sfx: Option<u8>,
    pub row: Option<u16>,
    /// A note name (`"E5"`), `"rest"` for a silent row, or `null` when no
    /// row applies (note_off / music_pattern_change events).
    pub note: Option<String>,
    pub wave: Option<u8>,
    pub vol: Option<u8>,
    pub from_music: bool,
    /// Only set on `music_pattern_change` events: the pattern now playing,
    /// or `null` if music just stopped.
    pub pattern: Option<u8>,
}

/// Diff one frame's channel snapshot (and music pattern) against the
/// previous frame's and append any resulting events to `events`.
///
/// - `note_on`: a channel goes from idle to busy, or stays busy but its sfx
///   id changes (stolen/reclaimed mid-note).
/// - `row_change`: same sfx, row index advanced (or looped).
/// - `note_off`: a channel goes from busy to idle.
/// - `music_pattern_change`: `music_pattern()` itself changed.
pub fn record_events(
    events: &mut Vec<AudioEvent>,
    frame: u64,
    prev: &[ChannelInfo; CHANNEL_COUNT],
    cur: &[ChannelInfo; CHANNEL_COUNT],
    prev_pattern: Option<u8>,
    cur_pattern: Option<u8>,
    cart: &Cart,
) {
    if cur_pattern != prev_pattern {
        events.push(AudioEvent {
            frame,
            channel: None,
            kind: AudioEventKind::MusicPatternChange,
            sfx: None,
            row: None,
            note: None,
            wave: None,
            vol: None,
            from_music: false,
            pattern: cur_pattern,
        });
    }

    for (i, (p, c)) in prev.iter().zip(cur.iter()).enumerate() {
        let channel = i as u8;
        if c.busy && (!p.busy || c.sfx != p.sfx) {
            let (note, wave, vol) = resolve_row(cart, c.sfx, c.row);
            events.push(AudioEvent {
                frame,
                channel: Some(channel),
                kind: AudioEventKind::NoteOn,
                sfx: c.sfx,
                row: Some(c.row),
                note,
                wave,
                vol,
                from_music: c.from_music,
                pattern: None,
            });
        } else if c.busy && c.sfx == p.sfx && c.row != p.row {
            let (note, wave, vol) = resolve_row(cart, c.sfx, c.row);
            events.push(AudioEvent {
                frame,
                channel: Some(channel),
                kind: AudioEventKind::RowChange,
                sfx: c.sfx,
                row: Some(c.row),
                note,
                wave,
                vol,
                from_music: c.from_music,
                pattern: None,
            });
        } else if !c.busy && p.busy {
            events.push(AudioEvent {
                frame,
                channel: Some(channel),
                kind: AudioEventKind::NoteOff,
                sfx: p.sfx,
                row: Some(p.row),
                note: None,
                wave: None,
                vol: Some(0),
                from_music: p.from_music,
                pattern: None,
            });
        }
    }
}

/// Current per-channel state, with the row's note name resolved, for the
/// `audio_state` RPC / nothing-to-diff snapshot use case.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelState {
    pub index: u8,
    pub sfx: Option<u8>,
    pub row: u16,
    pub wave: u8,
    pub vol: u8,
    pub from_music: bool,
    pub busy: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioState {
    pub channels: Vec<ChannelState>,
    pub music_pattern: Option<u8>,
    pub frame_count: u64,
}

/// Snapshot `console`'s current channels + music pattern, resolving each
/// busy channel's row to a note name via its cart.
pub fn audio_state(console: &Console) -> AudioState {
    let cart = console.cart();
    let channels = console
        .audio_channels()
        .iter()
        .enumerate()
        .map(|(i, c)| ChannelState {
            index: i as u8,
            sfx: c.sfx,
            row: c.row,
            wave: c.wave,
            vol: c.vol,
            from_music: c.from_music,
            busy: c.busy,
            note: if c.busy {
                resolve_row(cart, c.sfx, c.row).0
            } else {
                None
            },
        })
        .collect();
    AudioState {
        channels,
        music_pattern: console.music_pattern(),
        frame_count: console.frame_count(),
    }
}

// ---------------------------------------------------------------------------
// Layer 2: numeric signal stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize)]
pub struct StatsWindow {
    pub start_frame: u64,
    pub rms: f32,
    pub peak: f32,
    pub clipped: u32,
}

/// Per-window RMS/peak/clip-count over `samples` (`SAMPLES_PER_FRAME`
/// samples per frame, `window_frames` frames per window; the last window is
/// shorter when the log doesn't divide evenly). "Clipped" counts samples at
/// or past +-0.999 — the synth itself hard-clamps to `[-1, 1]`, so a
/// healthy mix should show zero.
pub fn compute_stats(samples: &[f32], window_frames: u64) -> Vec<StatsWindow> {
    let window_frames = window_frames.max(1);
    let window_len = window_frames as usize * SAMPLES_PER_FRAME;
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut frame = 0u64;
    while start < samples.len() {
        let end = (start + window_len).min(samples.len());
        let chunk = &samples[start..end];
        let mut sum_sq = 0f64;
        let mut peak = 0f32;
        let mut clipped = 0u32;
        for &s in chunk {
            sum_sq += f64::from(s) * f64::from(s);
            let a = s.abs();
            if a > peak {
                peak = a;
            }
            if a >= 0.999 {
                clipped += 1;
            }
        }
        let rms = (sum_sq / chunk.len().max(1) as f64).sqrt() as f32;
        out.push(StatsWindow {
            start_frame: frame,
            rms,
            peak,
            clipped,
        });
        start = end;
        frame += window_frames;
    }
    out
}

// ---------------------------------------------------------------------------
// WAV (for humans)
// ---------------------------------------------------------------------------

/// Hand-rolled 44-byte-header, 16-bit PCM mono 44100 Hz WAV. No dependency
/// beyond what's already in this crate.
pub fn encode_wav(samples: &[f32]) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32;
    let mut buf = Vec::with_capacity(44 + data_size as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_size).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size (PCM)
    buf.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // channels = mono
    buf.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    let byte_rate = SAMPLE_RATE * 2; // sample_rate * channels(1) * bytes/sample(2)
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align = channels * bytes/sample
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf
}

// ---------------------------------------------------------------------------
// Layer 3: semitone-grid spectrogram
// ---------------------------------------------------------------------------

/// Frames per Goertzel analysis window (≈50ms).
const GOERTZEL_WINDOW_FRAMES: usize = 3;
/// Samples per Goertzel analysis window: 3 * 735 = 2205 (≈50ms @ 44100 Hz).
const GOERTZEL_WINDOW_SAMPLES: usize = GOERTZEL_WINDOW_FRAMES * SAMPLES_PER_FRAME;
const NOTE_COUNT: usize = 96;
/// dB floor the color ramp maps to fully dark.
const DB_FLOOR: f32 = -60.0;
/// Windows per second (20 windows * 2205 samples = 44100 = 1 second), used
/// for the once-a-second vertical gridlines.
const WINDOWS_PER_SECOND: usize = SAMPLE_RATE as usize / GOERTZEL_WINDOW_SAMPLES;

/// Precomputed per-note-bin Goertzel coefficients (cosine of the bin's
/// angular frequency, computed once with `f64` std math — never at
/// per-sample runtime cost, and never claimed deterministic across
/// platforms: this is analysis tooling, not the synth).
struct GoertzelBin {
    coeff: f64,
    cos_w: f64,
    sin_w: f64,
}

fn goertzel_bins() -> [GoertzelBin; NOTE_COUNT] {
    std::array::from_fn(|i| {
        let freq = f64::from(NOTE_FREQ[i]);
        let w = 2.0 * std::f64::consts::PI * freq / f64::from(SAMPLE_RATE);
        GoertzelBin {
            coeff: 2.0 * w.cos(),
            cos_w: w.cos(),
            sin_w: w.sin(),
        }
    })
}

/// Goertzel magnitude of `bin`'s frequency over a fixed-length window,
/// normalized so a full-scale sine at exactly that frequency reads ~1.0.
fn goertzel_mag(bin: &GoertzelBin, window: &[f32; GOERTZEL_WINDOW_SAMPLES]) -> f64 {
    let mut s1 = 0.0f64;
    let mut s2 = 0.0f64;
    for &x in window {
        let s0 = f64::from(x) + bin.coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let real = s1 - s2 * bin.cos_w;
    let imag = s2 * bin.sin_w;
    (real * real + imag * imag).sqrt() / (GOERTZEL_WINDOW_SAMPLES as f64 / 2.0)
}

pub struct Spectrogram {
    pub png: Vec<u8>,
    pub windows: usize,
    pub width: u32,
    pub height: u32,
}

/// Render a semitone-grid spectrogram PNG: X = time (one column per
/// `GOERTZEL_WINDOW_SAMPLES`-sample window, ~50ms), Y = 96 semitone rows
/// (B7 at the top, C0 at the bottom, matching a piano roll). Each
/// (window, note) magnitude comes from a Goertzel filter tuned to that
/// note's exact `NOTE_FREQ`, log-scaled to dB and mapped through a
/// dark-navy -> blue -> yellow-white ramp. Faint gridlines mark octave
/// boundaries (every 12 rows) and one-second ticks (every 20 columns).
pub fn render_spectrogram(samples: &[f32], cell: u32) -> Spectrogram {
    let cell = cell.max(1);
    let windows = samples.len().div_ceil(GOERTZEL_WINDOW_SAMPLES).max(1);
    let bins = goertzel_bins();

    // Compute dB magnitude per (window, note) up front so gridlines drawn
    // afterwards blend with already-final colors.
    let mut db = vec![DB_FLOOR; windows * NOTE_COUNT];
    let mut buf = [0f32; GOERTZEL_WINDOW_SAMPLES];
    for (w, db_row) in db.chunks_exact_mut(NOTE_COUNT).enumerate() {
        buf.fill(0.0);
        let start = w * GOERTZEL_WINDOW_SAMPLES;
        if start < samples.len() {
            let end = (start + GOERTZEL_WINDOW_SAMPLES).min(samples.len());
            buf[..end - start].copy_from_slice(&samples[start..end]);
        }
        for (n, bin) in bins.iter().enumerate() {
            let mag = goertzel_mag(bin, &buf) as f32;
            let d = 20.0 * (mag + 1e-6).log10();
            db_row[n] = d.clamp(DB_FLOOR, 0.0);
        }
    }

    let width = windows as u32 * cell;
    let height = NOTE_COUNT as u32 * cell;
    let mut rgba = vec![0u8; width as usize * height as usize * 4];

    for (w, db_row) in db.chunks_exact(NOTE_COUNT).enumerate() {
        for (n, &d) in db_row.iter().enumerate() {
            let t = (d - DB_FLOOR) / -DB_FLOOR;
            let color = ramp_color(t);
            let x0 = w as u32 * cell;
            let y0 = (NOTE_COUNT - 1 - n) as u32 * cell; // B7 at top, C0 at bottom
            fill_cell(
                &mut rgba,
                width,
                x0,
                y0,
                cell,
                [color[0], color[1], color[2], 255],
            );
        }
    }

    // Faint octave-boundary gridlines, slightly brighter than the
    // once-a-second ones.
    for octave_boundary in (12..NOTE_COUNT).step_by(12) {
        let y = (NOTE_COUNT - octave_boundary) as u32 * cell;
        brighten_row(&mut rgba, width, height, y, 28);
    }
    // Faint one-second gridlines.
    for w in (WINDOWS_PER_SECOND..windows).step_by(WINDOWS_PER_SECOND) {
        let x = w as u32 * cell;
        brighten_col(&mut rgba, width, height, x, 16);
    }

    Spectrogram {
        png: encode_png_rgba(&rgba, width, height),
        windows,
        width,
        height,
    }
}

fn fill_cell(rgba: &mut [u8], width: u32, x0: u32, y0: u32, cell: u32, color: [u8; 4]) {
    for dy in 0..cell {
        for dx in 0..cell {
            let idx = (((y0 + dy) * width + (x0 + dx)) * 4) as usize;
            rgba[idx..idx + 4].copy_from_slice(&color);
        }
    }
}

fn brighten_row(rgba: &mut [u8], width: u32, height: u32, y: u32, amount: u8) {
    if y >= height {
        return;
    }
    for x in 0..width {
        let idx = ((y * width + x) * 4) as usize;
        for c in rgba[idx..idx + 3].iter_mut() {
            *c = c.saturating_add(amount);
        }
    }
}

fn brighten_col(rgba: &mut [u8], width: u32, height: u32, x: u32, amount: u8) {
    if x >= width {
        return;
    }
    for y in 0..height {
        let idx = ((y * width + x) * 4) as usize;
        for c in rgba[idx..idx + 3].iter_mut() {
            *c = c.saturating_add(amount);
        }
    }
}

/// Dark navy (`#1a1c2c`, this project's palette index 0) through blues to a
/// warm yellow-white, matching the console palette's aesthetic.
fn ramp_color(t: f32) -> [u8; 3] {
    const STOPS: [[u8; 3]; 4] = [
        [0x1a, 0x1c, 0x2c],
        [0x29, 0x36, 0x6f],
        [0x41, 0xa6, 0xf6],
        [0xff, 0xcd, 0x75],
    ];
    let segs = STOPS.len() - 1;
    let scaled = t.clamp(0.0, 1.0) * segs as f32;
    let i = (scaled as usize).min(segs - 1);
    let local = scaled - i as f32;
    let a = STOPS[i];
    let b = STOPS[i + 1];
    [
        lerp_u8(a[0], b[0], local),
        lerp_u8(a[1], b[1], local),
        lerp_u8(a[2], b[2], local),
    ]
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8
}

fn encode_png_rgba(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .expect("PNG header write cannot fail on an in-memory buffer");
        writer
            .write_image_data(rgba)
            .expect("PNG data write cannot fail on an in-memory buffer");
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_name_round_trips_through_parse_note() {
        for n in 0..96u8 {
            let name = note_name(n);
            assert_eq!(
                console_core::parse_note(&name),
                Some(n),
                "note_name({n}) = {name:?} did not round-trip"
            );
        }
        assert_eq!(note_name(0), "C0");
        assert_eq!(note_name(57), "A4");
        assert_eq!(note_name(49), "C#4");
        assert_eq!(note_name(95), "B7");
    }

    #[test]
    fn wav_header_is_well_formed() {
        let samples = vec![0.0f32; 735 * 3];
        let bytes = encode_wav(&samples);
        assert_eq!(bytes.len(), 44 + samples.len() * 2);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(&bytes[36..40], b"data");
        let data_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(data_size as usize, samples.len() * 2);
        let riff_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        assert_eq!(riff_size, 36 + data_size);
    }

    #[test]
    fn ramp_color_spans_dark_to_bright() {
        let dark = ramp_color(0.0);
        let bright = ramp_color(1.0);
        assert_eq!(dark, [0x1a, 0x1c, 0x2c]);
        assert_eq!(bright, [0xff, 0xcd, 0x75]);
    }
}

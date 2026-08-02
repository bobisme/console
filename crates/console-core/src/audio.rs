//! Deterministic audio: tracker data model, text parsers, sequencer and synth.
//!
//! Everything here is built from integer arithmetic, `+`/`-`/`*` on floats and
//! comparisons. There is no `powf`, `sin`, `exp` or table lookup that depends on
//! libm, so native and `wasm32-unknown-emscripten` builds emit bit-identical
//! samples. The only "expensive" number in the whole path is the note table,
//! which is a `const` array of f32 literals (see [`NOTE_FREQ`]).

use std::collections::BTreeMap;

use crate::error::Error;

/// Output sample rate in Hz.
pub const SAMPLE_RATE: u32 = 44100;

/// Samples rendered per `step()`: 44100 / 60, exactly.
pub const SAMPLES_PER_FRAME: usize = 735;

/// Number of synth channels.
pub const CHANNEL_COUNT: usize = 4;

/// One frame of mono f32 audio.
pub type AudioFrame = [f32; SAMPLES_PER_FRAME];

/// Highest addressable sfx / pattern id.
pub const MAX_ID: u8 = 63;

/// Maximum rows in one sfx.
pub const MAX_SFX_ROWS: usize = 32;

/// Number of waveforms (0..=5).
pub const WAVE_COUNT: u8 = 6;

/// Highest volume level.
pub const MAX_VOL: u8 = 7;

/// Waveform id of the LFSR noise generator.
const WAVE_NOISE: u8 = 5;

/// Samples a full-scale amplitude ramp takes.
pub const RAMP_SAMPLES: u32 = 64;

/// Amplitude change per sample. 1/64 is exactly representable in f32, so the
/// ramp is bit-reproducible and lands exactly on its target.
const RAMP_STEP: f32 = 1.0 / RAMP_SAMPLES as f32;

/// Per-channel mix gain; four channels sum to at most 1.0 before clamping.
const MIX_GAIN: f32 = 0.25;

/// Initial (and only) seed of the noise LFSR. Non-zero, fixed forever: the
/// noise stream is a function of playback alone and never of the game PRNG.
const LFSR_SEED: u16 = 0xACE1;

/// Phase increment per Hz, in 32-bit fixed point: `2^32 / 44100`.
/// A single `f64` multiply turns a note frequency into a `u32` increment; IEEE
/// multiplication is correctly rounded, so every target agrees.
const PHASE_PER_HZ: f64 = 4294967296.0 / 44100.0;

/// Linear volume levels, `vol / 7`, as a table so the render loop never divides.
const VOL_LEVELS: [f32; 8] = [
    0.0,
    1.0 / 7.0,
    2.0 / 7.0,
    3.0 / 7.0,
    4.0 / 7.0,
    5.0 / 7.0,
    6.0 / 7.0,
    1.0,
];

/// Equal-tempered frequencies for `C0`..=`B7`, A4 = 440 Hz.
///
/// Generated once and pasted in (never computed at runtime) with:
///
/// ```text
/// python3 -c "
/// import struct
/// f32 = lambda x: struct.unpack('<f', struct.pack('<f', x))[0]
/// for n in range(96):
///     print(f32(440.0 * 2.0 ** ((n - 57) / 12.0)))"
/// ```
///
/// Each literal is the shortest decimal that round-trips to the same `f32`.
pub const NOTE_FREQ: [f32; 96] = [
    // C0..B0
    16.351599, 17.323914, 18.354048, 19.445436, 20.601723, 21.826765, 23.124651, 24.499714,
    25.956543, 27.5, 29.135235, 30.867706,
    // C1..B1
    32.703197, 34.647827, 36.708096, 38.890873, 41.203445, 43.65353, 46.249302, 48.999428,
    51.913086, 55.0, 58.27047, 61.735413,
    // C2..B2
    65.406395, 69.295654, 73.41619, 77.781746, 82.40689, 87.30706, 92.498604, 97.998856, 103.82617,
    110.0, 116.54094, 123.470825,
    // C3..B3
    130.81279, 138.59131, 146.83238, 155.56349, 164.81378, 174.61412, 184.99721, 195.99771,
    207.65234, 220.0, 233.08188, 246.94165,
    // C4..B4
    261.62558, 277.18262, 293.66476, 311.12698, 329.62756, 349.22824, 369.99442, 391.99542,
    415.3047, 440.0, 466.16376, 493.8833,
    // C5..B5
    523.25116, 554.36523, 587.3295, 622.25397, 659.2551, 698.4565, 739.98883, 783.99084, 830.6094,
    880.0, 932.3275, 987.7666,
    // C6..B6
    1046.5023, 1108.7305, 1174.659, 1244.5079, 1318.5103, 1396.913, 1479.9777, 1567.9817,
    1661.2188, 1760.0, 1864.655, 1975.5332,
    // C7..B7
    2093.0046, 2217.461, 2349.318, 2489.0159, 2637.0205, 2793.826, 2959.9553, 3135.9634, 3322.4375,
    3520.0, 3729.31, 3951.0664,
];

/// Fixed-point phase increment for a note index (0 = C0, 95 = B7).
fn note_increment(note: u8) -> u32 {
    let hz = NOTE_FREQ[note as usize % NOTE_FREQ.len()];
    // `+ 0.5` then truncate = round-half-up. Multiply, add and the saturating
    // float->int cast are all exactly specified, so every target agrees.
    (f64::from(hz) * PHASE_PER_HZ + 0.5) as u32
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// One row of a sfx: either a note or a rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfxRow {
    /// Silence this row (volume 0; frequency and waveform are untouched).
    Rest,
    /// Play `note` (0 = C0 .. 95 = B7) with waveform 0..=5 and volume 0..=7.
    Note { note: u8, wave: u8, vol: u8 },
}

/// A parsed `__sfx__` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sfx {
    /// Frames each row is held for (1..=255).
    pub speed: u8,
    /// `loop=<start>,<end>`: after `end` plays, jump back to `start`.
    /// Honoured for `sfx()` playback and ignored under `music()`.
    pub loop_range: Option<(u8, u8)>,
    /// 1..=32 rows.
    pub rows: Vec<SfxRow>,
}

impl Sfx {
    /// Frames one full non-looping pass takes.
    pub fn duration(&self) -> u32 {
        self.rows.len() as u32 * u32::from(self.speed)
    }
}

/// What happens when a music pattern finishes one pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternEnd {
    /// Play the next existing pattern id, or halt if there is none.
    Next,
    /// Halt music.
    Stop,
    /// Jump to this pattern id.
    Loop(u8),
}

/// A parsed `__music__` entry: four channel slots plus a continuation rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pattern {
    /// Per-channel sfx id, `None` for a `-` slot.
    pub slots: [Option<u8>; CHANNEL_COUNT],
    pub end: PatternEnd,
}

/// Everything a cart's `__sfx__` and `__music__` sections describe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioBank {
    sfx: BTreeMap<u8, Sfx>,
    patterns: BTreeMap<u8, Pattern>,
}

impl AudioBank {
    /// Sfx `id`, if the cart defines it.
    pub fn sfx(&self, id: u8) -> Option<&Sfx> {
        self.sfx.get(&id)
    }

    /// Pattern `id`, if the cart defines it.
    pub fn pattern(&self, id: u8) -> Option<&Pattern> {
        self.patterns.get(&id)
    }

    /// Defined sfx ids, ascending.
    pub fn sfx_ids(&self) -> impl Iterator<Item = u8> + '_ {
        self.sfx.keys().copied()
    }

    /// Defined pattern ids, ascending.
    pub fn pattern_ids(&self) -> impl Iterator<Item = u8> + '_ {
        self.patterns.keys().copied()
    }

    /// True when the cart has neither sfx nor patterns.
    pub fn is_empty(&self) -> bool {
        self.sfx.is_empty() && self.patterns.is_empty()
    }

    /// The lowest pattern id greater than `id`.
    pub fn next_pattern_after(&self, id: u8) -> Option<u8> {
        self.patterns.range(id.saturating_add(1)..).next().map(|(k, _)| *k)
    }

    /// Parse the raw text of `__sfx__` and `__music__` (either may be absent).
    pub(crate) fn parse(sfx_text: Option<&str>, music_text: Option<&str>) -> Result<AudioBank, Error> {
        let sfx = match sfx_text {
            Some(t) => parse_sfx_section(t)?,
            None => BTreeMap::new(),
        };
        let (patterns, loop_lines) = match music_text {
            Some(t) => parse_music_section(t, &sfx)?,
            None => (BTreeMap::new(), Vec::new()),
        };
        for (id, line) in loop_lines {
            if let Some(Pattern {
                end: PatternEnd::Loop(target),
                ..
            }) = patterns.get(&id)
                && !patterns.contains_key(target)
            {
                return Err(cart_err(
                    "__music__",
                    line,
                    format!("loop target pattern {target} is not defined"),
                ));
            }
        }
        Ok(AudioBank { sfx, patterns })
    }
}

// ---------------------------------------------------------------------------
// Text parsing
// ---------------------------------------------------------------------------

fn cart_err(section: &str, line: usize, msg: impl AsRef<str>) -> Error {
    Error::Cart(format!("{section} line {line}: {}", msg.as_ref()))
}

/// Strip a `#`-comment (only when `#` starts the line, so `C#4` is safe) and
/// surrounding whitespace.
fn clean(raw: &str) -> &str {
    let t = raw.trim();
    if t.starts_with('#') { "" } else { t }
}

/// Parse a note name such as `C4`, `c4` or `A#7` into 0..=95 (C0..B7).
pub fn parse_note(token: &str) -> Option<u8> {
    let b = token.as_bytes();
    if b.len() < 2 || b.len() > 3 {
        return None;
    }
    let semitone: u8 = match b[0].to_ascii_uppercase() {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let (sharp, oct_byte) = if b.len() == 3 {
        if b[1] != b'#' {
            return None;
        }
        (1u8, b[2])
    } else {
        (0u8, b[1])
    };
    if !oct_byte.is_ascii_digit() {
        return None;
    }
    let octave = u16::from(oct_byte - b'0');
    let index = octave * 12 + u16::from(semitone) + u16::from(sharp);
    if index > 95 { None } else { Some(index as u8) }
}

/// True for a rest token: one or more `-` and nothing else.
fn is_rest(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|c| c == b'-')
}

fn parse_u8_in(
    section: &str,
    line: usize,
    what: &str,
    text: &str,
    max: u8,
) -> Result<u8, Error> {
    let v: u32 = text
        .parse()
        .map_err(|_| cart_err(section, line, format!("{what} must be a number, found {text:?}")))?;
    if v > u32::from(max) {
        return Err(cart_err(
            section,
            line,
            format!("{what} must be 0-{max}, found {v}"),
        ));
    }
    Ok(v as u8)
}

fn parse_sfx_section(text: &str) -> Result<BTreeMap<u8, Sfx>, Error> {
    const SEC: &str = "__sfx__";
    let mut out: BTreeMap<u8, Sfx> = BTreeMap::new();
    let mut current: Option<(u8, Sfx, usize)> = None;

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let body = clean(raw);
        if body.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = body.split_whitespace().collect();

        if tokens[0].eq_ignore_ascii_case("sfx") {
            if let Some((id, sfx, hdr)) = current.take() {
                finish_sfx(&mut out, id, sfx, hdr)?;
            }
            current = Some(parse_sfx_header(line, &tokens)?);
            continue;
        }

        let Some((id, sfx, _)) = current.as_mut() else {
            return Err(cart_err(
                SEC,
                line,
                format!("row {body:?} appears before any `sfx <id> speed=<n>` header"),
            ));
        };
        if sfx.rows.len() >= MAX_SFX_ROWS {
            return Err(cart_err(
                SEC,
                line,
                format!("sfx {id} has more than {MAX_SFX_ROWS} rows"),
            ));
        }

        if tokens.len() == 1 && is_rest(tokens[0]) {
            sfx.rows.push(SfxRow::Rest);
            continue;
        }
        if tokens.len() != 3 {
            return Err(cart_err(
                SEC,
                line,
                format!("expected `NOTE WAVE VOL` or `---`, found {body:?}"),
            ));
        }
        let note = parse_note(tokens[0]).ok_or_else(|| {
            cart_err(
                SEC,
                line,
                format!("bad note {:?} (expected C0-B7, e.g. `C#4`)", tokens[0]),
            )
        })?;
        let wave = parse_u8_in(SEC, line, "wave", tokens[1], WAVE_COUNT - 1)?;
        let vol = parse_u8_in(SEC, line, "vol", tokens[2], MAX_VOL)?;
        sfx.rows.push(SfxRow::Note { note, wave, vol });
    }

    if let Some((id, sfx, hdr)) = current.take() {
        finish_sfx(&mut out, id, sfx, hdr)?;
    }
    Ok(out)
}

fn parse_sfx_header(line: usize, tokens: &[&str]) -> Result<(u8, Sfx, usize), Error> {
    const SEC: &str = "__sfx__";
    if tokens.len() < 2 {
        return Err(cart_err(
            SEC,
            line,
            "expected `sfx <id 0-63> speed=<1-255> [loop=<start>,<end>]`",
        ));
    }
    let id = parse_u8_in(SEC, line, "sfx id", tokens[1], MAX_ID)?;
    let mut speed: Option<u8> = None;
    let mut loop_range: Option<(u8, u8)> = None;

    for tok in &tokens[2..] {
        let Some((key, value)) = tok.split_once('=') else {
            return Err(cart_err(
                SEC,
                line,
                format!("unexpected {tok:?} in sfx header (want `speed=` or `loop=`)"),
            ));
        };
        match key.to_ascii_lowercase().as_str() {
            "speed" => {
                let s = parse_u8_in(SEC, line, "speed", value, 255)?;
                if s == 0 {
                    return Err(cart_err(SEC, line, "speed must be 1-255, found 0"));
                }
                speed = Some(s);
            }
            "loop" => {
                let Some((a, b)) = value.split_once(',') else {
                    return Err(cart_err(
                        SEC,
                        line,
                        format!("loop must be `loop=<start>,<end>`, found {value:?}"),
                    ));
                };
                let start = parse_u8_in(SEC, line, "loop start", a, (MAX_SFX_ROWS - 1) as u8)?;
                let end = parse_u8_in(SEC, line, "loop end", b, (MAX_SFX_ROWS - 1) as u8)?;
                if start > end {
                    return Err(cart_err(
                        SEC,
                        line,
                        format!("loop start {start} is after loop end {end}"),
                    ));
                }
                loop_range = Some((start, end));
            }
            other => {
                return Err(cart_err(
                    SEC,
                    line,
                    format!("unknown sfx header key {other:?} (want `speed` or `loop`)"),
                ));
            }
        }
    }

    let Some(speed) = speed else {
        return Err(cart_err(SEC, line, format!("sfx {id} is missing `speed=<n>`")));
    };
    Ok((
        id,
        Sfx {
            speed,
            loop_range,
            rows: Vec::new(),
        },
        line,
    ))
}

fn finish_sfx(
    out: &mut BTreeMap<u8, Sfx>,
    id: u8,
    sfx: Sfx,
    header_line: usize,
) -> Result<(), Error> {
    const SEC: &str = "__sfx__";
    if sfx.rows.is_empty() {
        return Err(cart_err(SEC, header_line, format!("sfx {id} has no rows")));
    }
    if let Some((_, end)) = sfx.loop_range
        && usize::from(end) >= sfx.rows.len()
    {
        return Err(cart_err(
            SEC,
            header_line,
            format!(
                "loop end {end} is past the last row ({} rows)",
                sfx.rows.len()
            ),
        ));
    }
    if out.insert(id, sfx).is_some() {
        return Err(cart_err(SEC, header_line, format!("duplicate sfx id {id}")));
    }
    Ok(())
}

type MusicParse = (BTreeMap<u8, Pattern>, Vec<(u8, usize)>);

fn parse_music_section(text: &str, sfx: &BTreeMap<u8, Sfx>) -> Result<MusicParse, Error> {
    const SEC: &str = "__music__";
    let mut out: BTreeMap<u8, Pattern> = BTreeMap::new();
    let mut lines_of = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let body = clean(raw);
        if body.is_empty() {
            continue;
        }
        let Some((head, tail)) = body.split_once(':') else {
            return Err(cart_err(
                SEC,
                line,
                format!("expected `pat <id> [stop|loop=<id>] : a b c d`, found {body:?}"),
            ));
        };
        let head: Vec<&str> = head.split_whitespace().collect();
        if head.is_empty() || !head[0].eq_ignore_ascii_case("pat") {
            return Err(cart_err(
                SEC,
                line,
                format!("pattern lines must start with `pat`, found {body:?}"),
            ));
        }
        if head.len() < 2 {
            return Err(cart_err(SEC, line, "missing pattern id"));
        }
        let id = parse_u8_in(SEC, line, "pattern id", head[1], MAX_ID)?;

        let mut end = PatternEnd::Next;
        for tok in &head[2..] {
            if tok.eq_ignore_ascii_case("stop") {
                end = PatternEnd::Stop;
            } else if let Some(v) = tok
                .strip_prefix("loop=")
                .or_else(|| tok.strip_prefix("LOOP="))
            {
                end = PatternEnd::Loop(parse_u8_in(SEC, line, "loop target", v, MAX_ID)?);
            } else {
                return Err(cart_err(
                    SEC,
                    line,
                    format!("unknown pattern flag {tok:?} (want `stop` or `loop=<id>`)"),
                ));
            }
        }

        let slot_tokens: Vec<&str> = tail.split_whitespace().collect();
        if slot_tokens.len() != CHANNEL_COUNT {
            return Err(cart_err(
                SEC,
                line,
                format!(
                    "expected {CHANNEL_COUNT} channel slots after `:`, found {}",
                    slot_tokens.len()
                ),
            ));
        }
        let mut slots = [None; CHANNEL_COUNT];
        for (ch, tok) in slot_tokens.iter().enumerate() {
            if is_rest(tok) {
                continue;
            }
            let sid = parse_u8_in(SEC, line, "slot sfx id", tok, MAX_ID)?;
            if !sfx.contains_key(&sid) {
                return Err(cart_err(
                    SEC,
                    line,
                    format!("channel {ch} refers to sfx {sid}, which is not defined in __sfx__"),
                ));
            }
            slots[ch] = Some(sid);
        }

        if out.insert(id, Pattern { slots, end }).is_some() {
            return Err(cart_err(SEC, line, format!("duplicate pattern id {id}")));
        }
        lines_of.push((id, line));
    }
    Ok((out, lines_of))
}

// ---------------------------------------------------------------------------
// Synth
// ---------------------------------------------------------------------------

/// Who owns a channel right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Free,
    Sfx,
    Music,
}

/// Row cursor for a sfx playing on one channel.
#[derive(Debug, Clone, Copy)]
struct Cursor {
    id: u8,
    row: u16,
    frames_left: u32,
}

#[derive(Debug, Clone)]
struct Channel {
    phase: u32,
    inc: u32,
    wave: u8,
    vol: u8,
    amp: f32,
    /// Voice queued behind a click-guard ramp: applied once `amp` reaches 0.
    pending: Option<(u32, u8, u8)>,
    cursor: Option<Cursor>,
    owner: Owner,
}

impl Channel {
    const fn new() -> Channel {
        Channel {
            phase: 0,
            inc: 0,
            wave: 0,
            vol: 0,
            amp: 0.0,
            pending: None,
            cursor: None,
            owner: Owner::Free,
        }
    }

    /// Set frequency, waveform and volume.
    ///
    /// A pure volume change glides (the waveform stays continuous, so there is
    /// nothing to click). A frequency or waveform change is queued: the
    /// amplitude ramps to zero first, the new voice is swapped in at silence,
    /// then the amplitude ramps back up. Phase is never reset, so held notes
    /// stay legato.
    fn set_voice(&mut self, inc: u32, wave: u8, vol: u8) {
        let (cur_inc, cur_wave) = match self.pending {
            Some((i, w, _)) => (i, w),
            None => (self.inc, self.wave),
        };
        if inc == cur_inc && wave == cur_wave {
            self.set_vol(vol);
        } else if self.amp == 0.0 && self.pending.is_none() {
            self.inc = inc;
            self.wave = wave;
            self.vol = vol;
        } else {
            self.pending = Some((inc, wave, vol));
        }
    }

    fn set_vol(&mut self, vol: u8) {
        match &mut self.pending {
            Some(p) => p.2 = vol,
            None => self.vol = vol,
        }
    }

    /// Release the channel: ramp to silence and forget any queued voice.
    fn stop(&mut self) {
        self.pending = None;
        self.vol = 0;
        self.cursor = None;
        self.owner = Owner::Free;
    }

    fn next_sample(&mut self, lfsr: &mut u16) -> f32 {
        if self.amp == 0.0
            && let Some((inc, wave, vol)) = self.pending.take()
        {
            self.inc = inc;
            self.wave = wave;
            self.vol = vol;
        }

        let target = if self.pending.is_some() {
            0.0
        } else {
            VOL_LEVELS[self.vol as usize]
        };
        if self.amp < target {
            self.amp += RAMP_STEP;
            if self.amp > target {
                self.amp = target;
            }
        } else if self.amp > target {
            self.amp -= RAMP_STEP;
            if self.amp < target {
                self.amp = target;
            }
        }

        // Fully idle: do not advance phase and do not clock the noise LFSR.
        if self.amp == 0.0 && target == 0.0 {
            return 0.0;
        }

        let prev = self.phase;
        self.phase = self.phase.wrapping_add(self.inc);
        if self.wave == WAVE_NOISE && self.phase < prev {
            *lfsr = lfsr_next(*lfsr);
        }
        self.amp * wave_value(self.wave, self.phase, *lfsr)
    }
}

/// 16-bit LFSR with NES-style taps: feedback = bit0 XOR bit1, shifted in at the top.
fn lfsr_next(s: u16) -> u16 {
    let fb = (s ^ (s >> 1)) & 1;
    (s >> 1) | (fb << 15)
}

/// Phase in [0, 1) with 24-bit resolution. `(phase >> 8)` is at most 2^24 - 1,
/// exactly representable in f32, and 2^-24 is an exact power of two, so this
/// multiply is exact on every target.
fn phase_unit(phase: u32) -> f32 {
    (phase >> 8) as f32 * (1.0 / 16_777_216.0)
}

fn wave_value(wave: u8, phase: u32, lfsr: u16) -> f32 {
    match wave {
        // pulse 12.5%
        0 => {
            if phase < 0x2000_0000 {
                1.0
            } else {
                -1.0
            }
        }
        // pulse 25%
        1 => {
            if phase < 0x4000_0000 {
                1.0
            } else {
                -1.0
            }
        }
        // square 50%
        2 => {
            if phase < 0x8000_0000 {
                1.0
            } else {
                -1.0
            }
        }
        // triangle
        3 => {
            let t = phase_unit(phase);
            if phase < 0x8000_0000 {
                4.0 * t - 1.0
            } else {
                3.0 - 4.0 * t
            }
        }
        // saw
        4 => 2.0 * phase_unit(phase) - 1.0,
        // noise
        _ => {
            if lfsr & 1 != 0 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct MusicState {
    pat: u8,
    frames_left: u32,
}

/// A read-only snapshot of one channel, for hosts and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelInfo {
    /// The sfx currently sequencing this channel, if any.
    pub sfx: Option<u8>,
    /// Row index within that sfx.
    pub row: u16,
    /// Current waveform 0..=5.
    pub wave: u8,
    /// Current target volume 0..=7.
    pub vol: u8,
    /// True when the channel is claimed by `music()`.
    pub from_music: bool,
    /// True when the channel is claimed by `sfx()` or `music()`.
    pub busy: bool,
}

/// The synth plus sequencer. Owned by [`State`](crate::state::State) so the Lua
/// closures can reach it.
pub struct Audio {
    bank: AudioBank,
    channels: [Channel; CHANNEL_COUNT],
    music: Option<MusicState>,
    lfsr: u16,
    out: Box<AudioFrame>,
}

impl std::fmt::Debug for Audio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Audio")
            .field("music", &self.music.map(|m| m.pat))
            .field("channels", &self.channel_info())
            .finish()
    }
}

impl Audio {
    pub fn new(bank: AudioBank) -> Audio {
        Audio {
            bank,
            channels: [const { Channel::new() }; CHANNEL_COUNT],
            music: None,
            lfsr: LFSR_SEED,
            out: Box::new([0.0; SAMPLES_PER_FRAME]),
        }
    }

    /// The samples produced by the most recent [`Audio::render`].
    pub fn frame(&self) -> &AudioFrame {
        &self.out
    }

    /// The pattern music is currently playing.
    pub fn music_pattern(&self) -> Option<u8> {
        self.music.map(|m| m.pat)
    }

    pub fn channel_info(&self) -> [ChannelInfo; CHANNEL_COUNT] {
        std::array::from_fn(|i| {
            let c = &self.channels[i];
            ChannelInfo {
                sfx: c.cursor.map(|k| k.id),
                row: c.cursor.map_or(0, |k| k.row),
                wave: c.pending.map_or(c.wave, |p| p.1),
                vol: c.pending.map_or(c.vol, |p| p.2),
                from_music: c.owner == Owner::Music,
                busy: c.owner != Owner::Free,
            }
        })
    }

    // ---- Lua entry points --------------------------------------------------

    /// `sfx(n, ch)`. `n == -1` stops (that channel, or every sfx channel when
    /// `ch == -1`). `ch == -1` auto-picks a channel.
    pub fn lua_sfx(&mut self, n: i32, ch: i32) -> Result<(), String> {
        if !(-1..=i32::from(MAX_ID)).contains(&n) {
            return Err(format!(
                "sfx: id {n} out of range (expected 0-{MAX_ID}, or -1 to stop)"
            ));
        }
        if !(-1..CHANNEL_COUNT as i32).contains(&ch) {
            return Err(format!(
                "sfx: channel {ch} out of range (expected 0-{}, or -1 for auto)",
                CHANNEL_COUNT - 1
            ));
        }

        if n < 0 {
            if ch < 0 {
                for c in &mut self.channels {
                    if c.owner == Owner::Sfx {
                        c.stop();
                    }
                }
            } else {
                self.channels[ch as usize].stop();
            }
            return Ok(());
        }

        let id = n as u8;
        if !self.bank.sfx.contains_key(&id) {
            return Err(format!("sfx: {} {}", id, self.missing_sfx_hint()));
        }
        let ch = if ch >= 0 {
            ch as usize
        } else {
            self.auto_channel()
        };
        self.start_sfx(ch, id, Owner::Sfx);
        Ok(())
    }

    /// `music(n)`. `n == -1` stops music and releases its channels.
    pub fn lua_music(&mut self, n: i32) -> Result<(), String> {
        if !(-1..=i32::from(MAX_ID)).contains(&n) {
            return Err(format!(
                "music: pattern {n} out of range (expected 0-{MAX_ID}, or -1 to stop)"
            ));
        }
        if n < 0 {
            self.stop_music();
            return Ok(());
        }
        let id = n as u8;
        if !self.bank.patterns.contains_key(&id) {
            return Err(format!("music: pattern {} {}", id, self.missing_pattern_hint()));
        }
        self.start_pattern(id);
        Ok(())
    }

    fn missing_sfx_hint(&self) -> String {
        if self.bank.sfx.is_empty() {
            "is not defined: this cart has no __sfx__ section".to_string()
        } else {
            let ids: Vec<String> = self.bank.sfx_ids().map(|i| i.to_string()).collect();
            format!("is not defined (this cart defines sfx {})", ids.join(", "))
        }
    }

    fn missing_pattern_hint(&self) -> String {
        if self.bank.patterns.is_empty() {
            "is not defined: this cart has no __music__ section".to_string()
        } else {
            let ids: Vec<String> = self.bank.pattern_ids().map(|i| i.to_string()).collect();
            format!("is not defined (this cart defines patterns {})", ids.join(", "))
        }
    }

    // ---- sequencing --------------------------------------------------------

    /// First channel busy with neither music nor a sfx; channel 3 if all busy.
    fn auto_channel(&self) -> usize {
        self.channels
            .iter()
            .position(|c| c.owner == Owner::Free)
            .unwrap_or(CHANNEL_COUNT - 1)
    }

    fn start_sfx(&mut self, ch: usize, id: u8, owner: Owner) {
        let Audio { bank, channels, .. } = self;
        let Some(sfx) = bank.sfx.get(&id) else {
            return;
        };
        let c = &mut channels[ch];
        c.cursor = Some(Cursor {
            id,
            row: 0,
            frames_left: u32::from(sfx.speed),
        });
        c.owner = owner;
        apply_row(c, sfx, 0);
    }

    fn stop_music(&mut self) {
        for c in &mut self.channels {
            if c.owner == Owner::Music {
                c.stop();
            }
        }
        self.music = None;
    }

    fn start_pattern(&mut self, id: u8) {
        let Some(pat) = self.bank.pattern(id).copied() else {
            return;
        };
        // Release channels this pattern does not use, then claim the ones it does.
        for (ch, c) in self.channels.iter_mut().enumerate() {
            if c.owner == Owner::Music && pat.slots[ch].is_none() {
                c.stop();
            }
        }
        let mut duration = 1u32;
        for ch in 0..CHANNEL_COUNT {
            let Some(sid) = pat.slots[ch] else { continue };
            if let Some(sfx) = self.bank.sfx(sid) {
                duration = duration.max(sfx.duration());
            }
            self.start_sfx(ch, sid, Owner::Music);
        }
        self.music = Some(MusicState {
            pat: id,
            frames_left: duration,
        });
    }

    /// Advance one frame of sequencing. Runs *after* the frame's samples are
    /// rendered, so a `sfx()` issued from `_update` is audible immediately and
    /// row changes land on the following frame.
    pub fn advance(&mut self) {
        let Audio { bank, channels, .. } = self;
        for c in channels.iter_mut() {
            let Some(mut cur) = c.cursor else { continue };
            cur.frames_left -= 1;
            if cur.frames_left > 0 {
                c.cursor = Some(cur);
                continue;
            }
            let Some(sfx) = bank.sfx.get(&cur.id) else {
                c.stop();
                continue;
            };
            let under_music = c.owner == Owner::Music;
            let mut next = cur.row + 1;
            if !under_music
                && let Some((start, end)) = sfx.loop_range
                && cur.row == u16::from(end)
            {
                next = u16::from(start);
            }
            if usize::from(next) >= sfx.rows.len() {
                if under_music {
                    // Stay claimed but silent until the pattern ends.
                    c.cursor = None;
                    c.set_vol(0);
                } else {
                    c.stop();
                }
                continue;
            }
            cur.row = next;
            cur.frames_left = u32::from(sfx.speed);
            c.cursor = Some(cur);
            apply_row(c, sfx, usize::from(next));
        }

        if let Some(mut m) = self.music {
            m.frames_left -= 1;
            if m.frames_left > 0 {
                self.music = Some(m);
            } else {
                let end = self.bank.pattern(m.pat).map_or(PatternEnd::Stop, |p| p.end);
                match end {
                    PatternEnd::Stop => self.stop_music(),
                    PatternEnd::Loop(target) => {
                        if self.bank.pattern(target).is_some() {
                            self.start_pattern(target);
                        } else {
                            self.stop_music();
                        }
                    }
                    PatternEnd::Next => match self.bank.next_pattern_after(m.pat) {
                        Some(next) => self.start_pattern(next),
                        None => self.stop_music(),
                    },
                }
            }
        }
    }

    /// Render exactly [`SAMPLES_PER_FRAME`] samples from the current channel state.
    pub fn render(&mut self) {
        let Audio {
            channels,
            lfsr,
            out,
            ..
        } = self;
        for slot in out.iter_mut() {
            let mut acc = 0.0f32;
            for c in channels.iter_mut() {
                acc += c.next_sample(lfsr);
            }
            *slot = (acc * MIX_GAIN).clamp(-1.0, 1.0);
        }
    }

    /// Zero the output buffer (used when a frame halts).
    pub fn silence(&mut self) {
        self.out.fill(0.0);
    }
}

fn apply_row(c: &mut Channel, sfx: &Sfx, row: usize) {
    match sfx.rows[row] {
        SfxRow::Rest => c.set_vol(0),
        SfxRow::Note { note, wave, vol } => c.set_voice(note_increment(note), wave, vol),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_table_anchors() {
        assert_eq!(NOTE_FREQ[57], 440.0);
        assert_eq!(NOTE_FREQ[45], 220.0);
        assert_eq!(NOTE_FREQ[69], 880.0);
        assert_eq!(NOTE_FREQ.len(), 96);
        // Monotonically increasing across the whole table.
        assert!(NOTE_FREQ.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn note_names_map_to_indices() {
        assert_eq!(parse_note("C0"), Some(0));
        assert_eq!(parse_note("c0"), Some(0));
        assert_eq!(parse_note("A4"), Some(57));
        assert_eq!(parse_note("C#4"), Some(49));
        assert_eq!(parse_note("B7"), Some(95));
        assert_eq!(parse_note("C8"), None);
        assert_eq!(parse_note("B#7"), None);
        assert_eq!(parse_note("H4"), None);
        assert_eq!(parse_note("Cb4"), None);
        assert_eq!(parse_note("C"), None);
        assert_eq!(parse_note("C#41"), None);
    }

    #[test]
    fn lfsr_is_a_long_cycle_and_never_sticks() {
        let mut s = LFSR_SEED;
        let mut zeros = 0;
        for _ in 0..10_000 {
            s = lfsr_next(s);
            assert_ne!(s, 0, "LFSR must never reach the absorbing zero state");
            zeros += u32::from(s & 1 == 0);
        }
        // Roughly balanced output bits.
        assert!((3000..7000).contains(&zeros), "biased noise: {zeros}");
    }

    #[test]
    fn waveforms_stay_in_range() {
        for wave in 0..WAVE_COUNT {
            for step in 0..512u32 {
                let phase = step.wrapping_mul(0x0080_0000);
                let v = wave_value(wave, phase, 0xACE1);
                assert!((-1.0..=1.0).contains(&v), "wave {wave} produced {v}");
            }
        }
    }

    #[test]
    fn triangle_is_continuous_at_the_peak() {
        let a = wave_value(3, 0x7fff_ff00, 0);
        let b = wave_value(3, 0x8000_0000, 0);
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }

    #[test]
    fn phase_increment_matches_frequency() {
        // Each note should wrap the accumulator ~freq times per second.
        for note in 0..96u8 {
            let inc = u64::from(note_increment(note));
            let wraps = (inc * u64::from(SAMPLE_RATE)) as f64 / 4294967296.0;
            let want = f64::from(NOTE_FREQ[note as usize]);
            assert!(
                (wraps - want).abs() < 0.001,
                "note {note}: {wraps} wraps/s vs {want} Hz"
            );
        }
    }
}

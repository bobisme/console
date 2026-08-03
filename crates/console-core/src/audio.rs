//! Deterministic audio: tracker data model, text parsers, sequencer and synth.
//!
//! Everything here is built from integer arithmetic, `+`/`-`/`*`/`/` on floats
//! and comparisons. There is no `powf`, `sin`, `exp` or table lookup that
//! depends on libm, so native and `wasm32-unknown-emscripten` builds emit
//! bit-identical samples. The only "expensive" number in the whole path is the
//! note table, which is a `const` array of f32 literals (see [`NOTE_FREQ`]).
//!
//! PoC v2 adds musical expression on top of that contract, all of it still
//! transcendental-free (SPEC "Music authoring (PoC v2)"):
//!
//! * [`Instrument`]s (`__instruments__`) give a waveform an envelope, vibrato
//!   and/or a pitch sweep; sfx rows may name one instead of a wave digit.
//! * An effect column ([`Fx`]) puts arpeggio, slide, vibrato or fade on a row.
//! * Fractional semitones resolve by *linear interpolation* between adjacent
//!   [`NOTE_FREQ`] entries ([`freq_at`]), vibrato scales frequency by
//!   `1 + lfo * cents * `[`CENTS_TO_RATIO`], and the LFO is an integer-phase
//!   triangle ([`LFO_STEPS`] units per cycle).
//!
//! All of it is strictly additive: a row with a bare wave digit and no effect
//! never allocates a [`Modulation`] and reaches the synth through exactly the
//! PoC v1 statements, so old carts render the same samples bit for bit.
//!
//! On top of that sits the optional [`Master`] bus (`master` line in
//! `__instruments__`, or the Lua `master()` setter): pre-gain into an odd
//! polynomial soft clipper, a one-pole tone lowpass and a tape-style hiss
//! floor. It is off by default and, when off, the mix takes literally the same
//! statement it always did — see [`Audio::render`].

use std::collections::BTreeMap;

use crate::error::Error;

/// Output sample rate in Hz.
pub const SAMPLE_RATE: u32 = 44100;

/// Samples rendered per `step()`: 44100 / 60, exactly.
pub const SAMPLES_PER_FRAME: usize = 735;

/// Number of synth channels.
pub const CHANNEL_COUNT: usize = 6;

/// Smallest number of slots a `__music__` pattern line may list. Patterns
/// predate the 4 -> 6 channel widening, so a 4-slot line stays legal forever
/// and the missing trailing slots are silent.
pub const MIN_PATTERN_SLOTS: usize = 4;

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

/// Largest `arp` / `sl` semitone magnitude an effect may name (two octaves).
pub const MAX_FX_SEMIS: i32 = 24;

/// Frames each step of an `arp` effect is held for (SPEC: "2 frames per step").
pub const ARP_FRAMES_PER_STEP: u32 = 2;

/// Phase units in one full vibrato LFO cycle. The LFO phase is a 6-bit
/// accumulator advanced by `rate` units per frame, so one cycle lasts
/// `64 / rate` frames (`0.9375 * rate` Hz at 60 fps) and the phase sequence
/// itself repeats exactly every `64 / gcd(64, rate)` frames.
pub const LFO_STEPS: u32 = 64;

/// `ln(2) / 1200`: the first-order cents-to-frequency-ratio factor, so
/// `freq * (1 + lfo * cents * CENTS_TO_RATIO)` bends pitch by `cents` without
/// a `powf`. Written as a literal (never computed) so every target agrees.
// The literal is spelled out to full f64 precision because that is the number
// SPEC names; rounding it to f32 at compile time is exact and reproducible.
#[allow(clippy::excessive_precision)]
pub const CENTS_TO_RATIO: f32 = 0.0005776226504666211;

/// Highest vibrato depth in cents.
pub const MAX_VIB_CENTS: u8 = 100;

/// Highest vibrato rate (LFO phase units per frame).
pub const MAX_VIB_RATE: u8 = 16;

/// Waveform id of the LFSR noise generator.
const WAVE_NOISE: u8 = 5;

/// Samples a full-scale amplitude ramp takes.
pub const RAMP_SAMPLES: u32 = 64;

/// Amplitude change per sample. 1/64 is exactly representable in f32, so the
/// ramp is bit-reproducible and lands exactly on its target.
const RAMP_STEP: f32 = 1.0 / RAMP_SAMPLES as f32;

/// Per-channel mix gain. Deliberately **frozen at 1/4** even though the console
/// now has six channels: rescaling it to 1/6 would change every sample of every
/// existing cart (and every audio golden) for no musical gain, and would make
/// the four-voice carts written against PoC v1/v2 a third quieter.
///
/// Headroom, therefore, is authored rather than enforced:
///
/// - Four full-scale voices in phase still sum to exactly `1.0` — the old
///   worst case, unchanged.
/// - Six full-scale voices in phase sum to `1.5`, and the final
///   `clamp(-1.0, 1.0)` hard-clips the excess. Reaching it takes six
///   simultaneous vol-7 square/saw voices with aligned phase, which is a
///   mixing mistake in any tracker.
/// - Any non-zero `master drive` moves the peak onto the soft shaper, whose
///   output is bounded by `MAKEUP[drive] < 1.0`, so a driven mix cannot reach
///   the clamp at all — `drive=1` is effectively a free limiter.
///
/// `audio_stats` reports clipped-sample counts, and `mix_headroom_*` in
/// `tests/audio.rs` pins all three behaviours.
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

/// Fixed-point phase increment for a frequency in Hz.
fn inc_from_hz(hz: f32) -> u32 {
    // `+ 0.5` then truncate = round-half-up. Multiply, add and the saturating
    // float->int cast are all exactly specified, so every target agrees.
    (f64::from(hz) * PHASE_PER_HZ + 0.5) as u32
}

/// Fixed-point phase increment for a note index (0 = C0, 95 = B7).
fn note_increment(note: u8) -> u32 {
    inc_from_hz(NOTE_FREQ[note as usize % NOTE_FREQ.len()])
}

/// Frequency of `note` displaced by a fractional number of `semis`.
///
/// Per SPEC there are no transcendentals at runtime: a fractional semitone
/// `s = k + f` resolves as `NOTE_FREQ[k] * (1 - f) + NOTE_FREQ[k + 1] * f`,
/// clamped at both ends of the table. `semis == 0` returns the table entry
/// bit-for-bit, so an unmodulated note is identical to [`note_increment`].
pub fn freq_at(note: u8, semis: f32) -> f32 {
    let last = (NOTE_FREQ.len() - 1) as f32;
    let s = f32::from(note) + semis;
    if s <= 0.0 {
        return NOTE_FREQ[0];
    }
    if s >= last {
        return NOTE_FREQ[NOTE_FREQ.len() - 1];
    }
    let k = s as usize; // s > 0 and s < 95, so truncation == floor and k <= 94
    let f = s - k as f32;
    if f == 0.0 {
        return NOTE_FREQ[k];
    }
    NOTE_FREQ[k] * (1.0 - f) + NOTE_FREQ[k + 1] * f
}

/// Integer-phase triangle LFO: `phase` counts 0..[`LFO_STEPS`), the result
/// runs 0 -> +1 -> 0 -> -1 -> 0 in exact sixteenths.
fn lfo_triangle(phase: u32) -> f32 {
    let p = (phase % LFO_STEPS) as i32;
    let v = if p < 16 {
        p
    } else if p < 48 {
        32 - p
    } else {
        p - 64
    };
    v as f32 * (1.0 / 16.0)
}

/// `n / d` rounded half away from zero. `d` must be positive.
fn div_round(n: i32, d: i32) -> i32 {
    debug_assert!(d > 0);
    if n >= 0 {
        (2 * n + d) / (2 * d)
    } else {
        -((-2 * n + d) / (2 * d))
    }
}

// ---------------------------------------------------------------------------
// Sidechain ducking
// ---------------------------------------------------------------------------

/// Deepest `duck` depth: 7/7, i.e. the other channels are muted outright at
/// the trigger instant.
pub const MAX_DUCK_DEPTH: u8 = 7;

/// Samples the duck attack takes to reach full depth (~1.1 ms at 44100).
///
/// The same anti-click spirit as [`RAMP_SAMPLES`], and short enough that the
/// dip still lands *with* the transient rather than after it. The ramp is
/// linear and lands exactly on the target (the last step assigns rather than
/// accumulates), so it is bit-reproducible.
pub const DUCK_ATTACK_SAMPLES: u32 = 48;

/// The one global duck envelope.
///
/// A note-on of a `duck=` instrument on channel `t` [`DuckBus::trigger`]s it:
/// the attenuation ramps to `depth/7` over [`DUCK_ATTACK_SAMPLES`] samples and
/// then recovers linearly to zero across `release` frames. While it runs, the
/// mix multiplies every channel *except* `t` by `1 - atten`.
///
/// Everything is per-sample linear — adds, subtracts and comparisons, plus one
/// division per trigger to size the ramps — so it is bit-identical everywhere.
///
/// There is exactly one envelope, not one per trigger: if two `duck`
/// instruments fire on different channels in the same frame, the **last one
/// applied wins** (channels are visited in index order, so the highest-numbered
/// channel's trigger is the one that survives) and it owns the un-ducked slot
/// until it releases.
#[derive(Debug, Clone, Copy)]
struct DuckBus {
    /// Attenuation applied to the non-trigger channels right now, 0..=1.
    atten: f32,
    /// Attenuation the current attack ramp is heading for.
    peak: f32,
    /// Per-sample attack increment (may be negative if a shallower trigger
    /// interrupts a deeper one).
    attack_step: f32,
    /// Attack samples still to go; 0 means the envelope is releasing.
    attack_left: u32,
    /// Per-sample release decrement.
    release_step: f32,
    /// The channel that fired the live trigger. Never ducked; `None` when the
    /// envelope is idle.
    trigger_ch: Option<u8>,
}

impl DuckBus {
    const fn new() -> DuckBus {
        DuckBus {
            atten: 0.0,
            peak: 0.0,
            attack_step: 0.0,
            attack_left: 0,
            release_step: 0.0,
            trigger_ch: None,
        }
    }

    /// True when nothing is ducking, so the mixer can take the legacy path.
    fn is_idle(&self) -> bool {
        self.trigger_ch.is_none()
    }

    /// Fire (or re-fire) the envelope from channel `ch`.
    ///
    /// A re-trigger during the release restarts the attack from wherever the
    /// attenuation currently is and re-aims it at full depth — the classic
    /// pumping gesture — and hands the un-ducked slot to the new channel.
    fn trigger(&mut self, ch: usize, d: Duck) {
        // depth/7: exactly the volume ladder, reused so the two agree.
        let peak = VOL_LEVELS[usize::from(d.depth.min(MAX_DUCK_DEPTH))];
        let attack = DUCK_ATTACK_SAMPLES.max(1);
        self.peak = peak;
        self.attack_step = (peak - self.atten) / attack as f32;
        self.attack_left = attack;
        // `release` frames from full depth back to unity.
        let frames = u32::from(d.release).max(1);
        self.release_step = peak / (frames * SAMPLES_PER_FRAME as u32) as f32;
        self.trigger_ch = Some(ch as u8);
    }

    /// The gain the non-trigger channels are multiplied by this sample.
    fn gain(&self) -> f32 {
        1.0 - self.atten
    }

    /// Advance one sample. Called once per rendered sample while the envelope
    /// is live; a no-op once it has recovered.
    fn tick(&mut self) {
        if self.trigger_ch.is_none() {
            return;
        }
        if self.attack_left > 0 {
            self.attack_left -= 1;
            // The final step *assigns* the target so the ramp lands exactly on
            // `peak` no matter how the increments rounded.
            self.atten = if self.attack_left == 0 {
                self.peak
            } else {
                self.atten + self.attack_step
            };
            return;
        }
        self.atten -= self.release_step;
        if self.atten <= 0.0 {
            self.atten = 0.0;
            self.trigger_ch = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Master bus: drive / tone / hiss
// ---------------------------------------------------------------------------

/// Highest `drive` setting.
pub const MAX_DRIVE: u8 = 8;

/// Highest `tone` setting.
pub const MAX_TONE: u8 = 8;

/// Highest `hiss` setting.
pub const MAX_HISS: u8 = 4;

/// `master drive=<0-8> [tone=<0-8>] [hiss=<0-4>]` — the cart-global output
/// stage, applied to the channel sum *instead of* the plain clamp.
///
/// All-zero (the [`Default`]) means "no master line": the mix takes the PoC v1
/// statement unchanged and the samples are bit-identical to a console without
/// this feature. A cart may declare one `master` line in `__instruments__`,
/// and the Lua `master(drive, [tone], [hiss])` setter overrides it at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Master {
    /// Pre-gain into the soft clipper, 0..=[`MAX_DRIVE`]. 0 = bypass.
    pub drive: u8,
    /// One-pole lowpass darkness, 0..=[`MAX_TONE`]. 0 = bypass.
    pub tone: u8,
    /// Noise-floor level, 0..=[`MAX_HISS`]. 0 = silent.
    pub hiss: u8,
}

impl Master {
    /// The all-zero master: every stage bypassed.
    pub const OFF: Master = Master {
        drive: 0,
        tone: 0,
        hiss: 0,
    };

    /// True when no stage is engaged, i.e. the render loop must take the
    /// legacy `(sum * 0.25).clamp(-1, 1)` path.
    pub fn is_bypass(&self) -> bool {
        *self == Master::OFF
    }
}

/// Where the soft clipper stops curving and becomes a hard clip.
///
/// The shaper is the rational odd cubic `R(x) = x * (27 + x^2) / (27 + 9x^2)`
/// (SPEC's second option) rather than `1.5x - 0.5x^3`, for four reasons:
///
/// 1. **Harmonic profile.** `R` is odd, so it makes *only* odd harmonics — no
///    even-order buzz and no DC offset, which is what a push-pull output stage
///    does. Its series is `x - (8/27)x^3 + ...`, a third-harmonic coefficient
///    of -0.296 against the plain cubic's -0.5, so the first few drive
///    settings are audibly *warm* rather than immediately gritty.
/// 2. **A long soft region.** `R` only reaches full scale at `|x| = 3`, where
///    the plain cubic hard-clips at `|x| = 1`. The whole drive range therefore
///    sweeps through progressive compression instead of falling off a cliff
///    after one setting — the "console pushed into its ceiling" feel.
/// 3. **Clean seams.** `R(3) = 1` exactly *and* `R'(3) = 0`, because
///    `R'(x) = 9 (x^2 - 9)^2 / (27 + 9x^2)^2`. The hard-clip point is C1, so
///    there is no derivative discontinuity to spray aliasing.
/// 4. That same derivative is a square over a square: `R' >= 0` everywhere, so
///    the shaper is monotonic (strictly so inside the knee) and can never fold
///    the waveform back on itself.
///
/// `R'(0) = 1`, so drive 1 starts from unity small-signal gain.
///
/// Only `*`, `+`, `/` and comparisons are involved. IEEE-754 specifies all
/// three exactly, so native and wasm agree bit for bit.
const SHAPER_KNEE: f32 = 3.0;

/// The soft clipper. `|shape(x)| <= 1` for every finite `x`.
fn shape(x: f32) -> f32 {
    if x >= SHAPER_KNEE {
        return 1.0;
    }
    if x <= -SHAPER_KNEE {
        return -1.0;
    }
    let x2 = x * x;
    // In exact arithmetic the quotient cannot leave [-1, 1] inside the knee,
    // but f32 rounding can land a single ULP past it just short of ±3. The
    // clamp restores the invariant without touching the curve anywhere else.
    (x * (27.0 + x2) / (27.0 + 9.0 * x2)).clamp(-1.0, 1.0)
}

/// Pre-gain per drive setting: `1 + drive * 0.35`, written out as decimal
/// literals so nothing depends on how `0.35` accumulates. Index 0 is unused
/// (drive 0 bypasses the stage) and is 1.0 so the table is still an identity.
const PRE_GAIN: [f32; 9] = [1.0, 1.35, 1.7, 2.05, 2.4, 2.75, 3.1, 3.45, 3.8];

/// Reference level the makeup gain is normalised at: 0.7, a hot-but-unclipped
/// mix (the console's ceiling is 1.0 and a busy four-channel groove peaks
/// around here).
///
/// `f64` because it is an *authoring-time* number — it appears in the
/// generator below and never in the render path.
pub const MASTER_REF_LEVEL: f64 = 0.7;

/// Makeup gain per drive setting, `MAKEUP[d] = REF / R(PRE_GAIN[d] * REF)`
/// with `REF = 0.7`: a signal sitting exactly at the reference level comes out
/// of the stage at the level it went in, so raising `drive` adds density
/// rather than volume.
///
/// Generated once and pasted in (never computed at runtime) with:
///
/// ```text
/// python3 -c "
/// import struct
/// f32 = lambda x: struct.unpack('<f', struct.pack('<f', x))[0]
/// R = lambda x: 1.0 if x >= 3 else (-1.0 if x <= -3 else x*(27+x*x)/(27+9*x*x))
/// for g in [1.0, 1.35, 1.7, 2.05, 2.4, 2.75, 3.1, 3.45, 3.8]:
///     print(f32(0.7 / R(f32(g) * 0.7)))"
/// ```
///
/// Consequences, all intentional: the ceiling drops from 0.930 (drive 1) to
/// 0.700 (drive 8) while the small-signal gain climbs from 1.26 to 2.66, i.e.
/// up to +8.5 dB of level for quiet material against -3 dB of peak. That is
/// glue: the loud stays put, the quiet comes up.
#[allow(clippy::excessive_precision)]
const MAKEUP: [f32; 9] = [
    1.0, 0.9304656, 0.8227502, 0.76434356, 0.7321342, 0.7147121, 0.7058169, 0.70176744, 0.70030355,
];

/// One-pole lowpass coefficient per tone setting, `y += a * (x - y)`.
///
/// `a = 1 - exp(-2*pi*fc/44100)`, evaluated **at authoring time** — there is no
/// `exp` anywhere in the render path. The cutoffs are a roughly 1/3-octave
/// ladder from "just takes the fizz off" to "behind a curtain":
///
/// | tone | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
/// |------|---|---|---|---|---|---|---|---|---|
/// | fc Hz | off | 16000 | 12600 | 9900 | 7800 | 6100 | 4800 | 3800 | 3000 |
///
/// Generated once and pasted in with:
///
/// ```text
/// python3 -c "
/// import math, struct
/// f32 = lambda x: struct.unpack('<f', struct.pack('<f', x))[0]
/// for fc in [16000, 12600, 9900, 7800, 6100, 4800, 3800, 3000]:
///     print(f32(1.0 - math.exp(-2.0 * math.pi * fc / 44100.0)))"
/// ```
#[allow(clippy::excessive_precision)]
const TONE_A: [f32; 9] = [
    0.0, 0.8976763, 0.8339051, 0.75598145, 0.67087305, 0.5806724, 0.49534696, 0.41807184,
    0.34781536,
];

/// Cutoff in Hz behind each [`TONE_A`] entry, for documentation and tests.
pub const TONE_CUTOFF_HZ: [u32; 9] = [0, 16000, 12600, 9900, 7800, 6100, 4800, 3800, 3000];

/// Hiss amplitude per setting: `hiss / 2048`, so setting 4 is 2^-9 ≈ -54 dBFS
/// and setting 1 is -66 dBFS. Every value is an exact power-of-two multiple,
/// so the table is representable to the last bit on every target.
const HISS_LEVEL: [f32; 5] = [
    0.0,
    1.0 / 2048.0,
    2.0 / 2048.0,
    3.0 / 2048.0,
    4.0 / 2048.0,
];

/// Seed of the dedicated hiss LFSR. Distinct from [`LFSR_SEED`] so the noise
/// waveform and the noise floor never lock into the same pattern.
const HISS_SEED: u16 = 0x5EED;

/// Below this magnitude the tone filter's memory is snapped to zero. A
/// geometric decay never reaches exactly 0, and letting it trail off into
/// denormals costs cycles on some hosts for a signal 190 dB below anything
/// audible. A plain comparison, so it stays deterministic.
const DENORM_FLOOR: f32 = 1.0e-30;

/// The master bus' per-console runtime state: filter memory and hiss LFSR.
/// Reset by [`Audio::new`] (hence by `Console::new`) and never by anything
/// else, so two consoles fed the same cart and inputs see the same state.
#[derive(Debug, Clone, Copy)]
struct MasterState {
    /// One-pole lowpass memory.
    y: f32,
    /// Hiss LFSR, advanced once per rendered sample while the bus is engaged.
    lfsr: u16,
}

impl MasterState {
    const fn new() -> MasterState {
        MasterState {
            y: 0.0,
            lfsr: HISS_SEED,
        }
    }
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// `env=<attack>,<decay>,<sustain>`: a volume envelope in whole frames.
///
/// Volume climbs 0 -> the row's volume over `attack` frames (reaching it on
/// the last attack frame), then moves to `sustain` over `decay` frames, then
/// holds `sustain` until the row changes. `attack == 0` starts at the row
/// volume; `decay == 0` jumps straight to `sustain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Env {
    /// Frames of attack ramp, 0..=255.
    pub attack: u8,
    /// Frames of decay ramp, 0..=255.
    pub decay: u8,
    /// Level held after the decay, 0..=7.
    pub sustain: u8,
}

/// `vib=<cents>,<rate>,<delay>`: a triangle pitch LFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vib {
    /// Peak depth in cents, 1..=100.
    pub cents: u8,
    /// LFO phase units per frame, 1..=16. One cycle is [`LFO_STEPS`] units,
    /// i.e. `64 / rate` frames.
    pub rate: u8,
    /// Frames after note-on before the LFO starts, 0..=255.
    pub delay: u8,
}

impl Vib {
    /// Signed LFO value in [-1, 1] `frame` frames after note-on (0 while the
    /// delay is still running).
    pub fn value_at(&self, frame: u32) -> f32 {
        let delay = u32::from(self.delay);
        if frame < delay {
            return 0.0;
        }
        lfo_triangle((frame - delay) * u32::from(self.rate))
    }
}

/// `sweep=<semis>,<frames>`: a pitch glide from note-on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sweep {
    /// Signed semitone offset reached at the end of the sweep.
    pub semis: i8,
    /// Frames the sweep takes, 1..=255. The offset holds afterwards.
    pub frames: u8,
}

/// `duck=<depth>,<release>`: makes the instrument a *sidechain trigger*.
///
/// Every note-on row that names the instrument ducks the mix gain of the other
/// three channels; the channel that fired keeps its full level, so a kick
/// punches a hole for itself. See [`DuckBus`] for the envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duck {
    /// Attenuation applied to the other channels at the trigger instant,
    /// 1..=[`MAX_DUCK_DEPTH`], as sevenths (7 = the others go silent).
    pub depth: u8,
    /// Frames the linear recovery back to unity takes, 1..=255.
    pub release: u8,
}

/// One `inst <name> wave=<0-5> [env=...] [vib=...] [sweep=...] [duck=...]` entry.
///
/// A bare wave digit on a sfx row means the *implicit flat instrument*: that
/// waveform with no envelope, vibrato or sweep, which is exactly the PoC v1
/// behaviour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    /// `[a-z0-9_]+`, unique, never all-digits (those name the bare waveforms).
    pub name: String,
    /// Waveform 0..=5.
    pub wave: u8,
    pub env: Option<Env>,
    pub vib: Option<Vib>,
    pub sweep: Option<Sweep>,
    /// Sidechain trigger: every note-on of this instrument ducks the other
    /// channels. Independent of [`Instrument::is_flat`] — ducking costs the
    /// voice itself nothing per frame.
    pub duck: Option<Duck>,
}

impl Instrument {
    /// True when the instrument needs per-frame modulation. A flat instrument
    /// (`inst x wave=2`) takes exactly the same code path as a bare digit.
    ///
    /// `duck` is deliberately not part of this: it is a *mixer* property, so a
    /// `duck`-only instrument still renders through the PoC v1 statements.
    pub fn is_flat(&self) -> bool {
        self.env.is_none() && self.vib.is_none() && self.sweep.is_none()
    }
}

/// The optional 4th token on a note row. One effect per row; effect state is
/// per-row and resets at the next note row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fx {
    /// `arp<a>,<b>`: cycle pitch offsets 0, +a, +b semitones,
    /// [`ARP_FRAMES_PER_STEP`] frames per step.
    Arp { a: u8, b: u8 },
    /// `sl<±n>`: slide `semis` semitones linearly across the row's duration
    /// (the full offset lands exactly on the next row's boundary).
    Slide { semis: i8 },
    /// `vib` / `vib<cents>,<rate>`: vibrato for this row, overriding the
    /// instrument's. The bare form copies the instrument's setting at parse
    /// time; the explicit form has no delay.
    Vibrato(Vib),
    /// `fade<±n>`: ramp the volume by `levels` across the row, reaching
    /// `vol + levels` on the row's last frame.
    Fade { levels: i8 },
}

/// Per-row authoring extras that PoC v1 did not have. Kept alongside
/// [`SfxRow`] rather than inside it so the v1 row shape (and every consumer
/// matching on it) is untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RowMod {
    /// Index into [`AudioBank::instruments`] when the row named an
    /// instrument; `None` for a bare wave digit or a rest.
    pub inst: Option<u8>,
    /// The row's effect column, if any.
    pub fx: Option<Fx>,
}

/// `bpm=<n> [rows_per_beat=<r>]` at the top of `__music__`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tempo {
    pub bpm: u32,
    /// Rows per beat, default 4.
    pub rows_per_beat: u8,
    /// What `speed=auto` resolves to: `round(3600 / (bpm * rows_per_beat))`.
    pub speed: u8,
}

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
    /// Instrument / effect column for each row, same length as `rows`.
    /// All-default for a cart written against PoC v1.
    pub mods: Vec<RowMod>,
}

impl Sfx {
    /// Frames one full non-looping pass takes.
    pub fn duration(&self) -> u32 {
        self.rows.len() as u32 * u32::from(self.speed)
    }

    /// The instrument / effect column of `row` (default outside the sfx).
    pub fn row_mod(&self, row: usize) -> RowMod {
        self.mods.get(row).copied().unwrap_or_default()
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

/// A parsed `__music__` entry: one slot per channel plus a continuation rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pattern {
    /// Per-channel sfx id, `None` for a `-` slot or a slot the pattern line
    /// omitted entirely (a 4-slot line leaves channels 4 and 5 silent).
    pub slots: [Option<u8>; CHANNEL_COUNT],
    pub end: PatternEnd,
}

/// Everything a cart's `__instruments__`, `__sfx__` and `__music__` sections
/// describe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioBank {
    sfx: BTreeMap<u8, Sfx>,
    patterns: BTreeMap<u8, Pattern>,
    /// Instruments in declaration order; [`RowMod::inst`] indexes this.
    instruments: Vec<Instrument>,
    inst_by_name: BTreeMap<String, u8>,
    tempo: Option<Tempo>,
    /// The `master` line from `__instruments__`; all-zero when absent.
    master: Master,
}

impl AudioBank {
    /// Sfx `id`, if the cart defines it.
    pub fn sfx(&self, id: u8) -> Option<&Sfx> {
        self.sfx.get(&id)
    }

    /// Every instrument, in `__instruments__` declaration order.
    pub fn instruments(&self) -> &[Instrument] {
        &self.instruments
    }

    /// The instrument named `name`, if defined.
    pub fn instrument(&self, name: &str) -> Option<&Instrument> {
        self.inst_by_name
            .get(name)
            .and_then(|&i| self.instruments.get(usize::from(i)))
    }

    /// The instrument a [`RowMod::inst`] index refers to.
    pub fn instrument_at(&self, index: u8) -> Option<&Instrument> {
        self.instruments.get(usize::from(index))
    }

    /// The `bpm=` line from `__music__`, if the cart has one.
    pub fn tempo(&self) -> Option<Tempo> {
        self.tempo
    }

    /// The `master` line from `__instruments__`. All-zero (every stage
    /// bypassed) when the cart has none — which is the bit-identical legacy
    /// output path. Lua's `master()` overrides this at runtime without
    /// touching the bank.
    pub fn master(&self) -> Master {
        self.master
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

    /// True when the cart has no instruments, sfx, patterns or master line.
    pub fn is_empty(&self) -> bool {
        self.sfx.is_empty()
            && self.patterns.is_empty()
            && self.instruments.is_empty()
            && self.master.is_bypass()
    }

    /// The lowest pattern id greater than `id`.
    pub fn next_pattern_after(&self, id: u8) -> Option<u8> {
        self.patterns.range(id.saturating_add(1)..).next().map(|(k, _)| *k)
    }

    /// Parse the raw text of `__instruments__`, `__sfx__` and `__music__`
    /// (any of them may be absent).
    ///
    /// Section order in the cart file is irrelevant: the cart splitter hands
    /// all three over at once, instruments resolve first, then sfx rows (which
    /// may name them), then patterns. `speed=auto` needs the `bpm=` line, so
    /// `__music__`'s tempo header is read before `__sfx__` too.
    pub(crate) fn parse(
        inst_text: Option<&str>,
        sfx_text: Option<&str>,
        music_text: Option<&str>,
    ) -> Result<AudioBank, Error> {
        let (instruments, inst_by_name, master) = match inst_text {
            Some(t) => parse_instruments_section(t)?,
            None => (Vec::new(), BTreeMap::new(), Master::OFF),
        };
        let tempo = match music_text {
            Some(t) => parse_tempo_line(t)?,
            None => None,
        };
        let sfx = match sfx_text {
            Some(t) => parse_sfx_section(t, &instruments, &inst_by_name, tempo)?,
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
        Ok(AudioBank {
            sfx,
            patterns,
            instruments,
            inst_by_name,
            tempo,
            master,
        })
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

/// Like [`parse_u8_in`] but for a signed, explicitly bounded field.
fn parse_i32_in(
    section: &str,
    line: usize,
    what: &str,
    text: &str,
    min: i32,
    max: i32,
) -> Result<i32, Error> {
    let v: i32 = text
        .parse()
        .map_err(|_| cart_err(section, line, format!("{what} must be a number, found {text:?}")))?;
    if v < min || v > max {
        return Err(cart_err(
            section,
            line,
            format!("{what} must be {min}-{max}, found {v}"),
        ));
    }
    Ok(v)
}

/// Same rule as `__gfx_meta__` names: `[a-z0-9_]+`.
fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A row's 2nd token is a waveform when it is all digits; anything else is an
/// instrument name. `C4 6 7` therefore still reports "wave must be 0-5".
fn is_wave_digit(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit())
}

// ---- `__instruments__` -----------------------------------------------------

type InstTable = (Vec<Instrument>, BTreeMap<String, u8>, Master);

fn parse_instruments_section(text: &str) -> Result<InstTable, Error> {
    const SEC: &str = "__instruments__";
    let mut list: Vec<Instrument> = Vec::new();
    let mut by_name: BTreeMap<String, u8> = BTreeMap::new();
    let mut master: Option<Master> = None;

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let body = clean(raw);
        if body.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = body.split_whitespace().collect();
        if tokens[0].eq_ignore_ascii_case("master") {
            if master.is_some() {
                return Err(cart_err(
                    SEC,
                    line,
                    "a cart may declare at most one `master` line",
                ));
            }
            master = Some(parse_master_line(line, &tokens)?);
            continue;
        }
        if !tokens[0].eq_ignore_ascii_case("inst") {
            return Err(cart_err(
                SEC,
                line,
                format!(
                    "expected `inst <name> wave=<0-5> ...` or \
                     `master drive=<0-{MAX_DRIVE}> ...`, found {:?}",
                    tokens[0]
                ),
            ));
        }
        let inst = parse_inst_line(line, &tokens)?;
        if by_name.contains_key(&inst.name) {
            return Err(cart_err(
                SEC,
                line,
                format!("duplicate instrument name {:?}", inst.name),
            ));
        }
        if list.len() > usize::from(u8::MAX) {
            return Err(cart_err(SEC, line, "more than 256 instruments"));
        }
        by_name.insert(inst.name.clone(), list.len() as u8);
        list.push(inst);
    }
    Ok((list, by_name, master.unwrap_or_default()))
}

/// `master drive=<0-8> [tone=<0-8>] [hiss=<0-4>]`.
///
/// Every field is optional, but the line has to say *something*: a bare
/// `master` is almost certainly a typo for a line the author meant to fill in.
fn parse_master_line(line: usize, tokens: &[&str]) -> Result<Master, Error> {
    const SEC: &str = "__instruments__";
    let mut m = Master::OFF;
    let mut seen = false;

    for tok in &tokens[1..] {
        let Some((key, value)) = tok.split_once('=') else {
            return Err(cart_err(
                SEC,
                line,
                format!("unexpected {tok:?} in master line (want `drive=`, `tone=` or `hiss=`)"),
            ));
        };
        match key.to_ascii_lowercase().as_str() {
            "drive" => m.drive = parse_u8_in(SEC, line, "master drive", value, MAX_DRIVE)?,
            "tone" => m.tone = parse_u8_in(SEC, line, "master tone", value, MAX_TONE)?,
            "hiss" => m.hiss = parse_u8_in(SEC, line, "master hiss", value, MAX_HISS)?,
            other => {
                return Err(cart_err(
                    SEC,
                    line,
                    format!("unknown master key {other:?} (want `drive`, `tone` or `hiss`)"),
                ));
            }
        }
        seen = true;
    }

    if !seen {
        return Err(cart_err(
            SEC,
            line,
            format!(
                "master needs at least one of `drive=<0-{MAX_DRIVE}>`, `tone=<0-{MAX_TONE}>` or \
                 `hiss=<0-{MAX_HISS}>`"
            ),
        ));
    }
    Ok(m)
}

fn parse_inst_line(line: usize, tokens: &[&str]) -> Result<Instrument, Error> {
    const SEC: &str = "__instruments__";
    if tokens.len() < 2 {
        return Err(cart_err(
            SEC,
            line,
            "expected `inst <name> wave=<0-5> [env=<a>,<d>,<s>] [vib=<cents>,<rate>,<delay>] \
             [sweep=<semis>,<frames>] [duck=<depth>,<release>]`",
        ));
    }
    let name = tokens[1];
    if !valid_name(name) {
        return Err(cart_err(
            SEC,
            line,
            format!("instrument name {name:?} must match [a-z0-9_]+"),
        ));
    }
    if is_wave_digit(name) {
        return Err(cart_err(
            SEC,
            line,
            format!("instrument name {name:?} must not be a bare wave digit (0-5 already name the built-in waveforms)"),
        ));
    }

    let mut wave: Option<u8> = None;
    let mut env: Option<Env> = None;
    let mut vib: Option<Vib> = None;
    let mut sweep: Option<Sweep> = None;
    let mut duck: Option<Duck> = None;

    for tok in &tokens[2..] {
        let Some((key, value)) = tok.split_once('=') else {
            return Err(cart_err(
                SEC,
                line,
                format!(
                    "unexpected {tok:?} in inst line \
                     (want `wave=`, `env=`, `vib=`, `sweep=` or `duck=`)"
                ),
            ));
        };
        match key.to_ascii_lowercase().as_str() {
            "wave" => wave = Some(parse_u8_in(SEC, line, "wave", value, WAVE_COUNT - 1)?),
            "env" => {
                let parts: Vec<&str> = value.split(',').collect();
                if parts.len() != 3 {
                    return Err(cart_err(
                        SEC,
                        line,
                        format!("env must be `env=<attack>,<decay>,<sustain>`, found {value:?}"),
                    ));
                }
                env = Some(Env {
                    attack: parse_u8_in(SEC, line, "env attack", parts[0], u8::MAX)?,
                    decay: parse_u8_in(SEC, line, "env decay", parts[1], u8::MAX)?,
                    sustain: parse_u8_in(SEC, line, "env sustain", parts[2], MAX_VOL)?,
                });
            }
            "vib" => {
                let parts: Vec<&str> = value.split(',').collect();
                if parts.len() != 3 {
                    return Err(cart_err(
                        SEC,
                        line,
                        format!("vib must be `vib=<cents>,<rate>,<delay>`, found {value:?}"),
                    ));
                }
                vib = Some(Vib {
                    cents: parse_u8_range(SEC, line, "vib cents", parts[0], 1, MAX_VIB_CENTS)?,
                    rate: parse_u8_range(SEC, line, "vib rate", parts[1], 1, MAX_VIB_RATE)?,
                    delay: parse_u8_in(SEC, line, "vib delay", parts[2], u8::MAX)?,
                });
            }
            "sweep" => {
                let Some((a, b)) = value.split_once(',') else {
                    return Err(cart_err(
                        SEC,
                        line,
                        format!("sweep must be `sweep=<semis>,<frames>`, found {value:?}"),
                    ));
                };
                let semis = parse_i32_in(SEC, line, "sweep semitones", a, -96, 96)?;
                let frames = parse_u8_range(SEC, line, "sweep frames", b, 1, u8::MAX)?;
                sweep = Some(Sweep {
                    semis: semis as i8,
                    frames,
                });
            }
            "duck" => {
                let Some((d, r)) = value.split_once(',') else {
                    return Err(cart_err(
                        SEC,
                        line,
                        format!("duck must be `duck=<depth>,<release>`, found {value:?}"),
                    ));
                };
                duck = Some(Duck {
                    depth: parse_u8_range(SEC, line, "duck depth", d, 1, MAX_DUCK_DEPTH)?,
                    release: parse_u8_range(SEC, line, "duck release", r, 1, u8::MAX)?,
                });
            }
            other => {
                return Err(cart_err(
                    SEC,
                    line,
                    format!(
                        "unknown inst key {other:?} (want `wave`, `env`, `vib`, `sweep` or `duck`)"
                    ),
                ));
            }
        }
    }

    let Some(wave) = wave else {
        return Err(cart_err(
            SEC,
            line,
            format!("instrument {name} is missing `wave=<0-5>`"),
        ));
    };
    Ok(Instrument {
        name: name.to_string(),
        wave,
        env,
        vib,
        sweep,
        duck,
    })
}

/// `parse_u8_in` with a lower bound as well.
fn parse_u8_range(
    section: &str,
    line: usize,
    what: &str,
    text: &str,
    min: u8,
    max: u8,
) -> Result<u8, Error> {
    let v = parse_i32_in(section, line, what, text, i32::from(min), i32::from(max))?;
    Ok(v as u8)
}

// ---- the effect column -----------------------------------------------------

/// Parse the optional 4th token of a note row.
fn parse_fx(line: usize, token: &str, inst: Option<&Instrument>) -> Result<Fx, Error> {
    const SEC: &str = "__sfx__";
    let tok = token.to_ascii_lowercase();

    if let Some(v) = tok.strip_prefix("arp") {
        let Some((a, b)) = v.split_once(',') else {
            return Err(cart_err(
                SEC,
                line,
                format!("arp must be `arp<a>,<b>` (semitones), found {token:?}"),
            ));
        };
        let a = parse_i32_in(SEC, line, "arp first offset", a, 0, MAX_FX_SEMIS)?;
        let b = parse_i32_in(SEC, line, "arp second offset", b, 0, MAX_FX_SEMIS)?;
        return Ok(Fx::Arp {
            a: a as u8,
            b: b as u8,
        });
    }
    if let Some(v) = tok.strip_prefix("sl") {
        let semis = parse_i32_in(SEC, line, "slide semitones", v, -MAX_FX_SEMIS, MAX_FX_SEMIS)?;
        return Ok(Fx::Slide {
            semis: semis as i8,
        });
    }
    if let Some(v) = tok.strip_prefix("vib") {
        if v.is_empty() {
            let Some(vib) = inst.and_then(|i| i.vib) else {
                return Err(cart_err(
                    SEC,
                    line,
                    match inst {
                        Some(i) => format!(
                            "bare `vib` needs the row's instrument {:?} to declare `vib=<cents>,<rate>,<delay>`",
                            i.name
                        ),
                        None => "bare `vib` needs the row to name an instrument with `vib=<cents>,<rate>,<delay>`"
                            .to_string(),
                    },
                ));
            };
            return Ok(Fx::Vibrato(vib));
        }
        let Some((c, r)) = v.split_once(',') else {
            return Err(cart_err(
                SEC,
                line,
                format!("vib must be `vib` or `vib<cents>,<rate>`, found {token:?}"),
            ));
        };
        return Ok(Fx::Vibrato(Vib {
            cents: parse_u8_range(SEC, line, "vib cents", c, 1, MAX_VIB_CENTS)?,
            rate: parse_u8_range(SEC, line, "vib rate", r, 1, MAX_VIB_RATE)?,
            delay: 0,
        }));
    }
    if let Some(v) = tok.strip_prefix("fade") {
        let levels = parse_i32_in(
            SEC,
            line,
            "fade levels",
            v,
            -i32::from(MAX_VOL),
            i32::from(MAX_VOL),
        )?;
        return Ok(Fx::Fade {
            levels: levels as i8,
        });
    }
    Err(cart_err(
        SEC,
        line,
        format!("unknown effect {token:?} (want `arp<a>,<b>`, `sl<n>`, `vib[<cents>,<rate>]` or `fade<n>`)"),
    ))
}

// ---- `__sfx__` -------------------------------------------------------------

fn parse_sfx_section(
    text: &str,
    instruments: &[Instrument],
    inst_by_name: &BTreeMap<String, u8>,
    tempo: Option<Tempo>,
) -> Result<BTreeMap<u8, Sfx>, Error> {
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
            current = Some(parse_sfx_header(line, &tokens, tempo)?);
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
            sfx.mods.push(RowMod::default());
            continue;
        }
        if tokens.len() < 3 || tokens.len() > 4 {
            return Err(cart_err(
                SEC,
                line,
                format!("expected `NOTE WAVE VOL [FX]` or `---`, found {body:?}"),
            ));
        }
        let note = parse_note(tokens[0]).ok_or_else(|| {
            cart_err(
                SEC,
                line,
                format!("bad note {:?} (expected C0-B7, e.g. `C#4`)", tokens[0]),
            )
        })?;
        // Column 2 is either a bare wave digit (PoC v1) or an instrument name.
        let (wave, inst_index, inst) = if is_wave_digit(tokens[1]) {
            (
                parse_u8_in(SEC, line, "wave", tokens[1], WAVE_COUNT - 1)?,
                None,
                None,
            )
        } else {
            let Some(&idx) = inst_by_name.get(tokens[1]) else {
                return Err(cart_err(
                    SEC,
                    line,
                    format!(
                        "unknown instrument {:?} ({})",
                        tokens[1],
                        instrument_hint(instruments)
                    ),
                ));
            };
            let inst = &instruments[usize::from(idx)];
            (inst.wave, Some(idx), Some(inst))
        };
        let vol = parse_u8_in(SEC, line, "vol", tokens[2], MAX_VOL)?;
        let fx = match tokens.get(3) {
            Some(tok) => Some(parse_fx(line, tok, inst)?),
            None => None,
        };
        sfx.rows.push(SfxRow::Note { note, wave, vol });
        sfx.mods.push(RowMod {
            inst: inst_index,
            fx,
        });
    }

    if let Some((id, sfx, hdr)) = current.take() {
        finish_sfx(&mut out, id, sfx, hdr)?;
    }
    Ok(out)
}

/// "this cart defines no instruments" / "defined: a, b, c".
fn instrument_hint(instruments: &[Instrument]) -> String {
    if instruments.is_empty() {
        "this cart has no __instruments__ section; column 2 must be a wave digit 0-5".to_string()
    } else {
        let names: Vec<&str> = instruments.iter().map(|i| i.name.as_str()).collect();
        format!(
            "want a wave digit 0-5 or one of: {}",
            names.join(", ")
        )
    }
}

fn parse_sfx_header(
    line: usize,
    tokens: &[&str],
    tempo: Option<Tempo>,
) -> Result<(u8, Sfx, usize), Error> {
    const SEC: &str = "__sfx__";
    if tokens.len() < 2 {
        return Err(cart_err(
            SEC,
            line,
            "expected `sfx <id 0-63> speed=<1-255|auto> [loop=<start>,<end>]`",
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
            "speed" if value.eq_ignore_ascii_case("auto") => {
                let Some(t) = tempo else {
                    return Err(cart_err(
                        SEC,
                        line,
                        "speed=auto needs a `bpm=<n> [rows_per_beat=<r>]` line at the top of __music__",
                    ));
                };
                speed = Some(t.speed);
            }
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
            mods: Vec::new(),
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

/// True for the optional `bpm=...` header line of `__music__`.
fn is_tempo_line(body: &str) -> bool {
    // `get` rather than a slice: the line may be arbitrary UTF-8.
    body.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("bpm="))
}

/// Read `bpm=<n> [rows_per_beat=<r>]`, which may only be `__music__`'s first
/// content line. Absent -> `None`, and then `speed=auto` is an error.
fn parse_tempo_line(text: &str) -> Result<Option<Tempo>, Error> {
    const SEC: &str = "__music__";
    for (i, raw) in text.lines().enumerate() {
        let body = clean(raw);
        if body.is_empty() {
            continue;
        }
        if !is_tempo_line(body) {
            return Ok(None);
        }
        let line = i + 1;
        let tokens: Vec<&str> = body.split_whitespace().collect();
        let bpm = parse_i32_in(SEC, line, "bpm", &tokens[0][4..], 1, 1000)? as u32;
        let mut rows_per_beat = 4u8;
        for tok in &tokens[1..] {
            let Some((key, value)) = tok.split_once('=') else {
                return Err(cart_err(
                    SEC,
                    line,
                    format!("unexpected {tok:?} in tempo line (want `rows_per_beat=<r>`)"),
                ));
            };
            match key.to_ascii_lowercase().as_str() {
                "rows_per_beat" => {
                    rows_per_beat = parse_u8_range(SEC, line, "rows_per_beat", value, 1, 16)?;
                }
                other => {
                    return Err(cart_err(
                        SEC,
                        line,
                        format!("unknown tempo key {other:?} (want `rows_per_beat`)"),
                    ));
                }
            }
        }
        // round(3600 / (bpm * rows_per_beat)), half away from zero.
        let den = bpm * u32::from(rows_per_beat);
        let speed = (2 * 3600 + den) / (2 * den);
        if !(1..=255).contains(&speed) {
            return Err(cart_err(
                SEC,
                line,
                format!(
                    "bpm={bpm} rows_per_beat={rows_per_beat} gives speed={speed}, outside 1-255"
                ),
            ));
        }
        return Ok(Some(Tempo {
            bpm,
            rows_per_beat,
            speed: speed as u8,
        }));
    }
    Ok(None)
}

fn parse_music_section(text: &str, sfx: &BTreeMap<u8, Sfx>) -> Result<MusicParse, Error> {
    const SEC: &str = "__music__";
    let mut out: BTreeMap<u8, Pattern> = BTreeMap::new();
    let mut lines_of = Vec::new();
    let mut seen_content = false;

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let body = clean(raw);
        if body.is_empty() {
            continue;
        }
        if is_tempo_line(body) {
            if seen_content {
                return Err(cart_err(
                    SEC,
                    line,
                    "the `bpm=` line must be the first line of __music__",
                ));
            }
            seen_content = true;
            continue; // already validated by parse_tempo_line
        }
        seen_content = true;
        let Some((head, tail)) = body.split_once(':') else {
            return Err(cart_err(
                SEC,
                line,
                format!(
                    "expected `pat <id> [stop|loop=<id>] : ch0 ch1 ch2 ch3 [ch4 ch5]`, found {body:?}"
                ),
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

        // 4 slots (the pre-6-channel format) through 6 (one per channel).
        // Trailing slots a line omits stay `None`, i.e. silent.
        let slot_tokens: Vec<&str> = tail.split_whitespace().collect();
        if !(MIN_PATTERN_SLOTS..=CHANNEL_COUNT).contains(&slot_tokens.len()) {
            return Err(cart_err(
                SEC,
                line,
                format!(
                    "expected {MIN_PATTERN_SLOTS}-{CHANNEL_COUNT} channel slots after `:`, found {}",
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

/// Per-row, per-frame modulation: everything an instrument's envelope /
/// vibrato / sweep and the row's effect column contribute after note-on.
///
/// Only present when the row actually asks for modulation — a bare wave digit
/// with no effect leaves this `None` and takes the PoC v1 code path exactly.
#[derive(Debug, Clone, Copy)]
struct Modulation {
    /// Base note of the row.
    note: u8,
    /// The row's authored volume (the envelope's peak).
    vol: u8,
    /// Row duration in frames (the sfx speed): the span `sl` and `fade` use.
    dur: u32,
    /// Frames since note-on; 0 on the frame the row starts.
    frame: u32,
    env: Option<Env>,
    vib: Option<Vib>,
    sweep: Option<Sweep>,
    fx: Option<Fx>,
}

impl Modulation {
    /// Semitone offset from the base note this frame (sweep + arp/slide).
    fn semitone_offset(&self) -> f32 {
        let mut off = 0.0f32;
        if let Some(sw) = self.sweep {
            // `.max(1)`: the parser rejects 0 frames, but `Sweep` is a public
            // struct and this must never divide by zero.
            let n = u32::from(sw.frames).max(1);
            let p = self.frame.min(n);
            off += f32::from(sw.semis) * (p as f32 / n as f32);
        }
        match self.fx {
            Some(Fx::Arp { a, b }) => {
                let step = (self.frame / ARP_FRAMES_PER_STEP) % 3;
                off += f32::from(match step {
                    0 => 0,
                    1 => a,
                    _ => b,
                });
            }
            Some(Fx::Slide { semis }) => {
                // Linear across the row: the full offset lands exactly on the
                // next row's boundary (`frame == dur`), so a slide glides into
                // whatever the next row plays.
                let dur = self.dur.max(1);
                let p = self.frame.min(dur);
                off += f32::from(semis) * (p as f32 / dur as f32);
            }
            _ => {}
        }
        off
    }

    /// The row instrument's vibrato, or the one the effect column overrode it
    /// with.
    fn active_vib(&self) -> Option<Vib> {
        match self.fx {
            Some(Fx::Vibrato(v)) => Some(v),
            _ => self.vib,
        }
    }

    /// This frame's frequency in Hz.
    fn freq(&self) -> f32 {
        let hz = freq_at(self.note, self.semitone_offset());
        match self.active_vib() {
            Some(v) => {
                let lfo = v.value_at(self.frame);
                hz * (1.0 + lfo * f32::from(v.cents) * CENTS_TO_RATIO)
            }
            None => hz,
        }
    }

    /// This frame's volume level 0..=7 (envelope, then the fade effect).
    fn level(&self) -> u8 {
        let v = i32::from(self.vol);
        let f = self.frame as i32;
        let mut level = match self.env {
            None => v,
            Some(e) => {
                let a = i32::from(e.attack);
                let d = i32::from(e.decay);
                let s = i32::from(e.sustain);
                if f < a {
                    div_round(v * (f + 1), a)
                } else if f < a + d {
                    v + div_round((s - v) * (f - a + 1), d)
                } else {
                    s
                }
            }
        };
        if let Some(Fx::Fade { levels }) = self.fx {
            // The ramp completes *within* the row: `fade-7` really does reach
            // silence on the row's last frame.
            let span = self.dur.max(2) as i32 - 1;
            level += div_round(i32::from(levels) * f.min(span), span);
        }
        level.clamp(0, i32::from(MAX_VOL)) as u8
    }
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
    /// Per-frame modulation for the row currently playing, if it needs any.
    md: Option<Modulation>,
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
            md: None,
        }
    }

    /// Retune without the click guard.
    ///
    /// Unlike a waveform change, a frequency change is inaudible as a
    /// discontinuity: the phase accumulator keeps running, so the output stays
    /// continuous. Vibrato, slides, arpeggios and sweeps all use this so they
    /// are not chopped into 64-sample fade-outs.
    fn set_inc(&mut self, inc: u32) {
        match &mut self.pending {
            Some(p) => p.0 = inc,
            None => self.inc = inc,
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
        self.md = None;
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
    /// The live master setting: the cart's `master` line until Lua's
    /// `master()` overrides it.
    master: Master,
    /// Master bus memory (tone filter, hiss LFSR).
    mstate: MasterState,
    /// The single global sidechain duck envelope.
    duck: DuckBus,
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
        let master = bank.master();
        Audio {
            bank,
            channels: [const { Channel::new() }; CHANNEL_COUNT],
            music: None,
            lfsr: LFSR_SEED,
            master,
            mstate: MasterState::new(),
            duck: DuckBus::new(),
            out: Box::new([0.0; SAMPLES_PER_FRAME]),
        }
    }

    /// The master bus setting in force right now: the cart's `master` line
    /// unless Lua's `master()` has overridden it.
    pub fn master(&self) -> Master {
        self.master
    }

    /// Current sidechain attenuation (0 = not ducking) and the channel that is
    /// exempt from it. Exposed for tests and host inspection.
    pub fn duck_state(&self) -> (f32, Option<u8>) {
        (self.duck.atten, self.duck.trigger_ch)
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
    /// `ch == -1`). `ch == -1` auto-picks a channel: see [`Audio::auto_channel`].
    /// Explicit channels are `0..=5`.
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

    /// `master(drive, [tone], [hiss])`: replace the cart's `__instruments__`
    /// master line for the rest of the session. Omitted arguments are 0, so
    /// `master(0)` is a full reset to the clean legacy output path.
    ///
    /// Takes effect immediately, like `sfx()`/`music()`: a call from `_update`
    /// colours the same frame's 735 samples. The tone filter's memory and the
    /// hiss LFSR are *not* reset — only `Console::new` does that — so flipping
    /// the setting mid-note is a change of processing, not a restart.
    pub fn lua_master(&mut self, drive: i32, tone: i32, hiss: i32) -> Result<(), String> {
        for (what, v, max) in [
            ("drive", drive, MAX_DRIVE),
            ("tone", tone, MAX_TONE),
            ("hiss", hiss, MAX_HISS),
        ] {
            if !(0..=i32::from(max)).contains(&v) {
                return Err(format!(
                    "master: {what} {v} out of range (expected 0-{max})"
                ));
            }
        }
        self.master = Master {
            drive: drive as u8,
            tone: tone as u8,
            hiss: hiss as u8,
        };
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

    /// The lowest channel busy with neither music nor a sfx — so auto-allocated
    /// blips land on channels a song's pattern did not claim. When every
    /// channel is busy the highest one ([`CHANNEL_COUNT`] - 1, i.e. channel 5)
    /// is stolen, music or not.
    fn auto_channel(&self) -> usize {
        self.channels
            .iter()
            .position(|c| c.owner == Owner::Free)
            .unwrap_or(CHANNEL_COUNT - 1)
    }

    fn start_sfx(&mut self, ch: usize, id: u8, owner: Owner) {
        let Audio {
            bank,
            channels,
            duck,
            ..
        } = self;
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
        apply_row(c, ch, sfx, 0, &bank.instruments, duck);
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
        let Audio {
            bank,
            channels,
            duck,
            ..
        } = self;
        for (ch, c) in channels.iter_mut().enumerate() {
            let Some(mut cur) = c.cursor else { continue };
            cur.frames_left -= 1;
            if cur.frames_left > 0 {
                c.cursor = Some(cur);
                // Same row, next frame: envelope / vibrato / sweep / fx move.
                tick_modulation(c);
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
                    c.md = None;
                    c.set_vol(0);
                } else {
                    c.stop();
                }
                continue;
            }
            cur.row = next;
            cur.frames_left = u32::from(sfx.speed);
            c.cursor = Some(cur);
            apply_row(c, ch, sfx, usize::from(next), &bank.instruments, duck);
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
    ///
    /// The signal path, in order:
    ///
    /// ```text
    /// channels -> duck gain -> sum * 0.25 -> drive/shaper -> tone LP -> hiss -> clamp
    /// ```
    ///
    /// The **insertion point** is the `acc * MIX_GAIN` product: everything new
    /// consumes that value and replaces the plain `clamp(-1, 1)` that used to
    /// be applied to it (the clamp survives as a final safety net, because the
    /// hiss adds after the shaper has already bounded the signal). Ducking
    /// sits one step earlier still, on the per-channel samples going into the
    /// sum, so a driven mix pumps — the shaper sees the ducked signal.
    ///
    /// When nothing is engaged — no `master` line / `master(0)` *and* no duck
    /// envelope running — the loop is the PoC v1 statement, character for
    /// character, so old carts render bit-identical samples. (The general path
    /// would agree anyway: its duck gain is exactly `1.0` when idle and `x *
    /// 1.0 == x` in IEEE-754. The split is for clarity and speed, not safety.)
    pub fn render(&mut self) {
        let Audio {
            channels,
            lfsr,
            master,
            mstate,
            duck,
            out,
            ..
        } = self;

        if master.is_bypass() && duck.is_idle() {
            for slot in out.iter_mut() {
                let mut acc = 0.0f32;
                for c in channels.iter_mut() {
                    acc += c.next_sample(lfsr);
                }
                *slot = (acc * MIX_GAIN).clamp(-1.0, 1.0);
            }
            return;
        }

        let pre = PRE_GAIN[usize::from(master.drive)];
        let makeup = MAKEUP[usize::from(master.drive)];
        let a = TONE_A[usize::from(master.tone)];
        let hiss = HISS_LEVEL[usize::from(master.hiss)];

        for slot in out.iter_mut() {
            // ---- sidechain duck, then the channel sum ----------------------
            let g = duck.gain();
            let exempt = duck.trigger_ch;
            let mut acc = 0.0f32;
            for (i, c) in channels.iter_mut().enumerate() {
                let s = c.next_sample(lfsr);
                acc += if exempt == Some(i as u8) { s } else { s * g };
            }
            duck.tick();

            let mut v = acc * MIX_GAIN;

            // ---- drive: pre-gain, soft clip, makeup ------------------------
            if master.drive != 0 {
                v = shape(v * pre) * makeup;
            }

            // ---- tone: one-pole lowpass ------------------------------------
            if master.tone != 0 {
                mstate.y += a * (v - mstate.y);
                if mstate.y.abs() < DENORM_FLOOR {
                    mstate.y = 0.0;
                }
                v = mstate.y;
            } else {
                // Keep the memory tracking the signal even while the filter is
                // bypassed, so turning `tone` on mid-note does not thump.
                mstate.y = v;
            }

            // ---- hiss: dedicated LFSR, clocked every sample ----------------
            // Unconditionally advanced while the bus is engaged, so the noise
            // floor is a function of elapsed time and not of what the channels
            // happen to be doing.
            mstate.lfsr = lfsr_next(mstate.lfsr);
            if master.hiss != 0 {
                v += if mstate.lfsr & 1 != 0 { hiss } else { -hiss };
            }

            *slot = v.clamp(-1.0, 1.0);
        }
    }

    /// Zero the output buffer (used when a frame halts).
    pub fn silence(&mut self) {
        self.out.fill(0.0);
    }
}

/// Start `row` on channel `ch`: set the voice, arm this row's modulation (if
/// any) and fire the sidechain if the row's instrument is a duck trigger.
fn apply_row(
    c: &mut Channel,
    ch: usize,
    sfx: &Sfx,
    row: usize,
    instruments: &[Instrument],
    duck: &mut DuckBus,
) {
    match sfx.rows[row] {
        SfxRow::Rest => {
            c.md = None;
            c.set_vol(0);
        }
        SfxRow::Note { note, wave, vol } => {
            let m = sfx.row_mod(row);
            let named = m.inst.and_then(|i| instruments.get(usize::from(i)));
            // Any note-on of a `duck=` instrument re-fires the envelope, even a
            // silent one: the row is what triggers, not the audible level.
            if let Some(d) = named.and_then(|i| i.duck) {
                duck.trigger(ch, d);
            }
            let inst = named.filter(|i| !i.is_flat());
            if inst.is_none() && m.fx.is_none() {
                // PoC v1 path, bit-for-bit: no per-frame work at all.
                c.md = None;
                c.set_voice(note_increment(note), wave, vol);
                return;
            }
            let md = Modulation {
                note,
                vol,
                dur: u32::from(sfx.speed),
                frame: 0,
                env: inst.and_then(|i| i.env),
                vib: inst.and_then(|i| i.vib),
                sweep: inst.and_then(|i| i.sweep),
                fx: m.fx,
            };
            c.set_voice(inc_from_hz(md.freq()), wave, md.level());
            c.md = Some(md);
        }
    }
}

/// Advance a held row's modulation by one frame and re-apply it.
fn tick_modulation(c: &mut Channel) {
    let Some(mut md) = c.md else { return };
    md.frame += 1;
    c.md = Some(md);
    c.set_inc(inc_from_hz(md.freq()));
    c.set_vol(md.level());
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

    // -----------------------------------------------------------------
    // PoC v2: pitch math, LFO, envelope and effect trajectories
    // -----------------------------------------------------------------

    #[test]
    fn whole_semitones_hit_the_table_exactly() {
        for note in 0..96u8 {
            assert_eq!(freq_at(note, 0.0).to_bits(), NOTE_FREQ[note as usize].to_bits());
        }
        // An unmodulated note must produce the very same increment as v1.
        for note in 0..96u8 {
            assert_eq!(inc_from_hz(freq_at(note, 0.0)), note_increment(note));
        }
        assert_eq!(freq_at(45, 12.0), NOTE_FREQ[57]);
        assert_eq!(freq_at(57, -12.0), NOTE_FREQ[45]);
    }

    #[test]
    fn fractional_semitones_interpolate_linearly() {
        // s = k + f resolves as NOTE_FREQ[k]*(1-f) + NOTE_FREQ[k+1]*f.
        let want = NOTE_FREQ[57] * 0.5 + NOTE_FREQ[58] * 0.5;
        assert_eq!(freq_at(57, 0.5), want);
        let want = NOTE_FREQ[57] * 0.75 + NOTE_FREQ[58] * 0.25;
        assert_eq!(freq_at(57, 0.25), want);
        // Interpolation is monotonic and strictly between the neighbours.
        let mut prev = NOTE_FREQ[57];
        for i in 1..=16 {
            let f = freq_at(57, i as f32 / 16.0);
            assert!(f > prev, "not monotonic at {i}");
            assert!(f <= NOTE_FREQ[58]);
            prev = f;
        }
    }

    #[test]
    fn pitch_clamps_at_both_ends_of_the_table() {
        assert_eq!(freq_at(0, -5.0), NOTE_FREQ[0]);
        assert_eq!(freq_at(0, -0.5), NOTE_FREQ[0]);
        assert_eq!(freq_at(95, 5.0), NOTE_FREQ[95]);
        assert_eq!(freq_at(95, 0.5), NOTE_FREQ[95]);
        assert_eq!(freq_at(94, 1.0), NOTE_FREQ[95]);
    }

    #[test]
    fn lfo_is_an_exact_integer_triangle() {
        assert_eq!(lfo_triangle(0), 0.0);
        assert_eq!(lfo_triangle(8), 0.5);
        assert_eq!(lfo_triangle(16), 1.0);
        assert_eq!(lfo_triangle(32), 0.0);
        assert_eq!(lfo_triangle(48), -1.0);
        assert_eq!(lfo_triangle(56), -0.5);
        assert_eq!(lfo_triangle(64), 0.0);
        for p in 0..256 {
            let v = lfo_triangle(p);
            assert!((-1.0..=1.0).contains(&v));
            assert_eq!(v, lfo_triangle(p + LFO_STEPS));
        }
    }

    #[test]
    fn vib_period_is_exactly_64_over_rate_frames() {
        // rate = LFO phase units per frame, 64 units per cycle.
        for rate in 1..=MAX_VIB_RATE {
            let v = Vib {
                cents: 50,
                rate,
                delay: 0,
            };
            // The phase sequence repeats after 64 frames for every rate...
            for f in 0..200u32 {
                assert_eq!(v.value_at(f), v.value_at(f + LFO_STEPS));
            }
            // ...and one cycle is 64/rate frames, exact whenever rate | 64.
            if LFO_STEPS % u32::from(rate) == 0 {
                let period = LFO_STEPS / u32::from(rate);
                assert_eq!(v.value_at(0), 0.0);
                assert_eq!(v.value_at(period / 4), 1.0, "rate {rate} peak");
                assert_eq!(v.value_at(period / 2), 0.0);
                assert_eq!(v.value_at(period * 3 / 4), -1.0);
                for f in 0..64u32 {
                    assert_eq!(v.value_at(f), v.value_at(f + period));
                }
            }
        }
        // The delay holds the LFO at zero, then it starts from phase 0.
        let v = Vib {
            cents: 50,
            rate: 4,
            delay: 10,
        };
        for f in 0..10 {
            assert_eq!(v.value_at(f), 0.0);
        }
        assert_eq!(v.value_at(10), 0.0);
        assert_eq!(v.value_at(14), 1.0);
    }

    fn md(vol: u8, dur: u32) -> Modulation {
        Modulation {
            note: 57, // A4
            vol,
            dur,
            frame: 0,
            env: None,
            vib: None,
            sweep: None,
            fx: None,
        }
    }

    fn trajectory<T>(m: Modulation, frames: u32, f: impl Fn(&Modulation) -> T) -> Vec<T> {
        (0..frames)
            .map(|i| {
                let mut m = m;
                m.frame = i;
                f(&m)
            })
            .collect()
    }

    #[test]
    fn envelope_attack_decay_sustain_are_whole_frame_ramps() {
        // attack 4: 0 -> vol over four frames, reaching vol on frame 3.
        let mut m = md(7, 32);
        m.env = Some(Env {
            attack: 4,
            decay: 4,
            sustain: 3,
        });
        let levels = trajectory(m, 12, Modulation::level);
        assert_eq!(levels, vec![2, 4, 5, 7, 6, 5, 4, 3, 3, 3, 3, 3]);

        // attack 0 = start at the row volume, decay 0 = jump to sustain.
        let mut m = md(6, 32);
        m.env = Some(Env {
            attack: 0,
            decay: 0,
            sustain: 2,
        });
        assert_eq!(trajectory(m, 4, Modulation::level), vec![2, 2, 2, 2]);

        // The percussion shape: straight down to silence over `decay` frames.
        let mut m = md(6, 32);
        m.env = Some(Env {
            attack: 0,
            decay: 6,
            sustain: 0,
        });
        let levels = trajectory(m, 8, Modulation::level);
        assert_eq!(levels, vec![5, 4, 3, 2, 1, 0, 0, 0]);

        // No envelope at all is flat at the row volume.
        assert_eq!(trajectory(md(5, 32), 4, Modulation::level), vec![5; 4]);
    }

    #[test]
    fn sweep_is_a_linear_semitone_ramp_that_then_holds() {
        let mut m = md(7, 64);
        m.sweep = Some(Sweep {
            semis: -12,
            frames: 6,
        });
        let offs = trajectory(m, 9, Modulation::semitone_offset);
        assert_eq!(offs, vec![0.0, -2.0, -4.0, -6.0, -8.0, -10.0, -12.0, -12.0, -12.0]);
        // Halfway through the sweep the frequency is the linear interpolation
        // of the two neighbouring table entries.
        let mut half = m;
        half.frame = 3;
        assert_eq!(half.freq(), freq_at(57, -6.0));
    }

    #[test]
    fn arp_offsets_switch_every_two_frames() {
        let mut m = md(6, 32);
        m.fx = Some(Fx::Arp { a: 4, b: 7 });
        let offs = trajectory(m, 14, Modulation::semitone_offset);
        assert_eq!(
            offs,
            vec![0.0, 0.0, 4.0, 4.0, 7.0, 7.0, 0.0, 0.0, 4.0, 4.0, 7.0, 7.0, 0.0, 0.0]
        );
        assert_eq!(ARP_FRAMES_PER_STEP, 2);
        // Each step lands exactly on a table entry (whole semitones).
        let mut two = m;
        two.frame = 2;
        assert_eq!(two.freq(), NOTE_FREQ[61]);
    }

    #[test]
    fn slide_is_linear_across_the_row_and_arrives_at_the_boundary() {
        let mut m = md(6, 8);
        m.fx = Some(Fx::Slide { semis: 12 });
        let offs = trajectory(m, 9, Modulation::semitone_offset);
        assert_eq!(offs, vec![0.0, 1.5, 3.0, 4.5, 6.0, 7.5, 9.0, 10.5, 12.0]);
        // Midpoint frequency is the linear interpolation between A4+6 = D#5
        // and E5 (offset 6.0 is a whole semitone, so it is exactly D#5).
        let mut mid = m;
        mid.frame = 4;
        assert_eq!(mid.freq(), NOTE_FREQ[63]);
        // A quarter of the way: 3 semitones exactly.
        let mut q = m;
        q.frame = 2;
        assert_eq!(q.freq(), NOTE_FREQ[60]);
        // Fractional: 1.5 semitones above A4.
        let mut f = m;
        f.frame = 1;
        assert_eq!(f.freq(), NOTE_FREQ[58] * 0.5 + NOTE_FREQ[59] * 0.5);
        // Downward slides mirror.
        let mut dn = md(6, 4);
        dn.fx = Some(Fx::Slide { semis: -4 });
        assert_eq!(
            trajectory(dn, 5, Modulation::semitone_offset),
            vec![0.0, -1.0, -2.0, -3.0, -4.0]
        );
    }

    #[test]
    fn fade_reaches_its_endpoint_on_the_rows_last_frame() {
        let mut m = md(7, 8);
        m.fx = Some(Fx::Fade { levels: -7 });
        let levels = trajectory(m, 8, Modulation::level);
        assert_eq!(*levels.first().unwrap(), 7);
        assert_eq!(*levels.last().unwrap(), 0, "fade-7 from 7 must reach 0");
        assert_eq!(levels, vec![7, 6, 5, 4, 3, 2, 1, 0]);

        // Upward, and clamped at the ceiling.
        let mut up = md(2, 5);
        up.fx = Some(Fx::Fade { levels: 4 });
        assert_eq!(trajectory(up, 5, Modulation::level), vec![2, 3, 4, 5, 6]);
        let mut over = md(6, 5);
        over.fx = Some(Fx::Fade { levels: 7 });
        assert_eq!(trajectory(over, 5, Modulation::level), vec![6, 7, 7, 7, 7]);
        // A one-frame row is just the start value.
        let mut one = md(4, 1);
        one.fx = Some(Fx::Fade { levels: -4 });
        assert_eq!(trajectory(one, 1, Modulation::level), vec![4]);
    }

    #[test]
    fn vibrato_depth_is_the_cents_factor_times_the_lfo() {
        let mut m = md(6, 64);
        m.vib = Some(Vib {
            cents: 100,
            rate: 4,
            delay: 0,
        });
        // Frame 0 is the zero crossing of the triangle: no bend at all.
        assert_eq!(m.freq(), NOTE_FREQ[57]);
        // Quarter period (rate 4 -> 16-frame period) is the positive peak.
        let mut peak = m;
        peak.frame = 4;
        assert_eq!(peak.freq(), NOTE_FREQ[57] * (1.0 + 100.0 * CENTS_TO_RATIO));
        let mut trough = m;
        trough.frame = 12;
        assert_eq!(trough.freq(), NOTE_FREQ[57] * (1.0 - 100.0 * CENTS_TO_RATIO));
        // 100 cents is a semitone to within the linear approximation's error.
        let semitone = f64::from(NOTE_FREQ[58] / NOTE_FREQ[57]);
        let approx = f64::from(peak.freq() / NOTE_FREQ[57]);
        assert!((semitone - approx).abs() < 0.003, "{semitone} vs {approx}");
        // The effect column overrides the instrument's vibrato.
        let mut over = m;
        over.fx = Some(Fx::Vibrato(Vib {
            cents: 50,
            rate: 4,
            delay: 0,
        }));
        over.frame = 4;
        assert_eq!(over.freq(), NOTE_FREQ[57] * (1.0 + 50.0 * CENTS_TO_RATIO));
    }

    #[test]
    fn sweep_and_effects_stack_on_the_same_row() {
        let mut m = md(6, 8);
        m.sweep = Some(Sweep {
            semis: 4,
            frames: 4,
        });
        m.fx = Some(Fx::Arp { a: 3, b: 7 });
        // frame 2: sweep is halfway (+2) and the arp is on its second step.
        let mut f2 = m;
        f2.frame = 2;
        assert_eq!(f2.semitone_offset(), 5.0);
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(div_round(3, 2), 2);
        assert_eq!(div_round(-3, 2), -2);
        assert_eq!(div_round(1, 2), 1);
        assert_eq!(div_round(-1, 2), -1);
        assert_eq!(div_round(0, 3), 0);
        assert_eq!(div_round(7, 3), 2);
        assert_eq!(div_round(-7, 3), -2);
    }

    // -----------------------------------------------------------------
    // Master bus: shaper, makeup, tone table, hiss
    // -----------------------------------------------------------------

    /// The shaper written out in f64, exactly as the doc comment and the
    /// offline generator spell it.
    fn shape64(x: f64) -> f64 {
        if x >= 3.0 {
            1.0
        } else if x <= -3.0 {
            -1.0
        } else {
            x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
        }
    }

    #[test]
    fn shaper_is_odd_monotonic_and_bounded() {
        // Fixed points and the documented clip point.
        assert_eq!(shape(0.0), 0.0);
        assert_eq!(shape(3.0), 1.0);
        assert_eq!(shape(-3.0), -1.0);
        assert_eq!(shape(1000.0), 1.0);
        assert_eq!(shape(-1000.0), -1.0);
        // R(1) = 1*(27+1)/(27+9) = 28/36 = 7/9.
        assert_eq!(shape(1.0), 28.0 / 36.0);

        let mut prev = f32::NEG_INFINITY;
        for i in -4000..=4000 {
            let x = i as f32 * 0.001;
            let y = shape(x);
            // Odd symmetry, to the bit: every operation in the formula is
            // sign-symmetric.
            assert_eq!(y, -shape(-x), "not odd at {x}");
            assert!((-1.0..=1.0).contains(&y), "shape({x}) = {y} escaped [-1, 1]");
            // Monotonic. `R' >= 0` exactly, so the only way the sampled curve
            // can step backwards is f32 rounding right at the knee - allow one
            // epsilon of that and nothing more.
            assert!(
                y >= prev - f32::EPSILON,
                "not monotonic at {x}: {y} < {prev}"
            );
            prev = y;
        }
        // Unity small-signal gain: R'(0) = 1, so tiny inputs pass through.
        assert!((shape(1e-4) / 1e-4 - 1.0).abs() < 1e-6);
        // Compressive above that: the shaper only ever pulls level down.
        for i in 1..3000 {
            let x = i as f32 * 0.001;
            assert!(shape(x) <= x, "shape({x}) should sit below the input");
        }
        assert!(shape(0.5) < 0.5);
        assert!(shape(2.0) < 2.0);
    }

    #[test]
    fn makeup_table_matches_its_generator() {
        assert_eq!(PRE_GAIN.len(), usize::from(MAX_DRIVE) + 1);
        assert_eq!(MAKEUP.len(), PRE_GAIN.len());
        assert_eq!(PRE_GAIN[0], 1.0);
        assert_eq!(MAKEUP[0], 1.0);
        for d in 1..=usize::from(MAX_DRIVE) {
            // pre-gain is `1 + drive * 0.35`...
            let want = 1.0 + d as f64 * 0.35;
            assert!(
                (f64::from(PRE_GAIN[d]) - want).abs() < 1e-6,
                "PRE_GAIN[{d}] = {} but 1 + {d}*0.35 = {want}",
                PRE_GAIN[d]
            );
            // ...and makeup is REF / R(pre * REF), bit for bit.
            let m = (MASTER_REF_LEVEL
                / shape64(f64::from(PRE_GAIN[d]) * MASTER_REF_LEVEL))
                as f32;
            assert_eq!(
                MAKEUP[d].to_bits(),
                m.to_bits(),
                "MAKEUP[{d}] = {} but the generator says {m}",
                MAKEUP[d]
            );
        }
        // Drive raises the pre-gain and lowers the ceiling, monotonically.
        for d in 1..usize::from(MAX_DRIVE) {
            assert!(PRE_GAIN[d] < PRE_GAIN[d + 1]);
            assert!(MAKEUP[d] > MAKEUP[d + 1]);
        }
        // A signal at the reference level comes out at the reference level.
        for d in 1..=usize::from(MAX_DRIVE) {
            let out = f64::from(shape(PRE_GAIN[d] * 0.7) * MAKEUP[d]);
            assert!(
                (out - MASTER_REF_LEVEL).abs() < 1e-6,
                "drive {d}: reference level came out at {out}"
            );
        }
        // ...while quiet material gets louder as drive rises (that is glue).
        let quiet = 0.05f32;
        let mut prev = quiet;
        for d in 1..=usize::from(MAX_DRIVE) {
            let out = shape(quiet * PRE_GAIN[d]) * MAKEUP[d];
            assert!(out > prev, "drive {d} did not lift quiet material");
            prev = out;
        }
    }

    #[test]
    fn tone_table_matches_its_generator_and_darkens_monotonically() {
        assert_eq!(TONE_A.len(), usize::from(MAX_TONE) + 1);
        assert_eq!(TONE_CUTOFF_HZ.len(), TONE_A.len());
        assert_eq!(TONE_A[0], 0.0, "tone 0 is bypass");
        assert_eq!(TONE_CUTOFF_HZ[0], 0);
        for t in 1..=usize::from(MAX_TONE) {
            let fc = f64::from(TONE_CUTOFF_HZ[t]);
            // The authoring-time formula. `exp` lives here, in a test, and
            // never in the render path.
            let a = (1.0 - (-2.0 * std::f64::consts::PI * fc / 44100.0).exp()) as f32;
            assert_eq!(
                TONE_A[t].to_bits(),
                a.to_bits(),
                "TONE_A[{t}] = {} but 1 - exp(-2*pi*{fc}/44100) = {a}",
                TONE_A[t]
            );
            assert!((0.0..1.0).contains(&TONE_A[t]));
        }
        // Higher setting = darker: lower cutoff, smaller coefficient.
        for t in 1..usize::from(MAX_TONE) {
            assert!(
                TONE_CUTOFF_HZ[t] > TONE_CUTOFF_HZ[t + 1],
                "tone {t} is not brighter than {}",
                t + 1
            );
            assert!(TONE_A[t] > TONE_A[t + 1]);
        }
        assert_eq!(TONE_CUTOFF_HZ[usize::from(MAX_TONE)], 3000);
    }

    #[test]
    fn hiss_levels_are_exact_and_tiny() {
        assert_eq!(HISS_LEVEL.len(), usize::from(MAX_HISS) + 1);
        assert_eq!(HISS_LEVEL[0], 0.0);
        for h in 1..=usize::from(MAX_HISS) {
            assert_eq!(HISS_LEVEL[h], h as f32 / 2048.0);
            assert!(HISS_LEVEL[h] > HISS_LEVEL[h - 1]);
        }
        // The loudest hiss is still ~54 dB below full scale.
        assert!(HISS_LEVEL[usize::from(MAX_HISS)] < 0.002);
        // ...and its LFSR is a different stream from the noise waveform's.
        assert_ne!(HISS_SEED, LFSR_SEED);
        assert_ne!(HISS_SEED, 0);
    }

    #[test]
    fn master_defaults_to_a_full_bypass() {
        assert_eq!(Master::default(), Master::OFF);
        assert!(Master::default().is_bypass());
        assert!(!Master { drive: 1, tone: 0, hiss: 0 }.is_bypass());
        assert!(!Master { drive: 0, tone: 1, hiss: 0 }.is_bypass());
        assert!(!Master { drive: 0, tone: 0, hiss: 1 }.is_bypass());
    }

    // -----------------------------------------------------------------
    // Sidechain ducking
    // -----------------------------------------------------------------

    #[test]
    fn duck_attack_lands_exactly_on_depth() {
        for depth in 1..=MAX_DUCK_DEPTH {
            let mut d = DuckBus::new();
            assert!(d.is_idle());
            assert_eq!(d.gain(), 1.0);
            d.trigger(2, Duck { depth, release: 8 });
            assert_eq!(d.trigger_ch, Some(2));
            // The first sample is still un-ducked: the ramp is the anti-click.
            assert_eq!(d.atten, 0.0);
            let mut prev = 0.0;
            for k in 1..=DUCK_ATTACK_SAMPLES {
                d.tick();
                assert!(d.atten >= prev, "attack went backwards at {k}");
                prev = d.atten;
            }
            // Exactly depth/7 after the ramp, to the bit.
            assert_eq!(d.atten, VOL_LEVELS[usize::from(depth)]);
            assert_eq!(d.gain(), 1.0 - VOL_LEVELS[usize::from(depth)]);
        }
    }

    #[test]
    fn duck_release_is_linear_and_ends_idle() {
        let release = 4u8;
        let mut d = DuckBus::new();
        d.trigger(0, Duck { depth: 7, release });
        for _ in 0..DUCK_ATTACK_SAMPLES {
            d.tick();
        }
        assert_eq!(d.atten, 1.0);

        let span = u32::from(release) * SAMPLES_PER_FRAME as u32;
        for _ in 0..span / 2 {
            d.tick();
        }
        // Half the release has run, so half the attenuation is gone.
        assert!((d.atten - 0.5).abs() < 1e-4, "half-recovered: {}", d.atten);
        assert!(!d.is_idle());

        // A quarter more and three quarters are recovered.
        for _ in 0..span / 4 {
            d.tick();
        }
        assert!((d.atten - 0.25).abs() < 1e-4, "3/4-recovered: {}", d.atten);

        for _ in 0..span {
            d.tick();
        }
        assert_eq!(d.atten, 0.0);
        assert!(d.is_idle(), "the envelope must let go of the channel");
        assert_eq!(d.gain(), 1.0);
        // Ticking an idle bus is a no-op forever.
        for _ in 0..10_000 {
            d.tick();
        }
        assert_eq!(d.atten, 0.0);
        assert_eq!(d.trigger_ch, None);
    }

    #[test]
    fn retrigger_restores_full_depth_and_hands_over_the_channel() {
        let mut d = DuckBus::new();
        d.trigger(0, Duck { depth: 7, release: 4 });
        for _ in 0..DUCK_ATTACK_SAMPLES + 2000 {
            d.tick();
        }
        assert!(d.atten < 1.0 && d.atten > 0.0, "mid-release: {}", d.atten);

        // Re-fire from a different channel: full depth again after one attack
        // window, and the new channel is the exempt one.
        d.trigger(3, Duck { depth: 7, release: 4 });
        assert_eq!(d.trigger_ch, Some(3));
        for _ in 0..DUCK_ATTACK_SAMPLES {
            d.tick();
        }
        assert_eq!(d.atten, 1.0, "re-trigger must reach full depth again");

        // A shallower trigger over a deeper one ramps *down* to the new depth.
        d.trigger(1, Duck { depth: 2, release: 4 });
        for _ in 0..DUCK_ATTACK_SAMPLES {
            d.tick();
        }
        assert_eq!(d.atten, VOL_LEVELS[2]);
    }

    #[test]
    fn duck_depth_is_the_volume_ladder() {
        // depth/7 and vol/7 are deliberately the same numbers.
        for depth in 1..=MAX_DUCK_DEPTH {
            let mut d = DuckBus::new();
            d.trigger(0, Duck { depth, release: 1 });
            for _ in 0..DUCK_ATTACK_SAMPLES {
                d.tick();
            }
            assert_eq!(d.atten, f32::from(depth) / 7.0);
        }
        // Depth 7 is a full mute of the other channels.
        let mut d = DuckBus::new();
        d.trigger(0, Duck { depth: 7, release: 1 });
        for _ in 0..DUCK_ATTACK_SAMPLES {
            d.tick();
        }
        assert_eq!(d.gain(), 0.0);
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

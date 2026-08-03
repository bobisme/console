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
//!
//! Orthogonal to both is the optional [`Wavetable`] bank (`wavetable <slot>
//! <32 nibbles>` in `__instruments__`): eight slots of a custom single-cycle
//! waveform, 32 samples of 4 bits, addressed as `w0`-`w7` wherever a wave digit
//! is legal. It is a wave *source* and nothing more — every envelope, effect
//! and mixer feature applies to it unchanged — and a cart that declares none
//! cannot produce a waveform id above 5, so its samples are untouched.
//!
//! Beside *that* sits the 2-op FM oscillator ([`WAVE_FM`], `wave=6` plus
//! `fm=<ratio>,<index>[,<decay>]` on an instrument): one modulator phase-
//! modulating one carrier, both reading a `const` quarter-wave sine table
//! ([`SINE_QUARTER`]) with linear interpolation. No `sin` at runtime — the
//! table is generated at authoring time and pasted in like [`NOTE_FREQ`] — and
//! both operators run on `u32` phase accumulators, so the whole thing is as
//! bit-exact as a square wave. The modulator increment is derived from the
//! carrier's every sample, which is what makes vibrato, sweeps and slides bend
//! the pair together instead of detuning it.
//!
//! Beside it sits the optional [`Echo`] bus (`echo` line in `__instruments__`,
//! the per-instrument `echo=<0-8>` send, or the Lua `echo()` setter): one mono
//! delay line, fed post-duck from the voices that ask for it, with feedback and
//! a fixed one-pole lowpass inside the loop. Same contract — off by default,
//! and off means the legacy statement, byte for byte.

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

/// Waveform id of the **2-op FM oscillator** (`wave=6`): one modulator phase-
/// modulating one carrier, both reading [`SINE_QUARTER`].
///
/// It sits outside [`WAVE_COUNT`] on purpose. 0..=5 are *self-contained*
/// waveforms — a bare digit in a sfx row says everything there is to say about
/// them — whereas FM is meaningless without a ratio and an index, so it is
/// reachable only through an instrument that carries an [`Fm`]. Id 7 stays
/// reserved (periodic noise is the obvious next tenant).
pub const WAVE_FM: u8 = 6;

/// Samples in one wavetable: 32 nibbles describing one single cycle.
///
/// 32 is the classic wavetable-chip size (Game Boy wave RAM, Konami VRC6/N163)
/// and it divides the 32-bit phase accumulator exactly: the top 5 bits of
/// `phase` *are* the sample index, so playback is a shift and a load.
pub const WAVETABLE_LEN: usize = 32;

/// Wavetable slots a cart may define, addressed `w0`..`w7`.
pub const WAVETABLE_SLOTS: usize = 8;

/// First internal waveform id that addresses a wavetable: the cart syntax
/// `w<slot>` parses to `WAVE_TABLE_BASE + slot`, so slots occupy ids 8..=15.
///
/// The builtin waves are 0..=5, id 6 is the 2-op FM oscillator ([`WAVE_FM`])
/// and **id 7 is deliberately left free** for the oscillator still to come
/// (periodic noise). Nothing a pre-wavetable cart can write produces a `wave`
/// byte above 5, which is why adding this cannot move a single sample of a cart
/// with no `wavetable` line.
pub const WAVE_TABLE_BASE: u8 = 8;

/// Right shift that turns a phase accumulator into a wavetable index:
/// `32 - log2(WAVETABLE_LEN)`, so `phase >> WAVETABLE_SHIFT` is always 0..=31.
const WAVETABLE_SHIFT: u32 = 27;

/// Nibble (0..=15) to amplitude: `(2n - 15) / 15`, i.e. code 0 is exactly
/// `-1.0`, code 15 is exactly `+1.0`, and the ladder is symmetric about zero
/// (`n` and `15 - n` are exact negations of each other).
///
/// Consequences worth knowing before writing a table by hand:
///
/// - **Full scale matches the builtin oscillators.** The table
///   `ffffffffffffffff0000000000000000` renders the square wave (id 2)
///   sample for sample — `wavetable_square_is_the_builtin_square` pins it.
/// - **4 bits cannot represent zero.** The two centre codes are `7` = −1/15
///   and `8` = +1/15, so a table of all `8`s is not silence but a constant
///   +0.0667 DC offset. Pair every `8` with a `7` (the classic trick: write
///   the rising zero-crossing as `8` and the falling one as `7`) and the table
///   is exactly DC-free — the sum of `2n - 15` over the 32 samples is 0.
/// - The mapping is a division by 15 rather than by 16 precisely so that the
///   extremes hit ±1: a `/16` ladder would make `0f`-style tables lopsided by
///   −1/16 and every wavetable quieter than every builtin wave.
pub const NIBBLE_LEVEL: [f32; 16] = [
    -1.0,
    -13.0 / 15.0,
    -11.0 / 15.0,
    -9.0 / 15.0,
    -7.0 / 15.0,
    -5.0 / 15.0,
    -3.0 / 15.0,
    -1.0 / 15.0,
    1.0 / 15.0,
    3.0 / 15.0,
    5.0 / 15.0,
    7.0 / 15.0,
    9.0 / 15.0,
    11.0 / 15.0,
    13.0 / 15.0,
    1.0,
];

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
    25.956543, 27.5, 29.135235, 30.867706, // C1..B1
    32.703197, 34.647827, 36.708096, 38.890873, 41.203445, 43.65353, 46.249302, 48.999428,
    51.913086, 55.0, 58.27047, 61.735413, // C2..B2
    65.406395, 69.295654, 73.41619, 77.781746, 82.40689, 87.30706, 92.498604, 97.998856, 103.82617,
    110.0, 116.54094, 123.470825, // C3..B3
    130.81279, 138.59131, 146.83238, 155.56349, 164.81378, 174.61412, 184.99721, 195.99771,
    207.65234, 220.0, 233.08188, 246.94165, // C4..B4
    261.62558, 277.18262, 293.66476, 311.12698, 329.62756, 349.22824, 369.99442, 391.99542,
    415.3047, 440.0, 466.16376, 493.8833, // C5..B5
    523.25116, 554.36523, 587.3295, 622.25397, 659.2551, 698.4565, 739.98883, 783.99084, 830.6094,
    880.0, 932.3275, 987.7666, // C6..B6
    1046.5023, 1108.7305, 1174.659, 1244.5079, 1318.5103, 1396.913, 1479.9777, 1567.9817,
    1661.2188, 1760.0, 1864.655, 1975.5332, // C7..B7
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
// 2-op FM: the sine table, the index ladder and the index-decay envelope
// ---------------------------------------------------------------------------

/// One quarter of a sine cycle, 257 samples: `SINE_QUARTER[k] = sin(2*pi*k/1024)`
/// for `k` in `0..=256`, so index 0 is exactly `0.0` and index 256 exactly
/// `1.0`.
///
/// Generated once and pasted in (never computed at runtime — SPEC's
/// no-transcendentals rule) with:
///
/// ```text
/// python3 -c "
/// import math, struct
/// f32 = lambda x: struct.unpack('<f', struct.pack('<f', x))[0]
/// for k in range(257):
///     print(f32(math.sin(2.0 * math.pi * k / 1024.0)))"
/// ```
///
/// Each literal is the shortest decimal that round-trips to the same `f32`,
/// exactly as [`NOTE_FREQ`] is written.
///
/// *Why a quarter and not a full cycle*: the other three quadrants are exact
/// reflections of this one, and deriving them by negation and mirroring makes
/// the symmetries **bit-exact by construction** rather than merely true to
/// eight digits. [`sine_at`] therefore has exact zero crossings at phase 0 and
/// 2^31, exact peaks of ±1.0 at 2^30 and 3·2^30, and satisfies
/// `sine_at(-p) == -sine_at(p)` and `sine_at(p + 2^31) == -sine_at(p)` for
/// every `p` — properties `the_sine_table_is_*` in the unit tests pin. It is
/// also a quarter of the memory: 1 KiB rather than 4.
///
/// *Why 1024 points*: with the linear interpolation [`sine_at`] does, the
/// worst-case error of a 1024-point table is `(pi/1024)^2 / 8` ≈ 1.2e-6, i.e.
/// about −118 dBFS — two bits below the noise floor of the 16-bit WAV the
/// harness writes, and far smaller than the 1/15 quantisation a wavetable
/// voice lives with. Doubling it would buy nothing audible.
#[allow(clippy::excessive_precision)]
pub const SINE_QUARTER: [f32; 257] = [
    0.0,
    0.0061358847,
    0.012271538,
    0.01840673,
    0.024541229,
    0.030674804,
    0.036807224,
    0.04293826,
    0.049067676,
    0.055195246,
    0.061320737,
    0.06744392,
    0.07356457,
    0.07968244,
    0.08579731,
    0.091908954,
    0.09801714,
    0.10412163,
    0.110222206,
    0.11631863,
    0.12241068,
    0.1284981,
    0.1345807,
    0.14065824,
    0.14673047,
    0.15279719,
    0.15885815,
    0.16491312,
    0.17096189,
    0.17700422,
    0.18303989,
    0.18906866,
    0.19509032,
    0.20110464,
    0.20711137,
    0.21311031,
    0.21910124,
    0.22508392,
    0.2310581,
    0.2370236,
    0.24298018,
    0.24892761,
    0.25486565,
    0.2607941,
    0.26671275,
    0.27262136,
    0.2785197,
    0.28440753,
    0.29028466,
    0.2961509,
    0.30200595,
    0.30784965,
    0.31368175,
    0.31950203,
    0.3253103,
    0.3311063,
    0.33688986,
    0.34266073,
    0.34841868,
    0.35416353,
    0.35989505,
    0.36561298,
    0.3713172,
    0.37700742,
    0.38268343,
    0.38834503,
    0.39399204,
    0.3996242,
    0.4052413,
    0.41084316,
    0.41642955,
    0.42200026,
    0.42755508,
    0.43309382,
    0.43861625,
    0.44412214,
    0.44961134,
    0.45508358,
    0.46053872,
    0.4659765,
    0.47139674,
    0.47679922,
    0.48218378,
    0.48755017,
    0.4928982,
    0.49822766,
    0.50353837,
    0.50883013,
    0.51410276,
    0.519356,
    0.52458966,
    0.52980363,
    0.53499764,
    0.54017144,
    0.545325,
    0.55045795,
    0.55557024,
    0.56066155,
    0.5657318,
    0.57078075,
    0.57580817,
    0.58081394,
    0.58579785,
    0.5907597,
    0.5956993,
    0.60061646,
    0.60551107,
    0.6103828,
    0.6152316,
    0.6200572,
    0.6248595,
    0.62963825,
    0.6343933,
    0.63912445,
    0.64383155,
    0.6485144,
    0.65317285,
    0.6578067,
    0.6624158,
    0.66699994,
    0.671559,
    0.6760927,
    0.680601,
    0.6850837,
    0.68954057,
    0.69397146,
    0.69837624,
    0.70275474,
    0.70710677,
    0.7114322,
    0.71573085,
    0.72000253,
    0.7242471,
    0.72846437,
    0.7326543,
    0.7368166,
    0.7409511,
    0.74505776,
    0.7491364,
    0.7531868,
    0.7572088,
    0.7612024,
    0.76516724,
    0.76910335,
    0.77301043,
    0.7768885,
    0.7807372,
    0.78455657,
    0.7883464,
    0.79210657,
    0.7958369,
    0.79953724,
    0.8032075,
    0.8068476,
    0.81045717,
    0.8140363,
    0.8175848,
    0.8211025,
    0.8245893,
    0.82804507,
    0.8314696,
    0.8348629,
    0.8382247,
    0.841555,
    0.8448536,
    0.84812033,
    0.8513552,
    0.854558,
    0.8577286,
    0.86086696,
    0.86397284,
    0.86704624,
    0.87008697,
    0.873095,
    0.8760701,
    0.8790122,
    0.8819213,
    0.8847971,
    0.88763964,
    0.89044875,
    0.8932243,
    0.89596623,
    0.8986745,
    0.9013488,
    0.9039893,
    0.9065957,
    0.909168,
    0.91170603,
    0.9142098,
    0.9166791,
    0.9191139,
    0.92151403,
    0.9238795,
    0.9262102,
    0.9285061,
    0.93076694,
    0.9329928,
    0.9351835,
    0.937339,
    0.9394592,
    0.94154406,
    0.94359344,
    0.9456073,
    0.9475856,
    0.94952816,
    0.951435,
    0.953306,
    0.9551412,
    0.95694035,
    0.95870346,
    0.9604305,
    0.9621214,
    0.96377605,
    0.96539444,
    0.96697646,
    0.9685221,
    0.97003126,
    0.9715039,
    0.97293997,
    0.97433937,
    0.9757021,
    0.97702813,
    0.9783174,
    0.9795698,
    0.98078525,
    0.9819639,
    0.9831055,
    0.9842101,
    0.98527765,
    0.9863081,
    0.9873014,
    0.9882576,
    0.9891765,
    0.9900582,
    0.99090266,
    0.99170977,
    0.99247956,
    0.9932119,
    0.993907,
    0.9945646,
    0.9951847,
    0.9957674,
    0.9963126,
    0.9968203,
    0.99729043,
    0.99772304,
    0.9981181,
    0.99847555,
    0.99879545,
    0.99907774,
    0.99932235,
    0.9995294,
    0.9996988,
    0.9998306,
    0.9999247,
    0.99998116,
    1.0,
];

/// Peak phase deviation, in 32-bit phase units, contributed by **one unit of
/// modulation index**: `2^29`, i.e. an eighth of a cycle.
///
/// So `index` 0..=15 buys a peak deviation of `index/8` cycles, which is a
/// modulation index of `beta = 2*pi*index/8 = 0.785*index` radians in the
/// textbook `sin(wc*t + beta*sin(wm*t))` sense. The ladder in musical terms:
///
/// | index | beta | character |
/// |-------|------|-----------|
/// | 0     | 0    | a pure sine — the console's only clean one |
/// | 1-3   | 0.8-2.4 | one or two sidebands; warm, hollow, rhodes-ish |
/// | 4-6   | 3.1-4.7 | the Genesis bass/brass region |
/// | 7-10  | 5.5-7.9 | bright, glassy, obviously FM |
/// | 11-15 | 8.6-11.8 | clangorous; bells and metal |
///
/// `2^29` is an exact power of two, so the whole scale is exact in `f32`.
const FM_INDEX_PHASE: f32 = 536_870_912.0;

/// Index-decay half-life in **frames** per `decay` setting, `0` meaning "no
/// decay at all" (the index is held for the life of the note).
///
/// Roughly geometric from two seconds down to a single frame, so the ladder
/// spans "a pad that slowly loses its edge" to "a plucked transient that is
/// gone before the amplitude envelope has finished its attack".
pub const FM_DECAY_HALF_LIFE: [u8; 16] = [0, 120, 90, 64, 48, 36, 27, 20, 15, 11, 8, 6, 4, 3, 2, 1];

/// Per-frame index multiplier: `FM_DECAY_MUL[d] = 0.5^(1 / FM_DECAY_HALF_LIFE[d])`,
/// and exactly `1.0` at `d == 0`.
///
/// Generated once and pasted in (no `powf` at runtime) with:
///
/// ```text
/// python3 -c "
/// import struct
/// f32 = lambda x: struct.unpack('<f', struct.pack('<f', x))[0]
/// for h in [120, 90, 64, 48, 36, 27, 20, 15, 11, 8, 6, 4, 3, 2, 1]:
///     print(f32(0.5 ** (1.0 / h)))"
/// ```
///
/// The envelope is applied once per **frame** (in [`Channel::tick_fm`]), never
/// per sample: it is a musical gesture at the same 60 Hz grid `env`, `vib` and
/// `sweep` already live on, and one multiply per frame per voice cannot drift.
#[allow(clippy::excessive_precision)]
const FM_DECAY_MUL: [f32; 16] = [
    1.0, 0.9942404, 0.9923279, 0.989228, 0.9856632, 0.9809301, 0.9746546, 0.9659363, 0.9548416,
    0.9389309, 0.91700405, 0.8908987, 0.8408964, 0.7937005, 0.70710677, 0.5,
];

/// Index below which the decay envelope snaps to exactly zero.
///
/// A geometric decay never reaches 0, and an index of 1/1024 is a peak phase
/// deviation of 1/8192 of a cycle — utterly inaudible, but enough to keep the
/// carrier from being the *exact* sine that `index=0` promises. Snapping is a
/// plain comparison, so it stays deterministic; the same reasoning as
/// [`DENORM_FLOOR`], for musical rather than numerical reasons.
const FM_INDEX_FLOOR: f32 = 1.0 / 1024.0;

/// Sine of a 32-bit phase (one turn = 2^32), by table lookup with linear
/// interpolation.
///
/// The phase splits into a 10-bit table position and a 16-bit fraction; the low
/// 6 bits are discarded. Position 0..=1023 is mapped onto [`SINE_QUARTER`] by
/// quadrant:
///
/// ```text
/// q0 (0..255)     sin(x)          =  Q[i]      -> Q[i+1]
/// q1 (256..511)   sin(pi/2 + x)   =  Q[256-i]  -> Q[255-i]
/// q2 (512..767)   sin(pi + x)     = -Q[i]      -> -Q[i+1]
/// q3 (768..1023)  sin(3pi/2 + x)  = -Q[256-i]  -> -Q[255-i]
/// ```
///
/// Everything is `*`, `+`, `-` and an array index on `const` values, so it is
/// bit-identical on every target — and because the interpolation is written as
/// `a*(1-f) + b*f` with the *same* operand order in every quadrant, negating
/// the phase negates the result to the last bit (IEEE multiplication and
/// addition are exact under sign flips and commutative).
fn sine_at(phase: u32) -> f32 {
    let pos = (phase >> 22) as usize; // 0..=1023
    // Bits 6..22 of the phase: 16 fractional bits between adjacent entries.
    // The integer is at most 65535, so the cast is exact, and 2^-16 is an exact
    // power of two.
    let f = ((phase >> 6) & 0xffff) as f32 * (1.0 / 65_536.0);
    let i = pos & 0xff;
    let (a, b) = match pos >> 8 {
        0 => (SINE_QUARTER[i], SINE_QUARTER[i + 1]),
        1 => (SINE_QUARTER[256 - i], SINE_QUARTER[255 - i]),
        2 => (-SINE_QUARTER[i], -SINE_QUARTER[i + 1]),
        _ => (-SINE_QUARTER[256 - i], -SINE_QUARTER[255 - i]),
    };
    a * (1.0 - f) + b * f
}

/// The modulator's phase increment: the carrier's, scaled by the ratio.
///
/// `ratio_half` counts **halves** (see [`Fm::ratio_half`]), so this is
/// `inc * ratio_half / 2` in exact 64-bit integer arithmetic, truncated back
/// into the 32-bit accumulator. The truncation is modular, which is the right
/// answer: a modulator asked to run past the sample rate aliases down, exactly
/// as any digital phase accumulator does, and stays deterministic doing it.
fn fm_mod_increment(inc: u32, ratio_half: u8) -> u32 {
    ((u64::from(inc) * u64::from(ratio_half)) >> 1) as u32
}

/// The carrier's phase offset this sample: `index * sin(mod_phase)`, in phase
/// units.
///
/// `index` is the *live* index (the note's starting index after however much of
/// the decay envelope has run), so the peak magnitude is at most
/// `15 * 2^29 < 2^63` and the `f32 -> i64` cast cannot saturate. The
/// `i64 -> u32` truncation is the modular wrap the phase accumulator wants.
fn fm_deviation(mod_phase: u32, index: f32) -> u32 {
    (sine_at(mod_phase) * index * FM_INDEX_PHASE) as i64 as u32
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
const HISS_LEVEL: [f32; 5] = [0.0, 1.0 / 2048.0, 2.0 / 2048.0, 3.0 / 2048.0, 4.0 / 2048.0];

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
// Echo bus: one mono delay line with feedback (the SNES half)
// ---------------------------------------------------------------------------

/// Longest echo delay, **in frames**.
///
/// The delay time is expressed in whole console frames rather than
/// milliseconds or rows, for three reasons:
///
/// 1. **Exactness.** A frame is [`SAMPLES_PER_FRAME`] = 735 samples, always.
///    `delay=<n>` is therefore `n * 735` samples with no rounding, no
///    resampling and no fractional read pointer — the delay line is addressed
///    by integer index and every target agrees trivially.
/// 2. **SNES character.** The SNES echo length register (EDL) stepped in
///    16 ms units, 0..240 ms. One frame is 16.67 ms, so `delay=<n>` *is*
///    essentially EDL `n`, with the same coarse, steppy feel — you cannot dial
///    in 23 ms, and that is the point.
/// 3. **Musical addressing.** Row length is already in frames (`speed=`), so
///    an echo synced to the music is just arithmetic the author can do in their
///    head: at `speed=8`, `delay=8` is one row, `delay=12` a dotted row,
///    `delay=16` a beat.
///
/// 60 frames = exactly one second, which is well past the SNES' 240 ms and
/// makes [`ECHO_LINE_LEN`] exactly [`SAMPLE_RATE`] samples.
pub const MAX_ECHO_DELAY: u8 = 60;

/// Highest echo `feedback` setting.
pub const MAX_ECHO_FEEDBACK: u8 = 8;

/// Highest echo `level` (return) setting.
pub const MAX_ECHO_LEVEL: u8 = 8;

/// Highest per-instrument `echo=` send setting.
pub const MAX_ECHO_SEND: u8 = 8;

/// Lowest `fm=` ratio, in halves: `0.5`.
pub const MIN_FM_RATIO_HALF: u8 = 1;

/// Highest `fm=` ratio, in halves: `15.0`. The YM2612's MUL field stops at 15
/// too, and above it the modulator is well past anything a note can support
/// without aliasing into a different tone entirely.
pub const MAX_FM_RATIO_HALF: u8 = 30;

/// Highest `fm=` modulation index.
pub const MAX_FM_INDEX: u8 = 15;

/// Highest `fm=` index-decay setting.
pub const MAX_FM_DECAY: u8 = 15;

/// Samples in the delay line: [`MAX_ECHO_DELAY`] frames, i.e. one second.
///
/// The buffer is a fixed-size, zero-initialised array allocated once by
/// [`Audio::new`]. Nothing in the render path ever allocates, resizes or
/// reallocates it — changing `delay` only moves a read index.
pub const ECHO_LINE_LEN: usize = MAX_ECHO_DELAY as usize * SAMPLES_PER_FRAME;

/// Feedback gain per setting: `feedback * 7/64`.
///
/// **The loop always decays.** The maximum is `ECHO_FB[8] = 7/8 = 0.875`,
/// deliberately below unity: the one-pole filter in the loop has a DC gain of
/// exactly 1, so it cannot be relied on to tame a runaway, and the gain itself
/// has to do it. 0.875 is -1.16 dB per repeat, so a maximum-feedback echo takes
/// about 59 repeats to fall 60 dB — long, obviously "infinite" to the ear, and
/// still provably convergent.
///
/// `7/64` is an exact binary fraction, so every entry is exact in f32 and every
/// target agrees to the last bit.
const ECHO_FB: [f32; 9] = [
    0.0, 0.109375, 0.21875, 0.328125, 0.4375, 0.546875, 0.65625, 0.765625, 0.875,
];

/// The eighths ladder `n / 8`, used for **both** the master `level` (how loud
/// the delay line comes back) and each instrument's `echo=` send (how much of
/// that voice goes in). One table because they are the same unit: a fraction of
/// the voice's own level. Every entry is an exact binary fraction.
///
/// `8` therefore means "the echo returns at the same level a voice would" —
/// full scale, not a safe scale. See [`Audio::render`] for what the master bus
/// does about that.
const ECHO_GAIN: [f32; 9] = [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0];

/// Cutoff of the fixed one-pole lowpass **inside** the feedback loop, in Hz.
///
/// This is the whole reason echo repeats sit behind the dry signal instead of
/// crowding it: each pass through the loop is filtered again, so repeat *k* has
/// been lowpassed *k* times and the tail gets progressively darker and softer
/// until it is a hum. The SNES did the same job with its 8-tap FIR on the echo
/// path (whose stock coefficient sets were nearly all lowpass); one pole is the
/// cheap, deterministic, allocation-free equivalent.
///
/// 4800 Hz is the "gentle" setting: it barely touches a bass line, takes the
/// edge off a lead's harmonics on the first repeat and eats them by the third.
pub const ECHO_LP_CUTOFF_HZ: u32 = 4800;

/// The loop filter coefficient, `y += a * (x - y)` with
/// `a = 1 - exp(-2*pi * ECHO_LP_CUTOFF_HZ / 44100)`.
///
/// Evaluated **at authoring time** — no `exp` in the render path, ever.
/// Generated once and pasted in with:
///
/// ```text
/// python3 -c "
/// import math, struct
/// f32 = lambda x: struct.unpack('<f', struct.pack('<f', x))[0]
/// print(f32(1.0 - math.exp(-2.0 * math.pi * 4800 / 44100.0)))"
/// ```
#[allow(clippy::excessive_precision)]
const ECHO_LP_A: f32 = 0.49534696;

/// `echo delay=<1-60> feedback=<0-8> level=<0-8>` — the cart-global echo bus.
///
/// All-zero (the [`Default`]) means "no echo line": nothing is sent, nothing
/// returns, and the mix takes the PoC v1 statement unchanged. A cart may
/// declare one `echo` line in `__instruments__`, and the Lua
/// `echo(delay, feedback, level)` setter overrides it at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Echo {
    /// Delay time in whole frames, 1..=[`MAX_ECHO_DELAY`]. 0 = bus off.
    pub delay: u8,
    /// Feedback into the delay line, 0..=[`MAX_ECHO_FEEDBACK`] ([`ECHO_FB`]).
    pub feedback: u8,
    /// Return level of the delay line, 0..=[`MAX_ECHO_LEVEL`]. 0 = bus off.
    pub level: u8,
}

impl Echo {
    /// The all-zero echo: the bus is not running at all.
    pub const OFF: Echo = Echo {
        delay: 0,
        feedback: 0,
        level: 0,
    };

    /// True when the bus is not running, i.e. the render loop must not touch
    /// the delay line at all.
    ///
    /// Either endpoint being zero switches the whole bus off: a zero `delay`
    /// has no line to speak of and a zero `level` returns nothing, so in both
    /// cases running the loop could only cost cycles and perturb state. This is
    /// what makes the back-compat guarantee mechanical — a cart with no `echo`
    /// line has `Echo::OFF`, which is bypassed, so the mixer takes the legacy
    /// statement.
    pub fn is_bypass(&self) -> bool {
        self.delay == 0 || self.level == 0
    }

    /// The delay time in samples: `delay` frames of 735 samples, clamped to the
    /// line length.
    pub fn delay_samples(&self) -> usize {
        usize::from(self.delay.min(MAX_ECHO_DELAY)) * SAMPLES_PER_FRAME
    }
}

/// The echo bus' per-console runtime state: the delay line itself, its write
/// cursor and the loop filter's memory.
///
/// Allocated once (boxed, so the one-second buffer never sits on the stack) and
/// zero-initialised by [`Audio::new`]. [`EchoBus::tick`] only ever indexes it.
struct EchoBus {
    /// The delay line, oldest-to-newest around `pos`.
    line: Box<[f32; ECHO_LINE_LEN]>,
    /// Where the next sample is written.
    pos: usize,
    /// One-pole lowpass memory, inside the feedback loop.
    lp: f32,
}

impl EchoBus {
    fn new() -> EchoBus {
        // Built through a `Vec` rather than `Box::new([0.0; N])` on purpose:
        // the array literal is a 176 KB temporary that an unoptimised build
        // materialises on the stack before moving it to the heap, and
        // emscripten's default stack is far smaller than that. `vec![0.0; n]`
        // for a zero-bit element goes straight to `alloc_zeroed`.
        let line: Box<[f32; ECHO_LINE_LEN]> = vec![0.0f32; ECHO_LINE_LEN]
            .into_boxed_slice()
            .try_into()
            .expect("the vec is built with exactly ECHO_LINE_LEN elements");
        EchoBus {
            line,
            pos: 0,
            lp: 0.0,
        }
    }

    /// Forget everything: silence the line, rewind the cursor, clear the
    /// filter. Called when the bus is switched off, so re-enabling it later can
    /// never resurrect audio from an earlier scene.
    fn clear(&mut self) {
        self.line.fill(0.0);
        self.pos = 0;
        self.lp = 0.0;
    }

    /// True when the line and the filter hold nothing at all.
    fn is_silent(&self) -> bool {
        self.lp == 0.0 && self.line.iter().all(|&s| s == 0.0)
    }

    /// Run one sample through the bus and return the **delayed** sample (before
    /// the return level is applied).
    ///
    /// ```text
    /// delayed = line[pos - delay]
    /// line[pos] = lowpass(send + delayed * feedback)
    /// ```
    ///
    /// The filter sits *inside* the loop, so every repeat is filtered once
    /// more. Reading before writing is what makes `delay == ECHO_LINE_LEN`
    /// (i.e. `delay=60`) mean a full second rather than zero.
    ///
    /// Changing the delay time moves the read index without touching the line,
    /// which is the classic tape-echo behaviour: the repeats already in flight
    /// jump rather than glide. Nothing here allocates.
    fn tick(&mut self, send: f32, delay_samples: usize, feedback: f32) -> f32 {
        let d = delay_samples.clamp(1, ECHO_LINE_LEN);
        let read = (self.pos + ECHO_LINE_LEN - d) % ECHO_LINE_LEN;
        let delayed = self.line[read];

        self.lp += ECHO_LP_A * ((send + delayed * feedback) - self.lp);
        // Same reasoning as the tone filter: a geometric decay never reaches
        // exactly zero, and a line full of denormals is expensive for a signal
        // nobody can hear. A plain comparison, so it stays deterministic.
        if self.lp.abs() < DENORM_FLOOR {
            self.lp = 0.0;
        }
        self.line[self.pos] = self.lp;

        self.pos += 1;
        if self.pos == ECHO_LINE_LEN {
            self.pos = 0;
        }
        delayed
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

/// `fm=<ratio>,<index>[,<decay>]`: the parameters of the 2-op FM oscillator
/// ([`WAVE_FM`]). Required on a `wave=6` instrument, rejected on any other.
///
/// The model is the classic pair — one **modulator** phase-modulating one
/// **carrier**, both sine:
///
/// ```text
/// out(t) = sin(2*pi*fc*t + beta * sin(2*pi*ratio*fc*t))
/// ```
///
/// with `fc` the row's note (so the carrier is always at pitch), the modulator
/// locked to it by `ratio`, and `beta` the modulation index. One pair is a
/// small fraction of a YM2612's four operators and still covers most of what
/// people remember about that chip: the ratio picks the *harmonic family*
/// (integer = harmonic and pitched, half-integer = inharmonic and bell-like)
/// and the index picks how far up the series the energy reaches.
///
/// At runtime there is no `sin`: both operators read [`SINE_QUARTER`] through
/// [`sine_at`], and the modulator's output is added to the carrier's phase
/// accumulator as an integer (see [`fm_deviation`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fm {
    /// Modulator:carrier frequency ratio, counted in **halves**: 1..=30, i.e.
    /// 0.5 .. 15.0 in steps of 0.5.
    ///
    /// Halves rather than a free rational for two reasons. The real chips work
    /// this way — the YM2612's MUL field is 0.5 then the integers 1..15, and
    /// the DX7's coarse ratio is the integers plus a 0.5 — so the ladder is the
    /// idiom musicians already have in their fingers. And a half-integer ratio
    /// is exactly the inharmonic case worth having: ratio 3.5 places sidebands
    /// midway between harmonics, which is what makes a bell a bell. The
    /// arithmetic stays exact (`inc * ratio_half / 2` in `u64`), so there is no
    /// rounding to specify.
    pub ratio_half: u8,
    /// Modulation depth at note-on, 0..=[`MAX_FM_INDEX`]. See
    /// [`FM_INDEX_PHASE`] for what each step is worth. `0` is a pure sine.
    pub index: u8,
    /// Index-decay rate, 0..=[`MAX_FM_DECAY`]. `0` holds the index flat; higher
    /// settings halve it faster (see [`FM_DECAY_HALF_LIFE`]).
    ///
    /// This is the whole reason FM sounds alive rather than like an organ: a
    /// real plucked or struck tone is bright at the attack and dull by the
    /// time it decays, and on an FM voice that is the *index* falling, not the
    /// volume. It is a separate envelope from `env` on purpose — a Genesis
    /// electric piano holds its level while its brightness dies, and a bell
    /// does the opposite.
    pub decay: u8,
}

impl Fm {
    /// The modulator ratio as a number, e.g. `3.5`. Exact: `ratio_half / 2`.
    pub fn ratio(&self) -> f32 {
        f32::from(self.ratio_half) * 0.5
    }

    /// The ratio spelled the way a cart line spells it (`"1"`, `"3.5"`), so
    /// tooling can print an instrument back.
    pub fn ratio_text(&self) -> String {
        if self.ratio_half % 2 == 0 {
            format!("{}", self.ratio_half / 2)
        } else {
            format!("{}.5", self.ratio_half / 2)
        }
    }
}

/// One `wavetable <slot 0-7> <32 hex nibbles>` entry: a custom single-cycle
/// waveform, 32 samples of 4 bits each.
///
/// The nibbles are stored raw (0..=15) rather than as floats so the cart text
/// round-trips exactly and [`Wavetable::hex`] can print it back. [`NIBBLE_LEVEL`]
/// is the mapping to amplitude, and the synth precomputes it once per console
/// ([`WaveSet`]) — the render loop never divides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wavetable {
    /// 32 nibbles, 0..=15, in phase order (index 0 is phase 0).
    pub nibbles: [u8; WAVETABLE_LEN],
}

impl Wavetable {
    /// Amplitude of sample `i` (wrapping), via [`NIBBLE_LEVEL`].
    pub fn level(&self, i: usize) -> f32 {
        NIBBLE_LEVEL[usize::from(self.nibbles[i % WAVETABLE_LEN] & 0x0f)]
    }

    /// The 32 nibbles back as lowercase hex — exactly what the cart line said.
    pub fn hex(&self) -> String {
        self.nibbles
            .iter()
            .map(|n| char::from_digit(u32::from(*n & 0x0f), 16).unwrap_or('0'))
            .collect()
    }

    /// Sum of `2n - 15` over the table: zero for a DC-free table. Exact
    /// integer arithmetic, so "is this table centred?" is a decidable question
    /// rather than a float comparison.
    pub fn dc_sum(&self) -> i32 {
        self.nibbles.iter().map(|&n| 2 * i32::from(n) - 15).sum()
    }
}

/// The eight wavetable slots as amplitudes, precomputed once per [`Audio`].
///
/// Undefined slots are all-zero and unreachable: referencing one is a parse
/// error, so silence here is a belt-and-braces default, never a fallback the
/// musician can hear.
#[derive(Debug, Clone)]
struct WaveSet([[f32; WAVETABLE_LEN]; WAVETABLE_SLOTS]);

impl WaveSet {
    fn new(tables: &[Option<Wavetable>; WAVETABLE_SLOTS]) -> WaveSet {
        let mut out = [[0.0f32; WAVETABLE_LEN]; WAVETABLE_SLOTS];
        for (slot, table) in tables.iter().enumerate() {
            if let Some(t) = table {
                for (i, v) in out[slot].iter_mut().enumerate() {
                    *v = t.level(i);
                }
            }
        }
        WaveSet(out)
    }
}

/// One `inst <name> wave=<0-6|w0-w7> [fm=...] [env=...] [vib=...] [sweep=...]
/// [duck=...] [echo=<0-8>]` entry.
///
/// A bare wave digit on a sfx row means the *implicit flat instrument*: that
/// waveform with no envelope, vibrato or sweep, which is exactly the PoC v1
/// behaviour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    /// `[a-z0-9_]+`, unique, never all-digits (those name the bare waveforms)
    /// and never `w<digits>` (that spelling names a wavetable slot).
    pub name: String,
    /// Waveform: a builtin 0..=5, [`WAVE_FM`] (6) for the 2-op FM oscillator,
    /// or [`WAVE_TABLE_BASE`]` + slot` (8..=15) when the instrument said
    /// `wave=w<slot>`.
    pub wave: u8,
    /// `fm=<ratio>,<index>[,<decay>]`. `Some` exactly when `wave == WAVE_FM`:
    /// the parser rejects an FM instrument without parameters and parameters
    /// on a non-FM instrument, so the two can never disagree.
    pub fm: Option<Fm>,
    pub env: Option<Env>,
    pub vib: Option<Vib>,
    pub sweep: Option<Sweep>,
    /// Sidechain trigger: every note-on of this instrument ducks the other
    /// channels. Independent of [`Instrument::is_flat`] — ducking costs the
    /// voice itself nothing per frame.
    pub duck: Option<Duck>,
    /// `echo=<0-8>`: how much of this voice is sent to the [`Echo`] bus, in
    /// eighths ([`ECHO_GAIN`]). 0 (the default) is fully dry, and a cart whose
    /// instruments all send 0 never feeds the delay line at all.
    pub echo: u8,
}

impl Instrument {
    /// True when the instrument needs per-frame modulation. A flat instrument
    /// (`inst x wave=2`) takes exactly the same code path as a bare digit.
    ///
    /// `duck`, `echo` and `fm` are deliberately not part of this. The first two
    /// are *mixer* properties and the third is a property of the **wave
    /// source**, exactly like a wavetable slot: the FM pair runs inside the
    /// oscillator, off the channel's own phase accumulators, and needs no
    /// per-frame pitch or volume recomputation. So an instrument that only
    /// ducks, only sends to the echo, or only names an FM patch still renders
    /// through the PoC v1 statements.
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
    /// The `wavetable <slot> <32 nibbles>` lines from `__instruments__`.
    /// All-`None` for every cart written before wavetables existed.
    wavetables: [Option<Wavetable>; WAVETABLE_SLOTS],
    tempo: Option<Tempo>,
    /// The `master` line from `__instruments__`; all-zero when absent.
    master: Master,
    /// The `echo` line from `__instruments__`; all-zero when absent.
    echo: Echo,
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

    /// The wavetable in slot `id` (`w<id>`), if the cart defines it.
    pub fn wavetable(&self, id: u8) -> Option<&Wavetable> {
        self.wavetables
            .get(usize::from(id))
            .and_then(|t| t.as_ref())
    }

    /// All eight wavetable slots, `None` where the cart defined nothing.
    pub fn wavetables(&self) -> &[Option<Wavetable>; WAVETABLE_SLOTS] {
        &self.wavetables
    }

    /// The wavetable a *waveform id* refers to: `Some` only for the
    /// [`WAVE_TABLE_BASE`]-and-up ids that `w<slot>` produces.
    pub fn wavetable_for_wave(&self, wave: u8) -> Option<&Wavetable> {
        self.wavetable(wave.checked_sub(WAVE_TABLE_BASE)?)
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

    /// The `echo` line from `__instruments__`. All-zero (the bus switched off)
    /// when the cart has none — which is the bit-identical legacy output path.
    /// Lua's `echo()` overrides this at runtime without touching the bank.
    pub fn echo(&self) -> Echo {
        self.echo
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

    /// True when the cart has no instruments, wavetables, sfx, patterns,
    /// master or echo line.
    pub fn is_empty(&self) -> bool {
        self.sfx.is_empty()
            && self.patterns.is_empty()
            && self.instruments.is_empty()
            && self.wavetables.iter().all(Option::is_none)
            && self.master.is_bypass()
            && self.echo.is_bypass()
    }

    /// The lowest pattern id greater than `id`.
    pub fn next_pattern_after(&self, id: u8) -> Option<u8> {
        self.patterns
            .range(id.saturating_add(1)..)
            .next()
            .map(|(k, _)| *k)
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
        let (instruments, inst_by_name, wavetables, master, echo) = match inst_text {
            Some(t) => parse_instruments_section(t)?,
            None => (
                Vec::new(),
                BTreeMap::new(),
                [None; WAVETABLE_SLOTS],
                Master::OFF,
                Echo::OFF,
            ),
        };
        let tempo = match music_text {
            Some(t) => parse_tempo_line(t)?,
            None => None,
        };
        let sfx = match sfx_text {
            Some(t) => parse_sfx_section(t, &instruments, &inst_by_name, &wavetables, tempo)?,
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
            wavetables,
            tempo,
            master,
            echo,
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

fn parse_u8_in(section: &str, line: usize, what: &str, text: &str, max: u8) -> Result<u8, Error> {
    let v: u32 = text.parse().map_err(|_| {
        cart_err(
            section,
            line,
            format!("{what} must be a number, found {text:?}"),
        )
    })?;
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
    let v: i32 = text.parse().map_err(|_| {
        cart_err(
            section,
            line,
            format!("{what} must be a number, found {text:?}"),
        )
    })?;
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
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A row's 2nd token is a waveform when it is all digits; anything else is an
/// instrument name. `C4 9 7` therefore still reports "wave must be 0-5" rather
/// than "unknown instrument".
fn is_wave_digit(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit())
}

/// True for the `w<digits>` spelling that names a wavetable slot. Checked
/// *before* the instrument table, which is why instrument names may not look
/// like this ([`parse_inst_line`] rejects them).
fn is_wave_slot_token(token: &str) -> bool {
    match token.as_bytes() {
        [b'w' | b'W', rest @ ..] => !rest.is_empty() && rest.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// A wave *source* token on an `inst` line: a builtin digit `0`-`5`, `6` for
/// the 2-op FM oscillator, or `w0`-`w7` for a wavetable slot. Returns the
/// internal waveform id, which is [`WAVE_TABLE_BASE`]` + slot` for the
/// wavetable form.
///
/// Definedness is *not* checked here — the two callers differ on when they can
/// know (an `inst` line may precede the `wavetable` line it references), so
/// each validates against the slot table itself. Likewise `wave=6` is accepted
/// here and paired with its `fm=` parameters by [`parse_inst_line`], which is
/// the only place that can see the whole line.
fn parse_wave_source(section: &str, line: usize, what: &str, token: &str) -> Result<u8, Error> {
    if is_wave_slot_token(token) {
        let slot = parse_u8_in(
            section,
            line,
            &format!("{what} slot"),
            &token[1..],
            (WAVETABLE_SLOTS - 1) as u8,
        )?;
        return Ok(WAVE_TABLE_BASE + slot);
    }
    if is_wave_digit(token) {
        return parse_u8_in(section, line, what, token, WAVE_FM);
    }
    Err(cart_err(
        section,
        line,
        format!(
            "{what} must be 0-{WAVE_FM} (builtin) or w0-w{} (a wavetable), found {token:?}",
            WAVETABLE_SLOTS - 1
        ),
    ))
}

/// `fm=<ratio>,<index>[,<decay>]` — the 2-op FM parameters.
fn parse_fm_value(line: usize, value: &str) -> Result<Fm, Error> {
    const SEC: &str = "__instruments__";
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Err(cart_err(
            SEC,
            line,
            format!("fm must be `fm=<ratio>,<index>[,<decay>]`, found {value:?}"),
        ));
    }
    let ratio_half = parse_fm_ratio(line, parts[0])?;
    let index = parse_u8_in(SEC, line, "fm index", parts[1], MAX_FM_INDEX)?;
    let decay = match parts.get(2) {
        Some(t) => parse_u8_in(SEC, line, "fm decay", t, MAX_FM_DECAY)?,
        None => 0,
    };
    Ok(Fm {
        ratio_half,
        index,
        decay,
    })
}

/// The `<ratio>` field of `fm=`: `0.5` to `15` in steps of `0.5`, written the
/// way a musician writes it (`1`, `2`, `3.5`, `7`, `1.0` if they insist).
/// Returns the ratio in halves — see [`Fm::ratio_half`].
fn parse_fm_ratio(line: usize, text: &str) -> Result<u8, Error> {
    const SEC: &str = "__instruments__";
    let bad = || {
        cart_err(
            SEC,
            line,
            format!(
                "fm ratio must be {}-{} in steps of 0.5 \
                 (e.g. `0.5`, `1`, `2`, `3.5`, `7`), found {text:?}",
                f32::from(MIN_FM_RATIO_HALF) * 0.5,
                MAX_FM_RATIO_HALF / 2
            ),
        )
    };
    let (whole, half) = match text.split_once('.') {
        Some((w, "5")) => (w, 1u32),
        Some((w, "0")) => (w, 0u32),
        Some(_) => return Err(bad()),
        None => (text, 0u32),
    };
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let Ok(units) = whole.parse::<u32>() else {
        return Err(bad());
    };
    let halves = 2 * units + half;
    if !(u32::from(MIN_FM_RATIO_HALF)..=u32::from(MAX_FM_RATIO_HALF)).contains(&halves) {
        return Err(bad());
    }
    Ok(halves as u8)
}

/// "…, which the cart does not define" — the one error every undefined
/// wavetable reference reports, listing what *is* defined.
fn undefined_wavetable(
    section: &str,
    line: usize,
    who: &str,
    slot: u8,
    tables: &[Option<Wavetable>; WAVETABLE_SLOTS],
) -> Error {
    let defined: Vec<String> = tables
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_some())
        .map(|(i, _)| format!("w{i}"))
        .collect();
    let hint = if defined.is_empty() {
        "this cart defines no `wavetable` lines".to_string()
    } else {
        format!("defined: {}", defined.join(", "))
    };
    cart_err(
        section,
        line,
        format!("{who} references wavetable w{slot}, which the cart does not define ({hint})"),
    )
}

// ---- `__instruments__` -----------------------------------------------------

type InstTable = (
    Vec<Instrument>,
    BTreeMap<String, u8>,
    [Option<Wavetable>; WAVETABLE_SLOTS],
    Master,
    Echo,
);

fn parse_instruments_section(text: &str) -> Result<InstTable, Error> {
    const SEC: &str = "__instruments__";
    let mut list: Vec<Instrument> = Vec::new();
    let mut by_name: BTreeMap<String, u8> = BTreeMap::new();
    let mut wavetables: [Option<Wavetable>; WAVETABLE_SLOTS] = [None; WAVETABLE_SLOTS];
    let mut master: Option<Master> = None;
    let mut echo: Option<Echo> = None;
    // `inst` lines may reference a `wavetable` line further down the section
    // (same forward-reference rule as `__gfx_meta__`), so definedness is
    // checked once the whole section has parsed.
    let mut wave_refs: Vec<(usize, String, u8)> = Vec::new();

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
        // `echo` as the *first* token is the bus line. An instrument may still
        // be named `echo` (`inst echo wave=1 ...`), because that name is the
        // second token — the soundtest cart has had one since PoC v2.
        if tokens[0].eq_ignore_ascii_case("echo") {
            if echo.is_some() {
                return Err(cart_err(
                    SEC,
                    line,
                    "a cart may declare at most one `echo` line",
                ));
            }
            echo = Some(parse_echo_line(line, &tokens)?);
            continue;
        }
        if tokens[0].eq_ignore_ascii_case("wavetable") {
            let (slot, table) = parse_wavetable_line(line, &tokens)?;
            if wavetables[usize::from(slot)].is_some() {
                return Err(cart_err(SEC, line, format!("duplicate wavetable w{slot}")));
            }
            wavetables[usize::from(slot)] = Some(table);
            continue;
        }
        if !tokens[0].eq_ignore_ascii_case("inst") {
            return Err(cart_err(
                SEC,
                line,
                format!(
                    "expected `inst <name> wave=<0-6|w0-w7> ...`, \
                     `wavetable <slot 0-{}> <{WAVETABLE_LEN} hex nibbles>`, \
                     `master drive=<0-{MAX_DRIVE}> ...` or \
                     `echo delay=<1-{MAX_ECHO_DELAY}> ...`, found {:?}",
                    WAVETABLE_SLOTS - 1,
                    tokens[0]
                ),
            ));
        }
        let inst = parse_inst_line(line, &tokens)?;
        if inst.wave >= WAVE_TABLE_BASE {
            wave_refs.push((line, inst.name.clone(), inst.wave - WAVE_TABLE_BASE));
        }
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
    for (line, name, slot) in wave_refs {
        if wavetables[usize::from(slot)].is_none() {
            return Err(undefined_wavetable(
                SEC,
                line,
                &format!("instrument {name}"),
                slot,
                &wavetables,
            ));
        }
    }
    Ok((
        list,
        by_name,
        wavetables,
        master.unwrap_or_default(),
        echo.unwrap_or_default(),
    ))
}

/// `wavetable <slot 0-7> <32 hex nibbles>`.
///
/// The nibbles may be written as one 32-character run or split across as many
/// whitespace-separated groups as the author likes (`wavetable 0 89abcdef
/// fedcba98 …` reads far better in a cart than one long string) — every token
/// after the slot is concatenated, and the total must be exactly
/// [`WAVETABLE_LEN`] hex digits.
fn parse_wavetable_line(line: usize, tokens: &[&str]) -> Result<(u8, Wavetable), Error> {
    const SEC: &str = "__instruments__";
    if tokens.len() < 3 {
        return Err(cart_err(
            SEC,
            line,
            format!(
                "expected `wavetable <slot 0-{}> <{WAVETABLE_LEN} hex nibbles>` \
                 (the nibbles may be split into groups)",
                WAVETABLE_SLOTS - 1
            ),
        ));
    }
    let slot = parse_u8_in(
        SEC,
        line,
        "wavetable slot",
        tokens[1],
        (WAVETABLE_SLOTS - 1) as u8,
    )?;
    let digits: String = tokens[2..].concat();
    if digits.len() != WAVETABLE_LEN {
        return Err(cart_err(
            SEC,
            line,
            format!(
                "wavetable w{slot} needs exactly {WAVETABLE_LEN} hex nibbles, found {}",
                digits.len()
            ),
        ));
    }
    let mut nibbles = [0u8; WAVETABLE_LEN];
    for (i, c) in digits.chars().enumerate() {
        let Some(v) = c.to_digit(16) else {
            return Err(cart_err(
                SEC,
                line,
                format!("wavetable w{slot} sample {i}: {c:?} is not a hex nibble (0-9, a-f)"),
            ));
        };
        nibbles[i] = v as u8;
    }
    Ok((slot, Wavetable { nibbles }))
}

/// `echo delay=<1-60> feedback=<0-8> level=<0-8>`.
///
/// `delay` and `level` are required: they are the two endpoints that have to be
/// non-zero for the bus to do anything, so leaving one out is always a mistake
/// rather than a choice (a cart that wants no echo simply omits the line).
/// `feedback` is optional and defaults to 0 — a single slapback repeat.
fn parse_echo_line(line: usize, tokens: &[&str]) -> Result<Echo, Error> {
    const SEC: &str = "__instruments__";
    let mut delay: Option<u8> = None;
    let mut feedback = 0u8;
    let mut level: Option<u8> = None;

    for tok in &tokens[1..] {
        let Some((key, value)) = tok.split_once('=') else {
            return Err(cart_err(
                SEC,
                line,
                format!("unexpected {tok:?} in echo line (want `delay=`, `feedback=` or `level=`)"),
            ));
        };
        match key.to_ascii_lowercase().as_str() {
            "delay" => {
                delay = Some(parse_u8_range(
                    SEC,
                    line,
                    "echo delay",
                    value,
                    1,
                    MAX_ECHO_DELAY,
                )?);
            }
            "feedback" => {
                feedback = parse_u8_in(SEC, line, "echo feedback", value, MAX_ECHO_FEEDBACK)?;
            }
            "level" => level = Some(parse_u8_in(SEC, line, "echo level", value, MAX_ECHO_LEVEL)?),
            other => {
                return Err(cart_err(
                    SEC,
                    line,
                    format!("unknown echo key {other:?} (want `delay`, `feedback` or `level`)"),
                ));
            }
        }
    }

    let Some(delay) = delay else {
        return Err(cart_err(
            SEC,
            line,
            format!("echo needs `delay=<1-{MAX_ECHO_DELAY}>` (whole frames, 1 frame = 1/60 s)"),
        ));
    };
    let Some(level) = level else {
        return Err(cart_err(
            SEC,
            line,
            format!("echo needs `level=<0-{MAX_ECHO_LEVEL}>` (the return level; 0 = bus off)"),
        ));
    };
    Ok(Echo {
        delay,
        feedback,
        level,
    })
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
            "expected `inst <name> wave=<0-6|w0-w7> [fm=<ratio>,<index>,<decay>] \
             [env=<a>,<d>,<s>] [vib=<cents>,<rate>,<delay>] [sweep=<semis>,<frames>] \
             [duck=<depth>,<release>] [echo=<0-8>]`",
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
            format!(
                "instrument name {name:?} must not be a bare wave digit (0-5 already name the built-in waveforms)"
            ),
        ));
    }
    if is_wave_slot_token(name) {
        return Err(cart_err(
            SEC,
            line,
            format!(
                "instrument name {name:?} must not look like a wavetable slot \
                 (w0-w{} already name the wavetables)",
                WAVETABLE_SLOTS - 1
            ),
        ));
    }

    let mut wave: Option<u8> = None;
    let mut fm: Option<Fm> = None;
    let mut env: Option<Env> = None;
    let mut vib: Option<Vib> = None;
    let mut sweep: Option<Sweep> = None;
    let mut duck: Option<Duck> = None;
    let mut echo = 0u8;

    for tok in &tokens[2..] {
        let Some((key, value)) = tok.split_once('=') else {
            return Err(cart_err(
                SEC,
                line,
                format!(
                    "unexpected {tok:?} in inst line \
                     (want `wave=`, `fm=`, `env=`, `vib=`, `sweep=`, `duck=` or `echo=`)"
                ),
            ));
        };
        match key.to_ascii_lowercase().as_str() {
            "wave" => wave = Some(parse_wave_source(SEC, line, "wave", value)?),
            "fm" => fm = Some(parse_fm_value(line, value)?),
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
            "echo" => echo = parse_u8_in(SEC, line, "inst echo send", value, MAX_ECHO_SEND)?,
            other => {
                return Err(cart_err(
                    SEC,
                    line,
                    format!(
                        "unknown inst key {other:?} \
                         (want `wave`, `fm`, `env`, `vib`, `sweep`, `duck` or `echo`)"
                    ),
                ));
            }
        }
    }

    let Some(wave) = wave else {
        return Err(cart_err(
            SEC,
            line,
            format!("instrument {name} is missing `wave=<0-6>` (or `wave=w<slot>`)"),
        ));
    };
    // `wave=6` and `fm=` are two halves of one statement: neither is meaningful
    // alone, so neither is accepted alone. House style is a parse error rather
    // than a silent default — an FM patch nobody chose is a timbre nobody wants.
    if wave == WAVE_FM && fm.is_none() {
        return Err(cart_err(
            SEC,
            line,
            format!(
                "instrument {name} has `wave={WAVE_FM}` (2-op FM) but no \
                 `fm=<ratio>,<index>[,<decay>]`: FM has no useful default timbre, \
                 so the parameters are required (try `fm=1,6,12` for a bass)"
            ),
        ));
    }
    if wave != WAVE_FM && fm.is_some() {
        return Err(cart_err(
            SEC,
            line,
            format!(
                "instrument {name} has `fm=` but `wave={wave}`: FM parameters only \
                 mean anything on `wave={WAVE_FM}`"
            ),
        ));
    }
    Ok(Instrument {
        name: name.to_string(),
        wave,
        fm,
        env,
        vib,
        sweep,
        duck,
        echo,
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
        return Ok(Fx::Slide { semis: semis as i8 });
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
        format!(
            "unknown effect {token:?} (want `arp<a>,<b>`, `sl<n>`, `vib[<cents>,<rate>]` or `fade<n>`)"
        ),
    ))
}

// ---- `__sfx__` -------------------------------------------------------------

fn parse_sfx_section(
    text: &str,
    instruments: &[Instrument],
    inst_by_name: &BTreeMap<String, u8>,
    wavetables: &[Option<Wavetable>; WAVETABLE_SLOTS],
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
        // Column 2 is a bare wave digit (PoC v1), a `w<slot>` wavetable or an
        // instrument name. Both bare forms mean "the implicit flat instrument
        // on that waveform": no envelope, no vibrato, no echo send.
        let (wave, inst_index, inst) = if is_wave_digit(tokens[1]) {
            // A bare digit is the *self-contained* form, so it stops at 5:
            // wave 6 is FM and a digit carries no ratio or index. Saying so is
            // worth a bespoke message, because "wave must be 0-5" would read
            // like FM does not exist.
            if tokens[1].parse::<u64>().ok() == Some(u64::from(WAVE_FM)) {
                return Err(cart_err(
                    SEC,
                    line,
                    format!(
                        "wave {WAVE_FM} is the 2-op FM oscillator, which a bare digit cannot \
                         describe: declare `inst <name> wave={WAVE_FM} \
                         fm=<ratio>,<index>[,<decay>]` in __instruments__ and name it here"
                    ),
                ));
            }
            (
                parse_u8_in(SEC, line, "wave", tokens[1], WAVE_COUNT - 1)?,
                None,
                None,
            )
        } else if is_wave_slot_token(tokens[1]) {
            let wave = parse_wave_source(SEC, line, "wave", tokens[1])?;
            let slot = wave - WAVE_TABLE_BASE;
            if wavetables[usize::from(slot)].is_none() {
                return Err(undefined_wavetable(SEC, line, "row", slot, wavetables));
            }
            (wave, None, None)
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
            "want a wave digit 0-5, a wavetable w0-w{}, or one of: {}",
            WAVETABLE_SLOTS - 1,
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
        return Err(cart_err(
            SEC,
            line,
            format!("sfx {id} is missing `speed=<n>`"),
        ));
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
    body.get(..4)
        .is_some_and(|p| p.eq_ignore_ascii_case("bpm="))
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
    /// The [`Echo`] send of the instrument on the row currently playing, in
    /// eighths ([`ECHO_GAIN`]); 0 for a bare wave digit, i.e. fully dry.
    ///
    /// Set at note-on and left alone by rests, so a note's release tail keeps
    /// feeding the echo the way the note itself did. Cleared by
    /// [`Channel::stop`]: a released channel sends nothing.
    echo_send: u8,
    /// The FM patch of the row currently playing, if it named a `wave=6`
    /// instrument. `None` for every other voice, and then nothing in the FM
    /// path costs anything.
    ///
    /// Installed at note-on the way `echo_send` is, i.e. **immediately**,
    /// without waiting for the click guard's pending voice swap. The only
    /// window where that is visible is the ≤64-sample ramp-to-silence when a
    /// channel changes pitch, during which the outgoing tail briefly uses the
    /// incoming patch's ratio — inaudible under a fade to zero, and cheaper
    /// than a second pending slot.
    fm: Option<Fm>,
    /// The modulator's own phase accumulator. Continuous, exactly like the
    /// carrier's: notes are legato and nothing resets phase.
    mod_phase: u32,
    /// The **live** modulation index: [`Fm::index`] at note-on, then multiplied
    /// by [`FM_DECAY_MUL`] once per frame.
    fm_index: f32,
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
            echo_send: 0,
            fm: None,
            mod_phase: 0,
            fm_index: 0.0,
        }
    }

    /// Install (or clear) the FM patch a note-on asked for and re-arm the index
    /// envelope. A `None` patch is every non-FM voice.
    fn set_fm(&mut self, fm: Option<Fm>) {
        self.fm = fm;
        self.fm_index = fm.map_or(0.0, |f| f32::from(f.index));
    }

    /// Advance the index-decay envelope by one frame.
    ///
    /// Once per frame per channel, unconditionally, so the envelope is a
    /// function of elapsed time rather than of what the sequencer happened to
    /// do — and a no-op (one `Option` test) for every voice that is not FM.
    fn tick_fm(&mut self) {
        let Some(fm) = self.fm else { return };
        if fm.decay == 0 || self.fm_index == 0.0 {
            return;
        }
        self.fm_index *= FM_DECAY_MUL[usize::from(fm.decay.min(MAX_FM_DECAY))];
        if self.fm_index < FM_INDEX_FLOOR {
            self.fm_index = 0.0;
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
        self.echo_send = 0;
        self.fm = None;
        self.fm_index = 0.0;
    }

    fn next_sample(&mut self, lfsr: &mut u16, waves: &WaveSet) -> f32 {
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
        // 2-op FM: run the modulator off the *same* increment, scaled by the
        // ratio, and hand the carrier a displaced phase. Because the modulator
        // is derived from `inc` every sample rather than cached, vibrato,
        // slides, arpeggios and sweeps bend both operators together and the
        // ratio holds through all of them for free.
        let carrier_phase = match self.fm {
            Some(fm) if self.wave == WAVE_FM => {
                self.mod_phase = self
                    .mod_phase
                    .wrapping_add(fm_mod_increment(self.inc, fm.ratio_half));
                self.phase
                    .wrapping_add(fm_deviation(self.mod_phase, self.fm_index))
            }
            _ => self.phase,
        };
        self.amp * wave_value(self.wave, carrier_phase, *lfsr, waves)
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

/// One oscillator sample.
///
/// Waves 0..=5 are the builtin shapes, unchanged since PoC v1. [`WAVE_FM`] is
/// the FM carrier, a plain sine of the (already modulated) phase it is handed.
/// Anything at or above [`WAVE_TABLE_BASE`] is a wavetable slot: the top 5 bits
/// of the phase accumulator index the 32 precomputed amplitudes **with no
/// interpolation**.
///
/// *Why no interpolation*: the step edges are the sound. A 32-point table read
/// as a staircase is what a Game Boy, a VRC6 or an N163 does, and the aliasing
/// those edges throw off is the character the format is here to buy — smoothing
/// it would leave a dull, band-limited oscillator that the existing saw already
/// covers. It is also the cheapest possible read (one shift, one load, no
/// arithmetic), and it keeps the wavetable path exactly as bit-exact as the
/// builtin ones. Linear interpolation would be perfectly deterministic (it is
/// rational arithmetic on values from a const table), so the door is open for a
/// future per-instrument `interp=` flag — but the default must be crunchy.
fn wave_value(wave: u8, phase: u32, lfsr: u16, waves: &WaveSet) -> f32 {
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
        WAVE_NOISE => {
            if lfsr & 1 != 0 {
                1.0
            } else {
                -1.0
            }
        }
        // 2-op FM carrier. The caller ([`Channel::next_sample`]) has already
        // folded the modulator into `phase`, so all that is left here is the
        // carrier's own sine - which is exactly why `index=0` is a pure sine
        // and why every FM voice is one table lookup wide at this level.
        WAVE_FM => sine_at(phase),
        // wavetable slot: `phase >> 27` is 0..=31, so the index is total.
        // (Id 7 is unreachable - no cart syntax produces it - and wraps
        // harmlessly onto slot 7 rather than panicking.)
        w => {
            let slot = usize::from(w.wrapping_sub(WAVE_TABLE_BASE)) % WAVETABLE_SLOTS;
            waves.0[slot][(phase >> WAVETABLE_SHIFT) as usize]
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
    /// Current waveform: 0..=5 for the builtins, or 8 + slot (8..=15) for a
    /// wavetable voice, matching [`Instrument::wave`].
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
    /// The cart's wavetables as amplitudes, resolved once at load. Immutable
    /// for the life of the console — nothing at runtime can rewrite a table.
    waves: WaveSet,
    /// The live master setting: the cart's `master` line until Lua's
    /// `master()` overrides it.
    master: Master,
    /// Master bus memory (tone filter, hiss LFSR).
    mstate: MasterState,
    /// The live echo setting: the cart's `echo` line until Lua's `echo()`
    /// overrides it.
    echo: Echo,
    /// Echo bus memory (the delay line, its cursor, the loop filter).
    ebus: EchoBus,
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
        let echo = bank.echo();
        let waves = WaveSet::new(bank.wavetables());
        Audio {
            bank,
            channels: [const { Channel::new() }; CHANNEL_COUNT],
            music: None,
            lfsr: LFSR_SEED,
            waves,
            master,
            mstate: MasterState::new(),
            echo,
            ebus: EchoBus::new(),
            duck: DuckBus::new(),
            out: Box::new([0.0; SAMPLES_PER_FRAME]),
        }
    }

    /// The master bus setting in force right now: the cart's `master` line
    /// unless Lua's `master()` has overridden it.
    pub fn master(&self) -> Master {
        self.master
    }

    /// The echo bus setting in force right now: the cart's `echo` line unless
    /// Lua's `echo()` has overridden it.
    pub fn echo(&self) -> Echo {
        self.echo
    }

    /// True when the delay line holds nothing at all. Exposed for tests, which
    /// use it to prove that a killed bus really did forget its tail.
    pub fn echo_is_silent(&self) -> bool {
        self.ebus.is_silent()
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

    /// `echo(delay, [feedback], [level])`: replace the cart's `__instruments__`
    /// echo line for the rest of the session. Omitted arguments are 0, exactly
    /// like `master()`.
    ///
    /// The bus is switched off by anything that bypasses it — `echo(0)`,
    /// `echo(-1)` (the explicit "kill" spelling), `echo(0, 0, 0)` or any call
    /// with `level = 0`. Switching it off also **clears the delay line**, so
    /// re-enabling it later starts from silence instead of replaying whatever
    /// the previous scene left in there.
    ///
    /// Takes effect immediately, like `sfx()`/`music()`/`master()`: a call from
    /// `_update` colours the same frame's 735 samples. Changing the delay time
    /// while the bus stays on does *not* clear the line — the repeats already
    /// in flight jump to the new spacing, which is the tape-echo gesture.
    pub fn lua_echo(&mut self, delay: i32, feedback: i32, level: i32) -> Result<(), String> {
        if !(-1..=i32::from(MAX_ECHO_DELAY)).contains(&delay) {
            return Err(format!(
                "echo: delay {delay} out of range (expected 0-{MAX_ECHO_DELAY} frames, \
                 or -1 to switch the bus off)"
            ));
        }
        for (what, v, max) in [
            ("feedback", feedback, MAX_ECHO_FEEDBACK),
            ("level", level, MAX_ECHO_LEVEL),
        ] {
            if !(0..=i32::from(max)).contains(&v) {
                return Err(format!("echo: {what} {v} out of range (expected 0-{max})"));
            }
        }
        self.set_echo(Echo {
            // -1 is just a louder way of saying 0.
            delay: delay.max(0) as u8,
            feedback: feedback as u8,
            level: level as u8,
        });
        Ok(())
    }

    /// Install a new echo setting, flushing the delay line on the transition
    /// from running to off. Guarded on the transition rather than on the state,
    /// so a cart that calls `echo(0)` every frame (the way `soundtest.cart`
    /// calls `master(0)`) does not memset a second of audio 60 times a second.
    fn set_echo(&mut self, next: Echo) {
        if next.is_bypass() && !self.echo.is_bypass() {
            self.ebus.clear();
        }
        self.echo = next;
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
            return Err(format!(
                "music: pattern {} {}",
                id,
                self.missing_pattern_hint()
            ));
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
            format!(
                "is not defined (this cart defines patterns {})",
                ids.join(", ")
            )
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
            // The FM index-decay envelope runs on every channel, cursor or not:
            // it belongs to the note that is sounding, not to the sequencer. A
            // note-on later in this same loop re-arms it (see `apply_row`).
            c.tick_fm();
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
    ///                        ┌──────── feedback * 7/64 ◄───────┐
    ///                        │                                 │
    ///  channels ─► duck ─┬─► echo send ─►(+)─► delay line ─► loop LP ─┘
    ///   (6 voices) gain  │    (0-8)/8                │
    ///                    │                           ▼ * level/8
    ///                    └────── dry sum ──────►(+)◄─┘
    ///                                            │
    ///                                            ▼
    ///                          * 0.25 ─► drive/shaper ─► tone LP ─► hiss ─► clamp
    /// ```
    ///
    /// The **insertion point** for the master bus is the `acc * MIX_GAIN`
    /// product: everything new consumes that value and replaces the plain
    /// `clamp(-1, 1)` that used to be applied to it (the clamp survives as a
    /// final safety net, because the hiss adds after the shaper has already
    /// bounded the signal). Ducking sits one step earlier still, on the
    /// per-channel samples going into the sum, so a driven mix pumps — the
    /// shaper sees the ducked signal.
    ///
    /// The echo bus sits between them. Each voice's send is taken **post-duck**
    /// — the same value that enters the dry sum — so the echo pumps with the
    /// kick instead of filling in the hole the sidechain just dug. The return
    /// is added to the dry sum *before* `MIX_GAIN`, so `level=8` really is
    /// "as loud as a voice at unity send" and the whole thing is one number the
    /// master stage can work on. The return itself is not re-ducked: it is an
    /// aux return, and it already inherited the pump on the way in.
    ///
    /// **Headroom.** The echo adds energy, and nothing in the bus stops it
    /// exceeding full scale: six voices at `echo=8` into `feedback=8`
    /// (`7/8`) settle at up to `6 / (1 - 7/8) = 48` in channel units, which
    /// `level=8` returns in full. As with six loud dry voices, the no-drive
    /// path then hard-clips at the final clamp and any non-zero `master drive`
    /// soft-limits below full scale instead (`MAKEUP[drive] < 1`). The bus is
    /// bounded and finite at every setting — `echo_stress_*` in
    /// `tests/audio.rs` pins that — but "bounded" is not "clean": author sends
    /// at 2-4 and reach for `master drive=1` if the tail gets crunchy.
    ///
    /// When nothing is engaged — no `master` line / `master(0)`, no `echo`
    /// line / `echo(0)` *and* no duck envelope running — the loop is the PoC v1
    /// statement, character for character, so old carts render bit-identical
    /// samples. (The general path would agree anyway: its duck gain is exactly
    /// `1.0` when idle and `x * 1.0 == x` in IEEE-754. The split is for clarity
    /// and speed, not safety.)
    pub fn render(&mut self) {
        let Audio {
            channels,
            lfsr,
            waves,
            master,
            mstate,
            echo,
            ebus,
            duck,
            out,
            ..
        } = self;

        if master.is_bypass() && duck.is_idle() && echo.is_bypass() {
            for slot in out.iter_mut() {
                let mut acc = 0.0f32;
                for c in channels.iter_mut() {
                    acc += c.next_sample(lfsr, waves);
                }
                *slot = (acc * MIX_GAIN).clamp(-1.0, 1.0);
            }
            return;
        }

        let pre = PRE_GAIN[usize::from(master.drive)];
        let makeup = MAKEUP[usize::from(master.drive)];
        let a = TONE_A[usize::from(master.tone)];
        let hiss = HISS_LEVEL[usize::from(master.hiss)];

        let echo_on = !echo.is_bypass();
        let edelay = echo.delay_samples();
        let efb = ECHO_FB[usize::from(echo.feedback.min(MAX_ECHO_FEEDBACK))];
        let elevel = ECHO_GAIN[usize::from(echo.level.min(MAX_ECHO_LEVEL))];

        for slot in out.iter_mut() {
            // ---- sidechain duck, then the channel sum ----------------------
            let g = duck.gain();
            let exempt = duck.trigger_ch;
            let mut acc = 0.0f32;
            let mut send = 0.0f32;
            for (i, c) in channels.iter_mut().enumerate() {
                let s = c.next_sample(lfsr, waves);
                let s = if exempt == Some(i as u8) { s } else { s * g };
                acc += s;
                // `echo_send == 0` is both the default and the common case, so
                // a dry voice costs one integer comparison and nothing else.
                if echo_on && c.echo_send != 0 {
                    send += s * ECHO_GAIN[usize::from(c.echo_send.min(MAX_ECHO_SEND))];
                }
            }
            duck.tick();

            // ---- echo: delay line with feedback and a loop lowpass ---------
            if echo_on {
                acc += ebus.tick(send, edelay, efb) * elevel;
            }

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
/// any), fire the sidechain if the row's instrument is a duck trigger and
/// install that instrument's echo send.
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
            // The echo send follows the row's instrument, so a bare wave digit
            // (or an instrument without `echo=`) puts the channel back to dry.
            c.echo_send = named.map_or(0, |i| i.echo);
            // Same rule for the FM patch, and this is also the note-on that
            // re-arms the index-decay envelope: the brightness gesture restarts
            // on every struck note, which is the whole point of it.
            c.set_fm(named.and_then(|i| i.fm));
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

    /// Every slot filled with a full-scale ramp, for the oscillator tests.
    fn ramp_waves() -> WaveSet {
        let mut nibbles = [0u8; WAVETABLE_LEN];
        for (i, n) in nibbles.iter_mut().enumerate() {
            *n = (i / 2) as u8;
        }
        WaveSet::new(&[Some(Wavetable { nibbles }); WAVETABLE_SLOTS])
    }

    // ---- the FM sine table -------------------------------------------------

    #[test]
    fn the_sine_table_is_a_monotone_quarter_with_exact_endpoints() {
        assert_eq!(SINE_QUARTER.len(), 257);
        assert_eq!(SINE_QUARTER[0], 0.0);
        assert_eq!(SINE_QUARTER[256], 1.0);
        // Strictly increasing across the quarter: no duplicated or transposed
        // literal can hide in 257 pasted numbers.
        assert!(SINE_QUARTER.windows(2).all(|w| w[0] < w[1]));
        // ...and it really is a sine, to the accuracy the table can hold.
        for (k, &v) in SINE_QUARTER.iter().enumerate() {
            let want = (std::f64::consts::TAU * k as f64 / 1024.0).sin();
            assert!(
                (f64::from(v) - want).abs() < 1e-7,
                "SINE_QUARTER[{k}] = {v}, want {want}"
            );
        }
    }

    #[test]
    fn sine_at_hits_its_zeros_and_peaks_exactly() {
        assert_eq!(sine_at(0), 0.0);
        assert_eq!(sine_at(0x4000_0000), 1.0);
        assert_eq!(sine_at(0x8000_0000), 0.0);
        assert_eq!(sine_at(0xc000_0000), -1.0);
        // Nothing ever leaves the unit interval, interpolation included.
        for step in 0..4096u32 {
            let v = sine_at(step.wrapping_mul(0x0010_0000).wrapping_add(12_345));
            assert!((-1.0..=1.0).contains(&v), "sine_at produced {v}");
        }
    }

    #[test]
    fn sine_at_is_odd_and_half_cycle_antisymmetric() {
        // Both symmetries are *bit* exact, which is the reason the table is a
        // quarter wave derived by reflection rather than a full cycle of
        // independently rounded literals.
        for step in 0..2048u32 {
            let p = step.wrapping_mul(0x0020_0000).wrapping_add(64 * 777);
            assert_eq!(
                sine_at(p.wrapping_neg()).to_bits(),
                (-sine_at(p)).to_bits(),
                "sine_at(-p) != -sine_at(p) at {p:#010x}"
            );
            assert_eq!(
                sine_at(p.wrapping_add(0x8000_0000)).to_bits(),
                (-sine_at(p)).to_bits(),
                "half-cycle antisymmetry broke at {p:#010x}"
            );
        }
    }

    #[test]
    fn sine_at_crosses_zero_exactly_twice_per_cycle() {
        let mut crossings = 0;
        let mut prev = sine_at(0);
        for step in 1..=4096u32 {
            let v = sine_at(step.wrapping_mul(0x0010_0000));
            if (prev < 0.0) != (v < 0.0) {
                crossings += 1;
            }
            prev = v;
        }
        assert_eq!(crossings, 2, "one cycle of a sine has two sign changes");
    }

    #[test]
    fn the_fm_ratio_scales_the_modulator_increment_exactly() {
        // Halves, in exact integer arithmetic, with no rounding to argue about.
        assert_eq!(fm_mod_increment(1000, 2), 1000);
        assert_eq!(fm_mod_increment(1000, 1), 500);
        assert_eq!(fm_mod_increment(1000, 7), 3500);
        assert_eq!(fm_mod_increment(1000, 30), 15_000);
        // A modulator pushed past the accumulator wraps rather than saturating:
        // that is aliasing, which is what a digital oscillator does.
        assert_eq!(fm_mod_increment(0x8000_0000, 4), 0);
    }

    #[test]
    fn a_zero_index_deviates_the_carrier_by_nothing() {
        for step in 0..256u32 {
            assert_eq!(fm_deviation(step.wrapping_mul(0x0100_0000), 0.0), 0);
        }
    }

    #[test]
    fn the_index_decay_ladder_halves_on_schedule() {
        assert_eq!(FM_DECAY_MUL[0], 1.0);
        assert_eq!(FM_DECAY_HALF_LIFE[0], 0);
        // Faster settings decay faster, monotonically.
        assert!(FM_DECAY_MUL[1..].windows(2).all(|w| w[0] > w[1]));
        assert!(FM_DECAY_HALF_LIFE[1..].windows(2).all(|w| w[0] > w[1]));
        for d in 1..16usize {
            let hl = u32::from(FM_DECAY_HALF_LIFE[d]);
            let mut x = 1.0f32;
            for _ in 0..hl {
                x *= FM_DECAY_MUL[d];
            }
            assert!(
                (x - 0.5).abs() < 1e-4,
                "decay {d}: {hl} frames took the index to {x}, not 1/2"
            );
        }
    }

    #[test]
    fn waveforms_stay_in_range() {
        let waves = ramp_waves();
        for wave in (0..WAVE_COUNT).chain(WAVE_TABLE_BASE..WAVE_TABLE_BASE + 8) {
            for step in 0..512u32 {
                let phase = step.wrapping_mul(0x0080_0000);
                let v = wave_value(wave, phase, 0xACE1, &waves);
                assert!((-1.0..=1.0).contains(&v), "wave {wave} produced {v}");
            }
        }
    }

    #[test]
    fn triangle_is_continuous_at_the_peak() {
        let waves = WaveSet::new(&[None; WAVETABLE_SLOTS]);
        let a = wave_value(3, 0x7fff_ff00, 0, &waves);
        let b = wave_value(3, 0x8000_0000, 0, &waves);
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }

    #[test]
    fn the_nibble_ladder_is_symmetric_and_full_scale() {
        assert_eq!(NIBBLE_LEVEL[0], -1.0);
        assert_eq!(NIBBLE_LEVEL[15], 1.0);
        for n in 0..16 {
            // Code n and code 15-n are exact negations: a table and its
            // mirror image cancel to zero DC, bit for bit.
            assert_eq!(NIBBLE_LEVEL[n].to_bits(), (-NIBBLE_LEVEL[15 - n]).to_bits());
        }
        // Monotone, and the step is uniform (2/15 per code).
        assert!(NIBBLE_LEVEL.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_wavetable_reads_the_top_five_bits_of_the_phase() {
        let waves = ramp_waves();
        for i in 0..WAVETABLE_LEN {
            // Both ends of the sample's phase span read the same entry.
            let lo = (i as u32) << WAVETABLE_SHIFT;
            let hi = lo | ((1 << WAVETABLE_SHIFT) - 1);
            let expect = NIBBLE_LEVEL[i / 2];
            for phase in [lo, hi] {
                assert_eq!(
                    wave_value(WAVE_TABLE_BASE, phase, 0, &waves).to_bits(),
                    expect.to_bits(),
                    "sample {i} at phase {phase:#010x}"
                );
            }
        }
    }

    // -----------------------------------------------------------------
    // PoC v2: pitch math, LFO, envelope and effect trajectories
    // -----------------------------------------------------------------

    #[test]
    fn whole_semitones_hit_the_table_exactly() {
        for note in 0..96u8 {
            assert_eq!(
                freq_at(note, 0.0).to_bits(),
                NOTE_FREQ[note as usize].to_bits()
            );
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
        assert_eq!(
            offs,
            vec![0.0, -2.0, -4.0, -6.0, -8.0, -10.0, -12.0, -12.0, -12.0]
        );
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
            vec![
                0.0, 0.0, 4.0, 4.0, 7.0, 7.0, 0.0, 0.0, 4.0, 4.0, 7.0, 7.0, 0.0, 0.0
            ]
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
        assert_eq!(
            trough.freq(),
            NOTE_FREQ[57] * (1.0 - 100.0 * CENTS_TO_RATIO)
        );
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
            assert!(
                (-1.0..=1.0).contains(&y),
                "shape({x}) = {y} escaped [-1, 1]"
            );
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
            let m = (MASTER_REF_LEVEL / shape64(f64::from(PRE_GAIN[d]) * MASTER_REF_LEVEL)) as f32;
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
        assert!(
            !Master {
                drive: 1,
                tone: 0,
                hiss: 0
            }
            .is_bypass()
        );
        assert!(
            !Master {
                drive: 0,
                tone: 1,
                hiss: 0
            }
            .is_bypass()
        );
        assert!(
            !Master {
                drive: 0,
                tone: 0,
                hiss: 1
            }
            .is_bypass()
        );
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
        d.trigger(
            0,
            Duck {
                depth: 7,
                release: 4,
            },
        );
        for _ in 0..DUCK_ATTACK_SAMPLES + 2000 {
            d.tick();
        }
        assert!(d.atten < 1.0 && d.atten > 0.0, "mid-release: {}", d.atten);

        // Re-fire from a different channel: full depth again after one attack
        // window, and the new channel is the exempt one.
        d.trigger(
            3,
            Duck {
                depth: 7,
                release: 4,
            },
        );
        assert_eq!(d.trigger_ch, Some(3));
        for _ in 0..DUCK_ATTACK_SAMPLES {
            d.tick();
        }
        assert_eq!(d.atten, 1.0, "re-trigger must reach full depth again");

        // A shallower trigger over a deeper one ramps *down* to the new depth.
        d.trigger(
            1,
            Duck {
                depth: 2,
                release: 4,
            },
        );
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
        d.trigger(
            0,
            Duck {
                depth: 7,
                release: 1,
            },
        );
        for _ in 0..DUCK_ATTACK_SAMPLES {
            d.tick();
        }
        assert_eq!(d.gain(), 0.0);
    }

    // -----------------------------------------------------------------
    // Echo bus: tables, the delay line, the feedback loop
    // -----------------------------------------------------------------

    #[test]
    fn echo_gain_tables_are_exact_binary_fractions() {
        assert_eq!(ECHO_FB.len(), usize::from(MAX_ECHO_FEEDBACK) + 1);
        assert_eq!(ECHO_GAIN.len(), usize::from(MAX_ECHO_LEVEL) + 1);
        assert_eq!(ECHO_GAIN.len(), usize::from(MAX_ECHO_SEND) + 1);
        assert_eq!(ECHO_FB[0], 0.0);
        assert_eq!(ECHO_GAIN[0], 0.0);
        for f in 0..=usize::from(MAX_ECHO_FEEDBACK) {
            // f * 7/64, exactly.
            assert_eq!(ECHO_FB[f], f as f32 * (7.0 / 64.0));
            // ...and eighths for the level/send ladder.
            assert_eq!(ECHO_GAIN[f], f as f32 / 8.0);
        }
        // Monotonic in both.
        assert!(ECHO_FB.windows(2).all(|w| w[0] < w[1]));
        assert!(ECHO_GAIN.windows(2).all(|w| w[0] < w[1]));
        // The load-bearing invariant: the loop can never reach unity, so it
        // always decays. 7/8 at the top.
        assert_eq!(ECHO_FB[usize::from(MAX_ECHO_FEEDBACK)], 0.875);
        assert!(ECHO_FB[usize::from(MAX_ECHO_FEEDBACK)] < 1.0);
        // The return tops out at exactly unity.
        assert_eq!(ECHO_GAIN[usize::from(MAX_ECHO_LEVEL)], 1.0);
    }

    #[test]
    fn echo_loop_filter_matches_its_generator() {
        let fc = f64::from(ECHO_LP_CUTOFF_HZ);
        // The authoring-time formula. `exp` lives here, in a test, and never in
        // the render path.
        let a = (1.0 - (-2.0 * std::f64::consts::PI * fc / 44100.0).exp()) as f32;
        assert_eq!(
            ECHO_LP_A.to_bits(),
            a.to_bits(),
            "ECHO_LP_A = {ECHO_LP_A} but 1 - exp(-2*pi*{fc}/44100) = {a}"
        );
        // A lowpass, not a passthrough and not a resonator.
        assert!((0.0..1.0).contains(&ECHO_LP_A));
        // Its DC gain is exactly 1, which is *why* the feedback gain has to be
        // the thing that keeps the loop stable: feed a constant in and the
        // filter converges to it rather than shrinking it.
        let mut y = 0.0f32;
        for _ in 0..10_000 {
            y += ECHO_LP_A * (1.0 - y);
        }
        assert!((y - 1.0).abs() < 1e-6, "loop filter DC gain is {y}, not 1");
    }

    #[test]
    fn echo_delay_is_whole_frames_and_the_line_is_one_second() {
        assert_eq!(
            ECHO_LINE_LEN,
            usize::from(MAX_ECHO_DELAY) * SAMPLES_PER_FRAME
        );
        assert_eq!(ECHO_LINE_LEN, SAMPLE_RATE as usize, "60 frames = 1 second");
        for d in 0..=MAX_ECHO_DELAY {
            let e = Echo {
                delay: d,
                feedback: 0,
                level: 4,
            };
            assert_eq!(e.delay_samples(), usize::from(d) * SAMPLES_PER_FRAME);
        }
        // 16.67 ms a step: essentially the SNES EDL's 16 ms grid.
        assert_eq!(
            Echo {
                delay: 1,
                feedback: 0,
                level: 8
            }
            .delay_samples(),
            735
        );
    }

    #[test]
    fn echo_defaults_to_a_full_bypass() {
        assert_eq!(Echo::default(), Echo::OFF);
        assert!(Echo::default().is_bypass());
        // Either endpoint at zero switches the whole bus off.
        assert!(
            Echo {
                delay: 0,
                feedback: 8,
                level: 8
            }
            .is_bypass()
        );
        assert!(
            Echo {
                delay: 30,
                feedback: 8,
                level: 0
            }
            .is_bypass()
        );
        assert!(
            !Echo {
                delay: 1,
                feedback: 0,
                level: 1
            }
            .is_bypass()
        );
    }

    /// Push one impulse into a bus and collect `n` samples of what comes back
    /// out of the delay line (before the return level).
    fn echo_impulse(delay: usize, feedback: f32, n: usize) -> Vec<f32> {
        let mut bus = EchoBus::new();
        (0..n)
            .map(|i| bus.tick(if i == 0 { 1.0 } else { 0.0 }, delay, feedback))
            .collect()
    }

    #[test]
    fn echo_repeats_land_on_exact_sample_offsets() {
        let delay = 100;
        let out = echo_impulse(delay, ECHO_FB[8], delay * 4 + 4);
        // Nothing comes back before the delay has elapsed...
        assert!(
            out[..delay].iter().all(|&s| s == 0.0),
            "the line leaked before the first repeat"
        );
        // ...the first repeat starts exactly `delay` samples later...
        assert!(out[delay] > 0.0, "no first repeat at sample {delay}");
        // ...and so does every later one, because the loop is exactly one
        // delay long. (The impulse is smeared by the loop filter, so the
        // *energy* rather than a single sample is what recurs.)
        let energy = |k: usize| -> f32 {
            out[k * delay..(k * delay + delay).min(out.len())]
                .iter()
                .map(|s| s.abs())
                .sum()
        };
        assert_eq!(energy(0), 0.0);
        for k in 1..4 {
            assert!(energy(k) > 0.0, "repeat {k} is missing");
        }
    }

    #[test]
    fn echo_feedback_always_decays() {
        // Every feedback setting, including the maximum, loses energy per lap.
        for (f, &fb) in ECHO_FB.iter().enumerate().skip(1) {
            let delay = 64;
            let out = echo_impulse(delay, fb, delay * 6);
            let energy = |k: usize| -> f32 {
                out[k * delay..(k + 1) * delay]
                    .iter()
                    .map(|s| s.abs())
                    .sum()
            };
            for k in 1..5 {
                assert!(
                    energy(k + 1) < energy(k),
                    "feedback {f}: repeat {} is not quieter than {k}",
                    k + 1
                );
            }
            // ...and it is heading to silence, not to a floor.
            assert!(energy(5) < energy(1) * 0.95);
        }
        // Zero feedback is a single slapback: one repeat and nothing after it.
        let delay = 32;
        let out = echo_impulse(delay, ECHO_FB[0], delay * 4);
        assert!(out[delay..delay * 2].iter().any(|&s| s != 0.0));
        let tail: f32 = out[delay * 2..].iter().map(|s| s.abs()).sum();
        assert!(tail < 1e-6, "slapback left a tail of {tail}");
    }

    #[test]
    fn echo_line_is_bounded_at_maximum_feedback() {
        // Hold the input at the worst case a mix can produce (six full-scale
        // voices, pre-MIX_GAIN) with maximum feedback for a long time: the
        // geometric series converges to input / (1 - 7/8) = 8 * input.
        let mut bus = EchoBus::new();
        let mut peak = 0.0f32;
        for _ in 0..ECHO_LINE_LEN * 4 {
            let out = bus.tick(6.0, 735, ECHO_FB[8]);
            assert!(out.is_finite(), "the delay line produced {out}");
            peak = peak.max(out.abs());
        }
        assert!(peak <= 48.0 + 1e-3, "line ran away to {peak}");
        assert!(peak > 40.0, "the probe never charged the line ({peak})");

        // Cut the input and the line drains to exactly zero (the denormal
        // floor snaps the last of it), rather than ringing forever.
        for _ in 0..ECHO_LINE_LEN * 60 {
            bus.tick(0.0, 735, ECHO_FB[8]);
        }
        assert!(bus.is_silent(), "the line never emptied");
    }

    #[test]
    fn echo_clear_forgets_everything() {
        let mut bus = EchoBus::new();
        assert!(bus.is_silent());
        for _ in 0..2000 {
            bus.tick(0.5, 735, ECHO_FB[4]);
        }
        assert!(!bus.is_silent());
        bus.clear();
        assert!(bus.is_silent());
        assert_eq!(bus.pos, 0);
        assert_eq!(bus.lp, 0.0);
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

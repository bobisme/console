//! Audio: `__sfx__`/`__music__` parsing, the sequencer, the synth and the
//! determinism contract.

use console_core::{
    CHANNEL_COUNT, Cart, Console, DUCK_ATTACK_SAMPLES, Duck, Echo, Env, Error, Fx,
    MASTER_REF_LEVEL, MAX_DRIVE, MAX_DUCK_DEPTH, MAX_HISS, MAX_TONE, Master, NIBBLE_LEVEL,
    NOTE_FREQ, PatternEnd, RowMod, SAMPLE_RATE, SAMPLES_PER_FRAME, SfxRow, Sweep, Vib, WAVE_COUNT,
    WAVE_TABLE_BASE, WAVETABLE_LEN, WAVETABLE_SLOTS, Wavetable, freq_at, input,
};

const DEMO: &str = include_str!("../../../carts/demo.cart");
const SOUNDTEST: &str = include_str!("../../../carts/soundtest.cart");

/// FNV-1a, 64-bit. Inline so the test suite needs no dependencies.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Hash a sample stream by its exact bit patterns, little-endian.
fn hash_samples(samples: &[f32]) -> u64 {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        bytes.extend_from_slice(&s.to_bits().to_le_bytes());
    }
    fnv1a(&bytes)
}

fn console(body: &str) -> Console {
    Console::new(body, 0).expect("cart should load")
}

/// Step `frames` times with no input, collecting every rendered sample.
fn collect(con: &mut Console, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames * SAMPLES_PER_FRAME);
    for _ in 0..frames {
        con.step(0).expect("frame runs");
        out.extend_from_slice(con.audio_frame());
    }
    out
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

const PARSE_CART: &str = "\
__lua__
x = 1

__sfx__
# a comment line
sfx 0 speed=4
C0 0 7
C#4 1 6
---
B7 5 0

sfx 63 speed=255 loop=1,2
A4 2 3
G#3 3 4
D#5 4 5

__music__
pat 0 : 0 - - 63
pat 7 stop : - 63 - -
pat 9 loop=0 : 0 0 0 0 0 0
";

#[test]
fn sfx_section_round_trips() {
    let cart = Cart::parse(PARSE_CART).unwrap();

    let ids: Vec<u8> = cart.audio().sfx_ids().collect();
    assert_eq!(ids, vec![0, 63]);

    let a = cart.sfx(0).unwrap();
    assert_eq!(a.speed, 4);
    assert_eq!(a.loop_range, None);
    assert_eq!(a.rows.len(), 4);
    assert_eq!(a.rows[0], SfxRow::Note { note: 0, wave: 0, vol: 7 }); // C0
    assert_eq!(a.rows[1], SfxRow::Note { note: 49, wave: 1, vol: 6 }); // C#4
    assert_eq!(a.rows[2], SfxRow::Rest);
    assert_eq!(a.rows[3], SfxRow::Note { note: 95, wave: 5, vol: 0 }); // B7
    assert_eq!(a.duration(), 16);

    let b = cart.sfx(63).unwrap();
    assert_eq!(b.speed, 255);
    assert_eq!(b.loop_range, Some((1, 2)));
    assert_eq!(b.rows[0], SfxRow::Note { note: 57, wave: 2, vol: 3 }); // A4
    assert_eq!(b.rows[1], SfxRow::Note { note: 44, wave: 3, vol: 4 }); // G#3
    assert_eq!(b.rows[2], SfxRow::Note { note: 63, wave: 4, vol: 5 }); // D#5

    assert!(cart.sfx(1).is_none());
}

#[test]
fn music_section_round_trips() {
    let cart = Cart::parse(PARSE_CART).unwrap();

    let ids: Vec<u8> = cart.audio().pattern_ids().collect();
    assert_eq!(ids, vec![0, 7, 9]);

    let p0 = cart.pattern(0).unwrap();
    // A 4-slot line parses exactly as it always did; channels 4/5 stay silent.
    assert_eq!(p0.slots, [Some(0), None, None, Some(63), None, None]);
    assert_eq!(p0.end, PatternEnd::Next);

    let p7 = cart.pattern(7).unwrap();
    assert_eq!(p7.slots, [None, Some(63), None, None, None, None]);
    assert_eq!(p7.end, PatternEnd::Stop);

    // ...and a full six-slot line fills every channel.
    let p9 = cart.pattern(9).unwrap();
    assert_eq!(p9.slots, [Some(0); CHANNEL_COUNT]);
    assert_eq!(p9.end, PatternEnd::Loop(0));

    assert_eq!(cart.audio().next_pattern_after(0), Some(7));
    assert_eq!(cart.audio().next_pattern_after(7), Some(9));
    assert_eq!(cart.audio().next_pattern_after(9), None);
}

/// A `pat` line may list 4, 5 or 6 slots. Four is the pre-widening format and
/// must keep parsing byte-for-byte identically; the slots a shorter line omits
/// are silent, exactly like an explicit `-`.
#[test]
fn pattern_lines_accept_four_to_six_slots() {
    const HEAD: &str = "__lua__\n\n__sfx__\nsfx 0 speed=1\nC4 0 1\n\n__music__\n";

    let four = Cart::parse(&format!("{HEAD}pat 0 : 0 - - 0\n")).unwrap();
    assert_eq!(
        four.pattern(0).unwrap().slots,
        [Some(0), None, None, Some(0), None, None]
    );

    let five = Cart::parse(&format!("{HEAD}pat 0 : 0 - - 0 0\n")).unwrap();
    assert_eq!(
        five.pattern(0).unwrap().slots,
        [Some(0), None, None, Some(0), Some(0), None]
    );

    let six = Cart::parse(&format!("{HEAD}pat 0 : 0 - - 0 0 -\n")).unwrap();
    assert_eq!(
        six.pattern(0).unwrap().slots,
        [Some(0), None, None, Some(0), Some(0), None]
    );

    // A trailing `-` is exactly the same pattern as leaving the slot off.
    assert_eq!(five.pattern(0), six.pattern(0));

    // Flags still parse in front of the wider slot list.
    let flagged = Cart::parse(&format!("{HEAD}pat 3 loop=3 : - - - - - 0\n")).unwrap();
    assert_eq!(flagged.pattern(3).unwrap().end, PatternEnd::Loop(3));
    assert_eq!(
        flagged.pattern(3).unwrap().slots,
        [None, None, None, None, None, Some(0)]
    );
}

#[test]
fn octave_bounds_and_sharps() {
    // Every legal note name parses, and only those.
    let cart = Cart::parse("__lua__\n\n__sfx__\nsfx 0 speed=1\nC0 0 1\nB7 0 1\nA#0 0 1\nF#7 0 1\n")
        .unwrap();
    let rows = &cart.sfx(0).unwrap().rows;
    let notes: Vec<u8> = rows
        .iter()
        .map(|r| match r {
            SfxRow::Note { note, .. } => *note,
            SfxRow::Rest => panic!("no rests here"),
        })
        .collect();
    assert_eq!(notes, vec![0, 95, 10, 90]);

    for bad in ["C8", "B#7", "H4", "Cb4", "C-1", "C"] {
        let text = format!("__lua__\n\n__sfx__\nsfx 0 speed=1\n{bad} 0 1\n");
        let err = Cart::parse(&text).unwrap_err();
        assert!(err.to_string().contains("bad note"), "{bad}: {err}");
    }
}

/// Every malformed input must be an `Error::Cart` naming the offending line.
fn expect_cart_error(text: &str, line: usize, needle: &str) {
    let err = Cart::parse(text).unwrap_err();
    assert!(matches!(err, Error::Cart(_)), "{err:?}");
    let msg = err.to_string();
    assert!(
        msg.contains(&format!("line {line}")),
        "expected line {line} in: {msg}"
    );
    assert!(msg.contains(needle), "expected {needle:?} in: {msg}");
}

#[test]
fn malformed_sfx_is_a_line_numbered_cart_error() {
    expect_cart_error(
        "__lua__\n\n__sfx__\nsfx 0 speed=2\nC4 0 7\nQ9 0 7\n",
        3,
        "bad note",
    );
    expect_cart_error(
        "__lua__\n\n__sfx__\nsfx 0 speed=2\nC4 6 7\n",
        2,
        "wave must be 0-5",
    );
    expect_cart_error(
        "__lua__\n\n__sfx__\nsfx 0 speed=2\nC4 0 8\n",
        2,
        "vol must be 0-7",
    );
    expect_cart_error(
        "__lua__\n\n__sfx__\nsfx 0 speed=2\nC4 0\n",
        2,
        "expected `NOTE WAVE VOL [FX]`",
    );
    expect_cart_error("__lua__\n\n__sfx__\nsfx 64 speed=1\nC4 0 1\n", 1, "0-63");
    expect_cart_error("__lua__\n\n__sfx__\nsfx 0 speed=0\nC4 0 1\n", 1, "speed must be");
    expect_cart_error("__lua__\n\n__sfx__\nsfx 0\nC4 0 1\n", 1, "missing `speed=");
    expect_cart_error("__lua__\n\n__sfx__\nsfx 0 speed=1\n", 1, "has no rows");
    expect_cart_error("__lua__\n\n__sfx__\nC4 0 1\n", 1, "before any");
    expect_cart_error(
        "__lua__\n\n__sfx__\nsfx 0 speed=1 loop=0,4\nC4 0 1\nC4 0 1\n",
        1,
        "past the last row",
    );
    expect_cart_error(
        "__lua__\n\n__sfx__\nsfx 0 speed=1 loop=3,1\nC4 0 1\n",
        1,
        "loop start 3 is after",
    );
    expect_cart_error(
        "__lua__\n\n__sfx__\nsfx 0 speed=1\nC4 0 1\n\nsfx 0 speed=1\nC4 0 1\n",
        4,
        "duplicate sfx id 0",
    );

    // 33 rows is one too many; the error names the 33rd row's line.
    let mut text = String::from("__lua__\n\n__sfx__\nsfx 0 speed=1\n");
    for _ in 0..33 {
        text.push_str("C4 0 1\n");
    }
    expect_cart_error(&text, 34, "more than 32 rows");
}

#[test]
fn malformed_music_is_a_line_numbered_cart_error() {
    const SFX: &str = "__lua__\n\n__sfx__\nsfx 0 speed=1\nC4 0 1\n\n__music__\n";
    // Fewer than 4 slots and more than 6 are both rejected.
    expect_cart_error(&format!("{SFX}pat 0 : 0 - -\n"), 1, "4-6 channel slots");
    expect_cart_error(&format!("{SFX}pat 0 : 0 - - - - - -\n"), 1, "4-6 channel slots");
    expect_cart_error(&format!("{SFX}pat 0 0 - - -\n"), 1, "expected `pat");
    expect_cart_error(&format!("{SFX}nope 0 : - - - -\n"), 1, "must start with `pat`");
    expect_cart_error(&format!("{SFX}pat 64 : - - - -\n"), 1, "pattern id must be 0-63");
    expect_cart_error(&format!("{SFX}pat 0 wat : - - - -\n"), 1, "unknown pattern flag");
    expect_cart_error(&format!("{SFX}pat 0 : 0 1 - -\n"), 1, "sfx 1, which is not defined");
    expect_cart_error(
        &format!("{SFX}pat 0 loop=5 : 0 - - -\n"),
        1,
        "loop target pattern 5 is not defined",
    );
    expect_cart_error(
        &format!("{SFX}pat 0 : 0 - - -\npat 0 : 0 - - -\n"),
        2,
        "duplicate pattern id 0",
    );
}

#[test]
fn carts_without_audio_sections_are_unchanged() {
    let cart = Cart::parse("__lua__\nx = 1\n").unwrap();
    assert!(cart.audio().is_empty());
    assert!(cart.sfx(0).is_none());
    assert!(cart.pattern(0).is_none());
    assert_eq!(cart.audio().sfx_ids().count(), 0);
}

// ---------------------------------------------------------------------------
// Frame size / silence
// ---------------------------------------------------------------------------

#[test]
fn frame_is_exactly_one_sixtieth_of_a_second() {
    assert_eq!(SAMPLES_PER_FRAME, 735);
    assert_eq!(SAMPLE_RATE as usize, SAMPLES_PER_FRAME * 60);

    let mut con = console(DEMO);
    for _ in 0..10 {
        con.step(0).unwrap();
        assert_eq!(con.audio_frame().len(), SAMPLES_PER_FRAME);
    }
}

#[test]
fn a_fresh_console_is_silent() {
    let con = console(DEMO);
    assert!(con.audio_frame().iter().all(|&s| s == 0.0));
}

#[test]
fn silence_until_sfx_or_music_is_called() {
    // The cart defines audio but never plays it.
    let mut con = console(
        "__lua__\nfunction _draw() cls(1) end\n\n__sfx__\nsfx 0 speed=4\nC4 2 7\n\n__music__\npat 0 : 0 - - -\n",
    );
    let samples = collect(&mut con, 30);
    assert!(samples.iter().all(|&s| s == 0.0), "should be silent");

    // A cart with no audio sections at all is silent too.
    let mut plain = console("__lua__\nfunction _draw() cls(2) end\n");
    assert!(collect(&mut plain, 30).iter().all(|&s| s == 0.0));
}

#[test]
fn a_halted_console_renders_silence() {
    let mut con = console(
        "__lua__
         function _init() sfx(0) end
         function _update() if t() > 0.04 then boom() end end

__sfx__
sfx 0 speed=60 loop=0,0
C4 2 7
",
    );
    // Audible while it runs...
    let mut heard = false;
    for _ in 0..3 {
        con.step(0).unwrap();
        heard |= con.audio_frame().iter().any(|&s| s != 0.0);
    }
    assert!(heard, "sfx should be audible before the halt");

    // ...silent on the frame that halts and on every step afterwards.
    assert!(con.step(0).is_err());
    assert!(con.is_halted());
    assert!(con.audio_frame().iter().all(|&s| s == 0.0));
    assert!(con.step(0).is_err());
    assert!(con.audio_frame().iter().all(|&s| s == 0.0));
}

#[test]
fn samples_stay_inside_the_output_range() {
    let mut con = console(
        "__lua__\nfunction _init() for c = 0, 3 do sfx(0, c) end end\n\n__sfx__\nsfx 0 speed=4 loop=0,1\nC2 4 7\nE2 4 7\n",
    );
    let samples = collect(&mut con, 60);
    assert!(samples.iter().all(|&s| (-1.0..=1.0).contains(&s)));
    assert!(samples.iter().any(|&s| s != 0.0));
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// 240 frames of movement, A-bursts (which fire the blip sfx) and a B reset.
fn script() -> Vec<u8> {
    let mut log = Vec::new();
    let mut push = |count: usize, mask: u8| log.extend(std::iter::repeat_n(mask, count));
    push(10, 0);
    push(15, input::RIGHT);
    push(5, input::RIGHT | input::A);
    push(10, input::RIGHT | input::UP);
    push(1, input::A);
    push(9, 0);
    push(20, input::LEFT | input::DOWN);
    push(2, input::A);
    push(8, input::LEFT);
    push(1, input::B);
    push(14, 0);
    push(5, input::A);
    push(20, input::UP);
    push(30, 0);
    push(1, input::A);
    push(29, 0);
    push(1, input::B);
    push(9, 0);
    push(1, input::A);
    push(49, 0);
    assert_eq!(log.len(), 240);
    log
}

fn run_audio(cart: &str, seed: u64, inputs: &[u8]) -> Vec<f32> {
    let mut con = Console::new(cart, seed).expect("cart loads");
    let mut out = Vec::with_capacity(inputs.len() * SAMPLES_PER_FRAME);
    for &mask in inputs {
        con.step(mask).expect("frame runs");
        out.extend_from_slice(con.audio_frame());
    }
    out
}

#[test]
fn two_fresh_consoles_produce_bit_identical_audio() {
    let inputs = script();
    let a = run_audio(DEMO, 7, &inputs);
    let b = run_audio(DEMO, 7, &inputs);
    assert_eq!(a.len(), 240 * SAMPLES_PER_FRAME);
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        assert_eq!(
            x.to_bits(),
            y.to_bits(),
            "sample {i} (frame {}) diverged: {x} vs {y}",
            i / SAMPLES_PER_FRAME
        );
    }
    // The demo really does make noise, and the blips really do fire.
    assert!(a.iter().any(|&s| s != 0.0));
}

#[test]
fn audio_does_not_depend_on_the_prng_seed() {
    // Nothing in the demo's audio path reads rnd(), so the seed cannot move it.
    let inputs = script();
    assert_eq!(
        hash_samples(&run_audio(DEMO, 0, &inputs)),
        hash_samples(&run_audio(DEMO, 12345, &inputs))
    );
}

/// Golden hash of the demo cart's audio: FNV-1a over the little-endian bits of
/// the first 120 frames' samples (120 * 735 = 88200 f32 values), seed 0, no
/// input.
///
/// This value must also be produced by the wasm build - downstream agents
/// cross-check it against `con_audio()`. If it ever changes, the synth changed.
///
/// It survived the 4 -> 6 channel widening untouched, and that is the point:
/// `demo.cart` is a four-channel cart, `MIX_GAIN` did not move, and the two
/// channels the widening added stay silent unless a cart asks for them. This
/// test passing *is* the back-compat proof — `web/smoke.cjs`'s `AUDIO_GOLDEN`
/// still matches, so the committed wasm engine needs no rebuild.
const DEMO_AUDIO_GOLDEN: u64 = 0xbc2b_d5e1_f8c7_f31e;

#[test]
fn demo_cart_audio_matches_the_golden_hash() {
    let hash = hash_samples(&run_audio(DEMO, 0, &[0u8; 120]));
    assert_eq!(
        hash, DEMO_AUDIO_GOLDEN,
        "demo audio changed; new hash is {hash:#018x}"
    );
}

// ---------------------------------------------------------------------------
// Mix headroom (the 4 -> 6 channel widening)
// ---------------------------------------------------------------------------

/// `MIX_GAIN` stayed at 1/4 when the console went from four channels to six, so
/// every cart written before the widening renders bit-identical samples (see
/// [`DEMO_AUDIO_GOLDEN`] and [`SOUNDTEST_GROOVE_GOLDEN`], both unchanged). The
/// price is that six voices can now ask for more than full scale. These three
/// tests pin exactly where that line sits.
///
/// The probe is the worst case a cart can actually build: `n` channels all
/// playing the *same* triangle note, started on the same frame, so their phases
/// are locked together and the sum is `n * MIX_GAIN` times one triangle. A
/// triangle (rather than a square) makes clipping observable: an unclipped
/// triangle only touches its peak for an instant, while a clipped one sits flat
/// at ±1.0 for a measurable share of every half-cycle.
fn saturated_channels(n: usize, extra_init: &str) -> Vec<f32> {
    let starts: String = (0..n).map(|ch| format!("sfx(0, {ch}) ")).collect();
    let cart = format!(
        "__lua__\nfunction _init() {starts}{extra_init} end\n\n__sfx__\nsfx 0 speed=120\nC2 3 7\n"
    );
    let mut con = console(&cart);
    // Frame 1 is the 64-sample amplitude ramp; measure the steady state.
    collect(&mut con, 1);
    collect(&mut con, 10)
}

/// Fraction of samples pinned at exactly full scale.
fn pinned_fraction(samples: &[f32]) -> f32 {
    let pinned = samples.iter().filter(|s| s.abs() == 1.0).count();
    pinned as f32 / samples.len() as f32
}

#[test]
fn mix_headroom_four_channels_is_unchanged() {
    let samples = saturated_channels(4, "");
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    // Four full-scale voices sum to exactly 1.0 — the ceiling, touched but
    // never exceeded, exactly as in the four-channel console.
    assert!(peak <= 1.0, "peak {peak} exceeded full scale");
    assert!(peak > 0.99, "probe never got loud (peak {peak})");
    // A triangle only grazes its peak, so hardly any sample is pinned.
    let pinned = pinned_fraction(&samples);
    assert!(pinned < 0.01, "{pinned} of samples pinned: this is clipping");
}

#[test]
fn mix_headroom_six_channels_hard_clips_without_drive() {
    let samples = saturated_channels(6, "");
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert_eq!(peak, 1.0, "the final clamp bounds the output");
    // Six aligned full-scale voices want 6 * 0.25 = 1.5, so |sum| > 1 for the
    // outer third of the triangle: about a third of the samples come back
    // flat-topped. DOCUMENTED, not fixed — leaving MIX_GAIN alone is what keeps
    // every pre-existing cart bit-identical. Songs should not run six vol-7
    // voices in phase; `master drive=1` (below) is the cheap insurance.
    let pinned = pinned_fraction(&samples);
    assert!(
        (0.30..0.36).contains(&pinned),
        "expected ~1/3 of samples clipped, got {pinned}"
    );
}

#[test]
fn mix_headroom_six_channels_never_reaches_the_clamp_with_drive() {
    for drive in 1..=MAX_DRIVE {
        let samples = saturated_channels(6, &format!("master({drive})"));
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        // The shaper's output is bounded by MAKEUP[drive] < 1, so the hard
        // clamp is never engaged: any non-zero drive is a free limiter.
        assert!(peak < 1.0, "drive {drive} still hit the clamp (peak {peak})");
        assert_eq!(
            pinned_fraction(&samples),
            0.0,
            "drive {drive} pinned samples at full scale"
        );
        assert!(peak > 0.6, "drive {drive} probe went quiet (peak {peak})");
    }
}

/// The mix is only tight when voices *align*; a realistic six-voice arrangement
/// at sane levels has plenty of room.
#[test]
fn mix_headroom_six_moderate_channels_are_clean() {
    let starts: String = (0..CHANNEL_COUNT)
        .map(|ch| format!("sfx({ch}, {ch}) "))
        .collect();
    let mut sfx = String::new();
    for (i, note) in ["C2", "E2", "G2", "C3", "E3", "G3"].iter().enumerate() {
        sfx.push_str(&format!("\nsfx {i} speed=120\n{note} 3 4\n"));
    }
    let mut con = console(&format!(
        "__lua__\nfunction _init() {starts}end\n\n__sfx__{sfx}"
    ));
    collect(&mut con, 1);
    let samples = collect(&mut con, 10);
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    // 6 voices * (4/7) * 0.25 = 0.857 worst case, and they are not in phase.
    assert!(peak < 0.86, "peak {peak}");
    assert_eq!(pinned_fraction(&samples), 0.0);
}

// ---------------------------------------------------------------------------
// Click guard
// ---------------------------------------------------------------------------

#[test]
fn note_changes_are_ramped() {
    // Triangle (continuous within a period) so the only per-sample motion is
    // the waveform slope plus the amplitude ramp.
    let mut con = console(
        "__lua__\nfunction _init() sfx(0, 0) end\n\n__sfx__\nsfx 0 speed=4 loop=0,1\nC4 3 7\nE4 3 7\n",
    );
    let samples = collect(&mut con, 40);

    // amplitude 7/7 * 0.25 mix gain.
    let amp = 0.25f32;
    // Ramp moves the envelope by at most 1/64 of full scale per sample.
    let ramp = amp / 64.0;
    // Triangle traverses 4 units per period, so at E4 (329.63 Hz) one sample
    // moves it by 4 * 329.63 / 44100.
    let wave_step = amp * 4.0 * 329.63 / 44100.0;
    let bound = ramp + wave_step;

    let mut worst = 0.0f32;
    let mut worst_at = 0;
    for (i, w) in samples.windows(2).enumerate() {
        let d = (w[1] - w[0]).abs();
        if d > worst {
            worst = d;
            worst_at = i;
        }
    }
    assert!(
        worst <= bound,
        "click of {worst} at sample {worst_at} exceeds the {bound} guard"
    );
    // The note change really happened (otherwise the bound is vacuous).
    assert!(samples.iter().any(|&s| s.abs() > 0.2));
}

#[test]
fn volume_changes_are_ramped_too() {
    let mut con = console(
        "__lua__\nfunction _init() sfx(0, 0) end\n\n__sfx__\nsfx 0 speed=4 loop=0,3\nC4 2 7\n---\nC4 2 7\n---\n",
    );
    let samples = collect(&mut con, 40);
    // A square wave flips by 2 * amp; the envelope may add amp/64 on top.
    let bound = 0.25f32 * 2.0 + 0.25 / 64.0;
    let worst = samples
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(worst <= bound, "{worst} > {bound}");

    // A square wave has constant |value|, so |sample| *is* the envelope: every
    // rest and re-attack must move it by at most one ramp step.
    let jumps: Vec<(usize, f32)> = samples
        .windows(2)
        .enumerate()
        .filter(|(_, w)| (w[1].abs() - w[0].abs()).abs() > 0.25 / 64.0 + 1e-6)
        .map(|(i, w)| (i, w[1].abs() - w[0].abs()))
        .collect();
    assert!(jumps.is_empty(), "un-ramped envelope steps: {:?}", &jumps[..jumps.len().min(4)]);
    // The envelope really does open and close.
    assert!(samples.iter().any(|&s| s.abs() > 0.2));
    assert!(samples.contains(&0.0));
}

// ---------------------------------------------------------------------------
// Sequencer
// ---------------------------------------------------------------------------

const SEQ: &str = "\
__lua__
function _update() end

__sfx__
sfx 0 speed=1
C4 2 7
E4 2 7
G4 2 7

sfx 1 speed=1 loop=1,2
C3 2 7
E3 2 7
G3 2 7

sfx 2 speed=2
A4 2 7
B4 2 7

__music__
pat 0 : 0 1 - -
pat 1 stop : 2 - - -
";

fn seq_console(extra_init: &str) -> Console {
    let cart = SEQ.replace(
        "function _update() end",
        &format!("function _init() {extra_init} end\nfunction _update() end"),
    );
    Console::new(&cart, 0).expect("seq cart loads")
}

#[test]
fn sfx_plays_its_rows_then_stops() {
    let mut con = seq_console("sfx(0, 0)");
    for want in [0u16, 1, 2] {
        assert_eq!(con.audio_channels()[0].sfx, Some(0));
        assert_eq!(con.audio_channels()[0].row, want);
        con.step(0).unwrap();
    }
    // Three rows at speed 1 = three frames; the channel is free afterwards.
    assert_eq!(con.audio_channels()[0].sfx, None);
    assert!(!con.audio_channels()[0].busy);
}

#[test]
fn sfx_loop_repeats_its_rows() {
    let mut con = seq_console("sfx(1, 0)");
    // loop=1,2 -> rows go 0, 1, 2, 1, 2, 1, 2, ...
    let mut rows = Vec::new();
    for _ in 0..9 {
        rows.push(con.audio_channels()[0].row);
        con.step(0).unwrap();
    }
    assert_eq!(rows, vec![0, 1, 2, 1, 2, 1, 2, 1, 2]);
    assert!(con.audio_channels()[0].busy, "a looped sfx never ends");
}

#[test]
fn music_advances_to_the_next_pattern_then_stops() {
    let mut con = seq_console("music(0)");
    assert_eq!(con.music_pattern(), Some(0));
    // pat 0 duration = max(3*1, 3*1) = 3 frames.
    for _ in 0..3 {
        con.step(0).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(1), "pat 0 -> pat 1");
    assert_eq!(con.audio_channels()[0].sfx, Some(2));
    assert!(!con.audio_channels()[1].busy, "pat 1 releases channel 1");

    // pat 1 is `stop`: 2 rows * speed 2 = 4 frames.
    for _ in 0..4 {
        con.step(0).unwrap();
    }
    assert_eq!(con.music_pattern(), None, "`stop` halts music");
    assert!(con.audio_channels().iter().all(|c| !c.busy));
}

#[test]
fn music_loop_jumps_back() {
    let cart = SEQ.replace("pat 1 stop :", "pat 1 loop=0 :");
    let cart = cart.replace(
        "function _update() end",
        "function _init() music(0) end\nfunction _update() end",
    );
    let mut con = Console::new(&cart, 0).unwrap();
    for _ in 0..3 {
        con.step(0).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(1));
    for _ in 0..4 {
        con.step(0).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(0), "loop=0 jumps back");
}

#[test]
fn music_claims_its_slots_and_marks_them() {
    let con = seq_console("music(0)");
    let ch = con.audio_channels();
    assert!(ch[0].from_music && ch[0].busy);
    assert!(ch[1].from_music && ch[1].busy);
    // A 2-slot pattern leaves the other four channels genuinely free.
    assert!(ch[2..].iter().all(|c| !c.busy), "{ch:?}");
}

/// Slots 4 and 5 sequence like any other channel, and the slots a shorter
/// pattern line omits stay unclaimed — the recommended song shape, since it is
/// what keeps `sfx()` off the melody.
#[test]
fn music_sequences_the_widened_slots() {
    // Six slots: every channel is owned by music and playing.
    let six = SEQ.replace("pat 0 : 0 1 - -", "pat 0 : 0 1 2 0 1 2");
    let mut con = Console::new(
        &six.replace(
            "function _update() end",
            "function _init() music(0) end\nfunction _update() end",
        ),
        0,
    )
    .unwrap();
    let ch = con.audio_channels();
    assert!(ch.iter().all(|c| c.from_music && c.busy), "{ch:?}");
    assert_eq!(
        ch.map(|c| c.sfx),
        [Some(0), Some(1), Some(2), Some(0), Some(1), Some(2)]
    );
    // Channel 5 really sequences: sfx 2 is speed 2, so row 1 lands on frame 2.
    con.step(0).unwrap();
    con.step(0).unwrap();
    assert_eq!(con.audio_channels()[5].row, 1);

    // Five slots: channel 5 is left free for sfx.
    let five = SEQ.replace("pat 0 : 0 1 - -", "pat 0 : 0 1 2 0 1");
    let mut con = Console::new(
        &five.replace(
            "function _update() end",
            "function _init() music(0) end\nfunction _update() end",
        ),
        0,
    )
    .unwrap();
    assert!(con.audio_channels()[..5].iter().all(|c| c.from_music));
    assert!(!con.audio_channels()[5].busy);
    con.eval("sfx(2)").unwrap();
    assert_eq!(
        con.audio_channels()[5].sfx,
        Some(2),
        "the free slot absorbs the blip instead of stealing from the song"
    );
    assert!(con.audio_channels()[..5].iter().all(|c| c.from_music));
}

#[test]
fn sfx_during_music_picks_a_free_channel() {
    let mut con = seq_console("music(0)");
    con.eval("sfx(2)").unwrap();
    let ch = con.audio_channels();
    assert_eq!(ch[2].sfx, Some(2), "should land on the first free channel");
    assert!(!ch[2].from_music);
    // The music channels are untouched.
    assert_eq!(ch[0].sfx, Some(0));
    assert_eq!(ch[1].sfx, Some(1));

    // Auto-alloc keeps walking upward through the non-music channels.
    con.eval("sfx(0)").unwrap();
    assert_eq!(con.audio_channels()[3].sfx, Some(0));
    con.eval("sfx(0)").unwrap();
    assert_eq!(con.audio_channels()[4].sfx, Some(0));
    assert!(!con.audio_channels()[5].busy, "channel 5 is still free");
}

#[test]
fn auto_sfx_steals_the_highest_channel_when_everything_is_busy() {
    let mut con = seq_console("music(0)");
    // pat 0 owns channels 0 and 1; four blips fill 2, 3, 4 and 5.
    con.eval("sfx(2) sfx(2) sfx(2) sfx(2)").unwrap();
    let ch = con.audio_channels();
    assert!(ch.iter().all(|c| c.busy), "{ch:?}");
    assert_eq!(ch[5].sfx, Some(2), "the fourth blip landed on channel 5");

    con.eval("sfx(0)").unwrap();
    let ch = con.audio_channels();
    assert_eq!(ch[5].sfx, Some(0), "channel 5 is the steal target");
    assert_eq!(ch[0].sfx, Some(0), "music channel 0 is left alone");
    assert_eq!(ch[1].sfx, Some(1), "music channel 1 is left alone");
    for (i, c) in ch.iter().enumerate().take(5).skip(2) {
        assert_eq!(c.sfx, Some(2), "channel {i} is left alone");
    }
}

/// The steal target is the highest channel even when *music* owns it: a
/// six-slot song gives up channel 5 to the next auto-allocated `sfx()`. This is
/// why the recommended song shape leaves one or two channels unclaimed.
#[test]
fn auto_sfx_steals_from_music_when_a_song_claims_every_channel() {
    let cart = SEQ.replace("pat 0 : 0 1 - -", "pat 0 : 0 1 0 1 0 1");
    let cart = cart.replace(
        "function _update() end",
        "function _init() music(0) end\nfunction _update() end",
    );
    let mut con = Console::new(&cart, 0).unwrap();
    assert!(con.audio_channels().iter().all(|c| c.from_music));

    con.eval("sfx(2)").unwrap();
    let ch = con.audio_channels();
    assert_eq!(ch[5].sfx, Some(2));
    assert!(!ch[5].from_music, "channel 5 was taken away from music");
    assert!(ch[..5].iter().all(|c| c.from_music), "{ch:?}");
}

#[test]
fn explicit_channel_overrides_music_until_the_next_pattern() {
    let mut con = seq_console("music(0)");
    con.eval("sfx(2, 1)").unwrap();
    let ch = con.audio_channels();
    assert_eq!(ch[1].sfx, Some(2));
    assert!(!ch[1].from_music);
    // Pattern 1 does not use channel 1, so music does not take it back...
    for _ in 0..3 {
        con.step(0).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(1));
    assert!(!con.audio_channels()[1].from_music);
    // ...and once the borrowed sfx runs out the channel is simply free.
    for _ in 0..2 {
        con.step(0).unwrap();
    }
    assert!(!con.audio_channels()[1].busy);
}

#[test]
fn sfx_minus_one_stops_a_channel() {
    let mut con = seq_console("sfx(1, 0) sfx(1, 2)");
    assert!(con.audio_channels()[0].busy);
    con.eval("sfx(-1, 0)").unwrap();
    assert!(!con.audio_channels()[0].busy);
    assert!(con.audio_channels()[2].busy, "only channel 0 stops");

    // sfx(-1) with no channel stops every sfx channel.
    con.eval("sfx(-1)").unwrap();
    assert!(con.audio_channels().iter().all(|c| !c.busy));
}

/// The two channels the widening added are ordinary channels in every respect.
#[test]
fn sfx_minus_one_stops_the_new_top_channels() {
    let mut con = seq_console("sfx(1, 4) sfx(1, 5)");
    assert!(con.audio_channels()[4].busy && con.audio_channels()[5].busy);

    con.eval("sfx(-1, 5)").unwrap();
    assert!(!con.audio_channels()[5].busy);
    assert!(con.audio_channels()[4].busy, "only channel 5 stops");

    con.eval("sfx(-1, 4)").unwrap();
    assert!(con.audio_channels().iter().all(|c| !c.busy));
}

#[test]
fn sfx_minus_one_leaves_music_running_on_other_channels() {
    let mut con = seq_console("music(0)");
    con.eval("sfx(-1, 1)").unwrap();
    assert!(!con.audio_channels()[1].busy);
    assert_eq!(con.music_pattern(), Some(0), "music itself keeps its clock");
    assert!(con.audio_channels()[0].from_music);
}

#[test]
fn music_minus_one_stops_and_releases_channels() {
    let mut con = seq_console("music(0) sfx(2, 3)");
    con.eval("music(-1)").unwrap();
    assert_eq!(con.music_pattern(), None);
    let ch = con.audio_channels();
    assert!(!ch[0].busy && !ch[1].busy);
    assert_eq!(ch[3].sfx, Some(2), "a plain sfx survives music(-1)");
}

#[test]
fn music_falls_silent_after_it_stops() {
    let mut con = seq_console("music(0)");
    let playing = collect(&mut con, 7);
    assert!(playing.iter().any(|&s| s != 0.0));
    assert_eq!(con.music_pattern(), None);
    // Give the ramps a couple of frames to reach zero, then expect silence.
    let tail = collect(&mut con, 10);
    assert!(
        tail[SAMPLES_PER_FRAME..].iter().all(|&s| s == 0.0),
        "music should be fully silent once stopped"
    );
}

#[test]
fn sfx_from_update_is_audible_in_the_same_frame() {
    let mut con = console(
        "__lua__\nfunction _update() if t() == 0 then sfx(0, 0) end end\n\n__sfx__\nsfx 0 speed=8\nC4 2 7\n",
    );
    con.step(0).unwrap();
    assert!(
        con.audio_frame().iter().any(|&s| s != 0.0),
        "the very first frame must already contain the sfx"
    );
}

// ---------------------------------------------------------------------------
// Lua errors
// ---------------------------------------------------------------------------

fn lua_error(cart: &str, code: &str) -> String {
    let mut con = Console::new(cart, 0).unwrap();
    con.eval(code).unwrap_err().to_string()
}

#[test]
fn out_of_range_ids_error_clearly() {
    const NO_AUDIO: &str = "__lua__\nx = 1\n";
    let msg = lua_error(NO_AUDIO, "sfx(3)");
    assert!(msg.contains("no __sfx__ section"), "{msg}");
    let msg = lua_error(NO_AUDIO, "music(0)");
    assert!(msg.contains("no __music__ section"), "{msg}");

    let msg = lua_error(SEQ, "sfx(9)");
    assert!(msg.contains("is not defined") && msg.contains("0, 1, 2"), "{msg}");
    let msg = lua_error(SEQ, "music(9)");
    assert!(msg.contains("is not defined") && msg.contains("0, 1"), "{msg}");

    let msg = lua_error(SEQ, "sfx(64)");
    assert!(msg.contains("out of range"), "{msg}");
    let msg = lua_error(SEQ, "sfx(-2)");
    assert!(msg.contains("out of range"), "{msg}");
    // Channels are 0-5 now; 6 is the first one past the end.
    let msg = lua_error(SEQ, "sfx(0, 6)");
    assert!(msg.contains("channel 6 out of range"), "{msg}");
    assert!(msg.contains("expected 0-5"), "{msg}");
    let msg = lua_error(SEQ, "sfx(0, -2)");
    assert!(msg.contains("channel -2 out of range"), "{msg}");
    // ...and 4/5 are perfectly legal.
    let mut con = Console::new(SEQ, 0).unwrap();
    con.eval("sfx(0, 4) sfx(0, 5)").expect("channels 4 and 5 exist");
    assert_eq!(con.audio_channels()[4].sfx, Some(0));
    assert_eq!(con.audio_channels()[5].sfx, Some(0));
    let msg = lua_error(SEQ, "music(-2)");
    assert!(msg.contains("out of range"), "{msg}");
}

// ---------------------------------------------------------------------------
// PoC v2: `__instruments__`
// ---------------------------------------------------------------------------

const INST_CART: &str = "\
__lua__
x = 1

__instruments__
# every field, in every combination
inst plain wave=2
inst lead wave=1 env=4,6,3
inst wobble wave=0 vib=25,4,10
inst kick wave=3 sweep=-14,5 env=0,6,0
inst everything wave=4 env=1,2,3 vib=100,16,255 sweep=95,255

__sfx__
sfx 0 speed=8
A4 lead 6
C5 plain 5
---
G3 kick 7

__music__
bpm=120 rows_per_beat=4
pat 0 : 0 - - -
";

#[test]
fn instruments_section_round_trips() {
    let cart = Cart::parse(INST_CART).unwrap();
    let names: Vec<&str> = cart.instruments().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["plain", "lead", "wobble", "kick", "everything"],
        "declaration order is preserved"
    );

    let plain = cart.instrument("plain").unwrap();
    assert_eq!(plain.wave, 2);
    assert_eq!((plain.env, plain.vib, plain.sweep), (None, None, None));
    assert!(plain.is_flat());

    let lead = cart.instrument("lead").unwrap();
    assert_eq!(
        lead.env,
        Some(Env {
            attack: 4,
            decay: 6,
            sustain: 3
        })
    );
    assert!(!lead.is_flat());

    let wobble = cart.instrument("wobble").unwrap();
    assert_eq!(
        wobble.vib,
        Some(Vib {
            cents: 25,
            rate: 4,
            delay: 10
        })
    );

    let kick = cart.instrument("kick").unwrap();
    assert_eq!(
        kick.sweep,
        Some(Sweep {
            semis: -14,
            frames: 5
        })
    );

    let all = cart.instrument("everything").unwrap();
    assert_eq!(all.wave, 4);
    assert_eq!(
        all.vib,
        Some(Vib {
            cents: 100,
            rate: 16,
            delay: 255
        })
    );
    assert_eq!(
        all.sweep,
        Some(Sweep {
            semis: 95,
            frames: 255
        })
    );

    assert!(cart.instrument("nope").is_none());
    assert!(!cart.audio().is_empty());
    // A cart with no section at all still has none of this.
    assert!(Cart::parse("__lua__\nx=1\n").unwrap().instruments().is_empty());
}

#[test]
fn sfx_rows_resolve_instrument_names_to_their_waveform() {
    let cart = Cart::parse(INST_CART).unwrap();
    let sfx = cart.sfx(0).unwrap();
    // The row's `wave` is the instrument's, so every PoC v1 consumer of
    // SfxRow keeps working unchanged.
    assert_eq!(sfx.rows[0], SfxRow::Note { note: 57, wave: 1, vol: 6 });
    assert_eq!(sfx.rows[1], SfxRow::Note { note: 60, wave: 2, vol: 5 });
    assert_eq!(sfx.rows[2], SfxRow::Rest);
    assert_eq!(sfx.rows[3], SfxRow::Note { note: 43, wave: 3, vol: 7 });

    // ...and the instrument itself is on the side.
    let bank = cart.audio();
    assert_eq!(
        bank.instrument_at(sfx.row_mod(0).inst.unwrap()).unwrap().name,
        "lead"
    );
    assert_eq!(
        bank.instrument_at(sfx.row_mod(3).inst.unwrap()).unwrap().name,
        "kick"
    );
    assert_eq!(sfx.row_mod(2).inst, None, "rests name no instrument");
    assert_eq!(sfx.row_mod(99), Default::default(), "out of range is default");
    assert_eq!(sfx.mods.len(), sfx.rows.len());
}

#[test]
fn section_order_in_the_file_does_not_matter() {
    // __instruments__ *after* the sfx that use it, and after __music__.
    let text = "\
__lua__
x = 1

__sfx__
sfx 0 speed=auto
A4 horn 6

__music__
bpm=90 rows_per_beat=2
pat 0 : 0 - - -

__instruments__
inst horn wave=4 env=2,3,4
";
    let cart = Cart::parse(text).unwrap();
    assert_eq!(cart.instrument("horn").unwrap().wave, 4);
    assert_eq!(cart.sfx(0).unwrap().rows[0], SfxRow::Note { note: 57, wave: 4, vol: 6 });
    // ...and `speed=auto` still saw the tempo line: 3600/(90*2) = 20.
    assert_eq!(cart.sfx(0).unwrap().speed, 20);
}

#[test]
fn malformed_instruments_are_line_numbered_cart_errors() {
    fn inst_err(body: &str, line: usize, needle: &str) {
        expect_cart_error(&format!("__lua__\nx=1\n\n__instruments__\n{body}"), line, needle);
    }

    inst_err("inst a wave=6\n", 1, "wave must be 0-5");
    inst_err("inst a\n", 1, "missing `wave=<0-5>`");
    inst_err("inst\n", 1, "expected `inst <name>");
    inst_err("inst A wave=1\n", 1, "must match [a-z0-9_]+");
    inst_err("inst a-b wave=1\n", 1, "must match [a-z0-9_]+");
    inst_err("inst 3 wave=1\n", 1, "must not be a bare wave digit");
    inst_err("inst 12 wave=1\n", 1, "must not be a bare wave digit");
    inst_err("inst a wave=1\ninst a wave=2\n", 2, "duplicate instrument name");
    inst_err("wave=1\n", 1, "expected `inst <name>");
    inst_err("inst a wave=1 nope\n", 1, "unexpected \"nope\"");
    inst_err("inst a wave=1 boom=2\n", 1, "unknown inst key");

    // env
    inst_err("inst a wave=1 env=1,2\n", 1, "env must be `env=<attack>,<decay>,<sustain>`");
    inst_err("inst a wave=1 env=1,2,3,4\n", 1, "env must be");
    inst_err("inst a wave=1 env=256,0,0\n", 1, "env attack must be 0-255");
    inst_err("inst a wave=1 env=0,999,0\n", 1, "env decay must be 0-255");
    inst_err("inst a wave=1 env=0,0,8\n", 1, "env sustain must be 0-7");
    inst_err("inst a wave=1 env=x,0,0\n", 1, "env attack must be a number");

    // vib
    inst_err("inst a wave=1 vib=25,4\n", 1, "vib must be `vib=<cents>,<rate>,<delay>`");
    inst_err("inst a wave=1 vib=0,4,0\n", 1, "vib cents must be 1-100");
    inst_err("inst a wave=1 vib=101,4,0\n", 1, "vib cents must be 1-100");
    inst_err("inst a wave=1 vib=25,0,0\n", 1, "vib rate must be 1-16");
    inst_err("inst a wave=1 vib=25,17,0\n", 1, "vib rate must be 1-16");
    inst_err("inst a wave=1 vib=25,4,256\n", 1, "vib delay must be 0-255");

    // sweep
    inst_err("inst a wave=1 sweep=-12\n", 1, "sweep must be `sweep=<semis>,<frames>`");
    inst_err("inst a wave=1 sweep=-12,0\n", 1, "sweep frames must be 1-255");
    inst_err("inst a wave=1 sweep=-12,256\n", 1, "sweep frames must be 1-255");
    inst_err("inst a wave=1 sweep=97,4\n", 1, "sweep semitones must be -96-96");

    // line numbers count from the section start, blank/comment lines included
    inst_err("# note\n\ninst a wave=1\ninst b wave=9\n", 4, "wave must be 0-5");
}

#[test]
fn unknown_instrument_names_are_reported_on_the_row() {
    expect_cart_error(
        "__lua__\nx=1\n\n__instruments__\ninst lead wave=1\n\n__sfx__\nsfx 0 speed=4\nA4 bass 6\n",
        2,
        "unknown instrument \"bass\"",
    );
    // The hint lists what the cart does define.
    let err = Cart::parse(
        "__lua__\nx=1\n\n__instruments__\ninst lead wave=1\n\n__sfx__\nsfx 0 speed=4\nA4 bass 6\n",
    )
    .unwrap_err();
    assert!(err.to_string().contains("lead"), "{err}");

    // With no __instruments__ section at all the message says so.
    expect_cart_error(
        "__lua__\nx=1\n\n__sfx__\nsfx 0 speed=4\nA4 bass 6\n",
        2,
        "no __instruments__ section",
    );
    // A numeric column 2 is still validated as a waveform, as in PoC v1.
    expect_cart_error(
        "__lua__\nx=1\n\n__instruments__\ninst lead wave=1\n\n__sfx__\nsfx 0 speed=4\nA4 9 6\n",
        2,
        "wave must be 0-5",
    );
}

// ---------------------------------------------------------------------------
// PoC v2: the effect column
// ---------------------------------------------------------------------------

fn fx_of(text: &str) -> Fx {
    let cart = Cart::parse(text).unwrap();
    cart.sfx(0).unwrap().row_mod(0).fx.expect("row has an fx")
}

#[test]
fn effect_column_round_trips() {
    const HEAD: &str = "__lua__\nx=1\n\n__instruments__\ninst v wave=1 vib=30,8,4\n\n__sfx__\nsfx 0 speed=8\n";

    assert_eq!(
        fx_of(&format!("{HEAD}A4 1 6 arp3,7\n")),
        Fx::Arp { a: 3, b: 7 }
    );
    assert_eq!(
        fx_of(&format!("{HEAD}A4 1 6 arp0,24\n")),
        Fx::Arp { a: 0, b: 24 }
    );
    assert_eq!(
        fx_of(&format!("{HEAD}A4 1 6 sl+7\n")),
        Fx::Slide { semis: 7 }
    );
    assert_eq!(fx_of(&format!("{HEAD}A4 1 6 sl-12\n")), Fx::Slide { semis: -12 });
    assert_eq!(
        fx_of(&format!("{HEAD}A4 1 6 sl5\n")),
        Fx::Slide { semis: 5 },
        "the + sign is optional"
    );
    assert_eq!(
        fx_of(&format!("{HEAD}A4 1 6 fade-3\n")),
        Fx::Fade { levels: -3 }
    );
    assert_eq!(
        fx_of(&format!("{HEAD}A4 1 6 fade+2\n")),
        Fx::Fade { levels: 2 }
    );
    assert_eq!(
        fx_of(&format!("{HEAD}A4 1 6 vib50,4\n")),
        Fx::Vibrato(Vib {
            cents: 50,
            rate: 4,
            delay: 0
        }),
        "the explicit form has no delay"
    );
    // The bare form copies the row instrument's setting.
    assert_eq!(
        fx_of(&format!("{HEAD}A4 v 6 vib\n")),
        Fx::Vibrato(Vib {
            cents: 30,
            rate: 8,
            delay: 4
        })
    );
    // Case-insensitive, like every other keyword in the format.
    assert_eq!(fx_of(&format!("{HEAD}A4 1 6 ARP3,7\n")), Fx::Arp { a: 3, b: 7 });
    // Absent by default.
    let cart = Cart::parse(&format!("{HEAD}A4 1 6\n---\n")).unwrap();
    assert_eq!(cart.sfx(0).unwrap().row_mod(0).fx, None);
    assert_eq!(cart.sfx(0).unwrap().row_mod(1).fx, None);
}

#[test]
fn malformed_effects_are_line_numbered_cart_errors() {
    const HEAD: &str =
        "__lua__\nx=1\n\n__instruments__\ninst v wave=1 vib=30,8,4\ninst p wave=1\n\n__sfx__\nsfx 0 speed=8\n";
    fn fx_err(row: &str, needle: &str) {
        expect_cart_error(
            &format!("{HEAD}{row}\n"),
            2, // section-relative: line 1 is the `sfx` header
            needle,
        );
    }

    fx_err("A4 1 6 nope", "unknown effect \"nope\"");
    fx_err("A4 1 6 arp3", "arp must be `arp<a>,<b>`");
    fx_err("A4 1 6 arp3,25", "arp second offset must be 0-24");
    fx_err("A4 1 6 arp-1,3", "arp first offset must be 0-24");
    fx_err("A4 1 6 sl25", "slide semitones must be -24-24");
    fx_err("A4 1 6 sl-25", "slide semitones must be -24-24");
    fx_err("A4 1 6 slx", "slide semitones must be a number");
    fx_err("A4 1 6 vib101,4", "vib cents must be 1-100");
    fx_err("A4 1 6 vib50,17", "vib rate must be 1-16");
    fx_err("A4 1 6 vib50", "vib must be `vib` or `vib<cents>,<rate>`");
    fx_err("A4 1 6 fade8", "fade levels must be -7-7");
    fx_err("A4 1 6 fade-8", "fade levels must be -7-7");
    // Bare `vib` needs an instrument that has one.
    fx_err("A4 p 6 vib", "bare `vib` needs the row's instrument \"p\"");
    fx_err("A4 1 6 vib", "bare `vib` needs the row to name an instrument");
    // Only one effect per row.
    fx_err("A4 1 6 sl+2 vib", "expected `NOTE WAVE VOL [FX]`");
}

// ---------------------------------------------------------------------------
// PoC v2: tempo sugar
// ---------------------------------------------------------------------------

#[test]
fn speed_auto_resolves_from_the_bpm_line() {
    fn speed_for(bpm: &str) -> u8 {
        let text = format!(
            "__lua__\nx=1\n\n__sfx__\nsfx 0 speed=auto\nA4 1 6\n\n__music__\n{bpm}\npat 0 : 0 - - -\n"
        );
        Cart::parse(&text).unwrap().sfx(0).unwrap().speed
    }
    // round(3600 / (bpm * rows_per_beat)); rows_per_beat defaults to 4.
    assert_eq!(speed_for("bpm=120"), 8); // 7.5 -> 8
    assert_eq!(speed_for("bpm=120 rows_per_beat=4"), 8);
    assert_eq!(speed_for("bpm=125"), 7); // 7.2 -> 7
    assert_eq!(speed_for("bpm=112"), 8); // 8.035 -> 8
    assert_eq!(speed_for("bpm=90 rows_per_beat=2"), 20); // exactly 20
    assert_eq!(speed_for("bpm=150 rows_per_beat=4"), 6); // exactly 6
    assert_eq!(speed_for("bpm=100 rows_per_beat=8"), 5); // 4.5 -> 5 (half up)
    assert_eq!(speed_for("bpm=60 rows_per_beat=1"), 60); // one row per beat
    assert_eq!(speed_for("BPM=120"), 8, "case-insensitive");

    let cart = Cart::parse(INST_CART).unwrap();
    let tempo = cart.audio().tempo().unwrap();
    assert_eq!((tempo.bpm, tempo.rows_per_beat, tempo.speed), (120, 4, 8));
    // No bpm line -> no tempo.
    assert!(Cart::parse(PARSE_CART).unwrap().audio().tempo().is_none());
    // Numeric speeds are untouched by the presence of a tempo line.
    assert_eq!(cart.sfx(0).unwrap().speed, 8);
}

#[test]
fn speed_auto_without_a_bpm_line_is_an_error() {
    expect_cart_error(
        "__lua__\nx=1\n\n__sfx__\nsfx 0 speed=auto\nA4 1 6\n",
        1,
        "speed=auto needs a `bpm=",
    );
    // A __music__ section that does not open with bpm= does not count.
    expect_cart_error(
        "__lua__\nx=1\n\n__sfx__\nsfx 0 speed=auto\nA4 1 6\n\n__music__\npat 0 : 0 - - -\n",
        1,
        "speed=auto needs a `bpm=",
    );
}

#[test]
fn malformed_tempo_lines_are_line_numbered_cart_errors() {
    const SFX: &str = "__lua__\nx=1\n\n__sfx__\nsfx 0 speed=4\nA4 1 6\n\n__music__\n";
    expect_cart_error(&format!("{SFX}bpm=0\npat 0 : 0 - - -\n"), 1, "bpm must be 1-1000");
    expect_cart_error(&format!("{SFX}bpm=1001\npat 0 : 0 - - -\n"), 1, "bpm must be 1-1000");
    expect_cart_error(&format!("{SFX}bpm=x\npat 0 : 0 - - -\n"), 1, "bpm must be a number");
    expect_cart_error(
        &format!("{SFX}bpm=120 rows_per_beat=0\npat 0 : 0 - - -\n"),
        1,
        "rows_per_beat must be 1-16",
    );
    expect_cart_error(
        &format!("{SFX}bpm=120 rows_per_beat=17\npat 0 : 0 - - -\n"),
        1,
        "rows_per_beat must be 1-16",
    );
    expect_cart_error(
        &format!("{SFX}bpm=120 wat=2\npat 0 : 0 - - -\n"),
        1,
        "unknown tempo key",
    );
    expect_cart_error(&format!("{SFX}bpm=120 wat\npat 0 : 0 - - -\n"), 1, "unexpected \"wat\"");
    // 3600/(1000*16) rounds to 0, which is not a legal speed.
    expect_cart_error(
        &format!("{SFX}bpm=1000 rows_per_beat=16\npat 0 : 0 - - -\n"),
        1,
        "gives speed=0",
    );
    // ...and 3600/(1*1) = 3600 is past 255.
    expect_cart_error(&format!("{SFX}bpm=1\npat 0 : 0 - - -\n"), 1, "gives speed=900");
    // The tempo line may only be the first line.
    expect_cart_error(
        &format!("{SFX}pat 0 : 0 - - -\nbpm=120\n"),
        2,
        "must be the first line of __music__",
    );
}

// ---------------------------------------------------------------------------
// PoC v2: backward compatibility
// ---------------------------------------------------------------------------

/// A flat instrument, a cart with unused instruments and the bare-digit cart
/// must all render the same samples: the new paths are strictly additive.
#[test]
fn flat_instruments_are_bit_identical_to_bare_wave_digits() {
    const PLAY: &str = "__lua__\nfunction _init() sfx(0, 0) end\n";
    const ROWS: &str = "\nA4 {} 6\nC5 {} 5\n---\nE5 {} 7\n";

    let bare = format!(
        "{PLAY}\n__sfx__\nsfx 0 speed=6 loop=0,3{}",
        ROWS.replace("{}", "2")
    );
    let named = format!(
        "{PLAY}\n__instruments__\ninst horn wave=2\n\n__sfx__\nsfx 0 speed=6 loop=0,3{}",
        ROWS.replace("{}", "horn")
    );
    let unused = format!(
        "{PLAY}\n__instruments__\ninst horn wave=2 env=4,4,4 vib=50,4,0 sweep=-5,5\n\n__sfx__\nsfx 0 speed=6 loop=0,3{}",
        ROWS.replace("{}", "2")
    );

    let a = hash_samples(&run_audio(&bare, 0, &[0u8; 60]));
    let b = hash_samples(&run_audio(&named, 0, &[0u8; 60]));
    let c = hash_samples(&run_audio(&unused, 0, &[0u8; 60]));
    assert_eq!(a, b, "a flat instrument must not change a single sample");
    assert_eq!(a, c, "an unused instrument must not change a single sample");
}

// ---------------------------------------------------------------------------
// PoC v2: runtime trajectories
// ---------------------------------------------------------------------------

/// Volume level reported for channel 0 on each of the next `frames` frames.
fn vol_trajectory(con: &mut Console, frames: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames {
        out.push(con.audio_channels()[0].vol);
        con.step(0).unwrap();
    }
    out
}

/// Peak |sample| in the tail of each frame, where the 64-sample slew limiter
/// has certainly settled on that frame's target level.
fn frame_tail_peaks(samples: &[f32], frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|f| {
            let base = f * SAMPLES_PER_FRAME;
            samples[base + 300..base + SAMPLES_PER_FRAME]
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()))
        })
        .collect()
}

#[test]
fn envelopes_shape_the_volume_frame_by_frame() {
    let cart = "\
__lua__
function _init() sfx(0, 0) end

__instruments__
inst horn wave=2 env=4,4,3

__sfx__
sfx 0 speed=32
A4 horn 7
";
    let mut con = console(cart);
    // attack 4 (0 -> 7 by frame 3), decay 4 (7 -> 3 by frame 7), then sustain.
    assert_eq!(
        vol_trajectory(&mut con, 12),
        vec![2, 4, 5, 7, 6, 5, 4, 3, 3, 3, 3, 3]
    );

    // The same shape is audible: a square wave's |sample| *is* the envelope.
    let mut con = console(cart);
    let samples = collect(&mut con, 12);
    let peaks = frame_tail_peaks(&samples, 12);
    let want: Vec<f32> = [2, 4, 5, 7, 6, 5, 4, 3, 3, 3, 3, 3]
        .iter()
        .map(|&v| v as f32 / 7.0 * 0.25)
        .collect();
    for (i, (got, want)) in peaks.iter().zip(&want).enumerate() {
        assert!(
            (got - want).abs() < 1e-6,
            "frame {i}: heard {got}, wanted {want}"
        );
    }
}

#[test]
fn fade_ramps_the_volume_to_its_endpoint_within_the_row() {
    let cart = "\
__lua__
function _init() sfx(0, 0) end

__sfx__
sfx 0 speed=8
A4 2 7 fade-7
A4 2 1 fade+4
";
    let mut con = console(cart);
    let vols = vol_trajectory(&mut con, 16);
    assert_eq!(&vols[..8], &[7, 6, 5, 4, 3, 2, 1, 0], "fade-7 reaches silence");
    assert_eq!(&vols[8..], &[1, 2, 2, 3, 3, 4, 4, 5], "fade+4 reaches 1+4");
}

#[test]
fn a_percussion_instrument_sweeps_its_pitch_down_and_decays() {
    let cart = "\
__lua__
function _init() sfx(0, 0) end

__instruments__
inst kick wave=3 sweep=-12,5 env=0,6,0

__sfx__
sfx 0 speed=30
C3 kick 7
";
    let mut con = console(cart);
    assert_eq!(
        vol_trajectory(&mut con, 8),
        vec![6, 5, 3, 2, 1, 0, 0, 0],
        "env=0,6,0 decays from 7 to silence in six frames"
    );

    // The pitch really falls an octave over five frames.
    let mut con = console(cart);
    let samples = collect(&mut con, 6);
    let f0 = frame_freq(&samples, 0);
    let f5 = frame_freq(&samples, 4);
    assert!((f0 - 130.8).abs() < 6.0, "frame 0 should be C3, got {f0}");
    assert!((f5 - 69.3).abs() < 6.0, "frame 4 should be near C2, got {f5}");
}

/// Estimate the fundamental of one rendered frame from its rising zero
/// crossings. Only meaningful for the deterministic non-noise waveforms.
fn frame_freq(samples: &[f32], frame: usize) -> f32 {
    let f = &samples[frame * SAMPLES_PER_FRAME..(frame + 1) * SAMPLES_PER_FRAME];
    let mut edges = Vec::new();
    for i in 1..f.len() {
        if f[i - 1] < 0.0 && f[i] >= 0.0 {
            edges.push(i);
        }
    }
    assert!(edges.len() >= 2, "frame {frame} has no periodic content");
    let span = (edges[edges.len() - 1] - edges[0]) as f32;
    SAMPLE_RATE as f32 * (edges.len() - 1) as f32 / span
}

#[test]
fn a_slide_glides_linearly_across_its_row() {
    // One 60-frame row, sliding a whole octave up from A4.
    let cart = "\
__lua__
function _init() sfx(0, 0) end

__sfx__
sfx 0 speed=60
A4 2 7 sl+12
";
    let mut con = console(cart);
    let samples = collect(&mut con, 60);

    let start = frame_freq(&samples, 0);
    assert!((start - 440.0).abs() < 4.0, "starts at A4, got {start}");

    // Halfway through the row the offset is exactly +6 semitones, which the
    // note table resolves to D#5.
    let mid = frame_freq(&samples, 30);
    let want = freq_at(57, 6.0);
    assert!(
        (mid - want).abs() / want < 0.02,
        "midpoint should be {want} Hz (D#5), got {mid}"
    );
    // Quarter and three-quarter points are +3 and +9 semitones.
    for (frame, semis) in [(15u32, 3.0f32), (45.0 as usize as u32, 9.0)] {
        let got = frame_freq(&samples, frame as usize);
        let want = freq_at(57, semis);
        assert!(
            (got - want).abs() / want < 0.02,
            "frame {frame} should be {want} Hz, got {got}"
        );
    }
    // The last frame is one step short of the full octave: the offset lands
    // on the *boundary*, so a slide glides into whatever plays next.
    let last = frame_freq(&samples, 59);
    let full = freq_at(57, 12.0);
    assert!(last < full && last > freq_at(57, 11.0) * 0.99, "{last} vs {full}");
}

#[test]
fn an_arpeggio_switches_pitch_every_two_frames() {
    let cart = "\
__lua__
function _init() sfx(0, 0) end

__sfx__
sfx 0 speed=12
A4 2 7 arp4,7
";
    let mut con = console(cart);
    let samples = collect(&mut con, 12);
    // 0, 0, +4, +4, +7, +7, then back round.
    let want = [0.0f32, 0.0, 4.0, 4.0, 7.0, 7.0, 0.0, 0.0, 4.0, 4.0, 7.0, 7.0];
    for (frame, semis) in want.iter().enumerate() {
        let got = frame_freq(&samples, frame);
        let want = freq_at(57, *semis);
        assert!(
            (got - want).abs() / want < 0.03,
            "frame {frame}: heard {got}, wanted {want} (+{semis} semitones)"
        );
    }
}

#[test]
fn vibrato_bends_the_pitch_around_the_note() {
    // rate 4 -> a 16-frame LFO cycle: peak at frame 4, trough at frame 12.
    let cart = "\
__lua__
function _init() sfx(0, 0) end

__instruments__
inst wob wave=2 vib=100,4,0

__sfx__
sfx 0 speed=40
A4 wob 7
";
    let mut con = console(cart);
    let samples = collect(&mut con, 20);
    let flat = frame_freq(&samples, 0);
    let up = frame_freq(&samples, 4);
    let down = frame_freq(&samples, 12);
    assert!((flat - 440.0).abs() < 4.0, "{flat}");
    assert!(up > flat + 15.0, "peak {up} should be ~+100 cents of {flat}");
    assert!(down < flat - 15.0, "trough {down} should be ~-100 cents");
    // Delay holds the pitch still first.
    let delayed = "\
__lua__
function _init() sfx(0, 0) end

__instruments__
inst wob wave=2 vib=100,4,20

__sfx__
sfx 0 speed=40
A4 wob 7
";
    let mut con = console(delayed);
    let samples = collect(&mut con, 26);
    assert!((frame_freq(&samples, 4) - 440.0).abs() < 4.0, "still delayed");
    assert!(frame_freq(&samples, 24) > 455.0, "vibrato has started");
}

#[test]
fn effect_state_resets_at_the_next_note_row() {
    // A slide row followed by a plain row: the second row starts at its own
    // pitch, not where the slide left off.
    let cart = "\
__lua__
function _init() sfx(0, 0) end

__sfx__
sfx 0 speed=30
A4 2 7 sl+12
A4 2 7
";
    let mut con = console(cart);
    let samples = collect(&mut con, 60);
    let end_of_slide = frame_freq(&samples, 29);
    assert!(end_of_slide > 800.0, "the slide climbed: {end_of_slide}");
    let after = frame_freq(&samples, 31);
    assert!((after - 440.0).abs() < 5.0, "row 2 is back at A4, got {after}");
}

// ---------------------------------------------------------------------------
// Master bus: grammar
// ---------------------------------------------------------------------------

fn inst_cart(instruments: &str) -> Result<Cart, Error> {
    Cart::parse(&format!("__lua__\nx = 1\n\n__instruments__\n{instruments}\n"))
}

fn inst_err(instruments: &str) -> String {
    inst_cart(instruments).unwrap_err().to_string()
}

#[test]
fn master_line_parses_every_field() {
    let cart = inst_cart("inst lead wave=1\nmaster drive=5 tone=3 hiss=2").unwrap();
    assert_eq!(
        cart.master(),
        Master {
            drive: 5,
            tone: 3,
            hiss: 2
        }
    );
    // `Cart::master()` and `AudioBank::master()` agree.
    assert_eq!(cart.master(), cart.audio().master());
    // The instruments around it are untouched.
    assert_eq!(cart.instruments().len(), 1);
    assert_eq!(cart.instrument("lead").unwrap().wave, 1);
}

#[test]
fn master_fields_are_individually_optional() {
    assert_eq!(
        inst_cart("master drive=8").unwrap().master(),
        Master {
            drive: 8,
            tone: 0,
            hiss: 0
        }
    );
    assert_eq!(
        inst_cart("master tone=4").unwrap().master(),
        Master {
            drive: 0,
            tone: 4,
            hiss: 0
        }
    );
    assert_eq!(
        inst_cart("master hiss=1").unwrap().master(),
        Master {
            drive: 0,
            tone: 0,
            hiss: 1
        }
    );
    assert_eq!(
        inst_cart("master hiss=3 drive=2").unwrap().master(),
        Master {
            drive: 2,
            tone: 0,
            hiss: 3
        }
    );
    // `master drive=0` is legal and means exactly "bypassed".
    assert_eq!(inst_cart("master drive=0").unwrap().master(), Master::OFF);
    assert!(inst_cart("master drive=0").unwrap().master().is_bypass());
    // Keyword and keys are case-insensitive, like the rest of the format.
    assert_eq!(inst_cart("MASTER DRIVE=1 Tone=2").unwrap().master().tone, 2);
}

#[test]
fn a_cart_without_a_master_line_is_all_zero() {
    assert_eq!(Cart::parse(PARSE_CART).unwrap().master(), Master::default());
    assert_eq!(Master::default(), Master::OFF);
    assert_eq!(Cart::parse(DEMO).unwrap().master(), Master::OFF);
    // The soundtest cart deliberately declares none: entry 14 drives the bus
    // from Lua instead, so every other entry keeps the legacy output path.
    assert_eq!(Cart::parse(SOUNDTEST).unwrap().master(), Master::OFF);
}

#[test]
fn master_line_errors_are_line_numbered() {
    let e = inst_err("inst a wave=1\nmaster drive=1\nmaster tone=1");
    assert!(e.contains("__instruments__ line 3"), "{e}");
    assert!(e.contains("at most one"), "{e}");

    let e = inst_err("inst a wave=1\nmaster");
    assert!(e.contains("__instruments__ line 2"), "{e}");
    assert!(e.contains("at least one"), "{e}");

    let e = inst_err("master drive=9");
    assert!(e.contains("master drive must be 0-8, found 9"), "{e}");
    let e = inst_err("master tone=9");
    assert!(e.contains("master tone must be 0-8, found 9"), "{e}");
    let e = inst_err("master hiss=5");
    assert!(e.contains("master hiss must be 0-4, found 5"), "{e}");
    let e = inst_err("master drive=x");
    assert!(e.contains("master drive must be a number"), "{e}");

    let e = inst_err("\nmaster gain=2");
    assert!(e.contains("__instruments__ line 2"), "{e}");
    assert!(e.contains("unknown master key \"gain\""), "{e}");

    let e = inst_err("master drive");
    assert!(e.contains("unexpected \"drive\" in master line"), "{e}");

    // A line that is neither `inst` nor `master` still says so, and now
    // mentions both.
    let e = inst_err("mastr drive=1");
    assert!(e.contains("expected `inst"), "{e}");
    assert!(e.contains("`master drive="), "{e}");
}

// ---------------------------------------------------------------------------
// Master bus: the signal path
// ---------------------------------------------------------------------------

/// The soft clipper, respelled from the documented formula so these tests pin
/// the *curve* rather than whatever the implementation happens to compute.
fn shaper(x: f32) -> f32 {
    if x >= 3.0 {
        return 1.0;
    }
    if x <= -3.0 {
        return -1.0;
    }
    let x2 = x * x;
    (x * (27.0 + x2) / (27.0 + 9.0 * x2)).clamp(-1.0, 1.0)
}

/// `1 + drive * 0.35`. Index 0 is the unused bypass slot.
const PRE_GAIN: [f32; 9] = [1.0, 1.35, 1.7, 2.05, 2.4, 2.75, 3.1, 3.45, 3.8];

/// The documented makeup rule: `REF / R(pre * REF)` at the reference level.
fn makeup(drive: u8) -> f32 {
    if drive == 0 {
        return 1.0;
    }
    let x = f64::from(PRE_GAIN[usize::from(drive)]) * MASTER_REF_LEVEL;
    let r = if x >= 3.0 {
        1.0
    } else {
        x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
    };
    (MASTER_REF_LEVEL / r) as f32
}

/// The whole drive stage: pre-gain, soft clip, makeup.
fn drive_stage(v: f32, drive: u8) -> f32 {
    if drive == 0 {
        return v;
    }
    shaper(v * PRE_GAIN[usize::from(drive)]) * makeup(drive)
}

/// A cart that holds one square-wave note for a long time, optionally with a
/// master line.
fn tone_cart(master: &str) -> String {
    format!(
        "__lua__
function _init() sfx(0, 0) end

__instruments__
{master}

__sfx__
sfx 0 speed=200
A4 2 7
"
    )
}

#[test]
fn drive_is_the_documented_stage_applied_to_the_channel_sum() {
    // Same cart, same channels; the only difference is the master line. Every
    // sample of the driven render must be the dry sample pushed through the
    // documented pre-gain / shaper / makeup - which pins both the formula and
    // the insertion point (after `sum * 0.25`, instead of the plain clamp).
    for drive in 1..=MAX_DRIVE {
        let dry = run_audio(&tone_cart(""), 0, &[0u8; 12]);
        let wet = run_audio(&tone_cart(&format!("master drive={drive}")), 0, &[0u8; 12]);
        assert_eq!(dry.len(), wet.len());
        let mut differed = 0;
        for (i, (&d, &w)) in dry.iter().zip(&wet).enumerate() {
            assert_eq!(
                w.to_bits(),
                drive_stage(d, drive).to_bits(),
                "drive {drive}, sample {i}: {w} is not shape({d} * {}) * {}",
                PRE_GAIN[usize::from(drive)],
                makeup(drive)
            );
            differed += u32::from(w != d);
        }
        assert!(differed > 1000, "drive {drive} barely changed anything");
    }
}

#[test]
fn drive_zero_is_the_bit_identical_legacy_path() {
    let plain = run_audio(&tone_cart(""), 0, &[0u8; 20]);
    // An explicit all-zero master line, and a Lua `master(0)`, must both land
    // on exactly the same samples as having no master at all.
    let explicit = run_audio(&tone_cart("master drive=0"), 0, &[0u8; 20]);
    let lua_cart = tone_cart("").replace(
        "function _init()",
        "function _update() master(0) end\nfunction _init()",
    );
    let via_lua = run_audio(&lua_cart, 0, &[0u8; 20]);
    for (i, ((a, b), c)) in plain.iter().zip(&explicit).zip(&via_lua).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "sample {i}: master drive=0 changed the mix");
        assert_eq!(a.to_bits(), c.to_bits(), "sample {i}: master(0) changed the mix");
    }
    assert!(plain.iter().any(|&s| s != 0.0));
}

#[test]
fn silence_in_is_silence_out_at_every_drive_and_tone() {
    // Nothing playing: whatever the shaper and the filter do, they must do it
    // to zero and produce zero. (hiss = 0; hiss is the one stage that is
    // *supposed* to make sound out of nothing.)
    for drive in 0..=MAX_DRIVE {
        for tone in 0..=MAX_TONE {
            let cart = format!(
                "__lua__\nx = 1\n\n__instruments__\nmaster drive={drive} tone={tone} hiss=0\n"
            );
            let samples = run_audio(&cart, 0, &[0u8; 8]);
            assert!(
                samples.iter().all(|&s| s == 0.0),
                "drive {drive} tone {tone} made noise out of silence"
            );
        }
    }
    // And a playing cart that is *ramped down* to silence still lands on
    // exact zeros rather than a denormal tail.
    let cart = "__lua__
function _init() sfx(0, 0) end
function _update() if t() * 60 >= 2 then sfx(-1, 0) end end

__instruments__
master drive=6 tone=8

__sfx__
sfx 0 speed=200
A4 2 7
";
    let samples = run_audio(cart, 0, &[0u8; 90]);
    let tail = &samples[60 * SAMPLES_PER_FRAME..];
    assert!(
        tail.iter().all(|&s| s == 0.0),
        "the tone filter never settled to exact zero"
    );
}

/// Mean square of the sample-to-sample difference: a crude but honest
/// high-frequency energy meter.
fn delta_rms(samples: &[f32]) -> f64 {
    let mut acc = 0.0f64;
    for w in samples.windows(2) {
        let d = f64::from(w[1] - w[0]);
        acc += d * d;
    }
    (acc / (samples.len() - 1) as f64).sqrt()
}

#[test]
fn tone_darkens_a_square_wave_monotonically() {
    // A square wave is nothing but high-frequency edges, so the roughness of
    // the rendered signal has to fall as `tone` rises.
    let mut prev = f64::INFINITY;
    let mut measures = Vec::new();
    for tone in 0..=MAX_TONE {
        let samples = run_audio(&tone_cart(&format!("master drive=0 tone={tone}")), 0, &[0u8; 12]);
        let hf = delta_rms(&samples);
        assert!(
            hf < prev,
            "tone {tone} ({hf}) is not darker than tone {} ({prev})",
            tone - 1
        );
        prev = hf;
        measures.push(hf);
    }
    // The darkest setting has to be a real change, not a rounding wobble: a
    // one-pole at 3 kHz takes about half the edge energy out of a 440 Hz
    // square (the filter's own decay tails put some of it back).
    assert!(
        measures[usize::from(MAX_TONE)] < measures[0] * 0.6,
        "tone {MAX_TONE} only removed {:.1}% of the edge energy",
        100.0 * (1.0 - measures[usize::from(MAX_TONE)] / measures[0])
    );
    // Tone alone must not clip or blow up the level.
    let dark = run_audio(&tone_cart("master tone=8"), 0, &[0u8; 12]);
    assert!(dark.iter().all(|s| (-1.0..=1.0).contains(s)));
    assert!(dark.iter().any(|&s| s != 0.0));
}

#[test]
fn hiss_is_a_tiny_deterministic_noise_floor() {
    // With nothing playing the output *is* the hiss: a two-level signal at
    // exactly `hiss / 2048`.
    for hiss in 0..=MAX_HISS {
        let cart = format!("__lua__\nx = 1\n\n__instruments__\nmaster hiss={hiss}\n");
        let samples = run_audio(&cart, 0, &[0u8; 4]);
        let level = f32::from(hiss) / 2048.0;
        if hiss == 0 {
            assert!(samples.iter().all(|&s| s == 0.0), "hiss=0 must be silent");
            continue;
        }
        assert!(
            samples.iter().all(|&s| s.abs() == level),
            "hiss {hiss} is not exactly +-{level}"
        );
        assert!(samples.iter().any(|&s| s > 0.0) && samples.iter().any(|&s| s < 0.0));
        // Loud enough to hear on a quiet passage, quiet enough to be a floor.
        assert!(level > 0.0 && level <= 4.0 / 2048.0);
        // Bit-identical between two fresh consoles.
        let again = run_audio(&cart, 999, &[0u8; 4]);
        for (i, (a, b)) in samples.iter().zip(&again).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "hiss sample {i} diverged");
        }
    }
}

#[test]
fn the_hiss_stream_does_not_depend_on_the_channels() {
    // The dedicated LFSR is clocked once per rendered sample no matter what
    // the voices are doing, so subtracting a dry render from a hissing one
    // must reproduce the hiss-only stream exactly.
    let frames = [0u8; 10];
    let dry = run_audio(&tone_cart(""), 0, &frames);
    let hissing = run_audio(&tone_cart("master hiss=3"), 0, &frames);
    let floor = run_audio("__lua__\nx = 1\n\n__instruments__\nmaster hiss=3\n", 0, &frames);
    for (i, ((&d, &h), &f)) in dry.iter().zip(&hissing).zip(&floor).enumerate() {
        assert_eq!(
            (h - d).to_bits(),
            f.to_bits(),
            "sample {i}: the hiss stream drifted with the channels"
        );
    }
}

#[test]
fn lua_master_validates_its_ranges() {
    let mut con = console("__lua__\nx = 1\n");
    assert_eq!(con.master(), Master::OFF);

    con.eval("master(4)").unwrap();
    assert_eq!(
        con.master(),
        Master {
            drive: 4,
            tone: 0,
            hiss: 0
        },
        "omitted arguments default to 0"
    );
    con.eval("master(4, 2)").unwrap();
    assert_eq!(con.master().tone, 2);
    con.eval("master(8, 8, 4)").unwrap();
    assert_eq!(
        con.master(),
        Master {
            drive: 8,
            tone: 8,
            hiss: 4
        }
    );
    con.eval("master(0)").unwrap();
    assert_eq!(con.master(), Master::OFF, "master(0) is a full reset");

    for (code, want) in [
        ("master(9)", "drive 9 out of range (expected 0-8)"),
        ("master(-1)", "drive -1 out of range (expected 0-8)"),
        ("master(0, 9)", "tone 9 out of range (expected 0-8)"),
        ("master(0, -3)", "tone -3 out of range (expected 0-8)"),
        ("master(0, 0, 5)", "hiss 5 out of range (expected 0-4)"),
    ] {
        let err = con.eval(code).unwrap_err().to_string();
        assert!(err.contains(want), "{code}: {err}");
        assert_eq!(con.master(), Master::OFF, "{code} must not have applied");
    }
}

#[test]
fn the_master_bus_is_deterministic_across_consoles() {
    // Every stage engaged at once, including the hiss LFSR and the filter
    // memory, over a cart that is actually playing.
    let cart = tone_cart("master drive=6 tone=4 hiss=2");
    let inputs = [0u8; 60];
    let a = run_audio(&cart, 0, &inputs);
    let b = run_audio(&cart, 0, &inputs);
    let c = run_audio(&cart, 4_242_424_242, &inputs);
    for (i, ((x, y), z)) in a.iter().zip(&b).zip(&c).enumerate() {
        assert_eq!(x.to_bits(), y.to_bits(), "sample {i} diverged between consoles");
        assert_eq!(x.to_bits(), z.to_bits(), "sample {i} depends on the seed");
    }
    assert!(a.iter().any(|&s| s != 0.0));
    assert!(a.iter().all(|s| (-1.0..=1.0).contains(s)));
}

// ---------------------------------------------------------------------------
// Sidechain ducking
// ---------------------------------------------------------------------------

#[test]
fn duck_field_parses_and_is_range_checked() {
    let cart = inst_cart("inst kick wave=3 sweep=-18,4 env=0,8,0 duck=4,10").unwrap();
    let kick = cart.instrument("kick").unwrap();
    assert_eq!(
        kick.duck,
        Some(Duck {
            depth: 4,
            release: 10
        })
    );
    // `duck` is a mixer property, so it does not make the voice "modulated".
    assert_eq!(inst_cart("inst k wave=3 duck=1,1").unwrap().instrument("k").unwrap().duck,
        Some(Duck { depth: 1, release: 1 }));
    assert!(inst_cart("inst k wave=3 duck=1,1").unwrap().instrument("k").unwrap().is_flat());
    // Absent by default.
    assert_eq!(inst_cart("inst k wave=3").unwrap().instrument("k").unwrap().duck, None);
    assert_eq!(
        inst_cart("inst k wave=3 duck=7,255").unwrap().instrument("k").unwrap().duck,
        Some(Duck { depth: MAX_DUCK_DEPTH, release: 255 })
    );

    for (line, want) in [
        ("inst k wave=3 duck=4", "duck must be `duck=<depth>,<release>`"),
        ("inst k wave=3 duck=0,4", "duck depth must be 1-7, found 0"),
        ("inst k wave=3 duck=8,4", "duck depth must be 1-7, found 8"),
        ("inst k wave=3 duck=4,0", "duck release must be 1-255, found 0"),
        ("inst k wave=3 duck=4,256", "duck release must be 1-255, found 256"),
        ("inst k wave=3 duck=x,4", "duck depth must be a number"),
        ("inst k wave=3 ducky=1,2", "unknown inst key \"ducky\""),
    ] {
        let e = inst_err(line);
        assert!(e.contains("__instruments__ line 1"), "{line}: {e}");
        assert!(e.contains(want), "{line}: {e}");
    }
    // The "what keys are there" hints mention duck now.
    assert!(inst_err("inst k wave=3 nope=1").contains("`duck`"));
    assert!(inst_err("inst k wave=3 nope").contains("`duck=`"));
}

/// Channel 1 holds a square wave at full volume; a silent `duck=` trigger
/// fires on channel 0 at the start of frame 2, so every sample is a direct
/// readout of the duck envelope.
///
/// The dry level is exactly `1.0 * 0.25`, so `|sample| = 0.25 * (1 - atten)`.
fn duck_probe_cart(depth: u8, release: u8, master: &str) -> String {
    format!(
        "__lua__
local f = 0
function _init() sfx(1, 1) end
function _update()
  f = f + 1
  if f == 3 then sfx(0, 0) end
end

__instruments__
inst thump wave=2 duck={depth},{release}
{master}

__sfx__
sfx 0 speed=200
C4 thump 0

sfx 1 speed=200
A4 2 7
"
    )
}

/// Attenuation implied by a sample of the probe cart.
fn probe_atten(sample: f32) -> f32 {
    1.0 - sample.abs() / 0.25
}

#[test]
fn a_duck_trigger_dips_the_other_channels_by_exactly_depth_over_seven() {
    let (depth, release) = (4u8, 4u8);
    let samples = run_audio(&duck_probe_cart(depth, release, ""), 0, &[0u8; 12]);
    let trigger = 2 * SAMPLES_PER_FRAME; // frame index 2, where `sfx(0, 0)` fires
    let peak = f32::from(depth) / 7.0;

    // Before the trigger the mix is untouched: a full-scale square at 0.25.
    for (i, &s) in samples[..trigger].iter().enumerate().skip(SAMPLES_PER_FRAME) {
        assert_eq!(s.abs(), 0.25, "sample {i} was ducked before the trigger");
    }
    // The very first sample of the trigger is still dry - the ramp is the
    // anti-click - and the attenuation arrives over the attack window.
    assert_eq!(samples[trigger].abs(), 0.25);
    let attack = DUCK_ATTACK_SAMPLES as usize;
    let mut prev = 0.0f32;
    for k in 1..attack {
        let a = probe_atten(samples[trigger + k]);
        assert!(a >= prev - 1e-6, "the attack ramp went backwards at {k}");
        assert!(a < peak + 1e-6);
        prev = a;
    }
    // ...landing on exactly depth/7 at the end of it.
    let want = (1.0f32 - f32::from(depth) / 7.0) * 0.25;
    assert_eq!(
        samples[trigger + attack].abs(),
        want,
        "attenuation is not exactly {depth}/7 after {attack} samples"
    );

    // Linear recovery: half the attenuation is gone half way through the
    // release, a quarter of it three quarters of the way through.
    let span = usize::from(release) * SAMPLES_PER_FRAME;
    let half = probe_atten(samples[trigger + attack + span / 2]);
    assert!(
        (half - peak / 2.0).abs() < 1e-3,
        "at release/2 the attenuation was {half}, wanted {}",
        peak / 2.0
    );
    let three_quarters = probe_atten(samples[trigger + attack + 3 * span / 4]);
    assert!(
        (three_quarters - peak / 4.0).abs() < 1e-3,
        "at 3*release/4 the attenuation was {three_quarters}"
    );
    // And it is fully recovered afterwards.
    for (i, &s) in samples.iter().enumerate().skip(trigger + attack + span + 8) {
        assert_eq!(s.abs(), 0.25, "sample {i} never recovered");
    }
}

#[test]
fn duck_depth_seven_mutes_the_other_channels_outright() {
    let samples = run_audio(&duck_probe_cart(7, 4, ""), 0, &[0u8; 6]);
    let floor = 2 * SAMPLES_PER_FRAME + DUCK_ATTACK_SAMPLES as usize;
    assert_eq!(samples[floor], 0.0, "depth 7 should silence the other channels");
    // The release starts immediately afterwards, so the next samples are only
    // a hair above zero rather than exactly on it.
    assert!(samples[floor + 1].abs() < 1e-4, "{}", samples[floor + 1]);
    // Two frames later the recovery is well under way again.
    assert!(samples[floor + 2 * SAMPLES_PER_FRAME].abs() > 0.1);
}

#[test]
fn the_trigger_channel_is_never_ducked() {
    // The trigger instrument alone, at full volume, with and without `duck=`:
    // if the envelope touched its own channel the two would differ.
    let cart = |duck: &str| {
        format!(
            "__lua__
function _init() sfx(0, 0) end

__instruments__
inst thump wave=2{duck}

__sfx__
sfx 0 speed=8
C4 thump 7
C4 thump 7
C4 thump 7
C4 thump 7
"
        )
    };
    let plain = run_audio(&cart(""), 0, &[0u8; 30]);
    let ducking = run_audio(&cart(" duck=7,20"), 0, &[0u8; 30]);
    assert!(plain.iter().any(|&s| s != 0.0));
    for (i, (a, b)) in plain.iter().zip(&ducking).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "sample {i}: the trigger channel ducked itself"
        );
    }
}

#[test]
fn a_retrigger_restores_full_depth() {
    // Two triggers a few frames apart: the second dip must be as deep as the
    // first even though the first had not finished releasing.
    let cart = "__lua__
local f = 0
function _init() sfx(1, 1) end
function _update()
  f = f + 1
  if f == 3 or f == 6 then sfx(0, 0) end
end

__instruments__
inst thump wave=2 duck=5,20

__sfx__
sfx 0 speed=200
C4 thump 0

sfx 1 speed=200
A4 2 7
";
    let samples = run_audio(cart, 0, &[0u8; 12]);
    let attack = DUCK_ATTACK_SAMPLES as usize;
    let peak = 5.0f32 / 7.0;
    let first = probe_atten(samples[2 * SAMPLES_PER_FRAME + attack]);
    assert_eq!(first, peak);
    // Mid-release it has partly recovered...
    let between = probe_atten(samples[4 * SAMPLES_PER_FRAME]);
    assert!(between > 0.0 && between < peak, "mid-release: {between}");
    // ...and the re-trigger takes it all the way back down.
    let second = probe_atten(samples[5 * SAMPLES_PER_FRAME + attack]);
    assert_eq!(second, peak, "the re-trigger did not reach full depth");
}

#[test]
fn ducking_happens_before_the_shaper() {
    // Order matters: `shape(duck * x)` compresses the dip back up, while
    // `duck * shape(x)` would not. Rendering the same duck scenario with and
    // without `master drive=8` must relate by the drive stage alone.
    let dry = run_audio(&duck_probe_cart(6, 6, ""), 0, &[0u8; 14]);
    let wet = run_audio(&duck_probe_cart(6, 6, "master drive=8"), 0, &[0u8; 14]);
    for (i, (&d, &w)) in dry.iter().zip(&wet).enumerate() {
        assert_eq!(
            w.to_bits(),
            drive_stage(d, 8).to_bits(),
            "sample {i}: the duck and the shaper are in the wrong order"
        );
    }
    // Sanity: the drive really did squash the dip. Reference against a
    // settled pre-trigger sample, not sample 0 (the voice is still ramping in).
    let reference = 2 * SAMPLES_PER_FRAME - 1;
    let floor = 2 * SAMPLES_PER_FRAME + DUCK_ATTACK_SAMPLES as usize;
    let dip_dry = dry[floor].abs() / dry[reference].abs();
    let dip_wet = wet[floor].abs() / wet[reference].abs();
    assert!(dip_dry < 0.5, "the dry dip should be deep: {dip_dry}");
    assert!(
        dip_wet > dip_dry,
        "drive should shrink the dip: dry {dip_dry}, driven {dip_wet}"
    );
}

#[test]
fn ducking_is_deterministic_across_consoles() {
    let cart = duck_probe_cart(3, 8, "master drive=4 tone=2 hiss=1");
    let inputs = [0u8; 40];
    let a = run_audio(&cart, 0, &inputs);
    let b = run_audio(&cart, 0, &inputs);
    let c = run_audio(&cart, 7, &inputs);
    for (i, ((x, y), z)) in a.iter().zip(&b).zip(&c).enumerate() {
        assert_eq!(x.to_bits(), y.to_bits(), "sample {i} diverged between consoles");
        assert_eq!(x.to_bits(), z.to_bits(), "sample {i} depends on the seed");
    }
    // Two consoles also agree on the envelope state itself.
    let mut p = Console::new(&cart, 0).unwrap();
    let mut q = Console::new(&cart, 123).unwrap();
    for _ in 0..40 {
        p.step(0).unwrap();
        q.step(0).unwrap();
        assert_eq!(p.duck_state().1, q.duck_state().1);
        assert_eq!(p.duck_state().0.to_bits(), q.duck_state().0.to_bits());
    }
}

#[test]
fn a_cart_with_no_duck_instrument_never_leaves_the_legacy_path() {
    // The whole PoC v1/v2 corpus: no `duck=`, no `master`, so `duck_state`
    // stays idle and the mixer keeps taking the untouched statement.
    let mut con = Console::new(DEMO, 0).unwrap();
    for &mask in &script() {
        con.step(mask).unwrap();
        assert_eq!(con.duck_state(), (0.0, None));
        assert_eq!(con.master(), Master::OFF);
    }
}

// ---------------------------------------------------------------------------
// PoC v2: the soundtest cart
// ---------------------------------------------------------------------------

/// Menu entries in `carts/soundtest.cart`.
const SOUNDTEST_ENTRIES: usize = 16;

/// Zero-based menu index of "FULL GROOVE".
const SOUNDTEST_GROOVE: usize = 12;

/// Zero-based menu index of "SATURATION A/B".
const SOUNDTEST_AB: usize = 13;

/// Zero-based menu index of "ECHO  DELAY BUS".
///
/// New entries go on the **end** on purpose: `soundtest_script` navigates by
/// counting DOWN presses, so the two golden entries keep their exact input
/// scripts and their hashes stay comparable across releases.
const SOUNDTEST_ECHO: usize = 14;

/// Zero-based menu index of "WAVETABLE W0-W2" (the last entry), pattern 14.
const SOUNDTEST_WAVETABLE: usize = 15;

/// Frames the A/B entry spends on each side of the comparison: two bars at
/// 112 BPM / 4 rows per beat / speed 8 = 2 * 16 * 8.
const SOUNDTEST_AB_SPAN: usize = 256;

/// Menu navigation: `entry` presses of DOWN (with a release between them, so
/// `btnp` sees each one), then A to play.
fn soundtest_script(entry: usize, play_frames: usize) -> Vec<u8> {
    let mut log = Vec::new();
    for _ in 0..entry {
        log.push(input::DOWN);
        log.push(0);
    }
    log.push(input::A);
    log.extend(std::iter::repeat_n(0u8, play_frames));
    log
}

#[test]
fn the_soundtest_cart_loads_and_describes_itself() {
    let cart = Cart::parse(SOUNDTEST).unwrap();
    assert_eq!(cart.title(), "Sound Test");
    // 15 instruments, 23 sfx, one self-looping pattern per audition entry (the
    // A/B entry re-uses the groove's pattern 12).
    assert_eq!(cart.instruments().len(), 15);
    assert_eq!(cart.audio().sfx_ids().count(), 23);
    let pats: Vec<u8> = cart.audio().pattern_ids().collect();
    assert_eq!(pats, (0..=14).collect::<Vec<u8>>());
    for id in 0..=14u8 {
        assert_eq!(
            cart.pattern(id).unwrap().end,
            PatternEnd::Loop(id),
            "pattern {id} should loop on itself so it can be auditioned"
        );
    }
    // The whole point of the cart: every waveform, and each of the four fx.
    let waves: Vec<u8> = (0..6)
        .map(|w| cart.sfx(w).unwrap().rows[0])
        .map(|r| match r {
            SfxRow::Note { wave, .. } => wave,
            SfxRow::Rest => 99,
        })
        .collect();
    assert_eq!(waves, vec![0, 1, 2, 3, 4, 5]);
    let mut kinds = [false; 4];
    for id in cart.audio().sfx_ids() {
        for m in &cart.sfx(id).unwrap().mods {
            match m.fx {
                Some(Fx::Arp { .. }) => kinds[0] = true,
                Some(Fx::Slide { .. }) => kinds[1] = true,
                Some(Fx::Vibrato(_)) => kinds[2] = true,
                Some(Fx::Fade { .. }) => kinds[3] = true,
                None => {}
            }
        }
    }
    assert_eq!(kinds, [true, true, true, true], "arp/slide/vib/fade all used");
    // Instruments cover envelopes, vibrato and sweeps.
    assert!(cart.instruments().iter().any(|i| i.env.is_some()));
    assert!(cart.instruments().iter().any(|i| i.vib.is_some()));
    assert!(cart.instruments().iter().any(|i| i.sweep.is_some()));
    assert_eq!(cart.instrument("kick").unwrap().sweep.unwrap().semis, -18);
    // The kick is the cart's sidechain trigger.
    assert_eq!(
        cart.instrument("kick").unwrap().duck,
        Some(Duck {
            depth: 3,
            release: 8
        })
    );
    assert_eq!(
        cart.instruments().iter().filter(|i| i.duck.is_some()).count(),
        1,
        "only the kick should duck"
    );
    // Tempo sugar drives most of it.
    let tempo = cart.audio().tempo().unwrap();
    assert_eq!((tempo.bpm, tempo.rows_per_beat, tempo.speed), (112, 4, 8));
    // ...and the A/B entry's flip period really is two bars of it.
    assert_eq!(
        SOUNDTEST_AB_SPAN,
        2 * 4 * usize::from(tempo.rows_per_beat) * usize::from(tempo.speed)
    );
}

#[test]
fn every_soundtest_entry_makes_a_different_noise() {
    let mut seen: Vec<u64> = Vec::new();
    for entry in 0..SOUNDTEST_ENTRIES {
        // The A/B entry spends its first two bars dry, so it needs to run long
        // enough to reach the driven half before it sounds like anything else.
        let frames = if entry == SOUNDTEST_AB { 600 } else { 90 };
        let samples = run_audio(SOUNDTEST, 0, &soundtest_script(entry, frames));
        // Percussive entries are mostly gaps, so the bar is "clearly audible",
        // not "always ringing".
        let audible = samples.iter().filter(|s| **s != 0.0).count();
        assert!(
            audible > 8_000,
            "entry {entry} is nearly silent ({audible} nonzero samples)"
        );
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.1, "entry {entry} is too quiet to judge (peak {peak})");
        assert!(samples.iter().all(|s| (-1.0..=1.0).contains(s)));
        let h = hash_samples(&samples);
        assert!(!seen.contains(&h), "entry {entry} sounds like an earlier one");
        seen.push(h);
    }
}

#[test]
fn the_soundtest_menu_navigates_and_stops() {
    let mut con = Console::new(SOUNDTEST, 0).unwrap();
    // Nothing plays until A.
    for _ in 0..4 {
        con.step(0).unwrap();
    }
    assert_eq!(con.music_pattern(), None);
    assert!(con.audio_frame().iter().all(|&s| s == 0.0));

    // Down twice, then play: entry index 2 is pattern 2.
    for mask in soundtest_script(2, 3) {
        con.step(mask).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(2));
    assert!(con.audio_frame().iter().any(|&s| s != 0.0));

    // Up wraps to the top of the list, i.e. onto the last entry (WAVETABLE).
    for mask in [input::UP, 0, input::UP, 0, input::UP, 0, input::A, 0] {
        con.step(mask).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(14), "UP past the top wraps around");

    // ...and B stops.
    con.step(input::B).unwrap();
    assert_eq!(con.music_pattern(), None);
    let tail = collect(&mut con, 6);
    assert!(
        tail[SAMPLES_PER_FRAME..].iter().all(|&s| s == 0.0),
        "B should silence the console"
    );
}

/// Golden hash of the soundtest cart's "FULL GROOVE" entry (menu index 12,
/// pattern 12): FNV-1a over the little-endian bits of the samples rendered
/// while navigating there and then playing 150 frames, seed 0.
///
/// This is the PoC v2 counterpart of [`DEMO_AUDIO_GOLDEN`]: it pins the
/// envelope, vibrato, sweep, arpeggio, slide and fade paths all at once. If it
/// changes, the new synth vocabulary changed.
///
/// Re-recorded when the groove's `kick` gained `duck=3,8`: sidechain ducking
/// is the only thing that moved it. The master bus does *not* touch this
/// number, because the cart declares no `master` line and this entry sets
/// `master(0)` every frame - see
/// [`soundtest_ab_is_bit_identical_to_the_groove_while_it_is_dry`].
///
/// Unmoved by the 4 -> 6 channel widening: `MIX_GAIN` stayed at 1/4 and the
/// groove is a four-slot pattern, so channels 4 and 5 render silence into the
/// sum. See the `mix_headroom_*` tests for what six loud channels do instead.
const SOUNDTEST_GROOVE_GOLDEN: u64 = 0x993e_e511_8be1_bec4;

#[test]
fn soundtest_groove_matches_the_golden_hash() {
    let hash = hash_samples(&run_audio(
        SOUNDTEST,
        0,
        &soundtest_script(SOUNDTEST_GROOVE, 150),
    ));
    assert_eq!(
        hash, SOUNDTEST_GROOVE_GOLDEN,
        "soundtest groove audio changed; new hash is {hash:#018x}"
    );
}

/// Golden hash of the soundtest cart's "SATURATION A/B" entry (menu index 13):
/// the same groove pattern, with the cart's `_update` flipping the master bus
/// between `master(0)` and `master(4, 2)` every two bars. 600 played frames
/// covers a dry span, a driven span and part of the next dry one, so this pins
/// the shaper, the makeup table, the tone coefficients *and* the Lua setter.
const SOUNDTEST_AB_GOLDEN: u64 = 0xba78_0b63_7bd9_4ac3;

#[test]
fn soundtest_saturation_ab_matches_the_golden_hash() {
    let hash = hash_samples(&run_audio(SOUNDTEST, 0, &soundtest_script(SOUNDTEST_AB, 600)));
    assert_eq!(
        hash, SOUNDTEST_AB_GOLDEN,
        "soundtest A/B audio changed; new hash is {hash:#018x}"
    );
}

/// Samples of one soundtest entry, with the menu-navigation frames dropped so
/// two entries can be compared from the moment A is pressed.
fn soundtest_played(entry: usize, play_frames: usize) -> Vec<f32> {
    let all = run_audio(SOUNDTEST, 0, &soundtest_script(entry, play_frames));
    all[entry * 2 * SAMPLES_PER_FRAME..].to_vec()
}

#[test]
fn soundtest_ab_is_bit_identical_to_the_groove_while_it_is_dry() {
    // The cart has no `master` line, so entry 13's first two bars must be the
    // plain groove down to the last bit - this is the backward-compatibility
    // guarantee, measured rather than asserted.
    // Three full spans: dry, driven, dry again.
    let groove = soundtest_played(SOUNDTEST_GROOVE, 3 * SOUNDTEST_AB_SPAN);
    let ab = soundtest_played(SOUNDTEST_AB, 3 * SOUNDTEST_AB_SPAN);
    let dry = SOUNDTEST_AB_SPAN * SAMPLES_PER_FRAME;
    for i in 0..dry {
        assert_eq!(
            groove[i].to_bits(),
            ab[i].to_bits(),
            "sample {i} (frame {}) of the dry half is not the plain groove",
            i / SAMPLES_PER_FRAME
        );
    }
    // The driven half is a different signal, and a denser one: the shaper
    // trades peak for RMS, so the driven bars have to be louder on average.
    let driven = &ab[dry..dry * 2];
    let same_span = &groove[dry..dry * 2];
    assert!(
        driven
            .iter()
            .zip(same_span)
            .any(|(a, b)| a.to_bits() != b.to_bits()),
        "the driven half did not change anything"
    );
    let rms = |xs: &[f32]| -> f64 {
        (xs.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / xs.len() as f64).sqrt()
    };
    assert!(
        rms(driven) > rms(same_span) * 1.1,
        "driven RMS {} is not meaningfully above dry RMS {}",
        rms(driven),
        rms(same_span)
    );
    assert!(driven.iter().all(|s| (-1.0..=1.0).contains(s)));
    // ...and the third span is dry again, so it really does alternate.
    let third = &ab[dry * 2..dry * 3];
    let groove_third = &groove[dry * 2..dry * 3];
    for (i, (a, b)) in third.iter().zip(groove_third).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "sample {i} of the second dry span is not the plain groove"
        );
    }
}

#[test]
fn soundtest_ab_flips_the_master_bus_on_the_bar() {
    let mut con = Console::new(SOUNDTEST, 0).unwrap();
    for mask in soundtest_script(SOUNDTEST_AB, 0) {
        con.step(mask).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(12), "the A/B entry plays the groove");
    assert_eq!(con.master(), Master::OFF, "it starts dry");

    // Two bars of dry...
    for _ in 0..SOUNDTEST_AB_SPAN - 1 {
        con.step(0).unwrap();
        assert_eq!(con.master(), Master::OFF);
    }
    // ...then two bars of drive.
    for _ in 0..SOUNDTEST_AB_SPAN {
        con.step(0).unwrap();
        assert_eq!(
            con.master(),
            Master {
                drive: 4,
                tone: 2,
                hiss: 0
            }
        );
    }
    // ...and back.
    con.step(0).unwrap();
    assert_eq!(con.master(), Master::OFF);

    // Leaving the entry (B, or picking another one) restores the clean path.
    con.step(input::B).unwrap();
    assert_eq!(con.master(), Master::OFF);
    for mask in [input::UP, 0, input::A, 0, 0] {
        con.step(mask).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(12));
    for _ in 0..SOUNDTEST_AB_SPAN + 4 {
        con.step(0).unwrap();
        assert_eq!(
            con.master(),
            Master::OFF,
            "only the A/B entry may touch the master bus"
        );
    }
}

#[test]
fn the_soundtest_kick_ducks_the_rest_of_the_kit() {
    // Entry 10 is the drum kit: kick on channel 0, snare on 1, hat on 2. The
    // kick's `duck=3,8` has to show up as a live envelope that exempts
    // channel 0 and never anything else.
    let mut con = Console::new(SOUNDTEST, 0).unwrap();
    for mask in soundtest_script(10, 0) {
        con.step(mask).unwrap();
    }
    let mut saw_duck = false;
    let mut deepest = 0.0f32;
    for _ in 0..240 {
        con.step(0).unwrap();
        let (atten, ch) = con.duck_state();
        if let Some(ch) = ch {
            saw_duck = true;
            assert_eq!(ch, 0, "only the kick's channel may be exempt");
            deepest = deepest.max(atten);
        }
        assert!((0.0..=1.0).contains(&atten));
    }
    assert!(saw_duck, "the kick never fired the sidechain");
    // `duck_state` is only sampled on frame boundaries and the release is 8
    // frames long, so the deepest *observed* attenuation is about 7/8 of the
    // 3/7 peak - close to it, and never past it.
    assert!(
        (0.35..=3.0 / 7.0).contains(&deepest),
        "the duck should approach 3/7, reached {deepest}"
    );
}

#[test]
fn the_soundtest_groove_is_deterministic_across_consoles_and_seeds() {
    let script = soundtest_script(SOUNDTEST_GROOVE, 150);
    let a = run_audio(SOUNDTEST, 0, &script);
    let b = run_audio(SOUNDTEST, 0, &script);
    let c = run_audio(SOUNDTEST, 999_999, &script);
    assert_eq!(a.len(), script.len() * SAMPLES_PER_FRAME);
    for (i, ((x, y), z)) in a.iter().zip(&b).zip(&c).enumerate() {
        assert_eq!(
            x.to_bits(),
            y.to_bits(),
            "sample {i} (frame {}) diverged between consoles",
            i / SAMPLES_PER_FRAME
        );
        assert_eq!(
            x.to_bits(),
            z.to_bits(),
            "sample {i} (frame {}) depends on the PRNG seed",
            i / SAMPLES_PER_FRAME
        );
    }
    assert!(a.iter().any(|&s| s != 0.0));
}

// ---------------------------------------------------------------------------
// Audio never perturbs the framebuffer
// ---------------------------------------------------------------------------

#[test]
fn framebuffer_is_identical_with_and_without_audio() {
    // Same cart, same draw calls, audio calls stripped out.
    let muted = DEMO
        .replace("music(0)", "")
        .replace("sfx(4)", "")
        .replace("sfx(5)", "");
    assert!(!muted.contains("music(0)") && !muted.contains("sfx(4)"));

    let inputs = script();
    let mut a = Console::new(DEMO, 0).unwrap();
    let mut b = Console::new(&muted, 0).unwrap();
    for (i, &mask) in inputs.iter().enumerate() {
        a.step(mask).unwrap();
        b.step(mask).unwrap();
        assert_eq!(
            fnv1a(a.framebuffer()),
            fnv1a(b.framebuffer()),
            "audio changed the picture at frame {i}"
        );
    }
    assert_eq!(a.take_logs(), b.take_logs());

    // ...and the muted variant really is silent.
    let mut muted_con = Console::new(&muted, 0).unwrap();
    for &mask in &inputs {
        muted_con.step(mask).unwrap();
        assert!(muted_con.audio_frame().iter().all(|&s| s == 0.0));
    }
}

// ---------------------------------------------------------------------------
// The echo bus (SNES half): delay / feedback / level + per-instrument sends
// ---------------------------------------------------------------------------

/// One voice, one note, then silence: the impulse the echo tests measure.
///
/// `bus` is the `__instruments__` echo line (empty string = no line at all) and
/// `send` is the instrument's `echo=` value.
fn echo_cart(bus: &str, send: u8) -> String {
    format!(
        "__lua__\nfunction _init() sfx(0) end\n\n\
         __instruments__\n{bus}\ninst wet wave=2 echo={send}\n\n\
         __sfx__\nsfx 0 speed=3\nC4 wet 6\n---\n---\n---\n---\n---\n---\n---\n"
    )
}

/// The echo's own contribution: the same cart rendered with and without its
/// `echo` line, subtracted. Everything upstream of the bus is identical in the
/// two runs (the sequencer never sees the echo), so what is left is exactly
/// what the delay line returned, scaled by `MIX_GAIN`.
fn echo_only(bus: &str, send: u8, frames: usize) -> Vec<f32> {
    let inputs = vec![0u8; frames];
    let wet = run_audio(&echo_cart(bus, send), 0, &inputs);
    let dry = run_audio(&echo_cart("", send), 0, &inputs);
    wet.iter().zip(&dry).map(|(a, b)| a - b).collect()
}

#[test]
fn echo_defaults_off_and_the_legacy_corpus_never_engages_it() {
    // The bit-identity guarantee itself is measured by the three golden tests
    // (DEMO_AUDIO_GOLDEN, SOUNDTEST_GROOVE_GOLDEN, SOUNDTEST_AB_GOLDEN, all
    // unmoved by this feature). This test pins the *reason* they are unmoved:
    // no cart in the corpus has an echo line, so the bus is never engaged and
    // the mixer never leaves the legacy statement.
    for cart in [DEMO, SOUNDTEST] {
        assert_eq!(Cart::parse(cart).unwrap().echo(), Echo::OFF);
    }
    let mut con = Console::new(DEMO, 0).unwrap();
    for &mask in &script() {
        con.step(mask).unwrap();
        assert_eq!(con.echo(), Echo::OFF);
        assert!(con.echo_is_silent(), "an unused delay line must stay empty");
    }
}

#[test]
fn an_echo_send_with_no_echo_line_changes_nothing() {
    // `echo=8` on an instrument is inert until a cart declares the bus: the
    // send is a routing amount, not a switch.
    let inputs = vec![0u8; 24];
    let sent = run_audio(&echo_cart("", 8), 0, &inputs);
    let dry = run_audio(&echo_cart("", 0), 0, &inputs);
    for (i, (a, b)) in sent.iter().zip(&dry).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "sample {i} moved without an echo line");
    }
}

#[test]
fn the_echo_line_round_trips_through_the_parser() {
    let cart = Cart::parse(&echo_cart("echo delay=7 feedback=5 level=3", 4)).unwrap();
    assert_eq!(
        cart.echo(),
        Echo {
            delay: 7,
            feedback: 5,
            level: 3
        }
    );
    assert_eq!(cart.instrument("wet").unwrap().echo, 4);
    // `feedback` is optional and defaults to a single slapback repeat.
    let cart = Cart::parse(&echo_cart("echo delay=1 level=8", 0)).unwrap();
    assert_eq!(
        cart.echo(),
        Echo {
            delay: 1,
            feedback: 0,
            level: 8
        }
    );
    // A send is optional and defaults to fully dry.
    assert_eq!(cart.instrument("wet").unwrap().echo, 0);
    // A mixer property, not a modulation one: an `echo=`-only instrument still
    // takes the flat (PoC v1) render path.
    assert!(cart.instrument("wet").unwrap().is_flat());
    // The bus line does not collide with an instrument *named* echo — the
    // soundtest cart has had one since PoC v2.
    let cart = Cart::parse(
        "__lua__\nx=1\n\n__instruments__\necho delay=4 level=4\ninst echo wave=1 echo=6\n",
    )
    .unwrap();
    assert_eq!(cart.echo().delay, 4);
    assert_eq!(cart.instrument("echo").unwrap().echo, 6);
}

#[test]
fn echo_parse_errors_name_the_range() {
    let bad = [
        ("echo delay=0 level=4", "echo delay must be 1-60"),
        ("echo delay=61 level=4", "echo delay must be 1-60"),
        ("echo delay=4 level=9", "echo level must be 0-8"),
        ("echo delay=4 level=4 feedback=9", "echo feedback must be 0-8"),
        ("echo delay=x level=4", "echo delay must be a number"),
        ("echo level=4", "echo needs `delay=<1-60>`"),
        ("echo delay=4", "echo needs `level=<0-8>`"),
        ("echo delay=4 level=4 wet=2", "unknown echo key"),
        ("echo delay=4 level=4 nonsense", "unexpected"),
        (
            "echo delay=4 level=4\necho delay=8 level=2",
            "at most one `echo` line",
        ),
    ];
    for (line, want) in bad {
        let err = Cart::parse(&echo_cart(line, 0)).unwrap_err();
        let Error::Cart(msg) = err else {
            panic!("{line:?} should be a cart error, got {err:?}");
        };
        assert!(
            msg.contains(want),
            "{line:?} said {msg:?}, expected it to mention {want:?}"
        );
        assert!(msg.contains("__instruments__"), "{msg:?} lacks the section");
    }
    // ...and the per-instrument send has a range too.
    let err = Cart::parse(&echo_cart("echo delay=4 level=4", 9)).unwrap_err();
    let Error::Cart(msg) = err else { panic!("want a cart error") };
    assert!(msg.contains("inst echo send must be 0-8"), "{msg:?}");
}

#[test]
fn a_single_impulse_repeats_at_exact_sample_offsets() {
    // delay=3 frames = 3 * 735 = 2205 samples, no feedback: one repeat.
    let delay_frames = 3usize;
    let d = delay_frames * SAMPLES_PER_FRAME;
    let inputs = vec![0u8; 12];
    let wet = run_audio(&echo_cart("echo delay=3 feedback=0 level=8", 8), 0, &inputs);
    let dry = run_audio(&echo_cart("", 8), 0, &inputs);

    // Not one bit moves before the delay has elapsed...
    for i in 0..d {
        assert_eq!(
            wet[i].to_bits(),
            dry[i].to_bits(),
            "the echo leaked at sample {i}, {} samples early",
            d - i
        );
    }
    // ...and the very first sample of the repeat is exactly on the boundary.
    assert_ne!(
        wet[d].to_bits(),
        dry[d].to_bits(),
        "no repeat at sample {d} (frame {delay_frames})"
    );

    // The repeat is an attenuated copy of the note, not a new sound: it is
    // loudest in the delay window that follows the note.
    let echo = echo_only("echo delay=3 feedback=0 level=8", 8, 12);
    let window = |k: usize| -> f32 {
        echo[k * d..((k + 1) * d).min(echo.len())]
            .iter()
            .map(|s| s.abs())
            .sum()
    };
    assert_eq!(window(0), 0.0);
    assert!(window(1) > 0.0, "the repeat never arrived");
    // Zero feedback means it happens once. (The note itself is three rows
    // long, so window 2 catches the tail of the *note's* echo, not a second
    // lap; by window 3 there is nothing left at all.)
    assert!(window(3) < window(1) * 0.01, "slapback echoed twice");
}

#[test]
fn echo_repeats_get_quieter_as_the_feedback_loops() {
    let d = 2 * SAMPLES_PER_FRAME;
    let echo = echo_only("echo delay=2 feedback=5 level=6", 6, 40);
    let window = |k: usize| -> f32 {
        echo[k * d..((k + 1) * d).min(echo.len())]
            .iter()
            .map(|s| s.abs())
            .sum()
    };
    assert_eq!(window(0), 0.0, "nothing before the first delay");
    // The note is 8 rows * 3 frames = 24 frames long, so the line is still
    // being fed for the first 12 windows. Measure the free decay after that.
    let mut prev = window(13);
    assert!(prev > 0.0, "the tail is silent before it should be");
    for k in 14..20 {
        let e = window(k);
        assert!(e < prev, "window {k} ({e}) is not quieter than {prev}");
        prev = e;
    }
    // Higher feedback = a longer tail, always.
    let tail = |fb: u8| -> f32 {
        echo_only(&format!("echo delay=2 feedback={fb} level=6"), 6, 40)[18 * d..]
            .iter()
            .map(|s| s.abs())
            .sum()
    };
    for fb in 1..8u8 {
        assert!(
            tail(fb + 1) > tail(fb),
            "feedback {} did not outlast {fb}",
            fb + 1
        );
    }
}

#[test]
fn a_voice_that_sends_nothing_stays_dry_while_another_one_echoes() {
    // Two voices, one wet and one dry, on their own channels.
    let cart = |bus: &str, wet_send: u8| {
        format!(
            "__lua__\nfunction _init() sfx(0, 0) sfx(1, 1) end\n\n\
             __instruments__\n{bus}\n\
             inst wet wave=2 echo={wet_send}\ninst dry wave=1\n\n\
             __sfx__\n\
             sfx 0 speed=3\nC4 wet 5\n---\n---\n---\n---\n---\n---\n---\n\
             sfx 1 speed=3\nG4 dry 5\n---\n---\n---\n---\n---\n---\n---\n"
        )
    };
    let inputs = vec![0u8; 20];

    // The wet voice moves the mix...
    let a = run_audio(&cart("echo delay=2 feedback=4 level=6", 6), 0, &inputs);
    let b = run_audio(&cart("", 6), 0, &inputs);
    assert!(
        a.iter().zip(&b).any(|(x, y)| x.to_bits() != y.to_bits()),
        "the wet voice never reached the bus"
    );

    // ...and with its send at zero, a running bus is inaudible: both voices are
    // dry, so nothing is fed into the line and the sum is bit-identical to a
    // console without an echo bus at all.
    let on = run_audio(&cart("echo delay=2 feedback=4 level=6", 0), 0, &inputs);
    let off = run_audio(&cart("", 0), 0, &inputs);
    for (i, (x, y)) in on.iter().zip(&off).enumerate() {
        assert_eq!(
            x.to_bits(),
            y.to_bits(),
            "sample {i}: an all-dry mix must not hear the bus"
        );
    }
}

#[test]
fn lua_echo_overrides_the_cart_line_and_can_kill_the_bus() {
    let cart = "__lua__\n\
         local f = 0\n\
         function _init() sfx(0) end\n\
         function _update()\n\
           f = f + 1\n\
           if f == 10 then echo(6, 3, 5) end\n\
           if f == 20 then echo(0) end\n\
           if f == 30 then echo(-1) end\n\
         end\n\n\
         __instruments__\necho delay=2 feedback=7 level=8\ninst wet wave=2 echo=8\n\n\
         __sfx__\nsfx 0 speed=3 loop=0,0\nC4 wet 6\n";
    let mut con = Console::new(cart, 0).unwrap();

    // The cart's line is in force from the first frame.
    assert_eq!(
        con.echo(),
        Echo {
            delay: 2,
            feedback: 7,
            level: 8
        }
    );
    for _ in 0..10 {
        con.step(0).unwrap();
    }
    // ...until Lua replaces it wholesale, exactly like `master()`.
    assert_eq!(
        con.echo(),
        Echo {
            delay: 6,
            feedback: 3,
            level: 5
        }
    );
    assert!(!con.echo_is_silent(), "the line should be full of repeats");

    // `echo(0)` kills the bus *and* flushes the line, so the tail cannot come
    // back later.
    for _ in 0..10 {
        con.step(0).unwrap();
    }
    assert_eq!(con.echo(), Echo::OFF);
    assert!(con.echo_is_silent(), "killing the bus must empty the line");

    // `echo(-1)` is the same thing, spelled loudly.
    for _ in 0..10 {
        con.step(0).unwrap();
    }
    assert_eq!(con.echo(), Echo::OFF);

    // Range checks, straight out of Lua.
    for bad in ["echo(61)", "echo(-2)", "echo(4, 9, 4)", "echo(4, 0, 9)"] {
        let cart = format!("__lua__\nfunction _init() {bad} end\n");
        let err = Console::new(&cart, 0).unwrap_err();
        assert!(
            format!("{err}").contains("out of range"),
            "{bad} should be rejected, got {err}"
        );
    }
}

/// Six voices, all sending everything, into maximum feedback and a full-scale
/// return: the loudest thing a cart can ask the bus for. `stop_at` is the frame
/// the voices are cut, so the tail can be watched draining.
fn echo_stress_cart(master_line: &str, stop_at: u32) -> String {
    let starts: String = (0..CHANNEL_COUNT).map(|c| format!("sfx(0, {c}) ")).collect();
    format!(
        "__lua__\n\
         local f = 0\n\
         function _init() {starts} end\n\
         function _update() f = f + 1 if f == {stop_at} then sfx(-1) end end\n\n\
         __instruments__\n{master_line}\necho delay=1 feedback=8 level=8\n\
         inst loud wave=2 echo=8\n\n\
         __sfx__\nsfx 0 speed=240 loop=0,0\nC2 loud 7\n"
    )
}

#[test]
fn echo_stress_stays_bounded_and_finite() {
    let mut con = Console::new(&echo_stress_cart("", 200), 0).unwrap();
    let loud = collect(&mut con, 200);
    for (i, s) in loud.iter().enumerate() {
        assert!(s.is_finite(), "sample {i} is {s}");
        assert!(
            (-1.0..=1.0).contains(s),
            "sample {i} escaped full scale: {s}"
        );
    }
    // It really is slammed: without drive the final clamp is doing the work.
    assert!(
        loud.iter().filter(|s| s.abs() == 1.0).count() > loud.len() / 10,
        "the stress probe never got loud enough to prove anything"
    );

    // The voices are cut at frame 200. The tail must drain, monotonically
    // enough to be obviously convergent, and end in real silence.
    let tail = collect(&mut con, 400);
    let rms = |xs: &[f32]| -> f64 {
        (xs.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / xs.len() as f64).sqrt()
    };
    let window = |k: usize| -> f64 { rms(&tail[k * 60 * SAMPLES_PER_FRAME..(k + 1) * 60 * SAMPLES_PER_FRAME]) };
    assert!(window(0) > 0.0, "the tail vanished instantly");
    for k in 1..6 {
        assert!(
            window(k) < window(k - 1),
            "tail window {k} ({}) is not quieter than {}",
            window(k),
            window(k - 1)
        );
    }
    assert!(
        tail[tail.len() - SAMPLES_PER_FRAME..]
            .iter()
            .all(|s| s.abs() < 1e-4),
        "the echo never faded out"
    );
    assert!(tail.iter().all(|s| s.is_finite()));
}

#[test]
fn a_driven_master_bus_keeps_the_echo_below_full_scale() {
    // The same stress probe with the master bus engaged: the shaper's output is
    // bounded by MAKEUP[drive] < 1, so even an echo screaming into it cannot
    // reach the hard clamp. `master drive=1` is the cheap insurance the SPEC
    // recommends for echo-heavy carts.
    for drive in 1..=MAX_DRIVE {
        let cart = echo_stress_cart(&format!("master drive={drive}"), 10_000);
        let mut con = Console::new(&cart, 0).unwrap();
        let samples = collect(&mut con, 120);
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            peak < 1.0,
            "drive {drive} let the echo hit the clamp (peak {peak})"
        );
        assert!(samples.iter().all(|s| s.is_finite()));
    }
}

#[test]
fn the_echo_bus_is_deterministic_and_seed_independent() {
    let cart = echo_stress_cart("", 60);
    let inputs = vec![0u8; 200];
    let a = run_audio(&cart, 0, &inputs);
    let b = run_audio(&cart, 0, &inputs);
    let c = run_audio(&cart, 999_999, &inputs);
    for (i, ((x, y), z)) in a.iter().zip(&b).zip(&c).enumerate() {
        assert_eq!(x.to_bits(), y.to_bits(), "sample {i} is not reproducible");
        assert_eq!(x.to_bits(), z.to_bits(), "sample {i} depends on the seed");
    }
}

/// Golden hash of the soundtest cart's "ECHO  DELAY BUS" entry (menu index 14,
/// pattern 13): FNV-1a over the little-endian bits of the samples rendered
/// while navigating there and then playing 150 frames, seed 0.
///
/// 150 frames covers the first note, two of its repeats and the start of the
/// second note, so this pins the delay line, the feedback path, the loop
/// lowpass coefficient, the eighths ladder and the Lua `echo()` setter all at
/// once — the echo counterpart of [`SOUNDTEST_AB_GOLDEN`]. If it changes, the
/// echo bus changed.
const SOUNDTEST_ECHO_GOLDEN: u64 = 0x7a66_500c_06d8_d557;

#[test]
fn soundtest_echo_matches_the_golden_hash() {
    let hash = hash_samples(&run_audio(
        SOUNDTEST,
        0,
        &soundtest_script(SOUNDTEST_ECHO, 150),
    ));
    assert_eq!(
        hash, SOUNDTEST_ECHO_GOLDEN,
        "soundtest echo audio changed; new hash is {hash:#018x}"
    );
}

#[test]
fn the_soundtest_echo_entry_fills_its_gaps_with_repeats() {
    // The melody is four notes in 32 rows, each a 12-frame pluck, and the bus
    // is `echo(24, 5, 6)`. So the first note's gap has a shape that could not
    // come from anything else: sound, then real silence, then a repeat landing
    // exactly 24 frames after the note-on, then silence, then a quieter one.
    let s = soundtest_played(SOUNDTEST_ECHO, 80);
    let rms = |a: usize, b: usize| -> f64 {
        let x = &s[a * SAMPLES_PER_FRAME..b * SAMPLES_PER_FRAME];
        (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
    };

    let note = rms(0, 10);
    assert!(note > 0.05, "the pluck is inaudible ({note})");
    // The pluck's envelope reaches zero at frame 11 and the first repeat is
    // still 13 frames away: this window is *exactly* silent.
    assert_eq!(rms(12, 23), 0.0, "something is ringing before the repeat");
    // The repeat, on the beat it was asked for.
    let first = rms(24, 34);
    assert!(first > 0.02, "no repeat arrived at frame 24 ({first})");
    assert!(first < note, "the repeat is louder than the note");
    // Feedback: silence again, then a second, quieter lap 24 frames later.
    assert_eq!(rms(36, 47), 0.0, "the second gap is not clean");
    let second = rms(48, 58);
    assert!(second > 0.01, "the feedback lap never came back ({second})");
    assert!(
        second < first,
        "repeat 2 ({second}) is not quieter than repeat 1 ({first})"
    );
}

#[test]
fn only_the_soundtest_echo_entry_runs_the_bus() {
    let mut con = Console::new(SOUNDTEST, 0).unwrap();
    assert_eq!(con.echo(), Echo::OFF, "the cart declares no echo line");
    assert_eq!(Cart::parse(SOUNDTEST).unwrap().echo(), Echo::OFF);

    // Every entry but the last leaves the bus switched off, which is what
    // keeps the two older goldens bit-identical.
    for entry in 0..SOUNDTEST_ECHO {
        let mut con = Console::new(SOUNDTEST, 0).unwrap();
        for mask in soundtest_script(entry, 30) {
            con.step(mask).unwrap();
            assert_eq!(con.echo(), Echo::OFF, "entry {entry} touched the echo bus");
        }
        assert!(con.echo_is_silent());
    }

    // The ECHO entry switches it on from `_update`, and B switches it back off
    // and flushes the line.
    for mask in soundtest_script(SOUNDTEST_ECHO, 40) {
        con.step(mask).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(13));
    assert_eq!(
        con.echo(),
        Echo {
            delay: 24,
            feedback: 5,
            level: 6
        }
    );
    assert!(!con.echo_is_silent(), "the line should be holding repeats");

    con.step(input::B).unwrap();
    assert_eq!(con.echo(), Echo::OFF);
    assert!(con.echo_is_silent(), "stopping must flush the delay line");
    let tail = collect(&mut con, 6);
    assert!(
        tail[SAMPLES_PER_FRAME..].iter().all(|&s| s == 0.0),
        "B should silence the console, echo tail included"
    );
}

// ---------------------------------------------------------------------------
// Wavetables: `wavetable <slot> <32 nibbles>` + `wave=w<slot>`
// ---------------------------------------------------------------------------

/// A full-scale rising staircase: nibble `i / 2`, i.e. `00112233...eeff`.
/// Sweeps the whole nibble ladder in table order, so a playback test that
/// matches it sample for sample has pinned both the index math and the
/// amplitude mapping at once.
const RAMP_HEX: &str = "00112233445566778899aabbccddeeff";

/// The square wave, written as a table: the first half of the cycle at code
/// `f` (+1.0), the second at code `0` (-1.0).
const SQUARE_HEX: &str = "ffffffffffffffff0000000000000000";

fn ramp_nibbles() -> [u8; WAVETABLE_LEN] {
    std::array::from_fn(|i| (i / 2) as u8)
}

#[test]
fn wavetable_lines_parse_and_round_trip() {
    let cart = inst_cart(&format!(
        "wavetable 0 {RAMP_HEX}\n\
         wavetable 7 ffff0000 ffff0000 ffff0000 ffff0000\n\
         inst saw_ish wave=w0 env=0,8,4\n\
         inst buzzy   wave=w7"
    ))
    .unwrap();

    let w0 = cart.wavetable(0).expect("slot 0 is defined");
    assert_eq!(w0.nibbles, ramp_nibbles());
    // The text round-trips: `hex()` prints exactly what the cart said.
    assert_eq!(w0.hex(), RAMP_HEX);
    // Whitespace grouping is purely cosmetic - four groups of eight parse to
    // the same 32 samples as one run of 32 characters.
    let w7 = cart.wavetable(7).expect("slot 7 is defined");
    assert_eq!(w7.hex(), "ffff0000ffff0000ffff0000ffff0000");
    // Untouched slots stay empty.
    for slot in 1..7 {
        assert!(cart.wavetable(slot).is_none(), "slot {slot} should be empty");
    }
    assert_eq!(cart.audio().wavetables().iter().flatten().count(), 2);

    // `wave=w<slot>` resolves to WAVE_TABLE_BASE + slot, and every other
    // instrument field is untouched by the change of oscillator.
    let saw_ish = cart.instrument("saw_ish").unwrap();
    assert_eq!(saw_ish.wave, WAVE_TABLE_BASE);
    assert_eq!(saw_ish.env, Some(Env { attack: 0, decay: 8, sustain: 4 }));
    assert_eq!(cart.instrument("buzzy").unwrap().wave, WAVE_TABLE_BASE + 7);
    // ...and the bank can go back the other way.
    assert_eq!(
        cart.audio().wavetable_for_wave(WAVE_TABLE_BASE + 7).map(Wavetable::hex),
        Some(w7.hex())
    );
    assert!(cart.audio().wavetable_for_wave(2).is_none(), "builtin waves have no table");
}

#[test]
fn a_sfx_row_may_name_a_wavetable_slot_directly() {
    // `w<slot>` is a wave *source*, so it is legal anywhere a bare wave digit
    // is: the row means "the flat instrument on that table", exactly as `2`
    // means "the flat instrument on the square".
    let text = format!(
        "__lua__\nx = 1\n\n\
         __instruments__\nwavetable 3 {SQUARE_HEX}\n\n\
         __sfx__\nsfx 0 speed=8\nA4 w3 6\n---\n"
    );
    let cart = Cart::parse(&text).unwrap();
    assert_eq!(
        cart.sfx(0).unwrap().rows[0],
        SfxRow::Note { note: 57, wave: WAVE_TABLE_BASE + 3, vol: 6 }
    );
    // No instrument named, so no modulation and no echo send: the PoC v1 path.
    assert_eq!(cart.sfx(0).unwrap().row_mod(0), RowMod::default());
}

#[test]
fn malformed_wavetables_are_line_numbered_cart_errors() {
    fn wt_err(body: &str, line: usize, needle: &str) {
        expect_cart_error(&format!("__lua__\nx=1\n\n__instruments__\n{body}"), line, needle);
    }

    // Slot range and shape.
    wt_err("wavetable 8 00112233445566778899aabbccddeeff\n", 1, "wavetable slot must be 0-7");
    wt_err("wavetable 0\n", 1, "expected `wavetable <slot 0-7> <32 hex nibbles>`");
    wt_err("wavetable\n", 1, "expected `wavetable <slot 0-7>");
    // Too few / too many nibbles, counted after the groups are joined.
    wt_err("wavetable 0 0011223344556677 8899aabbccddee\n", 1, "needs exactly 32 hex nibbles, found 30");
    wt_err("wavetable 0 00112233445566778899aabbccddeeff0\n", 1, "found 33");
    // Bad hex, reported by sample index.
    wt_err("wavetable 0 00112233445566778899aabbccddeegf\n", 1, "sample 30: 'g' is not a hex nibble");
    // One slot, one table.
    wt_err(
        "wavetable 2 00112233445566778899aabbccddeeff\nwavetable 2 ffffffffffffffff0000000000000000\n",
        2,
        "duplicate wavetable w2",
    );
    // Undefined references are errors at parse time, never a silent fallback
    // to some default waveform - the console's house style.
    wt_err("inst lead wave=w1\n", 1, "references wavetable w1, which the cart does not define");
    wt_err(
        "wavetable 0 00112233445566778899aabbccddeeff\ninst lead wave=w5\n",
        2,
        "defined: w0",
    );
    wt_err("inst lead wave=w9\n", 1, "wave slot must be 0-7");
    wt_err("inst lead wave=wx\n", 1, "wave must be 0-5 (builtin) or w0-w7");
    // A slot name is reserved, so `w0` in a sfx row is never ambiguous.
    wt_err("inst w0 wave=1\n", 1, "must not look like a wavetable slot");

    // Same rules from a sfx row.
    expect_cart_error(
        "__lua__\nx=1\n\n__sfx__\nsfx 0 speed=8\nA4 w0 6\n",
        2,
        "references wavetable w0, which the cart does not define",
    );
    expect_cart_error(
        &format!("__lua__\nx=1\n\n__instruments__\nwavetable 0 {RAMP_HEX}\n\n__sfx__\nsfx 0 speed=8\nA4 w4 6\n"),
        2,
        "references wavetable w4",
    );
}

/// A one-note cart on a wavetable, played from `_init`.
fn wt_cart(hex: &str, row: &str) -> String {
    format!(
        "__lua__\nfunction _init() sfx(0, 0) end\n\n\
         __instruments__\nwavetable 0 {hex}\n\n\
         __sfx__\nsfx 0 speed=60\n{row}\n"
    )
}

#[test]
fn a_ramp_wavetable_plays_the_exact_expected_samples() {
    // C2 (65.406395 Hz) at vol 7 on the staircase table. Every number below
    // is reproduced from the documented model - fixed-point phase, top five
    // bits as the index, (2n-15)/15 as the amplitude, the 64-sample click
    // guard, then the 0.25 mix gain - so this is a bit-exact statement of
    // what "wavetable playback" means on this console.
    let mut con = console(&wt_cart(RAMP_HEX, "C2 w0 7"));
    let samples = collect(&mut con, 1);

    let nibbles = ramp_nibbles();
    let inc = (f64::from(NOTE_FREQ[24]) * 4_294_967_296.0 / 44_100.0 + 0.5) as u32;
    let mut phase: u32 = 0;
    for (k, &got) in samples.iter().enumerate() {
        let amp = ((k + 1) as f32 / 64.0).min(1.0);
        phase = phase.wrapping_add(inc);
        let level = NIBBLE_LEVEL[usize::from(nibbles[(phase >> 27) as usize])];
        let want = (amp * level) * 0.25;
        assert_eq!(
            got.to_bits(),
            want.to_bits(),
            "sample {k}: got {got}, want {want} (phase {phase:#010x}, index {})",
            phase >> 27
        );
    }
    // ...and the staircase really did walk the whole table inside one frame.
    assert!(samples.iter().any(|&s| s > 0.2) && samples.iter().any(|&s| s < -0.2));
}

#[test]
fn wavetable_square_is_the_builtin_square() {
    // The mapping is (2n-15)/15 precisely so that code 0 is -1.0 and code f is
    // +1.0: a table written as sixteen f's then sixteen 0's is not "like" the
    // builtin square, it *is* the builtin square, sample for sample.
    let mut builtin = console("__lua__\nfunction _init() sfx(0, 0) end\n\n__sfx__\nsfx 0 speed=60\nA4 2 6\n");
    let mut table = console(&wt_cart(SQUARE_HEX, "A4 w0 6"));
    let a = collect(&mut builtin, 30);
    let b = collect(&mut table, 30);
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        assert_eq!(x.to_bits(), y.to_bits(), "sample {i}: {x} vs {y}");
    }
    assert!(a.iter().any(|&s| s != 0.0));
}

#[test]
fn four_bits_cannot_hit_zero_so_a_flat_table_is_dc_not_silence() {
    // Codes 7 and 8 straddle zero at -1/15 and +1/15. An all-8 table is
    // therefore a constant +0.0667 DC offset rather than silence: audibly
    // nothing, but it is not the zero signal, and this pins the documented
    // behaviour so nobody "fixes" it into a /16 ladder later.
    let mut con = console(&wt_cart("88888888888888888888888888888888", "A4 w0 7"));
    let samples = collect(&mut con, 4);
    let steady = &samples[SAMPLES_PER_FRAME..];
    let dc = (1.0f32 / 15.0) * 0.25;
    for (i, &s) in steady.iter().enumerate() {
        assert_eq!(s.to_bits(), dc.to_bits(), "sample {i} is {s}, not the DC level");
    }
    assert!(dc.abs() < 0.02, "one half-code of DC should be inaudible, got {dc}");

    // The exact-zero question is decidable on the nibbles themselves.
    let flat = Wavetable { nibbles: [8; WAVETABLE_LEN] };
    assert_eq!(flat.dc_sum(), 32, "all-8 is one half-code high");
    assert_eq!(Wavetable { nibbles: [7; WAVETABLE_LEN] }.dc_sum(), -32);
    // ...and any table that pairs each code with its mirror is exactly DC-free.
    assert_eq!(Wavetable { nibbles: ramp_nibbles() }.dc_sum(), 0);
    let square = Wavetable {
        nibbles: std::array::from_fn(|i| if i < 16 { 15 } else { 0 }),
    };
    assert_eq!(square.dc_sum(), 0);
}

#[test]
fn a_wavetable_voice_composes_with_env_vib_sweep_fx_and_echo() {
    // Nothing about a wavetable is special downstream: it is a wave source, so
    // every existing instrument and mixer feature has to work on it unchanged.
    let cart = format!(
        "__lua__\n\
         function _init() sfx(0, 0) sfx(1, 1) end\n\n\
         __instruments__\n\
         wavetable 0 {RAMP_HEX}\n\
         echo delay=6 feedback=4 level=6\n\
         inst plain  wave=w0\n\
         inst shaped wave=w0 env=6,10,3 vib=40,8,2 echo=6\n\
         inst diving wave=w0 sweep=-12,20\n\n\
         __sfx__\n\
         sfx 0 speed=40\nA4 plain 6\nA4 shaped 6\nA4 diving 6 arp3,7\n\
         sfx 1 speed=40\n---\n---\n---\n"
    );
    let mut con = Console::new(&cart, 0).unwrap();
    let samples = collect(&mut con, 120);

    // Row 0 is the flat wavetable voice; row 1 adds envelope + vibrato and is
    // a different signal for it.
    let row = |n: usize| &samples[n * 40 * SAMPLES_PER_FRAME..(n + 1) * 40 * SAMPLES_PER_FRAME];
    assert!(
        row(0).iter().zip(row(1)).any(|(a, b)| a.to_bits() != b.to_bits()),
        "env/vib changed nothing on a wavetable voice"
    );
    // The envelope really is an envelope: row 1 starts quiet and swells.
    let rms = |xs: &[f32]| -> f64 {
        (xs.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / xs.len() as f64).sqrt()
    };
    let r1 = row(1);
    assert!(
        rms(&r1[..SAMPLES_PER_FRAME]) < rms(&r1[8 * SAMPLES_PER_FRAME..9 * SAMPLES_PER_FRAME]),
        "the 6-frame attack did not swell"
    );
    // Vibrato moves the pitch around: the frequency is not constant inside
    // row 1 the way it is inside row 0.
    let flat_a = frame_freq(&samples, 20);
    let flat_b = frame_freq(&samples, 30);
    assert!((flat_a - flat_b).abs() < 1.0, "the flat voice drifted: {flat_a} vs {flat_b}");
    let vib_lo = frame_freq(&samples, 50);
    let vib_hi = frame_freq(&samples, 54);
    assert!((vib_lo - vib_hi).abs() > 1.0, "vibrato did not bend the pitch");
    // Sweep: row 2 dives an octave over 20 frames, so it ends far below A4.
    let start = frame_freq(&samples, 81);
    let end = frame_freq(&samples, 99);
    assert!(end < start * 0.75, "sweep did not dive ({start} -> {end})");
    // The echo bus took the `echo=6` send from a wavetable voice like any
    // other, and the whole thing stayed inside the rails.
    assert!(!con.echo_is_silent(), "the wavetable voice never fed the delay line");
    assert!(samples.iter().all(|s| (-1.0..=1.0).contains(s)));
}

#[test]
fn wavetable_playback_is_reproducible_and_seed_independent() {
    let cart = wt_cart(RAMP_HEX, "C4 w0 6");
    let inputs = vec![0u8; 90];
    let a = run_audio(&cart, 0, &inputs);
    let b = run_audio(&cart, 0, &inputs);
    let c = run_audio(&cart, 4_242_424, &inputs);
    for (i, ((x, y), z)) in a.iter().zip(&b).zip(&c).enumerate() {
        assert_eq!(x.to_bits(), y.to_bits(), "sample {i} is not reproducible");
        assert_eq!(x.to_bits(), z.to_bits(), "sample {i} depends on the seed");
    }
}

#[test]
fn carts_without_wavetables_are_untouched() {
    // The bit-identity guarantee is *measured* by the four golden hashes
    // (`demo_cart_audio_matches_the_golden_hash`, `soundtest_groove_...`,
    // `soundtest_saturation_ab_...` and `soundtest_echo_...`), none of which
    // moved when wavetables landed. What is asserted here is the reason they
    // could not move: no pre-wavetable cart can produce a wave id above 5, and
    // an empty slot table is what every such cart parses to.
    for text in [DEMO, SOUNDTEST] {
        let cart = Cart::parse(text).unwrap();
        for id in 0..WAVETABLE_SLOTS as u8 {
            if text == DEMO {
                assert!(cart.wavetable(id).is_none(), "the demo cart declares no tables");
            }
        }
    }
    let demo = Cart::parse(DEMO).unwrap();
    for id in demo.audio().sfx_ids() {
        for row in &demo.sfx(id).unwrap().rows {
            if let SfxRow::Note { wave, .. } = row {
                assert!(*wave < WAVE_COUNT, "demo row uses wave {wave}");
            }
        }
    }
}

/// Golden hash of the soundtest cart's "WAVETABLE W0-W2" entry (menu index 15,
/// pattern 14): FNV-1a over the little-endian bits of the samples rendered
/// while navigating there and then playing 150 frames, seed 0.
///
/// 150 frames is most of the first bar: the hollow-table lead over the organ
/// pad, envelopes and all. It pins the nibble ladder, the index shift and the
/// no-interpolation decision at once - change any of the three and this moves.
const SOUNDTEST_WAVETABLE_GOLDEN: u64 = 0xf8c7_00f2_fd62_8f7b;

#[test]
fn soundtest_wavetable_matches_the_golden_hash() {
    let hash = hash_samples(&run_audio(
        SOUNDTEST,
        0,
        &soundtest_script(SOUNDTEST_WAVETABLE, 150),
    ));
    assert_eq!(
        hash, SOUNDTEST_WAVETABLE_GOLDEN,
        "soundtest wavetable audio changed; new hash is {hash:#018x}"
    );
}

#[test]
fn the_soundtest_wavetable_pad_holds_through_the_melody_gaps() {
    // The lead rests on every other row, so anything still sounding in those
    // gaps is the organ pad - and the pad only holds because `wt_organ` has no
    // envelope and its rows restate the same note. If someone gives it an
    // `env`, the retrigger turns the pad into a tremolo and this window stops
    // being continuous.
    let s = soundtest_played(SOUNDTEST_WAVETABLE, 130);
    let rms = |a: usize, b: usize| -> f64 {
        let x = &s[a * SAMPLES_PER_FRAME..b * SAMPLES_PER_FRAME];
        (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
    };
    // One frame-window per frame of the first bar: none of them is silent.
    for f in 2..126 {
        assert!(rms(f, f + 1) > 0.01, "frame {f} of the pad went silent");
    }
    // ...and the lead on top makes the note frames louder than the gap frames.
    assert!(rms(2, 8) > rms(12, 16), "the lead is not on top of the pad");
}

#[test]
fn the_soundtest_wavetable_entry_uses_three_dc_free_tables() {
    let cart = Cart::parse(SOUNDTEST).unwrap();
    let defined: Vec<u8> = (0..WAVETABLE_SLOTS as u8)
        .filter(|&i| cart.wavetable(i).is_some())
        .collect();
    assert_eq!(defined, vec![0, 1, 2], "w0 hollow, w1 organ, w2 buzz");
    for slot in defined {
        let t = cart.wavetable(slot).unwrap();
        assert_eq!(t.dc_sum(), 0, "table w{slot} is not DC-free");
        assert_eq!(t.nibbles.iter().copied().min(), Some(0), "w{slot} wastes headroom");
        assert_eq!(t.nibbles.iter().copied().max(), Some(15), "w{slot} wastes headroom");
    }
    // The three instruments that play them, and nothing else in the cart.
    for (name, slot) in [("wt_hollow", 0), ("wt_organ", 1), ("wt_buzz", 2)] {
        assert_eq!(cart.instrument(name).unwrap().wave, WAVE_TABLE_BASE + slot);
    }
    let table_voices = cart
        .instruments()
        .iter()
        .filter(|i| i.wave >= WAVE_TABLE_BASE)
        .count();
    assert_eq!(table_voices, 3);
    // Every other voice is still a builtin, which is why entries 0-14 render
    // exactly as they did before.
    assert!(
        cart.instruments()
            .iter()
            .filter(|i| !i.name.starts_with("wt_"))
            .all(|i| i.wave < WAVE_COUNT)
    );
}

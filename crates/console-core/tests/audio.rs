//! Audio: `__sfx__`/`__music__` parsing, the sequencer, the synth and the
//! determinism contract.

use console_core::{
    CHANNEL_COUNT, Cart, Console, Env, Error, Fx, PatternEnd, SAMPLE_RATE, SAMPLES_PER_FRAME,
    SfxRow, Sweep, Vib, freq_at, input,
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
pat 9 loop=0 : 0 0 0 0
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
    assert_eq!(p0.slots, [Some(0), None, None, Some(63)]);
    assert_eq!(p0.end, PatternEnd::Next);

    let p7 = cart.pattern(7).unwrap();
    assert_eq!(p7.slots, [None, Some(63), None, None]);
    assert_eq!(p7.end, PatternEnd::Stop);

    let p9 = cart.pattern(9).unwrap();
    assert_eq!(p9.slots, [Some(0); CHANNEL_COUNT]);
    assert_eq!(p9.end, PatternEnd::Loop(0));

    assert_eq!(cart.audio().next_pattern_after(0), Some(7));
    assert_eq!(cart.audio().next_pattern_after(7), Some(9));
    assert_eq!(cart.audio().next_pattern_after(9), None);
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
    expect_cart_error(&format!("{SFX}pat 0 : 0 - -\n"), 1, "4 channel slots");
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
    assert!(!ch[2].busy && !ch[3].busy);
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

    // The next auto sfx takes the remaining free channel.
    con.eval("sfx(0)").unwrap();
    assert_eq!(con.audio_channels()[3].sfx, Some(0));
}

#[test]
fn auto_sfx_steals_channel_three_when_everything_is_busy() {
    let mut con = seq_console("music(0)");
    con.eval("sfx(2) sfx(2)").unwrap();
    assert!(con.audio_channels().iter().all(|c| c.busy));
    con.eval("sfx(0)").unwrap();
    let ch = con.audio_channels();
    assert_eq!(ch[3].sfx, Some(0), "channel 3 is the steal target");
    assert_eq!(ch[0].sfx, Some(0), "music channel 0 is left alone");
    assert_eq!(ch[2].sfx, Some(2), "channel 2 is left alone");
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
    let msg = lua_error(SEQ, "sfx(0, 4)");
    assert!(msg.contains("channel 4 out of range"), "{msg}");
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
// PoC v2: the soundtest cart
// ---------------------------------------------------------------------------

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
    // 11 instruments, 20 sfx, one self-looping pattern per menu entry.
    assert_eq!(cart.instruments().len(), 11);
    assert_eq!(cart.audio().sfx_ids().count(), 20);
    let pats: Vec<u8> = cart.audio().pattern_ids().collect();
    assert_eq!(pats, (0..=12).collect::<Vec<u8>>());
    for id in 0..=12u8 {
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
    assert_eq!(cart.instrument("kick").unwrap().sweep.unwrap().semis, -14);
    // Tempo sugar drives most of it.
    let tempo = cart.audio().tempo().unwrap();
    assert_eq!((tempo.bpm, tempo.rows_per_beat, tempo.speed), (112, 4, 8));
}

#[test]
fn every_soundtest_entry_makes_a_different_noise() {
    let mut seen: Vec<u64> = Vec::new();
    for entry in 0..13 {
        let samples = run_audio(SOUNDTEST, 0, &soundtest_script(entry, 90));
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

    // Up wraps to the top of the list...
    for mask in [input::UP, 0, input::UP, 0, input::UP, 0, input::A, 0] {
        con.step(mask).unwrap();
    }
    assert_eq!(con.music_pattern(), Some(12), "UP past the top wraps around");

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
const SOUNDTEST_GROOVE_GOLDEN: u64 = 0x98fd_7369_d783_2a07;

#[test]
fn soundtest_groove_matches_the_golden_hash() {
    let hash = hash_samples(&run_audio(SOUNDTEST, 0, &soundtest_script(12, 150)));
    assert_eq!(
        hash, SOUNDTEST_GROOVE_GOLDEN,
        "soundtest groove audio changed; new hash is {hash:#018x}"
    );
}

#[test]
fn the_soundtest_groove_is_deterministic_across_consoles_and_seeds() {
    let script = soundtest_script(12, 150);
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

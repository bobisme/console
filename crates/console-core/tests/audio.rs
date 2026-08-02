//! Audio: `__sfx__`/`__music__` parsing, the sequencer, the synth and the
//! determinism contract.

use console_core::{
    Cart, CHANNEL_COUNT, Console, Error, PatternEnd, SAMPLE_RATE, SAMPLES_PER_FRAME, SfxRow, input,
};

const DEMO: &str = include_str!("../../../carts/demo.cart");

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
        "expected `NOTE WAVE VOL`",
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

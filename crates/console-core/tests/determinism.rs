//! The determinism contract: same cart + same seed + same inputs => identical pixels.

use console_core::{Console, input};

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

/// A scripted input log: 120 frames of movement, bursts and a reset.
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
    assert_eq!(log.len(), 120);
    log
}

/// Run a cart for the scripted inputs and hash the resulting framebuffer.
fn run(cart: &str, seed: u64, inputs: &[u8]) -> (u64, Vec<String>, u64) {
    let mut con = Console::new(cart, seed).expect("cart loads");
    for &mask in inputs {
        con.step(mask).expect("frame runs");
    }
    let hash = fnv1a(con.framebuffer());
    (hash, con.take_logs(), con.frame_count())
}

#[test]
fn demo_cart_is_reproducible_across_fresh_consoles() {
    let inputs = script();
    let (h1, logs1, f1) = run(DEMO, 0, &inputs);
    let (h2, logs2, f2) = run(DEMO, 0, &inputs);
    assert_eq!(h1, h2, "framebuffer hash must be stable");
    assert_eq!(logs1, logs2, "printh output must be stable");
    assert_eq!(f1, 120);
    assert_eq!(f2, 120);
    assert!(!logs1.is_empty(), "demo should log on button presses");
}

#[test]
fn every_frame_matches_frame_by_frame() {
    let inputs = script();
    let mut a = Console::new(DEMO, 3).unwrap();
    let mut b = Console::new(DEMO, 3).unwrap();
    for (i, &mask) in inputs.iter().enumerate() {
        a.step(mask).unwrap();
        b.step(mask).unwrap();
        assert_eq!(
            fnv1a(a.framebuffer()),
            fnv1a(b.framebuffer()),
            "divergence at frame {i}"
        );
    }
}

#[test]
fn different_seeds_diverge() {
    let inputs = script();
    let (h0, _, _) = run(DEMO, 0, &inputs);
    let (h1, _, _) = run(DEMO, 1, &inputs);
    let (h2, _, _) = run(DEMO, 12345, &inputs);
    assert_ne!(h0, h1);
    assert_ne!(h0, h2);
    assert_ne!(h1, h2);
}

#[test]
fn different_inputs_diverge() {
    let inputs = script();
    let mut other = inputs.clone();
    other[40] ^= input::LEFT;

    // The demo clamps and resets the player, so two input logs can converge on
    // the same final frame; what must differ is the sequence of frames.
    let mut a = Console::new(DEMO, 0).unwrap();
    let mut b = Console::new(DEMO, 0).unwrap();
    let mut diverged = false;
    for i in 0..inputs.len() {
        a.step(inputs[i]).unwrap();
        b.step(other[i]).unwrap();
        diverged |= fnv1a(a.framebuffer()) != fnv1a(b.framebuffer());
    }
    assert!(diverged, "different inputs must produce different frames");
}

const RND_CART: &str = "\
__lua__
vals = {}
function _update()
  vals[#vals+1] = rnd(1)
  pset(flr(rnd(144)), flr(rnd(256)), 1 + flr(rnd(15)))
end
function _draw() end
";

#[test]
fn rnd_is_seeded_and_reproducible() {
    let inputs = vec![0u8; 60];
    let (a, _, _) = run(RND_CART, 99, &inputs);
    let (b, _, _) = run(RND_CART, 99, &inputs);
    let (c, _, _) = run(RND_CART, 100, &inputs);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn rnd_range_and_srand_reset() {
    let mut con = Console::new(
        "__lua__
         function _update()
           lo, hi = 1e18, -1e18
           for i = 1, 500 do
             local v = rnd(10)
             if v < lo then lo = v end
             if v > hi then hi = v end
           end
           srand(7)
           a1, a2 = rnd(), rnd()
           srand(7)
           b1, b2 = rnd(), rnd()
         end",
        0,
    )
    .unwrap();
    con.step(0).unwrap();
    let f = |n: &str| con.get_global(n).unwrap().as_f64().unwrap();
    assert!(f("lo") >= 0.0 && f("hi") < 10.0);
    assert!(f("hi") > 8.0, "rnd should cover its range");
    assert_eq!(f("a1"), f("b1"));
    assert_eq!(f("a2"), f("b2"));
    assert_ne!(f("a1"), f("a2"));
}

#[test]
fn default_seed_zero_still_varies_between_calls() {
    let mut con = Console::new(
        "__lua__\nfunction _update() x, y = rnd(), rnd() end",
        0,
    )
    .unwrap();
    con.step(0).unwrap();
    let x = con.get_global("x").unwrap().as_f64().unwrap();
    let y = con.get_global("y").unwrap().as_f64().unwrap();
    assert_ne!(x, y);
    assert!((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y));
}

#[test]
fn demo_cart_actually_renders_something() {
    let inputs = script();
    let mut con = Console::new(DEMO, 0).unwrap();
    for &m in &inputs {
        con.step(m).unwrap();
    }
    let fb = con.framebuffer();
    let distinct: std::collections::BTreeSet<u8> = fb.iter().copied().collect();
    assert!(
        distinct.len() >= 6,
        "demo should use several colours, saw {distinct:?}"
    );
    assert!(!con.is_halted());
}

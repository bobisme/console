//! `__gfx_meta__`: sprite/anim authoring metadata parsing.

use console_core::{Cart, Console, Error};

const DEMO: &str = include_str!("../../../carts/demo.cart");

/// FNV-1a, 64-bit. Inline so the test suite needs no dependencies (same as
/// the other integration test files).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

// ---------------------------------------------------------------------------
// Round-trip
// ---------------------------------------------------------------------------

const RT: &str = "\
__lua__
x = 1

__gfx_meta__
# a leading comment, then a blank line

sprite player rect=1,0 size=1x1 anchor=4,7
sprite star rect=2,0 size=2x3
anim player.walk frames=0,1,2,3 fps=12 loop
anim player.idle frames=0 fps=1
";

#[test]
fn sprites_and_anims_round_trip() {
    let cart = Cart::parse(RT).unwrap();
    let meta = cart.gfx_meta();
    assert!(!meta.is_empty());

    let player = meta.sprite("player").unwrap();
    assert_eq!(player.rect, (1, 0));
    assert_eq!(player.size, (1, 1));
    assert_eq!(player.anchor, (4, 7)); // explicit

    let star = meta.sprite("star").unwrap();
    assert_eq!(star.rect, (2, 0));
    assert_eq!(star.size, (2, 3));
    // Default anchor: bottom-center, i.e. (w*8/2, h*8-1).
    assert_eq!(star.anchor, (8, 23));

    assert!(meta.sprite("nope").is_none());

    let walk = meta.anim("player.walk").unwrap();
    assert_eq!(walk.sprite, "player");
    assert_eq!(walk.frames, vec![0, 1, 2, 3]);
    assert_eq!(walk.fps, 12);
    assert!(walk.looped);

    let idle = meta.anim("player.idle").unwrap();
    assert_eq!(idle.frames, vec![0]);
    assert_eq!(idle.fps, 1);
    assert!(!idle.looped); // `loop` omitted

    assert!(meta.anim("player.nope").is_none());

    // Iteration order: alphabetical by name (BTreeMap-backed).
    let sprite_names: Vec<&str> = meta.sprites().map(|s| s.name.as_str()).collect();
    assert_eq!(sprite_names, vec!["player", "star"]);
    let anim_names: Vec<&str> = meta.anims().map(|a| a.name.as_str()).collect();
    assert_eq!(anim_names, vec!["player.idle", "player.walk"]);
}

#[test]
fn crlf_is_tolerated() {
    let text = "__lua__\r\nx = 1\r\n\r\n__gfx_meta__\r\nsprite p rect=0,0 size=1x1\r\n";
    let cart = Cart::parse(text).unwrap();
    let p = cart.gfx_meta().sprite("p").unwrap();
    assert_eq!(p.rect, (0, 0));
    assert_eq!(p.size, (1, 1));
}

#[test]
fn absent_section_is_empty() {
    let cart = Cart::parse("__lua__\nx = 1\n").unwrap();
    assert!(cart.gfx_meta().is_empty());
    assert!(cart.gfx_meta().sprite("player").is_none());
    assert_eq!(cart.gfx_meta().sprites().count(), 0);
    assert_eq!(cart.gfx_meta().anims().count(), 0);
}

// ---------------------------------------------------------------------------
// frame_rect
// ---------------------------------------------------------------------------

#[test]
fn frame_rect_identity_displacement_wrap_and_off_sheet() {
    let cart = Cart::parse("__lua__\nx=1\n\n__gfx_meta__\nsprite p rect=1,0 size=1x1\n").unwrap();
    let p = cart.gfx_meta().sprite("p").unwrap();

    // i=0: identity, at the sprite's own tile.
    assert_eq!(p.frame_rect(0), Some((8, 0, 8, 8)));
    // Horizontal displacement: one sprite-width to the right.
    assert_eq!(p.frame_rect(1), Some((16, 0, 8, 8)));
    assert_eq!(p.frame_rect(3), Some((32, 0, 8, 8)));
    // Wrap to the next row band: tx=1 + 15*1 = 16 -> tx'=0, ty'=1.
    assert_eq!(p.frame_rect(15), Some((0, 8, 8, 8)));
    // Far enough to fall off the bottom of the 16x16 sheet.
    assert_eq!(p.frame_rect(255), None);
}

#[test]
fn frame_rect_with_multi_tile_sprite() {
    let cart = Cart::parse("__lua__\nx=1\n\n__gfx_meta__\nsprite p rect=0,0 size=2x2\n").unwrap();
    let p = cart.gfx_meta().sprite("p").unwrap();
    assert_eq!(p.frame_rect(0), Some((0, 0, 16, 16)));
    // i=1 displaces by w=2 tiles -> tx'=2.
    assert_eq!(p.frame_rect(1), Some((16, 0, 16, 16)));
    // i=7: tx=0+7*2=14, fits (14+2=16); i=8: tx=16 -> wraps to tx'=0, ty'=2.
    assert_eq!(p.frame_rect(7), Some((112, 0, 16, 16)));
    assert_eq!(p.frame_rect(8), Some((0, 16, 16, 16)));
}

#[test]
fn anim_off_sheet_frame_is_rejected_at_parse_time() {
    let text = "__lua__\nx=1\n\n__gfx_meta__\nsprite p rect=15,15 size=1x1\nanim p.bad frames=0,20 fps=10\n";
    let err = Cart::parse(text).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("line 2"), "{msg}");
    assert!(msg.contains("outside the 16x16 tile sheet"), "{msg}");
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn expect_cart_error(gfx_body: &str, line: usize, needle: &str) {
    let text = format!("__lua__\nx=1\n\n__gfx_meta__\n{gfx_body}");
    let err = Cart::parse(&text).unwrap_err();
    assert!(matches!(err, Error::Cart(_)), "{err:?}");
    let msg = err.to_string();
    assert!(
        msg.contains(&format!("line {line}")),
        "expected line {line} in: {msg}"
    );
    assert!(msg.contains(needle), "expected {needle:?} in: {msg}");
}

#[test]
fn bad_sprite_name_chars() {
    expect_cart_error("sprite Player rect=0,0 size=1x1\n", 1, "[a-z0-9_]+");
    expect_cart_error("sprite pl-ayer rect=0,0 size=1x1\n", 1, "[a-z0-9_]+");
}

#[test]
fn duplicate_sprite_name() {
    expect_cart_error(
        "sprite p rect=0,0 size=1x1\nsprite p rect=1,0 size=1x1\n",
        2,
        "duplicate sprite name",
    );
}

#[test]
fn duplicate_anim_name() {
    expect_cart_error(
        "sprite p rect=0,0 size=1x1\nanim p.walk frames=0 fps=10\nanim p.walk frames=1 fps=10\n",
        3,
        "duplicate anim name",
    );
}

#[test]
fn anim_references_unknown_sprite() {
    expect_cart_error(
        "anim ghost.walk frames=0 fps=10\n",
        1,
        "not defined in __gfx_meta__",
    );
}

#[test]
fn anim_forward_reference_to_a_sprite_is_fine() {
    // The anim comes before the sprite it names; validation happens after
    // the whole section parses.
    let text = "__lua__\nx=1\n\n__gfx_meta__\nanim p.walk frames=0,1 fps=10\nsprite p rect=0,0 size=1x1\n";
    let cart = Cart::parse(text).unwrap();
    assert_eq!(cart.gfx_meta().anim("p.walk").unwrap().sprite, "p");
}

#[test]
fn bad_rect_range() {
    expect_cart_error("sprite p rect=16,0 size=1x1\n", 1, "rect tx must be 0-15");
    expect_cart_error("sprite p rect=0,16 size=1x1\n", 1, "rect ty must be 0-15");
}

#[test]
fn bad_size_range() {
    expect_cart_error("sprite p rect=0,0 size=0x1\n", 1, "size w must be 1-16");
    expect_cart_error("sprite p rect=0,0 size=1x17\n", 1, "size h must be 1-16");
}

#[test]
fn bad_fps_range() {
    expect_cart_error(
        "sprite p rect=0,0 size=1x1\nanim p.walk frames=0 fps=0\n",
        2,
        "fps must be 1-60",
    );
    expect_cart_error(
        "sprite p rect=0,0 size=1x1\nanim p.walk frames=0 fps=61\n",
        2,
        "fps must be 1-60",
    );
}

#[test]
fn empty_frames_is_an_error() {
    expect_cart_error(
        "sprite p rect=0,0 size=1x1\nanim p.walk frames= fps=10\n",
        2,
        "frames must be nonempty",
    );
}

#[test]
fn missing_dot_in_anim_name() {
    expect_cart_error("anim playerwalk frames=0 fps=10\n", 1, "must be `<sprite>.<label>`");
}

#[test]
fn missing_required_keys() {
    expect_cart_error("sprite p rect=0,0\n", 1, "missing `size=");
    expect_cart_error("sprite p size=1x1\n", 1, "missing `rect=");
    expect_cart_error(
        "sprite p rect=0,0 size=1x1\nanim p.walk fps=10\n",
        2,
        "missing `frames=",
    );
    expect_cart_error(
        "sprite p rect=0,0 size=1x1\nanim p.walk frames=0\n",
        2,
        "missing `fps=",
    );
}

#[test]
fn unknown_line_kind_is_an_error() {
    expect_cart_error("nope 0 0\n", 1, "expected `sprite` or `anim`");
}

// ---------------------------------------------------------------------------
// Rendering must be unaffected
// ---------------------------------------------------------------------------

#[test]
fn presence_of_gfx_meta_does_not_affect_rendering() {
    let plain = "__lua__\nfunction _draw() cls(3) spr(0, 10, 10) end\n";
    let with_meta =
        format!("{plain}\n__gfx_meta__\nsprite s rect=0,0 size=1x1\nanim s.a frames=0,1 fps=10 loop\n");

    let mut a = Console::new(plain, 0).unwrap();
    let mut b = Console::new(&with_meta, 0).unwrap();
    for _ in 0..30 {
        a.step(0).unwrap();
        b.step(0).unwrap();
        assert_eq!(fnv1a(a.framebuffer()), fnv1a(b.framebuffer()));
    }
}

// ---------------------------------------------------------------------------
// Demo cart
// ---------------------------------------------------------------------------

#[test]
fn demo_cart_gfx_meta_has_the_declared_sprites() {
    let cart = Cart::parse(DEMO).unwrap();
    let meta = cart.gfx_meta();
    assert!(!meta.is_empty());

    let names: Vec<&str> = meta.sprites().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["gem", "player", "star"]);

    let player = meta.sprite("player").unwrap();
    assert_eq!(player.rect, (1, 0));
    assert_eq!(player.size, (1, 1));
    assert_eq!(player.anchor, (4, 7));

    assert_eq!(meta.sprite("star").unwrap().rect, (2, 0));
    assert_eq!(meta.sprite("gem").unwrap().rect, (3, 0));

    // The 2-frame walk cycle authored via the sprite tools (frame 0 = tile
    // (1,0), frame 3 offset = the stride pose copied to tile (4,0)).
    let walk = meta.anim("player.walk").expect("player.walk anim");
    assert_eq!(walk.frames, vec![0, 3]);
    assert_eq!(walk.fps, 6);
    assert!(walk.looped);
}

#[test]
fn demo_cart_rendering_is_unaffected_by_gfx_meta() {
    // Strip just the `__gfx_meta__` section back out and confirm the
    // framebuffer (and printh log) is byte-identical, frame by frame.
    let start = DEMO.find("__gfx_meta__").expect("demo cart has a __gfx_meta__ section");
    let end = DEMO.find("__sfx__").expect("demo cart has a __sfx__ section");
    let without = format!("{}{}", &DEMO[..start], &DEMO[end..]);
    assert!(!without.contains("__gfx_meta__"));
    assert!(Cart::parse(&without).unwrap().gfx_meta().is_empty());

    let mut a = Console::new(DEMO, 0).unwrap();
    let mut b = Console::new(&without, 0).unwrap();
    for i in 0..120 {
        let ra = a.step(if i == 20 { console_core::input::A } else { 0 });
        let rb = b.step(if i == 20 { console_core::input::A } else { 0 });
        ra.unwrap();
        rb.unwrap();
        assert_eq!(
            fnv1a(a.framebuffer()),
            fnv1a(b.framebuffer()),
            "gfx_meta section perturbed frame {i}"
        );
    }
    assert_eq!(a.take_logs(), b.take_logs());
}

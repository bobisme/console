//! `__gfx_meta__`: sprite/anim declaration parsing.

use console_core::{Cart, Console, Error, FrameSpec};

/// Build the classic all-index frame list `Vec<FrameSpec>` from plain
/// indices, for asserting against `AnimDef::frames` without new-syntax noise
/// at every call site.
fn indices(ints: &[u8]) -> Vec<FrameSpec> {
    ints.iter().map(|&i| FrameSpec::Index(i)).collect()
}

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
    assert_eq!(walk.frames, indices(&[0, 1, 2, 3]));
    assert_eq!(walk.fps, 12);
    assert!(walk.looped);

    let idle = meta.anim("player.idle").unwrap();
    assert_eq!(idle.frames, indices(&[0]));
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
    let text =
        "__lua__\nx=1\n\n__gfx_meta__\nanim p.walk frames=0,1 fps=10\nsprite p rect=0,0 size=1x1\n";
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
    expect_cart_error(
        "anim playerwalk frames=0 fps=10\n",
        1,
        "must be `<sprite>.<label>`",
    );
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
    let with_meta = format!(
        "{plain}\n__gfx_meta__\nsprite s rect=0,0 size=1x1\nanim s.a frames=0,1 fps=10 loop\n"
    );

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
    assert_eq!(names, vec!["gem", "moth", "player", "star"]);

    // Player: tiles 0-5 of band 0. Anchor is the feet, so the walk cycle's
    // stride reads as ground contact.
    let player = meta.sprite("player").unwrap();
    assert_eq!(player.rect, (1, 0));
    assert_eq!(player.size, (1, 1));
    assert_eq!(player.anchor, (4, 7));

    // Floating pickups anchor at their visual centre, not bottom-centre.
    let star = meta.sprite("star").unwrap();
    assert_eq!(star.rect, (8, 0));
    assert_eq!(star.size, (1, 1));
    assert_eq!(star.anchor, (4, 4));

    let gem = meta.sprite("gem").unwrap();
    assert_eq!(gem.rect, (12, 0));
    assert_eq!(gem.size, (1, 1));
    assert_eq!(gem.anchor, (4, 4));

    // The moth is a 2x2 megatile anchored on its thorax (centre of mass).
    let moth = meta.sprite("moth").unwrap();
    assert_eq!(moth.rect, (12, 2));
    assert_eq!(moth.size, (2, 2));
    assert_eq!(moth.anchor, (8, 8));
}

#[test]
fn demo_cart_gfx_meta_has_the_declared_anims() {
    let cart = Cart::parse(DEMO).unwrap();
    let meta = cart.gfx_meta();

    let names: Vec<&str> = meta.anims().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "gem.sparkle",
            "moth.flap",
            "player.idle",
            "player.walk",
            "star.twinkle",
        ]
    );

    // 4-frame walk: contact / passing / contact' / passing'.
    let walk = meta.anim("player.walk").expect("player.walk anim");
    assert_eq!(walk.sprite, "player");
    assert_eq!(walk.frames, indices(&[0, 1, 2, 3]));
    assert_eq!(walk.fps, 8);
    assert!(walk.looped);

    // 2-frame breathing idle, deliberately much slower than the walk.
    let idle = meta.anim("player.idle").expect("player.idle anim");
    assert_eq!(idle.frames, indices(&[4, 5]));
    assert_eq!(idle.fps, 2);
    assert!(idle.looped);
    assert!(idle.fps < walk.fps);

    let twinkle = meta.anim("star.twinkle").expect("star.twinkle anim");
    assert_eq!(twinkle.frames, indices(&[0, 1, 2, 3]));
    assert_eq!(twinkle.fps, 8);
    assert!(twinkle.looped);

    let sparkle = meta.anim("gem.sparkle").expect("gem.sparkle anim");
    assert_eq!(sparkle.frames, indices(&[0, 1]));
    assert_eq!(sparkle.fps, 4);
    assert!(sparkle.looped);

    let flap = meta.anim("moth.flap").expect("moth.flap anim");
    assert_eq!(flap.frames, indices(&[0, 1, 2, 3]));
    assert_eq!(flap.fps, 6);
    assert!(flap.looped);
}

/// The moth's frame offsets deliberately run off the right edge of its row
/// band so the wrap rule (`tx' = (tx + i*w) % 16`, `ty' += h`) is exercised by
/// a real cart, not just by unit tests.
#[test]
fn demo_cart_moth_frames_wrap_to_the_next_row_band() {
    let cart = Cart::parse(DEMO).unwrap();
    let moth = cart.gfx_meta().sprite("moth").unwrap();

    // Frames 0 and 1 sit in band ty=2 at tx=12 and tx=14 ...
    assert_eq!(moth.frame_rect(0), Some((96, 16, 16, 16)));
    assert_eq!(moth.frame_rect(1), Some((112, 16, 16, 16)));
    // ... and frames 2 and 3 wrap into band ty=4 at tx=0 and tx=2.
    assert_eq!(moth.frame_rect(2), Some((0, 32, 16, 16)));
    assert_eq!(moth.frame_rect(3), Some((16, 32, 16, 16)));
}

/// Every frame the Lua actually draws must be backed by non-empty pixels, and
/// the sprite ids in the cart's ANIM_* tables must line up with the tiles the
/// metadata resolves to. Guards the one thing `__gfx_meta__` cannot: that the
/// hand-duplicated Lua frame lists still point at the right art.
#[test]
fn demo_cart_anim_frames_have_pixels_at_the_expected_sprite_ids() {
    let cart = Cart::parse(DEMO).unwrap();
    let meta = cart.gfx_meta();
    let sheet = cart.sprites();

    let cases: [(&str, &[u16]); 5] = [
        ("player.walk", &[1, 2, 3, 4]),
        ("player.idle", &[5, 6]),
        ("star.twinkle", &[8, 9, 10, 11]),
        ("gem.sparkle", &[12, 13]),
        ("moth.flap", &[44, 46, 64, 66]),
    ];

    for (name, ids) in cases {
        let anim = meta.anim(name).unwrap_or_else(|| panic!("{name} missing"));
        let sprite = meta.sprite(&anim.sprite).unwrap();
        assert_eq!(anim.frames.len(), ids.len(), "{name} frame count");

        for (pos, &id) in ids.iter().enumerate() {
            // Goes through `AnimDef::resolve_frame` (the single source of
            // truth for anim frame resolution), not `SpriteDef::frame_rect`
            // directly, since `anim.frames` is no longer bare `u8` indices.
            let (x, y, w, h) = anim
                .resolve_frame(sprite, pos)
                .unwrap_or_else(|| panic!("{name} frame {pos} off-sheet"));

            // Sprite id n lives at (n % 16 * 8, n / 16 * 8) -- the address the
            // Lua passes to spr().
            assert_eq!(
                (x, y),
                (u32::from(id) % 16 * 8, u32::from(id) / 16 * 8),
                "{name} frame {pos} should be sprite id {id}"
            );

            let opaque = (0..h)
                .flat_map(|dy| (0..w).map(move |dx| (dx, dy)))
                .filter(|&(dx, dy)| sheet[((y + dy) * 128 + (x + dx)) as usize] != 0)
                .count();
            assert!(opaque > 0, "{name} frame {pos} is blank");
        }
    }
}

/// `__gfx_meta__` used to be inert, and this test used to assert that stripping
/// it changed nothing. Since `aspr` it is load-bearing for any cart that plays
/// a declared anim — so what has to hold instead is that losing the section
/// fails *loudly*, naming the anim, rather than quietly drawing an empty sky.
#[test]
fn demo_cart_without_gfx_meta_halts_naming_the_missing_anim() {
    // Match the *headers*, not the first occurrence of the name: the cart's
    // Lua comments legitimately mention `__gfx_meta__`, and a bare `find` would
    // splice the cart apart mid-source.
    let header = |name: &str| {
        let pat = format!("\n{name}\n");
        DEMO.find(&pat)
            .map(|i| i + 1)
            .unwrap_or_else(|| panic!("demo cart has a {name} section header"))
    };
    let start = header("__gfx_meta__");
    let end = header("__sfx__");
    assert!(start < end, "__gfx_meta__ must precede __sfx__");
    let without = format!("{}{}", &DEMO[..start], &DEMO[end..]);
    assert!(!without.contains("\n__gfx_meta__\n"));
    assert!(Cart::parse(&without).unwrap().gfx_meta().is_empty());

    // The cart still loads (nothing draws until `_draw`), then halts on the
    // first `aspr`.
    let mut con = Console::new(&without, 0).unwrap();
    let err = con.step(0).unwrap_err().to_string();
    assert!(err.contains("aspr"), "{err}");
    assert!(err.contains("moth.flap"), "{err}");
    assert!(err.contains("no __gfx_meta__ section"), "{err}");
}

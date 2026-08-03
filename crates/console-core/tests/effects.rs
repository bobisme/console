//! Signature effects: `fillp` dither patterns, `sspr` scaled blits, `mosaic`
//! and `rshift`.
//!
//! The contract these tests pin down:
//!
//! * a cart that calls none of them draws bit-identically to before they
//!   existed (see [`the_new_effects_are_off_by_default`]);
//! * `fillp` is a 4x4 grid anchored to **screen** space, MSB = top-left, and it
//!   applies to the shape primitives only — never to `spr`, `map`, `print` or
//!   `cls`;
//! * `sspr` at 1:1 is byte-identical to `spr`, and every scale is
//!   nearest-neighbour integer stepping;
//! * `mosaic` and `rshift` are **framebuffer** effects: they are in the goldens
//!   and in `screen_text`, unlike the scanout-only display palette;
//! * the end-of-frame order is `mosaic` then `rshift`, and `rshift` wraps each
//!   scanline around the 144-pixel line rather than clipping it.

use console_core::{Console, FB_LEN, MAX_MOSAIC, SCREEN_H, SCREEN_W};

/// Sprite 0 is an 8x8 gradient with no colour 0 in it; sprite 1 is the 2x2
/// marker (colours 1..4) plus a lone 5 at (7, 7) used by the other test files.
const SHEET: &str = "\
1234567812000000
2345678934000000
3456789a00000000
456789ab00000000
56789abc00000000
6789abcd00000000
789abcde00000000
89abcdef00000005
";

fn cart(draw: &str) -> String {
    format!("__lua__\nfunction _draw()\n{draw}\nend\n\n__sprites__\n{SHEET}")
}

/// Run one frame of a cart whose whole job is to draw.
fn run(draw: &str) -> Console {
    let mut con = Console::new(&cart(draw), 0).expect("cart should load");
    con.step(0).expect("frame should run");
    con
}

fn px(con: &Console, x: usize, y: usize) -> u8 {
    con.framebuffer()[y * SCREEN_W + x]
}

fn count(con: &Console, c: u8) -> usize {
    con.framebuffer().iter().filter(|&&p| p == c).count()
}

/// The framebuffer as `screen_text` would render it: one hex digit per pixel.
fn screen_text(con: &Console) -> Vec<String> {
    con.framebuffer()
        .chunks(SCREEN_W)
        .map(|row| row.iter().map(|p| format!("{p:x}")).collect())
        .collect()
}

// ---------------------------------------------------------------------------
// backward compatibility
// ---------------------------------------------------------------------------

#[test]
fn the_new_effects_are_off_by_default() {
    let ds = run("cls(0)").draw_state();
    assert_eq!(ds.fillp(), 0, "the default fill is solid");
    assert_eq!(ds.mosaic(), 1, "mosaic is off");
    assert!(!ds.rshift_active(), "no scanline is shifted");
    assert!((0..SCREEN_H as i32).all(|y| ds.rshift(y) == 0));
}

#[test]
fn explicit_defaults_are_identical_to_untouched_state() {
    let scene = "cls(1) rectfill(10, 10, 30, 30, 7) circfill(70, 70, 12, 4)
                 print(\"HI\", 4, 4, 11) line(0, 0, 143, 255, 3) spr(0, 20, 90)";
    let plain = run(scene);
    let reset = run(&format!("fillp() mosaic() rshift()\n{scene}"));
    assert_eq!(plain.framebuffer(), reset.framebuffer());
    let reset = run(&format!("fillp(0) mosaic(1) rshift(0, 0)\n{scene}"));
    assert_eq!(plain.framebuffer(), reset.framebuffer());
    let reset = run(&format!(
        "fillp(0) mosaic(1) for y = 0, 255 do rshift(y, 0) end\n{scene}"
    ));
    assert_eq!(plain.framebuffer(), reset.framebuffer());
}

#[test]
fn a_solid_fill_ignores_the_secondary_nibble() {
    // Shape colours are now `c0 + c1 * 16`. Without a pattern the high nibble
    // is never consulted, so every old cart that passed a big number keeps the
    // colour it always got.
    let plain = run("cls(0) rectfill(0, 0, 9, 9, 7)");
    for c in ["7 + 2 * 16", "0x27", "7 + 15 * 16", "-9"] {
        let wide = run(&format!("cls(0) rectfill(0, 0, 9, 9, {c})"));
        assert_eq!(
            plain.framebuffer(),
            wide.framebuffer(),
            "colour `{c}` should still draw as 7"
        );
    }
}

// ---------------------------------------------------------------------------
// fillp: pattern layout
// ---------------------------------------------------------------------------

#[test]
fn fillp_checkerboard_is_pinned_pixel_by_pixel() {
    // 0x5a5a = 0101 1010 0101 1010: a 4x4 checker, primary where (x + y) is
    // even. A set bit with no secondary colour draws nothing at all.
    let con = run("cls(9) fillp(0x5a5a) rectfill(0, 0, 7, 7, 7)");
    for y in 0..8 {
        for x in 0..8 {
            let want = if (x + y) % 2 == 0 { 7 } else { 9 };
            assert_eq!(px(&con, x, y), want, "at ({x}, {y})");
        }
    }
    assert_eq!(count(&con, 7), 32);
}

#[test]
fn fillp_bit_order_is_msb_top_left_row_major() {
    // One bit at a time pins the orientation: 0x8000 is the top-left cell,
    // 0x4000 the one to its right, 0x0800 the start of the second row and
    // 0x0001 the bottom-right.
    for (pattern, hole) in [
        (0x8000, (0, 0)),
        (0x4000, (1, 0)),
        (0x1000, (3, 0)),
        (0x0800, (0, 1)),
        (0x0080, (0, 2)),
        (0x0008, (0, 3)),
        (0x0001, (3, 3)),
    ] {
        let con = run(&format!("cls(9) fillp({pattern}) rectfill(0, 0, 3, 3, 7)"));
        assert_eq!(
            count(&con, 7),
            15,
            "pattern {pattern:#06x} punches one hole"
        );
        assert_eq!(
            px(&con, hole.0, hole.1),
            9,
            "pattern {pattern:#06x} should clear ({}, {})",
            hole.0,
            hole.1
        );
    }
}

#[test]
fn fillp_tiles_the_whole_shape() {
    // 0x8000 on a 12x12 rect: a hole every four pixels in both directions.
    let con = run("cls(9) fillp(0x8000) rectfill(0, 0, 11, 11, 7)");
    assert_eq!(count(&con, 9), FB_LEN - 144 + 9);
    for by in 0..3 {
        for bx in 0..3 {
            assert_eq!(px(&con, bx * 4, by * 4), 9);
            assert_eq!(px(&con, bx * 4 + 1, by * 4), 7);
        }
    }
}

#[test]
fn fillp_secondary_colour_comes_from_the_high_nibble() {
    // c0 + c1 * 16: set bits draw c1 instead of punching a hole.
    let con = run("cls(9) fillp(0x8000) rectfill(0, 0, 3, 3, 7 + 2 * 16)");
    assert_eq!(px(&con, 0, 0), 2, "the set bit drew the secondary colour");
    assert_eq!(px(&con, 1, 0), 7);
    assert_eq!(count(&con, 2), 1);
    assert_eq!(count(&con, 7), 15);
    assert_eq!(count(&con, 9), FB_LEN - 16);

    // A two-colour checker covers every pixel it touches.
    let con = run("cls(9) fillp(0x5a5a) rectfill(0, 0, 7, 7, 3 + 12 * 16)");
    assert_eq!(count(&con, 3), 32);
    assert_eq!(count(&con, 12), 32);
    assert_eq!(count(&con, 9), FB_LEN - 64);
    assert_eq!(px(&con, 0, 0), 3);
    assert_eq!(px(&con, 1, 0), 12);
}

#[test]
fn the_draw_palette_remaps_both_fill_colours() {
    let con = run("cls(9) pal(7, 5) pal(2, 11) fillp(0x5a5a) rectfill(0, 0, 3, 3, 7 + 2 * 16)");
    assert_eq!(px(&con, 0, 0), 5);
    assert_eq!(px(&con, 1, 0), 11);
    assert_eq!(count(&con, 7) + count(&con, 2), 0);
}

// ---------------------------------------------------------------------------
// fillp: which primitives, and screen anchoring
// ---------------------------------------------------------------------------

#[test]
fn fillp_applies_to_every_shape_primitive() {
    // 0xffff: every bit set, no secondary colour => the shape vanishes.
    for shape in [
        "pset(2, 2, 7)",
        "line(0, 0, 40, 40, 7)",
        "rect(0, 0, 20, 20, 7)",
        "rectfill(0, 0, 20, 20, 7)",
        "circ(30, 30, 10, 7)",
        "circfill(30, 30, 10, 7)",
    ] {
        let con = run(&format!("cls(9) fillp(0xffff) {shape}"));
        assert_eq!(count(&con, 9), FB_LEN, "{shape} ignored the fill pattern");
    }

    // ...and a half pattern halves each of them.
    for shape in ["rectfill(0, 0, 7, 7, 7)", "line(0, 0, 0, 15, 7)"] {
        let solid = run(&format!("cls(9) {shape}"));
        let dithered = run(&format!("cls(9) fillp(0x5a5a) {shape}"));
        assert!(count(&dithered, 7) > 0);
        assert!(
            count(&dithered, 7) < count(&solid, 7),
            "{shape} should lose pixels to the pattern"
        );
    }
}

#[test]
fn fillp_never_touches_sprites_text_or_cls() {
    let scene = "cls(9) spr(0, 0, 0) spr(1, 40, 0) print(\"HI\", 0, 40, 11)";
    let plain = run(scene);
    let dithered = run(&format!("fillp(0xa5a5) {scene}"));
    assert_eq!(
        plain.framebuffer(),
        dithered.framebuffer(),
        "fillp is a shape effect only"
    );
    assert_eq!(count(&dithered, 9), count(&plain, 9));

    // cls fills the screen whatever the pattern says.
    let con = run("fillp(0xffff) cls(9)");
    assert_eq!(count(&con, 9), FB_LEN);

    // sspr is the sprite path too.
    let plain = run("cls(9) sspr(0, 0, 8, 8, 0, 0, 16, 16)");
    let dithered = run("cls(9) fillp(0xffff) sspr(0, 0, 8, 8, 0, 0, 16, 16)");
    assert_eq!(plain.framebuffer(), dithered.framebuffer());
}

#[test]
fn the_pattern_grid_is_anchored_to_the_screen_not_the_shape() {
    // Moving the shape by one pixel moves it *through* the pattern: the hole
    // stays at screen x%4 == 0, y%4 == 0.
    let con = run("cls(9) fillp(0x8000) rectfill(1, 1, 4, 4, 7)");
    assert_eq!(px(&con, 4, 4), 9, "the hole is at a screen multiple of 4");
    assert_eq!(px(&con, 1, 1), 7);
    assert_eq!(count(&con, 7), 15);

    // The camera is applied first, so the pattern shimmers as the world
    // scrolls: the same world rect under two cameras dithers differently.
    let a = run("cls(9) camera(0, 0) fillp(0x8000) rectfill(4, 4, 7, 7, 7)");
    let b = run("cls(9) camera(1, 0) fillp(0x8000) rectfill(4, 4, 7, 7, 7)");
    assert_eq!(px(&a, 4, 4), 9);
    assert_eq!(px(&b, 3, 4), 7, "the shape moved but the grid did not");
    assert_eq!(px(&b, 4, 4), 9);

    // A camera that shifts the shape by a whole grid cell reproduces the
    // unshifted screen exactly.
    let plain = run("cls(9) fillp(0x1248) rectfill(0, 0, 15, 15, 7)");
    let shifted = run("cls(9) camera(4, 4) fillp(0x1248) rectfill(4, 4, 19, 19, 7)");
    assert_eq!(plain.framebuffer(), shifted.framebuffer());
}

#[test]
fn the_clip_rect_bounds_a_dithered_shape() {
    let con = run("cls(9) clip(0, 0, 4, 4) fillp(0x5a5a) rectfill(0, 0, 143, 255, 7)");
    assert_eq!(count(&con, 7), 8, "half of a 4x4 window");
    assert_eq!(px(&con, 0, 0), 7);
    assert_eq!(px(&con, 4, 0), 9);
}

// ---------------------------------------------------------------------------
// fillp: state
// ---------------------------------------------------------------------------

#[test]
fn fillp_with_no_args_or_zero_is_solid_again() {
    let plain = run("cls(9) rectfill(0, 0, 9, 9, 7)");
    for reset in ["fillp()", "fillp(0)"] {
        let con = run(&format!(
            "cls(9) fillp(0x5a5a) {reset} rectfill(0, 0, 9, 9, 7)"
        ));
        assert_eq!(plain.framebuffer(), con.framebuffer(), "{reset}");
    }
}

#[test]
fn pal_does_not_reset_fillp() {
    // PICO-8 keeps the two apart: `pal()` resets both palette maps and `palt`,
    // and leaves the fill pattern exactly where it was.
    let con = run("cls(9) fillp(0x5a5a) pal() palt() rectfill(0, 0, 7, 7, 7)");
    assert_eq!(con.draw_state().fillp(), 0x5a5a);
    assert_eq!(count(&con, 7), 32);
}

#[test]
fn fillp_persists_across_frames() {
    let mut con = Console::new(
        &cart("rectfill(0, 0, 7, 7, 7)").replace(
            "function _draw()",
            "function _init() fillp(0x5a5a) end\nfunction _draw()\ncls(9)",
        ),
        0,
    )
    .unwrap();
    for _ in 0..5 {
        con.step(0).unwrap();
    }
    assert_eq!(con.draw_state().fillp(), 0x5a5a);
    assert_eq!(count(&con, 7), 32, "nothing resets it at a frame boundary");
}

#[test]
fn fillp_masks_its_argument_to_sixteen_bits() {
    let con = run("cls(9) fillp(0x15a5a) rectfill(0, 0, 3, 3, 7)");
    assert_eq!(con.draw_state().fillp(), 0x5a5a);
    let con = run("cls(9) fillp(-1) rectfill(0, 0, 3, 3, 7)");
    assert_eq!(con.draw_state().fillp(), 0xffff);
    assert_eq!(count(&con, 7), 0);
}

// ---------------------------------------------------------------------------
// sspr: 1:1 is spr
// ---------------------------------------------------------------------------

#[test]
fn sspr_at_one_to_one_is_byte_identical_to_spr() {
    // The whole point of sharing the sprite rules: for any draw state and any
    // flip, an unscaled sspr and the equivalent spr agree exactly.
    for state in [
        "",
        "camera(5, 9)",
        "camera(-3, -7)",
        "clip(4, 4, 9, 9)",
        "clip(0, 0, 0, 0)",
        "pal(1, 11) pal(3, 14)",
        "palt(2, true)",
        "palt(0, false)",
        "camera(6, 6) clip(2, 2, 20, 20) pal(2, 12) palt(1, true)",
    ] {
        for (fx, fy) in [
            ("false", "false"),
            ("true", "false"),
            ("false", "true"),
            ("true", "true"),
        ] {
            for (n, sx) in [(0, 0), (1, 8)] {
                let via_sspr = run(&format!(
                    "cls(9) {state} sspr({sx}, 0, 8, 8, 20, 30, 8, 8, {fx}, {fy})"
                ));
                let via_spr = run(&format!(
                    "cls(9) {state} spr({n}, 20, 30, 1, 1, {fx}, {fy})"
                ));
                assert_eq!(
                    via_sspr.framebuffer(),
                    via_spr.framebuffer(),
                    "sspr diverged from spr under `{state}` flip=({fx}, {fy}) sprite {n}"
                );
            }
        }
    }
}

#[test]
fn the_destination_size_defaults_to_the_source_size() {
    let explicit = run("cls(9) sspr(0, 0, 8, 8, 10, 10, 8, 8)");
    let implied = run("cls(9) sspr(0, 0, 8, 8, 10, 10)");
    assert_eq!(explicit.framebuffer(), implied.framebuffer());

    // One dimension given, the other defaulted.
    let con = run("cls(9) sspr(0, 0, 4, 4, 0, 0, 8)");
    assert_eq!(px(&con, 7, 3), px(&con, 6, 3), "x doubled, y did not");
    assert_eq!(px(&con, 0, 4), 9, "the destination is still 4 tall");
}

// ---------------------------------------------------------------------------
// sspr: scaling
// ---------------------------------------------------------------------------

#[test]
fn sspr_doubles_each_source_pixel_into_a_two_by_two_block() {
    // Source 2x2 = [1 2 / 2 3] (top-left of the gradient sprite).
    let con = run("cls(9) sspr(0, 0, 2, 2, 0, 0, 4, 4)");
    let want = [[1, 1, 2, 2], [1, 1, 2, 2], [2, 2, 3, 3], [2, 2, 3, 3]];
    for (y, row) in want.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            assert_eq!(px(&con, x, y), c, "at ({x}, {y})");
        }
    }
    assert_eq!(count(&con, 9), FB_LEN - 16);
}

#[test]
fn sspr_halving_samples_every_other_source_pixel() {
    // Source 4x4 at (0,0), destination 2x2: the step is exactly 2, so the
    // samples are the source pixels at (0,0), (2,0), (0,2), (2,2).
    let con = run("cls(9) sspr(0, 0, 4, 4, 0, 0, 2, 2)");
    assert_eq!(px(&con, 0, 0), 1);
    assert_eq!(px(&con, 1, 0), 3);
    assert_eq!(px(&con, 0, 1), 3);
    assert_eq!(px(&con, 1, 1), 5);
    assert_eq!(count(&con, 9), FB_LEN - 4);
}

#[test]
fn a_fractional_scale_steps_in_fixed_point() {
    // 3 source pixels into 2 destination pixels: step = 1.5, so the samples are
    // source column 0 and column 1 (1.5 floored), not 0 and 2.
    let con = run("cls(9) sspr(0, 0, 3, 3, 0, 0, 2, 2)");
    assert_eq!(px(&con, 0, 0), 1);
    assert_eq!(px(&con, 1, 0), 2);
    assert_eq!(px(&con, 0, 1), 2);
    assert_eq!(px(&con, 1, 1), 3);

    // A big non-integer stretch stays monotone and never samples off the rect.
    let con = run("cls(9) sspr(0, 0, 3, 1, 0, 0, 7, 1)");
    let row: Vec<u8> = (0..7).map(|x| px(&con, x, 0)).collect();
    assert_eq!(row, vec![1, 1, 1, 2, 2, 3, 3]);
}

#[test]
fn sspr_flips_the_scaled_result() {
    let con = run("cls(9) sspr(0, 0, 4, 1, 0, 0, 8, 1, true)");
    let row: Vec<u8> = (0..8).map(|x| px(&con, x, 0)).collect();
    assert_eq!(row, vec![4, 4, 3, 3, 2, 2, 1, 1]);

    let con = run("cls(9) sspr(0, 0, 1, 4, 0, 0, 1, 8, false, true)");
    let colm: Vec<u8> = (0..8).map(|y| px(&con, 0, y)).collect();
    assert_eq!(colm, vec![4, 4, 3, 3, 2, 2, 1, 1]);
}

#[test]
fn sspr_takes_a_rectangle_from_anywhere_on_the_sheet() {
    // A 2x2 window at (3, 1): source rows 1 and 2 are "2345678" and "3456789".
    let con = run("cls(9) sspr(3, 1, 2, 2, 0, 0, 2, 2)");
    assert_eq!(px(&con, 0, 0), 5);
    assert_eq!(px(&con, 1, 0), 6);
    assert_eq!(px(&con, 0, 1), 6);
    assert_eq!(px(&con, 1, 1), 7);

    // Source pixels off the 128x128 sheet are skipped, not wrapped.
    let con = run("cls(9) sspr(126, 126, 8, 8, 0, 0, 8, 8)");
    assert_eq!(count(&con, 9), FB_LEN, "off-sheet source draws nothing");
}

// ---------------------------------------------------------------------------
// sspr: draw state and degenerate input
// ---------------------------------------------------------------------------

#[test]
fn sspr_respects_camera_clip_palt_and_pal_when_scaled() {
    // camera
    let con = run("cls(9) camera(10, 20) sspr(0, 0, 2, 2, 10, 20, 4, 4)");
    assert_eq!(px(&con, 0, 0), 1);
    assert_eq!(px(&con, 3, 3), 3);
    assert_eq!(count(&con, 9), FB_LEN - 16);

    // clip, in screen space after the camera
    let con = run("cls(9) clip(0, 0, 2, 2) sspr(0, 0, 2, 2, 0, 0, 4, 4)");
    assert_eq!(count(&con, 1), 4);
    assert_eq!(count(&con, 3), 0);

    // palt on the SOURCE colour, before the draw palette
    let con = run("cls(9) sspr(8, 0, 8, 8, 0, 0, 16, 16)");
    assert_eq!(px(&con, 0, 0), 1);
    assert_eq!(px(&con, 4, 0), 9, "sprite 1's colour 0 is transparent");
    let con = run("cls(9) palt(1, true) sspr(8, 0, 8, 8, 0, 0, 16, 16)");
    assert_eq!(px(&con, 0, 0), 9);
    let con = run("cls(9) pal(1, 0) sspr(8, 0, 8, 8, 0, 0, 16, 16)");
    assert_eq!(px(&con, 0, 0), 0, "remapping to 0 does not make it vanish");

    // pal on the destination pixels
    let con = run("cls(9) pal(1, 11) sspr(0, 0, 2, 2, 0, 0, 4, 4)");
    assert_eq!(px(&con, 0, 0), 11);
    assert_eq!(count(&con, 1), 0);
}

#[test]
fn degenerate_rectangles_draw_nothing() {
    for spec in [
        "sspr(0, 0, 0, 8, 0, 0, 8, 8)",
        "sspr(0, 0, 8, 0, 0, 0, 8, 8)",
        "sspr(0, 0, 8, 8, 0, 0, 0, 8)",
        "sspr(0, 0, 8, 8, 0, 0, 8, 0)",
        "sspr(0, 0, -8, -8, 0, 0, 8, 8)",
        "sspr(0, 0, 8, 8, 0, 0, -8, -8)",
        "sspr(0, 0, 8, 8, 0, 0, -8, 8)",
        "sspr(0, 0, 8, 8, 200, 400, 8, 8)",
        "sspr(0, 0, 8, 8, -20, -20, 8, 8)",
    ] {
        let con = run(&format!("cls(9) {spec}"));
        assert_eq!(count(&con, 9), FB_LEN, "{spec} should draw nothing");
    }

    // A negative destination size does NOT mirror — that is what the flip flags
    // are for.
    let mirrored = run("cls(9) sspr(0, 0, 4, 4, 10, 10, 4, 4, true)");
    let negative = run("cls(9) sspr(0, 0, 4, 4, 10, 10, -4, 4)");
    assert_ne!(mirrored.framebuffer(), negative.framebuffer());
    assert_eq!(count(&negative, 9), FB_LEN);
}

#[test]
fn a_huge_stretch_stays_on_screen_without_panicking() {
    let con = run("cls(9) sspr(0, 0, 1, 1, -1000, -1000, 4000, 4000)");
    assert_eq!(count(&con, 1), FB_LEN, "one source pixel fills the screen");

    let con = run("cls(9) sspr(0, 0, 128, 128, 0, 0, 1, 1)");
    assert_eq!(count(&con, 9), FB_LEN - 1);
}

// ---------------------------------------------------------------------------
// mosaic
// ---------------------------------------------------------------------------

#[test]
fn mosaic_replaces_each_block_with_its_top_left_pixel() {
    // A single lit pixel at a block origin floods its whole 4x4 block...
    let con = run("cls(9) pset(0, 0, 7) mosaic(4)");
    assert_eq!(count(&con, 7), 16);
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(px(&con, x, y), 7);
        }
    }
    assert_eq!(px(&con, 4, 0), 9);

    // ...and a lit pixel anywhere else in the block disappears, because the
    // top-left pixel wins (indexed colour cannot be averaged).
    let con = run("cls(9) pset(1, 1, 7) mosaic(4)");
    assert_eq!(count(&con, 7), 0);
    assert_eq!(count(&con, 9), FB_LEN);
}

#[test]
fn mosaic_blocks_are_anchored_to_the_screen_origin() {
    let con = run("cls(9) rectfill(0, 0, 143, 255, 9) pset(8, 8, 3) pset(9, 12, 4) mosaic(8)");
    // (8, 8) is a block origin: its 8x8 block is all colour 3.
    assert_eq!(count(&con, 3), 64);
    assert_eq!(px(&con, 15, 15), 3);
    assert_eq!(px(&con, 16, 8), 9);
    // (9, 12) is interior to the same block and is simply lost.
    assert_eq!(count(&con, 4), 0);
}

#[test]
fn a_factor_that_does_not_divide_the_screen_leaves_narrow_edge_blocks() {
    // 144 = 28 * 5 + 4 and 256 = 51 * 5 + 1, so the right edge is 4 wide and
    // the bottom edge one row tall.
    let con = run("cls(9) pset(140, 0, 3) pset(0, 255, 4) mosaic(5)");
    assert_eq!(count(&con, 3), 4 * 5, "the edge block is 4 wide, 5 tall");
    assert_eq!(px(&con, 143, 4), 3);
    assert_eq!(px(&con, 139, 0), 9);
    assert_eq!(count(&con, 4), 5, "the bottom edge block is one row tall");
    assert_eq!(px(&con, 4, 255), 4);
}

#[test]
fn mosaic_is_in_the_framebuffer_so_screen_text_shows_it() {
    // The contrast with the display palette: that is a scanout map and leaves
    // the framebuffer alone, while mosaic really rewrites the pixels.
    let con = run("cls(9) rectfill(0, 0, 3, 3, 7) mosaic(8)");
    let text = screen_text(&con);
    assert_eq!(text.len(), SCREEN_H);
    assert_eq!(&text[0][..9], "777777779");
    assert_eq!(&text[7][..9], "777777779");
    assert_eq!(&text[8][..9], "999999999");
}

#[test]
fn mosaic_one_and_bare_mosaic_are_no_ops() {
    let plain = run("cls(9) circfill(70, 70, 30, 7) print(\"HI\", 3, 3, 11)");
    for spec in ["mosaic(1)", "mosaic()", "mosaic(0)", "mosaic(-4)"] {
        let con = run(&format!(
            "cls(9) circfill(70, 70, 30, 7) print(\"HI\", 3, 3, 11) {spec}"
        ));
        assert_eq!(plain.framebuffer(), con.framebuffer(), "{spec}");
        assert_eq!(con.draw_state().mosaic(), 1);
    }
}

#[test]
fn the_mosaic_factor_is_clamped() {
    let con = run("cls(9) mosaic(1000)");
    assert_eq!(con.draw_state().mosaic(), MAX_MOSAIC);
    // Still a sane screen: 144x256 in 32-pixel blocks.
    let con = run("cls(9) pset(0, 0, 7) mosaic(1000)");
    assert_eq!(count(&con, 7), (MAX_MOSAIC as usize).pow(2));
}

#[test]
fn mosaic_persists_across_frames_and_does_not_feed_back() {
    let mut con = Console::new(
        &cart("cls(9)\npset(1, 1, 7)\nraw = pget(1, 1)\nblock = pget(0, 0)"),
        0,
    )
    .unwrap();
    con.eval("mosaic(4)").unwrap();
    for _ in 0..5 {
        con.step(0).unwrap();
    }
    assert_eq!(con.draw_state().mosaic(), 4, "nothing resets it");
    // The cart still draws — and reads — at full resolution...
    assert_eq!(con.get_global("raw").unwrap().as_i64(), Some(7));
    assert_eq!(con.get_global("block").unwrap().as_i64(), Some(9));
    // ...and only the presented frame is pixelated. (0, 0) is the block's
    // top-left source pixel, so the whole block takes the clear colour and the
    // lone lit pixel is gone.
    assert_eq!(count(&con, 7), 0);
    assert_eq!(count(&con, 9), FB_LEN);

    // Five frames of an un-cleared, mosaicked screen do not compound: the
    // effect is recomputed from the pristine draw buffer every frame.
    let mut a = Console::new(&cart("pset(3, 3, 7) mosaic(4)"), 0).unwrap();
    a.step(0).unwrap();
    let first = *a.framebuffer();
    for _ in 0..9 {
        a.step(0).unwrap();
    }
    assert_eq!(&first, a.framebuffer());
}

#[test]
fn mosaic_composes_with_the_rest_of_the_draw_state() {
    // The display palette stays scanout-only even under mosaic.
    let con = run("cls(9) rectfill(0, 0, 7, 7, 7) pal(7, 2, 1) mosaic(8)");
    assert_eq!(count(&con, 7), 64);
    assert_eq!(count(&con, 2), 0);
    assert_eq!(con.display_palette()[7], 2);

    // The clip rect bounds the drawing, not the mosaic: blocks are always the
    // whole screen.
    let con = run("cls(9) clip(0, 0, 4, 4) cls(3) clip() mosaic(8)");
    assert_eq!(count(&con, 3), 64, "the 4x4 clear grew to its 8x8 block");
}

// ---------------------------------------------------------------------------
// rshift (per-scanline raster displacement)
// ---------------------------------------------------------------------------

#[test]
fn a_positive_shift_moves_the_line_right_pixel_by_pixel() {
    // Asymmetric content, so the direction cannot be read either way round:
    // colour 7 sits immediately left of colour 3.
    let con = run("cls(9) pset(0, 5, 7) pset(1, 5, 3) rshift(5, 2)");
    assert_eq!(px(&con, 0, 5), 9);
    assert_eq!(px(&con, 1, 5), 9);
    assert_eq!(px(&con, 2, 5), 7, "positive dx moves content RIGHT");
    assert_eq!(px(&con, 3, 5), 3, "and keeps the two in order");
    assert_eq!(px(&con, 4, 5), 9);
    // Only that one scanline moved.
    assert_eq!(count(&con, 7), 1);
    assert_eq!(count(&con, 3), 1);
    for y in [4, 6] {
        assert!(
            (0..SCREEN_W).all(|x| px(&con, x, y) == 9),
            "row {y} is clean"
        );
    }
    assert_eq!(con.draw_state().rshift(5), 2);
    assert!(con.draw_state().rshift_active());
}

#[test]
fn shifts_wrap_around_the_line_in_both_directions() {
    // Right off the edge and back in at x = 0.
    let con = run("cls(9) pset(0, 5, 7) pset(143, 5, 3) rshift(5, 1)");
    assert_eq!(px(&con, 1, 5), 7);
    assert_eq!(px(&con, 0, 5), 3, "the right edge wrapped to the left one");

    // Negative dx moves left, and the left edge wraps to the right one.
    let con = run("cls(9) pset(0, 5, 7) pset(1, 5, 3) rshift(5, -1)");
    assert_eq!(px(&con, 143, 5), 7);
    assert_eq!(px(&con, 0, 5), 3);
}

#[test]
fn a_shift_is_reduced_modulo_the_screen_width() {
    // dx, dx + 144 and dx - 144 are the same shift, so a sweep never has to
    // clamp: -142 == 2 == 146 == 1010.
    let two = run("cls(9) pset(0, 5, 7) pset(1, 5, 3) rshift(5, 2)");
    for dx in ["146", "-142", "2 + 144 * 7", "2 - 144 * 7"] {
        let con = run(&format!(
            "cls(9) pset(0, 5, 7) pset(1, 5, 3) rshift(5, {dx})"
        ));
        assert_eq!(two.framebuffer(), con.framebuffer(), "rshift(5, {dx})");
        assert_eq!(con.draw_state().rshift(5), 2, "stored as dx mod 144");
    }
    // A whole-screen-width shift is the identity, not a no-op call: it is
    // stored as 0 and the pass is skipped.
    let con = run("cls(9) pset(0, 5, 7) rshift(5, 144)");
    assert_eq!(px(&con, 0, 5), 7);
    assert_eq!(con.draw_state().rshift(5), 0);
    assert!(!con.draw_state().rshift_active());
}

#[test]
fn a_bare_rshift_clears_every_line_and_one_argument_clears_one() {
    let plain = run("cls(9) rectfill(0, 0, 40, 40, 7) circfill(70, 70, 12, 3)");
    let scene = "cls(9) rectfill(0, 0, 40, 40, 7) circfill(70, 70, 12, 3)";

    let swept = run(&format!("for y = 0, 255 do rshift(y, y % 17) end {scene}"));
    assert_ne!(plain.framebuffer(), swept.framebuffer(), "the sweep shows");

    let cleared = run(&format!(
        "for y = 0, 255 do rshift(y, y % 17) end rshift() {scene}"
    ));
    assert_eq!(plain.framebuffer(), cleared.framebuffer());
    assert!(!cleared.draw_state().rshift_active());

    // `rshift(y)` is `rshift(y, 0)`: it clears just that line.
    let one = run(&format!("rshift(5, 9) rshift(9, 9) rshift(5) {scene}"));
    assert_eq!(one.draw_state().rshift(5), 0);
    assert_eq!(one.draw_state().rshift(9), 9);
    assert!(one.draw_state().rshift_active());
}

#[test]
fn a_scanline_off_the_screen_is_a_no_op() {
    let plain = run("cls(9) rectfill(0, 0, 40, 40, 7)");
    let con = run("cls(9) rectfill(0, 0, 40, 40, 7) rshift(-1, 5) rshift(256, 5) rshift(9999, 5)");
    assert_eq!(plain.framebuffer(), con.framebuffer());
    assert!(!con.draw_state().rshift_active());
    assert_eq!(con.draw_state().rshift(-1), 0);
    assert_eq!(con.draw_state().rshift(256), 0);
}

#[test]
fn mosaic_runs_before_rshift() {
    // mosaic(8) grows the 4x4 patch into the whole (0, 0) block, then the shift
    // displaces row 0 of the *finished* frame — it never slices a block open.
    let con = run("cls(9) rectfill(0, 0, 3, 3, 7) mosaic(8) rshift(0, 8)");
    assert_eq!(
        count(&con, 7),
        64,
        "the block survives, one row of it moved"
    );
    assert_eq!(px(&con, 0, 0), 9, "row 0 vacated its first 8 columns...");
    assert_eq!(px(&con, 8, 0), 7, "...and landed 8 to the right");
    // The discriminating pair: had the shift run first, mosaic would have
    // re-blocked row 0 and (0, 1) would read 9 while (8, 1) read 7.
    assert_eq!(px(&con, 0, 1), 7);
    assert_eq!(px(&con, 8, 1), 9);
}

#[test]
fn rshift_is_in_the_framebuffer_so_screen_text_shows_it() {
    let con = run("cls(9) rectfill(0, 0, 3, 3, 7) rshift(1, 2)");
    let text = screen_text(&con);
    assert_eq!(text.len(), SCREEN_H);
    assert_eq!(&text[0][..8], "77779999");
    assert_eq!(&text[1][..8], "99777799");
    assert_eq!(&text[2][..8], "77779999");
}

#[test]
fn rshift_persists_across_frames_and_does_not_feed_back() {
    let mut con = Console::new(
        &cart("cls(9)\npset(0, 3, 7)\nraw = pget(0, 3)\nmoved = pget(5, 3)"),
        0,
    )
    .unwrap();
    con.eval("rshift(3, 5)").unwrap();
    for _ in 0..5 {
        con.step(0).unwrap();
    }
    assert_eq!(con.draw_state().rshift(3), 5, "nothing resets the table");

    // `pget` reads the pristine draw buffer: the cart still sees its pixel
    // where it drew it, un-shifted.
    assert_eq!(con.get_global("raw").unwrap().as_i64(), Some(7));
    assert_eq!(con.get_global("moved").unwrap().as_i64(), Some(9));
    // Only the presented frame is displaced.
    assert_eq!(px(&con, 0, 3), 9);
    assert_eq!(px(&con, 5, 3), 7);

    // Ten frames of an un-cleared, shifted screen do not compound: the effect
    // is recomputed from the pristine draw buffer every frame, so the pixel
    // sits at x = 5 forever instead of walking right.
    let mut a = Console::new(&cart("pset(0, 3, 7) rshift(3, 5)"), 0).unwrap();
    a.step(0).unwrap();
    let first = *a.framebuffer();
    for _ in 0..9 {
        a.step(0).unwrap();
    }
    assert_eq!(&first, a.framebuffer());
    assert_eq!(px(&a, 5, 3), 7);
}

#[test]
fn rshift_composes_with_the_rest_of_the_draw_state() {
    // The display palette stays scanout-only under a shift too.
    let con = run("cls(9) rectfill(0, 0, 7, 7, 7) pal(7, 2, 1) rshift(0, 4)");
    assert_eq!(count(&con, 7), 64);
    assert_eq!(count(&con, 2), 0);
    assert_eq!(con.display_palette()[7], 2);

    // The clip rect bounds the drawing, not the raster pass: a shifted line
    // wraps across the whole 144-pixel screen regardless of the clip.
    let con = run("cls(9) clip(0, 0, 8, 8) cls(3) clip() rshift(0, 140)");
    // The 8 clipped columns 0..7 land on 140..143 and wrap onto 0..3.
    assert_eq!(px(&con, 140, 0), 3, "the clipped clear moved to the right");
    assert_eq!(px(&con, 143, 0), 3);
    assert_eq!(
        px(&con, 3, 0),
        3,
        "and the overflow wrapped back to the left"
    );
    assert_eq!(px(&con, 4, 0), 9);
    assert_eq!(px(&con, 139, 0), 9);
    assert_eq!(count(&con, 3), 8 * 8, "nothing gained or lost");
}

#[test]
fn a_full_sine_sweep_is_a_seamless_wrap() {
    // The idiom carts use: 256 cheap calls a frame. Every pixel that leaves one
    // edge arrives at the other, so no colour is ever created or destroyed.
    let con = run("cls(9) rectfill(0, 0, 60, 255, 7)
         for y = 0, 255 do rshift(y, 4 * sin(y / 32)) end");
    assert_eq!(count(&con, 7), 61 * 256, "wrap, not clip: nothing is lost");
    assert_eq!(count(&con, 9), FB_LEN - 61 * 256);
}

// ---------------------------------------------------------------------------
// determinism
// ---------------------------------------------------------------------------

#[test]
fn the_effects_replay_identically() {
    let body = "\
        function _update()
          f = flr(t() * 60)
        end
        function _draw()
          cls(1)
          camera(f % 7, f % 5)
          fillp((f * 4919) % 65536)
          rectfill(0, 0, 100, 100, 7 + (f % 15) * 16)
          circfill(70, 70, 20 + f % 9, 3)
          sspr(0, 0, 8, 8, f % 20, 40, 8 + f % 24, 8 + f % 13, f % 2 == 0, f % 3 == 0)
          mosaic(1 + f % 6)
          for y = 0, 255 do rshift(y, (f % 5) * sin(y / 32 + t())) end
        end";
    let text = cart("").replace("function _draw()\n\nend", body);

    let mut a = Console::new(&text, 7).unwrap();
    let mut b = Console::new(&text, 7).unwrap();
    for _ in 0..60 {
        a.step(0).unwrap();
        b.step(0).unwrap();
    }
    assert_eq!(a.framebuffer(), b.framebuffer());
    assert_eq!(a.draw_state().fillp(), b.draw_state().fillp());
    assert_eq!(a.draw_state().mosaic(), b.draw_state().mosaic());
    assert_eq!(a.draw_state(), b.draw_state(), "shift table included");

    // A fresh console replaying the same frames lands in the same place.
    let mut c = Console::new(&text, 7).unwrap();
    for _ in 0..60 {
        c.step(0).unwrap();
    }
    assert_eq!(a.framebuffer(), c.framebuffer());
}

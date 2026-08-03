//! Draw state: `camera`, `clip`, `pal`, `palt`.
//!
//! The contract these tests pin down:
//!
//! * every default is a no-op, so carts written before draw state existed
//!   render bit-identically (see [`defaults_change_nothing`]);
//! * the framebuffer only ever holds *draw-space* indices — the display
//!   palette lives beside it and is applied by the host at scanout;
//! * none of this state resets at a frame boundary.

use console_core::{Console, FB_LEN, IDENTITY_PAL, SCREEN_H, SCREEN_W};

/// Build a console from a bare Lua body (no meta, no sprites).
fn lua_console(body: &str) -> Console {
    Console::new(&format!("__lua__\n{body}\n"), 0).expect("cart should load")
}

/// Run one frame of a cart whose whole job is to draw.
fn draw_once(body: &str) -> Console {
    let mut con = lua_console(&format!("function _draw()\n{body}\nend"));
    con.step(0).expect("frame should run");
    con
}

fn px(con: &Console, x: usize, y: usize) -> u8 {
    con.framebuffer()[y * SCREEN_W + x]
}

fn count(con: &Console, c: u8) -> usize {
    con.framebuffer().iter().filter(|&&p| p == c).count()
}

/// Cart with sprite 1 = a 2x2 marker of colours 1..4 plus a lone 5 at (7, 7).
/// Same sheet as `tests/drawing.rs` so the two files read alike.
const SPRITE_CART: &str = "\
__lua__
function _draw() cls(0) DRAW() end

__sprites__
0000000012000000
0000000034000000
0000000000000000
0000000000000000
0000000000000000
0000000000000000
0000000000000000
0000000000000005
";

fn spr_console(draw: &str) -> Console {
    let text = SPRITE_CART.replace("DRAW()", draw);
    let mut con = Console::new(&text, 0).unwrap();
    con.step(0).unwrap();
    con
}

// ---------------------------------------------------------------------------
// backward compatibility
// ---------------------------------------------------------------------------

#[test]
fn defaults_change_nothing() {
    let con = lua_console("");
    assert_eq!(con.display_palette(), &IDENTITY_PAL);

    let ds = con.draw_state();
    assert_eq!(ds.camera(), (0, 0));
    assert_eq!(ds.clip(), (0, 0, SCREEN_W as i32 - 1, SCREEN_H as i32 - 1));
    assert_eq!(ds.draw_palette(), &IDENTITY_PAL);
    assert_eq!(ds.display_palette(), &IDENTITY_PAL);
    assert_eq!(ds.palt_mask(), 1, "only colour 0 is transparent by default");
}

#[test]
fn explicit_defaults_are_identical_to_untouched_state() {
    let scene = "cls(1) rectfill(10, 10, 30, 30, 7) circfill(70, 70, 12, 4)
                 print(\"HI\", 4, 4, 11) line(0, 0, 143, 255, 3)";
    let plain = draw_once(scene);
    let reset = draw_once(&format!("camera() clip() pal() palt()\n{scene}"));
    assert_eq!(plain.framebuffer(), reset.framebuffer());
}

// ---------------------------------------------------------------------------
// camera
// ---------------------------------------------------------------------------

#[test]
fn camera_offsets_every_primitive() {
    // pset
    let con = draw_once("camera(10, 20) pset(15, 25, 9)");
    assert_eq!(px(&con, 5, 5), 9);
    assert_eq!(count(&con, 9), 1);

    // rectfill
    let con = draw_once("camera(4, 4) rectfill(4, 4, 6, 6, 7)");
    assert_eq!(px(&con, 0, 0), 7);
    assert_eq!(px(&con, 2, 2), 7);
    assert_eq!(count(&con, 7), 9);

    // rect outline
    let con = draw_once("camera(1, 1) rect(1, 1, 10, 10, 3)");
    assert_eq!(px(&con, 0, 0), 3);
    assert_eq!(px(&con, 9, 9), 3);
    assert_eq!(px(&con, 5, 5), 0);

    // line
    let con = draw_once("camera(3, 3) line(3, 3, 13, 8, 6)");
    assert_eq!(px(&con, 0, 0), 6);
    assert_eq!(px(&con, 10, 5), 6);

    // circ / circfill
    let con = draw_once("camera(20, 20) circfill(40, 40, 3, 5)");
    assert_eq!(px(&con, 20, 20), 5);
    let con = draw_once("camera(20, 20) circ(40, 40, 3, 5)");
    assert_eq!(px(&con, 20, 20), 0, "outline is hollow");
    assert_eq!(px(&con, 23, 20), 5);

    // print
    let shifted = draw_once("camera(-8, -6) print(\"HI\", 0, 0, 11)");
    let direct = draw_once("print(\"HI\", 8, 6, 11)");
    assert_eq!(shifted.framebuffer(), direct.framebuffer());

    // spr
    let con = spr_console("camera(50, 60) spr(1, 50, 60)");
    assert_eq!(px(&con, 0, 0), 1);
    assert_eq!(px(&con, 1, 1), 4);
    assert_eq!(px(&con, 7, 7), 5);
}

#[test]
fn camera_can_push_drawing_off_screen() {
    let con = draw_once("camera(200, 0) rectfill(0, 0, 20, 20, 7)");
    assert_eq!(count(&con, 7), 0);
    // Negative camera moves content the other way.
    let con = draw_once("camera(-5, -5) pset(0, 0, 9)");
    assert_eq!(px(&con, 5, 5), 9);
}

#[test]
fn camera_with_no_args_resets_and_cls_and_pget_ignore_it() {
    let con = draw_once("camera(30, 30) camera() pset(1, 1, 9)");
    assert_eq!(px(&con, 1, 1), 9);

    // cls is always the whole screen, camera or not.
    let con = draw_once("camera(40, 40) cls(6)");
    assert_eq!(count(&con, 6), FB_LEN);

    // pget reads screen space.
    let con = draw_once("cls(0) pset(0, 0, 9) camera(10, 10) got = pget(0, 0) off = pget(10, 10)");
    assert_eq!(con.get_global("got").unwrap().as_i64(), Some(9));
    assert_eq!(con.get_global("off").unwrap().as_i64(), Some(0));
}

// ---------------------------------------------------------------------------
// clip
// ---------------------------------------------------------------------------

#[test]
fn clip_bounds_every_primitive() {
    let con = draw_once("clip(10, 10, 5, 5) rectfill(0, 0, 143, 255, 7)");
    assert_eq!(count(&con, 7), 25);
    assert_eq!(px(&con, 10, 10), 7);
    assert_eq!(px(&con, 14, 14), 7);
    assert_eq!(px(&con, 15, 14), 0);
    assert_eq!(px(&con, 9, 10), 0);

    let con = draw_once("clip(10, 10, 5, 5) circfill(12, 12, 40, 4)");
    assert_eq!(count(&con, 4), 25);

    let con = draw_once("clip(10, 10, 5, 5) line(0, 0, 143, 255, 3)");
    for (i, &p) in con.framebuffer().iter().enumerate() {
        if p == 3 {
            let (x, y) = (i % SCREEN_W, i / SCREEN_W);
            assert!((10..15).contains(&x) && (10..15).contains(&y));
        }
    }

    let con = draw_once("clip(0, 0, 8, 4) print(\"HELLO\", 0, 0, 11)");
    for (i, &p) in con.framebuffer().iter().enumerate() {
        if p == 11 {
            let (x, y) = (i % SCREEN_W, i / SCREEN_W);
            assert!(x < 8 && y < 4, "print escaped the clip at ({x}, {y})");
        }
    }

    let con = spr_console("clip(0, 0, 1, 1) spr(1, 0, 0)");
    assert_eq!(count(&con, 1), 1);
    assert_eq!(count(&con, 2) + count(&con, 3) + count(&con, 4), 0);
}

#[test]
fn clip_is_screen_space_applied_after_the_camera() {
    // The clip window stays at screen (0,0)-(9,9) even though the camera
    // shifts what lands there.
    let con = draw_once("clip(0, 0, 10, 10) camera(100, 100) rectfill(100, 100, 200, 200, 7)");
    assert_eq!(count(&con, 7), 100);
    assert_eq!(px(&con, 0, 0), 7);
    assert_eq!(px(&con, 9, 9), 7);
    assert_eq!(px(&con, 10, 10), 0);
}

#[test]
fn clip_clamps_negative_and_overflowing_rects() {
    // Straddling the top-left corner: only the on-screen part survives.
    let con = draw_once("clip(-20, -20, 30, 30) cls(7)");
    assert_eq!(count(&con, 7), 10 * 10);
    assert_eq!(px(&con, 9, 9), 7);
    assert_eq!(px(&con, 10, 10), 0);

    // Straddling the bottom-right corner.
    let con = draw_once("clip(140, 250, 1000, 1000) cls(5)");
    assert_eq!(count(&con, 5), 4 * 6);
    assert_eq!(px(&con, 143, 255), 5);

    // Entirely off screen, and zero/negative sizes: nothing draws, no panic.
    for spec in [
        "clip(200, 300, 10, 10)",
        "clip(0, 0, 0, 10)",
        "clip(0, 0, 10, 0)",
        "clip(5, 5, -8, -8)",
        "clip(-100, -100, 50, 50)",
    ] {
        let con = draw_once(&format!(
            "{spec} cls(7) rectfill(0, 0, 143, 255, 7) spr(0, 0, 0)"
        ));
        assert_eq!(count(&con, 7), 0, "{spec} should clip everything away");
    }
}

#[test]
fn clip_with_no_args_restores_the_full_screen() {
    let con = draw_once("clip(0, 0, 4, 4) clip() cls(7)");
    assert_eq!(count(&con, 7), FB_LEN);
}

#[test]
fn cls_respects_the_clip_rect() {
    // The documented choice: cls is a windowed clear, not a screen wipe.
    let con = draw_once("cls(3) clip(20, 20, 10, 10) cls(7)");
    assert_eq!(count(&con, 7), 100);
    assert_eq!(count(&con, 3), FB_LEN - 100);
    assert_eq!(px(&con, 20, 20), 7);
    assert_eq!(px(&con, 30, 20), 3);
}

// ---------------------------------------------------------------------------
// pal: draw palette
// ---------------------------------------------------------------------------

#[test]
fn pal_draw_remap_rewrites_shape_colours_in_the_framebuffer() {
    let con = draw_once("pal(7, 2) rectfill(0, 0, 9, 9, 7)");
    assert_eq!(count(&con, 2), 100);
    assert_eq!(
        count(&con, 7),
        0,
        "the framebuffer really holds the new index"
    );

    // Chains do not compose: the map is looked up once, per draw.
    let con = draw_once("pal(7, 2) pal(2, 5) rectfill(0, 0, 9, 9, 7)");
    assert_eq!(count(&con, 2), 100);
    assert_eq!(count(&con, 5), 0);

    // print and pset go through it too.
    let con = draw_once("pal(11, 4) print(\"HI\", 0, 0, 11) pset(100, 100, 11)");
    assert_eq!(count(&con, 11), 0);
    assert!(count(&con, 4) > 1);
}

#[test]
fn pal_draw_remap_rewrites_sprite_pixels() {
    let con = spr_console("pal(1, 9) pal(4, 8) spr(1, 0, 0)");
    assert_eq!(px(&con, 0, 0), 9);
    assert_eq!(px(&con, 1, 0), 2, "unmapped colours pass through");
    assert_eq!(px(&con, 1, 1), 8);
    assert_eq!(px(&con, 7, 7), 5);
}

#[test]
fn pal_draw_remap_does_not_move_the_display_palette() {
    let con = draw_once("pal(7, 2) rectfill(0, 0, 4, 4, 7)");
    assert_eq!(con.display_palette(), &IDENTITY_PAL);
}

// ---------------------------------------------------------------------------
// pal: display palette
// ---------------------------------------------------------------------------

#[test]
fn pal_display_remap_leaves_the_framebuffer_alone() {
    let con = draw_once("cls(1) rectfill(0, 0, 9, 9, 7) pal(7, 2, 1) pal(1, 0, 1)");
    // Indices are exactly what was drawn...
    assert_eq!(count(&con, 7), 100);
    assert_eq!(count(&con, 2), 0);
    assert_eq!(count(&con, 1), FB_LEN - 100);
    // ...and the fade lives entirely in the display map.
    let mut expected = IDENTITY_PAL;
    expected[7] = 2;
    expected[1] = 0;
    assert_eq!(con.display_palette(), &expected);
}

#[test]
fn display_remap_before_drawing_is_still_only_a_display_map() {
    let faded = draw_once("pal(7, 0, 1) rectfill(0, 0, 9, 9, 7)");
    let plain = draw_once("rectfill(0, 0, 9, 9, 7)");
    assert_eq!(faded.framebuffer(), plain.framebuffer());
    assert_eq!(faded.display_palette()[7], 0);
}

#[test]
fn a_whole_screen_fade_costs_sixteen_calls_and_no_redraw() {
    // The reason the display palette exists: fade every colour to 0 without
    // touching a single pixel.
    let con = draw_once(
        "cls(3) circfill(70, 70, 30, 11)
         for i = 0, 15 do pal(i, 0, 1) end",
    );
    assert_eq!(con.display_palette(), &[0u8; 16]);
    assert!(count(&con, 11) > 0, "pixels are untouched by the fade");
    assert!(count(&con, 3) > 0);
}

#[test]
fn display_pal_survives_a_reset_of_the_draw_pal_only_via_full_reset() {
    let con = draw_once("pal(7, 2) pal(3, 4, 1) pal() rectfill(0, 0, 4, 4, 7)");
    assert_eq!(count(&con, 7), 25, "draw map was reset");
    assert_eq!(
        con.display_palette(),
        &IDENTITY_PAL,
        "display map was reset"
    );
}

// ---------------------------------------------------------------------------
// palt
// ---------------------------------------------------------------------------

#[test]
fn palt_sets_custom_transparency() {
    // Make colour 2 transparent as well as 0.
    let con = spr_console("cls(9) palt(2, true) spr(1, 0, 0)");
    assert_eq!(px(&con, 0, 0), 1);
    assert_eq!(px(&con, 1, 0), 9, "colour 2 is now see-through");
    assert_eq!(px(&con, 0, 1), 3);

    // Make colour 0 opaque: the sprite becomes a solid 8x8 block.
    let con = spr_console("cls(9) palt(0, false) spr(1, 0, 0)");
    assert_eq!(px(&con, 2, 0), 0);
    assert_eq!(px(&con, 6, 6), 0);
    assert_eq!(count(&con, 9), FB_LEN - 64);

    // A bare `palt(c)` means "opaque".
    let con = spr_console("cls(9) palt(0) spr(1, 0, 0)");
    assert_eq!(px(&con, 2, 0), 0);
}

#[test]
fn palt_resets_to_colour_zero_only() {
    let con = spr_console("cls(9) palt(0, false) palt(2, true) palt() spr(1, 0, 0)");
    assert_eq!(px(&con, 1, 0), 2, "colour 2 is opaque again");
    assert_eq!(px(&con, 2, 0), 9, "colour 0 is transparent again");
}

#[test]
fn transparency_is_tested_before_the_draw_palette() {
    // palt keys on the sprite's SOURCE colour. Mapping 1 -> 0 must not make
    // those pixels vanish...
    let con = spr_console("cls(9) pal(1, 0) spr(1, 0, 0)");
    assert_eq!(px(&con, 0, 0), 0, "source colour 1 is opaque, drawn as 0");
    assert_eq!(px(&con, 2, 0), 9, "source colour 0 is still transparent");

    // ...and mapping 0 -> 7 must not make the transparent pixels appear.
    let con = spr_console("cls(9) pal(0, 7) spr(1, 0, 0)");
    assert_eq!(px(&con, 2, 0), 9);
    assert_eq!(count(&con, 7), 0);

    // Marking 1 transparent hides it even though it is remapped.
    let con = spr_console("cls(9) palt(1, true) pal(1, 6) spr(1, 0, 0)");
    assert_eq!(px(&con, 0, 0), 9);
    assert_eq!(count(&con, 6), 0);
}

#[test]
fn pal_with_no_args_resets_palt_too() {
    let con = spr_console("cls(9) palt(0, false) pal() spr(1, 0, 0)");
    assert_eq!(px(&con, 2, 0), 9, "pal() restored the default transparency");
}

// ---------------------------------------------------------------------------
// persistence
// ---------------------------------------------------------------------------

#[test]
fn draw_state_persists_across_steps() {
    let mut con = lua_console(
        "function _init() camera(10, 10) clip(0, 0, 40, 40) pal(7, 2) palt(0, false) pal(3, 5, 1) end
         function _draw() pset(10, 10, 7) end",
    );
    for _ in 0..5 {
        con.step(0).unwrap();
    }
    // Camera and draw palette are still in force five frames later.
    assert_eq!(px(&con, 0, 0), 2);

    let ds = con.draw_state();
    assert_eq!(ds.camera(), (10, 10));
    assert_eq!(ds.clip(), (0, 0, 39, 39));
    assert_eq!(ds.draw_palette()[7], 2);
    assert_eq!(ds.palt_mask(), 0);
    assert_eq!(con.display_palette()[3], 5);
}

#[test]
fn a_fresh_console_starts_from_the_defaults() {
    let mut con = lua_console("function _draw() camera(50, 50) pal(1, 2, 1) end");
    con.step(0).unwrap();
    assert_eq!(con.draw_state().camera(), (50, 50));

    let fresh = lua_console("function _draw() end");
    assert_eq!(fresh.draw_state().camera(), (0, 0));
    assert_eq!(fresh.display_palette(), &IDENTITY_PAL);
}

#[test]
fn draw_state_is_deterministic_across_runs() {
    let body = "function _update() camera(t() * 60, 0) end
                function _draw()
                  clip(4, 4, 100, 100) cls(1)
                  pal(7, (flr(t() * 60) % 15) + 1)
                  rectfill(10, 10, 60, 60, 7)
                  pal(2, flr(t() * 60) % 16, 1)
                end";
    let mut a = lua_console(body);
    let mut b = lua_console(body);
    for _ in 0..30 {
        a.step(0).unwrap();
        b.step(0).unwrap();
    }
    assert_eq!(a.framebuffer(), b.framebuffer());
    assert_eq!(a.display_palette(), b.display_palette());
}

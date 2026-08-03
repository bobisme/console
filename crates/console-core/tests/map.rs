//! The tile map: the `__map__` cart section and `map()` / `mget` / `mset`.
//!
//! The contract these tests pin down:
//!
//! * `__map__` is a hex grid, two digits per cell, that pads with tile 0 and
//!   rejects (rather than truncates) anything that runs off the 128x64 map;
//! * **tile 0 is the empty cell** — `map()` skips it instead of drawing
//!   sprite 0, so an unset cell costs nothing and shows nothing;
//! * `map()` is exactly a sequence of `spr()` calls: the camera, the clip
//!   rectangle, `pal` and `palt` all apply identically (see
//!   [`map_is_indistinguishable_from_the_equivalent_spr_calls`]);
//! * a cart with no `__map__` gets an all-zero map and the whole API still
//!   works, so nothing that predates the map changes;
//! * `mset` is ordinary console state and replays deterministically.

use console_core::{Cart, Console, Error, FB_LEN, MAP_H, MAP_W, SCREEN_W};

/// Sprite sheet for these tests: solid 8x8 tiles of one colour each.
///
/// Sprite 0 is deliberately **not** blank (solid colour 5) so that "tile 0 is
/// skipped" is distinguishable from "tile 0 draws sprite 0, which happens to be
/// invisible".
///
/// | sprite | colour |
/// |--------|--------|
/// | 0      | 5      |
/// | 1      | 1      |
/// | 2      | 2      |
/// | 3      | 3      |
/// | 17     | 4      |
fn sheet() -> String {
    let mut s = String::new();
    for _ in 0..8 {
        s.push_str("55555555111111112222222233333333\n");
    }
    for _ in 0..8 {
        s.push_str("0000000044444444\n");
    }
    s
}

/// A cart with the sheet above, the given `__map__` body and `draw` as `_draw`.
fn cart_text(map: &str, draw: &str) -> String {
    format!(
        "__lua__\nfunction _draw()\n{draw}\nend\n\n__sprites__\n{}\n__map__\n{map}\n",
        sheet()
    )
}

/// A 2x2 corner of map: tile 1, tile 2 / empty, tile 3.
const CORNER: &str = "0102\n0003";

fn run(map: &str, draw: &str) -> Console {
    let mut con = Console::new(&cart_text(map, draw), 0).expect("cart should load");
    con.step(0).expect("frame should run");
    con
}

fn px(con: &Console, x: usize, y: usize) -> u8 {
    con.framebuffer()[y * SCREEN_W + x]
}

fn count(con: &Console, c: u8) -> usize {
    con.framebuffer().iter().filter(|&&p| p == c).count()
}

/// `mget` through the Lua API, so the tests exercise the binding itself.
fn mget(con: &mut Console, cx: i32, cy: i32) -> i64 {
    con.eval(&format!("return mget({cx}, {cy})"))
        .unwrap()
        .as_i64()
        .expect("mget returns a number")
}

// ---------------------------------------------------------------------------
// __map__ parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_a_hex_grid_two_digits_per_cell() {
    let cart = Cart::parse("__lua__\nx=1\n\n__map__\n0102ff\n1000\n").unwrap();
    let m = cart.map();
    assert_eq!(m[0], 1);
    assert_eq!(m[1], 2);
    assert_eq!(m[2], 255, "ff is the top tile id");
    assert_eq!(m[MAP_W], 16, "10 is sixteen, not one-zero");
    assert_eq!(m[MAP_W + 1], 0);
    assert_eq!(m.len(), MAP_W * MAP_H);
}

#[test]
fn short_rows_pad_and_missing_rows_are_tile_zero() {
    let cart = Cart::parse("__lua__\nx=1\n\n__map__\n01\n0203\n").unwrap();
    let m = cart.map();
    assert_eq!(m[0], 1);
    // The rest of row 0 pads with tile 0...
    assert!(m[1..MAP_W].iter().all(|&t| t == 0));
    assert_eq!(&m[MAP_W..MAP_W + 2], &[2, 3]);
    // ...and every row past the last authored one is empty.
    assert!(m[2 * MAP_W..].iter().all(|&t| t == 0));
}

#[test]
fn comments_and_blank_lines_do_not_consume_rows() {
    let text = "__lua__\nx=1\n\n__map__\n\
                # the ground floor\n\
                0101\n\
                \n\
                # upper deck\n\
                0202\n";
    let cart = Cart::parse(text).unwrap();
    let m = cart.map();
    assert_eq!(&m[0..2], &[1, 1]);
    assert_eq!(&m[MAP_W..MAP_W + 2], &[2, 2]);
    assert!(m[2 * MAP_W..].iter().all(|&t| t == 0));
}

#[test]
fn crlf_and_trailing_whitespace_are_tolerated() {
    let cart = Cart::parse("__lua__\r\nx=1\r\n\r\n__map__\r\n0102  \r\n").unwrap();
    assert_eq!(&cart.map()[0..2], &[1, 2]);
}

#[test]
fn an_overlong_row_is_rejected_and_names_the_line() {
    let row = "01".repeat(MAP_W + 1);
    let err = Cart::parse(&format!("__lua__\nx=1\n\n__map__\n0000\n{row}\n")).unwrap_err();
    assert!(matches!(err, Error::Cart(_)));
    let msg = err.to_string();
    assert!(msg.contains("__map__ line 2"), "{msg}");
    assert!(msg.contains("128"), "{msg}");
}

#[test]
fn too_many_rows_are_rejected_and_name_the_line() {
    let mut body = String::new();
    for _ in 0..MAP_H + 1 {
        body.push_str("01\n");
    }
    let err = Cart::parse(&format!("__lua__\nx=1\n\n__map__\n{body}")).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains(&format!("__map__ line {}", MAP_H + 1)),
        "{msg}"
    );
    assert!(msg.contains("64"), "{msg}");
}

#[test]
fn exactly_the_full_map_is_accepted() {
    let row = "01".repeat(MAP_W);
    let body = format!("{row}\n").repeat(MAP_H);
    let cart = Cart::parse(&format!("__lua__\nx=1\n\n__map__\n{body}")).unwrap();
    assert!(cart.map().iter().all(|&t| t == 1));
}

#[test]
fn an_odd_length_row_is_rejected() {
    let err = Cart::parse("__lua__\nx=1\n\n__map__\n010\n").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("__map__ line 1"), "{msg}");
    assert!(msg.contains("2 hex digits"), "{msg}");
}

#[test]
fn a_non_hex_digit_is_rejected() {
    let err = Cart::parse("__lua__\nx=1\n\n__map__\n0102\n01zz\n").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("__map__ line 2"), "{msg}");
    assert!(msg.contains("hex digit"), "{msg}");
}

// ---------------------------------------------------------------------------
// back compatibility
// ---------------------------------------------------------------------------

#[test]
fn a_cart_with_no_map_section_gets_an_all_zero_map() {
    let cart = Cart::parse("__lua__\nx=1\n").unwrap();
    assert!(cart.map().iter().all(|&t| t == 0));
}

#[test]
fn the_map_api_works_without_a_map_section() {
    // Same sheet, no __map__ at all: map() draws nothing over the clear colour,
    // mget reads 0, and mset still writes.
    let text = format!(
        "__lua__\nfunction _draw() cls(9) map() end\n\n__sprites__\n{}\n",
        sheet()
    );
    let mut con = Console::new(&text, 0).unwrap();
    con.step(0).unwrap();
    assert_eq!(count(&con, 9), FB_LEN, "an empty map draws nothing at all");
    assert_eq!(mget(&mut con, 0, 0), 0);

    con.eval("mset(3, 4, 17)").unwrap();
    assert_eq!(mget(&mut con, 3, 4), 17);
}

// ---------------------------------------------------------------------------
// map(): drawing
// ---------------------------------------------------------------------------

#[test]
fn map_draws_each_cell_as_its_sprite() {
    let con = run(CORNER, "cls(0) map()");
    // cell (0,0) = tile 1 -> colour 1
    assert_eq!(px(&con, 0, 0), 1);
    assert_eq!(px(&con, 7, 7), 1);
    // cell (1,0) = tile 2 -> colour 2, eight pixels to the right
    assert_eq!(px(&con, 8, 0), 2);
    assert_eq!(px(&con, 15, 7), 2);
    // cell (1,1) = tile 3 -> colour 3, eight pixels down
    assert_eq!(px(&con, 8, 8), 3);
    assert_eq!(px(&con, 15, 15), 3);
    // exactly one 8x8 tile of each, nothing bleeding sideways
    assert_eq!(count(&con, 1), 64);
    assert_eq!(count(&con, 2), 64);
    assert_eq!(count(&con, 3), 64);
}

#[test]
fn tile_zero_is_the_empty_cell_and_is_never_drawn() {
    // Sprite 0 in this cart is a solid block of colour 5, so "skipped" and
    // "drew sprite 0" are distinguishable.
    let con = run(CORNER, "cls(9) map()");
    assert_eq!(count(&con, 5), 0, "tile 0 must not draw sprite 0");
    // The one interior empty cell keeps the clear colour...
    assert_eq!(px(&con, 0, 8), 9);
    assert_eq!(px(&con, 7, 15), 9);
    // ...as does every cell past the authored corner.
    assert_eq!(count(&con, 9), FB_LEN - 3 * 64);

    // Even with colour 0 made opaque, an empty cell still draws nothing: the
    // skip happens per cell, not per pixel.
    let con = run(CORNER, "cls(9) palt(0, false) map()");
    assert_eq!(count(&con, 5), 0);
    assert_eq!(px(&con, 0, 8), 9);
}

#[test]
fn map_defaults_draw_the_whole_map_at_the_origin() {
    let explicit = run(CORNER, &format!("cls(0) map(0, 0, 0, 0, {MAP_W}, {MAP_H})"));
    let bare = run(CORNER, "cls(0) map()");
    assert_eq!(explicit.framebuffer(), bare.framebuffer());
}

#[test]
fn map_draws_a_sub_block_at_a_screen_position() {
    // Just cell (1,0) — tile 2 — placed at screen (20, 30).
    let con = run(CORNER, "cls(0) map(1, 0, 20, 30, 1, 1)");
    assert_eq!(px(&con, 20, 30), 2);
    assert_eq!(px(&con, 27, 37), 2);
    assert_eq!(count(&con, 2), 64);
    assert_eq!(
        count(&con, 1),
        0,
        "the neighbouring cell is outside the block"
    );
    assert_eq!(count(&con, 3), 0);
}

#[test]
fn a_non_positive_cel_size_draws_nothing() {
    for spec in [
        "map(0, 0, 0, 0, 0, 4)",
        "map(0, 0, 0, 0, 4, 0)",
        "map(0, 0, 0, 0, -4, -4)",
    ] {
        let con = run(CORNER, &format!("cls(9) {spec}"));
        assert_eq!(count(&con, 9), FB_LEN, "{spec} should draw nothing");
    }
}

#[test]
fn cels_outside_the_map_are_skipped_without_wrapping() {
    // A 4x4 block starting two cells before the origin: only the four real
    // cells draw, and they keep their offset inside the block.
    let con = run(CORNER, "cls(9) map(-2, -2, 0, 0, 4, 4)");
    assert_eq!(px(&con, 16, 16), 1);
    assert_eq!(px(&con, 24, 16), 2);
    assert_eq!(px(&con, 24, 24), 3);
    assert_eq!(px(&con, 16, 24), 9, "cell (0,1) is empty, not wrapped");
    assert_eq!(count(&con, 9), FB_LEN - 3 * 64);

    // Past the far corner there is nothing to draw and nothing wraps back.
    let con = run(
        CORNER,
        &format!("cls(9) map({}, {}, 0, 0, 8, 8)", MAP_W - 2, MAP_H - 2),
    );
    assert_eq!(count(&con, 9), FB_LEN);

    // Entirely off the map in both directions: still no error, still nothing.
    let con = run(CORNER, "cls(9) map(500, 500, 0, 0, 32, 32)");
    assert_eq!(count(&con, 9), FB_LEN);
    let con = run(CORNER, "cls(9) map(-500, -500, 0, 0, 32, 32)");
    assert_eq!(count(&con, 9), FB_LEN);
}

#[test]
fn a_full_screen_map_covers_every_pixel() {
    // 18x32 tiles is the whole 144x256 portrait screen.
    let mut body = String::new();
    for _ in 0..32 {
        body.push_str(&"01".repeat(18));
        body.push('\n');
    }
    let con = run(&body, "cls(9) map()");
    assert_eq!(count(&con, 1), FB_LEN);
}

// ---------------------------------------------------------------------------
// map() and the draw state
// ---------------------------------------------------------------------------

#[test]
fn map_is_indistinguishable_from_the_equivalent_spr_calls() {
    // The whole point of routing map() through the sprite blit: for any draw
    // state, the two produce byte-identical framebuffers.
    for state in [
        "",
        "camera(5, 9)",
        "camera(-3, -7)",
        "clip(4, 4, 9, 9)",
        "pal(1, 11) pal(3, 14)",
        "palt(2, true)",
        "palt(0, false)",
        "camera(6, 6) clip(2, 2, 20, 20) pal(2, 12) palt(1, true)",
    ] {
        let via_map = run(CORNER, &format!("cls(9) {state} map(0, 0, 30, 40, 2, 2)"));
        let via_spr = run(
            CORNER,
            &format!("cls(9) {state} spr(1, 30, 40) spr(2, 38, 40) spr(3, 38, 48)"),
        );
        assert_eq!(
            via_map.framebuffer(),
            via_spr.framebuffer(),
            "map() diverged from spr() under `{state}`"
        );
    }
}

#[test]
fn the_camera_offsets_the_map_destination() {
    let con = run(CORNER, "cls(9) camera(8, 8) map(0, 0, 8, 8, 2, 2)");
    assert_eq!(px(&con, 0, 0), 1, "camera cancels the destination");
    assert_eq!(px(&con, 8, 0), 2);
    assert_eq!(px(&con, 8, 8), 3);

    // Pushed entirely off screen, nothing survives.
    let con = run(CORNER, "cls(9) camera(300, 300) map()");
    assert_eq!(count(&con, 9), FB_LEN);
}

#[test]
fn the_clip_rect_bounds_the_map() {
    // A 4x4 window over the top-left tile only.
    let con = run(CORNER, "cls(9) clip(0, 0, 4, 4) map()");
    assert_eq!(count(&con, 1), 16);
    assert_eq!(count(&con, 2), 0);
    assert_eq!(count(&con, 3), 0);
    assert_eq!(px(&con, 3, 3), 1);
    assert_eq!(px(&con, 4, 3), 9);

    // Clip is screen space, applied after the camera.
    let con = run(
        CORNER,
        "cls(9) clip(0, 0, 8, 8) camera(8, 8) map(0, 0, 8, 8, 2, 2)",
    );
    assert_eq!(
        count(&con, 1),
        64,
        "the camera brought cell (0,0) into the window"
    );
    assert_eq!(count(&con, 2), 0);

    // An empty clip draws nothing.
    let con = run(CORNER, "cls(9) clip(0, 0, 0, 0) map()");
    assert_eq!(count(&con, 9), FB_LEN);
}

#[test]
fn the_draw_palette_remaps_map_pixels() {
    let con = run(CORNER, "cls(0) pal(1, 11) pal(3, 14) map()");
    assert_eq!(px(&con, 0, 0), 11);
    assert_eq!(px(&con, 8, 0), 2, "unmapped colours pass through");
    assert_eq!(px(&con, 8, 8), 14);
    assert_eq!(
        count(&con, 1),
        0,
        "the framebuffer holds the remapped index"
    );
}

#[test]
fn palt_applies_per_tile_pixel_on_the_source_colour() {
    // Colour 2 transparent: cell (1,0) vanishes, the others stay.
    let con = run(CORNER, "cls(9) palt(2, true) map()");
    assert_eq!(px(&con, 8, 0), 9);
    assert_eq!(count(&con, 2), 0);
    assert_eq!(count(&con, 1), 64);

    // Transparency is tested BEFORE the draw palette, exactly as in spr():
    // mapping 1 -> 0 draws colour 0 rather than making the tile disappear.
    let con = run(CORNER, "cls(9) pal(1, 0) map()");
    assert_eq!(px(&con, 0, 0), 0);
    assert_eq!(count(&con, 0), 64);

    // ...and marking 1 transparent hides it even though it is also remapped.
    let con = run(CORNER, "cls(9) palt(1, true) pal(1, 6) map()");
    assert_eq!(px(&con, 0, 0), 9);
    assert_eq!(count(&con, 6), 0);
}

// ---------------------------------------------------------------------------
// mget / mset
// ---------------------------------------------------------------------------

#[test]
fn mget_reads_authored_cells_and_zero_off_the_map() {
    let mut con = run(CORNER, "cls(0)");
    assert_eq!(mget(&mut con, 0, 0), 1);
    assert_eq!(mget(&mut con, 1, 0), 2);
    assert_eq!(mget(&mut con, 0, 1), 0);
    assert_eq!(mget(&mut con, 1, 1), 3);

    for (cx, cy) in [
        (-1, 0),
        (0, -1),
        (MAP_W as i32, 0),
        (0, MAP_H as i32),
        (9999, 9999),
        (-9999, -9999),
    ] {
        assert_eq!(mget(&mut con, cx, cy), 0, "mget({cx}, {cy}) should read 0");
    }
}

#[test]
fn mset_round_trips_and_ignores_cells_off_the_map() {
    let mut con = run(CORNER, "cls(0)");
    con.eval("mset(10, 20, 255) mset(0, 0, 0)").unwrap();
    assert_eq!(mget(&mut con, 10, 20), 255);
    assert_eq!(
        mget(&mut con, 0, 0),
        0,
        "a cell can be cleared back to empty"
    );

    // The far corner is in range; one past it is not.
    con.eval(&format!("mset({}, {}, 7)", MAP_W - 1, MAP_H - 1))
        .unwrap();
    assert_eq!(mget(&mut con, MAP_W as i32 - 1, MAP_H as i32 - 1), 7);

    for (cx, cy) in [(-1, 0), (0, -1), (MAP_W as i32, 0), (0, MAP_H as i32)] {
        con.eval(&format!("mset({cx}, {cy}, 9)")).unwrap();
        assert_eq!(mget(&mut con, cx, cy), 0);
    }

    // Fractional coordinates floor, like every other coordinate in the API.
    con.eval("mset(2.9, 3.9, 12)").unwrap();
    assert_eq!(mget(&mut con, 2, 3), 12);
}

#[test]
fn mset_is_visible_in_the_next_map_draw() {
    let mut con = Console::new(
        &cart_text(
            CORNER,
            "cls(9)\nif t() > 0 then mset(0, 1, 17) end\nmap(0, 0, 0, 0, 2, 2)",
        ),
        0,
    )
    .unwrap();

    con.step(0).unwrap();
    assert_eq!(px(&con, 0, 8), 9, "cell (0,1) starts empty");

    // Frame 2 sets the cell before drawing: tile 17 is the colour-4 sprite in
    // the second sheet row band.
    con.step(0).unwrap();
    assert_eq!(px(&con, 0, 8), 4);
    assert_eq!(px(&con, 7, 15), 4);
    assert_eq!(mget(&mut con, 0, 1), 17);
}

#[test]
fn mset_persists_across_frames_without_being_redrawn() {
    let mut con = Console::new(
        &cart_text(
            CORNER,
            "cls(9)\nif t() == 0 then mset(4, 4, 2) end\nmap(0, 0, 0, 0, 8, 8)",
        ),
        0,
    )
    .unwrap();
    for _ in 0..10 {
        con.step(0).unwrap();
    }
    assert_eq!(
        mget(&mut con, 4, 4),
        2,
        "nothing resets the map at a frame boundary"
    );
    assert_eq!(px(&con, 32, 32), 2);
}

#[test]
fn mset_does_not_touch_the_carts_own_map() {
    let mut con = run(CORNER, "cls(0)");
    con.eval("mset(0, 0, 99)").unwrap();
    assert_eq!(mget(&mut con, 0, 0), 99);
    assert_eq!(
        con.cart().map()[0],
        1,
        "the parsed cart is the pristine map"
    );
}

// ---------------------------------------------------------------------------
// determinism
// ---------------------------------------------------------------------------

#[test]
fn map_drawing_and_mutation_replay_identically() {
    // Terrain that carves itself away as the camera scrolls: every frame both
    // draws the map and mutates it.
    let body = "\
        function _update()
          local f = flr(t() * 60)
          mset(f % 16, f % 8, (f % 3) + 1)
          if f % 4 == 0 then mset(f % 16, 7 - (f % 8), 0) end
        end
        function _draw()
          cls(9)
          camera(flr(t() * 30), 0)
          map(0, 0, 0, 0, 18, 32)
        end";
    let text = cart_text(CORNER, "").replace("function _draw()\n\nend", body);

    let mut a = Console::new(&text, 7).unwrap();
    let mut b = Console::new(&text, 7).unwrap();
    for _ in 0..60 {
        a.step(0).unwrap();
        b.step(0).unwrap();
    }
    assert_eq!(a.framebuffer(), b.framebuffer());
    assert_eq!(mget(&mut a, 3, 3), mget(&mut b, 3, 3));

    // And a fresh console replaying the same inputs lands in the same place.
    let mut c = Console::new(&text, 7).unwrap();
    for _ in 0..60 {
        c.step(0).unwrap();
    }
    assert_eq!(a.framebuffer(), c.framebuffer());
}

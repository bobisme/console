use console_core::{Console, FB_LEN, SCREEN_H, SCREEN_W};

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

#[test]
fn framebuffer_is_the_documented_shape() {
    let con = lua_console("");
    assert_eq!(con.framebuffer().len(), FB_LEN);
    assert_eq!(FB_LEN, SCREEN_W * SCREEN_H);
    assert_eq!((SCREEN_W, SCREEN_H), (192, 320));
    assert!(con.framebuffer().iter().all(|&p| p == 0));
}

#[test]
fn cls_fills_and_masks_colour() {
    let con = draw_once("cls(7)");
    assert_eq!(count(&con, 7), FB_LEN);
    // All six color bits survive.
    let con = draw_once("cls(23)");
    assert_eq!(count(&con, 23), FB_LEN);
    // Values beyond the palette wrap with the six-bit mask.
    let con = draw_once("cls(71)");
    assert_eq!(count(&con, 7), FB_LEN);
    // negative wraps into range too
    let con = draw_once("cls(-1)");
    assert_eq!(count(&con, 63), FB_LEN);
}

#[test]
fn pset_pget_roundtrip_including_out_of_bounds() {
    let con = draw_once(
        "cls(0)
         pset(10, 20, 9)
         pset(-5, 5, 3) pset(5, -5, 3) pset(192, 5, 3) pset(5, 320, 3)
         got = pget(10, 20)
         oob = pget(-1, -1)
         oob2 = pget(1000, 1000)",
    );
    assert_eq!(px(&con, 10, 20), 9);
    assert_eq!(count(&con, 9), 1);
    assert_eq!(count(&con, 3), 0, "out-of-bounds pset must be a no-op");
    assert_eq!(con.get_global("got").unwrap().as_i64(), Some(9));
    assert_eq!(con.get_global("oob").unwrap().as_i64(), Some(0));
    assert_eq!(con.get_global("oob2").unwrap().as_i64(), Some(0));
}

#[test]
fn coordinates_are_floored() {
    let con = draw_once("pset(10.9, 20.9, 5) pset(-0.2, 0, 6)");
    assert_eq!(px(&con, 10, 20), 5);
    assert_eq!(count(&con, 6), 0, "-0.2 floors to -1, off screen");
}

#[test]
fn rectfill_is_inclusive_and_clips() {
    let con = draw_once("rectfill(2, 3, 4, 5, 7)");
    assert_eq!(count(&con, 7), 3 * 3);
    assert_eq!(px(&con, 2, 3), 7);
    assert_eq!(px(&con, 4, 5), 7);
    assert_eq!(px(&con, 5, 5), 0);

    // Fully off-screen and partially off-screen rects.
    let con = draw_once("rectfill(-50, -50, -10, -10, 4)");
    assert_eq!(count(&con, 4), 0);
    let con = draw_once("rectfill(-10, -10, 1, 1, 4)");
    assert_eq!(count(&con, 4), 4);
    let con = draw_once("rectfill(190, 318, 400, 400, 4)");
    assert_eq!(count(&con, 4), 4);
    // Reversed coordinates still fill the same region.
    let con = draw_once("rectfill(4, 5, 2, 3, 7)");
    assert_eq!(count(&con, 7), 9);
}

#[test]
fn rect_outline_only() {
    let con = draw_once("rect(0, 0, 9, 9, 3)");
    assert_eq!(count(&con, 3), 10 * 4 - 4);
    assert_eq!(px(&con, 5, 5), 0);
}

#[test]
fn circ_and_circfill() {
    let con = draw_once("circfill(20, 20, 5, 11)");
    let filled = count(&con, 11);
    assert!((60..=100).contains(&filled), "unexpected area {filled}");
    assert_eq!(px(&con, 20, 20), 11);

    let con = draw_once("circ(20, 20, 5, 11)");
    assert_eq!(px(&con, 20, 20), 0, "outline must be hollow");
    assert!(count(&con, 11) > 0);

    // Clipped at every edge without panicking.
    let con = draw_once("circfill(0, 0, 40, 2) circfill(191, 319, 40, 2)");
    assert!(count(&con, 2) > 0);

    // r = 0 draws a single pixel; r < 0 draws nothing.
    let con = draw_once("circfill(30, 30, 0, 5)");
    assert_eq!(count(&con, 5), 1);
    let con = draw_once("circfill(30, 30, -1, 5)");
    assert_eq!(count(&con, 5), 0);
}

#[test]
fn line_hits_both_endpoints() {
    let con = draw_once("line(3, 3, 30, 12, 6)");
    assert_eq!(px(&con, 3, 3), 6);
    assert_eq!(px(&con, 30, 12), 6);
    // Off-screen lines clip rather than panic.
    let con = draw_once("line(-100, -100, 300, 400, 6)");
    assert!(count(&con, 6) > 0);
}

/// Cart with sprite 0 blank and sprite 1 = a distinctive L-shaped marker.
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

#[test]
fn spr_draws_with_colour_zero_transparent() {
    let con = spr_console("cls(9) spr(1, 0, 0)");
    // The 2x2 block of non-zero pixels lands at the sprite origin...
    assert_eq!(px(&con, 0, 0), 1);
    assert_eq!(px(&con, 1, 0), 2);
    assert_eq!(px(&con, 0, 1), 3);
    assert_eq!(px(&con, 1, 1), 4);
    // ...and every transparent pixel leaves the background untouched.
    assert_eq!(px(&con, 2, 0), 9);
    assert_eq!(px(&con, 6, 6), 9);
    // Sprite 1 has exactly 5 opaque pixels (the 2x2 marker plus one at 7,7).
    assert_eq!(px(&con, 7, 7), 5);
    assert_eq!(count(&con, 9), FB_LEN - 5);
}

#[test]
fn spr_flips() {
    // flip_x mirrors within the 8px sprite cell: column 0 -> column 7.
    let con = spr_console("spr(1, 0, 0, 1, 1, true, false)");
    assert_eq!(px(&con, 7, 0), 1);
    assert_eq!(px(&con, 6, 0), 2);
    assert_eq!(px(&con, 7, 1), 3);

    let con = spr_console("spr(1, 0, 0, 1, 1, false, true)");
    assert_eq!(px(&con, 0, 7), 1);
    assert_eq!(px(&con, 1, 6), 4);

    let con = spr_console("spr(1, 0, 0, 1, 1, true, true)");
    assert_eq!(px(&con, 7, 7), 1);
    assert_eq!(px(&con, 6, 6), 4);
}

#[test]
fn spr_multi_sprite_blocks_and_clipping() {
    // 2x1 block starting at sprite 0 covers sheet columns 0..15, which
    // includes sprite 1's marker at columns 8..9.
    let con = spr_console("spr(0, 0, 0, 2, 1)");
    assert_eq!(px(&con, 8, 0), 1);
    assert_eq!(px(&con, 9, 1), 4);

    // Drawn off the left edge: only the on-screen part appears.
    let con = spr_console("spr(1, -1, -1)");
    assert_eq!(px(&con, 0, 0), 4);
    assert_eq!(count(&con, 1), 0);

    // Entirely off screen: nothing at all.
    let con = spr_console("spr(1, -20, -20) spr(1, 200, 300)");
    assert_eq!(count(&con, 1) + count(&con, 4), 0);

    // Zero/negative sizes are no-ops.
    let con = spr_console("spr(1, 0, 0, 0, 0)");
    assert_eq!(count(&con, 1), 0);
}

#[test]
fn spr_reads_the_right_sheet_row() {
    // Sheet row 7 column 15 is colour 5; that is sprite 1, pixel (7, 7).
    let con = spr_console("spr(1, 100, 100)");
    assert_eq!(px(&con, 107, 107), 5);
}

#[test]
fn print_draws_pixels() {
    let con = draw_once("print(\"HELLO\", 4, 4, 11)");
    let lit = count(&con, 11);
    assert!(lit > 0, "print must draw something");
    // All within the 5x(4*5) cell region it claims.
    for (i, &p) in con.framebuffer().iter().enumerate() {
        if p == 11 {
            let (x, y) = (i % SCREEN_W, i / SCREEN_W);
            assert!((4..4 + 5 * 4).contains(&x) && (4..4 + 5).contains(&y));
        }
    }
    // Default colour is 12.
    let con = draw_once("print(\"HI\", 0, 0)");
    assert!(count(&con, 12) > 0);
    // Lowercase renders (as uppercase glyphs).
    let a = draw_once("print(\"abc\", 0, 0, 7)");
    let b = draw_once("print(\"ABC\", 0, 0, 7)");
    assert_eq!(a.framebuffer(), b.framebuffer());
    // Numbers are accepted and formatted without a trailing .0
    let n = draw_once("print(3.0, 0, 0, 7)");
    let s = draw_once("print(\"3\", 0, 0, 7)");
    assert_eq!(n.framebuffer(), s.framebuffer());
    // Off-screen text clips silently.
    draw_once("print(\"CLIPPED\", -100, -100, 7) print(\"CLIPPED\", 200, 400, 7)");
}

#[test]
fn print_handles_newlines_and_full_ascii() {
    let con = draw_once(
        "print(\" !\\\"#$%&'()*+,-./0123456789:;<=>?@\", 0, 0, 7)\n\
         print(\"ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\\\]^_`\", 0, 8, 7)\n\
         print(\"abcdefghijklmnopqrstuvwxyz{|}~\", 0, 16, 7)\n\
         print(\"two\\nlines\", 0, 30, 5)",
    );
    assert!(count(&con, 7) > 100);
    // The second line of the \n string starts 6px lower.
    assert!(count(&con, 5) > 0);
}

//! The 1024-entry sprite-address space and its per-tile flag bytes.

use console_core::{Cart, Console, SCREEN_W, SHEET_TILES, SHEET_W, TILE_COUNT, TILE_ID_MAX};

fn flag_grid(last: &str) -> String {
    let mut text = String::new();
    for row in 0..SHEET_TILES {
        text.push_str(&"00".repeat(SHEET_TILES - usize::from(row == SHEET_TILES - 1)));
        if row == SHEET_TILES - 1 {
            text.push_str(last);
        }
        text.push('\n');
    }
    text
}

fn cart_with_flags(flags: &str, lua: &str) -> String {
    format!("__lua__\n{lua}\n\n__gfx_flags__\n{flags}")
}

#[test]
fn flags_parse_across_the_complete_atlas() {
    let text = cart_with_flags(&flag_grid("a5"), "x=1");
    let cart = Cart::parse(&text).unwrap();
    assert_eq!(cart.gfx_flags().len(), TILE_COUNT);
    assert!(
        cart.gfx_flags()[..usize::from(TILE_ID_MAX)]
            .iter()
            .all(|&flags| flags == 0)
    );
    assert_eq!(cart.gfx_flags()[usize::from(TILE_ID_MAX)], 0xa5);
}

#[test]
fn fget_and_fset_reach_tile_1023_and_support_byte_or_bit_forms() {
    let text = cart_with_flags(&flag_grid("a5"), "x=1");
    let mut console = Console::new(&text, 0).unwrap();

    assert_eq!(
        console.eval("return fget(1023)").unwrap().as_i64(),
        Some(0xa5)
    );
    assert_eq!(
        console.eval("return fget(1023, 0)").unwrap().as_boolean(),
        Some(true)
    );
    assert_eq!(
        console.eval("return fget(1023, 1)").unwrap().as_boolean(),
        Some(false)
    );

    console.eval("fset(1023, 0x12)").unwrap();
    assert_eq!(
        console.eval("return fget(1023)").unwrap().as_i64(),
        Some(0x12)
    );
    console.eval("fset(1023, 7, true)").unwrap();
    assert_eq!(
        console.eval("return fget(1023)").unwrap().as_i64(),
        Some(0x92)
    );
    console.eval("fset(1023, 1, false)").unwrap();
    assert_eq!(
        console.eval("return fget(1023)").unwrap().as_i64(),
        Some(0x90)
    );

    // A fresh console restores the cart-authored byte.
    let mut reset = Console::new(&text, 0).unwrap();
    assert_eq!(
        reset.eval("return fget(1023)").unwrap().as_i64(),
        Some(0xa5)
    );
}

#[test]
fn flag_api_rejects_invalid_addresses_bits_and_bytes_without_wrapping() {
    let text = cart_with_flags("", "x=1");
    for (lua, expected) in [
        ("return fget(1024)", "fget: tile id must be 0-1023"),
        ("fset(-1, 1)", "fset: tile id must be 0-1023"),
        ("return fget(0, 8)", "fget: flag bit must be 0-7"),
        ("fset(0, -1, true)", "fset: flag bit must be 0-7"),
        ("fset(0, 256)", "fset: flag byte must be 0-255"),
    ] {
        let mut console = Console::new(&text, 0).unwrap();
        let error = console.eval(lua).unwrap_err();
        assert!(error.to_string().contains(expected), "{lua}: {error}");
    }
}

#[test]
fn malformed_flag_rows_name_the_gfx_flags_section() {
    for (body, expected) in [
        ("0", "each flag is 2 hex digits"),
        ("gg", "__gfx_flags__ line 1: expected hex digit"),
        (
            &format!("{}\n", "00".repeat(SHEET_TILES + 1)),
            "flag grid is 32 cells wide",
        ),
        (
            &format!("{}\n", "00\n".repeat(SHEET_TILES + 1)),
            "flag grid is at most 32 rows tall",
        ),
    ] {
        let error = Cart::parse(&cart_with_flags(body, "x=1")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("__gfx_flags__"), "{message}");
        assert!(message.contains(expected), "{message}");
        assert!(
            !message.contains("__map__"),
            "wrong section in diagnostic: {message}"
        );
    }
}

fn sheet_with_last_tile(color: char) -> String {
    let mut sheet = String::with_capacity((SHEET_W + 1) * SHEET_W);
    for y in 0..SHEET_W {
        for x in 0..SHEET_W {
            sheet.push(if x >= SHEET_W - 8 && y >= SHEET_W - 8 {
                color
            } else {
                '0'
            });
        }
        sheet.push('\n');
    }
    sheet
}

#[test]
fn sprite_and_map_drawing_reach_tile_1023() {
    let sheet = sheet_with_last_tile('7');
    for draw in ["spr(1023, 0, 0)", "map(0, 0, 0, 0, 1, 1)"] {
        let map = if draw.starts_with("map") {
            "\n__map__\n# map-format=hex3\n3ff\n"
        } else {
            ""
        };
        let cart =
            format!("__lua__\nfunction _draw() cls(0) {draw} end\n\n__sprites__\n{sheet}{map}");
        let mut console = Console::new(&cart, 0).unwrap();
        console.step(0).unwrap();
        assert_eq!(console.framebuffer()[0], 7, "{draw}");
        assert_eq!(console.framebuffer()[7 * SCREEN_W + 7], 7, "{draw}");
    }
}

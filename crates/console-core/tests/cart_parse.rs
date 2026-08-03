use console_core::{Cart, Error, SHEET_W};

#[test]
fn parses_all_sections() {
    let text = "\
__meta__
title=Test Cart
author = someone
version=0

__lua__
function _init() x = 1 end
-- trailing comment

__sprites__
0123456789abcdef
fedcba9876543210
";
    let cart = Cart::parse(text).unwrap();
    assert_eq!(cart.title(), "Test Cart");
    assert_eq!(cart.meta_get("author"), Some("someone"));
    assert_eq!(cart.meta_get("version"), Some("0"));
    assert_eq!(cart.meta_get("nope"), None);
    assert_eq!(cart.meta().len(), 3);
    assert!(cart.lua().contains("function _init()"));
    assert!(cart.lua().contains("-- trailing comment"));

    // Row 0 is 0..f then zero-padded to the full sheet width.
    for x in 0..16 {
        assert_eq!(cart.sprites()[x], x as u8);
    }
    for x in 16..SHEET_W {
        assert_eq!(cart.sprites()[x], 0);
    }
    // Row 1 is reversed.
    for x in 0..16 {
        assert_eq!(cart.sprites()[SHEET_W + x], 15 - x as u8);
    }
    // Everything past the two provided rows is zero.
    assert!(cart.sprites()[2 * SHEET_W..].iter().all(|&p| p == 0));

    let names: Vec<&str> = cart.section_names().collect();
    assert_eq!(names, vec!["lua", "meta", "sprites"]);
}

#[test]
fn missing_optional_sections_are_defaults() {
    let cart = Cart::parse("__lua__\nx = 1\n").unwrap();
    assert_eq!(cart.title(), "untitled");
    assert!(cart.meta().is_empty());
    assert!(cart.sprites().iter().all(|&p| p == 0));
    assert_eq!(cart.lua().trim(), "x = 1");
}

#[test]
fn missing_lua_section_errors() {
    let err = Cart::parse("__meta__\ntitle=x\n").unwrap_err();
    assert!(matches!(err, Error::Cart(_)));
    assert!(err.to_string().contains("__lua__"));
}

#[test]
fn crlf_and_trailing_whitespace_are_tolerated() {
    let text = "__meta__\r\ntitle=CRLF\r\n\r\n__lua__  \r\nx = 2\r\n\r\n__sprites__\r\nff  \r\n";
    let cart = Cart::parse(text).unwrap();
    assert_eq!(cart.title(), "CRLF");
    assert_eq!(cart.lua(), "x = 2\n\n");
    assert_eq!(cart.sprites()[0], 15);
    assert_eq!(cart.sprites()[1], 15);
    assert_eq!(cart.sprites()[2], 0);
}

#[test]
fn unknown_sections_are_preserved_not_rejected() {
    let text = "__lua__\nx=1\n\n__save__\nsome future data\n\n__gfx_flags__\n01\n";
    let cart = Cart::parse(text).unwrap();
    assert_eq!(cart.section("save").unwrap().trim(), "some future data");
    assert!(cart.section("gfx_flags").is_some());
    assert!(cart.section("nothing").is_none());
    let names: Vec<&str> = cart.section_names().collect();
    assert_eq!(names, vec!["gfx_flags", "lua", "save"]);
}

#[test]
fn text_before_first_header_is_ignored() {
    let cart = Cart::parse("free-form preamble\n\n__lua__\nx=1\n").unwrap();
    assert_eq!(cart.lua().trim(), "x=1");
}

#[test]
fn demo_cart_parses() {
    let cart = Cart::parse(include_str!("../../../carts/demo.cart")).unwrap();
    assert_eq!(cart.title(), "Micro Dash");
    // Sprite 1 (the player) starts at x=8 on sheet row 0.
    assert_ne!(cart.sprites()[8 + 2], 0);
    // Sprite 0 is blank. This is a cart-authoring convention, not an engine
    // requirement -- console-core's `spr()` treats sprite index 0 like any
    // other (see gfx.rs), so nothing in the runtime actually depends on it.
    // The convention exists because color 0 is spr()'s transparent color
    // (SPEC.md "Palette"), so an all-color-0 sprite 0 is a natural,
    // by-construction no-op/placeholder id -- e.g. safe to pass to spr()
    // before an animation table is populated, or to leave unused tile slots
    // pointing at. demo.cart's own sprites all start at tile 1 (see
    // `__gfx_meta__` below: `sprite player rect=1,0 ...`), so this assertion
    // is really "nothing accidentally drew into the reserved slot" -- a
    // regression guard for the shipped demo cart, documented in SKILL.md's
    // sprite-authoring section so other carts can follow the same
    // convention deliberately instead of tripping over it.
    assert!(cart.sprites()[0..8].iter().all(|&p| p == 0));
}

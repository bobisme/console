//! Integration tests for `console-agent sprite gif` (SPEC.md "Sprite &
//! animation authoring (PoC v1)") — an animated preview of an anim, encoded
//! as an infinite-looping GIF at its declared fps.
//!
//! The main assertion drives it against `carts/demo.cart`'s real
//! `player.walk` anim and decodes the result back with the `gif` crate's own
//! reader, so a bug that produces a byte-valid-but-wrong GIF (bad dimensions,
//! dropped/duplicated frames) would be caught, not just "it wrote some
//! bytes".

use console_agent::sprite::view::{self, GifOpts};
use console_core::{Cart, PALETTE};

fn demo_cart() -> Cart {
    let path = format!("{}/../../carts/demo.cart", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    Cart::parse(&text).expect("demo.cart parses")
}

/// Decode `bytes` as a GIF and return `(width, height, frame_count)`.
fn decode(bytes: &[u8]) -> (u16, u16, usize) {
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = opts
        .read_info(std::io::Cursor::new(bytes))
        .expect("valid gif header");
    let (w, h) = (decoder.width(), decoder.height());
    let mut n = 0;
    while decoder.read_next_frame().expect("decode frame").is_some() {
        n += 1;
    }
    (w, h, n)
}

#[test]
fn gif_of_demo_cart_player_walk_decodes_to_the_right_shape() {
    let cart = demo_cart();
    // player.walk: 4 frames, 1x1-tile (8 sheet px) sprite, fps=8, loop.
    let opts = GifOpts {
        zoom: 4,
        ..GifOpts::default()
    };
    let out = view::gif(&cart, "player.walk", &opts).expect("encode gif");

    assert_eq!(out.frames, 4);
    assert_eq!((out.width, out.height), (32, 32));

    let (w, h, n) = decode(&out.bytes);
    assert_eq!(
        (w, h),
        (32, 32),
        "decoded dimensions must match what we asked to encode"
    );
    assert_eq!(n, 4, "decoded frame count must match the anim's frame list");
}

#[test]
fn gif_defaults_to_zoom_8() {
    let cart = demo_cart();
    let out = view::gif(&cart, "player.walk", &GifOpts::default()).expect("encode gif");
    assert_eq!((out.width, out.height), (64, 64));
    let (w, h, _) = decode(&out.bytes);
    assert_eq!((w, h), (64, 64));
}

#[test]
fn gif_delay_matches_declared_fps() {
    let cart = demo_cart();
    // fps=8 -> 125ms/frame -> 12.5, rounds to 13 hundredths (130ms).
    let out = view::gif(
        &cart,
        "player.walk",
        &GifOpts {
            zoom: 2,
            ..GifOpts::default()
        },
    )
    .expect("encode gif");

    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = opts
        .read_info(std::io::Cursor::new(&out.bytes))
        .expect("valid gif header");
    let frame = decoder
        .read_next_frame()
        .expect("decode frame")
        .expect("at least one frame");
    assert_eq!(frame.delay, 13, "125ms rounds to 13 hundredths of a second");
}

#[test]
fn gif_grid_and_anchor_change_the_encoded_pixels() {
    let cart = demo_cart();
    let plain = view::gif(
        &cart,
        "player.walk",
        &GifOpts {
            zoom: 8,
            ..GifOpts::default()
        },
    )
    .expect("plain gif");
    let grid = view::gif(
        &cart,
        "player.walk",
        &GifOpts {
            zoom: 8,
            grid: true,
            ..GifOpts::default()
        },
    )
    .expect("grid gif");
    let anchored = view::gif(
        &cart,
        "player.walk",
        &GifOpts {
            zoom: 8,
            anchor: true,
            ..GifOpts::default()
        },
    )
    .expect("anchor gif");

    assert_ne!(
        plain.bytes, grid.bytes,
        "--grid must change the encoded frames"
    );
    assert_ne!(
        plain.bytes, anchored.bytes,
        "--anchor must change the encoded frames"
    );
    // Overlays don't change the animation's shape, only its pixels.
    assert_eq!(plain.frames, grid.frames);
    assert_eq!((plain.width, plain.height), (grid.width, grid.height));
}

#[test]
fn gif_uses_the_cart_preview_palette() {
    let cart = Cart::parse(
        "__meta__\npreview_palette=30,1,2,31\n\n__lua__\n\n__sprites__\n033\n033\n\n__gfx_meta__\nsprite p rect=0,0 size=1x1\nanim p.idle frames=0 fps=1 loop\n",
    )
    .unwrap();
    let out = view::gif(
        &cart,
        "p.idle",
        &GifOpts {
            zoom: 1,
            ..GifOpts::default()
        },
    )
    .unwrap();

    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = opts.read_info(std::io::Cursor::new(out.bytes)).unwrap();
    let frame = decoder.read_next_frame().unwrap().unwrap();
    assert_ne!(&frame.buffer[..3], &PALETTE[30]);
    assert_eq!(&frame.buffer[4..7], &PALETTE[31]);
}

#[test]
fn gif_rejects_a_non_anim_target() {
    let cart = demo_cart();
    let err = view::gif(&cart, "player", &GifOpts::default()).unwrap_err();
    assert!(err.contains("is not an anim"), "unexpected error: {err}");
}

#[test]
fn cli_sprite_gif_writes_a_decodable_file() {
    let out_path = std::env::temp_dir().join(format!(
        "console-agent-sprite-gif-cli-{}.gif",
        std::process::id()
    ));
    let cart_path = format!("{}/../../carts/demo.cart", env!("CARGO_MANIFEST_DIR"));

    let bin = env!("CARGO_BIN_EXE_console-agent");
    let output = std::process::Command::new(bin)
        .args([
            "sprite",
            "gif",
            &cart_path,
            "player.walk",
            "--zoom",
            "4",
            "-o",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("spawn console-agent");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bytes = std::fs::read(&out_path).expect("read gif output");
    let (w, h, n) = decode(&bytes);
    assert_eq!((w, h, n), (32, 32, 4));
    let _ = std::fs::remove_file(&out_path);
}

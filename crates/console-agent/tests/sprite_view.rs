//! Tests for the sprite inspection tools (`console_agent::sprite::view`) and
//! their RPC mirrors.
//!
//! Everything runs against a hand-built cart declared inline below, so the
//! expected numbers can be worked out on paper rather than read back out of
//! the code under test. The sheet holds three 1x1-tile frames of the sprite
//! `dot` (anchor 4,7), laid out left to right in tiles 0, 1 and 2:
//!
//! ```text
//!   frame 0            frame 1                 frame 2
//!   (1,1)=3 (2,1)=3    same three, plus        (3,3)=5 (4,3)=5
//!   (1,2)=3            (5,5)=9 (6,5)=9 (6,6)=9
//!   area 3             area 6                  area 2
//! ```
//!
//! So frame 0 -> frame 1 changes exactly three pixels, color 9 lives in
//! frame 1 alone, and no frame's silhouette overlaps frame 2's.

use console_agent::rpc::handle;
use console_agent::session::Session;
use console_agent::sprite::view::{self, Image, RenderOpts};
use console_core::{Cart, PALETTE};
use serde_json::{Value, json};

const CART: &str = "\
__meta__
title=Sprite View Test

__lua__
function _init() end
function _update() end
function _draw() end

__sprites__
000000000000000000000000
033000000330000000000000
030000000300000000000000
000000000000000000055000
000000000000000000000000
000000000000099000000000
000000000000009000000000
000000000000000000000000

__gfx_meta__
sprite dot rect=0,0 size=1x1 anchor=4,7
anim dot.wave frames=0,1 fps=4 loop
anim dot.once frames=0,1 fps=4
anim dot.tri frames=0,1,2 fps=4 loop
";

const CHECKER_A: [u8; 3] = [0x14, 0x16, 0x1f];
const CHECKER_B: [u8; 3] = [0x1d, 0x21, 0x30];

fn cart() -> Cart {
    Cart::parse(CART).expect("test cart parses")
}

fn px(img: &Image, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
}

/// The colour of sheet pixel `(sx, sy)`, sampled in the middle of the
/// `zoom`x`zoom` block it was blown up into.
fn cell(img: &Image, zoom: u32, sx: u32, sy: u32) -> [u8; 3] {
    px(img, sx * zoom + zoom / 2, sy * zoom + zoom / 2)
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("console-agent-sprite-{}-{name}", std::process::id()))
}

fn approx(value: &Value, expected: f64) {
    let got = value.as_f64().unwrap_or_else(|| panic!("{value} is not a number"));
    assert!(
        (got - expected).abs() < 1e-9,
        "expected {expected}, got {got}"
    );
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

#[test]
fn render_dimensions_and_checkerboard() {
    let cart = cart();
    let img = view::render(&cart, "dot", &RenderOpts::default()).expect("render dot");

    // 1x1 tile at the default zoom of 8: 8 sheet pixels * 8 = 64.
    assert_eq!((img.width, img.height), (64, 64));

    // Color 0 lets the checkerboard through, in 4-sheet-pixel cells (32
    // device pixels at zoom 8), so the corners alternate tone.
    assert_eq!(px(&img, 0, 0), CHECKER_A);
    assert_eq!(px(&img, 32, 0), CHECKER_B);
    assert_eq!(px(&img, 0, 32), CHECKER_B);
    assert_eq!(px(&img, 32, 32), CHECKER_A);
    // (3,1) is empty, still inside the first checker cell.
    assert_eq!(px(&img, 24, 8), CHECKER_A);

    // Sheet pixel (1,1) is palette 3, filling the whole 8x8 block.
    assert_eq!(px(&img, 8, 8), PALETTE[3]);
    assert_eq!(px(&img, 15, 15), PALETTE[3]);
    assert_eq!(px(&img, 16, 8), PALETTE[3]); // (2,1)
    assert_eq!(px(&img, 8, 16), PALETTE[3]); // (1,2)

    assert_eq!(img.frames, 1);
}

#[test]
fn render_png_matches_the_rgba_buffer() {
    let cart = cart();
    let img = view::render(&cart, "dot", &RenderOpts::default()).expect("render dot");

    let decoder = png::Decoder::new(std::io::Cursor::new(&img.png));
    let mut reader = decoder.read_info().expect("valid png header");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("known buffer size")];
    let info = reader.next_frame(&mut buf).expect("decode png frame");
    assert_eq!((info.width, info.height), (64, 64));
    assert_eq!(&buf[..info.buffer_size()], &img.rgba[..]);
}

#[test]
fn render_zoom_scales_the_image() {
    let cart = cart();
    let opts = RenderOpts {
        zoom: 12,
        ..RenderOpts::default()
    };
    let img = view::render(&cart, "dot", &opts).expect("render at zoom 12");
    assert_eq!((img.width, img.height), (96, 96));
    assert_eq!(px(&img, 12, 12), PALETTE[3]); // sheet (1,1)

    let bad = view::render(&cart, "dot", &RenderOpts { zoom: 0, ..opts });
    assert!(bad.unwrap_err().contains("zoom must be"));
}

#[test]
fn render_grid_adds_the_closing_boundary() {
    let cart = cart();
    let opts = RenderOpts {
        grid: true,
        ..RenderOpts::default()
    };
    let img = view::render(&cart, "dot", &opts).expect("render with grid");
    // One extra device pixel so the right/bottom tile boundary is drawn.
    assert_eq!((img.width, img.height), (65, 65));
    // Tile boundaries at x = 0 and x = 64 differ from the bare checkerboard.
    assert_ne!(px(&img, 0, 40), CHECKER_B);
    assert_ne!(px(&img, 64, 40), px(&img, 40, 40));
}

#[test]
fn render_indices_draws_glyph_pixels_only_from_zoom_6() {
    let cart = cart();
    let plain = view::render(&cart, "dot", &RenderOpts::default()).expect("plain");
    let labelled = view::render(
        &cart,
        "dot",
        &RenderOpts {
            indices: true,
            ..RenderOpts::default()
        },
    )
    .expect("indices");
    let changed = plain
        .rgba
        .iter()
        .zip(&labelled.rgba)
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > 0, "--indices drew nothing at zoom 8");

    // 3x5 glyphs do not fit below zoom 6, so the flag is silently a no-op.
    let small = RenderOpts {
        zoom: 5,
        ..RenderOpts::default()
    };
    let small_plain = view::render(&cart, "dot", &small).expect("small plain");
    let small_labelled = view::render(
        &cart,
        "dot",
        &RenderOpts {
            indices: true,
            ..small
        },
    )
    .expect("small indices");
    assert_eq!(small_plain.rgba, small_labelled.rgba);
}

#[test]
fn render_anchor_draws_a_color_4_crosshair() {
    let cart = cart();
    let plain = view::render(&cart, "dot", &RenderOpts::default()).expect("plain");
    let crossed = view::render(
        &cart,
        "dot",
        &RenderOpts {
            anchor: true,
            ..RenderOpts::default()
        },
    )
    .expect("anchor");

    let has_color_4 = |img: &Image| {
        (0..img.height).any(|y| (0..img.width).any(|x| px(img, x, y) == PALETTE[4]))
    };
    assert!(!has_color_4(&plain));
    assert!(has_color_4(&crossed));
    // The anchor is (4,7): the crosshair centre is inside that pixel cell.
    assert_eq!(px(&crossed, 4 * 8 + 4, 7 * 8 + 4), PALETTE[4]);
}

#[test]
fn render_accepts_raw_rects_but_they_have_no_anchor() {
    let cart = cart();
    let img = view::render(&cart, "0,0,1,1", &RenderOpts::default()).expect("rect render");
    assert_eq!((img.width, img.height), (64, 64));
    assert_eq!(px(&img, 8, 8), PALETTE[3]);

    let err = view::render(
        &cart,
        "0,0,1,1",
        &RenderOpts {
            anchor: true,
            ..RenderOpts::default()
        },
    )
    .unwrap_err();
    assert!(err.contains("anchor"), "unexpected error: {err}");
}

#[test]
fn render_frame_of_an_anim_indexes_the_anim_frame_list() {
    let cart = cart();
    // dot.tri frame 2 is the sprite's third frame: (3,3)=5, (4,3)=5.
    let img = view::render(
        &cart,
        "dot.tri",
        &RenderOpts {
            frame: Some(2),
            ..RenderOpts::default()
        },
    )
    .expect("render dot.tri frame 2");
    assert_eq!(px(&img, 3 * 8, 3 * 8), PALETTE[5]);
    assert_eq!(px(&img, 4 * 8, 3 * 8), PALETTE[5]);
    assert_eq!(px(&img, 8, 8), CHECKER_A); // frame 0's pixel is not here

    let err = view::render(
        &cart,
        "dot.tri",
        &RenderOpts {
            frame: Some(3),
            ..RenderOpts::default()
        },
    )
    .unwrap_err();
    assert!(err.contains("out of range"), "unexpected error: {err}");
}

// ---------------------------------------------------------------------------
// strip
// ---------------------------------------------------------------------------

#[test]
fn strip_lays_frames_out_with_2px_separators() {
    let cart = cart();
    let img = view::strip(&cart, "dot.wave", 4, false).expect("strip dot.wave");

    // 2 frames * (8 sheet px * zoom 4) + 1 separator of 2px.
    assert_eq!(img.width, 2 * 8 * 4 + 2);
    assert_eq!(img.height, 8 * 4);
    assert_eq!(img.frames, 2);

    // The separator is a solid dark gutter, not sprite content.
    assert_eq!(px(&img, 32, 0), [0x0a, 0x0b, 0x10]);
    assert_eq!(px(&img, 33, 31), [0x0a, 0x0b, 0x10]);

    // Both frames' shared (1,1) pixel sits at the same y in each cell.
    assert_eq!(px(&img, 4, 4), PALETTE[3]);
    assert_eq!(px(&img, 34 + 4, 4), PALETTE[3]);
    // Frame 1's exclusive (5,5) pixel only exists in the second cell.
    assert_eq!(px(&img, 34 + 20, 20), PALETTE[9]);
    assert_eq!(px(&img, 20, 20), CHECKER_A);
}

#[test]
fn strip_anchor_marks_every_frame() {
    let cart = cart();
    let img = view::strip(&cart, "dot.wave", 4, true).expect("strip with anchor");
    // Anchor (4,7) centre in each cell, at zoom 4.
    assert_eq!(px(&img, 4 * 4 + 2, 7 * 4 + 2), PALETTE[4]);
    assert_eq!(px(&img, 34 + 4 * 4 + 2, 7 * 4 + 2), PALETTE[4]);
}

// ---------------------------------------------------------------------------
// onion
// ---------------------------------------------------------------------------

fn is_reddish(p: [u8; 3]) -> bool {
    p[0] > 90 && p[1] < 40 && p[2] < 40
}

fn is_greenish(p: [u8; 3]) -> bool {
    p[1] > 90 && p[0] < 40 && p[2] < 40
}

#[test]
fn onion_tints_neighbours_red_and_green_around_a_solid_frame() {
    let cart = cart();
    // dot.tri loops, so frame 0's neighbours are frame 2 (prev) and 1 (next).
    let img = view::onion(&cart, "dot.tri", 0, 4).expect("onion dot.tri 0");
    assert_eq!((img.width, img.height), (32, 32));
    assert_eq!(img.frames, 3);

    // Solid frame 0 at full opacity. Frame 1 (the green ghost) owns these
    // same three pixels, so this also pins down "the solid frame wins".
    assert_eq!(cell(&img, 4, 1, 1), PALETTE[3]);
    assert_eq!(cell(&img, 4, 2, 1), PALETTE[3]);
    assert_eq!(cell(&img, 4, 1, 2), PALETTE[3]);

    // Previous frame (2) is red where the solid frame has nothing.
    let p = cell(&img, 4, 3, 3);
    assert!(is_reddish(p), "prev ghost should be red, got {p:?}");
    assert!(is_reddish(cell(&img, 4, 4, 3)));

    // Next frame (1) is green on its exclusive pixels.
    let p = cell(&img, 4, 5, 5);
    assert!(is_greenish(p), "next ghost should be green, got {p:?}");
    assert!(is_greenish(cell(&img, 4, 6, 6)));

    // Ghosts skip color 0: an empty cell keeps the checkerboard.
    assert_eq!(cell(&img, 4, 7, 0), CHECKER_B);
}

#[test]
fn onion_clamps_at_the_ends_of_a_non_looping_anim() {
    let cart = cart();
    // dot.once does not loop, so frame 0 has no previous frame at all.
    let img = view::onion(&cart, "dot.once", 0, 4).expect("onion dot.once 0");
    assert_eq!(img.frames, 2, "solid + next only");
    let any_red = (0..img.height).any(|y| (0..img.width).any(|x| is_reddish(px(&img, x, y))));
    assert!(!any_red, "a clamped first frame must have no red ghost");
    let any_green = (0..img.height).any(|y| (0..img.width).any(|x| is_greenish(px(&img, x, y))));
    assert!(any_green, "frame 1 should still ghost in green");

    let err = view::onion(&cart, "dot.once", 2, 4).unwrap_err();
    assert!(err.contains("out of range"), "unexpected error: {err}");
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

#[test]
fn diff_marks_exactly_the_changed_pixels_in_magenta() {
    let cart = cart();
    let img = view::diff(&cart, "dot.wave", 0, 1, 2).expect("diff dot.wave 0 1");
    assert_eq!((img.width, img.height), (16, 16));

    // The three pixels frame 1 adds over frame 0.
    let changed = [(5u32, 5u32), (6, 5), (6, 6)];
    for sy in 0..8 {
        for sx in 0..8 {
            let expect_magenta = changed.contains(&(sx, sy));
            for dy in 0..2 {
                for dx in 0..2 {
                    let p = px(&img, sx * 2 + dx, sy * 2 + dy);
                    assert_eq!(
                        p == [255, 0, 255],
                        expect_magenta,
                        "sheet pixel ({sx},{sy}) rendered {p:?}"
                    );
                }
            }
        }
    }

    // Unchanged frame-B content is drawn at ~35% brightness: palette 3 is
    // (239,125,87) -> (84,44,30).
    assert_eq!(cell(&img, 2, 1, 1), [84, 44, 30]);
    // Empty in both frames -> untouched checkerboard.
    assert_eq!(px(&img, 0, 0), CHECKER_A);
}

// ---------------------------------------------------------------------------
// ghost
// ---------------------------------------------------------------------------

#[test]
fn ghost_overlays_every_frame_at_low_alpha() {
    let cart = cart();
    let img = view::ghost(&cart, "dot.tri", 4).expect("ghost dot.tri");
    assert_eq!((img.width, img.height), (32, 32));
    assert_eq!(img.frames, 3);

    // Each frame is composited at the same low alpha, so a pixel two frames
    // share ends up with two applications of ink and a lone pixel with one.
    let alpha = 0.85f32 / 3.0;
    let mix = |dst: u8, src: u8| {
        (f32::from(dst) * (1.0 - alpha) + f32::from(src) * alpha).round() as u8
    };
    let over = |dst: [u8; 3], src: [u8; 3]| {
        [
            mix(dst[0], src[0]),
            mix(dst[1], src[1]),
            mix(dst[2], src[2]),
        ]
    };

    let once3 = over(CHECKER_A, PALETTE[3]);
    let twice3 = over(once3, PALETTE[3]);
    assert_eq!(cell(&img, 4, 1, 1), twice3, "shared by frames 0 and 1");
    assert_eq!(cell(&img, 4, 1, 2), twice3, "shared by frames 0 and 1");
    assert_eq!(cell(&img, 4, 3, 3), over(CHECKER_A, PALETTE[5]), "frame 2 only");
    assert_eq!(cell(&img, 4, 5, 5), over(CHECKER_A, PALETTE[9]), "frame 1 only");

    assert!(twice3[0] > once3[0], "overlap must accumulate");
    assert_ne!(twice3, PALETTE[3], "ghosts are never fully opaque");
}

// ---------------------------------------------------------------------------
// lint
// ---------------------------------------------------------------------------

#[test]
fn lint_numbers_match_the_hand_built_frames() {
    let cart = cart();
    let out = view::lint(&cart, &["dot.wave".to_string()]).expect("lint dot.wave");
    let anim = &out["anims"][0];

    assert_eq!(anim["anim"], "dot.wave");
    assert_eq!(anim["sprite"], "dot");
    assert_eq!(anim["fps"], 4);
    assert_eq!(anim["loop"], true);
    assert_eq!(anim["anchor"], json!([4, 7]));
    assert_eq!(anim["frame_size"], json!([8, 8]));

    let f0 = &anim["frames"][0];
    assert_eq!(f0["index"], 0);
    assert_eq!(f0["sprite_frame"], 0);
    assert_eq!(f0["silhouette_area"], 3);
    // Pixels (1,1),(2,1),(1,2) minus anchor (4,7).
    assert_eq!(
        f0["bbox"],
        json!({"x0": -3, "y0": -6, "x1": -2, "y1": -5, "w": 2, "h": 2})
    );
    // Centroid (4/3, 4/3) - (4,7).
    approx(&f0["centroid"][0], -2.67);
    approx(&f0["centroid"][1], -5.67);
    assert_eq!(f0["palette"], json!({"0": 61, "3": 3}));

    let f1 = &anim["frames"][1];
    assert_eq!(f1["silhouette_area"], 6);
    assert_eq!(
        f1["bbox"],
        json!({"x0": -3, "y0": -6, "x1": 2, "y1": -1, "w": 6, "h": 6})
    );
    // Centroid (21/6, 20/6) - (4,7).
    approx(&f1["centroid"][0], -0.5);
    approx(&f1["centroid"][1], -3.67);
    assert_eq!(f1["palette"], json!({"0": 58, "3": 3, "9": 3}));

    // Color 9 exists in frame 1 only; color 3 is in both, so it is not listed.
    assert_eq!(
        anim["colors_unique_to_single_frame"],
        json!([{"color": 9, "frame": 1, "count": 3}])
    );
}

#[test]
fn lint_pairs_are_loop_aware() {
    let cart = cart();
    let out = view::lint(&cart, &["dot.wave".to_string(), "dot.once".to_string()])
        .expect("lint two anims");

    let looped = &out["anims"][0]["pairs"];
    let pairs = looped.as_array().expect("pairs array");
    assert_eq!(pairs.len(), 2, "a looped 2-frame anim wraps: 0->1 and 1->0");

    let forward = &pairs[0];
    assert_eq!(forward["from"], 0);
    assert_eq!(forward["to"], 1);
    assert_eq!(forward["changed_pixels"], 3);
    approx(&forward["area_drift_pct"], 100.0); // 3 -> 6 pixels
    approx(&forward["centroid_drift"]["dx"], 2.17);
    approx(&forward["centroid_drift"]["dy"], 2.0);
    approx(&forward["centroid_drift"]["distance"], 2.95);
    assert_eq!(
        forward["bbox_drift"],
        json!({"dx0": 0, "dy0": 0, "dx1": 4, "dy1": 4, "dw": 4, "dh": 4})
    );

    let wrap = &pairs[1];
    assert_eq!(wrap["from"], 1);
    assert_eq!(wrap["to"], 0);
    assert_eq!(wrap["changed_pixels"], 3);
    approx(&wrap["area_drift_pct"], -50.0); // 6 -> 3 pixels

    // The non-looping anim stops at the last consecutive pair.
    let once = out["anims"][1]["pairs"].as_array().expect("pairs array");
    assert_eq!(once.len(), 1);
    assert_eq!(once[0]["from"], 0);
    assert_eq!(once[0]["to"], 1);
}

#[test]
fn lint_with_no_names_reports_every_anim() {
    let cart = cart();
    let out = view::lint(&cart, &[]).expect("lint all");
    let names: Vec<&str> = out["anims"]
        .as_array()
        .expect("anims array")
        .iter()
        .map(|a| a["anim"].as_str().expect("anim name"))
        .collect();
    assert_eq!(names, ["dot.once", "dot.tri", "dot.wave"]);
}

// ---------------------------------------------------------------------------
// target errors
// ---------------------------------------------------------------------------

#[test]
fn anim_only_commands_reject_sprites_and_rects() {
    let cart = cart();
    for target in ["dot", "0,0,1,1"] {
        let err = view::strip(&cart, target, 8, false).unwrap_err();
        assert!(err.contains("is not an anim"), "strip {target}: {err}");
        let err = view::ghost(&cart, target, 8).unwrap_err();
        assert!(err.contains("is not an anim"), "ghost {target}: {err}");
        let err = view::lint(&cart, &[target.to_string()]).unwrap_err();
        assert!(err.contains("is not an anim"), "lint {target}: {err}");
    }
}

// ---------------------------------------------------------------------------
// RPC
// ---------------------------------------------------------------------------

fn loaded_session() -> Session {
    let mut session = Session::new();
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 0, "method": "load_cart", "params": {"text": CART}}),
    );
    assert!(resp.get("error").is_none(), "load_cart failed: {resp}");
    session
}

fn call(session: &mut Session, method: &str, params: Value) -> Value {
    handle(
        session,
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
    )
}

#[test]
fn rpc_sprite_verbs_write_images_and_report_sizes() {
    let mut session = loaded_session();

    let path = temp_path("render.png");
    let p = path.to_str().expect("utf-8 path");
    let resp = call(
        &mut session,
        "sprite_render",
        json!({"target": "dot", "zoom": 4, "grid": true, "indices": true, "anchor": true, "path": p}),
    );
    assert!(resp.get("error").is_none(), "sprite_render: {resp}");
    assert_eq!(resp["result"]["ok"], true);
    assert_eq!(resp["result"]["width"], 33);
    assert_eq!(resp["result"]["height"], 33);
    assert!(std::fs::metadata(&path).expect("render png exists").len() > 0);
    let _ = std::fs::remove_file(&path);

    let path = temp_path("strip.png");
    let p = path.to_str().expect("utf-8 path");
    let resp = call(
        &mut session,
        "sprite_strip",
        json!({"anim": "dot.wave", "zoom": 4, "anchor": true, "path": p}),
    );
    assert_eq!(resp["result"]["width"], 66);
    assert_eq!(resp["result"]["frames"], 2);
    let _ = std::fs::remove_file(&path);

    let path = temp_path("onion.png");
    let p = path.to_str().expect("utf-8 path");
    let resp = call(
        &mut session,
        "sprite_onion",
        json!({"anim": "dot.tri", "frame": 1, "zoom": 4, "path": p}),
    );
    assert_eq!(resp["result"]["frames"], 3);
    let _ = std::fs::remove_file(&path);

    let path = temp_path("diff.png");
    let p = path.to_str().expect("utf-8 path");
    let resp = call(
        &mut session,
        "sprite_diff",
        json!({"anim": "dot.wave", "frame_a": 0, "frame_b": 1, "zoom": 2, "path": p}),
    );
    assert_eq!(resp["result"]["width"], 16);
    let _ = std::fs::remove_file(&path);

    let path = temp_path("ghost.png");
    let p = path.to_str().expect("utf-8 path");
    let resp = call(
        &mut session,
        "sprite_ghost",
        json!({"anim": "dot.tri", "zoom": 4, "path": p}),
    );
    assert_eq!(resp["result"]["frames"], 3);
    let _ = std::fs::remove_file(&path);

    // lint comes back inline, no file involved.
    let resp = call(&mut session, "sprite_lint", json!({"anims": ["dot.wave"]}));
    assert!(resp.get("error").is_none(), "sprite_lint: {resp}");
    assert_eq!(resp["result"]["anims"][0]["anim"], "dot.wave");
    assert_eq!(resp["result"]["anims"][0]["frames"][1]["silhouette_area"], 6);

    let resp = call(&mut session, "sprite_lint", json!({}));
    assert_eq!(
        resp["result"]["anims"].as_array().expect("anims").len(),
        3,
        "no \"anims\" param means every anim"
    );
}

#[test]
fn rpc_sprite_verbs_report_bad_params_and_missing_carts() {
    let mut session = loaded_session();

    let resp = call(&mut session, "sprite_render", json!({"path": "/dev/null"}));
    assert_eq!(resp["error"]["code"], -32602);
    assert!(
        resp["error"]["message"]
            .as_str()
            .expect("message")
            .contains("target")
    );

    let resp = call(&mut session, "sprite_strip", json!({"anim": "dot", "path": "/dev/null"}));
    assert_eq!(resp["error"]["code"], -32602);

    let resp = call(
        &mut session,
        "sprite_diff",
        json!({"anim": "dot.wave", "frame_a": 0, "path": "/dev/null"}),
    );
    assert_eq!(resp["error"]["code"], -32602);

    // No cart loaded at all.
    let mut empty = Session::new();
    for (method, params) in [
        ("sprite_render", json!({"target": "dot", "path": "/dev/null"})),
        ("sprite_strip", json!({"anim": "dot.wave", "path": "/dev/null"})),
        ("sprite_onion", json!({"anim": "dot.wave", "path": "/dev/null"})),
        (
            "sprite_diff",
            json!({"anim": "dot.wave", "frame_a": 0, "frame_b": 1, "path": "/dev/null"}),
        ),
        ("sprite_ghost", json!({"anim": "dot.wave", "path": "/dev/null"})),
        ("sprite_lint", json!({})),
    ] {
        let resp = call(&mut empty, method, params);
        assert_eq!(resp["error"]["code"], -32002, "{method}: {resp}");
    }
}

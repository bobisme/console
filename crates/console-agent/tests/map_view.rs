//! Tests for the map inspection tools (`console_agent::map::view`) and their
//! RPC mirrors — SPEC.md "Tile map authoring" plus this bone's map-agent-
//! tooling addition.
//!
//! The fixture cart below declares tiles 1-3 on the sheet (tile 0 stays the
//! conventional blank/empty id) and a tiny 4x2-cell map using them:
//!
//! ```text
//!   sheet: tile1 = solid color 3, tile2 = solid color 5, tile3 = BLANK
//!          (referenced by the map but never drawn -- the lint "typo" case)
//!
//!   map row0: 001 001 000 002 (cells (0,0)=1 (1,0)=1 (2,0)=0 (3,0)=2)
//!   map row1: 003 000 000 000 (cell  (0,1)=3, rest implicit/explicit 0)
//! ```
//!
//! So the used extent is `cx=0 cy=0 cw=4 ch=2`, tile 1 appears twice, tiles
//! 2 and 3 once each, and tile 3 is the one "blank sprite" reference.

use console_agent::map::view::{self, MapRenderOpts};
use console_agent::rpc::handle;
use console_agent::session::Session;
use console_core::{Cart, MAP_FORMAT_MARKER, PALETTE};
use serde_json::{Value, json};

/// A 128-char sheet row with `segments` (byte offset, hex text) overlaid on
/// an all-zero background, matching `sprite::transform`'s full-row rewrite
/// convention.
fn row128(segments: &[(usize, &str)]) -> String {
    let mut chars = vec!['0'; 128];
    for (offset, text) in segments {
        for (i, c) in text.chars().enumerate() {
            chars[offset + i] = c;
        }
    }
    chars.into_iter().collect()
}

fn fixture_cart() -> String {
    // Tile n's pixels sit at sheet x = n*8..n*8+8 within these 8 rows (y=0..8).
    // Tile 1 = color 3 (offset 8), tile 2 = color 5 (offset 16), tile 3 is
    // left at the all-zero background -- referenced by the map but blank.
    let sheet_row = row128(&[(8, "33333333"), (16, "55555555")]);
    let mut s = String::from("__lua__\nfunction _init() end\n\n__sprites__\n");
    for _ in 0..8 {
        s.push_str(&sheet_row);
        s.push('\n');
    }
    s.push_str("__map__\n# map-format=hex3\n001001000002\n003\n");
    s
}

fn cart() -> Cart {
    Cart::parse(&fixture_cart()).expect("fixture cart parses")
}

fn preview_cart() -> Cart {
    Cart::parse(&fixture_cart().replacen(
        "__lua__",
        "__meta__\npreview_palette=30,1,2,31,4,14\n\n__lua__",
        1,
    ))
    .expect("preview palette fixture parses")
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("console-map-view-{}-{name}", std::process::id()))
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

#[test]
fn render_default_region_is_the_used_extent() {
    let cart = cart();
    let img = view::render(
        &cart,
        console_agent::map::parse_region(None, cart.map()).unwrap(),
        &MapRenderOpts::default(),
    )
    .expect("render default region");

    // used extent is 4x2 cells at zoom 8: 4*8*8 x 2*8*8.
    assert_eq!((img.width, img.height), (256, 128));
    assert_eq!(img.frames, 1);
}

#[test]
fn render_paints_known_tiles_and_skips_tile_zero() {
    let cart = cart();
    let region = (0, 0, 4, 2);
    let img = view::render(&cart, region, &MapRenderOpts::default()).expect("render");

    let sample = |cx: u32, cy: u32| {
        let x = cx * 64 + 32;
        let y = cy * 64 + 32;
        let i = ((y * img.width + x) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
    };

    assert_eq!(sample(0, 0), PALETTE[3], "cell (0,0) is tile 1 -> color 3");
    assert_eq!(sample(1, 0), PALETTE[3], "cell (1,0) is tile 1 -> color 3");
    assert_eq!(sample(3, 0), PALETTE[5], "cell (3,0) is tile 2 -> color 5");

    // Tile 0 (cell 2,0) is skipped: checkerboard, not palette color 0.
    let empty = sample(2, 0);
    assert_ne!(empty, PALETTE[3]);
    assert_ne!(empty, PALETTE[5]);

    // Cell (0,1) is tile 3, which is referenced but its sheet art is blank
    // (all color 0). It is NOT skipped at the cell level (tile != 0), but
    // every one of its pixels is individually transparent, so it *looks*
    // identical to an empty cell -- exactly the "probable typo" case `lint`
    // calls out under `blank_sprite_tiles`.
    assert_eq!(
        sample(0, 1),
        empty,
        "blank-sprite tile 3 looks like an empty cell"
    );
}

#[test]
fn render_uses_preview_palette_while_map_dump_stays_raw() {
    let cart = preview_cart();
    let img = view::render(&cart, (0, 0, 4, 2), &MapRenderOpts::default()).unwrap();
    let sample = |cx: u32, cy: u32| {
        let i = (((cy * 64 + 32) * img.width + cx * 64 + 32) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
    };
    assert_eq!(sample(0, 0), PALETTE[31]);
    assert_ne!(
        sample(2, 0),
        PALETTE[30],
        "tile/source zero stays transparent"
    );
    assert_eq!(sample(3, 0), PALETTE[14]);
    assert_eq!(
        sample(0, 1),
        sample(2, 0),
        "nonzero tile 3 contains source color 0, which must remain transparent"
    );
    assert_eq!(
        view::dump(&cart, (0, 0, 4, 1)).unwrap(),
        "# map-format=hex3\n# cx=0 cy=0 cw=4 ch=1\n001001000002\n"
    );
}

#[test]
fn render_zoom_and_grid() {
    let cart = cart();
    let region = (0, 0, 1, 1);
    let plain = view::render(&cart, region, &MapRenderOpts::default()).expect("plain");
    assert_eq!((plain.width, plain.height), (64, 64));

    let zoomed = view::render(
        &cart,
        region,
        &MapRenderOpts {
            zoom: 4,
            ..MapRenderOpts::default()
        },
    )
    .expect("zoom 4");
    assert_eq!((zoomed.width, zoomed.height), (32, 32));

    let grid = view::render(
        &cart,
        region,
        &MapRenderOpts {
            grid: true,
            ..MapRenderOpts::default()
        },
    )
    .expect("grid");
    // One extra device pixel closes the boundary, same convention as
    // `sprite render --grid`.
    assert_eq!((grid.width, grid.height), (65, 65));

    let bad = view::render(
        &cart,
        region,
        &MapRenderOpts {
            zoom: 0,
            ..MapRenderOpts::default()
        },
    );
    assert!(bad.unwrap_err().contains("zoom must be"));
}

#[test]
fn render_ids_overlay_changes_only_nonempty_cells() {
    let cart = cart();
    let region = (0, 0, 4, 2);
    let plain = view::render(&cart, region, &MapRenderOpts::default()).expect("plain");
    let labelled = view::render(
        &cart,
        region,
        &MapRenderOpts {
            ids: true,
            ..MapRenderOpts::default()
        },
    )
    .expect("ids");

    assert_ne!(
        plain.rgba, labelled.rgba,
        "--ids must draw something over a non-empty cell"
    );

    // Cell (2,0) is tile 0 (empty): no id glyph should be drawn there, so
    // that whole 64x64 block must be untouched by the overlay.
    let cell_unchanged = |cx: u32, cy: u32| {
        let (x0, y0) = (cx * 64, cy * 64);
        (0..64).all(|dy| {
            (0..64).all(|dx| {
                let i = (((y0 + dy) * plain.width + (x0 + dx)) * 4) as usize;
                plain.rgba[i..i + 4] == labelled.rgba[i..i + 4]
            })
        })
    };
    assert!(cell_unchanged(2, 0), "tile 0 cell must not get an id label");
}

#[test]
fn render_rejects_out_of_bounds_region() {
    let cart = cart();
    let err = view::render(&cart, (126, 0, 4, 1), &MapRenderOpts::default()).unwrap_err();
    assert!(err.contains("outside"), "{err}");
}

// ---------------------------------------------------------------------------
// dump
// ---------------------------------------------------------------------------

#[test]
fn dump_prints_header_and_hex_rows() {
    let cart = cart();
    let out = view::dump(&cart, (0, 0, 4, 2)).unwrap();
    let mut lines = out.lines();
    assert_eq!(lines.next().unwrap(), MAP_FORMAT_MARKER);
    assert_eq!(lines.next().unwrap(), "# cx=0 cy=0 cw=4 ch=2");
    assert_eq!(lines.next().unwrap(), "001001000002");
    assert_eq!(lines.next().unwrap(), "003000000000");
    assert!(lines.next().is_none());
}

#[test]
fn dump_default_region_matches_used_extent() {
    let cart = cart();
    let region = console_agent::map::parse_region(None, cart.map()).unwrap();
    assert_eq!(region, (0, 0, 4, 2));
    let out = view::dump(&cart, region).unwrap();
    assert!(out.starts_with("# map-format=hex3\n# cx=0 cy=0 cw=4 ch=2\n"));
}

#[test]
fn dump_a_sub_region() {
    let cart = cart();
    let out = view::dump(&cart, (3, 0, 1, 1)).unwrap();
    let mut lines = out.lines();
    assert_eq!(lines.next().unwrap(), MAP_FORMAT_MARKER);
    assert_eq!(lines.next().unwrap(), "# cx=3 cy=0 cw=1 ch=1");
    assert_eq!(lines.next().unwrap(), "002");
}

// ---------------------------------------------------------------------------
// lint
// ---------------------------------------------------------------------------

#[test]
fn lint_reports_extent_counts_and_blank_tiles() {
    let cart = cart();
    let out = view::lint(&cart);

    assert_eq!(out["map_w"], 128);
    assert_eq!(out["map_h"], 64);
    assert_eq!(out["total_cells"], 128 * 64);
    assert_eq!(out["nonzero_cells"], 4);
    assert_eq!(
        out["used_extent"],
        json!({"cx": 0, "cy": 0, "cw": 4, "ch": 2})
    );
    assert_eq!(out["distinct_tiles"], 3);

    let fill_pct = out["fill_pct"].as_f64().unwrap();
    assert!((fill_pct - 0.05).abs() < 1e-9, "{fill_pct}");

    assert_eq!(
        out["tile_counts"],
        json!([{"tile": 1, "count": 2}, {"tile": 2, "count": 1}, {"tile": 3, "count": 1}])
    );
    assert_eq!(out["blank_sprite_tiles"], json!([{"tile": 3, "count": 1}]));
}

#[test]
fn lint_on_an_empty_map_reports_null_extent() {
    let text = "__lua__\nfunction _init() end\n";
    let cart = Cart::parse(text).unwrap();
    let out = view::lint(&cart);
    assert_eq!(out["nonzero_cells"], 0);
    assert_eq!(out["used_extent"], Value::Null);
    assert_eq!(out["tile_counts"], json!([]));
    assert_eq!(out["blank_sprite_tiles"], json!([]));
    assert_eq!(out["fill_pct"], 0.0);
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[test]
fn cli_render_writes_a_png() {
    let path = temp_path("render.cart");
    std::fs::write(&path, fixture_cart()).unwrap();
    let out = temp_path("render.png");

    let code = view::cli_view(&args(&[
        "render",
        path.to_str().unwrap(),
        "0,0,4,2",
        "-o",
        out.to_str().unwrap(),
    ]));
    assert_eq!(code, 0);
    assert!(std::fs::metadata(&out).expect("png written").len() > 0);
}

#[test]
fn cli_lint_takes_no_region_argument() {
    let path = temp_path("lint.cart");
    std::fs::write(&path, fixture_cart()).unwrap();

    let code = view::cli_view(&args(&["lint", path.to_str().unwrap(), "0,0,1,1"]));
    assert_ne!(code, 0, "map lint must reject a region argument");
}

// ---------------------------------------------------------------------------
// RPC
// ---------------------------------------------------------------------------

fn loaded_session() -> Session {
    let mut session = Session::new();
    let text = fixture_cart();
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 0, "method": "load_cart", "params": {"text": text}}),
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
fn rpc_map_verbs_are_read_only_mirrors() {
    let mut session = loaded_session();

    let path = temp_path("rpc-render.png");
    let p = path.to_str().unwrap();
    let resp = call(
        &mut session,
        "map_render",
        json!({"region": "0,0,4,2", "path": p}),
    );
    assert!(resp.get("error").is_none(), "map_render: {resp}");
    assert_eq!(resp["result"]["width"], 256);
    assert_eq!(resp["result"]["height"], 128);
    let _ = std::fs::remove_file(&path);

    let resp = call(&mut session, "map_dump", json!({"region": "0,0,4,2"}));
    assert!(resp.get("error").is_none(), "map_dump: {resp}");
    assert!(
        resp["result"]["text"]
            .as_str()
            .unwrap()
            .starts_with("# map-format=hex3\n# cx=0 cy=0 cw=4 ch=2\n")
    );

    let resp = call(&mut session, "map_lint", json!({}));
    assert!(resp.get("error").is_none(), "map_lint: {resp}");
    assert_eq!(resp["result"]["nonzero_cells"], 4);

    // No cart loaded at all.
    let mut empty = Session::new();
    for (method, params) in [
        ("map_render", json!({"path": "/dev/null"})),
        ("map_dump", json!({})),
        ("map_lint", json!({})),
    ] {
        let resp = call(&mut empty, method, params);
        assert_eq!(resp["error"]["code"], -32002, "{method}: {resp}");
    }
}

#[test]
fn rpc_map_verbs_select_authored_or_live_runtime_state() {
    let mut session = loaded_session();
    let resp = call(
        &mut session,
        "eval",
        json!({"code": "mset(0,0,2); mset(4,0,1)"}),
    );
    assert!(resp.get("error").is_none(), "eval: {resp}");

    let authored = call(
        &mut session,
        "map_dump",
        json!({"source": "authored", "region": "0,0,5,1"}),
    );
    assert_eq!(
        authored["result"]["text"].as_str().unwrap().lines().nth(2),
        Some("001001000002000")
    );

    let defaulted = call(&mut session, "map_dump", json!({"region": "0,0,5,1"}));
    assert_eq!(defaulted["result"], authored["result"]);

    let live = call(
        &mut session,
        "map_dump",
        json!({"source": "live", "region": "0,0,5,1"}),
    );
    assert_eq!(
        live["result"]["text"].as_str().unwrap().lines().nth(2),
        Some("002001000002001")
    );

    let live_lint = call(&mut session, "map_lint", json!({"source": "live"}));
    assert_eq!(live_lint["result"]["nonzero_cells"], 5);
    assert_eq!(
        live_lint["result"]["used_extent"],
        json!({"cx": 0, "cy": 0, "cw": 5, "ch": 2})
    );

    let bad = call(&mut session, "map_dump", json!({"source": "snapshot"}));
    assert_eq!(bad["error"]["code"], -32602, "{bad}");
    assert!(
        bad["error"]["message"]
            .as_str()
            .unwrap()
            .contains("authored")
    );

    for (method, params) in [
        (
            "map_render",
            json!({"source":17, "path":"/dev/null", "region":"0,0,1,1"}),
        ),
        ("map_dump", json!({"source":17, "region":"0,0,1,1"})),
        ("map_lint", json!({"source":17})),
    ] {
        let bad_type = call(&mut session, method, params);
        assert_eq!(bad_type["error"]["code"], -32602, "{method}: {bad_type}");
        assert!(
            bad_type["error"]["message"]
                .as_str()
                .unwrap()
                .contains("must be a string"),
            "{method}: {bad_type}"
        );
    }
}

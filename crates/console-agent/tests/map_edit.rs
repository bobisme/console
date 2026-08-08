//! Integration tests for `console map edit` (`copy`/`shift`/`fill`/
//! `clear`) — the `__map__` analog of `sprite_edit.rs`'s region transforms,
//! driven directly against [`console_agent::map::transform::cli_edit`] with
//! scratch cart files under `std::env::temp_dir()`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::map::transform::cli_edit;
use console_core::{Cart, MAP_FORMAT_MARKER, TileId};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_cart(tag: &str, text: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "console-map-edit-test-{}-{n}-{tag}.cart",
        std::process::id()
    ));
    std::fs::write(&path, text).expect("write temp cart");
    path
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read temp cart")
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn cart(map_rows: &[&str]) -> String {
    let mut s = format!("__lua__\nfunction _init() end\n\n__map__\n{MAP_FORMAT_MARKER}\n");
    for row in map_rows {
        s.push_str(row);
        s.push('\n');
    }
    s
}

/// The tile ids of the first `n` cells of row `y`, read back out of a
/// re-parsed cart — the common shape every op test's assertion takes.
fn row_tiles(cart: &Cart, y: usize, n: usize) -> Vec<TileId> {
    (0..n)
        .map(|x| cart.map()[y * console_core::MAP_W + x])
        .collect()
}

// ---------------------------------------------------------------------
// shift
// ---------------------------------------------------------------------

#[test]
fn shift_moves_cells_and_clears_vacated() {
    // Row 0: cells 0..4 = [1,2,3,4].
    let text = cart(&["001002003004"]);
    let path = temp_cart("shift-basic", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "0,0,4,1",
        "--dx",
        "1",
    ]));
    assert_eq!(code, 0);

    let out = Cart::parse(&read(&path)).unwrap();
    // Shifted right by 1 within the region: [0,1,2,3] (cell 3's old value 4
    // fell off the 4-wide region and is dropped).
    assert_eq!(row_tiles(&out, 0, 4), vec![0, 1, 2, 3]);
}

#[test]
fn shift_dx_defaults_to_zero() {
    let text = cart(&["001002003004"]);
    let path = temp_cart("shift-dx-default", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "0,0,4,1",
        "--dy",
        "0",
    ]));
    assert_eq!(code, 0);
    // No actual change (dx=0 dy=0): a legal no-op, file untouched.
    assert_eq!(read(&path), text);
}

#[test]
fn shift_vertically_within_a_column() {
    // Column 0, rows 0..3 = 1,2,3.
    let text = cart(&["001", "002", "003"]);
    let path = temp_cart("shift-vertical", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "0,0,1,3",
        "--dy",
        "1",
    ]));
    assert_eq!(code, 0);

    let out = Cart::parse(&read(&path)).unwrap();
    assert_eq!(out.map()[0], 0, "row 0 vacated");
    assert_eq!(out.map()[console_core::MAP_W], 1, "row 1 <- old row 0");
    assert_eq!(
        out.map()[console_core::MAP_W * 2],
        2,
        "row 2 <- old row 1 (old row 2 dropped)"
    );
}

#[test]
fn shift_requires_explicit_region() {
    let text = cart(&["001"]);
    let path = temp_cart("shift-no-region", &text);
    let before = read(&path);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "shift", "--dx", "1"]));
    assert_ne!(code, 0, "shift with no region must be rejected");
    assert_eq!(read(&path), before);
}

// ---------------------------------------------------------------------
// fill
// ---------------------------------------------------------------------

#[test]
fn fill_writes_the_tile_id_across_the_region() {
    let text = cart(&["000000000000", "000000000000"]);
    let path = temp_cart("fill-basic", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "fill", "0,0,4,2", "1f"]));
    assert_eq!(code, 0);

    let out = Cart::parse(&read(&path)).unwrap();
    assert_eq!(row_tiles(&out, 0, 4), vec![0x1f; 4]);
    assert_eq!(row_tiles(&out, 1, 4), vec![0x1f; 4]);
}

#[test]
fn fill_accepts_a_single_hex_digit() {
    let text = cart(&["000"]);
    let path = temp_cart("fill-single-digit", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "fill", "0,0,1,1", "5"]));
    assert_eq!(code, 0);
    let out = Cart::parse(&read(&path)).unwrap();
    assert_eq!(out.map()[0], 5);
}

#[test]
fn fill_rejects_a_bad_tile_id() {
    let text = cart(&["000"]);
    let path = temp_cart("fill-bad-id", &text);
    let before = read(&path);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "fill", "0,0,1,1", "zz"]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

// ---------------------------------------------------------------------
// clear
// ---------------------------------------------------------------------

#[test]
fn clear_zeroes_the_region() {
    let text = cart(&["001002003004"]);
    let path = temp_cart("clear-basic", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "clear", "0,0,2,1"]));
    assert_eq!(code, 0);

    let out = Cart::parse(&read(&path)).unwrap();
    assert_eq!(
        row_tiles(&out, 0, 4),
        vec![0, 0, 3, 4],
        "only the first 2 cells cleared"
    );
}

#[test]
fn clear_identical_region_is_a_noop() {
    let text = cart(&["000000000000"]);
    let path = temp_cart("clear-noop", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "clear", "0,0,4,1"]));
    assert_eq!(code, 0);
    assert_eq!(read(&path), text);
}

// ---------------------------------------------------------------------
// copy
// ---------------------------------------------------------------------

#[test]
fn copy_duplicates_the_region_to_the_destination() {
    let text = cart(&["001002000000"]);
    let path = temp_cart("copy-basic", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "copy", "0,0,2,1", "2,0"]));
    assert_eq!(code, 0);

    let out = Cart::parse(&read(&path)).unwrap();
    assert_eq!(
        row_tiles(&out, 0, 4),
        vec![1, 2, 1, 2],
        "source preserved, dest overwritten"
    );
}

#[test]
fn copy_overlapping_source_and_dest_uses_a_snapshot() {
    // Row 0: [1,2,3,0]. Copy region [0..3) to dest starting at 1: expect
    // [1,1,2,3] -- the write must not read its own half-overwritten output.
    let text = cart(&["001002003000"]);
    let path = temp_cart("copy-overlap", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "copy", "0,0,3,1", "1,0"]));
    assert_eq!(code, 0);

    let out = Cart::parse(&read(&path)).unwrap();
    assert_eq!(row_tiles(&out, 0, 4), vec![1, 1, 2, 3]);
}

#[test]
fn copy_destination_out_of_bounds_errors_without_modifying_file() {
    let text = cart(&["001002"]);
    let path = temp_cart("copy-oob", &text);
    let before = read(&path);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "copy", "0,0,2,1", "127,0"]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

// ---------------------------------------------------------------------
// --dry-run and shared errors
// ---------------------------------------------------------------------

#[test]
fn dry_run_leaves_the_file_untouched_and_prints_report() {
    let text = cart(&["000000000000"]);
    let path = temp_cart("edit-dry-run", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "fill",
        "0,0,4,1",
        "9",
        "--dry-run",
    ]));
    assert_eq!(code, 0);
    assert_eq!(read(&path), text, "dry-run must not write the file");
}

#[test]
fn unknown_op_is_rejected() {
    let text = cart(&["000"]);
    let path = temp_cart("edit-unknown-op", &text);
    let before = read(&path);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "flip", "0,0,1,1"]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn edit_creates_the_map_section_when_absent() {
    let text = "__lua__\nfunction _init() end\n";
    let path = temp_cart("edit-creates-section", text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "fill", "0,0,2,1", "7"]));
    assert_eq!(code, 0);

    let out = read(&path);
    assert!(out.contains("__map__"));
    let out_cart = Cart::parse(&out).unwrap();
    assert_eq!(row_tiles(&out_cart, 0, 2), vec![7, 7]);
}

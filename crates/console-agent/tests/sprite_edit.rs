//! Integration tests for `console sprite edit` (SPEC.md "Sprite &
//! animation authoring (PoC v1)" > "Transforms"), driven directly against
//! [`console_agent::sprite::transform::cli_edit`] (no process spawning,
//! except for one test that checks real stdout formatting) with scratch
//! cart files under `std::env::temp_dir()`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::sprite::transform::cli_edit;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh scratch file path under the OS temp dir, unique per call (so
/// tests running in parallel never collide).
fn temp_cart(tag: &str, text: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "console-sprite-edit-test-{}-{n}-{tag}.cart",
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

/// Build a minimal cart: `__lua__` + optional `__gfx_meta__` + `__sprites__`
/// with the given row lines (each may be shorter than 256 chars; missing
/// suffix and missing trailing rows both default to zero, per
/// `console_core::cart::parse_sprites`).
fn cart(gfx_meta: &str, sprite_rows: &[&str]) -> String {
    let mut s = String::from("__lua__\nfunction _init() end\n\n");
    if !gfx_meta.is_empty() {
        s.push_str("__gfx_meta__\n");
        s.push_str(gfx_meta);
        if !gfx_meta.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');
    }
    s.push_str("__sprites__\n");
    for row in sprite_rows {
        s.push_str(row);
        s.push('\n');
    }
    s
}

/// A 256-char sheet row with `segments` (byte offset, hex text) overlaid on
/// an all-zero background — the shape every expected "changed row" takes,
/// since `transform.rs` always re-encodes a changed row at full sheet
/// width.
fn row256(segments: &[(usize, &str)]) -> String {
    let mut chars = vec!['0'; 256];
    for (offset, text) in segments {
        for (i, c) in text.chars().enumerate() {
            chars[offset + i] = c;
        }
    }
    chars.into_iter().collect()
}

fn zero_row() -> String {
    "0".repeat(256)
}

// ---------------------------------------------------------------------
// shift
// ---------------------------------------------------------------------

#[test]
fn shift_fill_moves_pixel_and_clears_vacated() {
    let text = cart("sprite player rect=0,0 size=1x1", &["a0000000"]);
    let path = temp_cart("shift-fill", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "player",
        "--dx",
        "1",
        "--dy",
        "0",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let row0 = out
        .split("__sprites__\n")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    assert_eq!(
        row0,
        row256(&[(1, "a")]),
        "pixel moved from x=0 to x=1, vacated x=0 cleared"
    );
}

#[test]
fn shift_dx_alone_defaults_dy_to_zero() {
    let text = cart("sprite player rect=0,0 size=1x1", &["a0000000"]);
    let path = temp_cart("shift-dx-only", &text);

    // --dy omitted entirely: must default to 0, not error.
    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "player",
        "--dx",
        "1",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let row0 = out
        .split("__sprites__\n")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    assert_eq!(
        row0,
        row256(&[(1, "a")]),
        "dy defaults to 0, pixel only moves in x"
    );
}

#[test]
fn shift_dy_alone_defaults_dx_to_zero() {
    let rows: Vec<String> = (0..8u32)
        .map(|y| char::from_digit(y, 16).unwrap().to_string().repeat(8))
        .collect();
    let row_refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let text = cart("sprite player rect=0,0 size=1x1", &row_refs);
    let path = temp_cart("shift-dy-only", &text);

    // --dx omitted entirely: must default to 0, not error.
    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "player",
        "--dy",
        "2",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let after_lines: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    assert_eq!(
        after_lines[3],
        row256(&[(0, "11111111")]),
        "row3 <- src(row1), no x shift"
    );
}

#[test]
fn shift_with_no_dx_or_dy_is_a_legal_no_op() {
    let text = cart("sprite player rect=0,0 size=1x1", &["a0000000"]);
    let path = temp_cart("shift-noop", &text);

    // Neither --dx nor --dy given: both default to 0 -- shifting by (0,0) is
    // a legal no-op, not an error.
    let code = cli_edit(&args(&[path.to_str().unwrap(), "shift", "player"]));
    assert_eq!(code, 0);
    assert_eq!(read(&path), text, "shift by (0,0) changes nothing");
}

#[test]
fn shift_wrap_handles_negative_delta() {
    let text = cart("sprite player rect=0,0 size=1x1", &["a0000000"]);
    let path = temp_cart("shift-wrap-neg", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "player",
        "--dx",
        "-1",
        "--dy",
        "0",
        "--wrap",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let row0 = out
        .split("__sprites__\n")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    assert_eq!(row0, row256(&[(7, "a")]), "wraps around to x=7");
}

#[test]
fn shift_vertical_fill_only_rewrites_rows_that_actually_changed() {
    // Row y holds hex digit y repeated. Shifting down by 2 (fill) makes
    // row0 end up identical to its own original content ("00000000"),
    // even though row0 is inside the shifted rect -- it must NOT be
    // rewritten, since nothing about it changed.
    let rows: Vec<String> = (0..8u32)
        .map(|y| char::from_digit(y, 16).unwrap().to_string().repeat(8))
        .collect();
    let row_refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let text = cart("sprite player rect=0,0 size=1x1", &row_refs);
    let path = temp_cart("shift-vfill", &text);

    let before = read(&path);
    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "player",
        "--dx",
        "0",
        "--dy",
        "2",
    ]));
    assert_eq!(code, 0);
    let after = read(&path);

    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    assert_eq!(
        before_lines.len(),
        after_lines.len(),
        "no rows appended, none removed"
    );

    let sprites_header = before_lines
        .iter()
        .position(|l| *l == "__sprites__")
        .unwrap();
    let sprite_line_of_row = |y: usize| sprites_header + 1 + y;

    // Row 0 unaffected (dy shift pulls in out-of-bounds -> 0, same as its
    // original content) - must stay byte-identical.
    assert_eq!(
        after_lines[sprite_line_of_row(0)],
        before_lines[sprite_line_of_row(0)],
        "row 0 happens to be unchanged and must not be rewritten"
    );
    // Rows 1..7 do change.
    let expected = [
        row256(&[]),                // row1 <- src(row -1) OOB -> 0
        row256(&[]),                // row2 <- src(row 0) = "00000000"
        row256(&[(0, "11111111")]), // row3 <- src(row1)
        row256(&[(0, "22222222")]), // row4 <- src(row2)
        row256(&[(0, "33333333")]), // row5 <- src(row3)
        row256(&[(0, "44444444")]), // row6 <- src(row4)
        row256(&[(0, "55555555")]), // row7 <- src(row5)
    ];
    for (i, exp) in expected.iter().enumerate() {
        let y = i + 1;
        assert_eq!(&after_lines[sprite_line_of_row(y)], exp, "row {y} mismatch");
    }
}

// ---------------------------------------------------------------------
// flip
// ---------------------------------------------------------------------

#[test]
fn flip_horizontal_reverses_columns() {
    let rows = vec!["01234567"; 8];
    let text = cart("sprite player rect=0,0 size=1x1", &rows);
    let path = temp_cart("flip-h", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "flip",
        "player",
        "--horizontal",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    for line in out.split("__sprites__\n").nth(1).unwrap().lines() {
        assert_eq!(line, row256(&[(0, "76543210")]));
    }
}

#[test]
fn flip_vertical_reverses_rows() {
    let rows: Vec<String> = (0..8u32)
        .map(|y| char::from_digit(y, 16).unwrap().to_string().repeat(8))
        .collect();
    let row_refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let text = cart("sprite player rect=0,0 size=1x1", &row_refs);
    let path = temp_cart("flip-v", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "flip",
        "player",
        "--vertical",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let new_rows: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    assert_eq!(new_rows.len(), 8);
    for (y, row) in new_rows.iter().enumerate() {
        let expected_digit = char::from_digit(7 - y as u32, 16)
            .unwrap()
            .to_string()
            .repeat(8);
        assert_eq!(*row, row256(&[(0, &expected_digit)]), "row {y}");
    }
}

// ---------------------------------------------------------------------
// rotate
// ---------------------------------------------------------------------

#[test]
fn rotate_cw_8x8_moves_corners() {
    // TL=1 TR=2 BR=3 BL=4 (row0: col0=1,col7=2; row7: col0=4,col7=3).
    let rows = [
        "10000002", "00000000", "00000000", "00000000", "00000000", "00000000", "00000000",
        "40000003",
    ];
    let text = cart("sprite player rect=0,0 size=1x1", &rows);
    let path = temp_cart("rotate-cw", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "rotate", "player", "--cw"]));
    assert_eq!(code, 0);

    let out = read(&path);
    // The 8 physical rows all still exist (they were all already present);
    // only 2 of them actually changed value -- the rest must stay byte
    // identical to their original (short, 8-char) form.
    let before: Vec<&str> = text
        .split("__sprites__\n")
        .nth(1)
        .unwrap()
        .lines()
        .collect();
    let after: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    assert_eq!(before.len(), after.len());
    let changed: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    assert_eq!(changed, vec![0, 7], "only row0 and row7 actually change");
    // After cw: new TL=old BL=4, new TR=old TL=1, new BR=old TR=2, new BL=old BR=3.
    assert_eq!(after[0], row256(&[(0, "4"), (7, "1")]));
    assert_eq!(after[7], row256(&[(0, "3"), (7, "2")]));
}

#[test]
fn rotate_ccw_8x8_moves_corners() {
    let rows = [
        "10000002", "00000000", "00000000", "00000000", "00000000", "00000000", "00000000",
        "40000003",
    ];
    let text = cart("sprite player rect=0,0 size=1x1", &rows);
    let path = temp_cart("rotate-ccw", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "rotate",
        "player",
        "--ccw",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let before: Vec<&str> = text
        .split("__sprites__\n")
        .nth(1)
        .unwrap()
        .lines()
        .collect();
    let after: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    assert_eq!(before.len(), after.len());
    let changed: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    assert_eq!(changed, vec![0, 7]);
    // After ccw: new TL=old TR=2, new TR=old BR=3, new BR=old BL=4, new BL=old TL=1.
    assert_eq!(after[0], row256(&[(0, "2"), (7, "3")]));
    assert_eq!(after[7], row256(&[(0, "1"), (7, "4")]));
}

#[test]
fn rotate_16x16_on_a_2x2_tile_sprite() {
    let rows = {
        let mut v = vec!["0".to_string(); 24];
        v[8] = format!("1{}2", "0".repeat(14));
        v[23] = format!("4{}3", "0".repeat(14));
        v
    };
    let row_refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let text = cart("sprite big rect=0,1 size=2x2", &row_refs);
    let path = temp_cart("rotate-16", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "rotate", "big", "--cw"]));
    assert_eq!(code, 0);

    let out = read(&path);
    let before: Vec<&str> = text
        .split("__sprites__\n")
        .nth(1)
        .unwrap()
        .lines()
        .collect();
    let after: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    assert_eq!(before.len(), after.len());
    let changed: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    assert_eq!(
        changed,
        vec![8, 23],
        "only row 8 and row 23 actually change"
    );
    assert_eq!(after[8], row256(&[(0, "4"), (15, "1")]));
    assert_eq!(after[23], row256(&[(0, "3"), (15, "2")]));
}

#[test]
fn rotate_rejects_non_square_region() {
    let text = cart("", &["00000000"]);
    let path = temp_cart("rotate-nonsquare", &text);
    let before = read(&path);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "rotate",
        "0,0,2,1",
        "--cw",
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before, "file must be untouched on error");
}

// ---------------------------------------------------------------------
// copy
// ---------------------------------------------------------------------

#[test]
fn copy_rect_to_sprite_non_overlapping() {
    let rows: Vec<String> = (1..=8u32)
        .map(|d| char::from_digit(d, 16).unwrap().to_string().repeat(8))
        .collect();
    let row_refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let text = cart("sprite dest rect=2,0 size=1x1", &row_refs);
    let path = temp_cart("copy-rect-sprite", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "copy", "0,0,1,1", "dest"]));
    assert_eq!(code, 0);

    let out = read(&path);
    let new_rows: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    assert_eq!(new_rows.len(), 8);
    for (y, row) in new_rows.iter().enumerate() {
        let digit = char::from_digit(y as u32 + 1, 16)
            .unwrap()
            .to_string()
            .repeat(8);
        assert_eq!(*row, row256(&[(0, &digit), (16, &digit)]), "row {y}");
    }
}

#[test]
fn copy_overlapping_regions_behave_as_if_through_a_temp_buffer() {
    // src = tiles (0,0)-(2,1) => pixels x0..16,y0..8. dst = tiles (1,0)-(2,1)
    // => pixels x8..24,y0..8. They overlap on x8..16. A naive left-to-right
    // in-place copy would clobber src reads with already-written dst
    // values; the correct (buffered) result keeps every dst pixel equal to
    // the *original* src pixel 8 columns to its left.
    let rows = vec!["0123456789abcdef"; 8];
    let text = cart("", &rows);
    let path = temp_cart("copy-overlap", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "copy",
        "0,0,2,1",
        "1,0,2,1",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let new_rows: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    assert_eq!(new_rows.len(), 8);
    for row in &new_rows {
        assert_eq!(*row, row256(&[(0, "01234567"), (8, "0123456789abcdef")]));
    }
}

#[test]
fn copy_size_mismatch_errors_without_modifying_file() {
    let text = cart("", &["00000000"]);
    let path = temp_cart("copy-mismatch", &text);
    let before = read(&path);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "copy",
        "0,0,1,1",
        "0,0,2,1",
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

// ---------------------------------------------------------------------
// clear
// ---------------------------------------------------------------------

#[test]
fn clear_zeroes_the_whole_region() {
    let rows = vec!["ffffffff"; 8];
    let text = cart("sprite player rect=0,0 size=1x1", &rows);
    let path = temp_cart("clear", &text);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "clear", "player"]));
    assert_eq!(code, 0);

    let out = read(&path);
    let new_rows: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    assert_eq!(new_rows.len(), 8);
    for row in &new_rows {
        assert_eq!(*row, zero_row());
    }
}

// ---------------------------------------------------------------------
// byte preservation: the cardinal rule
// ---------------------------------------------------------------------

#[test]
fn only_the_changed_sprite_row_is_touched_everything_else_is_byte_identical() {
    let text = concat!(
        "__meta__\n",
        "title = Byte  Preservation   Cart\n",
        "-- a comment with weird   spacing\n",
        "custom_field=42\n",
        "\n",
        "__lua__\n",
        "function _init() end\n",
        "\n",
        "__gfx_meta__\n",
        "sprite player rect=0,0 size=1x1\n",
        "\n",
        "__sprites__\n",
        "a0000000\n",
        "00000000   \n", // trailing spaces on an untouched row: must survive
        "00000000\n",
        "00000000\n",
        "00000000\n",
        "00000000\n",
        "00000000\n",
        "00000000\n",
        "\n",
        "__notes__\n",
        "this line has trailing spaces   \n",
        "line with CRLF ending\r\n",
        "plain last line\n",
    );
    let path = temp_cart("byte-preserve", text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "player",
        "--dx",
        "1",
        "--dy",
        "0",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let before: Vec<&str> = text.split('\n').collect();
    let after: Vec<&str> = out.split('\n').collect();

    assert_eq!(before.len(), after.len(), "no lines added or removed");

    let row0_idx = before.iter().position(|l| *l == "a0000000").unwrap();
    for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
        if i == row0_idx {
            assert_eq!(*a, row256(&[(1, "a")]), "the one changed row");
        } else {
            assert_eq!(a, b, "line {i} must be byte-identical: {b:?} vs {a:?}");
        }
    }
}

// ---------------------------------------------------------------------
// appending rows beyond a short __sprites__ section
// ---------------------------------------------------------------------

#[test]
fn shift_appends_missing_rows_with_zero_fillers_for_the_gap() {
    let text = "__lua__\nfunction _init() end\n\n__gfx_meta__\nsprite player rect=0,0 size=1x1\n\n__sprites__\na0000000\n__notes__\ntail\n";
    let path = temp_cart("append-gap", text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "player",
        "--dx",
        "0",
        "--dy",
        "3",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    assert!(
        out.ends_with("__notes__\ntail\n"),
        "trailing section preserved: {out:?}"
    );

    let sprite_lines: Vec<&str> = out
        .split("__sprites__\n")
        .nth(1)
        .unwrap()
        .split("__notes__")
        .next()
        .unwrap()
        .lines()
        .collect();
    assert_eq!(
        sprite_lines.len(),
        4,
        "row0 (overwritten) + 3 appended rows: {sprite_lines:?}"
    );
    assert_eq!(sprite_lines[0], zero_row(), "row0 vacated by the shift");
    assert_eq!(
        sprite_lines[1],
        zero_row(),
        "row1 filler (still implicitly zero)"
    );
    assert_eq!(
        sprite_lines[2],
        zero_row(),
        "row2 filler (still implicitly zero)"
    );
    assert_eq!(
        sprite_lines[3],
        row256(&[(0, "a")]),
        "row3 receives the shifted pixel"
    );
}

// ---------------------------------------------------------------------
// --dry-run
// ---------------------------------------------------------------------

#[test]
fn dry_run_leaves_the_file_untouched() {
    let text = cart("sprite player rect=0,0 size=1x1", &["a0000000"]);
    let path = temp_cart("dry-run", &text);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "shift",
        "player",
        "--dx",
        "1",
        "--dy",
        "0",
        "--dry-run",
    ]));
    assert_eq!(code, 0);
    assert_eq!(read(&path), text, "dry-run must not write the file");
}

#[test]
fn dry_run_prints_the_expected_line_number_and_content_to_stdout() {
    let text = cart("sprite player rect=0,0 size=1x1", &["a0000000"]);
    let path = temp_cart("dry-run-stdout", &text);
    // Sprite row 0 is line 4 (1-based): __lua__, fn, blank, __gfx_meta__...
    // recompute precisely instead of hard-coding.
    let expected_lineno = text.lines().position(|l| l == "a0000000").unwrap() + 1;
    let expected_content = row256(&[(1, "a")]);

    let bin = env!("CARGO_BIN_EXE_console");
    let output = std::process::Command::new(bin)
        .args([
            "sprite",
            "edit",
            path.to_str().unwrap(),
            "shift",
            "player",
            "--dx",
            "1",
            "--dy",
            "0",
            "--dry-run",
        ])
        .output()
        .expect("spawn console");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim_end(),
        format!("{expected_lineno}: {expected_content}")
    );
    assert_eq!(
        read(&path),
        text,
        "dry-run via the real binary must not write either"
    );
}

// ---------------------------------------------------------------------
// error paths
// ---------------------------------------------------------------------

#[test]
fn unknown_target_errors_without_modifying_file() {
    let text = cart("sprite player rect=0,0 size=1x1", &["a0000000"]);
    let path = temp_cart("err-unknown-target", &text);
    let before = read(&path);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "clear", "nope"]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn frame_off_sheet_errors_without_modifying_file() {
    let text = cart("sprite full rect=0,0 size=32x32", &["00000000"]);
    let path = temp_cart("err-frame-off-sheet", &text);
    let before = read(&path);

    let code = cli_edit(&args(&[
        path.to_str().unwrap(),
        "clear",
        "full",
        "--frame",
        "1",
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn bad_op_name_errors_without_modifying_file() {
    let text = cart("", &["00000000"]);
    let path = temp_cart("err-bad-op", &text);
    let before = read(&path);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "frobnicate", "0,0,1,1"]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn corrupt_cart_is_rejected_before_editing() {
    let path = temp_cart("err-corrupt", "__meta__\ntitle=x\n"); // no __lua__
    let before = read(&path);

    let code = cli_edit(&args(&[path.to_str().unwrap(), "clear", "0,0,1,1"]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

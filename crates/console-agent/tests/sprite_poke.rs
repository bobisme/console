//! Integration tests for `console-agent sprite dump` and `sprite poke`
//! (SPEC.md "Sprite & animation authoring (PoC v1)" > "Transforms" — the
//! write half of the pair that lets an agent write pixels directly instead
//! of hand-editing `__sprites__` hex).
//!
//! Mirrors the structure of `sprite_edit.rs`: driven directly against
//! [`console_agent::sprite::transform::cli_poke`] and
//! [`console_agent::sprite::view::dump`] with scratch cart files under
//! `std::env::temp_dir()`, plus a couple of real-process tests for
//! `--stdin` and the dump-piped-into-poke round trip.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::sprite::transform::cli_poke;
use console_agent::sprite::view;
use console_core::Cart;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_cart(tag: &str, text: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "console-agent-sprite-poke-test-{}-{n}-{tag}.cart",
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
/// with the given row lines (each may be shorter than 128 chars; missing
/// suffix and missing trailing rows both default to zero).
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

fn row128(segments: &[(usize, &str)]) -> String {
    let mut chars = vec!['0'; 128];
    for (offset, text) in segments {
        for (i, c) in text.chars().enumerate() {
            chars[offset + i] = c;
        }
    }
    chars.into_iter().collect()
}

// ---------------------------------------------------------------------
// dump
// ---------------------------------------------------------------------

#[test]
fn dump_prints_header_and_rows() {
    let rows = [
        "12345678", "abcdef01", "00000000", "00000000", "00000000", "00000000", "00000000",
        "00000000",
    ];
    let text = cart("sprite player rect=0,0 size=1x1", &rows);
    let cart_parsed = Cart::parse(&text).unwrap();

    let out = view::dump(&cart_parsed, "player", 0).expect("dump player");
    let mut lines = out.lines();
    assert_eq!(lines.next().unwrap(), "# x=0 y=0 w=8 h=8");
    assert_eq!(lines.next().unwrap(), "12345678");
    assert_eq!(lines.next().unwrap(), "abcdef01");
    for _ in 0..6 {
        assert_eq!(lines.next().unwrap(), "00000000");
    }
    assert!(lines.next().is_none());
}

#[test]
fn dump_frame_selects_a_displaced_rect() {
    // anim.frame semantics for dump match sprite edit: --frame is the raw
    // sheet frame index (rect displaced N sprite-widths right).
    let rows = vec!["00112233"; 8];
    let text = cart("sprite player rect=0,0 size=1x1", &rows);
    let cart_parsed = Cart::parse(&text).unwrap();

    let out = view::dump(&cart_parsed, "player", 1).expect("dump frame 1");
    assert_eq!(out.lines().next().unwrap(), "# x=8 y=0 w=8 h=8");
}

#[test]
fn dump_rejects_unknown_target() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"]);
    let cart_parsed = Cart::parse(&text).unwrap();
    let err = view::dump(&cart_parsed, "nope", 0).unwrap_err();
    assert!(err.contains("unknown target"), "{err}");
}

// ---------------------------------------------------------------------
// poke: basic writes
// ---------------------------------------------------------------------

#[test]
fn poke_rows_overwrites_the_region() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"; 8]);
    let path = temp_cart("poke-basic", &text);

    let new_rows = [
        "12345678", "9abcdef0", "11111111", "22222222", "33333333", "44444444", "55555555",
        "66666666",
    ];
    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        &new_rows.join(","),
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let rows: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    for (y, row) in rows.iter().enumerate() {
        assert_eq!(*row, row128(&[(0, new_rows[y])]), "row {y}");
    }
}

#[test]
fn poke_only_rewrites_rows_that_actually_changed() {
    let rows: Vec<String> = (0..8u32)
        .map(|y| char::from_digit(y, 16).unwrap().to_string().repeat(8))
        .collect();
    let row_refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let text = cart("sprite player rect=0,0 size=1x1", &row_refs);
    let path = temp_cart("poke-selective", &text);

    // Same content, except row 3 changes.
    let mut new_rows: Vec<String> = rows.clone();
    new_rows[3] = "aaaaaaaa".to_string();
    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        &new_rows.join(","),
    ]));
    assert_eq!(code, 0);

    let before: Vec<&str> = text
        .split("__sprites__\n")
        .nth(1)
        .unwrap()
        .lines()
        .collect();
    let out = read(&path);
    let after: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    let changed: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    assert_eq!(changed, vec![3], "only row 3 actually changed");
    assert_eq!(after[3], row128(&[(0, "aaaaaaaa")]));
}

#[test]
fn poke_identical_rows_is_a_legal_noop() {
    let rows = vec!["a0000000"; 8];
    let text = cart("sprite player rect=0,0 size=1x1", &rows);
    let path = temp_cart("poke-noop", &text);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        &rows.join(","),
    ]));
    assert_eq!(code, 0);
    assert_eq!(
        read(&path),
        text,
        "poking identical content changes nothing"
    );
}

#[test]
fn poke_frame_selects_a_displaced_rect() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"; 8]);
    let path = temp_cart("poke-frame", &text);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--frame",
        "1",
        "--rows",
        &["ffffffff"; 8].join(","),
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let rows: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    for row in &rows {
        // frame 1 is displaced one sprite-width (8px) to the right.
        assert_eq!(*row, row128(&[(8, "ffffffff")]));
    }
}

#[test]
fn poke_dry_run_leaves_the_file_untouched_and_prints_report() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"; 8]);
    let path = temp_cart("poke-dry-run", &text);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        &["a0000000"; 8].join(","),
        "--dry-run",
    ]));
    assert_eq!(code, 0);
    assert_eq!(read(&path), text, "dry-run must not write the file");
}

// ---------------------------------------------------------------------
// poke: error paths (row count / length / hex validity)
// ---------------------------------------------------------------------

#[test]
fn poke_wrong_row_count_errors_without_modifying_file() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"; 8]);
    let path = temp_cart("poke-err-count", &text);
    let before = read(&path);

    // Only 6 rows for an 8-row region.
    let rows: Vec<&str> = vec!["00000000"; 6];
    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        &rows.join(","),
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn poke_wrong_row_length_errors_naming_expected_and_got() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"; 8]);
    let path = temp_cart("poke-err-length", &text);
    let before = read(&path);

    let mut rows: Vec<String> = vec!["00000000".to_string(); 8];
    rows[2] = "abc".to_string(); // too short: 3 chars instead of 8
    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        &rows.join(","),
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before, "file must be untouched on error");
}

#[test]
fn poke_bad_hex_char_errors_without_modifying_file() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"; 8]);
    let path = temp_cart("poke-err-hex", &text);
    let before = read(&path);

    let mut rows: Vec<String> = vec!["00000000".to_string(); 8];
    rows[4] = "0000zz00".to_string();
    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        &rows.join(","),
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before, "file must be untouched on error");
}

#[test]
fn poke_unknown_target_errors_without_modifying_file() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"]);
    let path = temp_cart("poke-err-target", &text);
    let before = read(&path);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "nope",
        "--rows",
        "00000000",
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn poke_requires_rows_or_stdin() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"]);
    let path = temp_cart("poke-err-no-source", &text);
    let before = read(&path);

    let code = cli_poke(&args(&[path.to_str().unwrap(), "player"]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn poke_rejects_both_rows_and_stdin() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"]);
    let path = temp_cart("poke-err-both-sources", &text);
    let before = read(&path);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        "00000000",
        "--stdin",
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

// ---------------------------------------------------------------------
// round trip: dump . poke is a no-op, poke . dump agrees
// ---------------------------------------------------------------------

#[test]
fn dump_then_poke_is_a_noop() {
    let rows = [
        "12345678", "9abcdef0", "fedcba98", "00000001", "10000000", "aaaaaaaa", "55555555",
        "ffffffff",
    ];
    let text = cart("sprite player rect=0,0 size=1x1", &rows);
    let path = temp_cart("roundtrip-dump-then-poke", &text);

    let cart_parsed = Cart::parse(&text).unwrap();
    let dumped = view::dump(&cart_parsed, "player", 0).expect("dump player");
    let dumped_rows: Vec<&str> = dumped.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(
        dumped_rows, rows,
        "dump must echo back exactly what was there"
    );

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        &dumped_rows.join(","),
    ]));
    assert_eq!(code, 0);
    assert_eq!(
        read(&path),
        text,
        "poking dump's own output must be a no-op"
    );
}

#[test]
fn poke_then_dump_round_trips() {
    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"; 8]);
    let path = temp_cart("roundtrip-poke-then-dump", &text);

    let new_rows = [
        "11223344", "55667788", "99aabbcc", "ddeeff00", "01010101", "20202020", "30303030",
        "40404040",
    ];
    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "player",
        "--rows",
        &new_rows.join(","),
    ]));
    assert_eq!(code, 0);

    let out_text = read(&path);
    let out_cart = Cart::parse(&out_text).unwrap();
    let dumped = view::dump(&out_cart, "player", 0).expect("dump after poke");
    let dumped_rows: Vec<&str> = dumped.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(
        dumped_rows, new_rows,
        "dump after poke must read back exactly what was poked"
    );
}

// ---------------------------------------------------------------------
// --stdin, and the real dump|poke pipe, via the actual binary
// ---------------------------------------------------------------------

#[test]
fn poke_stdin_reads_rows_and_skips_comment_lines() {
    use std::io::Write;

    let text = cart("sprite player rect=0,0 size=1x1", &["00000000"; 8]);
    let path = temp_cart("poke-stdin", &text);

    let bin = env!("CARGO_BIN_EXE_console-agent");
    let mut child = std::process::Command::new(bin)
        .args([
            "sprite",
            "poke",
            path.to_str().unwrap(),
            "player",
            "--stdin",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn console-agent");

    let stdin_rows = "# x=0 y=0 w=8 h=8\na1a1a1a1\nb2b2b2b2\nc3c3c3c3\nd4d4d4d4\ne5e5e5e5\nf6f6f6f6\n07070707\n18181818\n";
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_rows.as_bytes())
        .unwrap();
    let output = child.wait_with_output().expect("wait for console-agent");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let out = read(&path);
    let rows: Vec<&str> = out.split("__sprites__\n").nth(1).unwrap().lines().collect();
    let expected = [
        "a1a1a1a1", "b2b2b2b2", "c3c3c3c3", "d4d4d4d4", "e5e5e5e5", "f6f6f6f6", "07070707",
        "18181818",
    ];
    for (y, row) in rows.iter().enumerate() {
        assert_eq!(*row, row128(&[(0, expected[y])]), "row {y}");
    }
}

#[test]
fn real_dump_piped_into_real_poke_is_a_noop() {
    use std::io::Write;

    let rows = [
        "cafebabe", "deadbeef", "8badf00d", "0ff1ce00", "abad1dea", "b16b00b5", "f00dface",
        "5ca1ab1e",
    ];
    let text = cart("sprite player rect=0,0 size=1x1", &rows);
    let path = temp_cart("roundtrip-real-pipe", &text);
    let before = read(&path);

    let bin = env!("CARGO_BIN_EXE_console-agent");
    let dump_out = std::process::Command::new(bin)
        .args(["sprite", "dump", path.to_str().unwrap(), "player"])
        .output()
        .expect("spawn console-agent dump");
    assert!(
        dump_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&dump_out.stderr)
    );

    let mut poke = std::process::Command::new(bin)
        .args([
            "sprite",
            "poke",
            path.to_str().unwrap(),
            "player",
            "--stdin",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn console-agent poke");
    poke.stdin
        .take()
        .unwrap()
        .write_all(&dump_out.stdout)
        .unwrap();
    let poke_out = poke
        .wait_with_output()
        .expect("wait for console-agent poke");
    assert!(
        poke_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&poke_out.stderr)
    );

    assert_eq!(
        read(&path),
        before,
        "dump piped straight into poke must be a no-op"
    );
}

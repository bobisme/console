//! Integration tests for `console map dump` and `map poke` — the
//! `__map__` analog of `sprite_poke.rs`. Mirrors its structure: driven
//! directly against [`console_agent::map::transform::cli_poke`] and
//! [`console_agent::map::view::dump`] with scratch cart files under
//! `std::env::temp_dir()`, plus a couple of real-process tests for
//! `--stdin` and the dump-piped-into-poke round trip.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::map::transform::cli_poke;
use console_agent::map::view;
use console_core::Cart;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_cart(tag: &str, text: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "console-map-poke-test-{}-{n}-{tag}.cart",
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

/// A minimal cart with `__lua__` plus an optional `__map__` section (rows as
/// given, or omitted entirely when `map_rows` is empty, to exercise the
/// "poke creates the section" path).
fn cart(map_rows: &[&str]) -> String {
    let mut s = String::from("__lua__\nfunction _init() end\n\n");
    if !map_rows.is_empty() {
        s.push_str("__map__\n");
        for row in map_rows {
            s.push_str(row);
            s.push('\n');
        }
    }
    s
}

/// A full 128-cell (256 hex char) row with `segments` (cell offset, hex
/// pair text) overlaid on an all-zero background — the shape every changed
/// map row takes, since the rewrite always re-encodes a touched row at full
/// map width.
fn row256(segments: &[(usize, &str)]) -> String {
    let mut chars = vec!['0'; 256];
    for (cell_offset, text) in segments {
        let start = cell_offset * 2;
        for (i, c) in text.chars().enumerate() {
            chars[start + i] = c;
        }
    }
    chars.into_iter().collect()
}

// ---------------------------------------------------------------------
// dump
// ---------------------------------------------------------------------

#[test]
fn dump_prints_header_and_rows() {
    let text = cart(&["01020304", "0a0b0c0d"]);
    let cart_parsed = Cart::parse(&text).unwrap();

    let out = view::dump(&cart_parsed, (0, 0, 4, 2)).expect("dump region");
    let mut lines = out.lines();
    assert_eq!(lines.next().unwrap(), "# cx=0 cy=0 cw=4 ch=2");
    assert_eq!(lines.next().unwrap(), "01020304");
    assert_eq!(lines.next().unwrap(), "0a0b0c0d");
    assert!(lines.next().is_none());
}

#[test]
fn dump_rejects_out_of_bounds_region() {
    let text = cart(&["01"]);
    let cart_parsed = Cart::parse(&text).unwrap();
    let err = view::dump(&cart_parsed, (127, 0, 4, 1)).unwrap_err();
    assert!(err.contains("outside"), "{err}");
}

// ---------------------------------------------------------------------
// poke: basic writes
// ---------------------------------------------------------------------

#[test]
fn poke_rows_overwrites_the_region() {
    let text = cart(&["00000000", "00000000"]);
    let path = temp_cart("poke-basic", &text);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,4,2",
        "--rows",
        "01020304,0a0b0c0d",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    let rows: Vec<&str> = out.split("__map__\n").nth(1).unwrap().lines().collect();
    assert_eq!(rows[0], row256(&[(0, "01020304")]));
    assert_eq!(rows[1], row256(&[(0, "0a0b0c0d")]));
}

#[test]
fn poke_only_rewrites_rows_that_actually_changed() {
    let rows: Vec<String> = (0..4u32).map(|y| format!("{y:02x}{y:02x}")).collect();
    let row_refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let text = cart(&row_refs);
    let path = temp_cart("poke-selective", &text);

    // Same content, except row 2 changes.
    let mut new_rows = rows.clone();
    new_rows[2] = "aaaa".to_string();
    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,2,4",
        "--rows",
        &new_rows.join(","),
    ]));
    assert_eq!(code, 0);

    let before: Vec<&str> = text.split("__map__\n").nth(1).unwrap().lines().collect();
    let out = read(&path);
    let after: Vec<&str> = out.split("__map__\n").nth(1).unwrap().lines().collect();
    let changed: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    assert_eq!(changed, vec![2], "only row 2 actually changed");
    assert_eq!(after[2], row256(&[(0, "aaaa")]));
}

#[test]
fn poke_identical_rows_is_a_legal_noop() {
    let text = cart(&["a0000000"]);
    let path = temp_cart("poke-noop", &text);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,4,1",
        "--rows",
        "a0000000",
    ]));
    assert_eq!(code, 0);
    assert_eq!(
        read(&path),
        text,
        "poking identical content changes nothing"
    );
}

#[test]
fn poke_default_region_targets_the_used_extent() {
    let text = cart(&["01020000"]);
    let path = temp_cart("poke-default-region", &text);

    // Omit the region: it must resolve to the used extent, cx=0 cy=0 cw=2 ch=1
    // (cells 0 and 1 are the only non-zero ones).
    let code = cli_poke(&args(&[path.to_str().unwrap(), "--rows", "0a0b"]));
    assert_eq!(code, 0);

    let out = read(&path);
    let row0 = out
        .split("__map__\n")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    assert_eq!(row0, row256(&[(0, "0a0b")]));
}

#[test]
fn poke_dry_run_leaves_the_file_untouched_and_prints_report() {
    let text = cart(&["00000000"]);
    let path = temp_cart("poke-dry-run", &text);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,4,1",
        "--rows",
        "a0000000",
        "--dry-run",
    ]));
    assert_eq!(code, 0);
    assert_eq!(read(&path), text, "dry-run must not write the file");
}

// ---------------------------------------------------------------------
// poke: creates the __map__ section when absent
// ---------------------------------------------------------------------

#[test]
fn poke_creates_the_map_section_when_absent() {
    let text = cart(&[]); // no __map__ section at all
    assert!(!text.contains("__map__"));
    let path = temp_cart("poke-creates-section", &text);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,2,1",
        "--rows",
        "1f2a",
    ]));
    assert_eq!(code, 0);

    let out = read(&path);
    assert!(
        out.contains("__map__"),
        "poke must create the section: {out}"
    );
    let out_cart = Cart::parse(&out).expect("still a valid cart");
    assert_eq!(out_cart.map()[0], 0x1f);
    assert_eq!(out_cart.map()[1], 0x2a);
}

#[test]
fn poke_inserts_new_section_after_sprites_before_gfx_meta() {
    let text = "__lua__\nfunction _init() end\n\n__sprites__\na0000000\n\n__gfx_meta__\nsprite p rect=0,0 size=1x1\n";
    let path = temp_cart("poke-section-order", text);

    let code = cli_poke(&args(&[path.to_str().unwrap(), "0,0,1,1", "--rows", "05"]));
    assert_eq!(code, 0);

    let out = read(&path);
    let sprites_pos = out.find("__sprites__").unwrap();
    let map_pos = out.find("__map__").expect("map section inserted");
    let gfx_pos = out.find("__gfx_meta__").unwrap();
    assert!(sprites_pos < map_pos && map_pos < gfx_pos, "{out}");

    let out_cart = Cart::parse(&out).expect("still valid");
    assert_eq!(out_cart.map()[0], 0x05);
}

// ---------------------------------------------------------------------
// poke: error paths
// ---------------------------------------------------------------------

#[test]
fn poke_wrong_row_count_errors_without_modifying_file() {
    let text = cart(&["00000000", "00000000"]);
    let path = temp_cart("poke-err-count", &text);
    let before = read(&path);

    // Region is 2 rows tall, only 1 row supplied.
    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,4,2",
        "--rows",
        "01020304",
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn poke_wrong_row_length_errors_naming_expected_and_got() {
    let text = cart(&["00000000"]);
    let path = temp_cart("poke-err-length", &text);
    let before = read(&path);

    // Region is 4 cells wide (8 hex chars), row is only 4 chars.
    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,4,1",
        "--rows",
        "abcd",
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before, "file must be untouched on error");
}

#[test]
fn poke_bad_hex_char_errors_without_modifying_file() {
    let text = cart(&["00000000"]);
    let path = temp_cart("poke-err-hex", &text);
    let before = read(&path);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,4,1",
        "--rows",
        "00zz0000",
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before, "file must be untouched on error");
}

#[test]
fn poke_out_of_bounds_region_errors_without_modifying_file() {
    let text = cart(&["00"]);
    let path = temp_cart("poke-err-region", &text);
    let before = read(&path);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "127,0,4,1",
        "--rows",
        "01020304",
    ]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn poke_requires_rows_or_stdin() {
    let text = cart(&["00000000"]);
    let path = temp_cart("poke-err-no-source", &text);
    let before = read(&path);

    let code = cli_poke(&args(&[path.to_str().unwrap(), "0,0,4,1"]));
    assert_ne!(code, 0);
    assert_eq!(read(&path), before);
}

#[test]
fn poke_rejects_both_rows_and_stdin() {
    let text = cart(&["00000000"]);
    let path = temp_cart("poke-err-both-sources", &text);
    let before = read(&path);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,4,1",
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
    let text = cart(&["12345678", "9abcdef0", "fedcba98"]);
    let path = temp_cart("roundtrip-dump-then-poke", &text);

    let cart_parsed = Cart::parse(&text).unwrap();
    let dumped = view::dump(&cart_parsed, (0, 0, 4, 3)).expect("dump region");
    let dumped_rows: Vec<&str> = dumped.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(dumped_rows, ["12345678", "9abcdef0", "fedcba98"]);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,4,3",
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
    let text = cart(&["00000000", "00000000"]);
    let path = temp_cart("roundtrip-poke-then-dump", &text);

    let code = cli_poke(&args(&[
        path.to_str().unwrap(),
        "0,0,4,2",
        "--rows",
        "11223344,55667788",
    ]));
    assert_eq!(code, 0);

    let out_text = read(&path);
    let out_cart = Cart::parse(&out_text).unwrap();
    let dumped = view::dump(&out_cart, (0, 0, 4, 2)).expect("dump after poke");
    let dumped_rows: Vec<&str> = dumped.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(dumped_rows, ["11223344", "55667788"]);
}

// ---------------------------------------------------------------------
// --stdin, and the real dump|poke pipe, via the actual binary
// ---------------------------------------------------------------------

#[test]
fn poke_stdin_reads_rows_and_skips_comment_lines() {
    use std::io::Write;

    let text = cart(&["00000000", "00000000"]);
    let path = temp_cart("poke-stdin", &text);

    let bin = env!("CARGO_BIN_EXE_console");
    let mut child = std::process::Command::new(bin)
        .args(["map", "poke", path.to_str().unwrap(), "0,0,4,2", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn console");

    let stdin_rows = "# cx=0 cy=0 cw=4 ch=2\na1b2c3d4\ne5f60708\n";
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_rows.as_bytes())
        .unwrap();
    let output = child.wait_with_output().expect("wait for console");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let out = read(&path);
    let rows: Vec<&str> = out.split("__map__\n").nth(1).unwrap().lines().collect();
    assert_eq!(rows[0], row256(&[(0, "a1b2c3d4")]));
    assert_eq!(rows[1], row256(&[(0, "e5f60708")]));
}

#[test]
fn real_dump_piped_into_real_poke_is_a_noop() {
    use std::io::Write;

    let text = cart(&["cafebabe", "deadbeef"]);
    let path = temp_cart("roundtrip-real-pipe", &text);
    let before = read(&path);

    let bin = env!("CARGO_BIN_EXE_console");
    let dump_out = std::process::Command::new(bin)
        .args(["map", "dump", path.to_str().unwrap(), "0,0,4,2"])
        .output()
        .expect("spawn console dump");
    assert!(
        dump_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&dump_out.stderr)
    );

    let mut poke = std::process::Command::new(bin)
        .args(["map", "poke", path.to_str().unwrap(), "0,0,4,2", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn console poke");
    poke.stdin
        .take()
        .unwrap()
        .write_all(&dump_out.stdout)
        .unwrap();
    let poke_out = poke.wait_with_output().expect("wait for console poke");
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

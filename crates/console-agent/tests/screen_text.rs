use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn scratch() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("console-screen-text-{}-{id}", std::process::id()))
}

fn run(cart: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .arg("run")
        .arg(cart)
        .args(args)
        .output()
        .expect("run console")
}

fn fixture() -> PathBuf {
    let dir = scratch();
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("screen.cart");
    fs::write(
        &cart,
        "__lua__\nfunction _draw() cls(0) rectfill(2,3,4,4,5) pset(10,10,63) end\n",
    )
    .unwrap();
    cart
}

#[test]
fn cli_preserves_full_dump_and_makes_a_small_crop_easy() {
    let cart = fixture();
    let full = run(&cart, &["--frames", "1", "--screen-text"]);
    assert!(
        full.status.success(),
        "{}",
        String::from_utf8_lossy(&full.stderr)
    );
    let full = String::from_utf8(full.stdout).unwrap();
    assert_eq!(full.lines().count(), 320);
    assert!(full.lines().all(|line| line.len() == 192));

    let crop = run(&cart, &["--frames", "1", "--screen-text-region", "1,2,5,4"]);
    assert!(
        crop.status.success(),
        "{}",
        String::from_utf8_lossy(&crop.stderr)
    );
    assert_eq!(
        String::from_utf8(crop.stdout).unwrap(),
        "00000\n05550\n05550\n00000\n"
    );
}

#[test]
fn cli_summary_is_single_line_bounded_json() {
    let cart = fixture();
    let output = run(&cart, &["--frames", "1", "--screen-text-summary"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.len() < 2_048,
        "summary grew unexpectedly large"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert!(report.get("lines").is_none());
    assert_eq!(report["palette_counts"]["5"], 6);
    assert_eq!(report["glyph_counts"]["_"], 1);
    assert_eq!(
        report["non_background_bounds"],
        serde_json::json!({"x":2,"y":3,"width":9,"height":8})
    );
    assert_eq!(report["truncation"]["line_characters_omitted"], 61_440);
}

#[test]
fn cli_rejects_bad_and_oversized_raw_regions_before_loading() {
    let missing = scratch().join("missing.cart");
    for region in ["0,0,0,1", "191,0,2,1", "0,0,192,100"] {
        let output = run(&missing, &["--screen-text-region", region]);
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("screen_text") || stderr.contains("screen-text"));
        assert!(
            !stderr.contains("cannot read"),
            "validation must precede cart I/O"
        );
    }

    let cart = fixture();
    let summary = run(
        &cart,
        &[
            "--screen-text-region",
            "0,0,192,100",
            "--screen-text-summary",
        ],
    );
    assert!(summary.status.success());
    assert!(summary.stdout.len() < 2_048);
}

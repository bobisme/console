//! End-to-end coverage for Apollo64 PNG interchange.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::palette::{decode_png_rgba, encode_png_rgba};
use console_core::{Cart, PALETTE};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_path(tag: &str, extension: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "console-palette-png-{}-{n}-{tag}.{extension}",
        std::process::id()
    ))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .args(args)
        .env_remove("FORMAT")
        .output()
        .expect("run console")
}

fn run_with_format_env(args: &[&str], format: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .args(args)
        .env("FORMAT", format)
        .output()
        .expect("run console")
}

fn base_cart() -> String {
    "__lua__\nfunction _init() end\n\n__gfx_meta__\nsprite icon rect=0,0 size=1x1\n\n__sprites__\n11111111\n11111111\n11111111\n11111111\n11111111\n11111111\n11111111\n11111111\n"
        .into()
}

fn as_str(path: &Path) -> &str {
    path.to_str().expect("utf-8 temp path")
}

#[test]
fn palette_show_writes_all_sixty_four_colors() {
    let out = temp_path("palette", "png");
    let output = run(&["palette", "show", "--cell", "1", "-o", as_str(&out)]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decoded = decode_png_rgba(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!((decoded.width, decoded.height), (8, 8));
    for (index, rgb) in PALETTE.iter().enumerate() {
        assert_eq!(&decoded.rgba[index * 4..index * 4 + 3], rgb);
    }
}

#[test]
fn exact_export_import_round_trip_preserves_source_indices() {
    let cart_path = temp_path("round-trip", "cart");
    let png_path = temp_path("round-trip", "png");
    std::fs::write(&cart_path, base_cart()).unwrap();

    let exported = run(&[
        "sprite",
        "export",
        as_str(&cart_path),
        "icon",
        "--palette",
        "source",
        "-o",
        as_str(&png_path),
    ]);
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let before = std::fs::read_to_string(&cart_path).unwrap();

    let dry = run(&[
        "sprite",
        "import",
        as_str(&cart_path),
        "icon",
        "--input",
        as_str(&png_path),
        "--dry-run",
        "--format",
        "json",
    ]);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert_eq!(std::fs::read_to_string(&cart_path).unwrap(), before);
    let report: serde_json::Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(report["changed_pixels"], 0);
    assert_eq!(report["written"], false);

    let imported = run(&[
        "sprite",
        "import",
        as_str(&cart_path),
        "icon",
        "--input",
        as_str(&png_path),
        "--format",
        "json",
    ]);
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    assert_eq!(
        Cart::parse(&std::fs::read_to_string(&cart_path).unwrap())
            .unwrap()
            .sprites()[0],
        1
    );

    let env_report = run_with_format_env(
        &[
            "sprite",
            "import",
            as_str(&cart_path),
            "icon",
            "--input",
            as_str(&png_path),
            "--dry-run",
        ],
        "json",
    );
    assert!(env_report.status.success());
    let report: serde_json::Value = serde_json::from_slice(&env_report.stdout).unwrap();
    assert_eq!(report["command"], "sprite import");

    let text_report = run(&[
        "sprite",
        "import",
        as_str(&cart_path),
        "icon",
        "--input",
        as_str(&png_path),
        "--dry-run",
    ]);
    assert!(text_report.status.success());
    let text = String::from_utf8(text_report.stdout).unwrap();
    for field in [
        "palette_indices=[1]",
        "changed_rows=[]",
        "partial_alpha_pixels=0",
        "alpha_threshold=128",
        "dry_run=true",
    ] {
        assert!(text.contains(field), "missing {field:?} in {text:?}");
    }
}

#[test]
fn import_rejects_wrong_dimensions_non_palette_rgb_and_color_budget() {
    let cart_path = temp_path("reject", "cart");
    std::fs::write(&cart_path, base_cart()).unwrap();

    let wrong_size = temp_path("wrong-size", "png");
    std::fs::write(&wrong_size, encode_png_rgba(&[1, 2, 3, 255], 1, 1)).unwrap();
    let output = run(&[
        "sprite",
        "import",
        as_str(&cart_path),
        "icon",
        "--input",
        as_str(&wrong_size),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("resize explicitly"));

    let arbitrary = temp_path("arbitrary", "png");
    let mut rgba = vec![0u8; 8 * 8 * 4];
    for px in rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&[1, 2, 3, 255]);
    }
    std::fs::write(&arbitrary, encode_png_rgba(&rgba, 8, 8)).unwrap();
    let output = run(&[
        "sprite",
        "import",
        as_str(&cart_path),
        "icon",
        "--input",
        as_str(&arbitrary),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("non-Apollo"));

    let nearest = run(&[
        "sprite",
        "import",
        as_str(&cart_path),
        "icon",
        "--input",
        as_str(&arbitrary),
        "--mapping",
        "nearest",
        "--dry-run",
        "--format",
        "json",
    ]);
    assert!(
        nearest.status.success(),
        "{}",
        String::from_utf8_lossy(&nearest.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&nearest.stdout).unwrap();
    assert_eq!(report["mapping"], "nearest");
    assert_eq!(report["changed_pixels"], 64);
    assert_eq!(report["written"], false);

    for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
        let rgb = PALETTE[1 + i % 2];
        px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    std::fs::write(&arbitrary, encode_png_rgba(&rgba, 8, 8)).unwrap();
    let output = run(&[
        "sprite",
        "import",
        as_str(&cart_path),
        "icon",
        "--input",
        as_str(&arbitrary),
        "--max-colors",
        "1",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeding --max-colors"));
}

#[test]
fn quantize_is_exact_size_deterministic_and_budgeted() {
    let input = temp_path("source", "png");
    let out_a = temp_path("quant-a", "png");
    let out_b = temp_path("quant-b", "png");
    let mut rgba = Vec::new();
    for y in 0..4u8 {
        for x in 0..6u8 {
            rgba.extend_from_slice(&[x * 35, y * 60, 255 - x * 25, 255]);
        }
    }
    std::fs::write(&input, encode_png_rgba(&rgba, 6, 4)).unwrap();

    for (index, out) in [&out_a, &out_b].into_iter().enumerate() {
        let mut args = vec![
            "palette",
            "quantize",
            as_str(&input),
            "-o",
            as_str(out),
            "--colors",
            "3",
            "--dither",
            "none",
        ];
        if index == 0 {
            args.push("--json");
        } else {
            args.extend_from_slice(&["--format", "json"]);
        }
        let output = run(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["width"], 6);
        assert_eq!(report["height"], 4);
        assert!(report["output_colors"].as_u64().unwrap() <= 3);
        assert_eq!(report["resized"], false);
    }
    assert_eq!(
        std::fs::read(&out_a).unwrap(),
        std::fs::read(&out_b).unwrap()
    );

    let text_report = run(&[
        "palette",
        "quantize",
        as_str(&input),
        "-o",
        as_str(&out_a),
        "--colors",
        "3",
    ]);
    assert!(text_report.status.success());
    let text = String::from_utf8(text_report.stdout).unwrap();
    assert!(text.contains("color_budget=3"), "{text:?}");
    assert!(text.contains("selected_indices=["), "{text:?}");
}

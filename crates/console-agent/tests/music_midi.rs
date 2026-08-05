//! CLI contracts for MIDI conversion and source-file synth preview.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

fn temp_path(extension: &str) -> PathBuf {
    let serial = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "console-music-midi-{}-{serial}.{extension}",
        std::process::id()
    ))
}

fn simple_midi() -> Vec<u8> {
    vec![
        b'M', b'T', b'h', b'd', 0, 0, 0, 6, 0, 0, 0, 1, 0, 96, b'M', b'T', b'r', b'k', 0, 0, 0, 20,
        0, 0x90, 60, 100, 96, 0x80, 60, 0, 0, 0x90, 64, 80, 96, 0x80, 64, 0, 0, 0xff, 0x2f, 0,
    ]
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .args(args)
        .output()
        .expect("run console")
}

#[test]
fn midi_to_abc_stdout_and_out_are_identical() {
    let midi = temp_path("mid");
    let abc = temp_path("abc");
    std::fs::write(&midi, simple_midi()).unwrap();

    let stdout = run(&["music", "midi-to-abc", midi.to_str().unwrap()]);
    assert!(
        stdout.status.success(),
        "{}",
        String::from_utf8_lossy(&stdout.stderr)
    );
    assert!(stdout.stderr.is_empty());
    assert!(String::from_utf8_lossy(&stdout.stdout).starts_with("X:1\n"));

    let written = run(&[
        "music",
        "midi-to-abc",
        midi.to_str().unwrap(),
        "--out",
        abc.to_str().unwrap(),
    ]);
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert!(written.stdout.is_empty());
    assert_eq!(std::fs::read(&abc).unwrap(), stdout.stdout);

    let _ = std::fs::remove_file(midi);
    let _ = std::fs::remove_file(abc);
}

#[test]
fn play_dry_run_decodes_midi_and_abc_without_an_audio_device() {
    let midi = temp_path("midi");
    let abc = temp_path("abc");
    std::fs::write(&midi, simple_midi()).unwrap();
    std::fs::write(
        &abc,
        "X:1\nT:CLI ABC\nM:4/4\nL:1/4\nQ:1/4=120\nK:C\nC E G c\n",
    )
    .unwrap();

    for path in [&midi, &abc] {
        let output = run(&[
            "music",
            "play",
            path.to_str().unwrap(),
            "--seconds",
            "0.25",
            "--dry-run",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        let report = String::from_utf8_lossy(&output.stderr);
        assert!(report.contains("0.25s"), "{report}");
        assert!(report.contains("volume 0.50"), "{report}");
        assert!(report.contains("(dry run)"), "{report}");
    }

    let explicit = run(&[
        "music",
        "play",
        abc.to_str().unwrap(),
        "--volume",
        "0.25",
        "--dry-run",
    ]);
    assert!(explicit.status.success());
    assert!(
        String::from_utf8_lossy(&explicit.stderr).contains("volume 0.25"),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );

    let _ = std::fs::remove_file(midi);
    let _ = std::fs::remove_file(abc);
}

#[test]
fn play_usage_errors_are_distinct_from_runtime_errors() {
    let help = run(&["music", "play", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--volume 0..1"));
    assert!(help.stderr.is_empty());

    let usage = run(&["music", "play", "--seconds", "nope"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).contains("usage:"));

    for volume in ["-0.1", "1.1", "NaN", "loud"] {
        let usage = run(&["music", "play", "tune.mid", "--volume", volume]);
        assert_eq!(usage.status.code(), Some(2), "{volume}");
        let error = String::from_utf8_lossy(&usage.stderr);
        assert!(error.contains("--volume"), "{volume}: {error}");
        assert!(error.contains("usage:"), "{volume}: {error}");
    }

    let missing = run(&["music", "play", "/definitely/missing/tune.mid", "--dry-run"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&missing.stderr).contains("usage:"));
}

#[test]
fn midi_to_abc_usage_errors_are_distinct_from_source_errors() {
    let usage = run(&["music", "midi-to-abc", "--wat"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).contains("usage:"));

    let missing = run(&["music", "midi-to-abc", "/definitely/missing/tune.mid"]);
    assert_eq!(missing.status.code(), Some(1));
    let error = String::from_utf8_lossy(&missing.stderr);
    assert!(error.contains("cannot read MIDI"), "{error}");
    assert!(!error.contains("usage:"), "{error}");
}

#[test]
fn hostile_abc_length_is_a_decode_error_not_a_panic() {
    let abc = temp_path("abc");
    std::fs::write(&abc, "X:1\nL:1/8\nQ:1/4=120\nK:C\nC/9223372036854775807\n").unwrap();
    let output = run(&["music", "play", abc.to_str().unwrap(), "--dry-run"]);
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("length component is too large"), "{error}");
    assert!(!error.contains("panicked"), "{error}");
    let _ = std::fs::remove_file(abc);
}

//! CLI contracts for lossless `.cmusic`, cart, and project playback.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "console-native-music-test-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .args(args)
        .output()
        .expect("run console")
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn track() -> &'static str {
    "console-music 1\n\
     __instruments__\n\
     wavetable 0 89acdeef ffeedca9 76532110 00112356\n\
     inst lead wave=w0 env=0,8,3 vib=12,3,2 echo=3\n\
     inst bass wave=6 fm=1,7,12 env=0,10,3\n\
     master drive=1 tone=1 hiss=0\n\
     echo delay=12 feedback=4 level=3\n\
     __sfx__\n\
     sfx 0 speed=auto\n\
     C4 lead 6 vib\n\
     E4 lead 6 arp4,7\n\
     G4 lead 6 fade-2\n\
     sfx 1 speed=auto\n\
     C2 bass 5 sl-12\n\
     C2 bass 5\n\
     __music__\n\
     bpm=120 rows_per_beat=4\n\
     pat 0 : 0 1 - -\n\
     pat 1 loop=1 : 0 1 - -\n"
}

#[test]
fn cmusic_dry_run_uses_native_song_form_and_host_gain() {
    let fixture = Fixture::new();
    let cmusic = fixture.write("theme.cmusic", track());
    let output = run(&["music", "play", path(&cmusic), "--song", "0", "--dry-run"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(report.contains("song 0"), "{report}");
    assert!(report.contains("pat 0 -> [pat 1 ->] loop to 1"), "{report}");
    assert!(report.contains("volume 0.50"), "{report}");
    assert!(
        report.contains("native, dry run, authored loop"),
        "{report}"
    );

    let clipped = run(&[
        "music",
        "play",
        path(&cmusic),
        "--seconds",
        "0.25",
        "--volume",
        "0.25",
        "--repeat",
        "--dry-run",
    ]);
    assert!(clipped.status.success());
    let report = String::from_utf8_lossy(&clipped.stderr);
    assert!(report.contains("0.25s"), "{report}");
    assert!(report.contains("volume 0.25"), "{report}");
    assert!(report.contains("native, dry run, repeat"), "{report}");
}

#[test]
fn cart_and_bundled_project_use_the_same_native_playback_command() {
    let fixture = Fixture::new();
    let bundle = console_agent::music::native::NativeMusic::parse(track()).unwrap();
    let cart = fixture.write("game.cart", &bundle.cart_text());
    let project_bundle = fixture.write("project/audio/game.cmusic", track());
    fixture.write("project/lua/main.lua", "function _draw() end\n");
    fixture.write(
        "project/console.toml",
        "manifest_version = 1\n\
         [cart]\n\
         title = \"Playable Project\"\n\
         [lua]\n\
         entry = \"lua/main.lua\"\n\
         [audio]\n\
         bundle = \"audio/game.cmusic\"\n",
    );
    assert!(project_bundle.exists());

    for input in [&cart, &fixture.0.join("project")] {
        let output = run(&[
            "music",
            "play",
            path(input),
            "--song",
            "1",
            "--seconds",
            "0.1",
            "--dry-run",
        ]);
        assert!(
            output.status.success(),
            "{}: {}",
            input.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let report = String::from_utf8_lossy(&output.stderr);
        assert!(report.contains("song 1"), "{report}");
        assert!(report.contains("0.10s"), "{report}");
        assert!(report.contains("(native, dry run)"), "{report}");
    }
}

#[test]
fn native_format_and_song_errors_are_clear_and_keep_exit_conventions() {
    let fixture = Fixture::new();
    let malformed = fixture.write("bad.cmusic", "X:1\nK:C\nC\n");
    let malformed = run(&["music", "play", path(&malformed), "--dry-run"]);
    assert_eq!(malformed.status.code(), Some(1));
    let error = String::from_utf8_lossy(&malformed.stderr);
    assert!(
        error.contains("missing \"console-music 1\" header"),
        "{error}"
    );
    assert!(!error.contains("usage:"), "{error}");

    let cmusic = fixture.write("valid.cmusic", track());
    let undefined = run(&["music", "play", path(&cmusic), "--song", "63", "--dry-run"]);
    assert_eq!(undefined.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&undefined.stderr).contains("no pattern 63"));

    let invalid_flag = run(&["music", "play", path(&cmusic), "--song", "64", "--dry-run"]);
    assert_eq!(invalid_flag.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid_flag.stderr).contains("usage:"));
}

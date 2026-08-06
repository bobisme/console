use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "console-run-eval-phases-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TempDir {
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

fn path(value: &Path) -> &str {
    value.to_str().expect("temporary path is UTF-8")
}

const PHASE_CART: &str = r#"__meta__
title=Eval Phases
__lua__
world=ecs.world("phase",{capacity=400})
events={"top"}
updates=0

function _init()
  events[#events+1]="init"
end

function _update()
  updates=updates+1
  events[#events+1]="update:"..updates..":"..(btn(4) and "A" or "-")
  world:spawn({tick={frame=updates}})
end

function _draw()
  events[#events+1]="draw:"..updates
  cls(world:count() >= 300 and 6 or 2)
end
"#;

#[test]
fn pre_frame_setup_runs_after_init_and_post_frame_inspection_precedes_captures() {
    let root = TempDir::new();
    let cart = root.join("phases.cart");
    let screenshot = root.join("final.png");
    let wav = root.join("three-frames.wav");
    std::fs::write(&cart, PHASE_CART).unwrap();

    // Put --eval-after first deliberately: command-line flag order does not
    // alter the fixed lifecycle phase of either eval.
    let output = run(&[
        "run",
        path(&cart),
        "--eval-after",
        "events[#events+1]='after:'..world:count(); cls(7); return {events=events,alive=world:count(),frames=t()*60}",
        "--frames",
        "3",
        "--input",
        "3:A",
        "--eval-before",
        "events[#events+1]='before:'..world:count(); for i=1,300 do world:spawn({mob=true}) end; return 'discarded'",
        "--screen-text",
        "--screenshot",
        path(&screenshot),
        "--wav",
        path(&wav),
    ]);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        321,
        "320 framebuffer rows plus one eval result"
    );
    assert!(
        lines[..320]
            .iter()
            .all(|line| line.len() == 192 && line.bytes().all(|byte| byte == b'7')),
        "screen text must observe the post-frame eval draw"
    );
    let result: serde_json::Value = serde_json::from_str(lines[320]).unwrap();
    assert_eq!(result["alive"], 303);
    assert_eq!(result["frames"], 3.0);
    assert_eq!(
        result["events"],
        serde_json::json!([
            "top",
            "init",
            "before:0",
            "update:1:A",
            "draw:1",
            "update:2:A",
            "draw:2",
            "update:3:A",
            "draw:3",
            "after:303"
        ])
    );
    assert_ne!(
        lines[320], "\"discarded\"",
        "setup result must not leak to stdout"
    );

    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&screenshot).unwrap(),
    ));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!(&pixels[..4], &[164, 221, 219, 255]);
    assert_eq!((info.width, info.height), (192, 320));

    let wav = std::fs::read(wav).unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(
        u32::from_le_bytes(wav[40..44].try_into().unwrap()),
        3 * 735 * 2,
        "audio capture contains exactly the three stepped frames"
    );
}

#[test]
fn failing_pre_frame_eval_stops_before_steps_and_artifacts() {
    let root = TempDir::new();
    let cart = root.join("phases.cart");
    let screenshot = root.join("must-not-exist.png");
    std::fs::write(&cart, PHASE_CART).unwrap();

    let output = run(&[
        "run",
        path(&cart),
        "--eval-before",
        "error('setup boom')",
        "--frames",
        "3",
        "--screenshot",
        path(&screenshot),
        "--eval-after",
        "return updates",
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!screenshot.exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("pre-frame eval failed"), "{stderr}");
    assert!(stderr.contains("setup boom"), "{stderr}");
}

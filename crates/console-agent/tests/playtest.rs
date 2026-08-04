mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use common::TestProject;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scratch(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "console-playtest-{}-{n}-{name}",
        std::process::id()
    ))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .args(args)
        .output()
        .expect("run console")
}

fn as_str(path: &Path) -> &str {
    path.to_str().expect("test path is UTF-8")
}

#[test]
fn lantern_scenario_runs_in_order_and_captures_every_artifact() {
    let root = root();
    let cart = root.join("carts/lantern-leap.cart");
    let scenario = root.join("carts/lantern-leap.playtest.json");
    let first = scratch("first");
    let second = scratch("second");

    for artifacts in [&first, &second] {
        let output = run(&[
            "playtest",
            as_str(&cart),
            "--scenario",
            as_str(&scenario),
            "--artifacts",
            as_str(artifacts),
            "--format",
            "json",
        ]);
        assert!(
            output.status.success(),
            "playtest failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
        assert_eq!(report["scenario"]["status"], "passed");
        assert_eq!(report["scenario"]["frame_count"], 34);
        assert_eq!(report["scenario"]["artifact_count"], 6);
        assert_eq!(report["stages"][5]["actual"], 1);
    }

    let files = [
        "lower-tower.png",
        "lower-tower.txt",
        "lower-tower.wav",
        "lower-tower-spectrum.png",
        "lower-tower-events.json",
        "lower-tower-stats.json",
    ];
    for file in files {
        let a = fs::read(first.join(file)).expect("first artifact");
        let b = fs::read(second.join(file)).expect("second artifact");
        assert_eq!(a, b, "{file} should be deterministic");
    }
    assert!(
        fs::read(first.join("lower-tower.png"))
            .unwrap()
            .starts_with(b"\x89PNG")
    );
    assert!(
        fs::read(first.join("lower-tower.wav"))
            .unwrap()
            .starts_with(b"RIFF")
    );
    let screen = fs::read_to_string(first.join("lower-tower.txt")).unwrap();
    assert_eq!(screen.lines().count(), 320);
    assert!(screen.lines().all(|line| line.len() == 192));
}

#[test]
fn assertion_failure_is_a_structured_exit_one() {
    let dir = scratch("assertion");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    let scenario = dir.join("fail.json");
    fs::write(
        &cart,
        "__lua__\ncount=0\nfunction _update() if btnp(4) then count=count+1 end end\n",
    )
    .unwrap();
    fs::write(
        &scenario,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "stages": [
                {"op":"eval", "code":"count=10"},
                {"op":"input", "frames":3, "buttons":"A"},
                {"op":"assert", "code":"return count", "equals":2}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = run(&[
        "playtest",
        as_str(&cart),
        "--scenario",
        as_str(&scenario),
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON failure report");
    assert_eq!(report["scenario"]["status"], "failed");
    assert_eq!(report["stages"][2]["expected"], 2);
    assert_eq!(report["stages"][2]["actual"], 11);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("stage 2 failed"),
        "failure should be actionable on stderr"
    );
}

#[test]
fn playtest_accepts_project_directories_and_explicit_manifests() {
    let project = TestProject::new("playtest", "Playtest Project", 23);
    let scenario = project.root().join("scenario.json");
    fs::write(
        &scenario,
        r#"{"version":1,"stages":[{"op":"assert","code":"return project_value","equals":23}]}"#,
    )
    .unwrap();

    for input in [project.root().to_path_buf(), project.manifest()] {
        let output = run(&[
            "playtest",
            as_str(&input),
            "--scenario",
            as_str(&scenario),
            "--format",
            "json",
        ]);
        assert!(
            output.status.success(),
            "project playtest failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["scenario"]["status"], "passed");
        assert_eq!(report["scenario"]["cart"], as_str(&input));
    }
    assert!(!project.root().join("build/game.cart").exists());
}

#[test]
fn unsafe_capture_sizes_are_rejected_before_execution() {
    let dir = scratch("capture-bounds");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    fs::write(&cart, "__lua__\n").unwrap();
    let artifacts = dir.join("artifacts");

    for (name, stage, expected) in [
        (
            "zoom.json",
            json!({"op":"capture", "screenshot":"screen.png", "zoom":u32::MAX}),
            "zoom must be 1..=16",
        ),
        (
            "cell.json",
            json!({"op":"capture", "spectrogram":"audio.png", "cell":u32::MAX}),
            "cell must be 1..=8",
        ),
    ] {
        let scenario = dir.join(name);
        fs::write(
            &scenario,
            serde_json::to_vec(&json!({"version":1, "stages":[stage]})).unwrap(),
        )
        .unwrap();
        let output = run(&[
            "playtest",
            as_str(&cart),
            "--scenario",
            as_str(&scenario),
            "--artifacts",
            as_str(&artifacts),
        ]);
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "unexpected stderr: {stderr}");
        assert!(!stderr.contains("panicked"));
    }
    assert!(!artifacts.exists());
}

#[test]
fn normalized_capture_aliases_are_rejected_before_writing() {
    let dir = scratch("path-alias");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    let scenario = dir.join("alias.json");
    let artifacts = dir.join("artifacts");
    fs::write(&cart, "__lua__\n").unwrap();
    fs::write(
        &scenario,
        r#"{"version":1,"stages":[{"op":"capture","screenshot":"same//artifact.bin","screen_text":"same/artifact.bin"}]}"#,
    )
    .unwrap();

    let output = run(&[
        "playtest",
        as_str(&cart),
        "--scenario",
        as_str(&scenario),
        "--artifacts",
        as_str(&artifacts),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("aliases an earlier artifact"));
    assert!(!artifacts.exists());
}

#[test]
fn invalid_input_buttons_are_schema_errors_before_any_stage_runs() {
    let dir = scratch("input-buttons");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    let scenario = dir.join("buttons.json");
    fs::write(&cart, "__lua__\n").unwrap();
    fs::write(
        &scenario,
        r#"{"version":1,"stages":[{"op":"eval","code":"error('must not run')"},{"op":"input","frames":1,"buttons":"Q"}]}"#,
    )
    .unwrap();

    let output = run(&["playtest", as_str(&cart), "--scenario", as_str(&scenario)]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown input button"));
    assert!(!stderr.contains("must not run"));
}

#[test]
fn invalid_schema_and_escape_paths_exit_two_without_writing() {
    let dir = scratch("schema");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    fs::write(&cart, "__lua__\n").unwrap();

    let unknown = dir.join("unknown.json");
    fs::write(
        &unknown,
        r#"{"version":1,"stages":[{"op":"input","frames":1,"wat":true}]}"#,
    )
    .unwrap();
    let output = run(&["playtest", as_str(&cart), "--scenario", as_str(&unknown)]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown field"));

    let escape = dir.join("escape.json");
    fs::write(
        &escape,
        r#"{"version":1,"stages":[{"op":"capture","screenshot":"../escape.png"}]}"#,
    )
    .unwrap();
    let artifacts = dir.join("artifacts");
    let output = run(&[
        "playtest",
        as_str(&cart),
        "--scenario",
        as_str(&escape),
        "--artifacts",
        as_str(&artifacts),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not allowed"));
    assert!(!dir.join("escape.png").exists());
}

#[test]
fn scenario_captures_text_layout_events() {
    let dir = scratch("text-events");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    let scenario = dir.join("text.json");
    let artifacts = dir.join("artifacts");
    fs::write(
        &cart,
        "__lua__\nfunction _draw() print('READY', 96, 12, 14, 'center') end\n",
    )
    .unwrap();
    fs::write(
        &scenario,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "stages": [
                {"op":"input", "frames":1, "buttons":""},
                {"op":"capture", "text_events":"layout.json", "from_frame":1}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = run(&[
        "playtest",
        as_str(&cart),
        "--scenario",
        as_str(&scenario),
        "--artifacts",
        as_str(&artifacts),
        "--format",
        "json",
    ]);
    assert!(
        output.status.success(),
        "playtest failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: Value =
        serde_json::from_slice(&fs::read(artifacts.join("layout.json")).unwrap()).unwrap();
    assert_eq!(artifact["events"][0]["frame"], 1);
    assert_eq!(artifact["events"][0]["text"], "READY");
    assert_eq!(artifact["events"][0]["align"], "center");
    assert_eq!(artifact["events"][0]["x"], 86);
    assert_eq!(artifact["events"][0]["width"], 20);
}

#[test]
fn scenario_captures_authored_and_live_maps_from_one_session() {
    let dir = scratch("live-map");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    let scenario = dir.join("map.json");
    let artifacts = dir.join("artifacts");
    fs::write(&cart, "__lua__\nfunction _init() end\n\n__map__\n0102\n").unwrap();
    fs::write(
        &scenario,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "stages": [
                {"op":"eval", "code":"mset(0,0,3)"},
                {"op":"capture", "map":{
                    "dump":"authored.txt", "lint":"authored.json", "region":"0,0,2,1"
                }},
                {"op":"capture", "map":{
                    "source":"live", "png":"live.png", "dump":"live.txt",
                    "lint":"live.json", "region":"0,0,2,1", "zoom":2,
                    "grid":true, "ids":true
                }}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = run(&[
        "playtest",
        as_str(&cart),
        "--scenario",
        as_str(&scenario),
        "--artifacts",
        as_str(&artifacts),
        "--format",
        "json",
    ]);
    assert!(
        output.status.success(),
        "playtest failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["scenario"]["artifact_count"], 5);
    assert_eq!(report["stages"][1]["artifacts"][0]["kind"], "map_dump");
    assert_eq!(report["stages"][2]["artifacts"][0]["kind"], "map_png");

    assert!(
        fs::read_to_string(artifacts.join("authored.txt"))
            .unwrap()
            .contains("0102")
    );
    assert!(
        fs::read_to_string(artifacts.join("live.txt"))
            .unwrap()
            .contains("0302")
    );
    assert!(
        fs::read(artifacts.join("live.png"))
            .unwrap()
            .starts_with(b"\x89PNG")
    );
    let authored: Value =
        serde_json::from_slice(&fs::read(artifacts.join("authored.json")).unwrap()).unwrap();
    let live: Value =
        serde_json::from_slice(&fs::read(artifacts.join("live.json")).unwrap()).unwrap();
    assert_eq!(authored["tile_counts"][0]["tile"], 1);
    assert_eq!(live["tile_counts"][0]["tile"], 2);
    assert_eq!(live["tile_counts"][1]["tile"], 3);
}

#[test]
fn invalid_nested_map_capture_is_rejected_before_writing() {
    let dir = scratch("map-schema");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    let artifacts = dir.join("artifacts");
    fs::write(&cart, "__lua__\n").unwrap();

    for (file, capture, expected) in [
        (
            "empty.json",
            json!({"map":{}}),
            "map capture has no outputs",
        ),
        (
            "source.json",
            json!({"map":{"source":"snapshot", "dump":"map.txt"}}),
            "unknown variant",
        ),
        (
            "zoom.json",
            json!({"map":{"dump":"map.txt", "zoom":u32::MAX}}),
            "map capture zoom must be 1..=16",
        ),
        (
            "alias.json",
            json!({"screenshot":"same//out", "map":{"dump":"same/out"}}),
            "aliases an earlier artifact",
        ),
    ] {
        let scenario = dir.join(file);
        fs::write(
            &scenario,
            serde_json::to_vec(&json!({
                "version":1,
                "stages":[{"op":"capture", "screenshot":capture["screenshot"], "map":capture["map"]}]
            }))
            .unwrap(),
        )
        .unwrap();
        let output = run(&[
            "playtest",
            as_str(&cart),
            "--scenario",
            as_str(&scenario),
            "--artifacts",
            as_str(&artifacts),
        ]);
        assert_eq!(output.status.code(), Some(2), "case {file}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "case {file}: {stderr}");
    }
    assert!(!artifacts.exists());
}

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
fn scenario_captures_transparent_semantic_layers_beside_collision_context() {
    let dir = scratch("semantic-layers");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    let scenario = dir.join("layers.json");
    let artifacts = dir.join("artifacts");
    fs::write(
        &cart,
        "__lua__\n\
         function _draw()\n\
           draw_tag('background') cls(2)\n\
           draw_tag('terrain') rectfill(10,20,30,21,7)\n\
           draw_tag('') pset(2,2,5)\n\
           draw_tag() pset(1,2,0)\n\
         end\n\
         __map__\n0102\n",
    )
    .unwrap();
    fs::write(
        &scenario,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "stages": [
                {"op":"input", "frames":1},
                {"op":"capture", "zoom":2,
                 "layers":{
                    "background":"layers/background.png",
                    "terrain":"layers/terrain.png",
                    "":"layers/empty-name.png",
                    "__untagged__":"layers/untagged.png"
                 },
                 "map":{"source":"live", "dump":"collision.txt", "region":"0,0,2,1"}}
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
    assert_eq!(
        report["stages"][1]["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|artifact| artifact["kind"] == "layer_png")
            .count(),
        4
    );
    assert!(
        fs::read_to_string(artifacts.join("collision.txt"))
            .unwrap()
            .contains("0102")
    );

    let terrain = console_agent::palette::decode_png_rgba(
        &fs::read(artifacts.join("layers/terrain.png")).unwrap(),
    )
    .unwrap();
    assert_eq!((terrain.width, terrain.height), (384, 640));
    let alpha = |x: usize, y: usize| terrain.rgba[(y * terrain.width as usize + x) * 4 + 3];
    assert_eq!(alpha(0, 0), 0, "untouched layer pixels are transparent");
    assert_eq!(alpha(20, 40), 255, "drawn terrain pixels are opaque");

    let untagged = console_agent::palette::decode_png_rgba(
        &fs::read(artifacts.join("layers/untagged.png")).unwrap(),
    )
    .unwrap();
    let pixel = (4 * untagged.width as usize + 2) * 4;
    assert_eq!(untagged.rgba[pixel + 3], 255, "real colour 0 stays opaque");
}

#[test]
fn missing_or_invalid_layer_tags_fail_without_layer_artifacts() {
    let dir = scratch("semantic-layer-errors");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    fs::write(
        &cart,
        "__lua__\nfunction _draw() draw_tag('terrain') pset(1,1,7) end\n",
    )
    .unwrap();

    let invalid = dir.join("invalid.json");
    let mut invalid_layers = serde_json::Map::new();
    invalid_layers.insert("x".repeat(65), json!("too-long.png"));
    fs::write(
        &invalid,
        serde_json::to_vec(&json!({
            "version":1,
            "stages":[{"op":"capture", "layers":Value::Object(invalid_layers)}]
        }))
        .unwrap(),
    )
    .unwrap();
    let output = run(&[
        "playtest",
        as_str(&cart),
        "--scenario",
        as_str(&invalid),
        "--artifacts",
        as_str(&dir.join("invalid-artifacts")),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("layer tag"));

    let missing = dir.join("missing.json");
    let missing_artifacts = dir.join("missing-artifacts");
    fs::write(
        &missing,
        serde_json::to_vec(&json!({
            "version":1,
            "stages":[
                {"op":"input", "frames":1},
                {"op":"capture", "layers":{
                    "terrain":"terrain.png", "actors":"actors.png"
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
        as_str(&missing),
        "--artifacts",
        as_str(&missing_artifacts),
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["stages"][1]["error"]
            .as_str()
            .unwrap()
            .contains("requested layer \"actors\"")
    );
    assert!(!missing_artifacts.join("terrain.png").exists());
    assert!(!missing_artifacts.join("actors.png").exists());
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

#[test]
fn visual_sequence_is_deterministic_and_preserves_native_reference_pixels() {
    let dir = scratch("visual-sequence");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("motion.cart");
    let scenario = dir.join("motion.json");
    let reference = dir.join("reference.png");
    fs::write(
        &cart,
        "__lua__\nx=0\nfunction _update() x=x+1 end\nfunction _draw() cls(1) rectfill(x,10,x+3,13,12) end\n",
    )
    .unwrap();
    let reference_rgba = vec![
        255, 0, 0, 255, 0, 255, 0, 192, 0, 0, 255, 128, 255, 255, 0, 64, 255, 0, 255, 0, 10, 20,
        30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255, 130, 140, 150, 255, 160,
        170, 180, 255, 190, 200, 210, 255, 220, 230, 240, 255, 1, 2, 3, 255, 4, 5, 6, 255,
    ];
    fs::write(
        &reference,
        console_agent::palette::encode_png_rgba(&reference_rgba, 5, 3),
    )
    .unwrap();
    fs::write(
        &scenario,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "seed": 17,
            "stages": [{
                "op": "sequence",
                "name": "hop arc",
                "frames": 12,
                "buttons": "R",
                "every": 3,
                "crop": {"x":0, "y":0, "w":32, "h":24},
                "zoom": 2,
                "columns": 2,
                "gif": "motion.gif",
                "strip": "motion-strip.png",
                "board": "motion-board.png",
                "reference": "reference.png"
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let first = dir.join("first");
    let second = dir.join("second");
    let mut first_report = None;
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
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["scenario"]["frame_count"], 12);
        assert_eq!(report["scenario"]["artifact_count"], 3);
        assert_eq!(
            report["stages"][0]["actual"]["sampled_frames"],
            json!([3, 6, 9, 12])
        );
        assert_eq!(report["stages"][0]["actual"]["scaling"], "nearest_neighbor");
        assert_eq!(
            report["stages"][0]["actual"]["gif"]["delay_centiseconds"],
            5
        );
        assert_eq!(
            report["stages"][0]["actual"]["reference"]["scale"],
            "native"
        );
        assert_eq!(
            report["stages"][0]["actual"]["reference"]["pixel_aligned"],
            false
        );
        first_report.get_or_insert(report);
    }
    for name in ["motion.gif", "motion-strip.png", "motion-board.png"] {
        assert_eq!(
            fs::read(first.join(name)).unwrap(),
            fs::read(second.join(name)).unwrap(),
            "{name} should be byte-identical across identical runs"
        );
    }

    let gif_bytes = fs::read(first.join("motion.gif")).unwrap();
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(std::io::Cursor::new(gif_bytes)).unwrap();
    assert_eq!((decoder.width(), decoder.height()), (64, 48));
    let mut delays = Vec::new();
    while let Some(frame) = decoder.read_next_frame().unwrap() {
        delays.push(frame.delay);
    }
    assert_eq!(delays, vec![5, 5, 5, 5]);

    let strip =
        console_agent::palette::decode_png_rgba(&fs::read(first.join("motion-strip.png")).unwrap())
            .unwrap();
    assert_eq!((strip.width, strip.height), (280, 48));

    let report = first_report.unwrap();
    let panel = &report["stages"][0]["actual"]["reference"]["panel"];
    let panel_x = panel["x"].as_u64().unwrap() as u32;
    let panel_y = panel["y"].as_u64().unwrap() as u32;
    let board =
        console_agent::palette::decode_png_rgba(&fs::read(first.join("motion-board.png")).unwrap())
            .unwrap();
    let mut copied_reference = Vec::new();
    for y in panel_y..panel_y + 3 {
        let start = ((y * board.width + panel_x) * 4) as usize;
        copied_reference.extend_from_slice(&board.rgba[start..start + 5 * 4]);
    }
    assert_eq!(copied_reference, reference_rgba);
}

#[test]
fn invalid_visual_sequences_are_rejected_before_execution_or_writing() {
    let dir = scratch("visual-sequence-schema");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("test.cart");
    let artifacts = dir.join("artifacts");
    fs::write(
        &cart,
        "__lua__\nfunction _init() error('must not load') end\n",
    )
    .unwrap();

    let cases = [
        (json!({"frames":0, "gif":"x.gif"}), "frames must be >= 1"),
        (
            json!({"frames":1, "every":0, "gif":"x.gif"}),
            "every must be >= 1",
        ),
        (
            json!({"frames":5, "every":2, "gif":"x.gif"}),
            "exactly divisible",
        ),
        (json!({"frames":241, "gif":"x.gif"}), "at most 240"),
        (
            json!({"frames":1, "crop":{"x":191,"y":0,"w":2,"h":1}, "gif":"x.gif"}),
            "exceeds the 192x320 screen",
        ),
        (json!({"frames":1}), "has no outputs"),
        (
            json!({"frames":1, "gif":"x.gif", "reference":"reference.png"}),
            "reference requires a board",
        ),
        (
            json!({"frames":1, "zoom":0, "gif":"x.gif"}),
            "zoom must be 1..=16",
        ),
        (
            json!({"frames":1, "columns":17, "board":"x.png"}),
            "columns must be 1..=16",
        ),
        (
            json!({"frames":1, "gif":"same//x", "strip":"same/x"}),
            "aliases an earlier artifact",
        ),
        (
            json!({"frames":240, "zoom":2, "strip":"huge.png"}),
            "exceeding the 67108864 byte limit",
        ),
        (
            json!({"frames":240, "zoom":16, "gif":"huge.gif"}),
            "sequence GIF aggregate RGBA work",
        ),
    ];
    for (index, (fields, expected)) in cases.into_iter().enumerate() {
        let mut stage = fields.as_object().unwrap().clone();
        stage.insert("op".to_string(), json!("sequence"));
        let scenario = dir.join(format!("invalid-{index}.json"));
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
        assert_eq!(output.status.code(), Some(2), "case {index}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "case {index}: {stderr}");
        assert!(!stderr.contains("must not load"), "case {index}: {stderr}");
    }
    assert!(!artifacts.exists());
}

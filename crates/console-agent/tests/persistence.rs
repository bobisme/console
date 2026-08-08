use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::rpc;
use console_agent::session::Session;
use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn scratch(label: &str) -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "console-persistence-{}-{n}-{label}",
        std::process::id()
    ))
}

fn write_cart(path: &Path, lua: &str) {
    std::fs::write(
        path,
        format!(
            "__meta__\ntitle=Persistence Test\nsave_id=org.example.persistence-test\nsave_version=2\n\n__lua__\n{lua}\n"
        ),
    )
    .unwrap();
}

#[test]
fn explicit_native_sidecar_round_trips_between_runs() {
    let dir = scratch("sidecar");
    std::fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("game.cart");
    let save = dir.join("game.save.json");
    write_cart(
        &cart,
        "local data=save_load() or {runs=0}\nfunction _init() data.runs=data.runs+1; assert(save_store(data)) end",
    );

    for expected in [1, 2] {
        let output = Command::new(env!("CARGO_BIN_EXE_console"))
            .args([
                "run",
                cart.to_str().unwrap(),
                "--save-file",
                save.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: Value = serde_json::from_slice(&std::fs::read(&save).unwrap()).unwrap();
        assert_eq!(document["data"]["runs"], expected);
        assert_eq!(document["version"], 2);
    }
}

#[test]
fn failed_frame_does_not_overwrite_native_sidecar() {
    let dir = scratch("failed-frame");
    std::fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("game.cart");
    let save = dir.join("game.save.json");
    let initial = r#"{"data":{"x":1},"id":"org.example.persistence-test","version":2}"#;
    std::fs::write(&save, initial).unwrap();
    write_cart(
        &cart,
        "function _update() save_store({x=2}); error('boom') end",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_console"))
        .args([
            "run",
            cart.to_str().unwrap(),
            "--frames",
            "1",
            "--save-file",
            save.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(std::fs::read_to_string(save).unwrap(), initial);
}

#[test]
fn native_adapter_flushes_an_earlier_success_before_a_later_crash() {
    let dir = scratch("flush-before-crash");
    std::fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("game.cart");
    let save = dir.join("game.save.json");
    write_cart(
        &cart,
        "local frame=0\nfunction _update() frame=frame+1; if frame==1 then save_store({x=7}) else error('later crash') end end",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_console"))
        .args([
            "run",
            cart.to_str().unwrap(),
            "--frames",
            "2",
            "--save-file",
            save.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let document: Value = serde_json::from_slice(&std::fs::read(save).unwrap()).unwrap();
    assert_eq!(document["data"]["x"], 7);
}

#[test]
fn playtest_injects_asserts_and_captures_save_data() {
    let dir = scratch("playtest");
    let artifacts = dir.join("artifacts");
    std::fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("game.cart");
    let scenario = dir.join("scenario.json");
    write_cart(
        &cart,
        "local data,old=save_load(); seen_version=old\nfunction _update() data.x=data.x+1; save_store(data) end",
    );
    std::fs::write(
        &scenario,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "initial_save": {"version": 1, "data": {"x": 4}},
            "stages": [
                {"op":"assert", "code":"return seen_version", "equals":1},
                {"op":"input", "frames":1},
                {"op":"save_assert", "version":2, "equals":{"x":5}},
                {"op":"capture", "save":"final-save.json"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_console"))
        .args([
            "playtest",
            cart.to_str().unwrap(),
            "--scenario",
            scenario.to_str().unwrap(),
            "--artifacts",
            artifacts.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["scenario"]["status"], "passed");
    let captured: Value =
        serde_json::from_slice(&std::fs::read(artifacts.join("final-save.json")).unwrap()).unwrap();
    assert_eq!(captured["data"]["x"], 5);
    assert_eq!(captured["version"], 2);
}

#[test]
fn rpc_uses_explicit_ephemeral_save_input_and_inspection() {
    let mut session = Session::new();
    let cart = "__meta__\nsave_id=org.example.rpc\nsave_version=3\n__lua__\nlocal d,v=save_load(); loaded=d.x; old=v; save_store({x=d.x+1})\n";
    let loaded = rpc::handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"load_cart","params":{
            "text":cart,"save":{"version":2,"data":{"x":9}}
        }}),
    );
    assert_eq!(loaded["result"]["ok"], true);
    let saved = rpc::handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":2,"method":"save_data","params":{}}),
    );
    assert_eq!(saved["result"]["document"]["data"]["x"], 10);
    assert_eq!(saved["result"]["document"]["version"], 3);
    assert_eq!(saved["result"]["revision"], 1);
}

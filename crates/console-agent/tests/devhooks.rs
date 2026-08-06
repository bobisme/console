use console_agent::hooks::dev_value_to_json;
use console_agent::rpc::handle;
use console_agent::session::Session;
use console_core::{DevHookPhase, DevValue};
use serde_json::json;
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const CART: &str = r#"__lua__
value = 0
devhook.register("add_before", {
  description="Add before the first frame",
  phase="pre_frame",
  run=function(args) value = value + args return value end,
})
devhook.register("add_after", {
  description="Add at a completed frame boundary",
  phase="post_frame",
  run=function(args) value = value + args return value end,
})
devhook.register("status", {
  description="Return current state",
  phase="post_frame",
  run=function(_) return {value=value} end,
})
function _update() value = value * 10 + 1 end
"#;

fn status(session: &mut Session) -> i64 {
    let value = session
        .invoke_dev_hook("status", DevHookPhase::PostFrame, DevValue::Null)
        .unwrap();
    dev_value_to_json(&value.result)["value"].as_i64().unwrap()
}

#[test]
fn save_load_replays_hook_and_step_order_and_reset_drops_calls() {
    let mut session = Session::new();
    session.load_cart(CART, 7).unwrap();
    session
        .invoke_dev_hook("add_before", DevHookPhase::PreFrame, DevValue::Integer(2))
        .unwrap();
    session.step(1, 0).unwrap(); // 2 -> 21
    session
        .invoke_dev_hook("add_after", DevHookPhase::PostFrame, DevValue::Integer(3))
        .unwrap(); // 24
    session.step(1, 0).unwrap(); // 241
    session.save_state("ordered").unwrap();

    session
        .invoke_dev_hook("add_after", DevHookPhase::PostFrame, DevValue::Integer(100))
        .unwrap();
    assert_eq!(status(&mut session), 341);
    let loaded = session.load_state("ordered").unwrap();
    assert_eq!(loaded.frame_count, 2);
    assert_eq!(status(&mut session), 241);

    session.reset(None).unwrap();
    assert_eq!(session.console().unwrap().frame_count(), 0);
    assert_eq!(
        status(&mut session),
        0,
        "reset must drop recorded hook calls"
    );
}

#[test]
fn pre_frame_hook_is_rejected_after_stepping_without_calling_lua() {
    let mut session = Session::new();
    session.load_cart(CART, 0).unwrap();
    session.step(1, 0).unwrap();
    let error = session
        .invoke_dev_hook("add_before", DevHookPhase::PreFrame, DevValue::Integer(9))
        .unwrap_err();
    assert!(error.to_string().contains("cannot run after frame 0"));
    assert_eq!(status(&mut session), 1);
}

#[test]
fn rpc_discovers_and_invokes_with_structured_response() {
    let mut session = Session::new();
    session.load_cart(CART, 0).unwrap();
    let listed = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"dev_hooks","params":{}}),
    );
    assert_eq!(listed["result"]["frame_count"], 0);
    assert_eq!(listed["result"]["hooks"][0]["name"], "add_before");
    assert_eq!(listed["result"]["hooks"][0]["phase"], "pre_frame");

    let invoked = handle(
        &mut session,
        json!({
            "jsonrpc":"2.0","id":2,"method":"dev_hook",
            "params":{"name":"add_before","args":4}
        }),
    );
    assert_eq!(
        invoked["result"],
        json!({"name":"add_before","phase":"pre_frame","frame_count":0,"result":4})
    );
    let unknown = handle(
        &mut session,
        json!({
            "jsonrpc":"2.0","id":3,"method":"dev_hook",
            "params":{"name":"missing"}
        }),
    );
    assert_eq!(unknown["error"]["code"], -32602);
}

#[test]
fn oneshot_hooks_are_structured_and_eval_after_stays_last() {
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "console-devhooks-{}-{serial}.cart",
        std::process::id()
    ));
    fs::write(&path, CART).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_console"))
        .args([
            "run",
            path.to_str().unwrap(),
            "--frames",
            "1",
            "--hook-before",
            "add_before=2",
            "--hook-after",
            "add_after=3",
            "--eval-after",
            "return value",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = String::from_utf8(output.stdout).unwrap();
    let lines = lines.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 3);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(lines[0]).unwrap()["result"],
        2
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(lines[1]).unwrap()["result"],
        24
    );
    assert_eq!(
        lines[2], "24",
        "eval-after must remain the final stdout record"
    );
}

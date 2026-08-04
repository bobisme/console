use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::rpc::handle;
use console_agent::session::{MAX_SESSION_DRAW_EVENTS, Session};
use serde_json::{Value, json};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const CART: &str = "__lua__\n\
function _draw()\n\
  draw_tag('actors')\n\
  pset(10,20,7)\n\
  draw_tag('fx')\n\
  rectfill(30,40,32,42,9)\n\
end\n";

fn scratch(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "console-draw-trace-{}-{n}-{name}",
        std::process::id()
    ))
}

fn rpc(session: &mut Session, id: u64, method: &str, params: Value) -> Value {
    handle(
        session,
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("temporary path is UTF-8")
}

#[test]
fn rpc_controls_filters_and_clears_draw_events() {
    let mut session = Session::new();
    let missing = rpc(&mut session, 1, "draw_events", json!({}));
    assert_eq!(missing["error"]["code"], -32002);

    assert_eq!(
        rpc(&mut session, 2, "load_cart", json!({"text": CART}))["result"]["ok"],
        true
    );
    let control = rpc(&mut session, 3, "draw_trace", json!({"enabled":true}));
    assert_eq!(control["result"]["enabled"], true);
    assert_eq!(control["result"]["capacity"], 65_536);
    assert_eq!(control["result"]["event_count"], 0);
    rpc(&mut session, 4, "step", json!({"frames":2,"input":""}));

    let unchanged = rpc(&mut session, 41, "draw_trace", json!({"enabled":true}));
    assert_eq!(unchanged["result"]["event_count"], 4);

    let filtered = rpc(
        &mut session,
        5,
        "draw_events",
        json!({"from_frame":2,"tag":"actors"}),
    );
    let events = filtered["result"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["frame"], 2);
    assert_eq!(events[0]["index"], 0);
    assert_eq!(events[0]["op"], "pset");
    assert_eq!(events[0]["tag"], "actors");
    assert_eq!(
        events[0]["world_bounds"],
        json!({"x":10,"y":20,"w":1,"h":1})
    );

    let drained = rpc(&mut session, 6, "draw_events", json!({"clear":true}));
    assert_eq!(drained["result"]["events"].as_array().unwrap().len(), 4);
    let empty = rpc(&mut session, 7, "draw_events", json!({}));
    assert_eq!(empty["result"]["events"], json!([]));
}

#[test]
fn rpc_rejects_mistyped_draw_trace_parameters() {
    let mut session = Session::new();
    rpc(&mut session, 1, "load_cart", json!({"text": CART}));
    for (method, params) in [
        ("draw_trace", json!({"enabled":"yes"})),
        ("draw_trace", json!({})),
        ("draw_events", json!({"from_frame":-1})),
        ("draw_events", json!({"tag":2})),
        ("draw_events", json!({"clear":1})),
    ] {
        let response = rpc(&mut session, 2, method, params);
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }

    let mut atomic = Session::new();
    let response = rpc(
        &mut atomic,
        3,
        "draw_trace",
        json!({"enabled":true,"clear":1}),
    );
    assert_eq!(response["error"]["code"], -32602);
    assert!(!atomic.draw_tracing(), "bad params must not change tracing");
}

#[test]
fn load_state_replays_the_same_draw_trace() {
    let mut session = Session::new();
    session.set_draw_tracing(true);
    session.load_cart(CART, 11).unwrap();
    session.step(2, 0).unwrap();
    session.save_state("mid").unwrap();
    session.step(1, 0).unwrap();
    let continuous = serde_json::to_value(session.draw_events(None, None).unwrap()).unwrap();

    session.load_state("mid").unwrap();
    session.step(1, 0).unwrap();
    let replayed = serde_json::to_value(session.draw_events(None, None).unwrap()).unwrap();
    assert_eq!(replayed, continuous);
}

#[test]
fn eval_draws_continue_the_current_frames_stable_order() {
    let mut session = Session::new();
    session.set_draw_tracing(true);
    session.load_cart(CART, 0).unwrap();
    session.step(1, 0).unwrap();
    session.eval("draw_tag('debug') pset(1,1,1)").unwrap();
    session.eval("pset(2,2,2)").unwrap();
    let trace = session.draw_events(None, None).unwrap();
    assert_eq!(
        trace
            .events
            .iter()
            .map(|event| event.index)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
}

#[test]
fn session_trace_retains_the_newest_events_and_counts_every_drop() {
    let cart = "__lua__\nfunction _draw() for i=1,5000 do pset(i,1,1) end end\n";
    let mut session = Session::new();
    session.set_draw_tracing(true);
    session.load_cart(cart, 0).unwrap();
    session.step(17, 0).unwrap();
    let trace = session.draw_events(None, None).unwrap();
    assert_eq!(trace.events.len(), MAX_SESSION_DRAW_EVENTS);
    assert_eq!(trace.events.first().unwrap().frame, 2);
    assert_eq!(trace.events.last().unwrap().frame, 17);
    let per_frame_drops = 5000 - console_core::MAX_DRAW_EVENTS_PER_FRAME;
    let ring_drops = 17 * console_core::MAX_DRAW_EVENTS_PER_FRAME - MAX_SESSION_DRAW_EVENTS;
    assert_eq!(trace.dropped as usize, 17 * per_frame_drops + ring_drops);
}

#[test]
fn run_and_playtest_write_deterministic_trace_artifacts() {
    let dir = scratch("artifacts");
    fs::create_dir_all(&dir).unwrap();
    let cart = dir.join("trace.cart");
    let run_trace = dir.join("run-trace.json");
    fs::write(&cart, CART).unwrap();

    let run = Command::new(env!("CARGO_BIN_EXE_console"))
        .args([
            "run",
            path_str(&cart),
            "--frames",
            "2",
            "--draw-trace",
            path_str(&run_trace),
        ])
        .output()
        .expect("run console");
    assert!(
        run.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let run_json: Value = serde_json::from_slice(&fs::read(&run_trace).unwrap()).unwrap();
    assert_eq!(run_json["enabled"], true);
    assert_eq!(run_json["events"].as_array().unwrap().len(), 4);

    let scenario = dir.join("scenario.json");
    fs::write(
        &scenario,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "seed": 11,
            "stages": [
                {"op":"input","frames":2,"buttons":""},
                {"op":"capture","draw_trace":"trace.json","from_frame":2}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let first = dir.join("first");
    let second = dir.join("second");
    for artifacts in [&first, &second] {
        let output = Command::new(env!("CARGO_BIN_EXE_console"))
            .args([
                "playtest",
                path_str(&cart),
                "--scenario",
                path_str(&scenario),
                "--artifacts",
                path_str(artifacts),
                "--format",
                "json",
            ])
            .output()
            .expect("run playtest");
        assert!(
            output.status.success(),
            "playtest failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["scenario"]["artifact_count"], 1);
        assert_eq!(report["stages"][1]["artifacts"][0]["kind"], "draw_trace");
    }
    let a = fs::read(first.join("trace.json")).unwrap();
    let b = fs::read(second.join("trace.json")).unwrap();
    assert_eq!(a, b);
    let trace: Value = serde_json::from_slice(&a).unwrap();
    assert_eq!(trace["events"].as_array().unwrap().len(), 2);
    assert!(
        trace["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["frame"] == 2)
    );
}

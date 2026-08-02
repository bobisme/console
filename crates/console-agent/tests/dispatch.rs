//! Unit tests for the dispatch layer (`console_agent::rpc::handle`) driven
//! directly against a [`Session`], with no process spawning involved.

use std::collections::HashSet;
use std::fs;

use console_agent::rpc::{handle, handle_line};
use console_agent::session::Session;
use serde_json::json;

fn demo_cart_text() -> String {
    let path = format!("{}/../../carts/demo.cart", env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("console-agent-test-{}-{name}", std::process::id()))
}

#[test]
fn full_session_flow_against_demo_cart() {
    let mut session = Session::new();
    let cart = demo_cart_text();

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"text": cart.as_str(), "seed": 42}}),
    );
    assert!(resp.get("error").is_none(), "unexpected error: {resp}");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["ok"], true);
    assert_eq!(resp["result"]["title"], "Micro Dash");

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 2, "method": "step", "params": {"frames": 10, "input": "R"}}),
    );
    assert_eq!(resp["result"]["frame_count"], 10);
    assert_eq!(resp["result"]["halted"], false);

    // screenshot: write PNG, decode it back, and check it isn't a flat
    // single-color image.
    let path = temp_path("screenshot.png");
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 3, "method": "screenshot", "params": {"path": path.to_str().unwrap()}}),
    );
    assert_eq!(resp["result"]["ok"], true);

    let decoder = png::Decoder::new(std::io::BufReader::new(
        fs::File::open(&path).expect("screenshot file exists"),
    ));
    let mut reader = decoder.read_info().expect("valid png header");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("known buffer size")];
    let info = reader.next_frame(&mut buf).expect("decode png frame");
    assert_eq!(info.width, 144);
    assert_eq!(info.height, 256);
    let pixels = &buf[..info.buffer_size()];
    let channels = info.color_type.samples();
    let mut colors: HashSet<&[u8]> = HashSet::new();
    for px in pixels.chunks_exact(channels) {
        colors.insert(px);
    }
    assert!(
        colors.len() >= 3,
        "expected at least 3 distinct colors in the screenshot, got {}",
        colors.len()
    );
    let _ = fs::remove_file(&path);

    // screen_text: 256 lines of 144 hex chars.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 4, "method": "screen_text", "params": {}}),
    );
    let lines = resp["result"]["lines"].as_array().expect("lines array");
    assert_eq!(lines.len(), 256);
    for line in lines {
        let s = line.as_str().unwrap();
        assert_eq!(s.len(), 144);
        assert!(
            s.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    // eval: array-shaped and object-shaped tables.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 5, "method": "eval", "params": {"code": "return {10, 20, 30}"}}),
    );
    assert_eq!(resp["result"]["result"], json!([10, 20, 30]));

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 6, "method": "eval", "params": {"code": "return {a=1, b=2}"}}),
    );
    assert_eq!(resp["result"]["result"], json!({"a": 1, "b": 2}));

    // get_global.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 7, "method": "get_global", "params": {"name": "_init"}}),
    );
    assert_eq!(resp["result"]["result"], "<function>");

    // logs: drains printh output buffered since _init.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 8, "method": "logs", "params": {}}),
    );
    let logs = resp["result"]["logs"].as_array().expect("logs array");
    assert!(
        logs.iter()
            .any(|l| l.as_str().unwrap().contains("micro dash init")),
        "expected init log line, got {logs:?}"
    );

    // A second logs call drains nothing new.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 9, "method": "logs", "params": {}}),
    );
    assert_eq!(resp["result"]["logs"], json!([]));

    // info.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 10, "method": "info", "params": {}}),
    );
    assert_eq!(resp["result"]["frame_count"], 10);
    assert_eq!(resp["result"]["seed"], 42);
    assert_eq!(resp["result"]["halted"], false);
    assert_eq!(resp["result"]["title"], "Micro Dash");
    assert_eq!(resp["result"]["input_log_len"], 10);
}

#[test]
fn input_string_and_int_mask_are_equivalent() {
    let cart = demo_cart_text();
    let mut by_string = Session::new();
    let mut by_int = Session::new();

    handle(
        &mut by_string,
        json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"text": cart.as_str(), "seed": 1}}),
    );
    handle(
        &mut by_int,
        json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"text": cart.as_str(), "seed": 1}}),
    );

    handle(
        &mut by_string,
        json!({"jsonrpc": "2.0", "id": 2, "method": "step", "params": {"frames": 20, "input": "RA"}}),
    );
    let mask = console_core::input::RIGHT | console_core::input::A;
    handle(
        &mut by_int,
        json!({"jsonrpc": "2.0", "id": 2, "method": "step", "params": {"frames": 20, "input": mask}}),
    );

    assert_eq!(
        by_string.console().unwrap().framebuffer(),
        by_int.console().unwrap().framebuffer()
    );
}

#[test]
fn save_state_and_load_state_reproduce_continuous_framebuffer() {
    let cart = demo_cart_text();

    // Continuous 90-frame run.
    let mut continuous = Session::new();
    handle(
        &mut continuous,
        json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"text": cart.as_str(), "seed": 7}}),
    );
    handle(
        &mut continuous,
        json!({"jsonrpc": "2.0", "id": 2, "method": "step", "params": {"frames": 90, "input": "R"}}),
    );
    let continuous_fb = continuous.console().unwrap().framebuffer().to_vec();

    // Step 60, save, step 30 more (to be undone), load, replay identically.
    let mut split = Session::new();
    handle(
        &mut split,
        json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"text": cart.as_str(), "seed": 7}}),
    );
    handle(
        &mut split,
        json!({"jsonrpc": "2.0", "id": 2, "method": "step", "params": {"frames": 60, "input": "R"}}),
    );
    let resp = handle(
        &mut split,
        json!({"jsonrpc": "2.0", "id": 3, "method": "save_state", "params": {"name": "mid"}}),
    );
    assert_eq!(resp["result"]["ok"], true);

    handle(
        &mut split,
        json!({"jsonrpc": "2.0", "id": 4, "method": "step", "params": {"frames": 30, "input": "R"}}),
    );

    let resp = handle(
        &mut split,
        json!({"jsonrpc": "2.0", "id": 5, "method": "load_state", "params": {"name": "mid"}}),
    );
    assert_eq!(resp["result"]["replayed_frames"], 60);
    assert_eq!(resp["result"]["halted"], false);

    handle(
        &mut split,
        json!({"jsonrpc": "2.0", "id": 6, "method": "step", "params": {"frames": 30, "input": "R"}}),
    );

    let split_fb = split.console().unwrap().framebuffer().to_vec();
    assert_eq!(
        continuous_fb, split_fb,
        "replayed framebuffer must match the continuous run"
    );
}

#[test]
fn halted_cart_reports_error_and_session_stays_alive() {
    let mut session = Session::new();
    let broken = "__lua__\nfunction _update() error('boom') end\n";

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"text": broken}}),
    );
    assert!(
        resp.get("error").is_none(),
        "load_cart of a valid-but-crashy cart should succeed: {resp}"
    );

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 2, "method": "step", "params": {"frames": 1}}),
    );
    assert_eq!(resp["result"]["halted"], true);
    assert!(resp["result"]["message"].as_str().unwrap().contains("boom"));

    // Stepping an already-halted console is a JSON-RPC error...
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 3, "method": "step", "params": {"frames": 1}}),
    );
    assert_eq!(resp["error"]["code"], -32000);

    // ...but the session is still alive and answers other queries.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 4, "method": "info", "params": {}}),
    );
    assert_eq!(resp["result"]["halted"], true);
}

#[test]
fn no_cart_loaded_is_reported_with_dedicated_code() {
    let mut session = Session::new();
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "step", "params": {"frames": 1}}),
    );
    assert_eq!(resp["error"]["code"], -32002);
    assert_eq!(resp["error"]["message"], "no cart loaded");
}

#[test]
fn unknown_method_is_reported() {
    let mut session = Session::new();
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "no_such_method", "params": {}}),
    );
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn bad_params_is_reported() {
    let mut session = Session::new();
    handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"text": demo_cart_text().as_str()}}),
    );

    // Missing "code" for eval.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 2, "method": "eval", "params": {}}),
    );
    assert_eq!(resp["error"]["code"], -32602);

    // Neither "path" nor "text" for load_cart.
    let mut fresh = Session::new();
    let resp = handle(
        &mut fresh,
        json!({"jsonrpc": "2.0", "id": 3, "method": "load_cart", "params": {}}),
    );
    assert_eq!(resp["error"]["code"], -32602);
}

#[test]
fn malformed_json_line_is_a_parse_error() {
    let mut session = Session::new();
    let line = handle_line(&mut session, "{not valid json");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["error"]["code"], -32700);
    assert_eq!(resp["id"], serde_json::Value::Null);
}

#[test]
fn cart_parse_error_on_load_is_a_cart_error() {
    let mut session = Session::new();
    // No __lua__ section at all.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"text": "__meta__\ntitle=x\n"}}),
    );
    assert_eq!(resp["error"]["code"], -32000);
    assert!(resp["error"]["data"]["message"].is_string());
}

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
    std::env::temp_dir().join(format!("console-test-{}-{name}", std::process::id()))
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
    assert_eq!(info.width, 192);
    assert_eq!(info.height, 320);
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

    // screen_text: 320 lines of 192 palette characters.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 4, "method": "screen_text", "params": {}}),
    );
    let lines = resp["result"]["lines"].as_array().expect("lines array");
    assert_eq!(resp["result"]["framebuffer_width"], 192);
    assert_eq!(resp["result"]["framebuffer_height"], 320);
    assert_eq!(
        resp["result"]["region"],
        json!({"x":0,"y":0,"width":192,"height":320})
    );
    assert_eq!(resp["result"]["truncation"]["truncated"], false);
    assert_eq!(lines.len(), 320);
    for line in lines {
        let s = line.as_str().unwrap();
        assert_eq!(s.len(), 192);
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
fn screen_text_rpc_supports_bounded_regions_and_compact_summaries() {
    let mut session = Session::new();
    let cart = "__lua__\nfunction _draw() cls(0) rectfill(2,3,4,4,5) pset(10,10,63) end\n";
    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"text":cart}}),
    );
    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":2,"method":"step","params":{"frames":1}}),
    );

    let cropped = handle(
        &mut session,
        json!({
            "jsonrpc":"2.0","id":3,"method":"screen_text",
            "params":{"region":{"x":1,"y":2,"width":5,"height":4}}
        }),
    );
    assert_eq!(
        cropped["result"]["lines"],
        json!(["00000", "05550", "05550", "00000"])
    );
    assert_eq!(cropped["result"]["palette_counts"]["5"], 6);
    assert_eq!(cropped["result"]["glyph_counts"]["5"], 6);
    assert_eq!(
        cropped["result"]["non_background_bounds"],
        json!({"x":2,"y":3,"width":3,"height":2})
    );
    assert_eq!(cropped["result"]["truncation"]["crop_right"], 186);

    let summary = handle(
        &mut session,
        json!({
            "jsonrpc":"2.0","id":4,"method":"screen_text",
            "params":{"summary":true}
        }),
    );
    assert!(summary["result"].get("lines").is_none());
    assert_eq!(summary["result"]["palette_counts"]["63"], 1);
    assert_eq!(summary["result"]["glyph_counts"]["_"], 1);
    assert_eq!(summary["result"]["truncation"]["cropped_pixels"], 0);
    assert_eq!(summary["result"]["truncation"]["lines_omitted"], true);
    assert_eq!(
        summary["result"]["truncation"]["line_characters_omitted"],
        192 * 320
    );
}

#[test]
fn screen_text_rpc_rejects_bad_bounds_types_unknowns_and_unbounded_crops() {
    let mut session = Session::new();
    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"text":"__lua__\n"}}),
    );

    let bad_params = [
        json!({"region":{"x":0,"y":0,"width":0,"height":1}}),
        json!({"region":{"x":191,"y":0,"width":2,"height":1}}),
        json!({"region":{"x":0,"y":0,"width":1.5,"height":1}}),
        json!({"region":{"x":0,"y":0,"width":1,"height":1,"extra":true}}),
        json!({"summary":"yes"}),
        json!({"extra":true}),
        json!({"region":{"x":0,"y":0,"width":192,"height":100}}),
    ];
    for (index, params) in bad_params.into_iter().enumerate() {
        let response = handle(
            &mut session,
            json!({"jsonrpc":"2.0","id":index,"method":"screen_text","params":params}),
        );
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }

    let bounded_summary = handle(
        &mut session,
        json!({
            "jsonrpc":"2.0","id":99,"method":"screen_text",
            "params":{"summary":true,"region":{"x":0,"y":0,"width":192,"height":100}}
        }),
    );
    assert!(bounded_summary.get("error").is_none(), "{bounded_summary}");
}

#[test]
fn ecs_query_is_bounded_projected_and_replay_stable() {
    let cart = r#"__lua__
world = ecs.world("game", {capacity=32})
function _init()
  for i=1,6 do world:spawn({pos={x=i,y=i*2,private=i*99},bullet={kind="arc"},tag=true}) end
end
function _update()
  world:each({"pos"}, function(_,pos) pos.x=pos.x+1 end)
end
"#;
    let mut session = Session::new();
    let loaded = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"text":cart}}),
    );
    assert!(loaded.get("error").is_none(), "load failed: {loaded}");
    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":2,"method":"step","params":{"frames":3}}),
    );
    let query = json!({
        "jsonrpc":"2.0","id":3,"method":"ecs_query","params":{
            "world":"game",
            "with":["bullet"],
            "select":{"pos":["x","y"],"bullet":["kind"],"tag":[]},
            "limit":2
        }
    });
    let first = handle(&mut session, query.clone());
    assert!(first.get("error").is_none(), "query failed: {first}");
    assert_eq!(first["result"]["frame_count"], 3);
    assert_eq!(first["result"]["alive"], 6);
    assert_eq!(first["result"]["matched"], 6);
    assert_eq!(first["result"]["returned"], 2);
    assert_eq!(first["result"]["truncated"], true);
    assert_eq!(first["result"]["next_after"], 2);
    assert_eq!(
        first["result"]["entities"][0]["components"]["pos"],
        json!({"x":4,"y":2})
    );
    assert!(first["result"]["entities"][0]["components"]["pos"]["private"].is_null());

    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":4,"method":"save_state","params":{"name":"three"}}),
    );
    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":5,"method":"step","params":{"frames":4}}),
    );
    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":6,"method":"load_state","params":{"name":"three"}}),
    );
    let replayed = handle(&mut session, query);
    assert_eq!(first["result"], replayed["result"]);

    // The host calls its registry-held inspector, not the cart-replaceable
    // public global.
    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":7,"method":"eval","params":{"code":"ecs=nil"}}),
    );
    let protected = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":8,"method":"ecs_query","params":{"world":"game","limit":1}}),
    );
    assert_eq!(protected["result"]["alive"], 6);
}

#[test]
fn ecs_query_rejects_unbounded_or_malformed_requests() {
    let mut session = Session::new();
    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"text":"__lua__\nworld=ecs.world('game')\n"}}),
    );
    for (params, needle) in [
        (json!({"world":"game","limit":0}), "limit"),
        (json!({"world":"game","limit":129}), "limit"),
        (json!({"world":"game","with":"bullet"}), "with"),
        (json!({"world":"bad world"}), "begin with"),
        (json!({"world":"game","with":["9bad"]}), "begin with"),
        (
            json!({"world":"game","select":{"pos":"x"}}),
            "select component",
        ),
        (json!({"limit":1}), "world"),
    ] {
        let response = handle(
            &mut session,
            json!({"jsonrpc":"2.0","id":2,"method":"ecs_query","params":params}),
        );
        assert_eq!(response["error"]["code"], -32602, "{response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains(needle),
            "{response}"
        );
    }
}

#[test]
fn text_events_report_anchors_bounds_camera_and_frame_filters() {
    let cart = "__lua__\n\
function _init() print('INIT', 96, 2, 7, 'center') end\n\
function _draw() camera(4, 0) print('HI', 96, 10, 9, 'center') end\n";
    let mut session = Session::new();

    let loaded = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"text":cart}}),
    );
    assert!(loaded.get("error").is_none(), "load failed: {loaded}");

    let init = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":2,"method":"text_events","params":{}}),
    );
    let init_events = init["result"].as_array().unwrap();
    assert_eq!(init_events.len(), 1);
    assert_eq!(init_events[0]["frame"], 0);
    assert_eq!(init_events[0]["text"], "INIT");
    assert_eq!(init_events[0]["align"], "center");
    assert_eq!(init_events[0]["x"], 88);
    assert_eq!(init_events[0]["width"], 16);

    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":3,"method":"step","params":{"frames":1}}),
    );
    let frame_one = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":4,"method":"text_events","params":{"from_frame":1}}),
    );
    let events = frame_one["result"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["frame"], 1);
    assert_eq!(events[0]["anchor_x"], 96);
    assert_eq!(events[0]["screen_anchor_x"], 92);
    assert_eq!(events[0]["x"], 88);
    assert_eq!(events[0]["y"], 10);
    assert_eq!(events[0]["width"], 8);
    assert_eq!(events[0]["height"], 6);
    assert_eq!(events[0]["color"], 9);
    assert_eq!(events[0]["visible"], true);
    assert_eq!(events[0]["clipped"], false);
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
    let continuous_text_events = continuous.text_events(None).unwrap();

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
    assert_eq!(
        continuous_text_events,
        split.text_events(None).unwrap(),
        "replayed text diagnostics must match the continuous run"
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

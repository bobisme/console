//! Regression coverage for the showcase cart's deterministic gameplay loop.

use std::fs;

use console_agent::rpc::handle;
use console_agent::session::Session;
use serde_json::{Value, json};

fn cart_text() -> String {
    let path = format!(
        "{}/../../carts/lantern-leap.cart",
        env!("CARGO_MANIFEST_DIR")
    );
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn request(session: &mut Session, id: u32, method: &str, params: Value) -> Value {
    let response = handle(
        session,
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    );
    assert!(
        response.get("error").is_none(),
        "{method} failed: {response}"
    );
    response["result"].clone()
}

#[test]
fn cart_can_collect_unlock_and_win_through_agent_hooks() {
    let mut session = Session::new();
    let cart = cart_text();
    let loaded = request(
        &mut session,
        1,
        "load_cart",
        json!({"text": cart, "seed": 0}),
    );
    assert_eq!(loaded["title"], "Lantern Leap");

    request(&mut session, 2, "step", json!({"frames": 1, "input": "A"}));
    request(&mut session, 3, "step", json!({"frames": 20, "input": ""}));

    for (id, (x, y)) in [(4, (48, 449)), (7, (112, 353)), (10, (24, 129))] {
        request(
            &mut session,
            id,
            "eval",
            json!({"code": format!("dev_warp({x},{y})")}),
        );
        request(
            &mut session,
            id + 1,
            "step",
            json!({"frames": 1, "input": ""}),
        );
    }
    let embers = request(&mut session, 13, "get_global", json!({"name": "embers"}));
    assert_eq!(embers["result"], 3);

    request(
        &mut session,
        14,
        "eval",
        json!({"code": "dev_warp(104,32)"}),
    );
    request(&mut session, 15, "step", json!({"frames": 1, "input": ""}));
    let gate = request(&mut session, 16, "get_global", json!({"name": "gate_open"}));
    assert_eq!(gate["result"], true);

    request(
        &mut session,
        17,
        "eval",
        json!({"code": "dev_warp(120,32)"}),
    );
    request(&mut session, 18, "step", json!({"frames": 1, "input": ""}));
    let scene = request(
        &mut session,
        19,
        "get_global",
        json!({"name": "game_scene"}),
    );
    assert_eq!(scene["result"], "win");

    let logs = request(&mut session, 20, "logs", json!({}));
    assert!(
        logs["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| { line.as_str().is_some_and(|line| line.contains("tower lit")) })
    );
}

#[test]
fn scripted_inputs_produce_identical_framebuffers() {
    let cart = cart_text();
    let mut first = Session::new();
    let mut second = Session::new();
    first.load_cart(&cart, 99).unwrap();
    second.load_cart(&cart, 99).unwrap();

    let script = [
        (1, console_core::input::A),
        (20, 0),
        (23, console_core::input::LEFT | console_core::input::A),
        (12, console_core::input::RIGHT),
        (24, console_core::input::LEFT),
        (9, console_core::input::RIGHT | console_core::input::B),
        (60, 0),
    ];
    for (frames, mask) in script {
        first.step(frames, mask).unwrap();
        second.step(frames, mask).unwrap();
    }

    assert_eq!(
        first.console().unwrap().framebuffer(),
        second.console().unwrap().framebuffer()
    );
    let colors: std::collections::BTreeSet<u8> = first
        .console()
        .unwrap()
        .framebuffer()
        .iter()
        .copied()
        .collect();
    assert!(colors.iter().all(|&c| c < 64));
    assert!(
        colors.iter().filter(|&&c| c >= 16).count() >= 8,
        "showcase should exercise the expanded palette, got {colors:?}"
    );
    assert_eq!(
        serde_json::to_value(first.audio_stats(30).unwrap()).unwrap(),
        serde_json::to_value(second.audio_stats(30).unwrap()).unwrap()
    );
}

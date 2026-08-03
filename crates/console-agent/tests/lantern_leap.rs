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

#[test]
fn gameplay_uses_the_authored_night_scene_color_roles() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 0).unwrap();
    session.step(1, console_core::input::A).unwrap();
    session.step(119, 0).unwrap();

    let colors: std::collections::BTreeSet<u8> = session
        .console()
        .unwrap()
        .framebuffer()
        .iter()
        .copied()
        .collect();

    // These are semantic roles, not arbitrary high-index coverage: cool night
    // blues, moss greens, amber light, red-orange danger, violet atmosphere,
    // and a complete neutral stone/UI ramp all need to reach the real frame.
    let roles = [
        (1, "midnight blue"),
        (4, "teal blue"),
        (5, "sky blue"),
        (7, "wisp cyan"),
        (11, "deep moss"),
        (14, "moss highlight"),
        (31, "lantern amber"),
        (36, "hazard red"),
        (38, "hazard orange"),
        (41, "violet atmosphere"),
        (48, "near-black shadow"),
        (49, "deep backdrop"),
        (51, "mid backdrop"),
        (52, "stone shadow"),
        (55, "stone body"),
        (59, "stone highlight"),
        (63, "moon white"),
    ];
    for (index, role) in roles {
        assert!(
            colors.contains(&index),
            "missing {role} (Apollo64 index {index}); rendered {colors:?}"
        );
    }
}

#[test]
fn deluxe_scene_uses_authored_variants_and_ambient_animation_metadata() {
    let parsed = console_core::Cart::parse(&cart_text()).unwrap();
    assert_eq!(
        &parsed.preview_palette().indices()[..16],
        &[48, 41, 36, 38, 31, 14, 11, 4, 1, 2, 5, 7, 63, 59, 55, 52]
    );

    let mut session = Session::new();
    session.load_cart(&cart_text(), 0).unwrap();
    session.step(1, console_core::input::A).unwrap();

    let detail = request(
        &mut session,
        1,
        "eval",
        json!({"code": r#"
            local counts = {chip=0, rune=0, tuft=0, bloom=0, bubble=0}
            for cy=0,63 do
              for cx=0,17 do
                local tile=mget(cx,cy)
                if tile==75 then counts.chip=counts.chip+1
                elseif tile==76 then counts.rune=counts.rune+1
                elseif tile==77 then counts.tuft=counts.tuft+1
                elseif tile==78 then counts.bloom=counts.bloom+1
                elseif tile==79 then counts.bubble=counts.bubble+1 end
              end
            end
            return {counts=counts,
              flame=anim_len("flame.flicker"), moth=anim_len("moth.flap"),
              grass=anim_len("grass.sway"), spark=anim_len("spark.twinkle"),
              rise=anim_len("player.rise"), apex=anim_len("player.apex"),
              fall=anim_len("player.fall"), climb=anim_len("player.climb"),
              stomp=anim_len("player.stomp")}
        "#}),
    );
    let result = &detail["result"];
    assert_eq!(result["flame"], 4);
    assert_eq!(result["moth"], 4);
    assert_eq!(result["grass"], 4);
    assert_eq!(result["spark"], 4);
    assert_eq!(result["rise"], 1);
    assert_eq!(result["apex"], 1);
    assert_eq!(result["fall"], 1);
    assert_eq!(result["climb"], 4);
    assert_eq!(result["stomp"], 3);
    for role in ["chip", "rune", "tuft", "bloom", "bubble"] {
        assert!(
            result["counts"][role].as_u64().unwrap() > 0,
            "runtime map omitted the {role} variant: {result}"
        );
    }
}

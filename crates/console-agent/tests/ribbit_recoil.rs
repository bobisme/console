//! Regression coverage for the RIBBIT RECOIL showcase cart.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use console_agent::rpc::handle;
use console_agent::session::Session;
use serde_json::{Value, json};

fn cart_text() -> String {
    let path = format!(
        "{}/../../carts/ribbit-recoil.cart",
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

fn start_game(session: &mut Session) {
    session.step(1, 0).unwrap();
    session.step(1, console_core::input::A).unwrap();
    session.step(1, 0).unwrap();
}

#[test]
fn controller_only_replay_traverses_the_authored_level() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cart = root.join("carts/ribbit-recoil.cart");
    let scenario = root.join("carts/ribbit-recoil-traversal.playtest.json");

    let first = console_agent::playtest::run_scenario(&cart, &scenario, None, None)
        .expect("run controller-only traversal");
    let second = console_agent::playtest::run_scenario(&cart, &scenario, None, None)
        .expect("repeat controller-only traversal");

    assert_eq!(first.scenario.status, "passed");
    assert_eq!(first.scenario.frame_count, 1_442);
    assert_eq!(first.scenario.stage_count, 45);
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(second).unwrap(),
        "the no-warp controller replay report must be deterministic"
    );

    let secret_scenario = root.join("carts/ribbit-recoil-secret-traversal.playtest.json");
    let secret_first = console_agent::playtest::run_scenario(&cart, &secret_scenario, None, None)
        .expect("run controller-only secret route");
    let secret_second = console_agent::playtest::run_scenario(&cart, &secret_scenario, None, None)
        .expect("repeat controller-only secret route");
    assert_eq!(secret_first.scenario.status, "passed");
    assert_eq!(secret_first.scenario.frame_count, 631);
    assert_eq!(secret_first.scenario.stage_count, 22);
    assert_eq!(
        serde_json::to_value(secret_first).unwrap(),
        serde_json::to_value(secret_second).unwrap(),
        "the controller-only branch replay must be deterministic"
    );
}

#[test]
fn tongue_latches_reels_and_releases_with_momentum() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 8_675_309).unwrap();
    start_game(&mut session);

    session
        .step(14, console_core::input::UP | console_core::input::A)
        .unwrap();
    let latched_response = request(
        &mut session,
        1,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    let latched = &latched_response["result"];
    assert_eq!(
        latched["latched"], true,
        "the first overhead girder should be reachable"
    );
    assert_eq!(latched["tongue"], "latched");

    session
        .step(46, console_core::input::UP | console_core::input::A)
        .unwrap();
    session
        .step(20, console_core::input::RIGHT | console_core::input::A)
        .unwrap();
    session.step(1, 0).unwrap();
    session.step(18, console_core::input::RIGHT).unwrap();

    let released_response = request(
        &mut session,
        2,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    let released = &released_response["result"];
    assert_eq!(released["tongue"], "idle");
    assert!(
        released["x"].as_f64().unwrap() > 75.0,
        "swing release should carry the frog right: {released}"
    );
}

#[test]
fn tongue_latches_every_solid_material_and_retracts_from_destroyed_terrain() {
    let cases = [
        ("steel", 192),
        ("girder", 193),
        ("mud", 194),
        ("breakable", 196),
    ];

    for (name, tile) in cases {
        let mut session = Session::new();
        session.load_cart(&cart_text(), 8_675_309).unwrap();
        start_game(&mut session);
        request(
            &mut session,
            1,
            "eval",
            json!({"code": format!(
                "dev_warp(88,400); for x=10,13 do mset(x,48,{tile}) end"
            )}),
        );
        let input = console_core::input::UP | console_core::input::A;
        session.step(18, input).unwrap();
        let latched = request(
            &mut session,
            2,
            "eval",
            json!({"code": "return dev_status().latched"}),
        );
        assert_eq!(latched["result"], true, "tongue should latch to {name}");

        if name == "breakable" {
            request(
                &mut session,
                3,
                "eval",
                json!({"code": "for x=10,13 do mset(x,48,0) end"}),
            );
            session.step(1, input).unwrap();
            let detached = request(
                &mut session,
                4,
                "eval",
                json!({"code": "return dev_status().latched"}),
            );
            assert_eq!(
                detached["result"], false,
                "destroying an anchor cell must safely release the tongue"
            );
        }
    }
}

#[test]
fn both_mutations_kill_insects_and_can_be_swapped() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 8_675_309).unwrap();
    start_game(&mut session);

    let laser_index = request(
        &mut session,
        1,
        "eval",
        json!({"code": "dev_grant('laser'); dev_warp(300,328); return dev_spawn_enemy('gnat',365,334)"}),
    )["result"]
        .as_u64()
        .unwrap();
    session.step(1, console_core::input::B).unwrap();
    let laser_hp = request(
        &mut session,
        2,
        "eval",
        json!({"code": format!("return dev_enemy_hp({laser_index})")}),
    );
    assert!(laser_hp["result"].as_i64().unwrap() <= 0);

    session.step(4, 0).unwrap();
    let fire_index = request(
        &mut session,
        3,
        "eval",
        json!({"code": "dev_grant('all'); dev_warp(604,408); return dev_spawn_enemy('beetle',650,408)"}),
    )["result"]
        .as_u64()
        .unwrap();
    session.step(24, console_core::input::B).unwrap();
    let fire_hp = request(
        &mut session,
        4,
        "eval",
        json!({"code": format!("return dev_enemy_hp({fire_index})")}),
    );
    assert!(fire_hp["result"].as_i64().unwrap() <= 0);

    session.step(6, 0).unwrap();
    session
        .step(1, console_core::input::DOWN | console_core::input::B)
        .unwrap();
    let cycled = request(
        &mut session,
        5,
        "eval",
        json!({"code": "return dev_status().mutation"}),
    );
    assert_eq!(cycled["result"], "LASER EYES");

    let demolished = request(
        &mut session,
        6,
        "eval",
        json!({"code": "dev_explode(368,416,4); return mget(44,54)"}),
    );
    assert_eq!(demolished["result"], 0, "egg bombs should breach red walls");
}

#[test]
fn boss_explosion_chain_opens_evac_and_finishes_level() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 8_675_309).unwrap();
    start_game(&mut session);

    request(
        &mut session,
        1,
        "eval",
        json!({"code": "dev_warp(790,392); dev_start_boss()"}),
    );
    session.step(30, 0).unwrap();
    let phase_one = request(
        &mut session,
        2,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(phase_one["result"]["boss_phase"], 1);
    assert_eq!(phase_one["result"]["boss_hp"], 8);
    assert!(phase_one["result"]["bullets"].as_u64().unwrap() >= 3);

    request(
        &mut session,
        3,
        "eval",
        json!({"code": "dev_damage_boss(3)"}),
    );
    session.step(1, 0).unwrap();
    let phase_two = request(
        &mut session,
        4,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(phase_two["result"]["boss_phase"], 2);
    assert_eq!(phase_two["result"]["boss_attack"], 1);
    assert_eq!(phase_two["result"]["boss_vulnerable"], false);
    assert_eq!(phase_two["result"]["boss_salvo2"], 0);
    session.step(29, console_core::input::LEFT).unwrap();
    let phase_two_pattern = request(
        &mut session,
        5,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(phase_two_pattern["result"]["boss_vulnerable"], true);
    assert_eq!(phase_two_pattern["result"]["boss_salvo2"], 1);

    request(
        &mut session,
        6,
        "eval",
        json!({"code": "dev_damage_boss(3)"}),
    );
    session.step(1, 0).unwrap();
    let phase_three = request(
        &mut session,
        7,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(phase_three["result"]["boss_phase"], 3);
    assert_eq!(phase_three["result"]["boss_attack"], 1);
    assert_eq!(phase_three["result"]["boss_vulnerable"], false);
    assert_eq!(phase_three["result"]["boss_salvo3"], 0);
    session.step(29, console_core::input::RIGHT).unwrap();
    let phase_three_pattern = request(
        &mut session,
        8,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(phase_three_pattern["result"]["boss_vulnerable"], true);
    assert_eq!(phase_three_pattern["result"]["boss_salvo3"], 1);

    let hp_before = request(
        &mut session,
        9,
        "eval",
        json!({"code": "return dev_status().hp"}),
    )["result"]
        .as_i64()
        .unwrap();
    request(
        &mut session,
        10,
        "eval",
        json!({"code": "dev_damage_boss(2)"}),
    );
    let defeated_response = request(
        &mut session,
        11,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    let defeated = &defeated_response["result"];
    assert_eq!(defeated["boss_defeated"], true);
    assert_eq!(defeated["boss_hp"], 0);
    assert_eq!(
        defeated["bullets"], 0,
        "defeat must clear hostile projectiles"
    );

    session.step(180, 0).unwrap();
    let safe = request(
        &mut session,
        12,
        "eval",
        json!({"code": "return {open=dev_status().exit_open,hp=dev_status().hp}"}),
    );
    assert_eq!(
        safe["result"]["hp"], hp_before,
        "180 idle post-defeat frames must be safe"
    );
    session.step(80, 0).unwrap();
    let exit = request(
        &mut session,
        13,
        "eval",
        json!({"code": "return dev_status().exit_open"}),
    );
    assert_eq!(exit["result"], true);

    request(
        &mut session,
        14,
        "eval",
        json!({"code": "dev_warp(918,430)"}),
    );
    session.step(1, 0).unwrap();
    let scene = request(
        &mut session,
        15,
        "eval",
        json!({"code": "return dev_status().scene"}),
    );
    assert_eq!(scene["result"], "win");
}

#[test]
fn authored_level_and_animation_contracts_are_present() {
    let cart = cart_text();
    let parsed = console_core::Cart::parse(&cart).unwrap();
    assert_eq!(parsed.title(), "RIBBIT RECOIL");
    assert_eq!(
        &parsed.preview_palette().indices()[..16],
        &[48, 48, 41, 36, 38, 31, 14, 11, 4, 2, 7, 63, 59, 55, 52, 45]
    );

    let mut session = Session::new();
    session.load_cart(&cart, 8_675_309).unwrap();
    start_game(&mut session);
    let authored = request(
        &mut session,
        1,
        "eval",
        json!({"code": r#"
            return {
              floor=mget(0,61), girder=mget(8,55), mud=mget(28,43),
              acid=mget(28,62), breakable=mget(44,54), bridge=mget(58,59),
              frog_run=anim_len('frog.run'), frog_swing=anim_len('frog.swing'),
              gnat=anim_len('gnat.fly'), beetle=anim_len('beetle.walk'),
              wasp=anim_len('wasp.fly'), boss=anim_len('buzzkill.rage')
            }
        "#}),
    );
    let result = &authored["result"];
    assert_eq!(result["floor"], 192);
    assert_eq!(result["girder"], 193);
    assert_eq!(result["mud"], 194);
    assert_eq!(result["acid"], 195);
    assert_eq!(result["breakable"], 196);
    assert_eq!(result["bridge"], 193);
    assert_eq!(result["frog_run"], 4);
    assert_eq!(result["frog_swing"], 4);
    assert_eq!(result["gnat"], 3);
    assert_eq!(result["beetle"], 3);
    assert_eq!(result["wasp"], 2);
    assert_eq!(result["boss"], 3);

    let colors: BTreeSet<u8> = session
        .console()
        .unwrap()
        .framebuffer()
        .iter()
        .copied()
        .collect();
    for (index, role) in [
        (2, "twilight blue"),
        (7, "laser cyan"),
        (14, "frog highlight"),
        (31, "hazard yellow"),
        (41, "mutation violet"),
        (48, "ink shadow"),
        (55, "steel"),
        (63, "white highlight"),
    ] {
        assert!(
            colors.contains(&index),
            "missing {role} (palette {index}) in {colors:?}"
        );
    }
}

#[test]
fn polish_contracts_cover_surface_grapple_branch_stats_and_separate_songs() {
    let cart = cart_text();
    assert!(
        !cart.contains("hooks={"),
        "floating grapple targets must stay removed"
    );
    assert!(
        !cart.contains("draw_hook"),
        "floating hook rendering must stay removed"
    );
    assert!(
        !cart.contains("for y=max(HUD_H,acid_y)"),
        "the full-width lower-screen wave distortion must stay removed"
    );
    for contract in [
        "pat 3 loop=0 : 35 36 37 38",
        "pat 11 loop=8 : 43 44 45 46",
        "inst siren",
        "inst warbrass",
        "secrets={{430,302,1500}}",
    ] {
        assert!(
            cart.contains(contract),
            "missing polish contract: {contract}"
        );
    }

    let mut session = Session::new();
    session.load_cart(&cart, 8_675_309).unwrap();
    start_game(&mut session);
    request(
        &mut session,
        1,
        "eval",
        json!({"code": "dev_warp(424,294)"}),
    );
    session.step(1, 0).unwrap();
    let stats = request(
        &mut session,
        2,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(stats["result"]["secrets"], 1);
    assert!(stats["result"]["score"].as_i64().unwrap() >= 1_500);
    assert!(stats["result"]["elapsed"].as_i64().unwrap() > 0);
    assert!(stats["result"]["rank"].as_str().is_some());

    let mut pacifist = Session::new();
    pacifist.load_cart(&cart, 8_675_309).unwrap();
    start_game(&mut pacifist);
    let pacifist_rank = request(
        &mut pacifist,
        3,
        "eval",
        json!({"code": "return dev_status().rank"}),
    );
    assert_eq!(
        pacifist_rank["result"], "B",
        "an enemy-free speed run must not receive an A rank"
    );
}

#[test]
fn scripted_run_is_framebuffer_and_audio_deterministic() {
    let cart = cart_text();
    let mut first = Session::new();
    let mut second = Session::new();
    first.load_cart(&cart, 424_242).unwrap();
    second.load_cart(&cart, 424_242).unwrap();

    let script = [
        (1, 0),
        (1, console_core::input::A),
        (1, 0),
        (14, console_core::input::UP | console_core::input::A),
        (46, console_core::input::UP | console_core::input::A),
        (20, console_core::input::RIGHT | console_core::input::A),
        (1, 0),
        (18, console_core::input::RIGHT),
        (90, 0),
    ];
    for (frames, mask) in script {
        first.step(frames, mask).unwrap();
        second.step(frames, mask).unwrap();
    }

    assert_eq!(
        first.console().unwrap().framebuffer(),
        second.console().unwrap().framebuffer()
    );
    assert_eq!(
        serde_json::to_value(first.audio_stats(6).unwrap()).unwrap(),
        serde_json::to_value(second.audio_stats(6).unwrap()).unwrap()
    );
    assert!(
        first
            .audio_stats(6)
            .unwrap()
            .iter()
            .all(|window| window.clipped == 0),
        "showcase audio should preserve headroom"
    );
}

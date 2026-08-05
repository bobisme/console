//! Regression coverage for the RIBBIT RECOIL showcase cart.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn atlas_source(file: &str) -> String {
    let path = format!(
        "{}/../../carts/ribbit-recoil-art/{file}",
        env!("CARGO_MANIFEST_DIR"),
    );
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn decode_palette_char(ch: char) -> u8 {
    const ALPHABET: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_";
    ALPHABET
        .find(ch)
        .unwrap_or_else(|| panic!("invalid atlas palette character {ch:?}")) as u8
}

fn atlas_sections(file: &str) -> BTreeMap<String, Vec<Vec<u8>>> {
    let mut sections = BTreeMap::<String, Vec<Vec<u8>>>::new();
    let mut current = None::<String>;
    for line in atlas_source(file).lines() {
        if let Some(name) = line.strip_prefix('@') {
            current = Some(name.to_owned());
            sections.entry(name.to_owned()).or_default();
        } else if !line.is_empty() && !line.starts_with('#') {
            let name = current
                .as_ref()
                .expect("atlas pixels must follow a @section");
            sections
                .get_mut(name)
                .unwrap()
                .push(line.chars().map(decode_palette_char).collect());
        }
    }
    sections
}

fn frog_atlas_sections() -> BTreeMap<String, Vec<Vec<u8>>> {
    atlas_sections("frog-atlas.pixels")
}

fn enemy_atlas_sections() -> BTreeMap<String, Vec<Vec<u8>>> {
    atlas_sections("enemy-atlas.pixels")
}

fn environment_atlas_sections() -> BTreeMap<String, Vec<Vec<u8>>> {
    atlas_sections("environment-atlas.pixels")
}

fn isolated_same_role_pixels(frame: &[Vec<u8>]) -> usize {
    let height = frame.len();
    let width = frame.first().map_or(0, Vec::len);
    let mut isolated = 0;
    for y in 0..height {
        for x in 0..width {
            let color = frame[y][x];
            if color == 0 {
                continue;
            }
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(height - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(width - 1);
            let has_same_neighbor = (y0..=y1)
                .any(|ny| (x0..=x1).any(|nx| (nx != x || ny != y) && frame[ny][nx] == color));
            isolated += usize::from(!has_same_neighbor);
        }
    }
    isolated
}

fn opaque_edge(frame: &[Vec<u8>], edge: &str) -> bool {
    match edge {
        "top" => frame.first().unwrap().iter().any(|&c| c != 0),
        "bottom" => frame.last().unwrap().iter().any(|&c| c != 0),
        "left" => frame.iter().any(|row| row.first() != Some(&0)),
        "right" => frame.iter().any(|row| row.last() != Some(&0)),
        _ => panic!("unknown edge {edge}"),
    }
}

fn composed_opaque_bounds(
    sections: &BTreeMap<String, Vec<Vec<u8>>>,
    placements: &[(&str, i32, i32)],
) -> (i32, i32, i32, i32) {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for (name, center_x, center_y) in placements {
        let frame = sections
            .get(*name)
            .unwrap_or_else(|| panic!("missing atlas section @{name}"));
        let anchor_x = frame[0].len() as i32 / 2;
        let anchor_y = frame.len() as i32 / 2;
        for (y, row) in frame.iter().enumerate() {
            for (x, color) in row.iter().enumerate() {
                if *color == 0 {
                    continue;
                }
                let px = center_x + x as i32 - anchor_x;
                let py = center_y + y as i32 - anchor_y;
                min_x = min_x.min(px);
                min_y = min_y.min(py);
                max_x = max_x.max(px + 1);
                max_y = max_y.max(py + 1);
            }
        }
    }
    (min_x, min_y, max_x, max_y)
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

fn environment_frame_at(cart: &str, camera_x: i32) -> (Vec<u8>, usize, u64) {
    let mut session = Session::new();
    session.set_draw_tracing(true);
    session.load_cart(cart, 8_675_309).unwrap();
    start_game(&mut session);
    session.clear_draw_events();
    request(
        &mut session,
        1,
        "eval",
        json!({"code": format!("dev_stage_environment_at({camera_x})")}),
    );
    session.step(1, 0).unwrap();
    let trace = session.draw_events(None, None).unwrap();
    (
        session.console().unwrap().framebuffer().to_vec(),
        trace.events.len(),
        trace.dropped,
    )
}

fn focal_trace_at(session: &mut Session, camera_x: i32, name: &str) -> usize {
    session.clear_draw_events();
    session
        .eval(&format!(
            "dev_stage_environment_cull_at({camera_x}); dev_draw_environment_focal('{name}')"
        ))
        .unwrap();
    let trace = session.draw_events(None, None).unwrap();
    trace.events.len()
}

#[test]
fn environment_scrolls_across_every_former_camera_boundary_without_a_scene_swap() {
    let cart = cart_text();
    for boundary in [36, 164, 292, 420, 548, 676] {
        let (before, before_events, before_dropped) = environment_frame_at(&cart, boundary - 1);
        let (after, after_events, after_dropped) = environment_frame_at(&cart, boundary + 1);
        let changed = before
            .iter()
            .zip(&after)
            .filter(|(left, right)| left != right)
            .count();

        assert!(
            changed > 0,
            "camera motion at x={boundary} must move the city"
        );
        assert!(
            changed * 100 < before.len() * 35,
            "camera crossing x={boundary} changed {changed}/{} pixels; this looks like a full-scene swap",
            before.len()
        );
        assert_eq!(
            before_dropped, 0,
            "draw trace overflowed before x={boundary} after {before_events} retained events"
        );
        assert_eq!(
            after_dropped, 0,
            "draw trace overflowed after x={boundary} after {after_events} retained events"
        );
    }
}

#[test]
fn environment_focal_props_enter_and_exit_on_their_true_pixel_extents() {
    let cart = cart_text();
    let mut session = Session::new();
    session.set_draw_tracing(true);
    session.load_cart(&cart, 8_675_309).unwrap();
    start_game(&mut session);

    for (name, left, right) in [
        ("lick_lab", 73, 130),
        ("loading_pipes", 153, 185),
        ("waterworks_machine", 267, 328),
        ("molt_sign", 317, 392),
        ("gene_pipes", 416, 448),
        ("gene_bar", 465, 546),
        ("croak_machine", 531, 592),
        ("water_tower", 601, 646),
        ("croak_sign", 654, 729),
        ("hr_hive", 786, 851),
    ] {
        for (camera_x, should_draw, edge) in [
            (left - console_core::SCREEN_W as i32, false, "before entry"),
            (left - console_core::SCREEN_W as i32 + 1, true, "at entry"),
            (right - 1, true, "before exit"),
            (right, false, "at exit"),
        ] {
            let events = focal_trace_at(&mut session, camera_x, name);
            assert_eq!(
                events > 0,
                should_draw,
                "{name} trace was wrong {edge} at camera x={camera_x}"
            );
        }
    }
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
    assert_eq!(first.scenario.frame_count, 1_487);
    assert_eq!(first.scenario.stage_count, 50);
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
    assert_eq!(secret_first.scenario.frame_count, 615);
    assert_eq!(secret_first.scenario.stage_count, 25);
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
        .step(14, console_core::input::UP | console_core::input::B)
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
        .step(46, console_core::input::UP | console_core::input::B)
        .unwrap();
    session
        .step(20, console_core::input::RIGHT | console_core::input::B)
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
        let input = console_core::input::UP | console_core::input::B;
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
fn hop_has_momentum_idle_ground_is_stable_and_runoff_is_immediately_fatal() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 8_675_309).unwrap();
    start_game(&mut session);
    session.step(180, 0).unwrap();
    let idle = request(
        &mut session,
        1,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(idle["result"]["grounded"], true);
    assert_eq!(idle["result"]["particles"], 0);
    assert_eq!(idle["result"]["movement_fx"], 0);
    assert_eq!(idle["result"]["landings"], 0);

    session
        .step(9, console_core::input::RIGHT | console_core::input::A)
        .unwrap();
    let hop = request(
        &mut session,
        2,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(hop["result"]["hops"], 1);
    assert!(hop["result"]["y"].as_f64().unwrap() < 450.0, "{hop}");
    assert!(hop["result"]["vx"].as_f64().unwrap() > 1.8, "{hop}");
    assert!(hop["result"]["vy"].as_f64().unwrap() < -3.0, "{hop}");

    let mut fatal = Session::new();
    fatal.load_cart(&cart_text(), 8_675_309).unwrap();
    start_game(&mut fatal);
    fatal.step(5, 0).unwrap();
    let staged = request(
        &mut fatal,
        3,
        "eval",
        json!({"code": "dev_warp(230,490); return dev_status()"}),
    );
    fatal.step(1, 0).unwrap();
    let drowned = request(
        &mut fatal,
        4,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(drowned["result"]["hp"], 0);
    assert_eq!(drowned["result"]["deaths"], 1);
    assert!(drowned["result"]["respawn"].as_u64().unwrap() > 0);
    assert_eq!(
        drowned["result"]["camera_x"], staged["result"]["camera_x"],
        "fatal runoff must freeze camera tracking"
    );
    assert_eq!(
        drowned["result"]["camera_y"], staged["result"]["camera_y"],
        "fatal runoff must freeze camera tracking"
    );
    // Fatal impact contributes hit-stop frames before the respawn countdown.
    fatal.step(70, 0).unwrap();
    let respawned = request(
        &mut fatal,
        5,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(respawned["result"]["hp"], 5);
    assert_eq!(respawned["result"]["respawn"], 0);
    assert!(respawned["result"]["x"].as_f64().unwrap() < 50.0);

    let mut post_boss = Session::new();
    post_boss.load_cart(&cart_text(), 8_675_309).unwrap();
    start_game(&mut post_boss);
    request(
        &mut post_boss,
        6,
        "eval",
        json!({"code": "dev_start_boss(); dev_damage_boss(99); dev_warp(230,490)"}),
    );
    post_boss.step(1, 0).unwrap();
    let post_boss_drowned = request(
        &mut post_boss,
        7,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(post_boss_drowned["result"]["boss_defeated"], true);
    assert_eq!(post_boss_drowned["result"]["hp"], 0);
    assert_eq!(post_boss_drowned["result"]["deaths"], 1);
    assert!(post_boss_drowned["result"]["respawn"].as_u64().unwrap() > 0);
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
    session
        .step(
            1,
            console_core::input::DOWN | console_core::input::RIGHT | console_core::input::A,
        )
        .unwrap();
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
    session
        .step(24, console_core::input::DOWN | console_core::input::A)
        .unwrap();
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
fn boss_camera_keeps_retreats_visible_and_the_drawn_rig_is_interactive() {
    let mut camera = Session::new();
    camera.load_cart(&cart_text(), 8_675_309).unwrap();
    start_game(&mut camera);
    request(
        &mut camera,
        1,
        "eval",
        json!({"code": "dev_start_boss(); dev_warp(650,432)"}),
    );
    camera.step(60, 0).unwrap();
    let framed = request(
        &mut camera,
        2,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    let player_screen_x =
        framed["result"]["x"].as_f64().unwrap() - framed["result"]["camera_x"].as_f64().unwrap();
    assert!(
        (15.0..=177.0).contains(&player_screen_x),
        "boss camera must keep a retreating or respawned frog visible: {framed}"
    );

    let mut rig = Session::new();
    rig.load_cart(&cart_text(), 8_675_309).unwrap();
    start_game(&mut rig);
    request(
        &mut rig,
        3,
        "eval",
        json!({"code": "dev_warp(760,425); dev_start_boss(); dev_boss_vulnerable()"}),
    );
    rig.step(12, console_core::input::RIGHT | console_core::input::B)
        .unwrap();
    let hit = request(&mut rig, 4, "eval", json!({"code": "return dev_status()"}));
    assert!(
        hit["result"]["boss_hp"].as_i64().unwrap() < 8,
        "the tongue must hit the visibly drawn upper-left command rig: {hit}"
    );
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
              frog_rise=anim_len('frog.rise'), frog_fall=anim_len('frog.fall'),
              frog_laser=anim_len('frog.laser'), frog_fire=anim_len('frog.fire'),
              gnat_fly=anim_len('gnat.fly'), gnat_attack=anim_len('gnat.attack'),
              beetle_walk=anim_len('beetle.walk'), beetle_attack=anim_len('beetle.attack'),
              wasp_fly=anim_len('wasp.fly'), wasp_attack=anim_len('wasp.attack'),
              boss_upper=anim_len('buzz_upper_pod.idle'),
              boss_lower=anim_len('buzz_lower_pod.idle'),
              boss_weak=anim_len('buzz_weak_open.idle'),
              boss_claw=anim_len('buzz_claw.idle'),
              boss_cannon=anim_len('buzz_cannon.idle')
            }
        "#}),
    );
    let result = &authored["result"];
    assert_eq!(result["floor"], 205);
    assert_eq!(result["girder"], 209);
    assert_eq!(result["mud"], 211);
    assert_eq!(result["acid"], 222);
    assert_eq!(result["breakable"], 196);
    assert_eq!(result["bridge"], 209);
    assert_eq!(result["frog_run"], 2);
    assert_eq!(result["frog_swing"], 4);
    assert_eq!(result["frog_rise"], 1);
    assert_eq!(result["frog_fall"], 1);
    assert_eq!(result["frog_laser"], 1);
    assert_eq!(result["frog_fire"], 1);
    assert_eq!(result["gnat_fly"], 2);
    assert_eq!(result["gnat_attack"], 1);
    assert_eq!(result["beetle_walk"], 2);
    assert_eq!(result["beetle_attack"], 1);
    assert_eq!(result["wasp_fly"], 2);
    assert_eq!(result["wasp_attack"], 1);
    assert_eq!(result["boss_upper"], 1);
    assert_eq!(result["boss_lower"], 1);
    assert_eq!(result["boss_weak"], 1);
    assert_eq!(result["boss_claw"], 1);
    assert_eq!(result["boss_cannon"], 1);

    // Let the deployment mosaic clear before asserting the authored frame's
    // sparse highlight clusters are present in the framebuffer.
    session.step(12, 0).unwrap();
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
fn frog_atlas_matches_source_and_obeys_semantic_pixel_contracts() {
    let cart = console_core::Cart::parse(&cart_text()).unwrap();
    let sections = frog_atlas_sections();
    let hero_inks = BTreeSet::from([8, 12, 14, 31, 38, 48, 63]);
    let full_frames = [
        ("idle", 0, 0),
        ("run_a", 3, 0),
        ("run_b", 6, 0),
        ("rise", 9, 0),
        ("fall", 12, 0),
        ("swing_tuck", 0, 3),
        ("swing_extend", 3, 3),
        ("laser_brace", 6, 3),
        ("fire_breath", 9, 3),
        ("hurt", 12, 3),
    ];
    let mut run_palettes = Vec::new();
    for (name, tx, ty) in full_frames {
        let frame = sections
            .get(name)
            .unwrap_or_else(|| panic!("missing atlas section @{name}"));
        assert_eq!(frame.len(), 24, "@{name} height");
        assert!(frame.iter().all(|row| row.len() == 24), "@{name} width");
        let colors: BTreeSet<u8> = frame
            .iter()
            .flatten()
            .copied()
            .filter(|&c| c != 0)
            .collect();
        assert!(
            (4..=7).contains(&colors.len()),
            "@{name} uses {} nontransparent indices: {colors:?}",
            colors.len()
        );
        assert!(
            colors.is_subset(&hero_inks),
            "@{name} escapes the semantic frog ramp: {colors:?}"
        );
        assert!(
            isolated_same_role_pixels(frame) <= 2,
            "@{name} exceeds the two-pixel isolated-accent budget"
        );
        if name == "run_a" || name == "run_b" {
            run_palettes.push(colors);
        }
        for (y, row) in frame.iter().enumerate() {
            for (x, expected) in row.iter().enumerate() {
                let sheet_index = (ty * 8 + y) * console_core::SHEET_W + tx * 8 + x;
                assert_eq!(
                    cart.sprites()[sheet_index],
                    *expected,
                    "@{name} differs from the allocated cart cell at ({x},{y})"
                );
            }
        }
    }
    assert_eq!(
        run_palettes[0], run_palettes[1],
        "the run loop must not flicker between material ramps"
    );

    let overlays: [(&str, usize, usize, &[u8]); 6] = [
        ("blink", 15, 0, &[8, 14, 48]),
        ("victory", 15, 1, &[38, 48, 63]),
        ("laser_eye", 15, 2, &[6, 7, 48, 63]),
        ("fire_throat", 15, 3, &[31, 38, 39, 48, 63]),
        ("laser_pickup", 15, 4, &[7, 48, 63]),
        ("fire_pickup", 15, 5, &[31, 38, 39, 48, 63]),
    ];
    for (name, tx, ty, allowed) in overlays {
        let frame = sections
            .get(name)
            .unwrap_or_else(|| panic!("missing atlas section @{name}"));
        assert_eq!(frame.len(), 8, "@{name} height");
        assert!(frame.iter().all(|row| row.len() == 8), "@{name} width");
        let allowed = BTreeSet::from_iter(allowed.iter().copied());
        let colors: BTreeSet<u8> = frame
            .iter()
            .flatten()
            .copied()
            .filter(|&c| c != 0)
            .collect();
        assert!(
            colors.is_subset(&allowed),
            "@{name} escapes its semantic overlay ramp: {colors:?}"
        );
        for (y, row) in frame.iter().enumerate() {
            for (x, expected) in row.iter().enumerate() {
                let sheet_index = (ty * 8 + y) * console_core::SHEET_W + tx * 8 + x;
                assert_eq!(
                    cart.sprites()[sheet_index],
                    *expected,
                    "@{name} differs from the allocated cart cell at ({x},{y})"
                );
            }
        }
    }
}

#[test]
fn boss_phase_three_bounds_follow_the_lowered_damaged_chassis() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 8_675_309).unwrap();
    start_game(&mut session);

    let p1_weapon = request(
        &mut session,
        1,
        "eval",
        json!({"code": "dev_stage_boss_art(1,'closed'); return dev_boss_bounds(false)"}),
    );
    let p1_contact = request(
        &mut session,
        2,
        "eval",
        json!({"code": "return dev_boss_bounds(true)"}),
    );
    let p1_weak = request(
        &mut session,
        5,
        "eval",
        json!({"code": "return dev_boss_weak_point()"}),
    );
    assert_eq!(p1_weapon["result"]["w"], 92);
    assert_eq!(p1_weapon["result"]["h"], 58);
    assert_eq!(p1_contact["result"]["w"], 50);
    assert_eq!(p1_contact["result"]["h"], 52);

    let p3_weapon = request(
        &mut session,
        3,
        "eval",
        json!({"code": "dev_stage_boss_art(3,'attack'); return dev_boss_bounds(false)"}),
    );
    let p3_contact = request(
        &mut session,
        4,
        "eval",
        json!({"code": "return dev_boss_bounds(true)"}),
    );
    let p3_weak = request(
        &mut session,
        6,
        "eval",
        json!({"code": "return dev_boss_weak_point()"}),
    );
    assert_eq!(p3_weapon["result"]["x"], p1_weapon["result"]["x"]);
    assert_eq!(p3_weapon["result"]["y"], p1_weapon["result"]["y"]);
    assert_eq!(p3_weapon["result"]["w"], 92);
    assert_eq!(p3_weapon["result"]["h"], 64);
    assert_eq!(p3_contact["result"]["x"], p1_contact["result"]["x"]);
    assert_eq!(
        p3_contact["result"]["y"].as_i64().unwrap(),
        p1_contact["result"]["y"].as_i64().unwrap() + 6
    );
    assert_eq!(p3_contact["result"]["w"], 50);
    assert_eq!(p3_contact["result"]["h"], 52);
    assert_eq!(p3_weak["result"]["x"], p1_weak["result"]["x"]);
    assert_eq!(
        p1_weak["result"]["y"].as_i64().unwrap(),
        p1_weapon["result"]["y"].as_i64().unwrap() + 29,
        "phase-one fallback aim must hit the visible shutter"
    );
    assert_eq!(
        p3_weak["result"]["y"].as_i64().unwrap(),
        p3_weapon["result"]["y"].as_i64().unwrap() + 35,
        "phase-three fallback aim must follow the shutter's six-pixel slump"
    );
}

#[test]
fn enemy_atlas_matches_source_and_obeys_semantic_pixel_contracts() {
    let cart = console_core::Cart::parse(&cart_text()).unwrap();
    let sections = enemy_atlas_sections();
    let insect_inks = BTreeSet::from([6, 8, 12, 14, 31, 36, 38, 48]);
    let boss_inks = BTreeSet::from([5, 12, 28, 31, 38, 48, 52, 54, 58, 61]);
    let allocations = [
        ("gnat_a", 0, 6, 16, 16, true),
        ("gnat_b", 2, 6, 16, 16, true),
        ("gnat_attack", 4, 6, 16, 16, true),
        ("wasp_a", 6, 6, 16, 16, true),
        ("wasp_b", 8, 6, 16, 16, true),
        ("wasp_attack", 10, 6, 16, 16, true),
        ("beetle_a", 12, 6, 16, 16, true),
        ("beetle_b", 14, 6, 16, 16, true),
        ("beetle_attack", 0, 8, 16, 16, true),
        ("upper_wing_l", 2, 8, 16, 16, false),
        ("upper_wing_r", 4, 8, 16, 16, false),
        ("lower_wing_l", 6, 8, 16, 16, false),
        ("lower_wing_r", 8, 8, 16, 16, false),
        ("side_armor_l", 10, 8, 16, 16, false),
        ("side_armor_r", 12, 8, 16, 16, false),
        ("weak_closed", 14, 8, 16, 16, false),
        ("weak_open", 0, 10, 16, 16, false),
        ("claw", 2, 10, 16, 16, false),
        ("cannon", 4, 10, 16, 16, false),
        ("upper_pod", 6, 10, 24, 24, false),
        ("lower_pod", 9, 10, 24, 24, false),
    ];

    assert_eq!(sections.len(), allocations.len());
    let mut family_palettes = BTreeMap::<&str, BTreeSet<u8>>::new();
    for (name, tx, ty, width, height, is_insect) in allocations {
        let frame = sections
            .get(name)
            .unwrap_or_else(|| panic!("missing atlas section @{name}"));
        assert_eq!(frame.len(), height, "@{name} height");
        assert!(frame.iter().all(|row| row.len() == width), "@{name} width");
        let colors: BTreeSet<u8> = frame
            .iter()
            .flatten()
            .copied()
            .filter(|&c| c != 0)
            .collect();
        assert!(
            isolated_same_role_pixels(frame) <= 2,
            "@{name} exceeds the two-pixel isolated-accent budget"
        );
        if is_insect {
            assert!(
                (3..=8).contains(&colors.len()),
                "@{name} uses {} nontransparent indices: {colors:?}",
                colors.len()
            );
            assert!(
                colors.is_subset(&insect_inks),
                "@{name} escapes the common-insect ramp: {colors:?}"
            );
            if !name.ends_with("_attack") {
                let family = name.split('_').next().unwrap();
                if let Some(expected) = family_palettes.get(family) {
                    assert_eq!(
                        &colors, expected,
                        "the {family} locomotion loop must not flicker between material ramps"
                    );
                } else {
                    family_palettes.insert(family, colors.clone());
                }
            }
        } else {
            assert!(
                colors.is_subset(&boss_inks),
                "@{name} escapes the authored boss material ramp: {colors:?}"
            );
        }

        for (y, row) in frame.iter().enumerate() {
            for (x, expected) in row.iter().enumerate() {
                let sheet_index = (ty * 8 + y) * console_core::SHEET_W + tx * 8 + x;
                assert_eq!(
                    cart.sprites()[sheet_index],
                    *expected,
                    "@{name} differs from the allocated cart cell at ({x},{y})"
                );
            }
        }
    }

    for (name, edges) in [
        ("upper_wing_l", &["top", "left"][..]),
        ("upper_wing_r", &["top", "right"][..]),
        ("side_armor_l", &["left", "bottom"][..]),
        ("side_armor_r", &["right", "bottom"][..]),
        ("claw", &["left", "bottom"][..]),
        ("cannon", &["right", "bottom"][..]),
        ("upper_pod", &["top"][..]),
    ] {
        let frame = sections.get(name).unwrap();
        for edge in edges {
            assert!(
                opaque_edge(frame, edge),
                "@{name} must visibly reach its {edge} envelope edge"
            );
        }
    }

    let phase_one = [
        ("upper_wing_l", -38, -44),
        ("upper_wing_r", 38, -44),
        ("lower_wing_l", -36, -24),
        ("lower_wing_r", 36, -24),
        ("upper_pod", 0, -38),
        ("lower_pod", 0, -14),
        ("side_armor_l", -17, -6),
        ("side_armor_r", 17, -6),
        ("weak_closed", 0, -23),
        ("claw", -38, -2),
        ("cannon", 38, -2),
    ];
    assert_eq!(
        composed_opaque_bounds(&sections, &phase_one),
        (-46, -52, 46, 6),
        "phase-one authored pixels must exactly fill the 92x58 weapon envelope"
    );
    let phase_one_contact = [
        ("upper_pod", 0, -38),
        ("lower_pod", 0, -14),
        ("side_armor_l", -17, -6),
        ("side_armor_r", 17, -6),
        ("weak_closed", 0, -23),
    ];
    assert_eq!(
        composed_opaque_bounds(&sections, &phase_one_contact),
        (-25, -50, 25, 2),
        "phase-one pod pixels must exactly fill the 50x52 contact envelope"
    );
    let phase_three = [
        ("upper_wing_r", 38, -44),
        ("lower_wing_r", 34, -30),
        ("upper_pod", 2, -32),
        ("lower_pod", 0, -8),
        ("side_armor_l", -17, 0),
        ("side_armor_r", 17, 0),
        ("weak_open", 0, -17),
        ("claw", -38, 4),
    ];
    assert_eq!(
        composed_opaque_bounds(&sections, &phase_three),
        (-46, -52, 46, 12),
        "phase-three authored pixels must exactly fill the dynamic 92x64 weapon envelope"
    );
    let phase_three_contact = [
        ("upper_pod", 2, -32),
        ("lower_pod", 0, -8),
        ("side_armor_l", -17, 0),
        ("side_armor_r", 17, 0),
        ("weak_open", 0, -17),
    ];
    assert_eq!(
        composed_opaque_bounds(&sections, &phase_three_contact),
        (-25, -44, 25, 8),
        "phase-three pod pixels must exactly fill its lowered 50x52 contact envelope"
    );
}

#[test]
fn environment_atlas_matches_source_and_allocated_sheet_cells() {
    let cart = console_core::Cart::parse(&cart_text()).unwrap();
    let sections = environment_atlas_sections();
    let allocations = [
        ("steel_cap", 0, 12, 8, 8),
        ("girder", 1, 12, 8, 8),
        ("slime_cap", 2, 12, 8, 8),
        ("acid", 3, 12, 8, 8),
        ("breakable", 4, 12, 8, 8),
        ("steel_seam", 12, 12, 8, 8),
        ("steel_left", 13, 12, 8, 8),
        ("steel_right", 14, 12, 8, 8),
        ("steel_damaged", 15, 12, 8, 8),
        ("brace", 0, 13, 8, 8),
        ("junction", 1, 13, 8, 8),
        ("cavity", 2, 13, 8, 8),
        ("masonry_top", 3, 13, 8, 8),
        ("masonry_face", 4, 13, 8, 8),
        ("masonry_corner", 5, 13, 8, 8),
        ("pipe_h", 6, 13, 8, 8),
        ("pipe_v", 7, 13, 8, 8),
        ("pipe_elbow", 8, 13, 8, 8),
        ("pipe_junction", 9, 13, 8, 8),
        ("vent_grille", 10, 13, 8, 8),
        ("fence_post", 11, 13, 8, 8),
        ("fence_wire", 12, 13, 8, 8),
        ("fence_damaged", 13, 13, 8, 8),
        ("acid_lip", 14, 13, 8, 8),
        ("prop_lamp", 0, 14, 16, 16),
        ("prop_coil", 2, 14, 16, 16),
        ("prop_crate", 4, 14, 16, 16),
        ("prop_sign", 6, 14, 8, 8),
        ("prop_vent", 7, 14, 8, 8),
        ("prop_antenna", 6, 15, 8, 8),
        ("prop_cable", 7, 15, 8, 8),
        ("rust_cap", 8, 15, 8, 8),
        ("rust_face", 9, 15, 8, 8),
        ("concrete_cap", 10, 15, 8, 8),
        ("concrete_face", 11, 15, 8, 8),
        ("lab_cap", 12, 15, 8, 8),
        ("lab_face", 13, 15, 8, 8),
        ("pipeworks_cap", 14, 15, 8, 8),
        ("pipeworks_face", 15, 15, 8, 8),
    ];

    assert_eq!(sections.len(), allocations.len());
    for (name, tx, ty, width, height) in allocations {
        let frame = sections
            .get(name)
            .unwrap_or_else(|| panic!("missing environment atlas section @{name}"));
        assert_eq!(frame.len(), height, "@{name} height");
        assert!(frame.iter().all(|row| row.len() == width), "@{name} width");
        let colors: BTreeSet<u8> = frame
            .iter()
            .flatten()
            .copied()
            .filter(|&c| c != 0)
            .collect();
        assert!(
            (1..=8).contains(&colors.len()),
            "@{name} uses {} nontransparent indices: {colors:?}",
            colors.len()
        );
        let isolated_budget = if width == 8 { 8 } else { 40 };
        assert!(
            isolated_same_role_pixels(frame) <= isolated_budget,
            "@{name} exceeds its {isolated_budget}-pixel isolated-accent budget"
        );
        for (y, row) in frame.iter().enumerate() {
            for (x, expected) in row.iter().enumerate() {
                let sheet_index = (ty * 8 + y) * console_core::SHEET_W + tx * 8 + x;
                assert_eq!(
                    cart.sprites()[sheet_index],
                    *expected,
                    "@{name} differs from the allocated cart cell at ({x},{y})"
                );
            }
        }
    }
}

#[test]
fn frog_atlas_builder_is_a_byte_exact_rebuild() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "ribbit-recoil-atlas-{}-{nonce}.cart",
        std::process::id()
    ));
    let original = cart_text();
    fs::write(&scratch, &original).unwrap();
    let output = Command::new("bash")
        .arg(root.join("carts/ribbit-recoil-art/build-frog-atlas.sh"))
        .arg(&scratch)
        .env("CONSOLE_BIN", env!("CARGO_BIN_EXE_console"))
        .output()
        .expect("run frog atlas builder");
    let rebuilt = fs::read_to_string(&scratch).expect("read rebuilt scratch cart");
    fs::remove_file(&scratch).expect("remove scratch cart");
    assert!(
        output.status.success(),
        "atlas builder failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(rebuilt, original, "atlas builder must be byte-exact");
}

#[test]
fn enemy_atlas_builder_is_a_byte_exact_rebuild() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "ribbit-recoil-enemy-atlas-{}-{nonce}.cart",
        std::process::id()
    ));
    let original = cart_text();
    fs::write(&scratch, &original).unwrap();
    let output = Command::new("bash")
        .arg(root.join("carts/ribbit-recoil-art/build-enemy-atlas.sh"))
        .arg(&scratch)
        .env("CONSOLE_BIN", env!("CARGO_BIN_EXE_console"))
        .output()
        .expect("run enemy atlas builder");
    let rebuilt = fs::read_to_string(&scratch).expect("read rebuilt scratch cart");
    fs::remove_file(&scratch).expect("remove scratch cart");
    assert!(
        output.status.success(),
        "enemy atlas builder failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(rebuilt, original, "enemy atlas builder must be byte-exact");
}

#[test]
fn environment_atlas_builder_is_a_byte_exact_rebuild() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "ribbit-recoil-environment-atlas-{}-{nonce}.cart",
        std::process::id()
    ));
    let original = cart_text();
    fs::write(&scratch, &original).unwrap();
    let output = Command::new("bash")
        .arg(root.join("carts/ribbit-recoil-art/build-environment-atlas.sh"))
        .arg(&scratch)
        .env("CONSOLE_BIN", env!("CARGO_BIN_EXE_console"))
        .output()
        .expect("run environment atlas builder");
    let rebuilt = fs::read_to_string(&scratch).expect("read rebuilt scratch cart");
    fs::remove_file(&scratch).expect("remove scratch cart");
    assert!(
        output.status.success(),
        "environment atlas builder failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        rebuilt, original,
        "environment atlas builder must be byte-exact"
    );
}

#[test]
fn frog_palette_scope_covers_all_render_modes_and_restores_legacy_ink() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 8_675_309).unwrap();
    let cases = [
        (
            "normal",
            "return dev_render_frog_probe('idle',1,false,'NONE')",
            "8,12,14,31,48,63",
        ),
        (
            "invulnerable",
            "return dev_render_frog_probe('idle',1,true,'NONE')",
            "14,63",
        ),
        (
            "persistent laser mutation",
            "return dev_render_frog_probe('idle',1,false,'LASER EYES')",
            "6,7,8,12,14,31,48,63",
        ),
        (
            "persistent fire mutation",
            "return dev_render_frog_probe('idle',1,false,'FIRE BREATH')",
            "8,12,14,31,38,39,48,63",
        ),
        (
            "laser recoil",
            "return dev_render_frog_probe('recoil',1,false,'LASER EYES')",
            "8,12,31,48,63",
        ),
        (
            "fire breath",
            "return dev_render_frog_probe('mutate',1,false,'FIRE BREATH')",
            "8,12,14,31,48,63",
        ),
        (
            "title scale",
            "return dev_render_frog_probe('idle',2,false,'NONE')",
            "8,12,14,31,48,63",
        ),
        (
            "victory scale",
            "return dev_render_frog_probe('victory',2,false,'NONE')",
            "8,12,14,31,38,48,63",
        ),
    ];
    for (index, (name, code, expected_colors)) in cases.into_iter().enumerate() {
        let response = request(
            &mut session,
            100 + index as u32,
            "eval",
            json!({"code": code}),
        );
        let probe = &response["result"];
        assert_eq!(probe["colors"], expected_colors, "wrong {name} palette");
        assert_eq!(
            probe["sentinel"], 14,
            "{name} failed to restore legacy ink 6 -> Apollo index 14"
        );
        assert_eq!(
            session.console().unwrap().draw_state().draw_palette()[6],
            14,
            "{name} left the draw palette in the wrong state"
        );
    }
}

#[test]
fn golden_fly_uses_authored_enemy_colors_and_restores_legacy_ink() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 8_675_309).unwrap();
    let response = request(
        &mut session,
        1,
        "eval",
        json!({"code": "return dev_render_secret_probe()"}),
    );
    let probe = &response["result"];
    assert_eq!(probe["colors"], "6,8,12,31,41,48,63");
    assert_eq!(
        probe["sentinel"], 14,
        "Golden Fly failed to restore legacy ink 6 -> Apollo index 14"
    );
    assert_eq!(
        session.console().unwrap().draw_state().draw_palette()[6],
        14,
        "Golden Fly left the draw palette in the authored identity scope"
    );
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
        "pat 5 loop=0 : 51 52 53 54",
        "pat 13 loop=8 : 59 60 61 62",
        "sfx 63 speed=1",
        "inst siren",
        "inst warbrass",
        "secrets={{354,402,1500}}",
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
        json!({"code": "for y=54,57 do for x=44,46 do mset(x,y,0) end end; dev_warp(388,424)"}),
    );
    session
        .step(
            40,
            console_core::input::UP | console_core::input::LEFT | console_core::input::B,
        )
        .unwrap();
    let stats = request(
        &mut session,
        2,
        "eval",
        json!({"code": "return dev_status()"}),
    );
    assert_eq!(
        stats["result"]["secrets"], 1,
        "the isolated latched-tongue secret route must collect the fly: {stats}"
    );
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
        (14, console_core::input::UP | console_core::input::B),
        (46, console_core::input::UP | console_core::input::B),
        (20, console_core::input::RIGHT | console_core::input::B),
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

//! End-to-end proof that the ECS prototype can drive an entity-heavy game.

use std::collections::BTreeMap;
use std::path::Path;

use console_agent::rpc::handle;
use console_agent::session::Session;
use console_agent::value::lua_to_json;
use serde_json::{Value, json};

fn cart_text() -> String {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/radiant-swarm");
    console_agent::project::compile_project(&project)
        .unwrap_or_else(|error| panic!("compiling {}: {error}", project.display()))
        .cart_text
}

fn eval_json(session: &mut Session, code: &str) -> Value {
    let value = session
        .eval(code)
        .unwrap_or_else(|error| panic!("evaluating {code:?}: {error}"));
    lua_to_json(&value)
}

fn stress(session: &mut Session) -> Value {
    eval_json(session, "dev_stress()");
    session.step(180, console_core::input::A).unwrap();
    eval_json(session, "return dev_status()")
}

#[test]
fn dense_swarm_is_deterministic_and_inspectable() {
    let cart = cart_text();
    let mut first = Session::new();
    let mut second = Session::new();
    first.load_cart(&cart, 11).unwrap();
    second.load_cart(&cart, 11).unwrap();

    let first_status = stress(&mut first);
    let second_status = stress(&mut second);
    assert_eq!(first_status, second_status);
    assert!(first_status["peak_alive"].as_u64().unwrap() >= 250);
    assert!(first_status["peak_bullets"].as_u64().unwrap() >= 120);
    assert_eq!(first_status["dropped_spawns"], 0);
    assert_eq!(
        first.console().unwrap().framebuffer(),
        second.console().unwrap().framebuffer()
    );

    let select = BTreeMap::from([
        ("hostile".to_string(), vec!["kind".to_string()]),
        ("pos".to_string(), vec!["x".to_string(), "y".to_string()]),
    ]);
    let required = vec!["hostile".to_string(), "pos".to_string()];
    let inspected = first
        .ecs_query("arena", &required, &select, 12, 0)
        .map(|value| lua_to_json(&value))
        .unwrap();
    assert_eq!(inspected["returned"], 12);
    assert!(inspected["matched"].as_u64().unwrap() >= 120);
    assert_eq!(inspected["truncated"], true);
    let entities = inspected["entities"].as_array().unwrap();
    assert!(
        entities
            .windows(2)
            .all(|pair| { pair[0]["id"].as_u64().unwrap() < pair[1]["id"].as_u64().unwrap() })
    );
    assert!(entities.iter().all(|entity| {
        entity["components"]["pos"].get("x").is_some()
            && entity["components"]["pos"].get("y").is_some()
            && entity["components"]["pos"].get("vx").is_none()
    }));

    first.step(1, console_core::input::B).unwrap();
    assert_eq!(eval_json(&mut first, "return dev_status().bombs"), 2);
    assert!(
        eval_json(&mut first, "return dev_status().enemy_bullets")
            .as_u64()
            .unwrap()
            < first_status["enemy_bullets"].as_u64().unwrap()
    );
}

#[test]
fn boss_can_spawn_and_end_the_run_in_victory() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 23).unwrap();
    eval_json(&mut session, "dev_stress()");
    session.step(540, 0).unwrap();

    let boss = eval_json(&mut session, "return dev_status()");
    assert_eq!(boss["phase"], "play");
    assert!(boss["boss_hp"].as_u64().unwrap() > 0, "{boss}");
    assert_eq!(boss["dropped_spawns"], 0);

    eval_json(&mut session, "dev_damage_boss(9999)");
    let won = eval_json(&mut session, "return dev_status()");
    assert_eq!(won["phase"], "victory");
    assert_eq!(won["boss_hp"], 0);
    assert_eq!(won["enemy_bullets"], 0);
}

#[test]
fn ecs_query_rpc_projects_the_showcase_without_cart_cooperation() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 11).unwrap();
    stress(&mut session);
    eval_json(&mut session, "ecs = nil");

    let response = handle(
        &mut session,
        json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"ecs_query",
            "params":{
                "world":"arena",
                "with":["enemy", "pos"],
                "select":{"enemy":["kind", "boss"], "pos":["x", "y"]},
                "limit":4
            }
        }),
    );
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["returned"], 4);
    assert_eq!(response["result"]["frame_count"], 180);
}

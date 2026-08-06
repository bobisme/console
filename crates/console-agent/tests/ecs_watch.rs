use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::rpc::handle;
use console_agent::session::Session;
use serde_json::{Value, json};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("console-ecs-watch-{}-{serial}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, value: &str) -> PathBuf {
        self.0.join(value)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rpc(session: &mut Session, id: u64, method: &str, params: Value) -> Value {
    handle(
        session,
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .args(args)
        .output()
        .expect("run console")
}

fn path(value: &Path) -> &str {
    value.to_str().expect("test path is UTF-8")
}

const CART: &str = r#"__meta__
title=Watch Fixture
__lua__
world=ecs.world("game",{capacity=32})
tick=0
function _init()
  world:spawn({mob={kind="seed"},pos={x=1,y=2,private=99}})
  world:spawn({mob={kind="seed"},pos={x=2,y=4,private=99}})
end
function _update()
  tick=tick+1
  world:spawn({mob={kind="spawn"},pos={x=tick+2,y=tick*3,private=99}})
  if tick==2 then world:despawn(1) end
end
"#;

fn load(session: &mut Session) {
    let response = rpc(session, 1, "load_cart", json!({"text":CART,"seed":7}));
    assert!(response.get("error").is_none(), "{response}");
}

fn define(session: &mut Session, limit: u64, delta_limit: u64) -> Value {
    rpc(
        session,
        2,
        "ecs_watch_define",
        json!({
            "name":"mobs",
            "world":"game",
            "with":["mob", "pos"],
            "select":{"mob":["kind"],"pos":["x","y"]},
            "limit":limit,
            "entity_delta_limit":delta_limit
        }),
    )
}

#[test]
fn named_watch_samples_selected_frames_and_reports_exact_deltas() {
    let mut session = Session::new();
    load(&mut session);
    let defined = define(&mut session, 8, 1);
    assert_eq!(defined["result"]["definition"]["name"], "mobs");
    assert_eq!(defined["result"]["budgets"]["definitions"], 32);

    let baseline = rpc(&mut session, 3, "ecs_watch_sample", json!({"name":"mobs"}));
    assert_eq!(baseline["result"]["frame_count"], 0);
    assert_eq!(baseline["result"]["matched"], 2);
    assert_eq!(baseline["result"]["delta"]["comparable"], false);
    assert_eq!(
        baseline["result"]["entities"][0]["components"]["pos"],
        json!({"x":1,"y":2})
    );

    let stepped = rpc(
        &mut session,
        4,
        "step",
        json!({"frames":1,"watches":["mobs"]}),
    );
    let first = &stepped["result"]["watches"][0];
    assert_eq!(first["frame_count"], 1);
    assert_eq!(first["sample_index"], 2);
    assert_eq!(first["matched"], 3);
    assert_eq!(first["delta"]["frames"], 1);
    assert_eq!(first["delta"]["alive"], 1);
    assert_eq!(first["delta"]["matched"], 1);
    assert_eq!(first["delta"]["returned"], 1);
    assert_eq!(first["delta"]["component_counts"]["mob"], 1);
    assert_eq!(first["delta"]["spawned"], json!([3]));
    assert_eq!(first["delta"]["despawned"], json!([]));
    assert_eq!(first["delta"]["entity_membership_complete"], true);
    assert_eq!(first["delta"]["truncated"], false);

    let second = rpc(
        &mut session,
        5,
        "step",
        json!({"frames":1,"watches":["mobs"]}),
    );
    let second = &second["result"]["watches"][0];
    assert_eq!(second["matched"], 3);
    assert_eq!(second["delta"]["spawned"], json!([4]));
    assert_eq!(second["delta"]["despawned"], json!([1]));
    assert_eq!(second["delta"]["alive"], 0);
    assert_eq!(second["delta"]["matched"], 0);

    let listed = rpc(&mut session, 6, "ecs_watch_list", json!({}));
    assert_eq!(listed["result"]["watches"][0]["samples"], 3);
    assert_eq!(listed["result"]["watches"][0]["last_frame_count"], 2);
    let removed = rpc(&mut session, 7, "ecs_watch_remove", json!({"name":"mobs"}));
    assert_eq!(removed["result"]["removed"], true);
}

#[test]
fn truncation_and_rewind_boundaries_are_explicit() {
    let mut session = Session::new();
    load(&mut session);
    let defined = define(&mut session, 2, 0);
    assert!(defined.get("error").is_none(), "{defined}");
    rpc(&mut session, 3, "ecs_watch_sample", json!({"name":"mobs"}));
    let sample = rpc(
        &mut session,
        4,
        "step",
        json!({"frames":1,"watches":["mobs"]}),
    );
    let sample = &sample["result"]["watches"][0];
    assert_eq!(sample["matched"], 3);
    assert_eq!(sample["returned"], 2);
    assert_eq!(sample["truncated"], true);
    assert_eq!(sample["delta"]["entity_membership_complete"], false);
    assert_eq!(sample["delta"]["truncated"], true);

    rpc(&mut session, 5, "save_state", json!({"name":"one"}));
    rpc(
        &mut session,
        6,
        "step",
        json!({"frames":1,"watches":["mobs"]}),
    );
    let loaded = rpc(&mut session, 7, "load_state", json!({"name":"one"}));
    assert!(loaded.get("error").is_none(), "{loaded}");
    let after_load = rpc(&mut session, 8, "ecs_watch_sample", json!({"name":"mobs"}));
    assert_eq!(after_load["result"]["sample_index"], 1);
    assert_eq!(after_load["result"]["delta"]["comparable"], false);
    assert_eq!(after_load["result"]["frame_count"], sample["frame_count"]);
    assert_eq!(after_load["result"]["matched"], sample["matched"]);
    assert_eq!(after_load["result"]["entities"], sample["entities"]);

    rpc(&mut session, 9, "reset", json!({}));
    let list = rpc(&mut session, 10, "ecs_watch_list", json!({}));
    assert_eq!(list["result"]["watches"][0]["samples"], 0);
    assert!(list["result"]["watches"][0]["last_frame_count"].is_null());

    rpc(
        &mut session,
        11,
        "load_cart",
        json!({"text":"__lua__\nworld=ecs.world('other')"}),
    );
    let cleared = rpc(&mut session, 12, "ecs_watch_list", json!({}));
    assert_eq!(cleared["result"]["watches"], json!([]));

    let mut capped = Session::new();
    load(&mut capped);
    define(&mut capped, 8, 0);
    rpc(&mut capped, 13, "ecs_watch_sample", json!({"name":"mobs"}));
    let capped = rpc(
        &mut capped,
        14,
        "step",
        json!({"frames":1,"watches":["mobs"]}),
    );
    let delta = &capped["result"]["watches"][0]["delta"];
    assert_eq!(delta["entity_membership_complete"], true);
    assert_eq!(delta["spawned"], json!([]));
    assert_eq!(delta["spawned_truncated"], true);
    assert_eq!(delta["truncated"], true);
}

#[test]
fn watch_rpc_reuses_query_bounds_and_protected_inspector() {
    let mut session = Session::new();
    load(&mut session);
    for (params, needle) in [
        (json!({"name":"mobs","world":"game","limit":129}), "limit"),
        (json!({"name":"bad watch","world":"game"}), "begin with"),
        (json!({"name":"mobs","world":"game","after":1}), "after"),
        (
            json!({"name":"mobs","world":"game","entity_delta_limit":129}),
            "entity_delta_limit",
        ),
    ] {
        let response = rpc(&mut session, 2, "ecs_watch_define", params);
        assert_eq!(response["error"]["code"], -32602, "{response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains(needle),
            "{response}"
        );
    }

    let defined = define(&mut session, 8, 8);
    assert!(defined.get("error").is_none(), "{defined}");
    rpc(&mut session, 3, "eval", json!({"code":"ecs=nil"}));
    let protected = rpc(&mut session, 4, "ecs_watch_sample", json!({"name":"mobs"}));
    assert_eq!(protected["result"]["matched"], 2);

    let before = session.console().unwrap().frame_count();
    let invalid_step = rpc(
        &mut session,
        5,
        "step",
        json!({"frames":10,"watches":["missing"]}),
    );
    assert_eq!(invalid_step["error"]["code"], -32602);
    assert_eq!(session.console().unwrap().frame_count(), before);

    for index in 0..31 {
        let response = rpc(
            &mut session,
            10 + index,
            "ecs_watch_define",
            json!({"name":format!("w{index}"),"world":"game"}),
        );
        assert!(response.get("error").is_none(), "{response}");
    }
    let over_limit = rpc(
        &mut session,
        50,
        "ecs_watch_define",
        json!({"name":"overflow","world":"game"}),
    );
    assert_eq!(over_limit["error"]["code"], -32602);
    assert!(
        over_limit["error"]["message"]
            .as_str()
            .unwrap()
            .contains("limit 32")
    );
}

#[test]
fn run_ecs_watch_emits_before_final_eval_and_rejects_duplicates() {
    let root = TempDir::new();
    let cart = root.join("watch.cart");
    std::fs::write(&cart, CART).unwrap();
    let definition = r#"{"name":"mobs","world":"game","with":["mob"],"limit":8}"#;
    let output = run(&[
        "run",
        path(&cart),
        "--frames",
        "1",
        "--ecs-watch",
        definition,
        "--eval-after",
        "return {kind='eval',frame=t()*60}",
    ]);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = String::from_utf8(output.stdout).unwrap();
    let lines = lines.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "one watch report and one eval report");
    let watch: Value = serde_json::from_str(lines[0]).unwrap();
    let eval: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(watch["watch"], "mobs");
    assert_eq!(watch["frame_count"], 1);
    assert_eq!(watch["matched"], 3);
    assert_eq!(eval, json!({"frame":1.0,"kind":"eval"}));

    let duplicate = run(&[
        "run",
        path(&cart),
        "--ecs-watch",
        definition,
        "--ecs-watch",
        definition,
    ]);
    assert_eq!(duplicate.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("only be specified once"));
}

#[test]
fn radiant_watch_playtest_observes_wave_bullet_growth_and_nova_clear() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = root.join("examples/radiant-swarm");
    let scenario = project.join("watch-playtest.json");
    let report = console_agent::playtest::run_scenario(&project, &scenario, None, None)
        .expect("run Radiant watch scenario");
    let report = serde_json::to_value(report).unwrap();
    assert_eq!(report["scenario"]["status"], "passed");

    let wave = &report["stages"][2]["actual"];
    assert!(wave["matched"].as_u64().unwrap() >= 5, "{wave}");
    assert!(wave["delta"]["matched"].as_i64().unwrap() >= 5, "{wave}");
    assert!(!wave["delta"]["spawned"].as_array().unwrap().is_empty());

    let storm = &report["stages"][5]["actual"];
    assert!(storm["matched"].as_u64().unwrap() >= 120, "{storm}");
    assert!(
        storm["delta"]["matched"].as_i64().unwrap() >= 120,
        "{storm}"
    );

    let nova = &report["stages"][7]["actual"];
    assert!(
        nova["matched"].as_u64().unwrap() < storm["matched"].as_u64().unwrap(),
        "storm={storm} nova={nova}"
    );
    assert!(nova["delta"]["matched"].as_i64().unwrap() < 0, "{nova}");
    assert!(!nova["delta"]["despawned"].as_array().unwrap().is_empty());
}

use console_agent::rpc::handle;
use console_agent::session::Session;
use serde_json::json;

const CART: &str = r#"__lua__
function _update()
  local frame = flr(t()*60)
  if frame == 0 then
    score_update(10)
    score_submit()
    score_submit()
  elseif frame == 1 then
    score_update(20)
    score_submit()
    leaderboard_show()
  end
end
"#;

#[test]
fn session_records_platform_events_and_keeps_host_max_outside_rewinds() {
    let mut session = Session::new();
    session.load_cart(CART, 0).unwrap();
    session.step(1, 0).unwrap();
    session.save_state("after-ten").unwrap();
    session.step(1, 0).unwrap();

    let report = session.platform_events(None).unwrap();
    assert_eq!(report.max_submitted_score, Some(20));
    assert_eq!(report.dropped, 0);
    assert_eq!(report.events.len(), 5);
    assert_eq!(report.events[0].frame, 1);
    assert_eq!(report.events[0].index, 0);
    assert_eq!(report.events[2].frame, 2);
    assert_eq!(report.events[4].index, 2);

    session.load_state("after-ten").unwrap();
    let replayed = session.platform_events(None).unwrap();
    assert_eq!(
        replayed.events.len(),
        2,
        "event log is rebuilt to the rewind"
    );
    assert_eq!(
        replayed.max_submitted_score,
        Some(20),
        "host-owned best is not cart save-state data and must not rewind"
    );

    session.reset(None).unwrap();
    let reset = session.platform_events(None).unwrap();
    assert!(reset.events.is_empty());
    assert_eq!(reset.max_submitted_score, Some(20));

    session.load_cart("__lua__\nscore_submit()\n", 0).unwrap();
    assert_eq!(
        session.platform_events(None).unwrap().max_submitted_score,
        Some(0),
        "loading a different cart starts a different local adapter scope"
    );
}

#[test]
fn rpc_exposes_bounded_events_and_host_max_without_lua_readback() {
    let mut session = Session::new();
    let loaded = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"text":CART}}),
    );
    assert!(loaded.get("error").is_none(), "{loaded}");
    handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":2,"method":"step","params":{"frames":2}}),
    );

    let events = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":3,"method":"platform_events","params":{"from_frame":2}}),
    );
    assert_eq!(events["result"]["capacity"], 65_536);
    assert_eq!(events["result"]["dropped"], 0);
    assert_eq!(events["result"]["max_submitted_score"], 20);
    assert_eq!(
        events["result"]["events"],
        json!([
            {"frame":2,"index":0,"kind":"score_update","score":20},
            {"frame":2,"index":1,"kind":"score_submit","score":20},
            {"frame":2,"index":2,"kind":"leaderboard_show"}
        ])
    );

    let info = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":4,"method":"info","params":{}}),
    );
    assert_eq!(info["result"]["max_submitted_score"], 20);
    let gameplay = handle(
        &mut session,
        json!({"jsonrpc":"2.0","id":5,"method":"eval","params":{"code":"return score_best"}}),
    );
    assert_eq!(gameplay["result"]["result"], serde_json::Value::Null);
}

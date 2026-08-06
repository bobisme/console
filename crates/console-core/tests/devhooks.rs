use std::collections::BTreeMap;

use console_core::{Console, DevHookPhase, DevValue};

const CART: &str = r#"__lua__
counter = 0
devhook.register("status", {
  description = "Return the counter",
  phase = "post_frame",
  run = function(_) return { counter = counter, nested = {true, "ok"} } end,
})
function _init()
  devhook.register("add", {
    description = "Add before frame one",
    phase = "pre_frame",
    run = function(args) counter = counter + args.amount return counter end,
  })
end
"#;

#[test]
fn lists_in_registration_order_and_invokes_bounded_values() {
    let mut console = Console::new(CART, 0).unwrap();
    let hooks = console.dev_hooks().unwrap();
    assert_eq!(
        hooks
            .iter()
            .map(|hook| hook.name.as_str())
            .collect::<Vec<_>>(),
        ["status", "add"]
    );
    assert_eq!(hooks[0].phase, DevHookPhase::PostFrame);
    assert_eq!(hooks[1].phase, DevHookPhase::PreFrame);

    let args = DevValue::Object(BTreeMap::from([(
        "amount".to_string(),
        DevValue::Integer(4),
    )]));
    assert_eq!(
        console
            .invoke_dev_hook("add", DevHookPhase::PreFrame, &args)
            .unwrap(),
        DevValue::Integer(4)
    );
    let DevValue::Object(status) = console
        .invoke_dev_hook("status", DevHookPhase::PostFrame, &DevValue::Null)
        .unwrap()
    else {
        panic!("status must return an object")
    };
    assert_eq!(status["counter"], DevValue::Integer(4));
}

#[test]
fn phase_mismatch_is_rejected_before_lua_runs() {
    let mut console = Console::new(CART, 0).unwrap();
    let args = DevValue::Object(BTreeMap::from([(
        "amount".to_string(),
        DevValue::Integer(9),
    )]));
    let error = console
        .invoke_dev_hook("add", DevHookPhase::PostFrame, &args)
        .unwrap_err();
    assert!(error.message().contains("has phase pre_frame"));
    assert_eq!(
        console
            .invoke_dev_hook("add", DevHookPhase::PreFrame, &args)
            .unwrap(),
        DevValue::Integer(9),
        "the rejected invocation must not have entered the callback"
    );
}

#[test]
fn host_closures_survive_global_replacement_and_registration_locks() {
    let mut console = Console::new(CART, 0).unwrap();
    console.eval("devhook = nil").unwrap();
    assert_eq!(console.dev_hooks().unwrap().len(), 2);
    let status = console
        .invoke_dev_hook("status", DevHookPhase::PostFrame, &DevValue::Null)
        .unwrap();
    assert!(matches!(status, DevValue::Object(_)));

    let late = Console::new(
        r#"__lua__
function _update()
  devhook.register("late", {description="late", phase="post_frame", run=function() end})
end
"#,
        0,
    )
    .and_then(|mut console| console.step(0).map(|_| console))
    .unwrap_err();
    assert!(
        late.message()
            .contains("registration is closed after _init")
    );
}

#[test]
fn invalid_metadata_and_unsupported_results_fail_closed() {
    for source in [
        r#"__lua__
devhook.register("same", {description="one", phase="post_frame", run=function() end})
devhook.register("same", {description="two", phase="post_frame", run=function() end})
"#,
        r#"__lua__
devhook.register("bad name", {description="bad", phase="post_frame", run=function() end})
"#,
        r#"__lua__
devhook.register("bad", {description="bad", phase="sometimes", run=function() end})
"#,
    ] {
        assert!(
            Console::new(source, 0).is_err(),
            "source should fail: {source}"
        );
    }

    let mut console = Console::new(
        r#"__lua__
devhook.register("mixed", {
  description="Return a table that JSON cannot represent",
  phase="post_frame",
  run=function() return {[1]="array", named="object"} end,
})
"#,
        0,
    )
    .unwrap();
    let error = console
        .invoke_dev_hook("mixed", DevHookPhase::PostFrame, &DevValue::Null)
        .unwrap_err();
    assert!(error.message().contains("dense arrays or objects"));
    assert!(
        console.is_halted(),
        "invalid results halt after the callback may have mutated state"
    );
}

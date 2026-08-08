use console_core::{Cart, Console, MAX_SAVE_BYTES};

fn cart(lua: &str) -> String {
    format!(
        "__meta__\ntitle=Save Test\nsave_id=org.example.save-test\nsave_version=2\n\n__lua__\n{lua}\n"
    )
}

#[test]
fn cart_requires_paired_valid_save_metadata() {
    let missing_version = "__meta__\nsave_id=game\n__lua__\n";
    assert!(
        Cart::parse(missing_version)
            .unwrap_err()
            .to_string()
            .contains("save_version")
    );
    let missing_id = "__meta__\nsave_version=1\n__lua__\n";
    assert!(
        Cart::parse(missing_id)
            .unwrap_err()
            .to_string()
            .contains("save_id")
    );
    let invalid_id = "__meta__\nsave_id=bad id\nsave_version=1\n__lua__\n";
    assert!(Cart::parse(invalid_id).is_err());
    let zero_version = "__meta__\nsave_id=game\nsave_version=0\n__lua__\n";
    assert!(Cart::parse(zero_version).is_err());

    let parsed = Cart::parse(&cart("")).unwrap();
    let config = parsed.save_config().unwrap();
    assert_eq!(config.id(), "org.example.save-test");
    assert_eq!(config.version(), 2);
}

#[test]
fn initial_save_is_available_before_init_and_reports_stored_schema() {
    let source = cart(
        r#"
function _init()
  loaded, loaded_version = save_load()
end
"#,
    );
    let initial = r#"{"data":{"unlocks":["dash",true]},"id":"org.example.save-test","version":1}"#;
    let mut console = Console::new_with_save(&source, 7, Some(initial)).unwrap();
    assert_eq!(
        console.get_global("loaded_version").unwrap().as_i64(),
        Some(1)
    );
    assert_eq!(
        console
            .eval("return loaded.unlocks[1]")
            .unwrap()
            .as_string()
            .unwrap()
            .to_str()
            .unwrap(),
        "dash"
    );
    assert_eq!(console.save_revision(), 0);
    assert!(console.save_diagnostic().is_none());
    let stored = console
        .eval("return save_store({migrated=loaded.unlocks[1]})")
        .unwrap();
    assert_eq!(stored.as_boolean(), Some(true));
    assert_eq!(
        console.save_document().unwrap(),
        r#"{"data":{"migrated":"dash"},"id":"org.example.save-test","version":2}"#
    );
}

#[test]
fn writes_are_canonical_bounded_and_atomic() {
    let source = cart(
        r#"
function _init()
  first_ok, first_error = save_store({z=1,a={true,"x"}})
  local cycle={}; cycle.self=cycle
  cycle_ok, cycle_error = save_store(cycle)
  huge_ok, huge_error = save_store({text=string.rep("x", 8200)})
end
"#,
    );
    let console = Console::new(&source, 0).unwrap();
    assert_eq!(
        console.get_global("first_ok").unwrap().as_boolean(),
        Some(true)
    );
    assert_eq!(
        console.get_global("cycle_ok").unwrap().as_boolean(),
        Some(false)
    );
    assert!(
        console
            .get_global("cycle_error")
            .unwrap()
            .as_string()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("cycles")
    );
    assert_eq!(
        console.get_global("huge_ok").unwrap().as_boolean(),
        Some(false)
    );
    let document = console.save_document().unwrap();
    assert_eq!(
        document,
        r#"{"data":{"a":[true,"x"],"z":1},"id":"org.example.save-test","version":2}"#
    );
    assert!(document.len() <= MAX_SAVE_BYTES);
    assert_eq!(
        console.save_revision(),
        1,
        "failed stores retain the prior commit"
    );
    assert!(console.save_diagnostic().unwrap().contains("maximum"));
}

#[test]
fn malformed_or_mismatched_initial_data_is_empty_and_diagnostic() {
    let source = cart("function _init() loaded=save_load() end");
    for initial in ["not json", r#"{"data":[],"id":"another.game","version":1}"#] {
        let console = Console::new_with_save(&source, 0, Some(initial)).unwrap();
        assert!(console.get_global("loaded").unwrap().is_nil());
        assert!(console.save_document().is_none());
        assert!(console.save_diagnostic().is_some());
    }
}

#[test]
fn same_initial_save_seed_and_input_are_deterministic() {
    let source = cart(
        r#"
local saved=save_load() or {x=0}
x=saved.x
function _update() x=x+1 end
function _draw() cls(x) end
"#,
    );
    let initial = r#"{"data":{"x":17},"id":"org.example.save-test","version":2}"#;
    let mut first = Console::new_with_save(&source, 44, Some(initial)).unwrap();
    let mut second = Console::new_with_save(&source, 44, Some(initial)).unwrap();
    for input in [0, 2, 2, 0, 16, 0] {
        first.step(input).unwrap();
        second.step(input).unwrap();
    }
    assert_eq!(first.framebuffer(), second.framebuffer());
    assert_eq!(first.audio_frame(), second.audio_frame());
    assert_eq!(first.save_document(), second.save_document());
}

#[test]
fn clear_is_an_explicit_dirty_commit() {
    let source = cart("function _init() ok,err=save_clear() end");
    let initial = r#"{"data":{"x":1},"id":"org.example.save-test","version":2}"#;
    let console = Console::new_with_save(&source, 0, Some(initial)).unwrap();
    assert_eq!(console.get_global("ok").unwrap().as_boolean(), Some(true));
    assert!(console.save_document().is_none());
    assert_eq!(console.save_revision(), 1);
}

#[test]
fn byte_limit_counts_canonical_utf8_and_json_escaping_exactly() {
    let source = cart("");
    let mut console = Console::new(&source, 0).unwrap();
    assert_eq!(
        console
            .eval(r#"return save_store({text="蛙\\\""})"#)
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    let escaped = console.save_document().unwrap();
    assert!(escaped.contains("蛙"));
    assert!(escaped.contains(r#"\\\""#));
    assert!(escaped.len() > escaped.chars().count());

    assert_eq!(
        console
            .eval("return save_store({text=''})")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    let overhead = console.save_document().unwrap().len();
    let payload = MAX_SAVE_BYTES - overhead;
    assert_eq!(
        console
            .eval(&format!(
                "return save_store({{text=string.rep('x',{payload})}})"
            ))
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert_eq!(console.save_document().unwrap().len(), MAX_SAVE_BYTES);
    let committed = console.save_document();
    assert_eq!(
        console
            .eval(&format!(
                "return save_store({{text=string.rep('x',{})}})",
                payload + 1
            ))
            .unwrap()
            .as_boolean(),
        Some(false)
    );
    assert_eq!(console.save_document(), committed);
}

#[test]
fn empty_lua_tables_have_one_documented_array_canonical_form() {
    let source = cart("function _init() loaded=save_load() end");
    for data in ["{}", "[]"] {
        let initial = format!(r#"{{"data":{data},"id":"org.example.save-test","version":2}}"#);
        let mut console = Console::new_with_save(&source, 0, Some(&initial)).unwrap();
        assert_eq!(
            console
                .eval("return save_store(loaded)")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        assert_eq!(
            console.save_document().unwrap(),
            r#"{"data":[],"id":"org.example.save-test","version":2}"#
        );
    }
}

#[test]
fn a_write_in_a_failed_frame_never_reaches_the_host_snapshot() {
    let source = cart(
        r#"
function _update()
  save_store({x=2})
  error("boom")
end
"#,
    );
    let initial = r#"{"data":{"x":1},"id":"org.example.save-test","version":2}"#;
    let mut console = Console::new_with_save(&source, 0, Some(initial)).unwrap();
    assert!(console.step(0).is_err());
    assert_eq!(console.save_document().as_deref(), Some(initial));
    assert_eq!(console.save_revision(), 0);
}

#[test]
fn failed_eval_rolls_back_cart_visible_save_before_a_later_success() {
    let source = cart("function _update() observed=(save_load()).x end");
    let initial = r#"{"data":{"x":1},"id":"org.example.save-test","version":2}"#;
    let mut console = Console::new_with_save(&source, 0, Some(initial)).unwrap();
    assert!(
        console
            .eval("save_store({x=2}); error('eval failed')")
            .is_err()
    );
    assert_eq!(
        console.eval("return (save_load()).x").unwrap().as_i64(),
        Some(1)
    );
    console.step(0).unwrap();
    assert_eq!(console.get_global("observed").unwrap().as_i64(), Some(1));
    assert_eq!(console.save_document().as_deref(), Some(initial));
    assert_eq!(console.save_revision(), 0);
}

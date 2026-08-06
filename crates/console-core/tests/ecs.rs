use std::collections::BTreeMap;

use console_core::mlua::{Table, Value};
use console_core::{Console, ECS_QUERY_MAX_LIMIT};

fn console(lua: &str) -> Console {
    Console::new(&format!("__lua__\n{lua}\n"), 0).expect("ECS cart loads")
}

fn table(value: Value) -> Table {
    match value {
        Value::Table(table) => table,
        other => panic!("expected table, got {other:?}"),
    }
}

#[test]
fn ecs_exists_before_cart_top_level_and_defers_structural_changes() {
    let mut console = console(
        r#"
world = ecs.world("game", {capacity=8})
a = world:spawn({pos={x=1}})
b = world:spawn({pos={x=2}})
c = world:spawn({pos={x=3}})
seen = {}

function exercise()
  world:each({"pos"}, function(id, pos)
    seen[#seen+1] = id
    if id == a then
      world:despawn(b)
      spawned = world:spawn({pos={x=4}})
      during = {
        old_alive=world:alive(b),
        new_alive=world:alive(spawned),
        count=world:count({"pos"})
      }
      world:add(c, "marked", true)
      world:remove(c, "pos")
      nested = world:count({"pos"})
    end
  end)
end
"#,
    );

    let result = table(
        console
            .eval(
                "exercise(); return {seen=seen,during=during,nested=nested,\
                 alive={world:alive(a),world:alive(b),world:alive(c),world:alive(spawned)},\
                 remaining=world:entities({'pos'}),marked=world:get(c,'marked')}",
            )
            .unwrap(),
    );
    let seen: Table = result.get("seen").unwrap();
    assert_eq!(
        seen.sequence_values::<u64>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        [1, 2, 3]
    );
    let during: Table = result.get("during").unwrap();
    assert!(during.get::<bool>("old_alive").unwrap());
    assert!(!during.get::<bool>("new_alive").unwrap());
    assert_eq!(during.get::<u64>("count").unwrap(), 3);
    assert_eq!(result.get::<u64>("nested").unwrap(), 3);
    let alive: Table = result.get("alive").unwrap();
    assert_eq!(
        alive
            .sequence_values::<bool>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        [true, false, true, true]
    );
    let remaining: Table = result.get("remaining").unwrap();
    assert_eq!(
        remaining
            .sequence_values::<u64>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        [1, 4]
    );
    assert!(result.get::<bool>("marked").unwrap());
}

#[test]
fn each_preserves_exact_callback_arity_zero_through_sixteen() {
    let mut console = console(
        r##"
world = ecs.world("arity")
full = {}
partial = {}
for i=1,16 do
  full["c"..i] = i * 100 + 1
  if i <= 8 then partial["c"..i] = i * 100 + 2 end
end
full_id = world:spawn(full)
partial_id = world:spawn(partial)
arity_results = {}

for count=0,16 do
  local required = {}
  for i=1,count do required[i] = "c"..i end
  local calls = 0
  local order = ""
  local returned = world:each(required, function(...)
    calls = calls + 1
    local argc = select("#", ...)
    if argc ~= count + 1 then
      error("arity "..count.." received "..argc.." arguments")
    end
    local id = select(1, ...)
    order = order .. id
    for i=1,count do
      local component = select(i + 1, ...)
      local expected = i * 100 + (id == full_id and 1 or 2)
      if component ~= expected then
        error("arity "..count.." component "..i.." was "..tostring(component))
      end
    end
  end)
  arity_results[count + 1] = {returned=returned, calls=calls, order=order}
end
"##,
    );

    let results = table(console.eval("return arity_results").unwrap());
    for count in 0..=16 {
        let result: Table = results.get(count + 1).unwrap();
        let expected = if count <= 8 { 2 } else { 1 };
        assert_eq!(
            result.get::<u64>("returned").unwrap(),
            expected,
            "filter arity {count}"
        );
        assert_eq!(result.get::<u64>("calls").unwrap(), expected);
        assert_eq!(
            result.get::<String>("order").unwrap(),
            if count <= 8 { "12" } else { "1" },
            "creation order at filter arity {count}"
        );
    }
}

#[test]
fn nested_each_observes_the_stable_outer_selection_and_flushes_fifo_once() {
    let mut console = console(
        r#"
world = ecs.world("nested", {capacity=8})
a = world:spawn({tag="a"})
b = world:spawn({tag="b"})
c = world:spawn({tag="c"})
outer_seen = {}
nested_seen = {}

outer_count = world:each({"tag"}, function(id, tag)
  outer_seen[#outer_seen + 1] = id
  if id == a then
    world:despawn(b)
    d = world:spawn({tag="d"})
    world:add(c, "marked", "yes")
    nested_count = world:each({"tag"}, function(nested_id, nested_tag)
      nested_seen[#nested_seen + 1] = nested_id
      if world:alive(d) then error("pending spawn became visible") end
      if world:has(c, "marked") then error("pending add became visible") end
      if nested_id == c then
        world:remove(c, "tag")
        e = world:spawn({tag="e"})
      end
    end)
  end
end)
"#,
    );

    let result = table(
        console
            .eval(
                "return {outer_count=outer_count,nested_count=nested_count,outer=outer_seen,nested=nested_seen,\
                 tagged=world:entities({'tag'}),marked=world:get(c,'marked'),\
                 alive={world:alive(a),world:alive(b),world:alive(c),world:alive(d),world:alive(e)}}",
            )
            .unwrap(),
    );
    assert_eq!(result.get::<u64>("outer_count").unwrap(), 3);
    assert_eq!(result.get::<u64>("nested_count").unwrap(), 3);
    for key in ["outer", "nested"] {
        let seen: Table = result.get(key).unwrap();
        assert_eq!(
            seen.sequence_values::<u64>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            [1, 2, 3],
            "{key} query order"
        );
    }
    let tagged: Table = result.get("tagged").unwrap();
    assert_eq!(
        tagged
            .sequence_values::<u64>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        [1, 4, 5],
        "queued operations must flush in despawn/spawn/add/remove/spawn order"
    );
    assert_eq!(result.get::<String>("marked").unwrap(), "yes");
    let alive: Table = result.get("alive").unwrap();
    assert_eq!(
        alive
            .sequence_values::<bool>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        [true, false, true, true, true]
    );
}

#[test]
fn callback_errors_restore_query_depth_and_capacity_never_reuses_ids() {
    let mut console = console(
        r#"
world = ecs.world("game", {capacity=3})
first = world:spawn({doomed=true})
keeper = world:spawn({tag=true})
ok, message = pcall(function()
  world:each({"doomed"}, function(id)
    world:despawn(id)
    from_error = world:spawn({tag=true})
    world:add(keeper, "marked", "flushed")
    world:each({"tag"}, function() error("query boom") end)
  end)
end)
after_error = {
  first_alive=world:alive(first),
  spawned_alive=world:alive(from_error),
  marked=world:get(keeper, "marked"),
  alive=world:count()
}
second = world:spawn({tag=true})
before_overflow = world:stats()
overflow_ok, overflow_message = pcall(function() world:spawn({tag=true}) end)
world:clear()
third = world:spawn({tag=true})
"#,
    );
    let result = table(
        console
            .eval(
                "return {ok=ok,message=message,after_error=after_error,second=second,before_overflow=before_overflow,overflow_ok=overflow_ok,\
                 overflow_message=overflow_message,\
                 third=third,count=world:count()}",
            )
            .unwrap(),
    );
    assert!(!result.get::<bool>("ok").unwrap());
    assert!(
        result
            .get::<String>("message")
            .unwrap()
            .contains("query boom")
    );
    let after_error: Table = result.get("after_error").unwrap();
    assert!(!after_error.get::<bool>("first_alive").unwrap());
    assert!(after_error.get::<bool>("spawned_alive").unwrap());
    assert_eq!(after_error.get::<String>("marked").unwrap(), "flushed");
    assert_eq!(after_error.get::<u64>("alive").unwrap(), 2);
    assert_eq!(result.get::<u64>("second").unwrap(), 4);
    let before_overflow: Table = result.get("before_overflow").unwrap();
    assert_eq!(before_overflow.get::<u64>("alive").unwrap(), 3);
    assert_eq!(before_overflow.get::<u64>("capacity").unwrap(), 3);
    assert!(!result.get::<bool>("overflow_ok").unwrap());
    assert!(
        result
            .get::<String>("overflow_message")
            .unwrap()
            .contains("capacity 3")
    );
    assert_eq!(result.get::<u64>("third").unwrap(), 5);
    assert_eq!(result.get::<u64>("count").unwrap(), 1);
}

#[test]
fn component_breadth_is_bounded_for_worlds_and_inspection() {
    let mut console = console(
        r#"
world = ecs.world("bounded")
wide = {}
for i=1,129 do wide["c"..i] = true end
wide_ok, wide_message = pcall(function() world:spawn(wide) end)
entity = world:spawn({c1=true})
for i=2,128 do world:add(entity, "c"..i, true) end
before_extra = world:stats()
extra_ok, extra_message = pcall(function() world:add(entity, "c129", true) end)
"#,
    );
    let result = table(
        console
            .eval(
                "return {wide_ok=wide_ok,wide_message=wide_message,entity=entity,\
                 before_extra=before_extra,extra_ok=extra_ok,extra_message=extra_message}",
            )
            .unwrap(),
    );
    assert!(!result.get::<bool>("wide_ok").unwrap());
    assert!(
        result
            .get::<String>("wide_message")
            .unwrap()
            .contains("at most 128 components")
    );
    assert_eq!(result.get::<u64>("entity").unwrap(), 1);
    let stats: Table = result.get("before_extra").unwrap();
    assert_eq!(stats.get::<u64>("component_type_count").unwrap(), 128);
    assert!(!result.get::<bool>("extra_ok").unwrap());
    assert!(
        result
            .get::<String>("extra_message")
            .unwrap()
            .contains("component type limit 128")
    );
}

#[test]
fn protected_inspector_projects_stably_and_pages_by_entity_id() {
    let console = console(
        r#"
world = ecs.world("game")
for i=1,5 do
  world:spawn({pos={x=i*3,y=i*5,secret="hidden"}, bullet={kind="fan",radius=2}, tag=true})
end
ecs = nil
"#,
    );
    // The public table is gone, but the host retained the original closure.
    assert!(matches!(console.get_global("ecs").unwrap(), Value::Nil));

    let required = vec!["bullet".to_string()];
    let mut select = BTreeMap::new();
    select.insert("pos".to_string(), vec!["x".to_string(), "y".to_string()]);
    select.insert(
        "bullet".to_string(),
        vec!["kind".to_string(), "radius".to_string()],
    );
    select.insert("tag".to_string(), Vec::new());

    let first = table(console.ecs_query("game", &required, &select, 2, 0).unwrap());
    assert_eq!(first.get::<u64>("alive").unwrap(), 5);
    assert_eq!(first.get::<u64>("matched").unwrap(), 5);
    assert_eq!(first.get::<u64>("returned").unwrap(), 2);
    assert!(first.get::<bool>("truncated").unwrap());
    assert_eq!(first.get::<u64>("next_after").unwrap(), 2);
    let entities: Table = first.get("entities").unwrap();
    let first_entity: Table = entities.get(1).unwrap();
    assert_eq!(first_entity.get::<u64>("id").unwrap(), 1);
    let components: Table = first_entity.get("components").unwrap();
    let pos: Table = components.get("pos").unwrap();
    assert_eq!(pos.get::<u64>("x").unwrap(), 3);
    assert_eq!(pos.get::<u64>("y").unwrap(), 5);
    assert!(pos.get::<Value>("secret").unwrap().is_nil());
    assert!(components.get::<bool>("tag").unwrap());

    let second = table(console.ecs_query("game", &required, &select, 2, 2).unwrap());
    let entities: Table = second.get("entities").unwrap();
    assert_eq!(
        entities.get::<Table>(1).unwrap().get::<u64>("id").unwrap(),
        3
    );
    assert_eq!(
        entities.get::<Table>(2).unwrap().get::<u64>("id").unwrap(),
        4
    );
    assert!(second.get::<bool>("truncated").unwrap());

    let last = table(
        console
            .ecs_query("game", &required, &select, ECS_QUERY_MAX_LIMIT, 4)
            .unwrap(),
    );
    assert_eq!(last.get::<u64>("returned").unwrap(), 1);
    assert!(!last.get::<bool>("truncated").unwrap());
    assert_eq!(last.get::<u64>("next_after").unwrap(), 5);
}

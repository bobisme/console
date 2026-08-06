//! Embedded deterministic Lua ECS and its protected host inspector.

use std::collections::BTreeMap;

use mlua::{Function, Lua, RegistryKey, Result as LuaResult, Table, Value};

pub const ECS_QUERY_DEFAULT_LIMIT: usize = 64;
pub const ECS_QUERY_MAX_LIMIT: usize = 128;
pub const ECS_QUERY_MAX_WITH: usize = 16;
pub const ECS_QUERY_MAX_SELECT: usize = 8;
pub const ECS_QUERY_MAX_FIELDS: usize = 16;
pub const ECS_MAX_SAFE_ID: u64 = 9_007_199_254_740_991;

pub fn install(lua: &Lua) -> LuaResult<RegistryKey> {
    let (library, inspect): (Table, Function) = lua
        .load(include_str!("ecs.lua"))
        .set_name("@console/ecs.lua")
        .eval()?;
    lua.globals().set("ecs", library)?;
    lua.create_registry_value(inspect)
}

pub fn query(
    lua: &Lua,
    inspector: &RegistryKey,
    world: &str,
    required: &[String],
    select: &BTreeMap<String, Vec<String>>,
    limit: usize,
    after: u64,
) -> LuaResult<Value> {
    let options = lua.create_table()?;
    let with = lua.create_sequence_from(required.iter().map(String::as_str))?;
    options.set("with", with)?;
    let selected = lua.create_table()?;
    for (component, fields) in select {
        let fields = lua.create_sequence_from(fields.iter().map(String::as_str))?;
        selected.set(component.as_str(), fields)?;
    }
    options.set("select", selected)?;
    options.set("limit", limit)?;
    options.set("after", after)?;
    lua.registry_value::<Function>(inspector)?
        .call((world, options))
}

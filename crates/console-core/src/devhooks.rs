//! Protected deterministic development-hook registry and bounded values.

use std::collections::BTreeMap;

use mlua::{Function, Lua, RegistryKey, Table, Value};
use serde::Serialize;

use crate::Error;

pub const DEV_HOOK_MAX_HOOKS: usize = 32;
pub const DEV_HOOK_MAX_NAME_BYTES: usize = 64;
pub const DEV_HOOK_MAX_DESCRIPTION_BYTES: usize = 160;
pub const DEV_HOOK_MAX_DEPTH: usize = 4;
pub const DEV_HOOK_MAX_NODES: usize = 128;
pub const DEV_HOOK_MAX_TABLE_ENTRIES: usize = 64;
pub const DEV_HOOK_MAX_STRING_BYTES: usize = 4096;
pub const DEV_HOOK_MAX_KEY_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DevHookPhase {
    PreFrame,
    PostFrame,
}

impl DevHookPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreFrame => "pre_frame",
            Self::PostFrame => "post_frame",
        }
    }

    fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "pre_frame" => Ok(Self::PreFrame),
            "post_frame" => Ok(Self::PostFrame),
            _ => Err(Error::Lua(format!(
                "invalid development hook phase {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DevHookInfo {
    pub name: String,
    pub description: String,
    pub phase: DevHookPhase,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DevValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Array(Vec<DevValue>),
    Object(BTreeMap<String, DevValue>),
}

pub(crate) struct Registry {
    list: RegistryKey,
    invoke: RegistryKey,
    lock: RegistryKey,
}

pub(crate) fn install(lua: &Lua) -> mlua::Result<Registry> {
    let (library, list, invoke, lock): (Table, Function, Function, Function) = lua
        .load(include_str!("devhooks.lua"))
        .set_name("@console/devhooks.lua")
        .eval()?;
    lua.globals().set("devhook", library)?;
    Ok(Registry {
        list: lua.create_registry_value(list)?,
        invoke: lua.create_registry_value(invoke)?,
        lock: lua.create_registry_value(lock)?,
    })
}

pub(crate) fn lock(lua: &Lua, registry: &Registry) -> mlua::Result<()> {
    lua.registry_value::<Function>(&registry.lock)?.call(())
}

pub(crate) fn list(lua: &Lua, registry: &Registry) -> Result<Vec<DevHookInfo>, Error> {
    let table: Table = lua.registry_value::<Function>(&registry.list)?.call(())?;
    let mut result = Vec::with_capacity(table.raw_len());
    for entry in table.sequence_values::<Table>() {
        let entry = entry?;
        let phase: String = entry.get("phase")?;
        result.push(DevHookInfo {
            name: entry.get("name")?,
            description: entry.get("description")?,
            phase: DevHookPhase::parse(&phase)?,
        });
    }
    Ok(result)
}

pub(crate) fn validate_phase(
    lua: &Lua,
    registry: &Registry,
    name: &str,
    expected_phase: DevHookPhase,
) -> Result<(), Error> {
    let info = list(lua, registry)?
        .into_iter()
        .find(|hook| hook.name == name)
        .ok_or_else(|| Error::Lua(format!("unknown development hook {name:?}")))?;
    if info.phase != expected_phase {
        return Err(Error::Lua(format!(
            "development hook {name:?} has phase {}, not {}",
            info.phase.as_str(),
            expected_phase.as_str()
        )));
    }
    Ok(())
}

pub(crate) fn invoke_validated(
    lua: &Lua,
    registry: &Registry,
    name: &str,
    expected_phase: DevHookPhase,
    args: &DevValue,
) -> Result<DevValue, Error> {
    let mut budget = Budget::default();
    let args = to_lua(lua, args, 0, &mut budget)?;
    let value: Value = lua.registry_value::<Function>(&registry.invoke)?.call((
        name,
        expected_phase.as_str(),
        args,
    ))?;
    let mut budget = Budget::default();
    from_lua(value, 0, &mut budget)
}

#[derive(Default)]
struct Budget {
    nodes: usize,
    entries: usize,
    strings: usize,
}

impl Budget {
    fn node(&mut self, depth: usize) -> Result<(), Error> {
        if depth > DEV_HOOK_MAX_DEPTH {
            return Err(bound_error("value nesting exceeds depth 4"));
        }
        self.nodes += 1;
        if self.nodes > DEV_HOOK_MAX_NODES {
            return Err(bound_error("value contains more than 128 nodes"));
        }
        Ok(())
    }

    fn entries(&mut self, count: usize) -> Result<(), Error> {
        self.entries += count;
        if self.entries > DEV_HOOK_MAX_TABLE_ENTRIES {
            return Err(bound_error("value contains more than 64 table entries"));
        }
        Ok(())
    }

    fn string(&mut self, value: &str) -> Result<(), Error> {
        self.strings += value.len();
        if self.strings > DEV_HOOK_MAX_STRING_BYTES {
            return Err(bound_error(
                "value contains more than 4096 bytes of strings",
            ));
        }
        Ok(())
    }
}

fn bound_error(message: impl Into<String>) -> Error {
    Error::Lua(format!("development hook value error: {}", message.into()))
}

fn to_lua(lua: &Lua, value: &DevValue, depth: usize, budget: &mut Budget) -> Result<Value, Error> {
    budget.node(depth)?;
    Ok(match value {
        DevValue::Null => Value::Nil,
        DevValue::Boolean(value) => Value::Boolean(*value),
        DevValue::Integer(value) => Value::Integer(*value),
        DevValue::Number(value) if value.is_finite() => Value::Number(*value),
        DevValue::Number(_) => return Err(bound_error("numbers must be finite")),
        DevValue::String(value) => {
            budget.string(value)?;
            Value::String(lua.create_string(value)?)
        }
        DevValue::Array(values) => {
            budget.entries(values.len())?;
            let table = lua.create_table()?;
            for (index, value) in values.iter().enumerate() {
                table.raw_set(index + 1, to_lua(lua, value, depth + 1, budget)?)?;
            }
            Value::Table(table)
        }
        DevValue::Object(values) => {
            budget.entries(values.len())?;
            let table = lua.create_table()?;
            for (key, value) in values {
                if key.is_empty() || key.len() > DEV_HOOK_MAX_KEY_BYTES {
                    return Err(bound_error("object keys must be 1-64 bytes"));
                }
                budget.string(key)?;
                table.set(key.as_str(), to_lua(lua, value, depth + 1, budget)?)?;
            }
            Value::Table(table)
        }
    })
}

fn from_lua(value: Value, depth: usize, budget: &mut Budget) -> Result<DevValue, Error> {
    budget.node(depth)?;
    match value {
        Value::Nil => Ok(DevValue::Null),
        Value::Boolean(value) => Ok(DevValue::Boolean(value)),
        Value::Integer(value) => Ok(DevValue::Integer(value)),
        Value::Number(value) if value.is_finite() => Ok(DevValue::Number(value)),
        Value::Number(_) => Err(bound_error("numbers must be finite")),
        Value::String(value) => {
            let value = value
                .to_str()
                .map_err(|_| bound_error("strings must be valid UTF-8"))?
                .to_string();
            budget.string(&value)?;
            Ok(DevValue::String(value))
        }
        Value::Table(table) => table_from_lua(table, depth, budget),
        other => Err(bound_error(format!(
            "unsupported Lua result type {}",
            other.type_name()
        ))),
    }
}

fn table_from_lua(table: Table, depth: usize, budget: &mut Budget) -> Result<DevValue, Error> {
    let mut entries = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        entries.push(pair?);
    }
    budget.entries(entries.len())?;
    if entries.is_empty() {
        return Ok(DevValue::Array(Vec::new()));
    }

    let count = entries.len();
    let mut sequence = vec![None; count];
    let mut array = true;
    for (key, value) in &entries {
        match key {
            Value::Integer(index) if *index >= 1 && (*index as usize) <= count => {
                let slot = &mut sequence[*index as usize - 1];
                if slot.is_some() {
                    return Err(bound_error("array contains duplicate indices"));
                }
                *slot = Some(value.clone());
            }
            _ => array = false,
        }
    }
    if array && sequence.iter().all(Option::is_some) {
        return sequence
            .into_iter()
            .map(|value| from_lua(value.expect("checked"), depth + 1, budget))
            .collect::<Result<Vec<_>, _>>()
            .map(DevValue::Array);
    }

    let mut result = BTreeMap::new();
    for (key, value) in entries {
        let Value::String(key) = key else {
            return Err(bound_error(
                "tables must be dense arrays or objects with string keys",
            ));
        };
        let key = key
            .to_str()
            .map_err(|_| bound_error("object keys must be valid UTF-8"))?
            .to_string();
        if key.is_empty() || key.len() > DEV_HOOK_MAX_KEY_BYTES {
            return Err(bound_error("object keys must be 1-64 bytes"));
        }
        budget.string(&key)?;
        if result
            .insert(key, from_lua(value, depth + 1, budget)?)
            .is_some()
        {
            return Err(bound_error("object contains duplicate keys"));
        }
    }
    Ok(DevValue::Object(result))
}

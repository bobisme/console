//! Bounded deterministic cart persistence values and Lua conversion.
//!
//! This module deliberately knows nothing about files, browser storage, or
//! platform SDKs. A host injects one serialized document before `_init`, then
//! observes committed revisions at successful execution boundaries.

use std::collections::{BTreeMap, HashSet};
use std::ffi::c_void;

use mlua::{Lua, MultiValue, Table, Value};
use serde::{Deserialize, Serialize};

use crate::Error;

/// Maximum UTF-8 byte length of the complete serialized host envelope.
pub const MAX_SAVE_BYTES: usize = 8 * 1024;
/// Maximum nested arrays/objects beneath the root value.
pub const MAX_SAVE_DEPTH: usize = 16;
/// Maximum number of values traversed while validating or converting.
pub const MAX_SAVE_NODES: usize = 4096;
/// Maximum UTF-8 byte length of one object key.
pub const MAX_SAVE_KEY_BYTES: usize = 128;
/// Largest integer that survives an intervening JavaScript JSON round trip.
pub const SAVE_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Stable host namespace and the schema emitted by new writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveConfig {
    id: String,
    version: u32,
}

impl SaveConfig {
    pub(crate) fn from_meta(meta: &BTreeMap<String, String>) -> Result<Option<Self>, Error> {
        match (meta.get("save_id"), meta.get("save_version")) {
            (None, None) => Ok(None),
            (Some(_), None) => Err(Error::Cart(
                "__meta__ save_id requires save_version=<positive integer>".into(),
            )),
            (None, Some(_)) => Err(Error::Cart(
                "__meta__ save_version requires a stable save_id".into(),
            )),
            (Some(id), Some(version)) => {
                if id.is_empty()
                    || id.len() > 128
                    || !id.as_bytes()[0].is_ascii_alphanumeric()
                    || !id.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })
                {
                    return Err(Error::Cart(
                        "__meta__ save_id must be 1-128 ASCII letters/digits/./_/- and begin with a letter or digit"
                            .into(),
                    ));
                }
                let version = version.parse::<u32>().map_err(|_| {
                    Error::Cart("__meta__ save_version must be a positive u32 integer".into())
                })?;
                if version == 0 {
                    return Err(Error::Cart(
                        "__meta__ save_version must be a positive u32 integer".into(),
                    ));
                }
                Ok(Some(Self {
                    id: id.clone(),
                    version,
                }))
            }
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> u32 {
        self.version
    }
}

/// JSON-like values accepted by persistence. Null is intentionally absent:
/// Lua nil deletes a table entry and cannot round-trip inside dense arrays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SaveValue {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Array(Vec<SaveValue>),
    Object(BTreeMap<String, SaveValue>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveDocument {
    data: SaveValue,
    id: String,
    version: u32,
}

/// Pure console state behind `save_load`, `save_store`, and `save_clear`.
#[derive(Debug, Clone)]
pub(crate) struct SaveState {
    config: Option<SaveConfig>,
    current: Option<SaveDocument>,
    revision: u32,
    diagnostic: Option<String>,
}

impl SaveState {
    pub(crate) fn new(config: Option<SaveConfig>, initial: Option<&str>) -> Self {
        let mut state = Self {
            config,
            current: None,
            revision: 0,
            diagnostic: None,
        };
        let Some(initial) = initial else {
            return state;
        };
        let Some(config) = &state.config else {
            state.diagnostic = Some("host supplied save data, but cart declares no save_id".into());
            return state;
        };
        if initial.len() > MAX_SAVE_BYTES {
            state.diagnostic = Some(format!(
                "initial save is {} serialized bytes; maximum is {MAX_SAVE_BYTES}",
                initial.len()
            ));
            return state;
        }
        let document = match serde_json::from_str::<SaveDocument>(initial) {
            Ok(document) => document,
            Err(error) => {
                state.diagnostic = Some(format!("invalid initial save document: {error}"));
                return state;
            }
        };
        if document.id != config.id {
            state.diagnostic = Some(format!(
                "initial save id {:?} does not match cart save_id {:?}",
                document.id, config.id
            ));
            return state;
        }
        if let Err(error) = validate_value(&document.data, 0, &mut 0) {
            state.diagnostic = Some(format!("invalid initial save data: {error}"));
            return state;
        }
        // Re-encoding proves the canonical form also fits the transport cap.
        match serialize_document(&document) {
            Ok(_) => state.current = Some(document),
            Err(error) => state.diagnostic = Some(error),
        }
        state
    }

    pub(crate) fn document(&self) -> Option<String> {
        self.current
            .as_ref()
            .and_then(|document| serialize_document(document).ok())
    }

    pub(crate) fn revision(&self) -> u32 {
        self.revision
    }

    pub(crate) fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    fn load(&self) -> Option<(SaveValue, u32)> {
        self.current
            .as_ref()
            .map(|document| (document.data.clone(), document.version))
    }

    fn store(&mut self, value: SaveValue) -> Result<(), String> {
        let config = self
            .config
            .as_ref()
            .ok_or("cart must declare save_id and save_version before calling save_store")?;
        validate_value(&value, 0, &mut 0)?;
        let document = SaveDocument {
            data: value,
            id: config.id.clone(),
            version: config.version,
        };
        serialize_document(&document)?;
        self.current = Some(document);
        self.revision = self.revision.saturating_add(1);
        self.diagnostic = None;
        Ok(())
    }

    fn clear(&mut self) -> Result<(), String> {
        if self.config.is_none() {
            return Err(
                "cart must declare save_id and save_version before calling save_clear".into(),
            );
        }
        self.current = None;
        self.revision = self.revision.saturating_add(1);
        self.diagnostic = None;
        Ok(())
    }

    fn reject(&mut self, message: String) -> (bool, Option<String>) {
        self.diagnostic = Some(message.clone());
        (false, Some(message))
    }
}

fn serialize_document(document: &SaveDocument) -> Result<String, String> {
    let text = serde_json::to_string(document)
        .map_err(|error| format!("cannot serialize save document: {error}"))?;
    if text.len() > MAX_SAVE_BYTES {
        return Err(format!(
            "save document is {} serialized bytes; maximum is {MAX_SAVE_BYTES}; previous save retained",
            text.len()
        ));
    }
    Ok(text)
}

/// Build the exact bounded envelope a host may inject before `_init`.
/// Tooling uses this instead of growing a second interpretation of save
/// limits, key ordering, number safety, or serialized byte accounting.
pub fn canonical_save_document(
    config: &SaveConfig,
    stored_version: u32,
    data: SaveValue,
) -> Result<String, String> {
    if stored_version == 0 {
        return Err("stored save version must be a positive u32 integer".into());
    }
    validate_value(&data, 0, &mut 0)?;
    serialize_document(&SaveDocument {
        data,
        id: config.id.clone(),
        version: stored_version,
    })
}

fn validate_value(value: &SaveValue, depth: usize, nodes: &mut usize) -> Result<(), String> {
    if depth > MAX_SAVE_DEPTH {
        return Err(format!("save nesting exceeds depth {MAX_SAVE_DEPTH}"));
    }
    *nodes += 1;
    if *nodes > MAX_SAVE_NODES {
        return Err(format!("save contains more than {MAX_SAVE_NODES} values"));
    }
    match value {
        SaveValue::Integer(value) if value.unsigned_abs() > SAVE_MAX_SAFE_INTEGER as u64 => Err(
            format!("integer {value} exceeds the JavaScript-safe range +/-{SAVE_MAX_SAFE_INTEGER}"),
        ),
        SaveValue::Number(value) if !value.is_finite() => Err("save numbers must be finite".into()),
        SaveValue::Array(values) => {
            for value in values {
                validate_value(value, depth + 1, nodes)?;
            }
            Ok(())
        }
        SaveValue::Object(values) => {
            for (key, value) in values {
                if key.is_empty() || key.len() > MAX_SAVE_KEY_BYTES {
                    return Err(format!(
                        "save object keys must be 1-{MAX_SAVE_KEY_BYTES} UTF-8 bytes"
                    ));
                }
                validate_value(value, depth + 1, nodes)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(crate) fn register(
    lua: &Lua,
    state: &std::rc::Rc<std::cell::RefCell<crate::state::State>>,
) -> mlua::Result<()> {
    let globals = lua.globals();

    let shared = state.clone();
    globals.set(
        "save_load",
        lua.create_function(move |lua, ()| {
            let Some((value, version)) = shared.borrow().save.load() else {
                return Ok(MultiValue::from_vec(vec![Value::Nil, Value::Nil]));
            };
            let mut nodes = 0;
            let value = to_lua(lua, &value, 0, &mut nodes)?;
            Ok(MultiValue::from_vec(vec![
                value,
                Value::Integer(i64::from(version)),
            ]))
        })?,
    )?;

    let shared = state.clone();
    globals.set(
        "save_store",
        lua.create_function(move |_, value: Value| {
            let converted = from_lua(value, 0, &mut 0, &mut HashSet::new());
            let mut state = shared.borrow_mut();
            match converted.and_then(|value| state.save.store(value)) {
                Ok(()) => Ok((true, None::<String>)),
                Err(error) => Ok(state.save.reject(error)),
            }
        })?,
    )?;

    let shared = state.clone();
    globals.set(
        "save_clear",
        lua.create_function(move |_, ()| {
            let mut state = shared.borrow_mut();
            match state.save.clear() {
                Ok(()) => Ok((true, None::<String>)),
                Err(error) => Ok(state.save.reject(error)),
            }
        })?,
    )?;
    Ok(())
}

fn from_lua(
    value: Value,
    depth: usize,
    nodes: &mut usize,
    stack: &mut HashSet<*const c_void>,
) -> Result<SaveValue, String> {
    if depth > MAX_SAVE_DEPTH {
        return Err(format!("save nesting exceeds depth {MAX_SAVE_DEPTH}"));
    }
    *nodes += 1;
    if *nodes > MAX_SAVE_NODES {
        return Err(format!("save contains more than {MAX_SAVE_NODES} values"));
    }
    match value {
        Value::Boolean(value) => Ok(SaveValue::Boolean(value)),
        Value::Integer(value) if value.unsigned_abs() <= SAVE_MAX_SAFE_INTEGER as u64 => {
            Ok(SaveValue::Integer(value))
        }
        Value::Integer(value) => Err(format!(
            "integer {value} exceeds the JavaScript-safe range +/-{SAVE_MAX_SAFE_INTEGER}"
        )),
        Value::Number(value) if value.is_finite() => Ok(SaveValue::Number(value)),
        Value::Number(_) => Err("save numbers must be finite".into()),
        Value::String(value) => value
            .to_str()
            .map(|value| SaveValue::String(value.to_string()))
            .map_err(|_| "save strings must be valid UTF-8".into()),
        Value::Table(table) => table_from_lua(table, depth, nodes, stack),
        Value::Nil => Err("save values cannot contain nil; omit object fields instead".into()),
        other => Err(format!("unsupported save value type {}", other.type_name())),
    }
}

fn table_from_lua(
    table: Table,
    depth: usize,
    nodes: &mut usize,
    stack: &mut HashSet<*const c_void>,
) -> Result<SaveValue, String> {
    let pointer = table.to_pointer();
    if !stack.insert(pointer) {
        return Err("save tables cannot contain cycles".into());
    }
    let result = (|| {
        let mut entries = Vec::new();
        for pair in table.pairs::<Value, Value>() {
            entries.push(pair.map_err(|error| format!("reading save table: {error}"))?);
            if entries.len() > MAX_SAVE_NODES {
                return Err(format!("save contains more than {MAX_SAVE_NODES} values"));
            }
        }
        if entries.is_empty() {
            return Ok(SaveValue::Array(Vec::new()));
        }
        let mut sequence = vec![None; entries.len()];
        let mut dense = true;
        for (key, value) in &entries {
            match key {
                Value::Integer(index) if *index >= 1 && (*index as usize) <= entries.len() => {
                    let slot = &mut sequence[*index as usize - 1];
                    if slot.is_some() {
                        return Err("save array contains duplicate indices".into());
                    }
                    *slot = Some(value.clone());
                }
                _ => dense = false,
            }
        }
        if dense && sequence.iter().all(Option::is_some) {
            return sequence
                .into_iter()
                .map(|value| from_lua(value.expect("checked"), depth + 1, nodes, stack))
                .collect::<Result<Vec<_>, _>>()
                .map(SaveValue::Array);
        }

        let mut object = BTreeMap::new();
        for (key, value) in entries {
            let Value::String(key) = key else {
                return Err("save tables must be dense arrays or objects with string keys".into());
            };
            let key = key
                .to_str()
                .map_err(|_| "save object keys must be valid UTF-8")?
                .to_string();
            if key.is_empty() || key.len() > MAX_SAVE_KEY_BYTES {
                return Err(format!(
                    "save object keys must be 1-{MAX_SAVE_KEY_BYTES} UTF-8 bytes"
                ));
            }
            let value = from_lua(value, depth + 1, nodes, stack)?;
            if object.insert(key, value).is_some() {
                return Err("save object contains duplicate keys".into());
            }
        }
        Ok(SaveValue::Object(object))
    })();
    stack.remove(&pointer);
    result
}

fn to_lua(lua: &Lua, value: &SaveValue, depth: usize, nodes: &mut usize) -> mlua::Result<Value> {
    if depth > MAX_SAVE_DEPTH {
        return Err(mlua::Error::RuntimeError(
            "stored save nesting exceeds its validated bound".into(),
        ));
    }
    *nodes += 1;
    if *nodes > MAX_SAVE_NODES {
        return Err(mlua::Error::RuntimeError(
            "stored save value count exceeds its validated bound".into(),
        ));
    }
    Ok(match value {
        SaveValue::Boolean(value) => Value::Boolean(*value),
        SaveValue::Integer(value) => Value::Integer(*value),
        SaveValue::Number(value) => Value::Number(*value),
        SaveValue::String(value) => Value::String(lua.create_string(value)?),
        SaveValue::Array(values) => {
            let table = lua.create_table()?;
            for (index, value) in values.iter().enumerate() {
                table.raw_set(index + 1, to_lua(lua, value, depth + 1, nodes)?)?;
            }
            Value::Table(table)
        }
        SaveValue::Object(values) => {
            let table = lua.create_table()?;
            for (key, value) in values {
                table.set(key.as_str(), to_lua(lua, value, depth + 1, nodes)?)?;
            }
            Value::Table(table)
        }
    })
}

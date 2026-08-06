//! Convert `mlua::Value` results (from `eval`/`get_global`) into JSON,
//! best-effort, for the RPC layer and the oneshot post-frame eval flags.

use console_core::mlua::{Table, Value};
use serde_json::{Map, Number, Value as Json};

/// How deep into nested tables we're willing to recurse. Also doubles as
/// cycle protection: a self-referential table just bottoms out into a
/// placeholder once the cap is hit instead of looping forever.
const MAX_DEPTH: usize = 6;

/// Serialize a Lua value to JSON, matching the console RPC contract:
/// `nil` -> `null`, booleans/integers/numbers/strings map directly, tables
/// are arrays when they are a plain 1..n sequence and objects (string keys)
/// otherwise, and anything else (functions, userdata, threads, ...) becomes
/// a short placeholder string like `"<function>"`.
pub fn lua_to_json(value: &Value) -> Json {
    to_json_depth(value, 0)
}

fn to_json_depth(value: &Value, depth: usize) -> Json {
    match value {
        Value::Nil => Json::Null,
        Value::Boolean(b) => Json::Bool(*b),
        Value::Integer(i) => Json::Number((*i).into()),
        Value::Number(f) => Number::from_f64(*f).map(Json::Number).unwrap_or(Json::Null),
        Value::String(s) => Json::String(s.to_string_lossy()),
        Value::Table(t) => {
            if depth >= MAX_DEPTH {
                Json::String("<table>".to_string())
            } else {
                table_to_json(t, depth)
            }
        }
        Value::Function(_) => Json::String("<function>".to_string()),
        Value::Thread(_) => Json::String("<thread>".to_string()),
        Value::UserData(_) => Json::String("<userdata>".to_string()),
        Value::LightUserData(_) => Json::String("<userdata>".to_string()),
        Value::Error(e) => Json::String(format!("<error: {e}>")),
        _ => Json::String("<unknown>".to_string()),
    }
}

/// A table is a JSON array when its keys are exactly the integers `1..=n`
/// for `n` the number of entries; otherwise every key (stringified) becomes
/// a JSON object field.
fn table_to_json(table: &Table, depth: usize) -> Json {
    let mut entries: Vec<(Value, Value)> = Vec::new();
    for pair in table.clone().pairs::<Value, Value>() {
        match pair {
            Ok(kv) => entries.push(kv),
            Err(_) => return Json::String("<table>".to_string()),
        }
    }

    let n = entries.len();
    // Sequential iff the key set is exactly {1, ..., n} (vacuously true for
    // the empty table, which we render as `[]`).
    let is_sequence = {
        let mut seen = vec![false; n];
        entries.iter().all(|(k, _)| match k {
            Value::Integer(i) if *i >= 1 && (*i as usize) <= n => {
                seen[*i as usize - 1] = true;
                true
            }
            _ => false,
        }) && seen.into_iter().all(|s| s)
    };

    if is_sequence {
        entries.sort_by_key(|(k, _)| match k {
            Value::Integer(i) => *i,
            _ => 0,
        });
        Json::Array(
            entries
                .iter()
                .map(|(_, v)| to_json_depth(v, depth + 1))
                .collect(),
        )
    } else {
        let mut map = Map::new();
        for (k, v) in &entries {
            map.insert(key_to_string(k), to_json_depth(v, depth + 1));
        }
        Json::Object(map)
    }
}

fn key_to_string(key: &Value) -> String {
    match key {
        Value::String(s) => s.to_string_lossy(),
        Value::Integer(i) => i.to_string(),
        Value::Number(f) => f.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Nil => "nil".to_string(),
        _ => "<key>".to_string(),
    }
}

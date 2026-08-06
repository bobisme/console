//! Shared development-hook CLI/RPC value validation and discovery command.

use std::collections::BTreeMap;
use std::path::Path;

use console_core::{
    DEV_HOOK_MAX_DEPTH, DEV_HOOK_MAX_KEY_BYTES, DEV_HOOK_MAX_NAME_BYTES, DEV_HOOK_MAX_NODES,
    DEV_HOOK_MAX_STRING_BYTES, DEV_HOOK_MAX_TABLE_ENTRIES, DevHookInfo, DevValue,
};
use serde_json::{Value as Json, json};

use crate::session::Session;

pub const USAGE: &str = "usage:\n  console hooks <cart|project> [--seed N]";

#[derive(Debug, Clone, PartialEq)]
pub struct HookCall {
    pub name: String,
    pub args: DevValue,
}

pub fn validate_name(name: &str) -> Result<String, String> {
    if name.is_empty() || name.len() > DEV_HOOK_MAX_NAME_BYTES {
        return Err(format!(
            "development hook names must be 1-{DEV_HOOK_MAX_NAME_BYTES} bytes"
        ));
    }
    let mut bytes = name.bytes();
    let starts_valid = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    let rest_valid =
        bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'));
    if !starts_valid || !rest_valid {
        return Err(
            "development hook names must begin with a letter/_ and contain only letters, digits, _, ., or -"
                .to_string(),
        );
    }
    Ok(name.to_string())
}

pub fn parse_call(value: &str) -> Result<HookCall, String> {
    let (name, args) = if let Some((name, source)) = value.split_once('=') {
        let args = serde_json::from_str(source)
            .map_err(|error| format!("invalid JSON arguments for hook {name:?}: {error}"))?;
        (name, args)
    } else {
        (value, Json::Null)
    };
    Ok(HookCall {
        name: validate_name(name)?,
        args: json_to_dev_value(&args)?,
    })
}

#[derive(Default)]
struct Budget {
    nodes: usize,
    entries: usize,
    strings: usize,
}

pub fn json_to_dev_value(value: &Json) -> Result<DevValue, String> {
    from_json(value, 0, &mut Budget::default())
}

fn from_json(value: &Json, depth: usize, budget: &mut Budget) -> Result<DevValue, String> {
    if depth > DEV_HOOK_MAX_DEPTH {
        return Err(format!(
            "development hook value nesting exceeds depth {DEV_HOOK_MAX_DEPTH}"
        ));
    }
    budget.nodes += 1;
    if budget.nodes > DEV_HOOK_MAX_NODES {
        return Err(format!(
            "development hook value contains more than {DEV_HOOK_MAX_NODES} nodes"
        ));
    }
    match value {
        Json::Null => Ok(DevValue::Null),
        Json::Bool(value) => Ok(DevValue::Boolean(*value)),
        Json::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(DevValue::Integer(value))
            } else if value.as_u64().is_some() {
                Err("development hook integers must fit signed 64-bit range".to_string())
            } else if let Some(value) = value.as_f64().filter(|value| value.is_finite()) {
                Ok(DevValue::Number(value))
            } else {
                Err("development hook numbers must be finite signed integers or floats".to_string())
            }
        }
        Json::String(value) => {
            add_string(budget, value)?;
            Ok(DevValue::String(value.clone()))
        }
        Json::Array(values) => {
            add_entries(budget, values.len())?;
            values
                .iter()
                .map(|value| from_json(value, depth + 1, budget))
                .collect::<Result<Vec<_>, _>>()
                .map(DevValue::Array)
        }
        Json::Object(values) => {
            add_entries(budget, values.len())?;
            let mut result = BTreeMap::new();
            for (key, value) in values {
                if key.is_empty() || key.len() > DEV_HOOK_MAX_KEY_BYTES {
                    return Err(format!(
                        "development hook object keys must be 1-{DEV_HOOK_MAX_KEY_BYTES} bytes"
                    ));
                }
                add_string(budget, key)?;
                result.insert(key.clone(), from_json(value, depth + 1, budget)?);
            }
            Ok(DevValue::Object(result))
        }
    }
}

fn add_entries(budget: &mut Budget, count: usize) -> Result<(), String> {
    budget.entries += count;
    if budget.entries > DEV_HOOK_MAX_TABLE_ENTRIES {
        return Err(format!(
            "development hook value contains more than {DEV_HOOK_MAX_TABLE_ENTRIES} table entries"
        ));
    }
    Ok(())
}

fn add_string(budget: &mut Budget, value: &str) -> Result<(), String> {
    budget.strings += value.len();
    if budget.strings > DEV_HOOK_MAX_STRING_BYTES {
        return Err(format!(
            "development hook value contains more than {DEV_HOOK_MAX_STRING_BYTES} bytes of strings"
        ));
    }
    Ok(())
}

pub fn dev_value_to_json(value: &DevValue) -> Json {
    serde_json::to_value(value).expect("DevValue always serializes to JSON")
}

pub fn hook_list_json(frame_count: u64, hooks: &[DevHookInfo]) -> Json {
    json!({"frame_count": frame_count, "hooks": hooks})
}

pub fn cli_hooks(args: &[String]) -> i32 {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        println!("{USAGE}");
        return 0;
    }
    let mut cart = None;
    let mut seed = 0;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--seed" => {
                let Some(value) = iter.next() else {
                    eprintln!("error: --seed requires a value");
                    return 2;
                };
                match value.parse() {
                    Ok(value) => seed = value,
                    Err(_) => {
                        eprintln!("error: invalid --seed value {value:?}");
                        return 2;
                    }
                }
            }
            value if value.starts_with("--") => {
                eprintln!("error: unknown flag {value:?}");
                return 2;
            }
            value if cart.is_none() => cart = Some(value),
            value => {
                eprintln!("error: unexpected extra argument {value:?}");
                return 2;
            }
        }
    }
    let Some(cart) = cart else {
        eprintln!("error: missing <cart|project> argument");
        eprintln!("{USAGE}");
        return 2;
    };
    let text = match crate::project::load_cart_text(Path::new(cart)) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let mut session = Session::new();
    if let Err(error) = session.load_cart(&text, seed) {
        eprintln!("error: {error}");
        return 1;
    }
    match session.dev_hooks() {
        Ok(hooks) => {
            println!("{}", hook_list_json(0, &hooks));
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

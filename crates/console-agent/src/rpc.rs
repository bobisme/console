//! JSON-RPC 2.0 dispatch over a [`Session`].
//!
//! [`handle`] takes one already-parsed request (as a `serde_json::Value`)
//! and a session, and returns the full response envelope as a `Value` —
//! this is the layer the unit tests exercise directly. [`handle_line`] and
//! [`run_serve`] are thin wrappers for the `serve` subcommand's stdin/stdout
//! line protocol.

use std::io::{self, BufRead, Write};

use console_core::input;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::session::{Session, SessionError};
use crate::value::lua_to_json;

/// A JSON-RPC error: `code` + `message` are always present, `data` carries
/// extra detail (e.g. the Lua traceback) when there is any.
struct RpcErr {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcErr {
    fn new(code: i64, message: impl Into<String>) -> RpcErr {
        RpcErr {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn with_data(code: i64, message: impl Into<String>, data: Value) -> RpcErr {
        RpcErr {
            code,
            message: message.into(),
            data: Some(data),
        }
    }

    fn bad_params(message: impl Into<String>) -> RpcErr {
        RpcErr::new(-32602, message.into())
    }
}

impl From<SessionError> for RpcErr {
    fn from(e: SessionError) -> Self {
        match e {
            SessionError::NoCart => RpcErr::new(-32002, "no cart loaded"),
            SessionError::BadParams(m) => RpcErr::new(-32602, m),
            SessionError::AlreadyHalted(m) => {
                RpcErr::with_data(-32000, "console already halted", json!({ "message": m }))
            }
            SessionError::Cart(err) => {
                RpcErr::with_data(-32000, err.to_string(), json!({ "message": err.message() }))
            }
            SessionError::Io(m) => RpcErr::new(-32602, m),
        }
    }
}

#[derive(Deserialize, Default)]
struct RawRequest {
    #[serde(default)]
    id: Value,
    #[serde(default)]
    method: String,
    #[serde(default)]
    params: Value,
}

/// Handle one already-parsed JSON-RPC request and return the response
/// envelope (never `Err` — malformed requests turn into JSON-RPC error
/// responses, they don't propagate as Rust errors).
pub fn handle(session: &mut Session, request: Value) -> Value {
    let raw: RawRequest = match serde_json::from_value(request) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                Value::Null,
                -32700,
                "parse error",
                Some(json!({ "message": e.to_string() })),
            );
        }
    };

    match dispatch(session, &raw.method, &raw.params) {
        Ok(result) => success_response(raw.id, result),
        Err(err) => error_response(raw.id, err.code, err.message, err.data),
    }
}

/// Parse one line of input as JSON, handle it, and serialize the response
/// back to a single line (no trailing newline).
pub fn handle_line(session: &mut Session, line: &str) -> String {
    let response = match serde_json::from_str::<Value>(line) {
        Ok(v) => handle(session, v),
        Err(e) => error_response(
            Value::Null,
            -32700,
            "parse error",
            Some(json!({ "message": e.to_string() })),
        ),
    };
    serde_json::to_string(&response).expect("response value is always serializable")
}

/// Run the `serve` loop: one JSON-RPC request per line of `reader`, one
/// response per line of `writer`, flushed after every line.
pub fn run_serve<R: BufRead, W: Write>(
    mut session: Session,
    reader: R,
    mut writer: W,
) -> io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_line(&mut session, &line);
        writer.write_all(response.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    Ok(())
}

fn success_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message.into() });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

fn dispatch(session: &mut Session, method: &str, params: &Value) -> Result<Value, RpcErr> {
    match method {
        "load_cart" => m_load_cart(session, params),
        "reset" => m_reset(session, params),
        "step" => m_step(session, params),
        "screenshot" => m_screenshot(session, params),
        "screen_text" => m_screen_text(session),
        "eval" => m_eval(session, params),
        "get_global" => m_get_global(session, params),
        "logs" => m_logs(session),
        "save_state" => m_save_state(session, params),
        "load_state" => m_load_state(session, params),
        "info" => m_info(session),
        other => Err(RpcErr::new(-32601, format!("unknown method {other:?}"))),
    }
}

fn string_param<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

fn m_load_cart(session: &mut Session, params: &Value) -> Result<Value, RpcErr> {
    let seed = params.get("seed").and_then(Value::as_u64).unwrap_or(0);
    let text = if let Some(text) = string_param(params, "text") {
        text.to_string()
    } else if let Some(path) = string_param(params, "path") {
        std::fs::read_to_string(path)
            .map_err(|e| RpcErr::bad_params(format!("cannot read {path:?}: {e}")))?
    } else {
        return Err(RpcErr::bad_params(
            "load_cart requires a \"path\" or \"text\" string param",
        ));
    };

    session.load_cart(&text, seed)?;
    let console = session.console()?;
    Ok(json!({ "ok": true, "title": console.cart().title(), "seed": console.seed() }))
}

fn m_reset(session: &mut Session, params: &Value) -> Result<Value, RpcErr> {
    let seed = params.get("seed").and_then(Value::as_u64);
    session.reset(seed)?;
    let console = session.console()?;
    Ok(json!({ "ok": true, "seed": console.seed() }))
}

fn parse_input_param(value: Option<&Value>) -> Result<u8, RpcErr> {
    match value {
        None | Some(Value::Null) => Ok(0),
        Some(Value::String(s)) => {
            input::parse(s).map_err(|c| RpcErr::bad_params(format!("unknown button {c:?}")))
        }
        Some(Value::Number(n)) => {
            let i = n.as_i64().ok_or_else(|| {
                RpcErr::bad_params("\"input\" must be an integer mask or a button string")
            })?;
            u8::try_from(i).map_err(|_| RpcErr::bad_params("\"input\" mask out of range 0..=255"))
        }
        Some(other) => Err(RpcErr::bad_params(format!(
            "\"input\" must be a string or integer, got {other}"
        ))),
    }
}

fn m_step(session: &mut Session, params: &Value) -> Result<Value, RpcErr> {
    let frames = params.get("frames").and_then(Value::as_u64).unwrap_or(1);
    let mask = parse_input_param(params.get("input"))?;
    let outcome = session.step(frames, mask)?;
    Ok(json!({
        "frame_count": outcome.frame_count,
        "halted": outcome.halted,
        "message": outcome.message,
    }))
}

fn m_screenshot(session: &mut Session, params: &Value) -> Result<Value, RpcErr> {
    let path = string_param(params, "path")
        .ok_or_else(|| RpcErr::bad_params("screenshot requires a \"path\" string param"))?;
    let png_bytes = session.screenshot_png()?;
    std::fs::write(path, &png_bytes)
        .map_err(|e| RpcErr::bad_params(format!("cannot write {path:?}: {e}")))?;
    Ok(json!({
        "ok": true,
        "path": path,
        "width": console_core::SCREEN_W,
        "height": console_core::SCREEN_H,
    }))
}

fn m_screen_text(session: &Session) -> Result<Value, RpcErr> {
    let lines = session.screen_text()?;
    Ok(json!({ "lines": lines }))
}

fn m_eval(session: &mut Session, params: &Value) -> Result<Value, RpcErr> {
    let code = string_param(params, "code")
        .ok_or_else(|| RpcErr::bad_params("eval requires a \"code\" string param"))?;
    let value = session.eval(code)?;
    Ok(json!({ "result": lua_to_json(&value) }))
}

fn m_get_global(session: &mut Session, params: &Value) -> Result<Value, RpcErr> {
    let name = string_param(params, "name")
        .ok_or_else(|| RpcErr::bad_params("get_global requires a \"name\" string param"))?;
    let value = session.get_global(name)?;
    Ok(json!({ "result": lua_to_json(&value) }))
}

fn m_logs(session: &mut Session) -> Result<Value, RpcErr> {
    let logs = session.logs()?;
    Ok(json!({ "logs": logs }))
}

fn m_save_state(session: &mut Session, params: &Value) -> Result<Value, RpcErr> {
    let name = string_param(params, "name")
        .ok_or_else(|| RpcErr::bad_params("save_state requires a \"name\" string param"))?;
    session.save_state(name)?;
    Ok(json!({ "ok": true }))
}

fn m_load_state(session: &mut Session, params: &Value) -> Result<Value, RpcErr> {
    let name = string_param(params, "name")
        .ok_or_else(|| RpcErr::bad_params("load_state requires a \"name\" string param"))?;
    let outcome = session.load_state(name)?;
    Ok(json!({
        "ok": true,
        "replayed_frames": outcome.frame_count,
        "frame_count": outcome.frame_count,
        "halted": outcome.halted,
        "message": outcome.message,
    }))
}

fn m_info(session: &Session) -> Result<Value, RpcErr> {
    let info = session.info()?;
    Ok(json!({
        "frame_count": info.frame_count,
        "seed": info.seed,
        "halted": info.halted.is_some(),
        "halt_message": info.halted,
        "title": info.title,
        "meta": info.meta,
        "input_log_len": info.input_log_len,
        "saved_states": info.saved_states,
    }))
}

//! The Lua-facing API (v0) and the sandbox.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, Result as LuaResult, Table, Value, Variadic};

use crate::gfx::{self, col, fl};
use crate::state::State;

/// Number of buttons in the input mask.
pub const BUTTON_COUNT: i32 = 7;

const TAU: f64 = std::f64::consts::TAU;

type Shared = Rc<RefCell<State>>;

/// Format a Lua value the way `print`/`printh` should show it.
///
/// Integral floats render without a trailing `.0` (PICO-8 style), which keeps
/// on-screen readouts tidy. Tables and functions render as their type name so
/// output never contains a nondeterministic pointer.
fn to_text(lua: &Lua, v: Value) -> String {
    match v {
        Value::Nil => "nil".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => {
            if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", n as i64)
            } else {
                format!("{n}")
            }
        }
        Value::String(s) => s.to_string_lossy(),
        other => match lua.coerce_string(other.clone()) {
            Ok(Some(s)) => s.to_string_lossy(),
            _ => other.type_name().to_string(),
        },
    }
}

/// Lua truthiness for optional boolean-ish arguments.
fn truthy(v: Option<&Value>) -> bool {
    !matches!(v, None | Some(Value::Nil) | Some(Value::Boolean(false)))
}

/// Return an integer when the value is integral, otherwise a float.
fn num(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 {
        Value::Integer(v as i64)
    } else {
        Value::Number(v)
    }
}

/// Globals removed by [`sandbox`].
pub const REMOVED_GLOBALS: &[&str] = &[
    "io",
    "os",
    "debug",
    "package",
    "require",
    "dofile",
    "loadfile",
    "load",
    "loadstring",
];

/// Install the sandbox: drop unsafe globals and stub out `math.random`.
pub fn sandbox(lua: &Lua) -> LuaResult<()> {
    let globals = lua.globals();
    // Anything that touches the host, the filesystem or the bytecode loader.
    // (`rawget`/`rawset`/`pcall`/`select`/... stay: they are pure and useful.)
    for name in REMOVED_GLOBALS {
        globals.set(*name, Value::Nil)?;
    }

    if let Ok(math) = globals.get::<Table>("math") {
        for (name, hint) in [("random", "rnd(n)"), ("randomseed", "srand(seed)")] {
            let msg = format!(
                "math.{name} is disabled for determinism - use {hint} instead \
                 (the console PRNG is seeded by the host)"
            );
            let f = lua.create_function(move |_, ()| -> LuaResult<()> {
                Err(mlua::Error::RuntimeError(msg.clone()))
            })?;
            math.set(name, f)?;
        }
    }
    Ok(())
}

/// Register the whole v0 API as globals.
pub fn register(lua: &Lua, state: &Shared) -> LuaResult<()> {
    let g = lua.globals();

    // ---- screen -----------------------------------------------------------
    let st = state.clone();
    g.set(
        "cls",
        lua.create_function(move |_, c: Option<f64>| {
            gfx::cls(&mut st.borrow_mut().fb, col(c.unwrap_or(0.0)));
            Ok(())
        })?,
    )?;

    let st = state.clone();
    g.set(
        "pset",
        lua.create_function(move |_, (x, y, c): (f64, f64, Option<f64>)| {
            gfx::pset(&mut st.borrow_mut().fb, fl(x), fl(y), col(c.unwrap_or(0.0)));
            Ok(())
        })?,
    )?;

    let st = state.clone();
    g.set(
        "pget",
        lua.create_function(move |_, (x, y): (f64, f64)| {
            Ok(gfx::pget(&st.borrow().fb, fl(x), fl(y)))
        })?,
    )?;

    let st = state.clone();
    g.set(
        "line",
        lua.create_function(
            move |_, (x0, y0, x1, y1, c): (f64, f64, f64, f64, Option<f64>)| {
                gfx::line(
                    &mut st.borrow_mut().fb,
                    fl(x0),
                    fl(y0),
                    fl(x1),
                    fl(y1),
                    col(c.unwrap_or(0.0)),
                );
                Ok(())
            },
        )?,
    )?;

    let st = state.clone();
    g.set(
        "rect",
        lua.create_function(
            move |_, (x0, y0, x1, y1, c): (f64, f64, f64, f64, Option<f64>)| {
                gfx::rect(
                    &mut st.borrow_mut().fb,
                    fl(x0),
                    fl(y0),
                    fl(x1),
                    fl(y1),
                    col(c.unwrap_or(0.0)),
                );
                Ok(())
            },
        )?,
    )?;

    let st = state.clone();
    g.set(
        "rectfill",
        lua.create_function(
            move |_, (x0, y0, x1, y1, c): (f64, f64, f64, f64, Option<f64>)| {
                gfx::rectfill(
                    &mut st.borrow_mut().fb,
                    fl(x0),
                    fl(y0),
                    fl(x1),
                    fl(y1),
                    col(c.unwrap_or(0.0)),
                );
                Ok(())
            },
        )?,
    )?;

    let st = state.clone();
    g.set(
        "circ",
        lua.create_function(move |_, (x, y, r, c): (f64, f64, Option<f64>, Option<f64>)| {
            gfx::circ(
                &mut st.borrow_mut().fb,
                fl(x),
                fl(y),
                fl(r.unwrap_or(4.0)),
                col(c.unwrap_or(0.0)),
            );
            Ok(())
        })?,
    )?;

    let st = state.clone();
    g.set(
        "circfill",
        lua.create_function(move |_, (x, y, r, c): (f64, f64, Option<f64>, Option<f64>)| {
            gfx::circfill(
                &mut st.borrow_mut().fb,
                fl(x),
                fl(y),
                fl(r.unwrap_or(4.0)),
                col(c.unwrap_or(0.0)),
            );
            Ok(())
        })?,
    )?;

    type SprArgs = (
        f64,
        f64,
        f64,
        Option<f64>,
        Option<f64>,
        Option<Value>,
        Option<Value>,
    );
    let st = state.clone();
    g.set(
        "spr",
        lua.create_function(move |_, (n, x, y, w, h, fx, fy): SprArgs| {
            let mut s = st.borrow_mut();
            let State { fb, sheet, .. } = &mut *s;
            gfx::spr(
                fb,
                sheet,
                fl(n),
                fl(x),
                fl(y),
                (fl(w.unwrap_or(1.0)), fl(h.unwrap_or(1.0))),
                (truthy(fx.as_ref()), truthy(fy.as_ref())),
            );
            Ok(())
        })?,
    )?;

    let st = state.clone();
    g.set(
        "print",
        lua.create_function(
            move |lua, (s, x, y, c): (Value, Option<f64>, Option<f64>, Option<f64>)| {
                // Coerce before borrowing: __tostring could re-enter Lua.
                let text = to_text(lua, s);
                gfx::print(
                    &mut st.borrow_mut().fb,
                    &text,
                    fl(x.unwrap_or(0.0)),
                    fl(y.unwrap_or(0.0)),
                    col(c.unwrap_or(12.0)),
                );
                Ok(())
            },
        )?,
    )?;

    // ---- input ------------------------------------------------------------
    let st = state.clone();
    g.set(
        "btn",
        lua.create_function(move |_, i: f64| {
            let i = fl(i);
            if !(0..BUTTON_COUNT).contains(&i) {
                return Ok(false);
            }
            Ok(st.borrow().input & (1 << i) != 0)
        })?,
    )?;

    let st = state.clone();
    g.set(
        "btnp",
        lua.create_function(move |_, i: f64| {
            let i = fl(i);
            if !(0..BUTTON_COUNT).contains(&i) {
                return Ok(false);
            }
            let s = st.borrow();
            let bit = 1u8 << i;
            Ok(s.input & bit != 0 && s.prev_input & bit == 0)
        })?,
    )?;

    // ---- randomness & time -------------------------------------------------
    let st = state.clone();
    g.set(
        "rnd",
        lua.create_function(move |_, n: Option<f64>| {
            let r = st.borrow_mut().rng.next_f64();
            Ok(r * n.unwrap_or(1.0))
        })?,
    )?;

    let st = state.clone();
    g.set(
        "srand",
        lua.create_function(move |_, seed: Option<f64>| {
            let s = seed.unwrap_or(0.0);
            let s = if s.is_finite() { s.floor() } else { 0.0 };
            st.borrow_mut().rng.reseed(s as i64 as u64);
            Ok(())
        })?,
    )?;

    let st = state.clone();
    g.set(
        "t",
        lua.create_function(move |_, ()| Ok(st.borrow().frame as f64 / 60.0))?,
    )?;

    // ---- math helpers ------------------------------------------------------
    g.set(
        "flr",
        lua.create_function(|_, x: f64| Ok(num(x.floor())))?,
    )?;
    g.set("ceil", lua.create_function(|_, x: f64| Ok(num(x.ceil())))?)?;
    g.set("abs", lua.create_function(|_, x: f64| Ok(num(x.abs())))?)?;
    g.set(
        "min",
        lua.create_function(|_, xs: Variadic<f64>| fold(xs, "min", f64::min))?,
    )?;
    g.set(
        "max",
        lua.create_function(|_, xs: Variadic<f64>| fold(xs, "max", f64::max))?,
    )?;
    g.set(
        "mid",
        lua.create_function(|_, (a, b, c): (f64, f64, f64)| {
            Ok(num(a.max(b).min(a.min(b).max(c))))
        })?,
    )?;
    // Turns, not radians. Sign is *not* inverted (unlike PICO-8).
    g.set(
        "sin",
        lua.create_function(|_, x: f64| Ok((x * TAU).sin()))?,
    )?;
    g.set(
        "cos",
        lua.create_function(|_, x: f64| Ok((x * TAU).cos()))?,
    )?;

    // ---- audio -------------------------------------------------------------
    // Both take effect immediately, so a call from `_update` or `_draw` is
    // audible in the same frame's 735 samples.
    let st = state.clone();
    g.set(
        "sfx",
        lua.create_function(move |_, (n, ch): (f64, Option<f64>)| {
            let ch = ch.map_or(-1, fl);
            st.borrow_mut()
                .audio
                .lua_sfx(fl(n), ch)
                .map_err(mlua::Error::RuntimeError)
        })?,
    )?;

    let st = state.clone();
    g.set(
        "music",
        lua.create_function(move |_, n: f64| {
            st.borrow_mut()
                .audio
                .lua_music(fl(n))
                .map_err(mlua::Error::RuntimeError)
        })?,
    )?;

    // ---- host --------------------------------------------------------------
    let st = state.clone();
    g.set(
        "printh",
        lua.create_function(move |lua, v: Value| {
            let text = to_text(lua, v);
            st.borrow_mut().logs.push(text);
            Ok(())
        })?,
    )?;

    Ok(())
}

fn fold(xs: Variadic<f64>, name: &str, f: fn(f64, f64) -> f64) -> LuaResult<Value> {
    let mut it = xs.into_iter();
    let first = it
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError(format!("{name}() needs at least one value")))?;
    Ok(num(it.fold(first, f)))
}

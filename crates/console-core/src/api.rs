//! The Lua-facing API (v0) and the sandbox.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, Result as LuaResult, Table, Value, Variadic};

use crate::gfx::{self, MAP_H, MAP_W, col, fl};
use crate::gfx_meta::{AnimDef, GfxMeta, SpriteDef};
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

/// Flat index of map cell `(cx, cy)`, or `None` when it is off the map.
/// Coordinates are floored the same way every other draw coordinate is.
fn cell(cx: f64, cy: f64) -> Option<usize> {
    let (x, y) = (fl(cx), fl(cy));
    if x < 0 || y < 0 || x as usize >= MAP_W || y as usize >= MAP_H {
        return None;
    }
    Some(y as usize * MAP_W + x as usize)
}

/// Floor a Lua float to a tile id, masked into 0..=255 (the sprite-sheet range
/// wraps, mirroring how [`col`] masks colours).
fn tile(v: f64) -> u8 {
    let f = v.floor();
    let i = if f.is_nan() { 0i64 } else { f as i64 };
    (i & 0xff) as u8
}

/// Console frames of playback for an anim whose origin is `t0`.
///
/// `t0` is floored like every other Lua number the API takes; omitting it means
/// 0, i.e. "phase-locked to the global frame counter".
fn elapsed(frame: u64, t0: Option<f64>) -> i64 {
    frame as i64 - i64::from(fl(t0.unwrap_or(0.0)))
}

/// Look up `name` ("sprite.label") in the cart's `__gfx_meta__`, together with
/// the sprite its frames are relative to.
///
/// An unknown name is a Lua error, not a silent no-op: a typo'd anim name that
/// simply drew nothing would be a nightmare to spot in a 60fps picture, so it
/// halts the cart the way every other cart-authoring mistake does.
fn anim_and_sprite<'a>(
    meta: &'a GfxMeta,
    who: &str,
    name: &str,
) -> LuaResult<(&'a AnimDef, &'a SpriteDef)> {
    let Some(anim) = meta.anim(name) else {
        let hint = if meta.is_empty() {
            "; this cart has no __gfx_meta__ section".to_string()
        } else {
            let names: Vec<&str> = meta.anims().map(|a| a.name.as_str()).collect();
            format!("; declared anims: {}", names.join(", "))
        };
        return Err(mlua::Error::RuntimeError(format!(
            "{who}: no anim named {name:?} in __gfx_meta__{hint}"
        )));
    };
    // Parse-time validation guarantees the sprite exists.
    let sprite = meta
        .sprite(&anim.sprite)
        .ok_or_else(|| mlua::Error::RuntimeError(format!("{who}: anim {name:?} has no sprite")))?;
    Ok((anim, sprite))
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
            let mut s = st.borrow_mut();
            let State { fb, draw, .. } = &mut *s;
            gfx::cls(fb, draw, col(c.unwrap_or(0.0)));
            Ok(())
        })?,
    )?;

    let st = state.clone();
    g.set(
        "pset",
        lua.create_function(move |_, (x, y, c): (f64, f64, Option<f64>)| {
            let mut s = st.borrow_mut();
            let State { fb, draw, .. } = &mut *s;
            gfx::pset(fb, draw, fl(x), fl(y), col(c.unwrap_or(0.0)));
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
                let mut s = st.borrow_mut();
                let State { fb, draw, .. } = &mut *s;
                gfx::line(
                    fb,
                    draw,
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
                let mut s = st.borrow_mut();
                let State { fb, draw, .. } = &mut *s;
                gfx::rect(
                    fb,
                    draw,
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
                let mut s = st.borrow_mut();
                let State { fb, draw, .. } = &mut *s;
                gfx::rectfill(
                    fb,
                    draw,
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
        lua.create_function(
            move |_, (x, y, r, c): (f64, f64, Option<f64>, Option<f64>)| {
                let mut s = st.borrow_mut();
                let State { fb, draw, .. } = &mut *s;
                gfx::circ(
                    fb,
                    draw,
                    fl(x),
                    fl(y),
                    fl(r.unwrap_or(4.0)),
                    col(c.unwrap_or(0.0)),
                );
                Ok(())
            },
        )?,
    )?;

    let st = state.clone();
    g.set(
        "circfill",
        lua.create_function(
            move |_, (x, y, r, c): (f64, f64, Option<f64>, Option<f64>)| {
                let mut s = st.borrow_mut();
                let State { fb, draw, .. } = &mut *s;
                gfx::circfill(
                    fb,
                    draw,
                    fl(x),
                    fl(y),
                    fl(r.unwrap_or(4.0)),
                    col(c.unwrap_or(0.0)),
                );
                Ok(())
            },
        )?,
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
            let State {
                fb, draw, sheet, ..
            } = &mut *s;
            gfx::spr(
                fb,
                draw,
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

    // `sspr(sx, sy, sw, sh, dx, dy, [dw=sw], [dh=sh], [flip_x], [flip_y])`:
    // blit an arbitrary sheet rectangle into an arbitrary screen rectangle,
    // scaled with nearest-neighbour sampling. Same pixel rules as `spr`
    // (camera, clip, `pal`, `palt`); a zero or negative size draws nothing.
    type SsprArgs = (
        f64,
        f64,
        f64,
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
        "sspr",
        lua.create_function(move |_, args: SsprArgs| {
            let (sx, sy, sw, sh, dx, dy, dw, dh, fx, fy) = args;
            let (sw, sh) = (fl(sw), fl(sh));
            let mut s = st.borrow_mut();
            let State {
                fb, draw, sheet, ..
            } = &mut *s;
            gfx::sspr(
                fb,
                draw,
                sheet,
                (fl(sx), fl(sy), sw, sh),
                (fl(dx), fl(dy), dw.map_or(sw, fl), dh.map_or(sh, fl)),
                (truthy(fx.as_ref()), truthy(fy.as_ref())),
            );
            Ok(())
        })?,
    )?;

    // ---- declared animations ------------------------------------------------
    // `aspr(name, x, y, [t0=0], [flip_x], [flip_y])`: draw the `__gfx_meta__`
    // anim `name` ("sprite.label") with its frame chosen by elapsed time.
    //
    // Stateless by construction: the frame is a pure function of
    // `frame_count - t0`, so there is no per-instance animation object, no new
    // console state, and a replay of the same inputs replays the same pixels.
    // `t0` is the frame-count origin — pass the frame a state change happened
    // on to restart the cycle there, or leave it out to phase-lock the anim to
    // the global clock (what ambient loops want).
    //
    // Unlike `spr`, `(x, y)` is the sprite's declared `anchor=` — the feet of a
    // walker, the centre of a floater — not its top-left. The anchor does NOT
    // mirror under `flip_x`: the destination rect is the same either way and
    // the flip only mirrors pixels inside it, exactly like `spr`.
    type AsprArgs = (
        mlua::LuaString,
        f64,
        f64,
        Option<f64>,
        Option<Value>,
        Option<Value>,
    );
    let st = state.clone();
    g.set(
        "aspr",
        lua.create_function(move |_, (name, x, y, t0, fx, fy): AsprArgs| {
            let name = name.to_str()?;
            let mut s = st.borrow_mut();
            let State {
                fb,
                draw,
                sheet,
                gfx_meta,
                frame,
                ..
            } = &mut *s;
            let (anim, sprite) = anim_and_sprite(gfx_meta, "aspr", &name)?;
            let pos = anim.frame_at(elapsed(*frame, t0));
            // Every frame is validated against the sheet at parse time, so a
            // `None` here would be a core bug rather than a cart error.
            let (sx, sy, w, h) = anim.resolve_frame(sprite, pos).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "aspr: anim {name:?} frame {pos} does not resolve to a sheet rect"
                ))
            })?;
            gfx::spr_rect(
                fb,
                draw,
                sheet,
                (sx as i32, sy as i32),
                (fl(x) - sprite.anchor.0, fl(y) - sprite.anchor.1),
                (w as i32, h as i32),
                (truthy(fx.as_ref()), truthy(fy.as_ref())),
            );
            Ok(())
        })?,
    )?;

    // `anim_len(name)`: how many frames the anim declares.
    let st = state.clone();
    g.set(
        "anim_len",
        lua.create_function(move |_, name: mlua::LuaString| {
            let name = name.to_str()?;
            let s = st.borrow();
            let (anim, _) = anim_and_sprite(&s.gfx_meta, "anim_len", &name)?;
            Ok(anim.len())
        })?,
    )?;

    // `anim_done(name, [t0=0])`: true once a ONE-SHOT anim has shown its last
    // frame for its full duration and playback has clamped there. Looping
    // anims are never done. The one-line end condition for an attack state.
    let st = state.clone();
    g.set(
        "anim_done",
        lua.create_function(move |_, (name, t0): (mlua::LuaString, Option<f64>)| {
            let name = name.to_str()?;
            let s = st.borrow();
            let (anim, _) = anim_and_sprite(&s.gfx_meta, "anim_done", &name)?;
            Ok(anim.done_at(elapsed(s.frame, t0)))
        })?,
    )?;

    // ---- tile map -----------------------------------------------------------
    // `map([cel_x=0], [cel_y=0], [sx=0], [sy=0], [cel_w=MAP_W], [cel_h=MAP_H])`:
    // blit a block of map cells. Bare `map()` draws the whole map at (0, 0).
    // Every cell goes through the same low-level blit as `spr`, so the camera,
    // the clip rect, `pal` and `palt` all apply exactly as they do to sprites.
    // Tile 0 is the empty cell and is skipped.
    type MapArgs = (
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    );
    let st = state.clone();
    g.set(
        "map",
        lua.create_function(move |_, (cx, cy, sx, sy, cw, ch): MapArgs| {
            let mut s = st.borrow_mut();
            let State {
                fb,
                draw,
                sheet,
                map,
                ..
            } = &mut *s;
            gfx::map(
                fb,
                draw,
                sheet,
                map,
                (fl(cx.unwrap_or(0.0)), fl(cy.unwrap_or(0.0))),
                (fl(sx.unwrap_or(0.0)), fl(sy.unwrap_or(0.0))),
                (
                    fl(cw.unwrap_or(MAP_W as f64)),
                    fl(ch.unwrap_or(MAP_H as f64)),
                ),
            );
            Ok(())
        })?,
    )?;

    // `mget(cx, cy)`: tile id at a map cell; off the map reads as 0.
    let st = state.clone();
    g.set(
        "mget",
        lua.create_function(move |_, (cx, cy): (f64, f64)| {
            Ok(match cell(cx, cy) {
                Some(i) => st.borrow().map[i],
                None => 0,
            })
        })?,
    )?;

    // `mset(cx, cy, [v=0])`: write a tile id. Off the map is a no-op. The map
    // is ordinary console state, so the change persists across frames and a
    // replay of the same inputs reproduces it.
    let st = state.clone();
    g.set(
        "mset",
        lua.create_function(move |_, (cx, cy, v): (f64, f64, Option<f64>)| {
            if let Some(i) = cell(cx, cy) {
                st.borrow_mut().map[i] = tile(v.unwrap_or(0.0));
            }
            Ok(())
        })?,
    )?;

    g.set(
        "text_size",
        lua.create_function(move |lua, value: Value| {
            let text = to_text(lua, value);
            Ok(gfx::text_size(&text))
        })?,
    )?;

    let st = state.clone();
    g.set(
        "print",
        lua.create_function(
            move |lua,
                  (s, x, y, c, align): (
                Value,
                Option<f64>,
                Option<f64>,
                Option<f64>,
                Option<String>,
            )| {
                // Coerce before borrowing: __tostring could re-enter Lua.
                let text = to_text(lua, s);
                let align = match align.as_deref() {
                    None => gfx::TextAlign::Left,
                    Some(value) => gfx::TextAlign::parse(value).ok_or_else(|| {
                        mlua::Error::RuntimeError(format!(
                            "print align must be left, center, or right; got {value:?}"
                        ))
                    })?,
                };
                let x = fl(x.unwrap_or(0.0));
                let y = fl(y.unwrap_or(0.0));
                let color = col(c.unwrap_or(12.0));
                let mut s = st.borrow_mut();
                let State {
                    fb,
                    draw,
                    text_draws,
                    ..
                } = &mut *s;
                let layout = gfx::print_aligned(fb, draw, &text, x, y, color, align);
                text_draws.push(gfx::TextDraw {
                    text,
                    anchor_x: x,
                    anchor_y: y,
                    color,
                    align,
                    layout,
                });
                Ok(())
            },
        )?,
    )?;

    // ---- draw state ---------------------------------------------------------
    // camera / clip / pal / palt all persist across frames (PICO-8 semantics):
    // nothing resets them at frame boundaries, only an explicit call does.

    // `camera([x], [y])`: integer draw offset subtracted from every subsequent
    // drawing op. No arguments resets to (0, 0). `pget` is unaffected.
    let st = state.clone();
    g.set(
        "camera",
        lua.create_function(move |_, (x, y): (Option<f64>, Option<f64>)| {
            st.borrow_mut()
                .draw
                .set_camera(fl(x.unwrap_or(0.0)), fl(y.unwrap_or(0.0)));
            Ok(())
        })?,
    )?;

    // `clip([x, y, w, h])`: clip rectangle in SCREEN space (applied after the
    // camera offset). No arguments resets to the full screen. `cls` respects it.
    type ClipArgs = (Option<f64>, Option<f64>, Option<f64>, Option<f64>);
    let st = state.clone();
    g.set(
        "clip",
        lua.create_function(move |_, (x, y, w, h): ClipArgs| {
            let mut s = st.borrow_mut();
            if x.is_none() && y.is_none() && w.is_none() && h.is_none() {
                s.draw.reset_clip();
            } else {
                s.draw.set_clip(
                    fl(x.unwrap_or(0.0)),
                    fl(y.unwrap_or(0.0)),
                    fl(w.unwrap_or(0.0)),
                    fl(h.unwrap_or(0.0)),
                );
            }
            Ok(())
        })?,
    )?;

    // `pal([c0], [c1], [p=0])`: p=0 remaps at draw time (the framebuffer really
    // changes), p=1 remaps at scanout (the framebuffer keeps its indices — this
    // is what makes whole-screen fades free). No arguments resets both maps and
    // `palt`.
    let st = state.clone();
    g.set(
        "pal",
        lua.create_function(
            move |_, (c0, c1, p): (Option<f64>, Option<f64>, Option<f64>)| {
                let mut s = st.borrow_mut();
                match c0 {
                    None => s.draw.reset_pal(),
                    Some(from) => {
                        let (from, to) = (col(from), col(c1.unwrap_or(from)));
                        if p.map_or(0, fl) == 1 {
                            s.draw.set_display_pal(from, to);
                        } else {
                            s.draw.set_draw_pal(from, to);
                        }
                    }
                }
                Ok(())
            },
        )?,
    )?;

    // `palt([c], [flag])`: which colours `spr` treats as transparent. Tested on
    // the sprite's SOURCE colour, before the draw palette remaps it. No
    // arguments resets to "only colour 0".
    let st = state.clone();
    g.set(
        "palt",
        lua.create_function(move |_, (c, flag): (Option<f64>, Option<Value>)| {
            let mut s = st.borrow_mut();
            match c {
                None => s.draw.reset_palt(),
                Some(c) => s.draw.set_palt(col(c), truthy(flag.as_ref())),
            }
            Ok(())
        })?,
    )?;

    // `fillp([p], [secondary])`: the 4x4 dither pattern used by the shape
    // primitives. A clear bit draws the shape's colour; a set bit draws the
    // optional secondary colour or punches a hole when it is omitted. No
    // arguments (or pattern 0) is solid. `pal()` deliberately does not reset it.
    let st = state.clone();
    g.set(
        "fillp",
        lua.create_function(move |_, (p, secondary): (Option<f64>, Option<f64>)| {
            let mut s = st.borrow_mut();
            match p {
                None => s.draw.reset_fillp(),
                Some(p) => {
                    let f = p.floor();
                    let i = if f.is_nan() { 0i64 } else { f as i64 };
                    s.draw.set_fillp((i & 0xffff) as u16, secondary.map(col));
                }
            }
            Ok(())
        })?,
    )?;

    // `mosaic([f=1])`: end-of-frame pixelation. Every f x f block of the
    // FINISHED frame becomes its top-left pixel, so unlike the display palette
    // this really is what `screen_text` and the goldens show. Clamped to
    // 1..=MAX_MOSAIC; 1 (or no arguments) is off.
    let st = state.clone();
    g.set(
        "mosaic",
        lua.create_function(move |_, f: Option<f64>| {
            st.borrow_mut().draw.set_mosaic(fl(f.unwrap_or(1.0)));
            Ok(())
        })?,
    )?;

    // `rshift([y], [dx=0])`: per-scanline horizontal shift of the FINISHED
    // frame, applied after `mosaic`. Positive dx moves the line right and the
    // line WRAPS (dx is reduced mod SCREEN_W, so -1 == 191). Write-only and
    // deliberately allocation-free: carts sweep it with 320 calls a frame.
    // No arguments clears every line; `rshift(y)` clears just line y.
    let st = state.clone();
    g.set(
        "rshift",
        lua.create_function(move |_, (y, dx): (Option<f64>, Option<f64>)| {
            let mut s = st.borrow_mut();
            match y {
                None => s.draw.reset_rshift(),
                Some(y) => s.draw.set_rshift(fl(y), fl(dx.unwrap_or(0.0))),
            }
            Ok(())
        })?,
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
    g.set("flr", lua.create_function(|_, x: f64| Ok(num(x.floor())))?)?;
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
    g.set("sin", lua.create_function(|_, x: f64| Ok((x * TAU).sin()))?)?;
    g.set("cos", lua.create_function(|_, x: f64| Ok((x * TAU).cos()))?)?;

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

    // `master(drive, [tone], [hiss])`: the cart-global output stage. Omitted
    // arguments are 0, so `master(0)` is "clean" and overrides whatever the
    // cart's `__instruments__` master line asked for.
    let st = state.clone();
    g.set(
        "master",
        lua.create_function(
            move |_, (drive, tone, hiss): (f64, Option<f64>, Option<f64>)| {
                st.borrow_mut()
                    .audio
                    .lua_master(fl(drive), tone.map_or(0, fl), hiss.map_or(0, fl))
                    .map_err(mlua::Error::RuntimeError)
            },
        )?,
    )?;

    // `echo(delay, [feedback], [level])`: the cart-global delay bus, mirroring
    // the `__instruments__` echo line. Omitted arguments are 0, so `echo(0)`
    // (or `echo(-1)`) switches the bus off and flushes the delay line.
    let st = state.clone();
    g.set(
        "echo",
        lua.create_function(
            move |_, (delay, feedback, level): (f64, Option<f64>, Option<f64>)| {
                st.borrow_mut()
                    .audio
                    .lua_echo(fl(delay), feedback.map_or(0, fl), level.map_or(0, fl))
                    .map_err(mlua::Error::RuntimeError)
            },
        )?,
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

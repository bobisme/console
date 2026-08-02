# Fantasy Console Spec (PoC v0)

A PICO-8-style fantasy console for vertical phone play, designed so AI agents are
first-class game developers: deterministic core, headless harness, text-native carts,
single-file HTML deployment.

## Architecture

Cargo workspace:

- `crates/console-core` — pure library. Lua 5.4 VM (mlua, vendored), software
  framebuffer, input, fixed timestep. **No windowing, no GPU, no audio device, no
  wall-clock, no filesystem access from Lua.** Compiles for native AND
  `wasm32-unknown-emscripten`.
- `crates/console-agent` — native binary. Headless harness for AI agents:
  oneshot CLI + JSON-RPC over stdio. PNG screenshots.
- `crates/console-web` — emscripten cdylib/staticlib exposing a C ABI over the core.
- `crates/console-pack` — native binary. Splices engine JS + cart into a single
  self-contained `game.html`.
- `web/` — HTML template, JS shell, touch controls (all inlined at pack time).
- `carts/` — example carts.

## Display

- **144×256 logical pixels** (9:16 portrait), fixed. Letterbox/integer-scale to fit.
- **60 fps fixed timestep.** Each frame: `_update()` then `_draw()`.
- Framebuffer: 144*256 bytes, one palette index (0–15) per pixel, row-major.

## Palette (fixed 16 colors — Sweetie-16)

| idx | hex     | | idx | hex     |
|-----|---------|-|-----|---------|
| 0   | #1a1c2c | | 8   | #29366f |
| 1   | #5d275d | | 9   | #3b5dc9 |
| 2   | #b13e53 | | 10  | #41a6f6 |
| 3   | #ef7d57 | | 11  | #73eff7 |
| 4   | #ffcd75 | | 12  | #f4f4f4 |
| 5   | #a7f070 | | 13  | #94b0c2 |
| 6   | #38b764 | | 14  | #566c86 |
| 7   | #257179 | | 15  | #333c57 |

Color 0 is the default clear color and the default transparent color in `spr()`.

## Input

6 buttons, one bit each in an input bitmask (u8):
bit 0 = left, 1 = right, 2 = up, 3 = down, 4 = A, 5 = B.

Keyboard: arrows/WASD for d-pad, Z/J = A, X/K = B. Letter form for CLI/RPC input
specs: `L R U D A B` (e.g. `"RA"` = right + A).

## Lua environment

- Lua 5.4 via mlua (`lua54` + `vendored`).
- Cart defines optional globals `_init()`, `_update()`, `_draw()`.
- Sandbox: remove `io`, `os`, `debug`, `package`/`require`, `dofile`, `loadfile`,
  `print` (replaced — see below). Keep `math`, `string`, `table`, `pairs`, etc.
  `math.random`/`math.randomseed` are replaced by the deterministic `rnd`/`srand`
  (calling them raises an error pointing at `rnd`).
- Lua runtime errors halt the cart; the harness/web shell surfaces message + traceback.

### API v0

| fn | behavior |
|----|----------|
| `cls([c=0])` | clear screen to color c |
| `pset(x, y, c)` / `pget(x, y)` | set/get pixel (out of bounds: pset no-op, pget → 0) |
| `line(x0, y0, x1, y1, c)` | Bresenham line |
| `rect(x0, y0, x1, y1, c)` / `rectfill(...)` | outline / filled rectangle (inclusive coords) |
| `circ(x, y, r, c)` / `circfill(...)` | midpoint circle outline / filled |
| `spr(n, x, y, [w=1], [h=1], [flip_x=false], [flip_y=false])` | draw sprite n (w×h sprites); color 0 transparent |
| `print(s, x, y, [c=12])` | draw text with built-in 4×6 font (ASCII 32–126; lowercase may render as uppercase) |
| `btn(i)` / `btnp(i)` | button held / just-pressed this frame |
| `rnd([n=1])` | deterministic float in [0, n) — PCG32 or xoshiro seeded PRNG in Rust |
| `srand(seed)` | reseed PRNG (reset seeds it to 0 unless overridden) |
| `t()` | seconds since cart start = frame_count / 60 (exact, from frame counter) |
| `flr(x)`, `ceil(x)`, `abs(x)`, `min/max/mid(...)`, `sin(x)`, `cos(x)` | conveniences; sin/cos take **turns** (PICO-8 style: `sin(0.25) = -1`... actually use standard sign: `sin(t)` = `math.sin(t*2π)`, PICO-8 inverts — we do NOT invert) |
| `printh(s)` | log line to host (harness `logs`, browser console). Never draws. |

All draw coordinates are floats, truncated toward negative infinity (`flr`) before use.

## Cart format (`.cart`, UTF-8 text)

Sections start with a `__name__` line. `__lua__` is required; others optional.

```
__meta__
title=Demo
author=someone
version=0

__lua__
-- Lua 5.4 source until next section header

__sprites__
128 lines × 128 hex chars (0-f), one char per pixel.
Sprite sheet is 128×128 px = 16×16 sprites of 8×8 px.
Sprite n occupies pixels (n%16*8, n//16*8)..+8. Missing/short section = all zeros.
```

## Determinism contract

Same cart + same seed + same per-frame input masks ⇒ byte-identical framebuffers
on every platform and target. No wall clock, no OS entropy, no float
non-determinism (avoid trig on the Rust side of the render path; Lua floats are
IEEE 754 doubles everywhere and are fine).

Save states = (cart hash, seed, input log); loading = reset + replay. Replays double
as regression tests: `(cart, input log) → expected framebuffer hash`.

## Agent harness (`console-agent`)

Oneshot: `console-agent run <cart> [--frames N] [--input SPEC] [--screenshot out.png]
[--screen-text] [--eval CODE] [--seed N]`
where SPEC is comma-separated `COUNT:BUTTONS`, e.g. `30:,10:R,5:RA,60:` (empty
buttons = no input).

Serve: `console-agent serve` — JSON-RPC 2.0, one request per line on stdin,
one response per line on stdout. Methods:

- `load_cart {path}` or `{text}` — load + `_init`
- `reset {seed?}` — reload cart state, reseed, clear input log
- `step {frames=1, input=""}` — advance; input as letter string or int mask
- `screenshot {path}` — write PNG (RGBA, 1:1 scale)
- `screen_text {}` — framebuffer as 256 lines of 144 hex chars
- `eval {code}` — run Lua, return result serialized to JSON (tables best-effort, depth-limited)
- `get_global {name}` — shorthand for eval returning that global
- `logs {}` — drain `printh` output
- `save_state {name}` / `load_state {name}` — replay-based
- `info {}` — frame count, cart meta, seed, input log length

Errors (bad cart, Lua error) come back as JSON-RPC errors with the Lua traceback in
`data`, and the console stays alive.

## Single-file HTML (`console-pack`)

`console-pack <cart> -o game.html [--engine <path to engine.js>]`

- Engine = emscripten `-sSINGLE_FILE=1 -sMODULARIZE=1` build of `console-web`
  (wasm base64-inlined into JS). Built rarely; committed to `web/engine.js` or
  rebuilt via script.
- `game.html` = template + inline engine JS + cart text in
  `<script type="text/cart">…</script>`. **Zero external requests** — must work
  from `file://`. The cart stays human/agent-editable inside the HTML.
- Shell: a handheld **device chassis**, not a full-screen overlay — the screen
  sits in a dark bezel at the top of a light device body, with an opaque
  control deck below: d-pad cross (left), small MENU pill (center), offset
  A/B buttons (right, Game Boy style). Screen integer-scales when possible;
  the deck keeps a finger-friendly height (~20% of viewport, clamped
  112–150px); the whole device centers in larger windows. Multi-touch with
  8-way d-pad angle detection; keyboard input; Escape or MENU opens a pause
  menu (RESUME / RESET) confined to the screen area — game logic does not
  step while paused, and no catch-up burst fires on resume. rAF loop with
  fixed-step accumulator (max 4 catch-up steps).
- C ABI (console-web): `con_init(cart_ptr, cart_len) -> i32` (0 ok),
  `con_step(input_mask)`, `con_fb() -> *const u8` (144*256 palette indices),
  `con_palette() -> *const u8` (16×3 RGB), `con_error() -> *const u8` (NUL-terminated
  UTF-8 or NULL), `con_alloc(len) -> *mut u8` / `con_free(ptr, len)` for the cart copy.

## Out of scope for PoC

Audio/music, map section, camera, palette remap, multiple carts,
save data, sprite editor. Design must not preclude them. (A minimal pause
menu — RESUME/RESET — already exists in the web shell.)

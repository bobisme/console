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

7 buttons, one bit each in an input bitmask (u8):
bit 0 = left, 1 = right, 2 = up, 3 = down, 4 = A, 5 = B,
6 = **menu** (start/select-style game input — carts read it like any button,
e.g. to open an in-game menu; distinct from the web shell's device menu).

Keyboard: arrows/WASD for d-pad, Z/J = A, X/K = B, Enter = menu. Letter form
for CLI/RPC input specs: `L R U D A B M` (e.g. `"RA"` = right + A).

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
| `sspr(sx, sy, sw, sh, dx, dy, [dw=sw], [dh=sh], [flip_x=false], [flip_y=false])` | blit the sheet rect (sx, sy, sw, sh) into the screen rect (dx, dy, dw, dh), nearest-neighbor scaled; same camera/clip/`pal`/`palt` rules as `spr`. Any size ≤ 0 draws nothing (negative sizes do **not** mirror) |
| `map([cel_x=0], [cel_y=0], [sx=0], [sy=0], [cel_w=128], [cel_h=64])` | draw a cel_w×cel_h block of map cells from cell (cel_x, cel_y) to (sx, sy); **tile 0 is skipped**. `map()` draws the whole map at 0,0 |
| `mget(cx, cy)` | tile id at map cell (cx, cy); off the map reads 0 |
| `mset(cx, cy, [v=0])` | write a tile id (0–255, masked); off the map is a no-op |
| `print(s, x, y, [c=12])` | draw text with built-in 4×6 font (ASCII 32–126; lowercase may render as uppercase) |
| `camera([x=0], [y=0])` | draw offset subtracted from all later draw coords; no args resets |
| `clip([x, y, w, h])` | clip rectangle in **screen** space; no args resets to full screen |
| `pal([c0], [c1], [p=0])` | p=0 draw-palette remap (rewrites pixels), p=1 display-palette remap (scanout only); no args resets both maps **and** `palt` |
| `palt([c], [flag])` | mark color c transparent in `spr()`; no args resets to "only color 0" |
| `fillp([p=0])` | 4×4 dither pattern for the **shape** primitives; 16-bit, bit 15 = top-left, row-major. Clear bit = the color's low nibble, set bit = its high nibble (or nothing when that nibble is 0). No args (or 0) = solid |
| `mosaic([f=1])` | end-of-frame pixelation: every f×f block of the finished frame becomes its top-left pixel. f clamped to 1–32; 1 (or no args) = off |
| `rshift([y], [dx=0])` | end-of-frame per-scanline horizontal shift: scanline y (0–255) is displaced dx pixels, positive = right, **wrapping** around the 144-wide line. dx is reduced mod 144 (so −1 = 143); y off screen is a no-op. Write-only: `rshift()` clears every line, `rshift(y)` clears line y |
| `btn(i)` / `btnp(i)` | button held / just-pressed this frame |
| `rnd([n=1])` | deterministic float in [0, n) — PCG32 or xoshiro seeded PRNG in Rust |
| `srand(seed)` | reseed PRNG (reset seeds it to 0 unless overridden) |
| `t()` | seconds since cart start = frame_count / 60 (exact, from frame counter) |
| `flr(x)`, `ceil(x)`, `abs(x)`, `min/max/mid(...)`, `sin(x)`, `cos(x)` | conveniences; sin/cos take **turns** (PICO-8 style: `sin(0.25) = -1`... actually use standard sign: `sin(t)` = `math.sin(t*2π)`, PICO-8 inverts — we do NOT invert) |
| `printh(s)` | log line to host (harness `logs`, browser console). Never draws. |

All draw coordinates are floats, truncated toward negative infinity (`flr`) before use.

### Draw state

`camera`, `clip`, `pal`, `palt`, `fillp`, `mosaic` and `rshift` form one block
of **persistent** draw state.
It is **never auto-reset at a frame boundary** (PICO-8 semantics): a cart that
calls `camera(0, -8)` once keeps that offset until it changes it. Only a cart
call — or a fresh console / `reset` — moves it. Every default is a no-op, so
carts that ignore all of this render exactly as before.

- **camera(x, y)** — an integer offset subtracted from the coordinates of
  every subsequent drawing op (`pset`, `line`, `rect`, `rectfill`, `circ`,
  `circfill`, `spr`, `map`, `print`). `pget` reads **screen** space and is
  unaffected; `cls` covers the whole screen and is unaffected.
- **clip(x, y, w, h)** — an inclusive rectangle in **screen** space, applied
  *after* the camera offset, clamped to the screen. `w` or `h` ≤ 0 (or a rect
  entirely off screen) yields an empty clip that draws nothing. **`cls`
  respects the clip** — it clears the clip window, not the screen, which makes
  windowed/split-screen effects work without a manual `rectfill`. This is the
  modern-PICO-8 behavior, not the pre-0.2 one.
- **pal(c0, c1, 0)** — *draw* palette. Colors are translated `c0 → c1` as
  pixels are written, so the framebuffer really holds the new index. Applies to
  shape colors, `print` and `spr` pixels. It does **not** compose: each draw
  does exactly one lookup. `cls` is exempt (it writes its color literally).
- **pal(c0, c1, 1)** — *display* palette: a 16-entry index → index map applied
  by the **host at scanout**, never to the framebuffer. This is what makes
  whole-screen fades and flashes free — 16 calls, zero redraw — while
  framebuffer goldens and `screen_text` stay in draw space and never move.
- **palt(c, flag)** — which colors `spr()` treats as transparent (default: only
  color 0). Transparency is tested on the sprite's **source** color, *before*
  the draw palette remaps it, so `pal(1, 0)` draws color-1 pixels as color 0
  rather than making them vanish.
- **fillp(p)** — a 4×4 dither pattern applied to the **shape** primitives:
  `pset`, `line`, `rect`, `rectfill`, `circ`, `circfill`. `p` is a 16-bit
  number; bit 15 is the top-left cell and the rows read left to right, top to
  bottom (`fillp(0x5a5a)` is a checkerboard, `fillp(0x8000)` punches one hole
  per 4×4 cell). `fillp()` or `fillp(0)` is solid again; the argument is masked
  to 16 bits.
  - **Two colors.** Shape color arguments are `c0 + c1*16`: a *clear* pattern
    bit draws the low nibble `c0`, a *set* bit draws the high nibble `c1`. When
    the high nibble is 0 — i.e. an ordinary `0–15` color — a set bit draws
    **nothing at all** and the framebuffer keeps what was underneath, which is
    what makes `fillp` a transparency stencil. (Consequence: "secondary =
    color 0, opaque" has no encoding. Draw it as `c1 = 0` primary with the
    pattern inverted.) Both nibbles go through the draw palette. With the
    default solid pattern the high nibble is never read, so an old cart that
    passes a color ≥ 16 gets exactly the color it always got.
  - **Screen-anchored.** The grid is `(x % 4, y % 4)` in **screen** space,
    after the camera. A shape that scrolls under a moving camera therefore
    shimmers through the pattern — that is the classic look, not a bug.
  - `pal()` does **not** reset `fillp` (nor does anything else): the palette
    maps, `palt` and the fill pattern are separate state, as in PICO-8. `cls`,
    `spr`, `sspr`, `map` and `print` all ignore the pattern entirely.
- **mosaic(f)** and **rshift(y, dx)** — see [Frame pipeline](#frame-pipeline)
  below. They are draw state in that they persist and live beside the rest, but
  they are applied once per frame rather than per drawing op. The 256-entry
  shift table is console state like the tile map: it survives across frames and
  a replay of the same inputs reproduces it exactly.

`sspr()` shares the sprite pixel path with `spr()`: the camera offsets the
destination rectangle, the clip rect bounds it, `palt` decides transparency on
the source color and the draw palette remaps what lands in the framebuffer.
Sampling is nearest-neighbor with the source stepped in u32.16 fixed point
(`step = (sw << 16) / dw`), so the pixel loop is integer-only and
bit-identical on every target. At `dw == sw, dh == sh` the step is exactly 1.0
and the result is byte-identical to the equivalent `spr()`, flips included.
Source pixels outside the 128×128 sheet are skipped, never wrapped.

### Frame pipeline

Each frame: `_update()`, `_draw()`, then **`mosaic`**, then **`rshift`**, then
the frame is presented (screenshot, `screen_text`, web canvas) with the
**display palette** applied by the host at scanout. The order is fixed and
load-bearing: mosaic pixelates the finished frame, rshift then displaces the
already-pixelated scanlines, so a shift can never slice a mosaic block into
partial halves (and water-over-mosaic reads the way the hardware effect did).
The post-effects sit on opposite sides of the ground truth from the display
palette:

- **mosaic(f)** rewrites the pixels. Every f×f block of the finished frame is
  replaced by its **top-left** pixel (top-left, not an average: the framebuffer
  holds palette indices, and averaging indices is meaningless). Blocks are
  anchored at screen (0, 0) and ignore the camera and the clip rect; a factor
  that does not divide 144 or 256 leaves a narrower block at the right/bottom
  edge. `f` is clamped to 1–32, and `mosaic()`/`mosaic(1)` turns it off.
  Because it is a framebuffer effect it **is** in `screen_text`, in PNG
  screenshots and in framebuffer goldens — that is the point: two lines of Lua
  (`mosaic(f)` ramping up, then down) give the classic pixelate-in/out scene
  transition, and it is as deterministic as everything else.
  The effect is computed from the pristine draw buffer every frame and never
  fed back: `pget` and the next frame's drawing still see full-resolution
  pixels, so a mosaicked screen does not compound frame after frame.
- **rshift(y, dx)** displaces one scanline of the finished frame horizontally —
  the HDMA raster trick, and the cheapest parallax/water/heat-haze/wobble on
  the machine. Scanline `y` (0–255) moves `dx` pixels; **positive `dx` moves
  content right**, and the line **wraps**: the pixel at column `x` lands at
  `(x + dx) mod 144`, so whatever leaves one edge arrives at the other. Wrap,
  not clip — that is what makes a sine sweep seamless instead of torn.
  - **Exact `dx` rule**: `dx` is reduced with a Euclidean remainder mod 144 and
    stored as `dx mod 144` in 0–143. Every argument is therefore legal and
    `dx`, `dx ± 144`, `dx ± 288`… are the same shift; `-1` is stored as 143 (a
    one-pixel left shift *is* a 143-pixel right shift on a wrapping line), and
    `rshift(y, 144)` stores 0, i.e. the identity. Fractional `dx` is floored
    like every other coordinate. `y` outside 0–255 is a no-op, like a `pset`
    off the screen.
  - **Write-only API.** `rshift()` with no arguments clears the whole table to
    0; `rshift(y)` is `rshift(y, 0)` and clears one line. There is no getter —
    carts recompute the sweep each frame
    (`for y=0,255 do rshift(y, 3*sin(t()+y/32)) end`), and that loop of 256
    calls allocates nothing and touches no tables.
  - Like mosaic it is a **framebuffer** effect, so it is in `screen_text`, in
    PNG screenshots and in framebuffer goldens; it is computed from the
    pristine draw buffer every frame, so `pget` and the next frame's drawing
    are unaffected and a shifted screen never compounds. An all-zero table (the
    default) skips the pass entirely, so a cart that never calls `rshift`
    presents a byte-identical framebuffer to one from before it existed.
- **pal(c0, c1, 1)** never touches the framebuffer at all (see above).

`map()` is defined as a sequence of `spr()` calls and shares their pixel path
exactly: the camera offsets its destination, the clip rect bounds it, `palt`
decides per-pixel transparency on the source color and the draw palette remaps
what lands in the framebuffer. Only the per-**cell** tile-0 skip is map-specific
— it happens before any pixel is read, so an empty cell is invisible even under
`palt(0, false)`.

Host surfaces: `Console::display_palette() -> &[u8; 16]` (identity by default)
and `Console::draw_state()`. `console-agent` applies the display palette when
it renders PNG screenshots; `screen_text` stays raw. The web shell composes
`palette[dpal[idx]]`. `DrawState::fillp()`, `DrawState::mosaic()` and
`DrawState::rshift(y)` / `rshift_table()` / `rshift_active()` expose the new
fields; hosts need no code for any of them, because both post-effects are
already folded into `Console::framebuffer()`.

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

__map__
Up to 64 lines × up to 128 cells, **2 hex chars per cell** = a tile id 00-ff
indexing the sprite sheet. Map is a fixed 128×64 cells of 8×8 px.
```

`__map__` follows the same hex-grid conventions as `__sprites__`: `#` starts a
comment line, blank lines and comments do not consume a row, and rows shorter
than 128 cells pad with tile 0 (missing rows are all tile 0). Unlike
`__sprites__` it **rejects** rather than truncates: a row longer than 128 cells,
a row with an odd number of digits, more than 64 rows, or a non-hex digit is an
`Error::Cart` naming the section-relative line — losing terrain silently is much
worse than failing to load.

**Tile 0 is the empty cell.** `map()` skips those cells entirely rather than
drawing sprite 0, which is the same convention that reserves sprite 0 as blank
(see "Sprite & animation authoring"). A cart with no `__map__` gets an all-zero
map, so `map()` draws nothing and `mget` reads 0 — carts written before the map
existed are byte-identical.

The map is **runtime-mutable**: `mset` writes to the console's live copy (the
parsed cart keeps the authored one), mutations persist across frames like any
other console state, and because they are driven only by cart code they replay
deterministically from `(cart, seed, input log)` with no special handling.

## Determinism contract

Same cart + same seed + same per-frame input masks ⇒ byte-identical framebuffers
on every platform and target. No wall clock, no OS entropy, no float
non-determinism (avoid trig on the Rust side of the render path; Lua floats are
IEEE 754 doubles everywhere and are fine).

Save states = (cart hash, seed, input log); loading = reset + replay. Replays double
as regression tests: `(cart, input log) → expected framebuffer hash`.

## Agent harness (`console-agent`)

Oneshot: `console-agent run <cart> [--frames N] [--input SPEC] [--screenshot out.png]
[--screenshot-zoom N] [--screen-text] [--eval CODE] [--seed N]`
where SPEC is comma-separated `COUNT:BUTTONS`, e.g. `30:,10:R,5:RA,60:` (empty
buttons = no input).

Serve: `console-agent serve` — JSON-RPC 2.0, one request per line on stdin,
one response per line on stdout. Methods:

- `load_cart {path}` or `{text}` — load + `_init`
- `reset {seed?}` — reload cart state, reseed, clear input log
- `step {frames=1, input=""}` — advance; input as letter string or int mask
- `screenshot {path, zoom=1}` — write PNG (RGBA), nearest-neighbor
  integer-upscaled by `zoom`
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
  control deck below: d-pad cross (left), a center cluster of a **triangle
  game-menu button** (input bit 6, Enter on keyboard) above the small
  device-MENU pill, and offset A/B buttons (right, Game Boy style). Screen scaling: **FIT** (default)
  fills the viewport fractionally; the pause menu has a **PIXELS: FIT/SHARP**
  toggle for integer-scaled crispness (localStorage `con-pp`). The deck
  keeps a finger-friendly height (~19% of viewport, clamped 108–150px); the
  device centers in larger windows. Multi-touch with 8-way d-pad angle
  detection; keyboard input; Escape or MENU opens the pause menu
  (RESUME / RESET / PIXELS / a **VOL slider** — perceptual `vol²` curve into
  a master GainNode, persisted as localStorage `con-vol`, default 60%)
  confined to the screen area — game logic does not step while paused, no
  catch-up burst on resume.
  rAF loop with fixed-step accumulator (max 4 catch-up steps). Audio output
  chain: AudioWorklet loaded from a `data:` module URL first on `file://`
  pages (null origin — some browsers refuse `blob:null` module loads) and a
  Blob URL otherwise, falling back to a ScriptProcessorNode; ~50ms silent
  prebuffer; `window.__console.audioState()` exposes
  mode/errors/framesPushed/volume for headless verification.
- C ABI (console-web): `con_init(cart_ptr, cart_len) -> i32` (0 ok),
  `con_step(input_mask)`, `con_fb() -> *const u8` (144*256 palette indices),
  `con_palette() -> *const u8` (16×3 RGB),
  `con_dpal() -> *const u8` (16 bytes: the display palette, index → index;
  identity unless the cart called `pal(c0, c1, 1)` — the shell composes
  `palette[dpal[idx]]`), `con_error() -> *const u8` (NUL-terminated
  UTF-8 or NULL), `con_alloc(len) -> *mut u8` / `con_free(ptr, len)` for the cart copy.

## Audio (PoC v1)

Principles: **deterministic** (const note table + linear ops + LFSR only — no
`powf`/`sin` at runtime, so native and wasm render bit-identical f32 samples),
**text-native** tracker format, and audio never feeds back into game logic
(framebuffer determinism is independent of audio).

- 44100 Hz, mono f32, exactly **735 samples per frame** (44100/60). The synth
  advances inside `step()`; a halted console renders silence.
- **6 channels**, summed with **0.25 gain each** (frozen at the four-channel
  value, so carts written for 4 channels render bit-identical samples), output
  clamped to [-1, 1]. Headroom is therefore authored, not enforced: four
  full-scale voices in phase reach exactly 1.0, six reach 1.5 and the final
  clamp hard-clips. Any non-zero `master drive` bounds the output below full
  scale (see the master bus) and acts as a free limiter.
- Waveforms: 0 = pulse 12.5%, 1 = pulse 25%, 2 = square 50%, 3 = triangle,
  4 = saw, 5 = noise (16-bit LFSR, NES-style taps, clocked from the channel
  frequency). Id 6 is the 2-op FM oscillator and ids 8–15 are the cart's own
  wavetables (both PoC v2, below); id 7 stays reserved.
- Notes `C0`–`B7` (A4 = 440). Frequencies come from a `const` table of 96 f32
  literals baked into the source (generated once, committed) — never computed
  at runtime.
- Volume 0–7, linear (`vol/7`). On any change of a channel's frequency,
  waveform, or volume, amplitude ramps linearly over 64 samples to avoid
  clicks (deterministic).
- Channels are continuous-phase: a new row sets freq/wave/vol without
  resetting phase (legato); rests just set volume 0.

### `__sfx__` section

```
sfx <id 0-63> speed=<frames-per-row 1-255> [loop=<start-row>,<end-row>]
C#4 2 7        <- row: NOTE WAVE VOL
---            <- rest (silence this row)
```

Up to 32 rows per sfx. `loop` jumps from end-row back to start-row while the
sfx keeps playing (looped sfx play until the channel is stopped or stolen).

### `__music__` section

```
pat <id 0-63> [stop|loop=<pat-id>] : ch0 ch1 ch2 ch3 [ch4 ch5]
```

Each slot is `<sfx id>` or `-` (silent channel). **4 to 6 slots**: slot *n* is
channel *n*, and trailing slots a line omits are silent, so every pre-6-channel
4-slot pattern parses exactly as before. Pattern duration = one pass
of `max(rows*speed)` over its sfx (sfx `loop` flags are ignored under music).
When a pattern ends: `loop=<id>` jumps there, `stop` halts music, otherwise
play the next existing pattern id, else halt.

### Lua API additions

| fn | behavior |
|----|----------|
| `sfx(n, [ch=-1])` | play sfx n; `ch` is 0–5, or −1 to auto-pick the lowest channel not busy with music or sfx, stealing channel **5** if all are busy. `sfx(-1, ch)` stops that channel; `sfx(-1)` stops every sfx channel. |
| `music(n)` | start music at pattern n (claims that pattern's non-`-` channels; re-claims per pattern). `music(-1)` stops music. |
| `master(drive, [tone], [hiss])` | override the cart's `master` line (omitted args = 0; `master(0)` = clean) |
| `echo(delay, [feedback], [level])` | override the cart's `echo` line (omitted args = 0; `echo(0)` / `echo(-1)` = off, and flushes the delay line) |

**Channel budget (best practice).** Music owns the channels its pattern names
and `sfx()` auto-allocation prefers the ones it does not, so a song may use all
six voices but **should leave one or two free**: a 4-slot song leaves channels 4
and 5 genuinely free for blips, whereas a 6-slot song forces the next
auto-allocated `sfx()` to steal channel 5 out from under the music (the old
4-channel behavior, where every song lost channel 3).

### Host surfaces

- console-core: `audio_frame(&self) -> &[f32; 735]` — the samples rendered by
  the most recent `step()`.
- console-agent — agents can't hear, so audio is inspectable in three layers
  (the session keeps an audio log + note-event log alongside the input log;
  replay-based `load_state` reproduces both):
  1. **Ground truth as data**: `audio_state {}` (per-channel: sfx, row,
     resolved note name, wave, vol, music ownership; current music pattern)
     and `audio_events {from_frame?}` (note_on / row_change / note_off /
     pattern_change log with frame numbers).
  2. **Signal stats**: `audio_stats {window_frames?=6}` — per-window RMS,
     peak, clipped-sample count over the rendered mix.
  3. **Vision**: `spectrogram {path, from_frame?, to_frame?, cell?=4}` — PNG
     heatmap on a semitone × time grid (96 rows = the console's own C0–B7
     note space, Goertzel per note bin, octave gridlines, 1s time ticks), so
     melodies read as note-block patterns.
  Plus `wav {path, from_frame?, to_frame?}` (16-bit PCM mono 44100, for
  humans and regression hashes; hand-rolled header, no new deps). Oneshot
  equivalents: `--wav`, `--spectrogram`, `--audio-events`, `--audio-stats`.
- console-web C ABI: `con_audio() -> *const f32` (latest frame's 735 samples,
  valid until the next `con_step`).
- Web shell: `AudioContext({sampleRate: 44100})` + an AudioWorklet created
  from a Blob URL (single-file safe). Created/resumed on the first user
  gesture (pointer or key). Main thread posts each frame's samples to the
  worklet (transferred); the worklet keeps a small ring (~8 frames) and plays
  silence on underrun. Pause menu open ⇒ no steps ⇒ ring drains to silence.

## Sprite & animation authoring (PoC v1)

Agents author pixels as hex text but can't see them, and they author
animation frames with no memory of neighboring frames' pixels. Tooling
follows the audio philosophy: ground truth as data, numeric lints, then
vision renders — plus mechanical transforms so agents never hand-shift hex.

### `__gfx_meta__` cart section (authoring metadata; runtime ignores it)

```
sprite <name> rect=<tx>,<ty> size=<w>x<h> [anchor=<px>,<py>]
anim <sprite>.<label> frames=<f0,f1,...> fps=<1-60> [loop] [frames_rect=<tx>,<ty>]
```

- `name`: `[a-z0-9_]+`, unique. `rect` in tile coords (0–15), `size` in
  tiles (1×1 up to 16×16). `anchor` in pixels relative to the sprite's
  top-left; default = bottom-center `(w*8/2, h*8-1)` (ground contact).
- Anim names are namespaced by sprite (`player.walk`). Each entry `f` in
  `frames=` is one of:
  - an index `i` — addresses the rect displaced `i` sprite-widths to the
    right of the frame-0 origin, wrapping to the next row band:
    `tx' = (tx + i*w) % 16`, `ty' = ty + ((tx + i*w) / 16) * h`. The origin
    `(tx, ty)` is the sprite's own `rect` by default.
  - an explicit tile coordinate `tx:ty` — the sprite's `WxH` rect anchored
    directly at tile `(tx, ty)` on the sheet, no wrap math, no relation to
    the sprite's `rect` or the anim's `frames_rect`. Lets a frame reuse any
    tile region, skip damaged/reserved tiles, or avoid the huge contiguous
    runs a megatile sprite's index addressing would otherwise demand.
  - a list may mix both forms freely, e.g. `frames=0,1,12:4,3`.
  - `frames_rect=<tx>,<ty>` (optional, anim-level) relocates the frame-0
    origin that INDEX entries in this anim count from: frame `i` resolves
    from `(tx, ty)` instead of the sprite's own `rect`, same
    displacement/wrap rule, same `WxH` as the sprite. Explicit `tx:ty`
    entries ignore it. This is what lets two anims of one sprite live in
    different, non-contiguous sheet regions.
  - Every resolved rect must fit the 16×16 tile sheet; back-compat: an anim
    using neither `frames_rect` nor any `tx:ty` entry parses and resolves
    exactly as before this grammar existed.
- Validation after the whole section parses (forward references fine);
  errors are `Error::Cart` with section-relative line numbers, naming the
  anim and the offending frame's position in its list. Section is optional;
  carts without it behave identically.
- Core API: `Cart::gfx_meta()` exposing sprite/anim defs plus
  `AnimDef::resolve_frame(sprite, pos) -> pixel rect`, the single place that
  composes the wrap-displacement rule with `frames_rect` relocation and
  explicit `tx:ty` frames — pixel data itself comes from the existing sprite
  sheet accessors. `pos` indexes the anim's own `frames` list (not a raw
  sheet frame index); `SpriteDef::frame_rect(i)` remains the classic
  sprite-rect-relative resolver used by non-anim frame addressing
  (`sprite dump`/`poke`/`edit`).

### Inspection tools (console-agent `sprite` subcommands + RPC verbs)

All operate on a cart file directly (no stepping). Renders default to
**zoom 8**, on a dark checkerboard (transparency = color 0 shows through),
optional `--grid` (tile boundaries), `--indices` (palette index glyph per
pixel cell), `--anchor` (crosshair, color 4). Targets: sprite name, anim
name, or raw rect `tx,ty,w,h`.

| command | output |
|---------|--------|
| `sprite render <cart> <target> [--frame N] -o out.png` | one frame, zoomed |
| `sprite strip <cart> <anim> -o` | all frames side by side, baselines aligned through the anchor |
| `sprite onion <cart> <anim> --frame N -o` | frame N full opacity; previous frame tinted red ~35%, next tinted green ~35% (loop-aware; color-0 pixels excluded from ghosts) |
| `sprite diff <cart> <anim> <frameA> <frameB> -o` | frame B dimmed ~35%; pixels that differ from frame A in bright magenta |
| `sprite ghost <cart> <anim> -o` | every frame overlaid at low alpha (motion accumulation) |
| `sprite lint <cart> [anim ...] [--max-drift PX] [--max-area-var PCT] [--max-changed PX] [--no-unique-colors] [--summary]` | JSON to stdout, per frame and per consecutive pair (loop-aware): changed-pixel count, silhouette area + % drift, centroid & bbox relative to anchor + per-frame drift, per-frame palette histogram, colors unique to a single frame. Each frame entry also carries `sprite_id`, the resolved sheet tile `[tx, ty]` that frame number lives at. Report-only with no thresholds (exit 0); agents do the asserting. |

`sprite lint`'s CI gate: any of `--max-drift <px>` (centroid-drift distance
between consecutive/wrap frames), `--max-area-var <pct>` (absolute
silhouette-area drift), `--max-changed <px>` (changed-pixel count between
consecutive/wrap frames), `--no-unique-colors` (any color that appears in
exactly one frame) turns the report into a gate: exit code 1 if anything
breaches its limit, plus a top-level `violations` array, one entry per
breach — `{"anim", "frame", "metric", "value", "limit"}` (`frame` is the
pair's later frame, or the frame a unique color lives in). No thresholds
given ⇒ unchanged report-only behavior (exit 0, no `violations` key).
`--summary` prints one line per anim (name, frame count, worst drift, worst
changed-pixel count, unique-color count) instead of the full JSON; it
combines with the threshold flags, which still gate the exit code.

RPC mirrors: `sprite_render`, `sprite_strip`, `sprite_onion`, `sprite_diff`,
`sprite_ghost`, `sprite_lint` against the session's loaded cart.
`sprite_lint` mirrors the CLI's threshold params (`max_drift`,
`max_area_var`, `max_changed`, `no_unique_colors`) and `summary`; since
JSON-RPC has no process exit code, it reports a `"violated"` boolean
instead.

### Transforms (console-agent `sprite edit` — CLI only, rewrites the cart file)

```
sprite edit <cart> shift <target> [--frame N] --dx <n> --dy <n> [--wrap]
sprite edit <cart> flip  <target> [--frame N] --horizontal|--vertical
sprite edit <cart> rotate <target> [--frame N] --cw|--ccw   (square only)
sprite edit <cart> copy  <sprite:frame|rect> <sprite:frame|rect>  (equal sizes)
sprite edit <cart> clear <target> [--frame N]
```

Vacated pixels fill with color 0 unless `--wrap`. Edits rewrite ONLY the
changed lines of `__sprites__`; every other byte of the cart file survives
verbatim (text carts must stay git-diff friendly). `--dry-run` prints the
would-be new hex lines instead of writing.

## Tile map agent tooling (PoC v1)

Agents author the `__map__` grid as hex text with no way to see the 128x64
cells it assembles into. Tooling mirrors the sprite tools' shape one level
up — cells instead of pixels, tile ids instead of palette indices — so the
same ground-truth/numeric-lint/render progression and the same atomic,
only-touch-changed-lines rewrite apply here too.

### Inspection tools (console-agent `map` subcommands + RPC verbs)

All operate on a cart file directly (no stepping). A `[cx,cy,cw,ch]` region
argument (cell coordinates) is optional on `render`/`dump`/`poke`, defaulting
to the **used extent** — the bounding box of non-zero cells, or a single
cell at the origin if the map is entirely empty. `render` reuses the sprite
tools' pixel path exactly: zoom defaults to 8, tile 0 shows the same dark
checkerboard sprite renders use for transparency (and, within a non-empty
tile, its own color-0 pixels are individually transparent too — only the
whole-cell tile-0 skip is map-specific), `--grid` overlays cell (= tile)
boundaries, `--ids` labels every non-empty cell with its tile id using the
sprite tools' hex-glyph font.

| command | output |
|---------|--------|
| `map render <cart> [cx,cy,cw,ch] [--zoom Z] [--grid] [--ids] -o out.png` | the region, zoomed, exactly as `map()` would draw it |
| `map dump <cart> [cx,cy,cw,ch]` | the region as hex rows (2 chars/cell), `#`-header naming the coordinates, mirroring `sprite dump` |
| `map lint <cart>` | JSON over the whole map: used extent, cell counts by tile id (top N), tile ids referenced whose sprite-sheet region is entirely blank (the map analog of "color unique to one frame" — usually a typo), and % fill. Report-only; agents do the asserting. |

RPC mirrors: `map_render`, `map_dump`, `map_lint` against the session's
loaded cart — read-only, like the `sprite_*` mirrors: there is no
`map_poke`/`map_edit` RPC verb, since mutating a cart file is a CLI-only
operation by design.

### Transforms (console-agent `map poke`/`map edit` — CLI only, rewrites the cart file)

```
map poke <cart> [cx,cy,cw,ch] (--rows <hex,hex,...> | --stdin) [--dry-run]
map edit <cart> copy  <cx,cy,cw,ch> <dest_cx,dest_cy>       [--dry-run]
map edit <cart> shift <cx,cy,cw,ch> [--dx <n>] [--dy <n>]   [--dry-run]
map edit <cart> fill  <cx,cy,cw,ch> <tile-hex>              [--dry-run]
map edit <cart> clear <cx,cy,cw,ch>                         [--dry-run]
```

`poke` writes rows back through the same region convention `dump` reads them
with, so `map dump | map poke --stdin` round-trips as a no-op (`--stdin`
skips `#`-prefixed lines, same as `sprite poke`). `map edit`'s region is
always explicit — unlike `render`/`dump`/`poke` it never defaults to the
used extent, since a region transform is destructive by nature. `shift`
drops cells that fall outside the region (no wrap) and fills vacated cells
with tile 0; `fill`/`clear` take a tile id in the `__map__` alphabet
directly (1-2 hex digits, `00`-`ff`).

Both rewrite ONLY the changed lines of `__map__`, exactly like `sprite
edit`/`sprite poke`: a changed row is re-encoded at the full 128-cell width,
and every other byte of the cart file — other sections, comments, ordering,
line endings — survives verbatim. If the cart has no `__map__` section yet,
`poke`/`edit` create one, inserted right after the cart's `__sprites__`
section (or right before `__gfx_meta__` if there is no `__sprites__`, or at
EOF if there is neither) — `__map__`'s slot in the cart anatomy above.
`--dry-run` prints the would-be new hex lines instead of writing.

## Music authoring (PoC v2)

The synth's expressiveness is the ceiling on music quality, so phase 1 is
engine vocabulary; inspection/transform tooling (phase 2/3) mirrors the
sprite tools. Determinism contract unchanged: no transcendentals at runtime —
pitch offsets resolve through NOTE_FREQ with **linear interpolation between
adjacent semitones**, vibrato applies linear frequency scaling with a const
cents factor, LFOs are integer-phase triangles.

### `__instruments__` section (phase 1)

```
inst <name> wave=<0-6|w0-w7> [fm=<ratio>,<index>[,<decay>]] [env=<attack>,<decay>,<sustain>] [vib=<cents>,<rate>,<delay>] [sweep=<semis>,<frames>] [echo=<0-8>]
```

- `name` `[a-z0-9_]+`, unique, must not shadow the bare wave digits 0–6 nor the
  `w<digits>` spelling that names a wavetable slot.
- `env`: attack frames (vol ramps 0→row vol), decay frames (then decays
  toward sustain), sustain level 0–7 held until the row/note changes.
  Default: flat at row volume.
- `vib`: depth in cents (1–100), rate as an integer LFO period divisor
  (1–16, higher = faster), delay frames before onset. Triangle LFO on pitch.
- `sweep`: signed semitone offset traversed over N frames from note-on
  (drum sweeps: `sweep=-12,6` = kick).
- Sfx rows may name an instrument in place of the wave digit
  (`A4 lead 5`); a bare digit means "flat instrument with that wave"
  (today's behavior, still valid — old carts unchanged). Bare digits stop at
  **5**: waveform 6 is FM and a digit cannot carry its parameters, so a row
  that says `6` is a parse error naming the `fm=` syntax.
- Percussion is just instruments: `inst kick wave=3 sweep=-14,5 env=0,6,0`,
  triggered by an ordinary note row giving the sweep's start pitch.

### Wavetables (phase 1.75)

```
wavetable <slot 0-7> <32 hex nibbles>   # in __instruments__, one line per slot
inst <name> wave=w<slot> ...            # …and any instrument may play it
A4 w3 6                                 # …as may a sfx row, like a wave digit
```

Eight slots of a custom **single-cycle waveform, 32 samples × 4 bits** — the
classic wavetable-chip format (Game Boy wave RAM, VRC6, N163). Off by default
in the strongest sense: no cart written before this can produce a waveform id
above 5, so a cart with no `wavetable` line renders bit-identical samples.

- **Slots and ids.** `w0`–`w7`. Internally a wavetable is just another waveform
  id, `8 + slot` (id 6 is the 2-op FM oscillator and id 7 stays reserved), so
  `ChannelInfo::wave` and `audio_state` report 8–15 for a wavetable voice.
- **Nibbles.** Exactly 32 hex digits, most significant sample first. They may be
  written as one run or split into whitespace-separated groups
  (`wavetable 0 8cefeede eedeefec 73101121 11211013`) — grouping is cosmetic.
- **Mapping**: nibble `n` plays at **`(2n − 15) / 15`**, so `0` = −1.0, `f` =
  +1.0, and codes `n` and `15 − n` are exact negations. Dividing by 15 rather
  than 16 is deliberate: it makes wavetables reach the same full scale as the
  builtin oscillators, and
  `wavetable 0 ffffffffffffffff0000000000000000` is therefore *the* square wave
  (id 2), sample for sample. The cost is that **4 bits cannot represent zero**:
  the two centre codes are `7` = −1/15 and `8` = +1/15, so an all-`8` table is
  a constant +0.0667 DC offset rather than silence. A table is exactly DC-free
  when `Σ(2n − 15) = 0`, i.e. when its codes pair up around the centre — the
  authoring rule is "one `7` for every `8`".
- **Playback**: the same fixed-point phase accumulator as every other wave. The
  top 5 bits of the 32-bit phase are the sample index
  (`phase >> 27`, always 0–31), so one cycle of the table plays per period of
  the note and there is no rounding anywhere.
- **No interpolation**, on purpose. The staircase edges are the sound: that
  crunch is what a Game Boy or an N163 gives you and it is the reason to have
  the format at all — a smoothed 32-point table would just be a duller saw.
  It is also the cheapest possible read (a shift and a load). Linear
  interpolation would be perfectly deterministic (rational arithmetic on const
  values), so a future per-instrument `interp=` flag stays possible; the
  *default* is crunchy.
- **Composes with everything.** A wavetable is a wave source and nothing else,
  so `env`, `vib`, `sweep`, `duck`, `echo=` and the whole fx column
  (`arp`/`sl`/`vib`/`fade`) apply unchanged — vibrato and sweeps modulate the
  phase increment exactly as they do for a builtin wave.
- **Errors at parse time, never a silent fallback** (house style): a slot
  outside 0–7, a nibble count other than 32, a non-hex character, two lines
  claiming the same slot, or a `w<slot>` reference to a slot the cart never
  defined. `inst` lines may reference a `wavetable` line further down the
  section (same forward-reference rule as `__gfx_meta__`); definedness is
  checked once the section has parsed.
- Memory is 8 × 32 f32 resolved once at load; nothing at runtime rewrites a
  table, and no Lua setter exposes them (a cart's timbres are cart data).

### 2-op FM (phase 1.9)

```
inst <name> wave=6 fm=<ratio>,<index>[,<decay>] ...   # in __instruments__
A2 fmbass 5                                            # …and any sfx row plays it
                                                       #   (NOTE INST VOL, as ever)
```

One **modulator** phase-modulating one **carrier**, both sine — the smallest
useful slice of a YM2612, and the one that gets you Genesis bass, electric
pianos, brass and bells. The model is the textbook pair:

```
out(t) = sin(2π·fc·t + β·sin(2π·ratio·fc·t))
```

`fc` is the row's note (the carrier is always at pitch), `ratio` locks the
modulator to it, `β` is the modulation index. `wave=6` and `fm=` are two halves
of one statement: neither is legal alone.

- **The sine table.** A `const` array of **257 f32 literals**, one quarter of a
  cycle at 1024 points per cycle (`SINE_QUARTER[k] = sin(2πk/1024)`), generated
  at authoring time and pasted into `audio.rs` exactly the way `NOTE_FREQ` is —
  there is no `sin` at runtime. The other three quadrants are derived by
  mirroring and negation, which makes the symmetries **bit-exact**: the
  oscillator's zeros land on exactly 0.0, its peaks on exactly ±1.0,
  `sine(−p) = −sine(p)` and `sine(p + ½ turn) = −sine(p)` to the last bit.
  Adjacent entries are **linearly interpolated** over a 16-bit fraction of the
  32-bit phase (`a·(1−f) + b·f`, rational arithmetic on const values, so every
  target agrees). Worst-case interpolation error is `(π/1024)²/8 ≈ 1.2e-6`,
  about −118 dBFS: inaudible, and two bits below the 16-bit WAV the harness
  writes.
- **`ratio`: 0.5 to 15 in steps of 0.5**, written the way a musician writes it
  (`0.5`, `1`, `2`, `3.5`, `7`; `2.0` is accepted too). Stored as an integer
  count of *halves*, so the modulator increment is
  `carrier_inc · ratio_half / 2` in exact 64-bit integer arithmetic — no
  rounding to specify. The ladder is the chips' own (the YM2612's MUL field is
  0.5 then 1–15), and the half-integers are the point of allowing halves at
  all: an **integer** ratio puts every sideband on a harmonic (pitched, and the
  waveform repeats once per carrier period), a **half-integer** ratio puts them
  midway between harmonics (inharmonic, bell-like, and the waveform only
  repeats every *two* carrier periods). A modulator pushed past the sample rate
  wraps the phase accumulator, i.e. it aliases — deterministic, and the same
  thing any digital oscillator does.
- **`index`: 0–15**, the depth at note-on. One step is a peak phase deviation
  of **1/8 cycle** (`2^29` phase units, an exact power of two), i.e.
  `β = 2π·index/8 ≈ 0.785·index` radians. `0` is a pure sine — waveform 6 with
  `index=0` is the console's only clean sine oscillator. 1–3 is warm and
  hollow, 4–6 is the Genesis bass/brass region, 7–10 is glassy, 11–15 is
  clangorous.
- **`decay`: 0–15** (optional, default 0), the **index envelope**: the index is
  multiplied once per *frame* by a const `0.5^(1/half-life)` factor, with
  half-lives running 120 frames at `decay=1` down to 1 frame at `decay=15`.
  `decay=0` holds the index flat for the life of the note. The index decays
  **toward zero** and snaps to exactly 0 below 1/1024 of a step (a geometric
  decay never arrives, and an index that small is a deviation of 1/8192 of a
  cycle).

  This is the gesture that makes FM sound alive: a struck tone is bright at the
  attack and dull by the time it decays, and on an FM voice that is the *index*
  falling, not the level. It is a **separate envelope from `env`** on purpose —
  an electric piano holds its level while its brightness dies, a bell does the
  opposite.
- **Composes with everything.** FM is a wave *source*, exactly like a
  wavetable, so `env`, `vib`, `sweep`, `duck`, `echo=` and the whole fx column
  (`arp`/`sl`/`vib`/`fade`) apply unchanged. The modulator's increment is
  derived from the carrier's **every sample** rather than cached, so a vibrato,
  slide, arpeggio or sweep bends *both* operators together and the ratio (and
  with it the timbre) holds through the bend — a swept FM voice is a
  transposition, never a detune.
- **Phase.** Both accumulators are the usual 32-bit fixed-point ones and both
  are continuous across notes, like every other waveform on this console: a
  note-on re-arms the index envelope but resets no phase.
- **Errors at parse time, never a silent fallback** (house style): `wave=6`
  without `fm=`, `fm=` on any other wave, a ratio off the 0.5 grid or outside
  0.5–15, an index or decay above 15, and a **bare `6` in a sfx row's wave
  column** (a digit carries no parameters, so the row has to name an
  instrument). Waveform id 7 remains reserved and is still rejected.
- **Off by default in the strongest sense**: no cart written before this can
  produce waveform id 6, because the only route to it is an `inst … wave=6
  fm=…` line that did not parse. Every existing audio golden is untouched.

### Master bus & sidechain ducking (phase 1.5)

```
master drive=<0-8> [tone=<0-8>] [hiss=<0-4>]     # in __instruments__, at most one
inst <name> ... [duck=<depth 1-7>,<release 1-255>]
```

Signal order: channels → duck gain → sum×0.25 → drive/soft-clip → tone
lowpass → hiss → clamp. All defaults zero = bit-identical legacy path.

- `drive`: pre-gain (1 + 0.35·drive) into the rational odd shaper
  `x·(27+x²)/(27+9x²)` (odd harmonics only, monotonic, C¹ hard-clip at ±3)
  with equal-loudness makeup normalized at the 0.7 reference level. Warm
  glue at 1–3, obvious drive at 5+.
- `tone`: one-pole lowpass, baked coefficient table (off, 16 kHz … 3 kHz).
- `hiss`: dedicated-LFSR tape floor, ≈−54 dBFS at 4.
- `duck=depth,release` marks an instrument as a sidechain **trigger**: its
  note-ons dip every *other* channel by depth/7 (≈1 ms anti-click attack,
  linear recovery over `release` frames, re-trigger restarts). The classic
  kick-pump.
- Lua: `master(drive, [tone], [hiss])` overrides the cart's master line at
  runtime (omitted args = 0; `master(0)` = clean). Game-scriptable — e.g.
  underwater = high tone, boss = drive up.

### Echo bus (phase 1.5)

```
echo delay=<1-60> feedback=<0-8> level=<0-8>   # in __instruments__, at most one
inst <name> ... [echo=<send 0-8>]
```

One mono delay line with feedback, fed by a per-voice send — the SNES echo
unit. Off by default; all defaults zero = bit-identical legacy path.

Signal flow (the echo bus sits between the sidechain and the master bus):

```
                       ┌──────── feedback * 7/64 ◄───────┐
                       │                                 │
 channels ─► duck ─┬─► echo send ─►(+)─► delay line ─► loop LP ─┘
  (6 voices) gain  │    (0-8)/8                │
                   │                           ▼ * level/8
                   └────── dry sum ──────►(+)◄─┘
                                           │
                                           ▼
                         * 0.25 ─► drive/shaper ─► tone LP ─► hiss ─► clamp
```

- **`delay`: whole frames, 1–60** (16.7 ms – 1 s). Frames, not milliseconds:
  a frame is exactly 735 samples, so the read pointer is always an integer
  index with no rounding or resampling. One frame is 16.67 ms, which is
  effectively the SNES EDL's 16 ms grid — coarse and steppy on purpose — and
  row length is already in frames, so tempo-synced echo is head arithmetic
  (at `speed=8`, `delay=8` is a row, `delay=24` a dotted eighth).
- **`feedback`: `f * 7/64`**, i.e. 0 … **7/8 = 0.875 maximum**. Deliberately
  below unity so the loop always decays (−1.16 dB per repeat at the top, 60 dB
  down after ~59 repeats). The loop filter cannot be relied on for stability —
  its DC gain is exactly 1 — so the gain itself is the guarantee.
- **`level` (return) and `echo=` (per-voice send): eighths, `n/8`**, so `8` is
  unity. Sends are taken **post-duck** (the echo pumps with the kick rather
  than filling the hole it dug) and the return is added to the dry sum before
  the ×0.25 mix gain, so `level=8` is "as loud as a voice at unity send".
- **Loop lowpass**: one pole, `y += a·(x − y)` with **a = 0.49534696**
  (`1 − exp(−2π·4800/44100)`, evaluated at authoring time — no `exp` at
  runtime). Every repeat is filtered again, so the tail darkens progressively
  and sits behind the dry signal. This is the SNES FIR's job, done with one
  pole.
- **Memory**: a fixed 44100-sample (one second) zero-initialised buffer,
  allocated once per console. Nothing in `step()` allocates; changing `delay`
  only moves a read index (repeats in flight jump, tape-echo style).
- **Off** means `delay == 0` **or** `level == 0`: the line is never touched and
  the mixer takes the PoC v1 statement, so a cart with no `echo` line renders
  bit-identical samples. An `echo=` send on an instrument is inert until a bus
  exists.
- **Headroom**: the echo adds energy and nothing inside the bus limits it.
  Six voices at `echo=8` into `feedback=8` settle at up to
  `6 / (1 − 7/8) = 48` in channel units; the no-drive path then hard-clips at
  the final clamp, and any non-zero `master drive` soft-limits below full scale
  instead. Bounded and finite at every setting, but author sends at 2–4 and
  reach for `master drive=1` on echo-heavy carts.
- Lua: `echo(delay, [feedback], [level])` overrides the cart's echo line at
  runtime (omitted args = 0). `echo(0)`, `echo(-1)` or any `level=0` switches
  the bus off **and flushes the delay line**, so re-enabling it later cannot
  replay an earlier scene's tail.

### Effects column (phase 1; optional 4th token on a note row)

| fx | behavior |
|----|----------|
| `arp<a>,<b>` | cycle pitch offsets 0,+a,+b semitones, 2 frames per step (the chiptune chord) |
| `sl<±n>` | slide n semitones across the row's duration (portamento) |
| `vib` or `vib<cents>,<rate>` | vibrato this row (bare form uses the instrument's setting) |
| `fade<±n>` | volume ramps by n levels across the row |

One fx per row for now. All fx are per-row state on the channel, reset at
the next note row.

### Tempo sugar (phase 1)

`__music__` may open with `bpm=<n> [rows_per_beat=<r>]` (default r=4).
Sfx may then declare `speed=auto` = `round(3600 / (bpm * r))`. Explicit
numeric speeds keep working everywhere.

### `carts/soundtest.cart` (phase 1)

A listening-session cart: d-pad browses a menu of audition entries — each
waveform, vibrato off/on comparison, arpeggio chord, slides, a drum kit
pattern, the two-pulse echo trick, one full 4-channel groove, a clean/driven
A/B of the master bus, a sparse melody through the echo bus, a wavetable
audition (a hollow lead and a gritty one over a held organ pad, three
32-nibble tables) and a 2-op FM audition (an Am–F–C–G phrase with an FM bass,
an electric piano and a bell, one classic patch per channel) — A plays the
selection, B stops. This is the vehicle for tuning instrument defaults
by ear; agents render entries to WAV via the harness for the same purpose.

`master` and `echo` are both cart-global, so the two entries that demonstrate
them drive the Lua setters and every other entry explicitly resets them
(`master(0)`, `echo(0)`) — which is why the cart declares neither line and
every pre-existing entry renders exactly as it always did. New entries are
appended to the **end** of the menu so the golden entries keep their input
scripts.

### Phase 2 (after phase 1 lands — planned, spec to be detailed then)

`music score` (all channels as one time-aligned text grid), `music lint`
(slot length/speed mismatches, out-of-key notes, vertical clashes, range
sanity), `music summarize` (chord skeleton per bar as text), piano-roll PNG,
`music render <cart> <pat> -o out.wav --loops N` (synth without stepping a
cart). Phase 3: transposes/copies/double-time, auto-echo onto a free
channel, ABC notation import.

## Out of scope for PoC

Multiple carts, save data, interactive sprite/sfx editors, sfx effects
columns (arpeggio/slide/vibrato), stereo, runtime anim helpers (`aspr()` —
revisit once `__gfx_meta__` proves out).
Design must not preclude them. (A minimal pause menu — RESUME/RESET —
already exists in the web shell.)

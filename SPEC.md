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
| `map([cel_x=0], [cel_y=0], [sx=0], [sy=0], [cel_w=128], [cel_h=64])` | draw a cel_w×cel_h block of map cells from cell (cel_x, cel_y) to (sx, sy); **tile 0 is skipped**. `map()` draws the whole map at 0,0 |
| `mget(cx, cy)` | tile id at map cell (cx, cy); off the map reads 0 |
| `mset(cx, cy, [v=0])` | write a tile id (0–255, masked); off the map is a no-op |
| `print(s, x, y, [c=12])` | draw text with built-in 4×6 font (ASCII 32–126; lowercase may render as uppercase) |
| `camera([x=0], [y=0])` | draw offset subtracted from all later draw coords; no args resets |
| `clip([x, y, w, h])` | clip rectangle in **screen** space; no args resets to full screen |
| `pal([c0], [c1], [p=0])` | p=0 draw-palette remap (rewrites pixels), p=1 display-palette remap (scanout only); no args resets both maps **and** `palt` |
| `palt([c], [flag])` | mark color c transparent in `spr()`; no args resets to "only color 0" |
| `btn(i)` / `btnp(i)` | button held / just-pressed this frame |
| `rnd([n=1])` | deterministic float in [0, n) — PCG32 or xoshiro seeded PRNG in Rust |
| `srand(seed)` | reseed PRNG (reset seeds it to 0 unless overridden) |
| `t()` | seconds since cart start = frame_count / 60 (exact, from frame counter) |
| `flr(x)`, `ceil(x)`, `abs(x)`, `min/max/mid(...)`, `sin(x)`, `cos(x)` | conveniences; sin/cos take **turns** (PICO-8 style: `sin(0.25) = -1`... actually use standard sign: `sin(t)` = `math.sin(t*2π)`, PICO-8 inverts — we do NOT invert) |
| `printh(s)` | log line to host (harness `logs`, browser console). Never draws. |

All draw coordinates are floats, truncated toward negative infinity (`flr`) before use.

### Draw state

`camera`, `clip`, `pal` and `palt` form one block of **persistent** draw state.
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

`map()` is defined as a sequence of `spr()` calls and shares their pixel path
exactly: the camera offsets its destination, the clip rect bounds it, `palt`
decides per-pixel transparency on the source color and the draw palette remaps
what lands in the framebuffer. Only the per-**cell** tile-0 skip is map-specific
— it happens before any pixel is read, so an empty cell is invisible even under
`palt(0, false)`.

Host surfaces: `Console::display_palette() -> &[u8; 16]` (identity by default)
and `Console::draw_state()`. `console-agent` applies the display palette when
it renders PNG screenshots; `screen_text` stays raw. The web shell composes
`palette[dpal[idx]]`.

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
  frequency).
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
anim <sprite>.<label> frames=<i0,i1,...> fps=<1-60> [loop]
```

- `name`: `[a-z0-9_]+`, unique. `rect` in tile coords (0–15), `size` in
  tiles (1×1 up to 16×16). `anchor` in pixels relative to the sprite's
  top-left; default = bottom-center `(w*8/2, h*8-1)` (ground contact).
- Anim names are namespaced by sprite (`player.walk`). Frame index `i`
  addresses the rect displaced `i` sprite-widths to the right, wrapping to
  the next row band: `tx' = (tx + i*w) % 16`, `ty' = ty + ((tx + i*w) / 16) * h`.
  Every resolved rect must fit the 16×16 tile sheet.
- Validation after the whole section parses (forward references fine);
  errors are `Error::Cart` with section-relative line numbers. Section is
  optional; carts without it behave identically.
- Core API: `Cart::gfx_meta()` exposing sprite/anim defs plus
  `resolve_frame(sprite, i) -> pixel rect` — pixel data itself comes from the
  existing sprite sheet accessors.

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
| `sprite lint <cart> [anim ...]` | JSON to stdout, per frame and per consecutive pair (loop-aware): changed-pixel count, silhouette area + % drift, centroid & bbox relative to anchor + per-frame drift, per-frame palette histogram, colors unique to a single frame. Report-only; agents do the asserting. |

RPC mirrors: `sprite_render`, `sprite_strip`, `sprite_onion`, `sprite_diff`,
`sprite_ghost`, `sprite_lint` against the session's loaded cart.

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

## Music authoring (PoC v2)

The synth's expressiveness is the ceiling on music quality, so phase 1 is
engine vocabulary; inspection/transform tooling (phase 2/3) mirrors the
sprite tools. Determinism contract unchanged: no transcendentals at runtime —
pitch offsets resolve through NOTE_FREQ with **linear interpolation between
adjacent semitones**, vibrato applies linear frequency scaling with a const
cents factor, LFOs are integer-phase triangles.

### `__instruments__` section (phase 1)

```
inst <name> wave=<0-5> [env=<attack>,<decay>,<sustain>] [vib=<cents>,<rate>,<delay>] [sweep=<semis>,<frames>]
```

- `name` `[a-z0-9_]+`, unique, must not shadow the bare wave digits 0–5.
- `env`: attack frames (vol ramps 0→row vol), decay frames (then decays
  toward sustain), sustain level 0–7 held until the row/note changes.
  Default: flat at row volume.
- `vib`: depth in cents (1–100), rate as an integer LFO period divisor
  (1–16, higher = faster), delay frames before onset. Triangle LFO on pitch.
- `sweep`: signed semitone offset traversed over N frames from note-on
  (drum sweeps: `sweep=-12,6` = kick).
- Sfx rows may name an instrument in place of the wave digit
  (`A4 lead 5`); a bare digit means "flat instrument with that wave"
  (today's behavior, still valid — old carts unchanged).
- Percussion is just instruments: `inst kick wave=3 sweep=-14,5 env=0,6,0`,
  triggered by an ordinary note row giving the sweep's start pitch.

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
pattern, the two-pulse echo trick, and one full 4-channel groove — A plays
the selection, B stops. This is the vehicle for tuning instrument defaults
by ear; agents render entries to WAV via the harness for the same purpose.

### Phase 2 (after phase 1 lands — planned, spec to be detailed then)

`music score` (all channels as one time-aligned text grid), `music lint`
(slot length/speed mismatches, out-of-key notes, vertical clashes, range
sanity), `music summarize` (chord skeleton per bar as text), piano-roll PNG,
`music render <cart> <pat> -o out.wav --loops N` (synth without stepping a
cart). Phase 3: transposes/copies/double-time, auto-echo onto a free
channel, ABC notation import.

## Out of scope for PoC

Multiple carts, save data, map authoring/preview tooling, interactive
sprite/sfx editors, sfx effects columns (arpeggio/slide/vibrato), stereo,
runtime anim helpers (`aspr()` — revisit once `__gfx_meta__` proves out).
Design must not preclude them. (A minimal pause menu — RESUME/RESET —
already exists in the web shell.)

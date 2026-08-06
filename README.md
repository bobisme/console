# A fantasy console for phones and AI agents

A PICO-8-inspired fantasy console: a tiny virtual machine with a fixed
display, palette, input, and audio spec, plus a Lua runtime. Two things
make it unusual:

1. **Games ship as one self-contained HTML file.** `console pack` splices a
   cart's Lua/sprite/sound text and the wasm engine into a single
   `game.html` with zero external requests — it works from `file://`, and
   the cart's source stays readable and editable inside the HTML.
2. **AI agents are first-class developers, not an afterthought.** There is
   no visual editor. An agent (or a human) writes carts as plain text and
    drives the console headlessly through `console`: step frames with
    scripted input, take a screenshot, exchange sprites with PNG editors,
    quantize artwork to Apollo64, dump the framebuffer as text, inspect audio
    as data, eval Lua expressions — all without a GPU, a display, or ears.

Both are anchored by a determinism contract: **same cart + same seed + same
per-frame input ⇒ byte-identical framebuffers and audio samples**, on native
and on wasm. That's what makes headless development trustworthy — an
agent's screenshot is exactly what a player's screen shows — and what makes
replays double as regression tests.

The project doesn't have a final name yet; `console` is the working title and
the single command used for running, inspecting, packing, and serving carts.

See [SPEC.md](SPEC.md) for the full, authoritative contract (every API
function, cart section format, and determinism rule). This README is an
orientation, not the spec.

## Specs at a glance

| | |
|---|---|
| Display | 192×320 logical pixels (3:5 portrait), 60 fps fixed timestep |
| Art | 8×8 tile unit; 128×128 sprite sheet; 24×40 visible tile cells |
| Palette | fixed 64-color Apollo64 palette, one byte-sized index per pixel |
| Input | 7 buttons: d-pad, A, B, menu |
| Audio | 6 channels, waveforms: pulse 12.5%/25%, square, triangle, saw, noise, plus 8 cart-defined 32×4-bit wavetables |
| Script | Lua 5.4 (mlua, vendored), sandboxed — no host `io`/`os`/`debug`/`package`; project builds provide a private static `require` |
| Entities | deterministic console-native Lua ECS; creation-order queries, deferred structural edits, bounded host inspection |
| Cart | projects compile normal source files into one plain-text `.cart`: Lua + sprites (64-character grid) + tile map + sfx/music (tracker text) |

192×320 is the retained platform resolution, not a transitional or optional
high-resolution mode. Carts should compose for the full canvas. The 8×8 sprite
is a storage and map unit rather than a target size for every object: prominent
actors and interactables will usually read better on a phone when assembled as
16–24px forms. The current 128×128 sprite sheet remains an intentional,
independent capacity limit; changing asset storage is a separate platform
decision from changing the display.

Draw state (`camera`, `clip`, `pal`, `palt`, `fillp`, `mosaic`, `rshift`)
persists across frames like PICO-8's, and there's a runtime-mutable tile map
(`map()`, `mget`, `mset`) plus scaled blits (`sspr`) as part of the core
drawing API. Animations are declared once in the cart's `__gfx_meta__` and
played by `aspr(name, x, y, [t0])`, which is stateless (the frame is a pure
function of the frame counter) and draws from the sprite's declared anchor —
see SPEC.md for exact semantics.

Entity-heavy carts can create named `ecs.world` instances and compose plain Lua
components without exposing Bevy or host internals. Queries are deterministic,
structural edits are deferred until iteration ends, and agents can inspect a
bounded field projection through the read-only `ecs_query` RPC. The
multi-file `examples/radiant-swarm` bullet hell is the reference vertical slice.
Named ECS watches reuse a bounded projection across selected frames and report
population/component/returned-ID deltas without retaining an unbounded log.

## Repo layout

| path | what |
|------|------|
| `crates/console-core` | the console itself: Lua VM, framebuffer, drawing/audio, cart parser. Pure and deterministic — no windowing, GPU, audio device, wall clock, or filesystem access from Lua. Builds for native and `wasm32-unknown-emscripten`. |
| `crates/console-agent` | the unified `console` CLI: headless runs, JSON-RPC, playtests, asset authoring, packing, and local serving |
| `crates/console-web` | emscripten build exposing a small C ABI over the core |
| `web/` | the device-chassis HTML/JS shell and the engine build recipe ([web/BUILD.md](web/BUILD.md)) |
| `carts/` | compact example carts: `demo.cart`, `soundtest.cart`, and the `lantern-leap.cart` platformer |
| `examples/` | multi-file source projects, including the ECS-heavy `radiant-swarm` bullet hell |
| `skills/build-cart` | a publishable skill for authoring carts — the thing to hand an agent |
| `SPEC.md` | the authoritative platform contract |

## Quickstart

Build everything:

```bash
cargo build --release
```

For a multi-file game, keep Lua and native cart sections in a project directory
and compile the deterministic distribution cart:

```text
my-game/
├── console.toml
├── scene.toml             # optional layered scene source
├── lua/main.lua
├── art/player.png
├── art/enemies.png
├── art/environment.png
├── art/environment.semantic
├── map.txt
├── gfx-meta.txt
├── instruments.txt
├── sfx.txt
└── music.txt
```

```bash
console build my-game
console build my-game --check
```

For larger environments, compile tile-aligned Apollo64 layers, semantic grids,
metatiles, seeded variants, autotiles, and anchored objects into those ordinary
project inputs before building:

```bash
console scene compile my-game/scene.toml --out my-game/generated
console scene compile my-game/scene.toml --out my-game/generated --check
console build my-game
```

The compiler emits a packed atlas, native map, collision/decorative/object Lua,
provenance, and labeled visual review sheets. It is an authoring tool only: the
resulting game uses the existing sprite, map, and Lua runtime.

The default output is `my-game/build/game.cart`; `[build].output` or `-o`
selects another path. See the project-manifest contract in [SPEC.md](SPEC.md).
PNG assets can be named and placed explicitly with `[[sprites]]`; the build
maps them through Apollo64 and generates both `__sprites__` and matching
`__gfx_meta__` declarations. Exact conversion is the default, while lossy
nearest or budgeted quantization must be requested in the manifest.

`console run`, `playtest`, `pack`, and `serve` also accept `my-game/` or its
explicit `console.toml` directly. They compile in memory, so ordinary iteration
does not require or modify `build/game.cart`; `serve` recompiles on every GET
and HEAD refresh. The complete setup and single-cart migration walkthrough is
[docs/project-workflow.md](docs/project-workflow.md); a working project that
uses every native cart section is in
[examples/agent-platformer](examples/agent-platformer).

Native instruments, SFX, effects, and pattern chains can stay in one playable
`console-music 1` bundle. Point `[audio].bundle` at a `.cmusic` file and
`console build` expands its `__instruments__`, `__sfx__`, and `__music__`
sections into the cart. This replaces, and cannot be mixed with, the three raw
audio entries under `[sections]`. A minimal buildable example is in
[examples/native-music](examples/native-music).

MIDI and ABC sources can be auditioned through the console's own six-channel
synth before they are reduced to cart rows. MIDI converts to pipeable ABC on
stdout by default:

```bash
console music midi-to-abc theme.mid > theme.abc
console music midi-to-abc theme.mid -o theme.abc
console music play theme.mid
console music play theme.abc --seconds 15 --volume 0.35
console music play theme.abc --repeat
console music play audio/game.cmusic --song 0 --volume 0.35
console music play my-game --song 0 --dry-run
```

`music play --dry-run` decodes and validates without opening an audio device,
which makes source validation usable in CI. ABC/MIDI previews render a bounded
pass; native inputs are parsed and planned without allocating PCM. Playback
defaults to `--volume 0.5`; pass a value from 0 (silent) to 1 (full synth
output) to change its linear output gain. Cart import remains an explicit later
step via `music import-abc`. `--repeat` loops the rendered track until Ctrl-C;
with `--seconds` it loops that selected prefix, while `--dry-run` validates one
pass and exits.
For `.cmusic`, cart, and project inputs, playback uses the exact native
instrument/effect grammar and song chain. Authored loops repeat naturally;
the renderer preserves oscillator and effect state between passes. `--repeat`
restarts a native one-shot after a release frame tapered to silence, including
when echo remains active.

Run a cart headlessly for 90 frames (idle 30, hold right 30, idle 30) and
take a 4x screenshot — this is exactly how an agent iterates on a cart:

```bash
./target/release/console run carts/demo.cart \
  --frames 90 --input "30:,30:R,30:" \
  --screenshot /tmp/frame90.png --screenshot-zoom 4 \
  --eval-after 'return {frame=t()*60}'
```

Pack the same cart into a single-file game:

```bash
./target/release/console pack carts/demo.cart -o dist/demo.html
```

Open `dist/demo.html` in any browser (double-click it — `file://` works, no
server needed). The default engine and HTML shell are embedded in `console`,
so packing works from any directory. Rebuilding the embedded wasm engine from
`crates/console-web` requires emsdk — see [web/BUILD.md](web/BUILD.md).

For a live edit-and-refresh loop, bundle and serve a cart locally:

```bash
console serve my-game
```

The command prints the URL and recompiles/re-bundles the project on each page
refresh. A standalone `.cart` remains accepted everywhere.

With `agent-browser` and Chromium provisioned, the repository's real-browser
acceptance gate packs and drives Lantern Leap end to end:

```bash
CONSOLE_BROWSER=/path/to/chromium just browser-check
```

It treats missing browser infrastructure as a failure and retains the exact
packed HTML, screenshot, and browser diagnostics under `out/browser-check/`
when an assertion fails.

## For AI agents

Hand an agent [skills/build-cart/SKILL.md](skills/build-cart/SKILL.md) — it
covers the cart format, the Lua API, determinism rules, and the full
authoring loop (sprite/animation tools, music/sfx tooling, packaging).

For programmatic control, `console rpc` speaks JSON-RPC 2.0 (one
request per line on stdin/stdout): load a cart, step frames with input,
pull a screenshot or the framebuffer as text (`screen_text`), `eval` Lua,
and inspect audio without ears (`audio_state`, `audio_events`,
`audio_stats`, `spectrogram`). Optional `draw_trace`/`draw_events` diagnostics
explain which tagged primitive, sprite, animation, map, or text call produced a
region without changing the rendered frame. `ecs_query` pages through selected
scalar fields from a named ECS world without requiring cart-authored debug
tables; `ecs_watch_*` saves those selectors and compares explicit samples.
Screen-text requests can select a strict native-pixel region or return a
compact count/bounds summary, avoiding a 192×320 glyph dump when an agent only
needs a HUD, dialog, or collision area. Full method list in SPEC.md.

For repeatable multi-stage acceptance, use a versioned playtest scenario
instead of hand-driving an RPC session:

```bash
console playtest carts/lantern-leap.cart \
  --scenario carts/lantern-leap.playtest.json \
  --artifacts /tmp/lantern-playtest --format json
```

Scenario stages run in file order and can evaluate Lua, hold an input mask for
an exact frame count, compare an evaluated value to JSON, and capture
screenshots, screen text, WAVs, spectrograms, audio events, and signal stats.
Captures can also emit authored or live-runtime map PNG, hex, and lint
artifacts or bounded draw-trace JSON. A capture can also map stable
`draw_tag()` names to transparent layer PNGs, preserving background, terrain,
actors, and effects separately beside the live collision-map evidence from the
same frame.
Sequence stages sample exact frame cadences into cropped, nearest-neighbor GIFs
and contact strips. They can also build labeled review boards beside an
optional reference PNG, which stays at its untouched native size and is
explicitly marked as non-pixel-aligned comparison art.
A final scenario `review` stage can consolidate named still and motion stages,
tag-isolated layers, a live/authored map panel, and reference art into one
deterministic diagnostic board. Its color, grayscale, luma-band, edge, and
false-color palette-index views ship with a JSON evidence report containing
counts and histograms—not a synthetic claim that the art is good.
Reviews can also enforce game-authored temporal limits. `boundary` checks
compare two named still stages, while `consecutive` checks find the worst pair
among a named motion sequence. Both report exact changed-pixel fractions,
support explicit allowed-motion rectangles, and can emit deterministic diff
heatmaps; exceeding the declared limit fails the playtest while preserving the
evidence. Optional tag-aware lint reports warnings for reserved palette roles,
bright background horizontals, weak actor/background luma separation, and
dense traversal-corridor edges. These are narrow authored contracts, not an
automated art score.

## Development

This repo uses a bones/maw workflow for changes — see
[AGENTS.md](AGENTS.md). Before sending a change, run the same gate configured
for Edict:

```bash
just check
```

This checks formatting, runs warning-free clippy and the full Rust test suite,
then cross-checks the committed wasm engine against native behavior. After a
release, install the current local `console` CLI with `just install`.

## Status

Early and in active development — expect breaking changes to the cart
format and API. No license has been chosen yet; there is no LICENSE file in
this repo.

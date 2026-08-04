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
   scripted input, take a screenshot, dump the framebuffer as text, inspect
   audio as data, eval Lua expressions — all without a GPU, a display, or
   ears.

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
| Script | Lua 5.4 (mlua, vendored), sandboxed — no `io`/`os`/`debug`/`require` |
| Cart | one plain-text file: Lua + sprites (64-character grid) + tile map + sfx/music (tracker text) |

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

## Repo layout

| path | what |
|------|------|
| `crates/console-core` | the console itself: Lua VM, framebuffer, drawing/audio, cart parser. Pure and deterministic — no windowing, GPU, audio device, wall clock, or filesystem access from Lua. Builds for native and `wasm32-unknown-emscripten`. |
| `crates/console-agent` | the unified `console` CLI: headless runs, JSON-RPC, playtests, asset authoring, packing, and local serving |
| `crates/console-web` | emscripten build exposing a small C ABI over the core |
| `web/` | the device-chassis HTML/JS shell and the engine build recipe ([web/BUILD.md](web/BUILD.md)) |
| `carts/` | example carts: `demo.cart`, `soundtest.cart`, the `lantern-leap.cart` platformer, and the tongue-grappling mutant action showcase `ribbit-recoil.cart` |
| `skills/build-cart` | a publishable skill for authoring carts — the thing to hand an agent |
| `SPEC.md` | the authoritative platform contract |

## Quickstart

Build everything:

```bash
cargo build --release
```

Run a cart headlessly for 90 frames (idle 30, hold right 30, idle 30) and
take a 4x screenshot — this is exactly how an agent iterates on a cart:

```bash
./target/release/console run carts/demo.cart \
  --frames 90 --input "30:,30:R,30:" \
  --screenshot /tmp/frame90.png --screenshot-zoom 4
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
console serve carts/demo.cart
```

The command prints the URL and re-bundles the cart on each page refresh.

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
`audio_stats`, `spectrogram`). Full method list in SPEC.md.

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

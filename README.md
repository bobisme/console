# A fantasy console for phones and AI agents

A PICO-8-inspired fantasy console: a tiny virtual machine with a fixed
display, palette, input, and audio spec, plus a Lua runtime. Two things
make it unusual:

1. **Games ship as one self-contained HTML file.** `console-pack` splices a
   cart's Lua/sprite/sound text and the wasm engine into a single
   `game.html` with zero external requests — it works from `file://`, and
   the cart's source stays readable and editable inside the HTML.
2. **AI agents are first-class developers, not an afterthought.** There is
   no visual editor. An agent (or a human) writes carts as plain text and
   drives the console headlessly through `console-agent`: step frames with
   scripted input, take a screenshot, dump the framebuffer as text, inspect
   audio as data, eval Lua expressions — all without a GPU, a display, or
   ears.

Both are anchored by a determinism contract: **same cart + same seed + same
per-frame input ⇒ byte-identical framebuffers and audio samples**, on native
and on wasm. That's what makes headless development trustworthy — an
agent's screenshot is exactly what a player's screen shows — and what makes
replays double as regression tests.

The project doesn't have a final name yet; "console" is the working title,
which is why the crates and binaries are all `console-*`.

See [SPEC.md](SPEC.md) for the full, authoritative contract (every API
function, cart section format, and determinism rule). This README is an
orientation, not the spec.

## Specs at a glance

| | |
|---|---|
| Display | 144×256 logical pixels (9:16 portrait), 60 fps fixed timestep |
| Palette | fixed 16 colors (Sweetie-16), one index per pixel |
| Input | 7 buttons: d-pad, A, B, menu |
| Audio | 6 channels, waveforms: pulse 12.5%/25%, square, triangle, saw, noise |
| Script | Lua 5.4 (mlua, vendored), sandboxed — no `io`/`os`/`debug`/`require` |
| Cart | one plain-text file: Lua + sprites (hex grid) + tile map + sfx/music (tracker text) |

Draw state (`camera`, `clip`, `pal`, `palt`, `fillp`, `mosaic`) persists across
frames like PICO-8's, and there's a runtime-mutable tile map (`map()`, `mget`,
`mset`) plus scaled blits (`sspr`) as part of the core drawing API — see
SPEC.md for exact semantics.

## Repo layout

| path | what |
|------|------|
| `crates/console-core` | the console itself: Lua VM, framebuffer, drawing/audio, cart parser. Pure and deterministic — no windowing, GPU, audio device, wall clock, or filesystem access from Lua. Builds for native and `wasm32-unknown-emscripten`. |
| `crates/console-agent` | headless dev CLI for agents: oneshot `run`, interactive `serve` (JSON-RPC over stdio), and `sprite` authoring/inspection tools |
| `crates/console-web` | emscripten build exposing a small C ABI over the core |
| `crates/console-pack` | packs a cart + the engine build into one self-contained `game.html` |
| `web/` | the device-chassis HTML/JS shell and the engine build recipe ([web/BUILD.md](web/BUILD.md)) |
| `carts/` | example carts: `demo.cart` (a small platformer-ish demo) and `soundtest.cart` (a synth listening session) |
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
./target/release/console-agent run carts/demo.cart \
  --frames 90 --input "30:,30:R,30:" \
  --screenshot /tmp/frame90.png --screenshot-zoom 4
```

Pack the same cart into a single-file game:

```bash
./target/release/console-pack carts/demo.cart -o dist/demo.html
```

Open `dist/demo.html` in any browser (double-click it — `file://` works, no
server needed). `console-pack` uses the engine build already committed at
`web/engine.js`; rebuilding that wasm engine from `crates/console-web`
requires emsdk — see [web/BUILD.md](web/BUILD.md) for the recipe.

## For AI agents

Hand an agent [skills/build-cart/SKILL.md](skills/build-cart/SKILL.md) — it
covers the cart format, the Lua API, determinism rules, and the full
authoring loop (sprite/animation tools, music/sfx tooling, packaging).

For programmatic control, `console-agent serve` speaks JSON-RPC 2.0 (one
request per line on stdin/stdout): load a cart, step frames with input,
pull a screenshot or the framebuffer as text (`screen_text`), `eval` Lua,
and inspect audio without ears (`audio_state`, `audio_events`,
`audio_stats`, `spectrogram`). Full method list in SPEC.md.

## Development

This repo uses a bones/maw workflow for changes — see
[AGENTS.md](AGENTS.md). Before sending a change, run:

```bash
cargo test --workspace
node web/smoke.cjs   # cross-checks the wasm engine build against native
```

## Status

Early and in active development — expect breaking changes to the cart
format and API. No license has been chosen yet; there is no LICENSE file in
this repo.

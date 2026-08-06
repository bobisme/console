# Building the web engine (console-web → engine.js)

Verified recipe (emsdk 6.0.5, rustc 1.97.0). Toolchain lives in `~/emsdk`
(user-level install); Rust target `wasm32-unknown-emscripten` must be added
(`rustup target add wasm32-unknown-emscripten`).

Just run the script — it is the recipe below, wrapped:

```bash
./web/build-engine.sh          # -> web/engine.js
EMSDK_DIR=/opt/emsdk ./web/build-engine.sh
```

Equivalent one-liner:

```bash
bash -c 'source ~/emsdk/emsdk_env.sh && \
  CFLAGS_wasm32_unknown_emscripten="-fwasm-exceptions" \
  RUSTFLAGS="-C link-arg=-sSINGLE_FILE=1 \
-C link-arg=-sSINGLE_FILE_BINARY_ENCODE=0 \
-C link-arg=-sMODULARIZE=1 \
-C link-arg=-sEXPORT_NAME=ConsoleEngine \
-C link-arg=-sALLOW_MEMORY_GROWTH=1 \
-C link-arg=-sEXPORTED_FUNCTIONS=_main,_con_alloc,_con_free,_con_init,_con_step,_con_fb,_con_width,_con_height,_con_audio,_con_color_count,_con_palette,_con_dpal,_con_error \
-C link-arg=-sEXPORTED_RUNTIME_METHODS=cwrap,UTF8ToString,HEAPU8" \
  cargo build -p console-web --target wasm32-unknown-emscripten --release'
```

Why each piece matters:

- `source ~/emsdk/emsdk_env.sh` — puts `emcc` on PATH; must be in the same
  shell that runs cargo.
- `CFLAGS_wasm32_unknown_emscripten="-fwasm-exceptions"` — **required.**
  rustc links this target with wasm exception handling, but the `cc` crate
  compiles mlua's vendored Lua C with plain emcc defaults, emitting legacy
  JS-longjmp calls → `undefined symbol: emscripten_longjmp` at link time.
  This env var makes `cc`-rs pass the flag to every vendored C compile.
- `SINGLE_FILE=1` inlines the wasm into the JS (no side `.wasm` file).
- `SINGLE_FILE_BINARY_ENCODE=0` — **required for the single-file HTML.**
  Since emscripten 4.x, `SINGLE_FILE` defaults to embedding the wasm as raw
  bytes in a JS string literal (`binaryDecode('\0asm…')`) rather than base64.
  That string contains NUL and non-ASCII bytes: inside an HTML `<script>` the
  parser rewrites every NUL to U+FFFD, and the whole page stops being plain
  text that a human or agent can edit (the spec requires the cart to stay
  editable inside `game.html`). `=0` restores base64. Cost: engine.js grows
  from ~623 KB to ~747 KB; the file is then pure ASCII.
- `MODULARIZE=1` + `EXPORT_NAME=ConsoleEngine` wraps it as a `ConsoleEngine()`
  async factory, which is what `web/template.html` calls.
- `ALLOW_MEMORY_GROWTH=1` — the cart buffer and Lua heap are allocated at
  runtime; without growth a large cart can hit the fixed 16 MB default.
- `EXPORTED_FUNCTIONS` — the `con_*` C ABI (see `SPEC.md`). `_main` must be
  listed too: naming the list overrides emcc's default, and this is an
  executable. Leading underscores are the C-symbol convention emcc expects.
  `_con_audio` (PoC v1) is part of that list: omit it and the shell silently
  falls back to a mute console — `web/template.html` feature-detects
  `Module["_con_audio"]` rather than failing, so a missing export costs you
  sound with no error message. `web/smoke.cjs` asserts it is present.
- `EXPORTED_RUNTIME_METHODS=cwrap,UTF8ToString,HEAPU8` — the JS-side helpers
  `web/template.html` uses. Without these they are stripped in a release build
  and the shell dies with `Module.cwrap is not a function`.
- The default `ENVIRONMENT` (web + worker + node) is kept deliberately so
  `web/smoke.cjs` can drive the same file under Node.

Output lands at
`target/wasm32-unknown-emscripten/release/console-web.js` (name follows the
crate/bin name); `build-engine.sh` copies it to `web/engine.js`.
The `console` build embeds that file and `web/template.html`; Cargo tracks both
inputs, so the next build/run refreshes the built-in packer assets. Run
`just install` again to refresh an already-installed `console`.

Gotcha: cargo may not notice RUSTFLAGS/EMCC_CFLAGS changes in its fingerprint —
if flags change and output looks stale, `touch` a source file or
`cargo clean -p console-web --target wasm32-unknown-emscripten` first.

Smoke test without a browser:

```bash
node web/smoke.cjs           # loads web/engine.js, runs carts/demo.cart 120 frames
```

Besides the C ABI and framebuffer checks, it cross-checks the synth: a fresh
console stepped 120 frames with input 0 must hash to `DEMO_AUDIO_GOLDEN` from
`crates/console-core/tests/audio.rs`, i.e. the wasm build renders **bit-identical
f32 samples** to the native build. Beware that console-core's test hasher
multiplies by `0x1000_0000_01b3`, which is 2^44 + 0x1b3 and *not* the canonical
FNV-1a-64 prime `0x100000001b3`; `smoke.cjs` mirrors that multiplier
(`CORE_HASH_PRIME`) so the constants line up. The two primes agree in the low 40
bits, so a mismatch shows up only in the top three nibbles of the hash.

The smoke also renders `print(..., "center")` and `print(..., "right")` through
the committed WASM engine and compares them with equivalent legacy-left calls.
This is an intentional engine-freshness tripwire: if the Rust API changes but
`web/engine.js` is not rebuilt, `just check` must fail before stale behavior is
packed into a cart.

To run the same structural WASM gate against an authored cart without comparing
it to `demo.cart`'s hashes, select the cart explicitly:

```bash
node web/smoke.cjs --cart carts/lantern-leap.cart
node web/smoke.cjs --cart carts/lantern-leap.cart --frames 180 --input-mask 16 --expect-audio
```

`--cart` implies generic mode. It checks that the cart loads, steps for multiple
frames without a surfaced runtime error, produces a non-uniform framebuffer
whose palette indices stay valid, keeps the framebuffer and audio pointers
stable, and emits only finite audio samples in `[-1, 1]`. `--expect-audio`
additionally requires at least one nonzero sample. The synthetic display-palette
and failing-cart probes still run, so generic mode also validates `con_dpal` and
`con_error`; only the demo-specific framebuffer/audio hashes and animation
comparison are skipped. `--input-mask` holds a numeric seven-button mask on
every stepped frame (for example, `16` is A), which is useful for starting a
title-screen game before asserting audio. Use `--engine PATH` to exercise a
different engine build, or `node web/smoke.cjs --help` for the full syntax.

Then pack the result:

```bash
cargo run -p console -- pack carts/demo.cart -o dist/demo.html
```

Packed pages expose a frozen read-only diagnostic handle immediately, even
while the engine is still booting:

```js
window.__console.status()      // lifecycle, frames, input and runtime telemetry
window.__console.screenState() // dimensions, framebuffer hash/colors, dpal
window.__console.audioState()  // context, pipeline, frames, nonzero evidence
```

Each call returns a frozen snapshot. The handle deliberately has no reset,
step, eval, Module, or heap access; browser checks must drive real keyboard,
pointer, and menu UI paths.

The status snapshot also reports read-only `rafCallbacks`, cumulative
`stepWallMs`, `maxStepBatchMs`, whole `droppedSimulationFrames`, and current,
peak, and growth-event counts for committed WASM linear memory. A discarded
frame is counted only when a complete fixed-timestep step remains beyond the
loop's four-step catch-up cap. Fractional accumulator time is not a frame. These
measurements diagnose packed-cart load; they do not grant access to the Module
or turn host-dependent timing into a pass threshold.

Fault containment has a real-browser regression (not part of the portable
`just check` gate because Chromium is an explicit prerequisite):

```bash
CONSOLE_BROWSER=/path/to/chromium just browser-diagnostics
```

The command packs Lantern Leap, injects a throwing canvas render dependency,
and requires diagnostics to transition to a latched `failed` state.

The combined packed-page acceptance gate needs `agent-browser` and an explicit
Chromium executable. Provision both first (for example, `agent-browser install`
plus a system Chromium), then run:

```bash
CONSOLE_BROWSER=/path/to/chromium just browser-check
```

This first runs the sustained Mirelight Survivors load gate, then packs Lantern
Leap to a temporary HTML file and opens that exact file over `file://`. Lantern
Leap requires a healthy boot and advancing 192x320 framebuffer, the
64-color/display-palette invariants, changing raw framebuffer and rendered
canvas pixels, trusted held pointer input, exact rising-edge touch haptic
requests for D-pad/A/B/game-menu/device-menu, a safe unsupported-vibration
fallback, audio unlock with nonzero samples, pause/resume and RESET through the
visible controls, and a network log limited to the exact `file://` document plus
in-memory worklet URLs. It also requires no browser page errors. Missing browser
infrastructure is an error, never a skipped check. These gates are intentionally
separate from portable `just check`.

Mirelight's gate uses trusted B then A pointer presses to select and start its
dense mode, then requires at least 600 successful fixed-timestep frames. It
samples the framebuffer at the start, midpoint, and end; rejects shell/cart,
page, console, palette, framebuffer, canvas, and external-network failures; and
captures effective FPS, step cost, discarded simulation frames, and committed
WASM memory. The script owns an isolated Chromium profile and connects directly
through CDP using Node's built-in WebSocket, so this sustained gate does not
depend on `agent-browser`. The 120-second timeout is only a hang watchdog.
Observed FPS, batch cost, backlog drops, and memory growth are retained as
evidence rather than compared with host-speed thresholds. Run it alone with:

```bash
CONSOLE_BROWSER=/path/to/chromium just browser-load-check
```

On a Lantern Leap failure the gate retains the packed page, screenshot,
diagnostic snapshots, network requests, page errors, and console messages in a
timestamped directory under `out/browser-check/`. Mirelight always retains
`metrics.json` and `final.png` under `out/mirelight-browser-check/`; failures
instead retain `diagnostics.json`, the exact `packed.html`, and a failure
screenshot. Set `CONSOLE_BROWSER_ARTIFACTS=/other/directory` to change either
artifact root. To exercise already-packed compatible carts directly:

```bash
CONSOLE_BROWSER=/path/to/chromium \
  node web/browser-smoke.cjs game.html --artifacts out/browser-check

CONSOLE_BROWSER=/path/to/chromium \
  node web/mirelight-browser-smoke.cjs mirelight.html \
    --frames 600 --artifacts out/mirelight-browser-check
```

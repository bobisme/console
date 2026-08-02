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
-C link-arg=-sEXPORTED_FUNCTIONS=_main,_con_alloc,_con_free,_con_init,_con_step,_con_fb,_con_palette,_con_error \
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
- `EXPORTED_RUNTIME_METHODS=cwrap,UTF8ToString,HEAPU8` — the JS-side helpers
  `web/template.html` uses. Without these they are stripped in a release build
  and the shell dies with `Module.cwrap is not a function`.
- The default `ENVIRONMENT` (web + worker + node) is kept deliberately so
  `web/smoke.cjs` can drive the same file under Node.

Output lands at
`target/wasm32-unknown-emscripten/release/console-web.js` (name follows the
crate/bin name); `build-engine.sh` copies it to `web/engine.js`.
`console-pack` splices it into `web/template.html` at `{{ENGINE_JS}}`.

Gotcha: cargo may not notice RUSTFLAGS/EMCC_CFLAGS changes in its fingerprint —
if flags change and output looks stale, `touch` a source file or
`cargo clean -p console-web --target wasm32-unknown-emscripten` first.

Smoke test without a browser:

```bash
node web/smoke.cjs           # loads web/engine.js, runs carts/demo.cart 120 frames
```

Then pack and eyeball the result:

```bash
cargo run -p console-pack -- carts/demo.cart -o dist/demo.html
```

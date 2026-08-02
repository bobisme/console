# console

A PICO-8-style fantasy console for vertical phone play, built so **AI agents are
first-class game developers**: deterministic core, headless control harness,
text-native carts, and single-file HTML deployment.

- **Display**: 144×256 (9:16 portrait), fixed 16-color palette (Sweetie-16), 60 fps
- **Input**: d-pad + A/B (6 buttons) — virtual touch controls on phones, keyboard on desktop
- **Games**: Lua 5.4 carts in a plain-text format (code, sprites as hex grids)
- **Ships as**: one self-contained `game.html` — works from `file://`, cart text
  stays embedded and editable inside the HTML

See [SPEC.md](SPEC.md) for the full contract (API, cart format, determinism rules).

## Layout

| path | what |
|------|------|
| `crates/console-core` | the console: Lua VM, framebuffer, drawing API, cart parser. Pure, deterministic, no I/O. |
| `crates/console-agent` | headless harness for AI agents: oneshot CLI + JSON-RPC over stdio (step, screenshot, eval, save/load states) |
| `crates/console-web` | C ABI over the core, built for `wasm32-unknown-emscripten` |
| `crates/console-pack` | splices engine + cart into a single `game.html` |
| `web/` | HTML shell template, engine build script + recipe ([web/BUILD.md](web/BUILD.md)) |
| `carts/` | example carts |

## Quick start

```bash
cargo test                                   # core + harness tests

# headless: run the demo cart 90 frames, holding RIGHT for 30, screenshot it
cargo run -p console-agent -- run carts/demo.cart \
  --frames 90 --input "30:,30:R,30:" --screenshot /tmp/frame90.png

# interactive JSON-RPC session (one request per line on stdin)
cargo run -p console-agent -- serve

# build the wasm engine (needs emsdk — see web/BUILD.md), then pack a game
./web/build-engine.sh
cargo run -p console-pack -- carts/demo.cart -o dist/demo.html
```

Open `dist/demo.html` in any browser (or send it to a phone) — that one file is
the whole game, and the cart source is still readable/editable inside it.

## Why agents can work on this

Same cart + same seed + same inputs ⇒ byte-identical frames, everywhere. An agent
develops entirely against `console-agent` (load cart → step frames with scripted
input → screenshot or `screen_text` → `eval` to inspect game state), and save
states are just replays, so any moment of gameplay is reproducible from
`(cart, seed, input log)`. Packing to HTML is a final, mechanical step.

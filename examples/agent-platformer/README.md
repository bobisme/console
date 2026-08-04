# Source Hopper

This compact platformer is the executable reference for a multi-file console
project. It deliberately uses nested Lua modules, four placed PNG assets,
authored animation metadata, a map, instruments, SFX, music, metadata, and a
deterministic playtest.

From the repository root:

```bash
console build examples/agent-platformer
console build examples/agent-platformer --check
console run examples/agent-platformer --frames 120 --input '30:,60:R,1:A,29:' \
  --screenshot /tmp/source-hopper.png --screenshot-zoom 2
console playtest examples/agent-platformer \
  --scenario examples/agent-platformer/playtest.json \
  --artifacts /tmp/source-hopper-playtest --format json
console serve examples/agent-platformer
console pack examples/agent-platformer -o /tmp/source-hopper.html
```

`build/source-hopper.cart` is generated and ignored. The other four commands
compile the source tree in memory and do not require that cart to exist.

Edit responsibilities are intentionally obvious:

- `lua/` contains gameplay modules; literal `require` names mirror paths.
- `art/` contains tile-aligned PNGs with explicit placements in `console.toml`.
- `data/` contains the map body and authored animation declarations.
- `audio/` contains reusable voices, sound effects, and the pattern list.
- `playtest.json` is the deterministic behavior and artifact gate.

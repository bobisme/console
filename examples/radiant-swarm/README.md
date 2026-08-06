# Radiant Swarm

A portrait bullet hell built as the vertical-slice proof for the console's
deterministic Lua ECS prototype. The game maintains stars, enemies, player and
hostile bullets, pickups, and particles in one named ECS world. Dense patterns
reach hundreds of simultaneous entities without introducing an engine-side
scheduler: `_update` still owns system order explicitly.

Controls:

- D-pad: move.
- Hold A: fire and focus for precise movement.
- B: spend one nova bomb, clear hostile bullets, and damage enemies.

Run or serve the source project directly:

```bash
console run examples/radiant-swarm --frames 300 --input '1:A,299:RA' \
  --screenshot /tmp/radiant.png --screenshot-zoom 2 \
  --eval 'return dev_status()'
console serve examples/radiant-swarm
```

Inspect live ECS state through JSON-RPC without relying on cart globals:

```json
{"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"path":"examples/radiant-swarm","seed":11}}
{"jsonrpc":"2.0","id":2,"method":"step","params":{"frames":300,"input":"A"}}
{"jsonrpc":"2.0","id":3,"method":"ecs_query","params":{"world":"arena","with":["hostile"],"select":{"pos":["x","y"],"hostile":["kind","radius","grazed"]},"limit":32}}
```

The `dev_stress()` hook starts an invulnerable deterministic stress pattern;
`playtest.json` uses it for repeatable entity-count, capacity, motion, visual,
and audio evidence.

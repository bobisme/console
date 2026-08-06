# Radiant Swarm

A portrait bullet hell built as the vertical-slice proof for the console's
deterministic Lua ECS prototype. The game maintains stars, enemies, player and
hostile bullets, pickups, and particles in one named ECS world. Dense patterns
reach hundreds of simultaneous entities without introducing an engine-side
scheduler: `_update` still owns system order explicitly.

Encounters arrive as named formations rather than isolated random spawns. Each
enemy family telegraphs its larger release, while the Choir boss develops
through three distinct phases with a brief bullet-clearing transition between
them. Graze chains reward staying close, and novas, pickups, damage, and phase
changes have separate visual and synthesizer feedback without hiding the
collision field.

Controls:

- D-pad: move.
- Hold A: fire and focus for precise movement.
- B: spend one nova bomb, clear hostile bullets, and damage enemies.

Run or serve the source project directly:

```bash
console run examples/radiant-swarm --frames 300 --input '1:A,299:RA' \
  --screenshot /tmp/radiant.png --screenshot-zoom 2 \
  --hook-after status
console run examples/radiant-swarm --frames 180 --input '180:A' \
  --hook-before stress --hook-after status
console hooks examples/radiant-swarm
console serve examples/radiant-swarm
```

Inspect live ECS state through JSON-RPC without relying on cart globals:

```json
{"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"path":"examples/radiant-swarm","seed":11}}
{"jsonrpc":"2.0","id":2,"method":"step","params":{"frames":300,"input":"A"}}
{"jsonrpc":"2.0","id":3,"method":"ecs_query","params":{"world":"arena","with":["hostile"],"select":{"pos":["x","y"],"hostile":["kind","radius","grazed"]},"limit":32}}
```

For population changes, define the projection once and sample selected frames:

```json
{"jsonrpc":"2.0","id":4,"method":"ecs_watch_define","params":{"name":"bullets","world":"arena","with":["hostile"],"select":{"hostile":["kind"],"pos":["x","y"]},"limit":128}}
{"jsonrpc":"2.0","id":5,"method":"step","params":{"frames":180,"input":"A","watches":["bullets"]}}
{"jsonrpc":"2.0","id":6,"method":"step","params":{"frames":1,"input":"B","watches":["bullets"]}}
```

`watch-playtest.json` is the compact regression for enemy-wave arrival, bullet
growth, and nova despawns.

The registered `stress` hook starts an invulnerable deterministic stress pattern;
`playtest.json` uses it for repeatable entity-count, capacity, motion, visual,
and audio evidence. The `status` hook also exposes the active formation, boss
phase/transition, graze chain, and short feedback timers for exact assertions.

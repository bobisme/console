# RIBBIT RECOIL compiled environment subset

This small project is the checked-in vertical slice for `console scene
compile`. It proves the replacement boundary for five production RIBBIT RECOIL
material cells with an ordinary exact Apollo64 PNG, a semantic grid, a seeded
layout, an autotile family, a weighted variant family, a metatile stamp, a
manual override, and two production object anchors. It deliberately does not
claim that the full production cart's richer topology substitution or legacy
atlas builder has been retired yet.

Compile and inspect it from the repository root:

```bash
console scene compile carts/ribbit-recoil-scene/scene.toml \
  --out carts/ribbit-recoil-scene/generated --format json
console build carts/ribbit-recoil-scene
console playtest carts/ribbit-recoil-scene \
  --scenario carts/ribbit-recoil-scene/playtest.json \
  --artifacts /tmp/ribbit-scene --format json
```

`generated/atlas.png`, `map.txt`, and the three Lua modules are normal Console
project inputs. There is no scene runtime dependency and no Paintop dependency.
The compiler also emits provenance plus labeled atlas, live-shape, 3x3 repeat,
used-adjacency, collision, and native-map review sheets. Generated outputs are
ignored; byte-stability is verified by the integration test.
The same test compares every named compiled tile's exact pixels and semantic
class, plus the frog and first mutation anchors, against the production cart.

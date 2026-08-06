# Mirelight Survivors

Mirelight Survivors is a complete, short survivors-like game and the console's
high-density ECS reference cart. A normal hunt ramps from 40 to 820 enemies
over a 75-second run. The title-screen density challenge begins at 820 enemies
and remains playable while projectiles, XP dew, and particles churn around the
player.

The live-world contract is deliberate rather than a screenshot trick:

- 820 enemies in the warmed challenge;
- at least 64 continuously replenished projectiles;
- live pickups and particles with spawn/despawn churn;
- 800–1000 total live entities at every measured stress frame;
- a 997-entity game ceiling inside an ECS world with capacity 1100; and
- zero dropped spawn requests in the deterministic acceptance trace.

If a runtime change cannot sustain that contract, fix or report the runtime
bottleneck. Do not lower the population target.

## Play

- D-pad: move.
- A: dash in the held direction.
- B: emit a damaging root pulse when it is ready.
- Automatic seed fire targets the nearest creature.
- At level-up, Up/Down selects an upgrade and A accepts it.

From the title, B toggles the dense-load selector and A begins the selected
hunt. The invulnerable `DENSE` challenge is the packed-browser test path; it
calls the same game setup used by the registered `stress` hook.

```bash
console serve examples/mirelight-survivors
console run examples/mirelight-survivors --frames 300 \
  --input '75:R,75:D,75:L,75:U' --seed 37 \
  --hook-before stress --hook-after status \
  --screenshot /tmp/mirelight.png --screenshot-zoom 2
```

## Spatial collision design

The 384×512 arena is divided into a 24×32 grid of 16-pixel cells. Two grids
are preallocated and swapped each frame. Enemy movement reads aggregate crowd
pressure from the previous grid while writing positions into the next one.
Projectiles then inspect only overlapping neighbor cells. Dense ID buffers,
grid buckets, and effect queues are reused rather than reconstructed.

Telemetry accumulates both spatial candidates and the naive
`enemy_count × projectile_count` pair count. The reference benchmark saw
133,108 spatial candidates versus 36,908,200 naive pairs over its complete
720-frame run. `spatial_reduction` is true only when candidates remain below
10% of naive pairs.

## Developer hooks and bounded watches

Discover registered metadata with `console hooks examples/mirelight-survivors`.

| hook | phase | purpose |
|---|---|---|
| `status` | post-frame | bounded gameplay, population, collision, and churn telemetry |
| `start` | pre-frame | begin a normal run |
| `stress` | pre-frame | begin the deterministic dense challenge |
| `grant_xp` | post-frame | exercise ordinary XP and level-up logic |
| `damage_player` | post-frame | exercise ordinary damage and death logic |
| `enter_finale` | post-frame | enter the ordinary final-wave transition |

`watch-playtest.json` defines `population`, `enemies`, `projectiles`,
`pickups`, and `particles` watches. The population watch uses global component
counts for exact category totals. The 820-enemy projection is intentionally
truncated to 128 entities, so its numeric matched count is exact while its
returned-ID membership is explicitly incomplete. The other categories fit
inside their watch bounds and produce complete churn IDs.

```bash
console playtest examples/mirelight-survivors \
  --scenario examples/mirelight-survivors/playtest.json \
  --artifacts /tmp/mirelight-playtest
console playtest examples/mirelight-survivors \
  --scenario examples/mirelight-survivors/stress-playtest.json \
  --artifacts /tmp/mirelight-stress
console playtest examples/mirelight-survivors \
  --scenario examples/mirelight-survivors/watch-playtest.json
```

## Native ceiling benchmark

The ignored integration test measures 120 warmup frames followed by 600
one-frame samples. Its counting global allocator surrounds only the measured
`Session::step` calls. The report includes mean/p50/p95/max frame cost,
allocation/deallocation calls and bytes, peak outstanding bytes above the
baseline, and the final semantic game telemetry.

```bash
cargo test --release -p console --test mirelight_survivors \
  benchmark_stress_window_emits_timing_allocation_and_churn_json \
  -- --ignored --exact --nocapture

# Optional same-machine regression gate:
CONSOLE_MIRELIGHT_MAX_P95_MS=6 cargo test --release -p console \
  --test mirelight_survivors \
  benchmark_stress_window_emits_timing_allocation_and_churn_json \
  -- --ignored --exact --nocapture
```

Reference observations on 2026-08-06, Linux x86_64, Rust 1.97.0, AMD Ryzen
Threadripper 9970X, release profile. The before and after runs used the same
seed, input trace, 120-frame warmup, 600-frame measurement, and 820-enemy
population contract; only allocation-efficient `world:each` internals changed.

| metric | before `bn-2go` | after `bn-2go` | change |
|---|---:|---:|---:|
| mean frame | 3.386579 ms | 2.804133 ms | -17.2% |
| p50 frame | 3.166172 ms | 2.754060 ms | -13.0% |
| p95 frame | 4.470705 ms | 3.262965 ms | -27.0% |
| max frame | 5.846865 ms | 4.307244 ms | -26.3% |
| allocation calls / 600 frames | 7,462,943 | 5,056,099 | -32.3% |
| allocated bytes / 600 frames | 453,538,832 | 326,921,874 | -27.9% |
| peak bytes above baseline | 3,664,819 | 4,290,717 | allocator-noisy |
| final live / minimum live / peak live | 961 / 931 / 966 | 961 / 931 / 966 | identical |
| spawned / despawned | 7,824 / 6,863 | 7,824 / 6,863 | identical |
| dropped spawns | 0 | 0 | identical |

`world:each` now scans the structurally stable creation order directly and
passes 0–16 requested components without building a selected-ID table or one
argument table per entity. The same-game result removes 2,406,844 allocation
calls and 126,616,958 allocated bytes from the measured window while preserving
framebuffer/gameplay determinism. The remaining roughly 8,427 allocation calls
and 545 KB per frame leave room for later profiling, but the targeted query
tables are no longer part of the hot path.

Timing is reported rather than universally gated because shared and lower-end
machines have different envelopes; use the optional threshold for controlled
hosts. The peak-outstanding observation is retained honestly but is not a
stable improvement metric: the process-wide allocator counter includes Lua GC
timing and allocations retained across the measurement boundary.

## Packed browser check

Pack the exact same source project, run the generic shell acceptance, then
drive the visible B-toggle/A-start path in the resulting single file:

```bash
console pack examples/mirelight-survivors -o /tmp/mirelight.html
CONSOLE_BROWSER=/path/to/chromium node web/browser-smoke.cjs /tmp/mirelight.html
```

The persistent `DENSE` HUD marker distinguishes the browser stress path. A
browser check proves the packed WASM remains healthy under real-time load;
native hooks and watches provide the exact population evidence because the
browser diagnostic handle intentionally exposes only frozen shell snapshots.

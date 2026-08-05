# Multi-file project workflow

Use this guide when creating a `console.toml` source tree or migrating a
monolithic cart. Read the domain references only when their detail is needed.
The repository's full operational guide is
[`docs/project-workflow.md`](../../../docs/project-workflow.md), and the
executable reference is
[`examples/agent-platformer`](../../../examples/agent-platformer).

## Contents

- [Choose the project form](#choose-the-project-form)
- [Agent build loop](#agent-build-loop)
- [Ownership rules](#ownership-rules)
- [Migration checklist](#migration-checklist)
- [What to read next](#what-to-read-next)

## Choose the project form

Prefer a project when Lua, art, maps, or audio should have independent files or
when PNG conversion belongs in a reproducible build. Keep a standalone cart for
small experiments and low-level `sprite`, `map`, or `music` mutation commands.
The runtime artifact is identical either way.

A useful layout is `lua/`, `art/`, `data/`, `audio/`, `playtest.json`, and an
ignored `build/`, all rooted beside `console.toml`. Start from
`examples/agent-platformer` rather than inventing manifest syntax.

## Agent build loop

1. Edit the smallest owning source file.
2. Run `console build PROJECT --format json` and inspect Lua/PNG provenance.
3. Run a short `console run PROJECT` state and screenshot check.
4. Run `console playtest PROJECT --scenario PROJECT/playtest.json`.
5. Run `console build PROJECT --check` when the generated cart is versioned.
6. Pack and browser-test the project before delivery.

`run`, `playtest`, `pack`, and `serve` compile the directory or explicit
`console.toml` in memory. `serve` recompiles every GET/HEAD and never serves a
stale bundle after an invalid edit.

## Ownership rules

- `[cart]` and `[cart.meta]` generate metadata.
- `[lua]` names one entry and root; literal dot-separated `require` names map to
  files below that root. No dynamic loader or filesystem is exposed at runtime.
- Each `[[sprites]]` names one tile-aligned PNG and an explicit nonoverlapping
  sheet placement. Exact mapping is the safe default; nearest and quantize are
  opt-in lossy conversions.
- Alternatively, `[sections].sprites` points at a lossless raw sheet body.
  Never combine it with `[[sprites]]`.
- `[sections]` also points at headerless bodies for map, graphics metadata,
  instruments, SFX, music, or custom sections.
- Alternatively, `[audio].bundle = "audio/game.cmusic"` expands one versioned,
  directly playable native audio bundle into instruments, SFX, and music. Do
  not combine it with the three audio keys under `[sections]`.
- `[build].output` is generated. Put lasting changes in sources, not the cart.

Every input path is project-relative and confined after canonicalization. Do
not use `..`, escaping symlinks, embedded `__section__` headers, overlapping
assets, or both PNG assets and `[sections].sprites`.

## Migration checklist

- Move `__lua__` into the entry and return tables from extracted modules.
- Move each native section body into a file without its header.
- Or put the three audio sections behind a `console-music 1` header in one
  `.cmusic` file and register it as `[audio].bundle`.
- Preserve the raw sheet through `[sections].sprites`, or export/redraw
  tile-aligned PNG regions and register explicit placements.
- Preserve animation declarations in the graphics-metadata body.
- Build, then compare old and new carts under identical seeds and inputs.
- Add deterministic state assertions, screenshots, and audio evidence.
- Decide whether generated carts are committed; enforce that decision in CI.

## What to read next

- Read [platform and cart format](platform-and-cart-format.md#multi-file-projects)
  for the exact manifest, ordering, validation, and path contract.
- Read [Lua API](lua-api.md#sandbox-and-determinism) for static module and
  runtime rules.
- Read [sprites and animation](sprites-and-animation.md) for PNG preparation,
  palette budgets, declarations, and visual review.
- Read [maps and metatiles](maps-and-metatiles.md) and
  [music and SFX](music-and-sfx.md) for native section authoring.
- Read [testing and shipping](testing-and-shipping.md) for the full evidence and
  browser-release ladder.

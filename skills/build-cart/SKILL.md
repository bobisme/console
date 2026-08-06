---
name: build-cart
description: Build, modify, inspect, test, migrate, and package games for the console fantasy-console platform. Use when creating or editing a .cart file or console.toml multi-file project; using console build; writing gameplay or entity-heavy ECS logic in the console Lua API; authoring sprites, declared animations, tile maps, metatiles, instruments, SFX, or music; using console CLI or JSON-RPC tooling; debugging deterministic behavior, entities, pixels, input, or audio; or producing and validating a single-file HTML game with console pack.
---

# Build console carts

Author either a small text-native `.cart` or a `console.toml` project with
separate Lua, art, map, and audio sources. A project compiles into the same one
portable `.cart` artifact. Work through `console`; do not infer visual or audio
results from source alone. Render, inspect, assert, and replay them.

## Route into the references

Load only the references needed for the current task. Every reference is linked
directly from this file; do not recursively hunt through the skill.

| Need | Read |
|---|---|
| Platform limits, palette, buttons, cart sections, data alphabets | [references/platform-and-cart-format.md](references/platform-and-cart-format.md) |
| Set up a multi-file project or migrate a monolithic cart | [references/project-workflow.md](references/project-workflow.md) |
| Any console Lua function or sandbox behavior | [references/lua-api.md](references/lua-api.md) |
| Exact syntax for every `console` command or JSON-RPC method | [references/command-reference.md](references/command-reference.md) |
| Draw, revise, animate, and validate pixel art | [references/sprites-and-animation.md](references/sprites-and-animation.md) |
| Build tile sets, maps, collision, metatiles, variants, and scrolling rooms | [references/maps-and-metatiles.md](references/maps-and-metatiles.md) |
| Art direction, foreground/background readability, reference-driven evidence, and strict visual review | [references/visual-direction-and-review.md](references/visual-direction-and-review.md) |
| Compose instruments, SFX, songs, mixes, and inspect audio | [references/music-and-sfx.md](references/music-and-sfx.md) |
| Deterministic tests, playtest scenarios, browser checks, and HTML delivery | [references/testing-and-shipping.md](references/testing-and-shipping.md) |

For a new multi-file game, read the project workflow first, then Lua API, the
relevant art/audio guides, and testing/shipping. For a focused edit, load only
its domain guide plus the command reference when exact flags or RPC fields
matter.

## Respect the platform contract

- Target the fixed 192×320 logical display: 24×40 visible 8×8 cells.
- Use palette indices 0–63 from Apollo64. Keep source color 0 transparent for
  sprites and tile 0 empty on maps.
- Treat 8×8 as the addressable art unit, not the ideal size of every object.
  Favor roughly 16–24px silhouettes for primary actors on a phone display.
- Budget the fixed 128×128 sprite sheet: 256 tile IDs shared by actors, terrain,
  animation frames, and decorative variants.
- Keep execution deterministic: use `rnd`/`srand`, frame counters, stable numeric
  iteration, and input-driven state. Never use wall-clock or unordered `pairs`
  iteration where order affects state, pixels, or audio.
- Remember that a Lua error halts the cart. Treat load and runtime errors as hard
  failures, not visual glitches.

## Follow the authoring loop

1. Inspect the existing cart and declarations before editing.
2. Make one coherent change in Lua or one data section.
3. Reparse or run the cart immediately.
4. Inspect the appropriate evidence: screenshot/screen text, sprite views, map
   render/lint, score/piano roll, audio events/stats, WAV, or spectrogram.
5. Encode important interactions as a versioned `playtest` scenario.
6. Repeat with the same seed and inputs when checking determinism.
7. Pack the cart and validate the actual single-file page for delivery.

Start with a short deterministic smoke run:

```bash
console run game.cart --frames 120 --input '30:,20:R,10:RA,60:' \
  --screenshot /tmp/game-f120.png --screenshot-zoom 2 \
  --eval 'return {x=player.x,y=player.y}'
```

For a multi-file game, pass its directory or `console.toml` in place of
`game.cart`; `run`, `playtest`, `pack`, and `serve` compile it in memory. Keep
low-level sprite/map/music read-write commands pointed at standalone carts.

Input segments are `COUNT:BUTTONS`; buttons are `L R U D A B M`, and an empty
button field means idle. Inspect the PNG rather than merely checking that it was
created.

Use `console rpc` when repeated reloads would dominate iteration:

```text
{"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"path":"game.cart","seed":0}}
{"jsonrpc":"2.0","id":2,"method":"step","params":{"frames":60,"input":"R"}}
{"jsonrpc":"2.0","id":3,"method":"screenshot","params":{"path":"/tmp/f60.png","zoom":2}}
```

Use the CLI write commands between sessions, then call `load_cart` again. RPC
inspection methods never rewrite the cart file.

## Prefer semantic declarations

- Declare named sprites and animations in `__gfx_meta__`; play ordinary cycles
  with `aspr` instead of duplicating frame tables in Lua.
- Keep terrain in the 8×8 map vocabulary and define larger metatiles as an
  authoring/gameplay convention. The runtime has no hidden metatile layer.
- For layered environments, use `console scene compile` to turn semantic PNG
  layers, named tile families, metatiles, autotiles, variants, and objects into
  an ordinary atlas/map/Lua project input set. Review its labeled evidence;
  gameplay still consumes only the native runtime APIs.
- Define reusable timbres in `__instruments__`; let SFX rows name instruments
  and let `__music__` arrange SFX IDs into songs.
- Put stable developer hooks in the cart when they make deterministic testing
  easier, such as `dev_status()`, `dev_warp(x,y)`, or `dev_start()`.

## Use tools instead of brittle text surgery

- Use `sprite dump`/`poke` and `sprite edit` for pixel rows and transforms.
- Use `sprite export`/`import` to round-trip exact-size assets through PNG
  editors. Use `palette quantize` explicitly before import when source art is
  not already Apollo64; never expect import to resize or silently reduce it.
- In a `console.toml` project, prefer explicit `[[sprites]]` PNG placements so
  `console build` deterministically generates the sheet and named metadata.
  Keep exact mapping as the default; opt into nearest/quantize deliberately.
- Use `map dump`/`poke` and `map edit` for cell grids and regions.
- Use `scene compile ... --check` to gate deterministic generated environment
  assets; keep exact Apollo64 mapping unless lossy conversion is intentional.
- Use `music edit` and `music import-abc` instead of manually shifting tracker
  rows or respelling every note. Use `music midi-to-abc` and `music play` to
  inspect and audition source music before spending the cart's row budget.
- Keep lossless native arrangements in `.cmusic` when instruments, effects,
  and mix settings must travel with the notes. Register one as `[audio].bundle`
  for `console build`, and audition the file, cart, or project with `music play
  --song N`. When replacing audio in an existing game, reserve/remap gameplay
  SFX IDs and audit every `music(N)` call; the bundle and Lua share one numeric
  namespace. Build once before `--check`, then lint the generated cart and run
  an input trace that proves gameplay cues do not trigger music phrases.
- Run write commands with `--dry-run` first when changing a nontrivial region.
  They reparse before writing, but the preview keeps intent reviewable.
- Use `--help` as live syntax authority if the installed tool and this checked-out
  skill differ. In this repository, `SPEC.md` and the command source remain the
  normative implementation contract.

## Validate proportionally

For any gameplay change:

- Run scripted input through `run` or `playtest` and assert state transitions.
- Capture representative beginning, action, failure, and completion frames.
- Check the same seed/input twice when logic, randomness, or effects changed.

For authored art or maps:

- Render at a large inspection zoom and inspect at 1× phone scale.
- Run sprite/map lint and review animation strips or onion sheets.
- Verify collision and visible tile data agree; a beautiful map with mismatched
  collision metadata is still broken.
- When art direction or readability matters, build the native/grayscale/motion/
  layer/collision evidence bundle and use an independent blind review from
  [references/visual-direction-and-review.md](references/visual-direction-and-review.md).

For audio:

- Audition ABC/MIDI sources and native `.cmusic`/cart/project audio with `music
  play`; use `--dry-run` where no audio device is available.
- Read `music score`, run `music lint`, and inspect the piano roll.
- Verify the running sequence with `audio_events` and the mix with `audio_stats`.
- Render a WAV for a human listening pass when musical quality matters.

Before handoff, follow the complete acceptance sequence in
[references/testing-and-shipping.md](references/testing-and-shipping.md). Do not
call a packed page done based only on native execution.

## Maintain this skill

Keep detailed facts in one reference rather than copying them into `SKILL.md`.
Keep every reference one link away from this entrypoint and give long references
a table of contents. After changing platform APIs or tools, update the matching
reference and run:

```bash
python3 skills/build-cart/scripts/check_reference_coverage.py
python3 /home/bob/.codex/skills/.system/skill-creator/scripts/quick_validate.py \
  skills/build-cart
```

# Multi-file project workflow

Use a `console.toml` project as the editable form of a game and treat the
generated `.cart` as a portable distribution artifact. The compiler combines
separate Lua modules, PNG sprites, maps, animation metadata, instruments, SFX,
music, and optional custom sections deterministically.

The complete executable example is
[`examples/agent-platformer`](../examples/agent-platformer). The normative
manifest and compiler contract remains in
[`SPEC.md`](../SPEC.md#multi-file-project-compiler-console-build).

## Contents

- [Choose a source layout](#choose-a-source-layout)
- [Write the manifest](#write-the-manifest)
- [Split Lua into static modules](#split-lua-into-static-modules)
- [Import PNG sprite assets](#import-png-sprite-assets)
- [Compile layered scenes](#compile-layered-scenes)
- [Keep native sections separate](#keep-native-sections-separate)
- [Build and iterate](#build-and-iterate)
- [Migrate an existing cart](#migrate-an-existing-cart)
- [Add repository gates](#add-repository-gates)
- [Diagnose failures](#diagnose-failures)

## Choose a source layout

A practical project keeps ownership visible:

```text
my-game/
├── console.toml
├── lua/
│   ├── main.lua
│   ├── game/player.lua
│   └── ui/hud.lua
├── art/
│   ├── player.png
│   ├── terrain.png
│   └── terrain.semantic
├── scene.toml             # optional source for generated/
├── generated/             # atlas, map, Lua, review evidence
├── data/
│   ├── map.txt
│   └── gfx-meta.txt
├── audio/
│   └── game.cmusic         # or three headerless instruments/sfx/music files
├── playtest.json
└── build/                 # generated; normally ignored
```

Use a standalone cart for a tiny experiment or for the low-level
`sprite`/`map`/`music` mutation commands. Use a project when code or assets
benefit from independent review, ordinary editors, reusable modules, or a
repeatable asset build. Both forms produce the same runtime cart.

## Write the manifest

Manifest version 1 names every input explicitly:

```toml
manifest_version = 1

[cart]
title = "My Game"
author = "Agent"
version = "1"
preview_palette = [0, 1, 8, 9, 63]

[cart.meta]
genre = "platformer"

[lua]
entry = "lua/main.lua"
root = "lua"

[build]
output = "build/my-game.cart"

[audio]
bundle = "audio/game.cmusic"

[[sprites]]
name = "player"
source = "art/player.png"
tile = [2, 4]
anchor = [8, 15]
mapping = "exact"
alpha_threshold = 128
max_colors = 8

[sections]
map = "data/map.txt"
gfx_meta = "data/gfx-meta.txt"
```

All manifest input paths are relative to the project and must stay inside it,
including after symlink resolution. Section files contain only their body,
without an `__section__` header. `[cart]` owns `__meta__` and `[lua]` owns
`__lua__`. Repeated `[[sprites]]` entries generate `__sprites__`; alternatively,
`[sections].sprites` can preserve one complete text sheet body losslessly. Do
not configure both forms. Additional lowercase section names are allowed.

`[audio].bundle` points at a versioned `.cmusic` file containing the native
`__instruments__`, `__sfx__`, and `__music__` sections. It is the convenient
choice when the audio bank should be playable and shareable as one lossless
asset:

```text
console-music 1
__instruments__
inst lead wave=1 env=0,8,3 vib=12,3,2 echo=3
master drive=1 tone=1 hiss=0
echo delay=12 feedback=4 level=3
__sfx__
sfx 0 speed=auto
C4 lead 6 vib
E4 lead 6 arp4,7
__music__
bpm=120 rows_per_beat=4
pat 0 loop=0 : 0 - - -
```

The section bodies use exactly the cart audio grammar; the wrapper adds only a
format/version header. `console build` expands them into the three canonical
cart sections and includes the bundle in build provenance. Do not combine
`[audio].bundle` with `[sections].instruments`, `.sfx`, or `.music`. To keep
those sources independently editable instead, omit `[audio]` and retain the
three headerless `[sections]` mappings shown below:

```toml
[sections]
instruments = "audio/instruments.txt"
sfx = "audio/sfx.txt"
music = "audio/music.txt"
```

Audition either representation without writing the configured build output:

```bash
console music play audio/game.cmusic --song 0
console music play . --song 0 --seconds 10 --volume 0.35
console music play . --song 0 --dry-run
```

### Migrating a game's audio without breaking gameplay cues

When a new `.cmusic` bundle replaces the audio sections of an existing cart,
the bundle and Lua still share the same SFX ID namespace (0–63). Before
building, inventory every `sfx(...)` and `music(...)` call in the old Lua:

- reserve a documented range for gameplay cues after the music bank;
- remap every conditional, bomb, death, boss, and pause branch, not only the
  obvious literal calls;
- rewrite song calls to pattern IDs that actually exist in the new bundle, and
  keep `music(-1)` only where silence is intentional;
- use a generator or checked-in mapping table so extraction is deterministic.

Validate the integration in this order:

```bash
console music play my-game/audio/game.cmusic --song 0 --dry-run
console build my-game
console music play my-game --song 0 --dry-run
console music lint my-game/build/game.cart --strict
console run my-game --frames 120 --input '30:,20:R,10:RA,60:' \
  --audio-events --eval 'return dev_status()'
console build my-game --check
```

The first build creates the configured generated cart; `--check` is a second
pass and is expected to fail on a clean checkout if run before that build.
Keep generated `build/` output ignored when the source project and `.cmusic`
bundle are authoritative.

## Split Lua into static modules

Literal imports mirror paths beneath `[lua].root`:

```lua
-- lua/main.lua
local player = require("game.player")
local hud = require 'ui.hud'
```

Each reachable module executes once inside a private function scope. Its
return value is cached; a module with no return yields `true`. Module locals
remain private, while deliberate global assignments still enter the game
environment.

Keep imports static and literal. Dynamic calls, aliases of `require`, missing
modules, cycles, escaping paths, and duplicate canonical sources are build
errors. Module names contain dot-separated ASCII letters, digits, and
underscores. Identifiers beginning `__console_` are reserved for generated
loader state. The compiler syntax-checks every source independently and maps
errors back to its original file and line.

The bundle does not grant runtime filesystem access: global `require` and
`package` remain absent after compilation.

## Import PNG sprite assets

Repeat `[[sprites]]` for each tile-aligned image. Width and height must be
nonzero multiples of 8; `tile = [x, y]` is an explicit location on the 16x16
tile sheet. Placements may not overlap and must fit. Names are stable
`[a-z0-9_]+` identifiers, and manifest order never controls placement.

Choose conversion deliberately:

- `exact` is the default. Every opaque RGB value must already be Apollo64;
  transparent pixels use alpha, because opaque palette color 0 is ambiguous.
- `nearest` maps each opaque source pixel to the nearest Apollo64 color.
- `quantize` first reduces the image deterministically to `max_colors`, then
  maps the result. Its default budget is 16.

For `exact` and `nearest`, `max_colors` is a validation limit, not an implicit
conversion. Pixels below `alpha_threshold` become transparent; partial alpha
at or above it becomes opaque. Build reports expose dimensions, placement,
source/output color counts, final palette indices, alpha counts, and mapping
error so lossy choices can be reviewed.

The compiler generates the full sprite sheet plus one named `sprite`
declaration per asset. Put animation declarations that refer to those names in
the file selected by `[sections].gfx_meta`, for example:

```text
anim player.walk frames=2:4,3:4 fps=8 loop
```

The compiler does not resize or dither PNGs. Prepare dimensions in an image
editor, inspect the JSON build report, then render or play the compiled result.

## Compile layered scenes

Use `console scene compile` when an environment owns multiple tile-aligned PNG
layers, semantic collision classes, repeated structures, or seeded layout
families. The version-1 `scene.toml` is separate from `console.toml`: it turns
authoring data into normal project inputs and does not add a runtime subsystem.

```bash
console scene compile my-game/scene.toml --out my-game/generated --format json
console scene compile my-game/scene.toml --out my-game/generated --check
```

Point `console.toml` at `generated/atlas.png` with one `[[sprites]]` placement,
at `generated/map.txt` with `[sections].map`, and load the generated Lua modules
with literal `require` calls. The output also includes `provenance.json` and
labeled review images for the packed atlas, live shape, 3×3 repetition, used
adjacency, collision, and native map. Lossy nearest/quantized mappings add a
heatmap; exact mapping is the default.

The scene manifest declares atlas capacity and placement, semantic classes,
named layers and tiles, edge metadata, metatiles, four-neighbor autotile tables,
weighted deterministic variants, stamps, overrides, and anchored objects. All
paths remain confined to the manifest directory, source PNGs stay at native
resolution, and validation completes before any output is published. See the
[normative schema](../SPEC.md#layered-scene-compiler-console-scene-compile) and
the executable [RIBBIT RECOIL environment
subset](../carts/ribbit-recoil-scene).

## Keep native grammar lossless

The section body formats are unchanged from a cart:

- `sprites`: 128 palette characters per row when preserving a raw sheet instead
  of assembling named PNG assets;
- `map`: up to 64 rows of up to 128 two-digit hexadecimal tile IDs;
- `gfx_meta`: named `sprite` and `anim` declarations;
- `instruments`: reusable voices, wavetables, master processing, and echo;
- `sfx`: note rows grouped by numeric effect ID;
- `music`: tempo plus patterns that schedule SFX IDs.

These bodies may live in independent `[sections]` files or the three audio
bodies may share a versioned `[audio].bundle`. The compiler emits native
sections in canonical order and unknown sections in name order. It normalizes
line endings, adds one final newline per section, parses the complete result,
and only then publishes it.

## Build and iterate

Write the configured output atomically:

```bash
console build my-game
console build my-game --format json
console build my-game --check
```

`--check` never writes. It succeeds only when the existing output is
byte-identical, which makes it suitable for CI. `-o out.cart` overrides the
configured destination. Repeated builds of unchanged inputs produce identical
bytes and the same content ID.

The normal gameplay and delivery commands accept either the directory or its
explicit manifest and compile in memory:

```bash
console run my-game --frames 120 --input '30:,60:R,1:A,29:' \
  --screenshot /tmp/frame.png --screenshot-zoom 2 \
  --eval 'return dev_status()'
console playtest my-game --scenario my-game/playtest.json \
  --artifacts /tmp/playtest --format json
console pack my-game -o dist/game.html
console serve my-game
```

These commands do not require or rewrite `[build].output`. `serve` recompiles
and repacks every GET and HEAD request; an invalid edit returns an error instead
of an old playable page. Low-level mutation commands remain cart-only, so build
the cart first when using them.

## Migrate an existing cart

1. Create the source layout and copy the body of `__lua__` to `lua/main.lua`.
2. Move cohesive Lua tables into modules, return their public table, and replace
   the old inline definitions with literal `require` calls.
3. Copy each native section body, without its header, into its matching data
   file and register it under `[sections]`. For audio, either do the same or
   preserve the three headers behind `console-music 1` in a `.cmusic` file and
   register `[audio].bundle`.
4. For a byte-oriented migration, copy the old `__sprites__` body to a file and
   register it as `[sections].sprites`. To adopt image tooling, instead export
   meaningful regions with `console sprite export` or prepare new tile-aligned
   PNGs, then give each asset an explicit nonoverlapping `[[sprites]]`
   placement. Do not mix the two forms. Preserve authored animation lines in
   `gfx-meta.txt`.
5. Encode title, author, version, preview palette, and custom metadata in
   `[cart]` and `[cart.meta]`.
6. Build once, inspect the report, and compare the compiled game against the old
   cart with the same seed and scripted input. Review screenshots and audio as
   well as state assertions.
7. Commit source inputs and either commit the generated cart as a release
   artifact or ignore `build/` and build it in release automation. Whichever
   policy you choose, enforce it consistently with `console build --check` or a
   fresh deterministic build.

Do not hand-edit the generated cart and expect the change to survive. Move a
useful edit back to its owning source file.

## Add repository gates

At minimum, automation should:

1. compile scene inputs twice and compare artifacts, when a scene manifest is
   present;
2. run `console scene compile --check` and `console build --check` when their
   generated outputs are committed;
3. parse and initialize the compiled cart;
4. execute a versioned playtest with exact state assertions;
5. verify representative PNG/audio artifacts exist and are deterministic;
6. pack the project and exercise the actual HTML in a browser before release.

The repository example is covered by a Rust integration test, so `just check`
detects broken source files, stale-output behavior, compilation drift, missing
native sections, and failures in run/playtest/pack.

## Diagnose failures

- Read the named source path and line first; Lua diagnostics are remapped out of
  generated bundle coordinates.
- Use `console build my-game --format json` to audit canonical inputs, source
  mappings, asset placements, palette choices, byte count, and content ID.
- A stale `--check` result means source and generated output differ; it does not
  rewrite either one.
- A PNG rejection is intentional: resize explicitly, choose a lossy mapping
  explicitly, fix alpha, move the tile rectangle, or adjust the color budget.
- If direct `run` or `serve` fails after an edit, fix the source. The commands
  will not fall back to an older generated cart.

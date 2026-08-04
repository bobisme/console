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
│   └── terrain.png
├── data/
│   ├── map.txt
│   └── gfx-meta.txt
├── audio/
│   ├── instruments.txt
│   ├── sfx.txt
│   └── music.txt
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
instruments = "audio/instruments.txt"
sfx = "audio/sfx.txt"
music = "audio/music.txt"
```

All manifest input paths are relative to the project and must stay inside it,
including after symlink resolution. Section files contain only their body,
without an `__section__` header. `[cart]` owns `__meta__` and `[lua]` owns
`__lua__`. Repeated `[[sprites]]` entries generate `__sprites__`; alternatively,
`[sections].sprites` can preserve one complete text sheet body losslessly. Do
not configure both forms. Additional lowercase section names are allowed.

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

## Keep native sections separate

The section body formats are unchanged from a cart:

- `sprites`: 128 palette characters per row when preserving a raw sheet instead
  of assembling named PNG assets;
- `map`: up to 64 rows of up to 128 two-digit hexadecimal tile IDs;
- `gfx_meta`: named `sprite` and `anim` declarations;
- `instruments`: reusable voices, wavetables, master processing, and echo;
- `sfx`: note rows grouped by numeric effect ID;
- `music`: tempo plus patterns that schedule SFX IDs.

The compiler emits native sections in canonical order and unknown sections in
name order. It normalizes line endings, adds one final newline per section,
parses the complete result, and only then publishes it.

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
3. Copy each native section body, without its header, into its matching data or
   audio file. Register those files under `[sections]`.
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

1. compile the project twice and compare bytes or content IDs;
2. run `console build --check` when generated carts are committed;
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

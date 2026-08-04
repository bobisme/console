# Platform and cart format

Use this reference when starting a cart, allocating art or map space, choosing
palette indices, or editing any cart data section.

## Contents

- [Platform constants](#platform-constants)
- [Input](#input)
- [Apollo64 palette](#apollo64-palette)
- [Multi-file projects](#multi-file-projects)
- [Minimal cart](#minimal-cart)
- [Section order and grammar](#section-order-and-grammar)
- [Metadata](#metadata)
- [Sprite sheet](#sprite-sheet)
- [Tile map](#tile-map)
- [Graphics metadata](#graphics-metadata)
- [Audio sections](#audio-sections)
- [Parsing and compatibility rules](#parsing-and-compatibility-rules)

## Platform constants

| Resource | Contract |
|---|---|
| Display | 192×320 logical pixels, fixed 3:5 portrait |
| Frame rate | 60 fixed updates per second; `_update()` then `_draw()` |
| Framebuffer | 61,440 bytes, row-major, one palette index per pixel |
| Palette | 64 fixed Apollo64 colors, indices 0–63 |
| Sprite sheet | 128×128 pixels; 16×16 addressable 8×8 tiles; 256 IDs |
| Tile map | 128×64 cells; each cell stores one sprite ID 0–255 |
| Visible tile area | 24×40 cells at 8×8 pixels |
| Audio | 44,100 Hz mono; 735 samples/frame; 6 channels |
| Notes | C0–B7, 96 semitones |
| SFX/music IDs | 0–63 |
| SFX rows | At most 32 per SFX; 1–255 frames per row |
| Buttons | 7: left, right, up, down, A, B, menu |

The web host preserves the 3:5 aspect ratio. Its default **FIT** mode may scale
fractionally; optional **SHARP** mode uses integer scaling. Logical coordinates
never change.

## Input

| Bit / Lua index | Button | CLI letter | Keyboard |
|---:|---|---|---|
| 0 | left | `L` | Left / A |
| 1 | right | `R` | Right / D |
| 2 | up | `U` | Up / W |
| 3 | down | `D` | Down / S |
| 4 | A | `A` | Z / J |
| 5 | B | `B` | X / K |
| 6 | game menu | `M` | Enter |

The game-menu button is cart input. It is distinct from the web shell's device
menu/pause control.

## Apollo64 palette

Sprite characters encode indices, not RGB. Use the palette as ramps: 0–7 blue/
cyan, 8–15 green, 16–23 brown/skin, 24–31 ochre, 32–39 red, 40–47 violet/pink,
and 48–63 neutrals. Build a small semantic ink set per scene instead of using
all 64 colors in every object.

| idx | hex | idx | hex | idx | hex | idx | hex |
|---:|:---|---:|:---|---:|:---|---:|:---|
| 0 | `#172038` | 16 | `#4d2b32` | 32 | `#241527` | 48 | `#090a14` |
| 1 | `#253a5e` | 17 | `#63393a` | 33 | `#411d31` | 49 | `#10141f` |
| 2 | `#3c5e8b` | 18 | `#7a4841` | 34 | `#5a2138` | 50 | `#151d28` |
| 3 | `#4576a3` | 19 | `#945f4c` | 35 | `#752438` | 51 | `#202e37` |
| 4 | `#4f8fba` | 20 | `#ad7757` | 36 | `#a53030` | 52 | `#2c3c43` |
| 5 | `#5fa7c7` | 21 | `#c09473` | 37 | `#ba4436` | 53 | `#394a50` |
| 6 | `#73bed3` | 22 | `#d7b594` | 38 | `#cf573c` | 54 | `#485e63` |
| 7 | `#a4dddb` | 23 | `#e7d5b3` | 39 | `#da863e` | 55 | `#577277` |
| 8 | `#19332d` | 24 | `#341c27` | 40 | `#1e1d39` | 56 | `#6c8486` |
| 9 | `#25562e` | 25 | `#602c2c` | 41 | `#402751` | 57 | `#819796` |
| 10 | `#336c32` | 26 | `#753a2d` | 42 | `#5b2f68` | 58 | `#94a6a4` |
| 11 | `#468232` | 27 | `#884b2b` | 43 | `#7a367b` | 59 | `#a8b5b2` |
| 12 | `#5d943b` | 28 | `#a4602c` | 44 | `#a23e8c` | 60 | `#b7c2bf` |
| 13 | `#75a743` | 29 | `#be772b` | 45 | `#c65197` | 61 | `#c7cfcc` |
| 14 | `#a8ca58` | 30 | `#de9e41` | 46 | `#d46b9d` | 62 | `#d9deda` |
| 15 | `#d0da91` | 31 | `#e8c170` | 47 | `#df84a5` | 63 | `#ebede9` |

The case-sensitive palette alphabet is:

```text
0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_
```

Character position equals palette index: `0` is 0, `a` is 10, `A` is 36,
`-` is 62, and `_` is 63.

## Multi-file projects

Use a `console.toml` project when the cart is large enough that Lua and data
should be reviewed independently. The command compiles normal source files into
the same portable cart format:

```toml
manifest_version = 1

[cart]
title = "My Game"
author = "Agent"
version = "1"

[cart.meta]
genre = "platformer"

[lua]
entry = "lua/main.lua"
root = "lua"

[build]
output = "build/game.cart"

[sections]
sprites = "sprites.txt"
map = "map.txt"
gfx_meta = "gfx-meta.txt"
instruments = "instruments.txt"
sfx = "sfx.txt"
music = "music.txt"
```

Run `console build my-game` to atomically write the configured output, or
`console build my-game --check` in CI to require byte-identical generated
content without writing. `-o`/`--out` overrides the destination. Build reports
support `--format text|pretty|json`.

Split Lua with literal imports:

```lua
local player = require("game.player") -- lua/game/player.lua
local hud = require 'ui.hud'           -- lua/ui/hud.lua
```

Only dot-separated ASCII names and literal calls are allowed. The compiler
follows reachable modules, rejects missing/dynamic/cyclic/aliased sources, and
emits each module as a private execute-once closure. A module return is cached;
no explicit return becomes `true`. The loader is lexical to the generated cart,
so runtime globals `require` and `package` remain absent and no host filesystem
capability is added. Build reports map generated Lua line ranges back to each
canonical source; syntax failures already name the original file and line.
Names beginning `__console_` are reserved for generated internals and fail the
build rather than risking a lexical collision.

Paths in the manifest are relative and cannot escape the project root through
`..` or symlinks. Every section source is UTF-8 and contains only the section
body, never its `__name__` header. `meta` and `lua` come from their typed tables;
other lowercase section names may be added under `[sections]`. The compiler
normalizes line endings, emits canonical section order, and reparses the whole
cart before replacing an output. The authoritative schema and error/ordering
contract are in the repository `SPEC.md`.

## Minimal cart

Only `__lua__` is required:

```cart
__meta__
title=Tiny cart
author=agent
version=1

__lua__
local x = 96

function _update()
  if btn(0) then x = x - 1 end
  if btn(1) then x = x + 1 end
end

function _draw()
  cls(48)
  circfill(x, 160, 6, 31)
  print("hello", 4, 4, 63)
end
```

Add optional data sections only when needed. A conventional readable order is:

```text
__meta__
__lua__
__sprites__
__map__
__gfx_meta__
__instruments__
__sfx__
__music__
```

## Section order and grammar

A section starts on a line containing `__name__`. Lua continues until the next
section header. In data sections, blank lines and lines beginning with `#` are
ignored. Preserve UTF-8 text and keep carts diff-friendly.

### `__meta__`

Use `key=value` lines. Recognized authoring keys include `title`, `author`,
`version`, and `preview_palette`; other metadata remains available through cart
metadata. `console pack` uses `title` for the HTML title.

### `__lua__`

Write sandboxed Lua 5.4. Define any of `_init`, `_update`, and `_draw`; all are
optional. See [lua-api.md](lua-api.md) for the complete console API and sandbox.

### `__sprites__`

Write up to 128 rows of up to 128 palette characters. Short and missing rows
are zero-filled. Pixel `(x,y)` is the character at column `x`, row `y`.
Addressable tile `n` starts at:

```text
tx = n % 16
ty = floor(n / 16)
pixel_x = tx * 8
pixel_y = ty * 8
```

Reserve tile 0 as blank by convention because map cell `00` means empty. Color
0 is the default transparent sprite ink.

### `__map__`

Write up to 64 rows of up to 128 cells. Each cell is exactly two hexadecimal
digits (`00`–`ff`) naming a sprite tile. Short/missing rows zero-fill. Blank and
comment lines do not consume map rows. Odd-length rows, non-hex digits, rows
wider than 128 cells, and more than 64 data rows are parse errors.

Map cell `00` is empty and `map()` skips it without reading sprite 0. Runtime
`mset` mutates the live map copy, not the cart text.

## Metadata

`preview_palette` is an optional comma-separated source-index to display-index
mapping:

```text
preview_palette=48,41,36,38,31,14,11,4
```

It accepts 1–64 decimal values in 0–63; the omitted tail maps identically.
Static sprite and map image tools apply it so compact semantic inks preview as
intended. It does not change Lua `pal`, raw dumps, lint, poke/edit operations,
the framebuffer, or `screen_text`. Source color 0 remains transparent in static
previews even if mapped elsewhere.

## Sprite sheet

The sheet is fixed capacity. Larger objects consume adjacent 8×8 tiles:

- a 16×16 actor uses a 2×2 tile rect;
- a 24×24 boss frame uses 3×3 tiles;
- a three-frame 16×24 walk can consume 18 tiles unless parts are reused.

Plan the sheet before drawing: group animation frames, reserve reusable terrain
families, and leave small gaps for variants. The display resolution does not
expand sheet capacity.

## Tile map

The runtime map always stores 8×8 tile IDs. Larger metatiles are a convention
implemented through arranged cells and Lua metadata; there is no second native
metatile grid. See [maps-and-metatiles.md](maps-and-metatiles.md) for patterns.

## Graphics metadata

Declare sprites and animations once for both tools and runtime:

```text
__gfx_meta__
sprite player rect=2,0 size=2x3 anchor=8,23
anim player.idle frames=0,1 fps=3 loop
anim player.walk frames=0,1,2,3 fps=10 loop frames_rect=2,3
anim player.hurt frames=10:8 fps=8
```

Grammar:

```text
sprite <name> rect=<tx>,<ty> size=<w>x<h> [anchor=<px>,<py>]
anim <sprite>.<label> frames=<frame,...> fps=<1-60> [loop] [frames_rect=<tx>,<ty>]
```

- Names match `[a-z0-9_]+` and are unique.
- Sprite rect/size use tile coordinates and must fit the 16×16 tile sheet.
- Default anchor is bottom-center: `(width_px/2, height_px-1)`.
- An integer frame displaces the chosen frame origin horizontally by the
  sprite width and wraps down by the sprite height.
- `frames_rect` changes the origin for integer entries in that animation.
- An explicit `tx:ty` frame directly pins the sprite-sized rect there and
  ignores `frames_rect`.
- Every resolved frame must fit the sheet. Forward sprite references within
  the complete section are validated after parsing.

## Audio sections

Use these top-level forms; see [music-and-sfx.md](music-and-sfx.md) for every
parameter and the composition workflow.

```text
__instruments__
wavetable <slot 0-7> <32 hex nibbles>
inst <name> wave=<0-7|w0-w7> [fm=...] [env=...] [vib=...] [trem=...]
  [sweep=...] [duck=...] [echo=...]
master drive=<0-8> [tone=<0-8>] [hiss=<0-4>]
echo delay=<1-60> feedback=<0-8> level=<0-8>

__sfx__
sfx <id 0-63> speed=<1-255|auto> [loop=<start>,<end>]
<NOTE|---> <WAVE|INST> <VOL 0-7> [FX]

__music__
bpm=<n> [rows_per_beat=<r>]
pat <id 0-63> [stop|loop=<id>] : <sfx|-> <sfx|-> <sfx|-> <sfx|-> [ch4] [ch5]
```

Write each `wavetable`, `inst`, `master`, `echo`, SFX header/row, tempo, and
pattern declaration on one physical cart line; wrapping above is explanatory.

Each pattern has 4–6 slots. Pattern duration is the longest selected SFX once;
SFX loop ranges are ignored during music playback.

## Parsing and compatibility rules

- Parse failures name the offending section/line and prevent a cart from
  loading. Do not depend on silent fallback.
- Unknown Lua globals are ordinary Lua behavior; unknown declared animation,
  SFX, pattern, instrument, or wavetable references are hard errors where the
  engine can validate them.
- Numeric draw colors are floored and masked to 0–63; map values are floored
  and masked to 0–255. Prefer valid explicit values rather than relying on wrap.
- Coordinate inputs are floored. Off-screen drawing is clipped; off-map `mget`
  returns 0 and off-map `mset` does nothing.
- `console` write commands preserve unrelated cart text and reparse the
  result before writing. Use `--dry-run` for reviewable changes.

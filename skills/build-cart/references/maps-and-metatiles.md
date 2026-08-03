# Maps and metatiles guide

Use this guide to design terrain tile families, assemble maps, represent larger
metatiles, keep collision consistent, add deterministic variation, and validate
scrolling rooms.

## Contents

- [Understand the native map](#understand-the-native-map)
- [Plan tile families](#plan-tile-families)
- [Build seamless terrain](#build-seamless-terrain)
- [Represent metatiles](#represent-metatiles)
- [Collision and properties](#collision-and-properties)
- [Author and transform maps](#author-and-transform-maps)
- [Variants and autotiling](#variants-and-autotiling)
- [Animation and dynamic terrain](#animation-and-dynamic-terrain)
- [Camera and room composition](#camera-and-room-composition)
- [Validation workflow](#validation-workflow)
- [Common failures](#common-failures)

## Understand the native map

The map is exactly 128×64 cells of 8×8 pixels. Each cell stores one sprite ID
0–255. Tile 0 is empty and skipped by `map()`. The visible display covers
24×40 cells; the full map covers 1024×512 world pixels.

There is no native collision flag, tile property table, metatile ID, layer, or
autotiler. Build those as explicit cart conventions. Keep them simple enough
that tools can still render the ground-truth 8×8 map.

`map()` shares the sprite draw path, so camera, clip, `pal`, `palt`, and
`preview_palette` behavior are predictable. `mget`/`mset` access the live map.

## Plan tile families

Allocate terrain by families rather than isolated attractive tiles. A useful
family for a solid material includes:

- interior/fill;
- top edge;
- bottom edge if visible;
- left/right edges;
- four outer corners;
- four inner corners;
- one or more cracks/tufts/pebbles;
- transition tiles to the neighboring material.

Keep IDs near one another and write a small manifest in Lua comments or
constants:

```lua
local tile = {
  empty=0x00,
  stone_fill=0x40,
  stone_top=0x41,
  stone_left=0x42,
  stone_right=0x43,
  stone_tl=0x44,
  stone_tr=0x45,
  grass_top_a=0x50,
  grass_top_b=0x51,
  spikes=0x60,
}
```

Direct named access is deterministic. Do not iterate this string-keyed table
with `pairs` to drive gameplay or rendering order.

## Build seamless terrain

For a repeating edge, opposite boundaries must agree pixel-for-pixel. Inspect
seams by rendering at least a 3×3 repetition, not one tile in isolation.

- Continue contours across tile boundaries at the same row/column.
- Avoid unique high-contrast pixels at an edge unless the adjacent tile
  continues them.
- Put variation inside the tile; preserve structural boundary pixels.
- Keep collision silhouettes simpler than decorative art.
- Use color/texture transitions to signal gameplay material changes.

Build a neutral base family first. Add variant tiles only after the base can
fill a room without visible seams.

## Represent metatiles

A metatile is an authoring/gameplay convention that expands into native cells.
For a 2×2 structure:

```lua
local meta = {
  stone_block = {
    w=2, h=2,
    tiles={0x44,0x41,
           0x42,0x40},
  },
  shrine = {
    w=3, h=3,
    tiles={0x70,0x71,0x72,
           0x80,0x81,0x82,
           0x90,0x91,0x92},
  },
}

local function stamp_meta(cx, cy, def)
  for j=0,def.h-1 do
    for i=0,def.w-1 do
      mset(cx+i, cy+j, def.tiles[j*def.w+i+1])
    end
  end
end
```

Prefer authoring static metatiles directly into `__map__` with `map poke`; use
`stamp_meta` for generated rooms, state changes, or deterministic setup.

Expansion loses the logical metatile identity: later `mget` sees component tile
IDs. If the game needs identity, use one of these explicit approaches:

- assign a distinctive origin/core tile and recognize the surrounding pattern;
- keep a numeric parallel room grid in Lua;
- store interactive objects separately and let the map remain presentation;
- stamp state variants from a known object record rather than reverse-parsing
  them from tiles.

For doors, chests, switches, and moving hazards, a separate object table is
usually clearer than encoding behavior into a decorative metatile.

## Collision and properties

Keep tile properties indexed by numeric tile ID:

```lua
local SOLID, HAZARD, ONEWAY = 1, 2, 3
local prop = {}

for id=0x40,0x4f do prop[id]=SOLID end
prop[0x50]=ONEWAY
prop[0x51]=ONEWAY
prop[0x60]=HAZARD

local function tile_at_px(x, y)
  return mget(flr(x/8), flr(y/8))
end

local function tile_prop_at_px(x, y)
  return prop[tile_at_px(x,y)] or 0
end
```

Test the actor's relevant corners/edge, not only its center. Resolve one axis at
a time to avoid diagonal tunneling:

```lua
local function solid_at(x, y)
  return (prop[tile_at_px(x,y)] or 0) == SOLID
end

local function move_x(p, dx)
  local step = dx < 0 and -1 or 1
  local pixels = flr(abs(dx))
  for n=1,pixels do
    local nx = p.x + step
    local edge = step < 0 and nx or nx + p.w - 1
    if solid_at(edge, p.y) or solid_at(edge, p.y+p.h-1) then
      p.vx = 0
      return
    end
    p.x = nx
  end
end
```

Adapt subpixel movement with an accumulator, but keep collision sampling and
resolution deterministic. Treat slopes as an explicit height table or object
system; do not assume a painted diagonal creates physical slope behavior.

Keep collision declarations adjacent to tile constants and add playtest asserts
for representative solids, hazards, one-way platforms, and empty decoration.

## Author and transform maps

Begin by inspecting the used extent:

```bash
console-agent map lint game.cart
console-agent map render game.cart --zoom 4 --grid --ids -o /tmp/map.png
console-agent map dump game.cart > /tmp/map.hex
```

Use `map poke` for exact region rows. Each row contains two hex digits per cell:

```bash
console-agent map poke game.cart 0,0,4,2 \
  --rows '44414145,42404043' --dry-run
console-agent map poke game.cart 0,0,4,2 \
  --rows '44414145,42404043'
```

Use transforms for structural changes:

- `copy` duplicates a room/platform region to a named destination origin;
- `shift` moves a region and zero-fills vacated cells;
- `fill` lays a repeated tile ID;
- `clear` restores `00`.

Every destructive `map edit` operation requires an explicit region. Keep it
tight so review diffs show only intended rows.

## Variants and autotiling

Use deterministic position-based variation so replays and reloads agree without
storing random results:

```lua
local function variant(cx, cy, count)
  return (cx*17 + cy*31 + 7) % count
end

local top_variants = {0x50,0x51,0x52}
local id = top_variants[variant(cx,cy,#top_variants)+1]
```

For autotiling, compute a four-neighbor mask in a fixed bit order and look up
the tile ID:

```lua
local function same_material(cx, cy)
  local id = mget(cx,cy)
  return id >= 0x40 and id <= 0x4f
end

local function neighbor_mask(cx, cy)
  local n = same_material(cx,cy-1) and 1 or 0
  local e = same_material(cx+1,cy) and 2 or 0
  local s = same_material(cx,cy+1) and 4 or 0
  local w = same_material(cx-1,cy) and 8 or 0
  return n+e+s+w
end
```

Define a 16-entry numeric lookup table. Run autotiling during generation or as
an authoring step, not every draw, unless terrain changes continuously. Inner
corners may require diagonal tests or a second overlay layer drawn manually.

## Animation and dynamic terrain

The native map stores one static tile ID per cell. Choose explicitly:

- cycle `mset` among tile variants based on a frame counter;
- leave the base cell empty and draw `aspr`/`spr` objects over `map()`;
- draw a second sparse list of animated coordinates;
- swap an entire metatile state for doors, broken blocks, or burned terrain.

Avoid mutating decorative tiles every frame when an overlay draw is simpler.
Use `mset` when the changed tile must also affect collision/lookups.

## Camera and room composition

The viewport is tall: showing 40 tile rows can expose hazards or goals earlier
than a narrower console. Compose intentional vertical reveals.

For a scrolling world:

```lua
local function clamp(v, lo, hi) return mid(lo, v, hi) end

function update_camera()
  local target_x = player.x - 96 + player.vx * 12
  local target_y = player.y - 190 + player.vy * 6
  cam_x = clamp(target_x, 0, 1024-192)
  cam_y = clamp(target_y, 0, 512-320)
end

function _draw()
  cls(48)
  camera(cam_x,cam_y)
  map()
  draw_objects()
  camera()
  draw_hud()
end
```

Use look-ahead modestly; too much reveals unloaded/undecorated map and makes
direction changes nauseating. For rooms smaller than the full map, clamp to the
room bounds rather than global maximums.

## Validation workflow

1. Run `map lint`; investigate every `blank_sprite_tiles` entry.
2. Render with `--grid --ids` to verify layout and IDs.
3. Render without labels to judge seams and composition.
4. Step gameplay and capture screen edges/camera extremes.
5. Assert collision against a known solid, empty decorative tile, hazard, and
   each special property.
6. Exercise dynamic terrain twice with the same seed/input and compare results.
7. Inspect packed phone scale; 8×8 noise can become illegible in motion.

`map lint` reports blank referenced tile IDs, not every cell coordinate. Locate
occurrences in the labeled render or hex dump before editing a tight region.

## Common failures

| Failure | Diagnosis / fix |
|---|---|
| Referenced tile is invisible | `map lint` reports a blank sheet tile or tile 0 was used. |
| Seams appear | Boundary pixels differ. Render repeated 3×3 blocks and repair the base family. |
| Collision disagrees with art | Property list omitted a variant/edge tile. Test every family ID. |
| Metatile partly changes | State transition updated only some component cells. Stamp from one object record. |
| Map tool edits too much | Region omitted/defaulted for poke or manually edited a full row. Use explicit small regions and dry-run. |
| Camera shows void | Clamp to authored room bounds and account for the 192×320 viewport. |
| Random decoration changes on replay | Replace sequential/global randomness with coordinate hashing or seeded generation. |
| Animated map costs complexity | Draw an object/overlay instead of rewriting cells unless lookup state must change. |

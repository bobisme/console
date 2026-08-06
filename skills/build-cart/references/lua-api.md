# Console Lua API

Use this reference whenever writing cart Lua. It documents every console-provided
global, callback order, persistent draw state, input, audio, determinism, and the
sandbox. Function arguments in square brackets are optional.

## Contents

- [Runtime and callbacks](#runtime-and-callbacks)
- [Entity component system](#entity-component-system)
- [Drawing primitives](#drawing-primitives)
- [Sprites and declared animations](#sprites-and-declared-animations)
- [Tile map](#tile-map)
- [Persistent draw state](#persistent-draw-state)
- [Frame effects and presentation order](#frame-effects-and-presentation-order)
- [Input](#input)
- [Time, random, and math](#time-random-and-math)
- [Audio](#audio)
- [Logging](#logging)
- [Sandbox and determinism](#sandbox-and-determinism)
- [Practical patterns](#practical-patterns)

## Runtime and callbacks

The cart runs Lua 5.4. Define any of these globals:

```lua
function _init()
  -- once, during cart load
end
function _update()
  -- once each fixed frame before _draw
end
function _draw()
  -- once each fixed frame after _update
end
```

One frame is always `_update()`, `_draw()`, framebuffer effects, then host
presentation. `t()` and animation timing derive from the 60 Hz frame counter.
A Lua error halts the cart permanently until reload/reset; the host exposes the
message and traceback.

All numeric draw coordinates are floored. Colors are floored and masked to the
low six bits (`0..63`). Optional booleans use Lua truthiness.

## Entity component system

Use the built-in `ecs` library when a game has many similarly processed
objects: bullets, particles, enemies, pickups, crowds, or effects. It is a
small deterministic Lua ECS owned by the console core, not an exposure of Bevy
ECS. There is no scheduler: call queries explicitly from `_update` and render
explicitly from `_draw`.

### Create and populate a world

```lua
local world=ecs.world("arena",{capacity=1200})
local player_id=world:spawn({
  pos={x=96,y=280},
  velocity={x=0,y=0},
  player=true,
})
```

`ecs.world(name,[options])` creates a uniquely named world. The default
capacity is 1024 entities; choose 1–4096. A cart may create at most 16 worlds.
World and component names are 1–64 bytes, start with a letter or `_`, and use
only letters, digits, `_`, `.`, or `-`. Entity IDs are monotonically increasing
integers and are never reused, even after `world:clear()`. A world registers at
most 128 distinct component names over its lifetime; an entity may contain at
most 128 components. Prefer a small stable component vocabulary.

The components argument is a string-keyed table. Its top level is copied on
spawn, while component values are retained. A component may be any non-nil Lua
value; mutable tables are conventional. Because nested values are retained by
reference, reusing the same component table in multiple `spawn` calls makes
those entities share it. Allocate a fresh mutable table per entity unless that
aliasing is intentional.

### World methods

| Method | Behavior |
|---|---|
| `world:name()` | Return this world's registered name. |
| `world:spawn(components)` | Create an entity and return its integer ID; error at capacity. |
| `world:despawn(id)` | Queue/remove a live entity and return true; false if absent/already queued. |
| `world:alive(id)` | Test current liveness. |
| `world:get(id,name)` | Return the component value/reference, or nil. |
| `world:has(id,name)` | Test component presence. |
| `world:add(id,name,value)` | Queue/add or replace a component; false for absent/queued entities. |
| `world:remove(id,name)` | Queue/remove a component; false for absent/queued entities. |
| `world:entities([with])` | Allocate matching IDs in creation order. |
| `world:each(with,callback)` | Visit matching entities in creation order as `callback(id, component...)`; return selected count. |
| `world:count([with])` | Count all live entities or those matching every filter. |
| `world:clear()` | Remove all entities; IDs remain monotonic. Forbidden during `each`. |
| `world:stats()` | Return `{name,alive,capacity,next_id,component_type_count,component_counts}`. |

Filters are dense arrays with at most 16 unique component names. An empty
filter selects every entity. `world:each({"pos","velocity"},fn)` passes values
in exactly that requested order:

```lua
world:each({"pos","velocity"},function(id,pos,velocity)
  pos.x=pos.x+velocity.x
  pos.y=pos.y+velocity.y
  if pos.y>340 then world:despawn(id) end
end)
```

Component-table field mutation is immediate. Structural calls (`spawn`,
`despawn`, `add`, `remove`) inside nested `each` calls are queued FIFO and
flush only after the outermost query. Consequently the selected entity set and
callback arguments stay valid, and new entities do not appear midway through a
query. `each` scans that stable order directly: it does not allocate a returned
ID list or a component-argument table for every entity. Prefer it for hot loops
and use `entities` when IDs must outlive the callback.

Until the outermost query flushes, `count()` and `stats()` exclude pending
spawns and include entities queued for despawn. Capacity checks count live plus
pending spawns; a queued despawn does not free a slot for a spawn in that same
query. Near capacity, collect replacement spawn specifications in a dense
array and create them after `each` returns.

### Bounded inspection

```lua
local page=ecs.inspect("arena",{
  with={"hostile","pos"},
  select={pos={"x","y"},hostile={"kind"}},
  limit=32,
  after=0,
})
```

`with` filters, `select` projects up to 8 components with at most 16 fields
each, `limit` defaults to 64 and is capped at 128, and `after` pages by entity
ID. The result has `world`, `alive`, `capacity`, `component_type_count`, `matched`, `returned`,
`truncated`, `budget_exhausted`, `next_after`, `component_counts`, and stable
`entities` entries. Only requested scalars are copied. Strings are capped at
256 bytes; a request is capped at 2048 scalar cells and 32768 string bytes.
Unsupported values are short type placeholders.

Prefer the host `ecs_query` JSON-RPC method for agent diagnostics: it calls a
registry-protected inspector even if cart code replaces the public `ecs`
global, and adds the observed `frame_count`. Do not feed inspection output
back into gameplay; normal world queries are cheaper and clearer. When paging,
do not step or structurally mutate the world until all pages are collected if
they must represent one coherent snapshot.

When observing the same bounded population at several frames, define an
`ecs_watch` through the host instead of resending this selector. Watches do not
change the Lua API or deterministic core: they retain one prior bounded host
sample and report count/component/returned-ID deltas. Read the command
reference for the lifecycle and truncation contract.

## Drawing primitives

| Function | Behavior |
|---|---|
| `cls([c=0])` | Clear the current clip rectangle to color `c`. It ignores camera but respects `clip`. |
| `pset(x,y,[c=0])` | Draw one pixel. Off-screen is a no-op. |
| `pget(x,y)` | Read raw framebuffer color in screen space; off-screen returns 0. Ignores camera and display-palette remaps. |
| `line(x0,y0,x1,y1,[c=0])` | Inclusive Bresenham line. |
| `rect(x0,y0,x1,y1,[c=0])` | Inclusive rectangle outline. |
| `rectfill(x0,y0,x1,y1,[c=0])` | Inclusive filled rectangle. |
| `circ(x,y,[r=4],[c=0])` | Midpoint circle outline. |
| `circfill(x,y,[r=4],[c=0])` | Filled circle. |
| `text_size(value)` | Return logical `(width,height)` using the built-in fixed font. |
| `print(value,[x=0],[y=0],[c=12],[align="left"])` | Draw text; align each line `left`, `center`, or `right` on x. Values are coerced predictably; lowercase may render uppercase. |
| `draw_tag([name])` | Label later opt-in draw-trace events with a semantic layer/system name; no argument clears it. Maximum 64 UTF-8 bytes. Never draws. |

`camera`, `clip`, the draw palette, and `fillp` affect shape primitives.
`print` ignores `fillp`. `cls` writes its literal color instead of applying the
draw palette.

### Text measurement and alignment

The font draws 3×5 ink in fixed 4×6 cells. `text_size` includes the trailing
spacing cell edge so blocks compose predictably: each byte adds 4px, each
newline adds a 6px line, and the widest line determines width.

```lua
local w,h=text_size("AB\nC") -- 8,12
print("TITLE",96,20,14,"center")
print("SCORE "..score,188,4,63,"right")
```

`y` is always the top. `left` makes x the line start, `center` centers every
line independently on x, and `right` makes every line end at x. Coordinates
are world-space and camera is applied afterward. Omit alignment for the
original left-aligned behavior. Prefer anchors over `#text*4` arithmetic;
`text_size` remains useful for panels, wrapping, and collision with UI.

## Sprites and declared animations

### `spr`

```lua
spr(n, x, y, [w=1], [h=1], [flip_x=false], [flip_y=false])
```

Draw the `w×h` block of 8×8 tiles beginning at tile ID `n`; `(x,y)` is the
top-left destination. Tile rows follow the 16-tiles-wide sheet. Source colors
marked transparent by `palt` are skipped before the draw palette remap. Color 0
is transparent by default.

### `sspr`

```lua
sspr(sx, sy, sw, sh, dx, dy, [dw=sw], [dh=sh],
     [flip_x=false], [flip_y=false])
```

Blit any source pixel rectangle to any destination size with deterministic
nearest-neighbor sampling. Source pixels outside the sheet are skipped. A
non-positive source or destination size draws nothing; negative sizes do not
mirror. At 1:1 it matches `spr`'s pixel path exactly.

### `aspr`, `anim_len`, `anim_done`

```lua
aspr(name, x, y, [t0=0], [flip_x=false], [flip_y=false])
anim_len(name)
anim_done(name, [t0=0])
```

`aspr` draws an animation from `__gfx_meta__`; `(x,y)` is the declared anchor,
not the top-left. Frame selection is stateless:

```text
position = floor((frame_count - floor(t0)) * fps / 60)
```

Looping animations wrap; one-shots clamp to their last frame. Capture a frame
origin when a state begins to restart playback:

```lua
local function frame() return flr(t() * 60) end

if btnp(4) and state == "idle" then
  state = "attack"
  attack_t0 = frame()
end

if state == "attack" and anim_done("hero.attack", attack_t0) then
  state = "idle"
end

function _draw()
  if state == "attack" then
    aspr("hero.attack", hero.x, hero.y, attack_t0, hero.left)
  else
    aspr("hero.idle", hero.x, hero.y) -- global phase is fine for ambience
  end
end
```

`anim_len` returns the declared frame count. `anim_done` becomes true only
after a one-shot's final frame has received its full duration; it is always
false for a loop. Unknown animation names are hard Lua errors. Flipping mirrors
pixels inside the destination rect but does not move/mirror the anchor.

## Tile map

| Function | Behavior |
|---|---|
| `map([cel_x=0],[cel_y=0],[sx=0],[sy=0],[cel_w=128],[cel_h=64])` | Draw a source cell block at screen/world destination `(sx,sy)`. Bare `map()` draws the full map. Tile 0 is skipped. |
| `mget(cx,cy)` | Read a live map tile ID; off-map returns 0. Coordinates floor. |
| `mset(cx,cy,[v=0])` | Write the live map; value floors/masks to 0–255. Off-map is a no-op. |

`map` is a sequence of sprite draws, so camera, clip, `pal`, and `palt` apply.
`mset` mutations persist across frames and reproduce through replay.

## Persistent draw state

Draw state never resets at frame boundaries. Reset or set it deliberately,
especially before scene transitions.

### `camera`

```lua
camera([x=0], [y=0])
```

Subtract `(x,y)` from later drawing coordinates. No args resets to `(0,0)`.
It affects shapes, sprites, maps, and text; not `cls` or `pget`.

### `clip`

```lua
clip([x,y,w,h])
```

Set a screen-space clip after camera transformation. No args resets to the full
screen. Non-positive dimensions or an entirely off-screen rectangle produce an
empty clip. `cls` respects the clip.

### `pal`

```lua
pal(c0, [c1=c0], [plane=0])
pal() -- reset draw palette, display palette, and palt
```

- Plane 0 remaps at draw time. The framebuffer stores `c1`.
- Plane 1 remaps only at presentation. The framebuffer and `screen_text` retain
  the original index; screenshots/web compose through the display map.
- A call performs one lookup, not chained remapping.
- `cls` bypasses the draw palette.

Use plane 1 for cheap full-screen fades or flashes without redrawing art.

### `palt`

```lua
palt(c, [transparent=false])
palt() -- reset so only color 0 is transparent
```

Transparency is checked on a sprite's source color before draw-palette remap.
It applies to `spr`, `sspr`, `aspr`, and map tiles, not shapes or text.
Calling `pal()` with no args also resets `palt`.

### `fillp`

```lua
fillp([pattern=0], [secondary])
```

Apply a 16-bit 4×4 pattern to shape primitives only. Bit 15 is top-left and
bits proceed row-major. Clear bits draw the shape color; set bits draw
`secondary`, or leave the existing framebuffer untouched when `secondary` is
omitted. The grid is anchored in screen space after camera transformation.
`fillp()`/`fillp(0)` restores solid fill. `pal()` does not reset it.

Useful patterns:

| Pattern | Appearance |
|---|---|
| `0x5a5a` | 50% checker blend |
| `0x8888` | 25% secondary |
| `0xeeee` | 75% secondary |
| `0x0f0f` | horizontal stripes |
| `0x3333` | vertical stripes |

## Frame effects and presentation order

```lua
mosaic([factor=1])
rshift([y], [dx=0])
```

`mosaic` replaces each finished `factor×factor` block with its top-left pixel;
factor clamps to 1–32 and 1/off is the default. `rshift(y,dx)` shifts one
finished scanline, positive right, wrapping modulo 192. `rshift()` clears the
whole shift table; `rshift(y)` clears one line. Off-screen `y` is a no-op.

Both settings persist, but each frame is derived from the pristine drawn frame,
so effects do not compound. Presentation order is:

```text
_update -> _draw -> mosaic -> rshift -> host display-palette mapping
```

`mosaic` and `rshift` rewrite the presented framebuffer and therefore appear in
screenshots and `screen_text`. Display `pal(...,1)` does not.

Example raster water:

```lua
rshift()
for y = 210, 319 do
  rshift(y, 3 * sin(t() * 0.5 + y / 40))
end
```

## Input

```lua
btn(i)   -- true while held
btnp(i)  -- true only on the transition into held this frame
```

Indices: `0=L`, `1=R`, `2=U`, `3=D`, `4=A`, `5=B`, `6=menu`. Out-of-range
indices return false. Use `btn` for continuous movement and `btnp` for actions,
toggles, and menu selection.

## Time, random, and math

| Function | Behavior |
|---|---|
| `t()` | Exact elapsed seconds: `frame_count / 60`. |
| `rnd([n=1])` | Deterministic float in `[0,n)`. |
| `srand([seed=0])` | Reseed the console PRNG; finite seed floors to an integer. |
| `flr(x)` | Floor. Returns an integer when representable. |
| `ceil(x)` | Ceiling. |
| `abs(x)` | Absolute value. |
| `min(...)`, `max(...)` | Fold one or more numeric arguments; zero args is an error. |
| `mid(a,b,c)` | Median/clamp helper. |
| `sin(turns)`, `cos(turns)` | Standard-sign trig in turns: one full cycle is 1. `sin(0.25)=1`; unlike PICO-8, sine is not inverted. |

`math.random` and `math.randomseed` deliberately raise and direct the author to
`rnd`/`srand`.

## Audio

| Function | Behavior |
|---|---|
| `sfx(id,[channel=-1])` | Play SFX 0–63. Channel −1 auto-picks the lowest free/non-music channel; if all are occupied it steals channel 5. |
| `sfx(-1,[channel])` | Stop one channel; omit channel/leave −1 to stop all SFX channels. |
| `music(pattern)` | Start the song chain at pattern 0–63. |
| `music(-1)` | Stop music. |
| `master(drive,[tone=0],[hiss=0])` | Override cart master settings: drive 0–8, tone 0–8, hiss 0–4. `master(0)` is clean. |
| `echo(delay,[feedback=0],[level=0])` | Override global echo: delay is -1 (kill) or 0–60 frames, feedback 0–8, level 0–8. Delay -1/0 or level 0 disables and flushes it. |

Audio calls from `_update` or `_draw` affect the same frame's 735 samples.
Runtime IDs/ranges are checked; invalid values halt the cart.

## Logging

```lua
printh(value)
```

Append a deterministic text line to the host log. It never draws. `run` emits
lines on stderr prefixed with `[log]`; JSON-RPC `logs` drains them.

## Sandbox and determinism

Available pure Lua facilities include normal table/string/math operations,
`ipairs`, numeric loops, `pcall`, `rawget`, and `rawset`. Removed globals:

```text
io os debug package require dofile loadfile load loadstring
```

That list describes the runtime and hand-authored cart chunk: there is no host
module loader or filesystem. A `console.toml` project may nevertheless split
Lua into static modules. `console build` recognizes only literal
`require("game.player")` and `require 'game.player'`, resolves them under
`[lua].root`, and compiles them into private cached closures. The generated
loader is lexical, so `require` and `package` remain `nil` globals at runtime.
Modules keep local scope, return one cached value, execute once, and return
`true` when they have no explicit return. Dynamic imports and cycles fail the
build. Keep the generated namespace free: identifiers beginning `__console_`
are reserved and rejected. See [platform and cart format](platform-and-cart-format.md#multi-file-projects)
for the manifest and diagnostics contract.

Do not use execution order from `pairs` where it can change state, rendering,
or sound. Prefer arrays/`ipairs`, numeric `for`, or a sorted explicit key list.
Do not derive behavior from host time. Use `t`, frame counters, and deterministic
input. Same cart + seed + per-frame input masks must produce byte-identical
framebuffers and audio.

## Practical patterns

Reset state at the beginning of a draw when scenes use different effects:

```lua
function begin_draw()
  camera()
  clip()
  pal()
  fillp()
  mosaic()
  rshift()
end
```

Use a stable fixed-step physics update:

```lua
function _update()
  local ax = (btn(1) and 1 or 0) - (btn(0) and 1 or 0)
  player.vx = mid(-2, player.vx + ax * 0.2, 2)
  player.x = player.x + player.vx
end
```

### Development hooks

Register small host-facing hooks without giving the game a second logic path:

```lua
devhook.register("status", {
  description="Return semantic player and scene state",
  phase="post_frame",
  run=function(_args)
    return {scene=scene,x=player.x,y=player.y,won=won}
  end,
})

devhook.register("start", {
  description="Enter play before frame one",
  phase="pre_frame",
  run=function(_args) start_game() end,
})
```

Registration is legal only at cart top-level or inside `_init`, and closes
before frame 1. The spec table accepts exactly `description`, `phase`, and
`run`. Names use the 1–64 byte identifier grammar; descriptions are 1–160
bytes; phases are `pre_frame` or `post_frame`; at most 32 hooks may be
registered. Duplicates, unknown fields, invalid metadata, and late
registration halt loading/execution. The public table has only `register`;
host discovery/invocation remains protected if cart code overwrites `devhook`.

Callbacks receive one argument and return one result. Both are bounded
JSON-like values: nil, boolean, finite number, UTF-8 string, dense array, or
string-key object. The limits are depth 4, 128 values, 64 aggregate table
entries, 4096 aggregate string bytes, and 64 bytes per nonempty key. Do not
return functions, userdata, coroutines, mixed/sparse tables, cycles, NaN, or
infinity. An empty Lua table becomes `[]`; include a named field when the
result must remain an object. Hooks remain inert during normal play and obey
the ordinary Lua sandbox and deterministic rules. A callback or result-contract
error halts the console so partially mutated state cannot continue outside the
replay log.

Use `pre_frame` only for setup that must precede frame 1. Use `post_frame` for
inspection or deliberate mutation at a completed boundary (frame 0 is a valid
boundary). Reset rebuilds registration and clears calls. Named save states
replay hook calls and steps in their original order. Inspect exact host syntax
in [the command reference](command-reference.md#hooks).

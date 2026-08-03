# Console Lua API

Use this reference whenever writing cart Lua. It documents every console-provided
global, callback order, persistent draw state, input, audio, determinism, and the
sandbox. Function arguments in square brackets are optional.

## Contents

- [Runtime and callbacks](#runtime-and-callbacks)
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
| `print(value,[x=0],[y=0],[c=12])` | Draw text using the built-in 4×6 ASCII font. Values are coerced predictably; lowercase may render uppercase. |

`camera`, `clip`, the draw palette, and `fillp` affect shape primitives.
`print` ignores `fillp`. `cls` writes its literal color instead of applying the
draw palette.

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

Expose small developer hooks for playtests without giving the game a second
logic path:

```lua
function dev_status()
  return {scene=scene, x=player.x, y=player.y, won=won}
end

function dev_warp(x, y)
  player.x, player.y = x, y
end
```

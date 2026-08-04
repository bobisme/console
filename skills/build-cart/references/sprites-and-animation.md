# Sprites and animation guide

Use this guide to plan the sheet, draw readable pixel art, declare multi-tile
sprites, animate without jitter, and validate the result with agent tools.

## Contents

- [Design for the display](#design-for-the-display)
- [Plan the sheet](#plan-the-sheet)
- [Choose semantic inks](#choose-semantic-inks)
- [Construct readable sprites](#construct-readable-sprites)
- [Author pixels safely](#author-pixels-safely)
- [Declare sprites and animation](#declare-sprites-and-animation)
- [Animation craft](#animation-craft)
- [Runtime playback](#runtime-playback)
- [Validation workflow](#validation-workflow)
- [Common failures](#common-failures)

## Design for the display

The logical canvas is 192×320, but physical phone presentation makes tiny
details smaller than a desktop inspection render suggests.

- Use 8×8 for particles, tiny pickups, UI icons, and repeating terrain.
- Prefer 16–24px primary actors, hazards, doors, and important props.
- Reserve 24–40px silhouettes for bosses or set pieces; they consume sheet
  space quickly.
- Judge the silhouette at 1× before polishing internal pixels.
- Preserve one-pixel gaps around moving limbs and interactive edges so forms do
  not merge into backgrounds.

A readable sprite usually needs three things before detail: a distinct outer
shape, a clear facing/action cue, and luminance separation from likely scenery.

## Plan the sheet

The sheet is 16×16 addressable tiles. Sketch an allocation table before adding
large animations. Group related frames in rectangles so `frames=` indices or
`frames_rect` can address them predictably.

One useful convention:

```text
tile rows 0-3    player and core animation
tile rows 4-6    enemies and hazards
tile rows 7-10   terrain families and metatile parts
tile rows 11-12  pickups, props, UI
tile rows 13-15  effects, variants, reserve
```

This is only a convention; existing carts may use another layout. Before
allocating, render/dump the proposed raw rect and inspect all current
`__gfx_meta__` declarations. Avoid overlapping two definitions accidentally.

Save capacity by:

- flipping a directional sprite at runtime when asymmetry is unimportant;
- reusing unchanged body parts and drawing a small overlay separately;
- sharing impact/effect frames between attacks;
- reusing terrain edges across several metatiles;
- keeping long decorative loops to 2–3 subtle frames.

## Choose semantic inks

Pick a compact role palette per sprite instead of sampling colors arbitrarily.
Typical roles:

```text
0 transparent
outline / deepest shadow
body shadow
body midtone
body light
highlight or gameplay accent
```

Use adjacent Apollo64 ramp values for material shading, then one contrasting
accent for eyes, damage, collectibles, or interaction. Four to six nonzero
colors usually produce a stronger small sprite than ten unrelated colors.

If runtime Lua globally remaps a compact source palette, mirror the stable
mapping in `__meta__.preview_palette` so static sprite/map renders show the
same roles. Do not bake transient damage flashes or scene fades into preview
metadata.

## Construct readable sprites

Work from large decisions to small ones:

1. Block the silhouette with one flat color.
2. Verify pose, facing, and contact point at 1×.
3. Add the darkest boundary only where it separates forms; avoid outlining
   every internal edge.
4. Add one shadow plane and one light plane. Prefer contiguous pixel clusters
   over isolated noise.
5. Add a sparse highlight/accent after the form reads.
6. Remove pixels that do not survive at phone scale.

Pixel-art heuristics:

- Keep curves as intentional step rhythms such as `1,1,2,2` rather than random
  staircase noise.
- Avoid single-pixel color islands unless they communicate a critical feature.
- Use selective outlines: darker on the shadow side, lighter/open on lit edges.
- Let animation carry life; do not force every material detail into one frame.
- Test the sprite over both light and dark nearby palette values.

## Author pixels safely

For an external pixel editor or generated bitmap, use an explicit PNG
round-trip. Export is raw source-palette art without checkerboard decoration:

```bash
console sprite export game.cart player --frame 0 --palette source \
  -o /tmp/player.png
console sprite import game.cart player --frame 0 --input /tmp/player.png \
  --mapping exact --max-colors 6 --dry-run
console sprite import game.cart player --frame 0 --input /tmp/player.png \
  --mapping exact --max-colors 6
```

PNG dimensions must equal the target exactly. If source art is not already
Apollo64, reduce it deliberately first—never make import guess at a resize or
color budget:

```bash
console palette show -o /tmp/apollo64.png --cell 16
console palette quantize /tmp/concept-crop.png -o /tmp/player-apollo.png \
  --colors 6 --alpha-threshold 128 --dither none --format json
```

Inspect the quantized preview before import. Prefer `--mapping exact` for the
final write; `--mapping nearest` is useful for a controlled one-step conversion
but makes every chosen index less explicit.

### PNGs in multi-file projects

For project source trees, make placement reproducible in `console.toml` instead
of importing into a generated cart:

```toml
[[sprites]]
name = "player"
source = "art/player.png"
tile = [2, 4]
anchor = [8, 15]
mapping = "exact"
max_colors = 8

[[sprites]]
name = "smoke_strip"
source = "art/smoke.png"
tile = [4, 4]
mapping = "quantize"
max_colors = 5
```

`console build` generates the full sheet and named graphics metadata. PNG
dimensions must be tile-aligned; entries need explicit nonoverlapping
placements. Exact is safe-by-default. Choose `nearest` or `quantize` only when
the loss is intentional, and inspect `sprite_assets` in the JSON build report
to verify the selected palette indices and color counts. Keep authored `anim`
lines in the file selected by `[sections].gfx_meta`; they may refer to these
generated sprite names.

Use `sprite dump` to extract exact palette rows and `sprite poke --stdin` to
write them back. The dump header begins with `#`, so it can pass through stdin.

```bash
console sprite dump game.cart player --frame 0 > /tmp/player.rows
console sprite poke game.cart player --frame 0 --stdin --dry-run \
  < /tmp/player.rows
console sprite poke game.cart player --frame 0 --stdin \
  < /tmp/player.rows
```

`poke` validates exact dimensions and palette characters. Prefer it over
counting characters inside a 128-column `__sprites__` row.

Use transforms for mechanical work:

- `copy` a strong base frame before posing a new frame;
- `shift` to adjust alignment without retyping rows;
- `flip` for initial opposing-direction variants;
- `rotate` only square effects/props where rotation is intended;
- `clear` abandoned regions before reallocating them.

Run nontrivial writes with `--dry-run`, then inspect the cart diff.

## Declare sprites and animation

```text
__gfx_meta__
sprite hero rect=0,0 size=2x3 anchor=8,23
anim hero.idle frames=0,1 fps=3 loop
anim hero.run frames=0,1,2,3 fps=12 loop frames_rect=0,3
anim hero.attack frames=8:6,10:6,12:6 fps=15
```

Anchor grounded characters at their foot contact. Anchor floaters near visual
center, doors at their hinge/base, and projectiles at their collision center.
All frames of one animation inherit the same sprite size and anchor.

Frame addressing:

- Integer `i` starts at the sprite rect or the animation's `frames_rect`, moves
  right by `i * sprite_width_tiles`, and wraps down by sprite height.
- Explicit `tx:ty` places that frame's sprite-sized rectangle directly.
- Mix both forms when a damaged/reserved area breaks a contiguous run.

Use `frames_rect` when one sprite has several animation strips. Use explicit
coordinates when frames are sparse or shared.

## Animation craft

Build animations around poses, not equal pixel churn.

### Idle

Use 2–4 frames at roughly 2–5 fps. Move one coherent mass (breathing, cloth,
lantern glow) by one pixel. Keep feet/contact anchor fixed.

### Walk/run

Use 4–6 key frames at roughly 8–15 fps:

1. contact;
2. down/compression;
3. passing;
4. up/extension;

Opposing limbs should create a readable diagonal. Let the torso move by at most
one pixel unless the gait is intentionally exaggerated.

### Attack/action

Use anticipation, contact, and recovery. The contact frame should have the
largest/clearest silhouette change. One-shot playback uses uniform frame
durations; use hand-written `spr` indexing if anticipation or recovery needs a
different duration per frame or if hitboxes depend on frame phases.

### Ambient variation

Phase-lock repeated lights or water with omitted `t0`. To avoid every instance
moving together, provide deterministic offsets such as `t0 = object.id * 7`.
Never randomize animation phase from host time.

## Runtime playback

Use `aspr` for ordinary uniform-rate loops and one-shots:

```lua
local function frame() return flr(t() * 60) end

function set_state(next)
  if state ~= next then
    state = next
    state_t0 = frame()
  end
end

function _draw()
  if state == "attack" then
    aspr("hero.attack", hero.x, hero.y, state_t0, hero.facing < 0)
  elseif abs(hero.vx) > 0.1 then
    aspr("hero.run", hero.x, hero.y, state_t0, hero.facing < 0)
  else
    aspr("hero.idle", hero.x, hero.y, state_t0, hero.facing < 0)
  end
end
```

Keep hand-rolled `spr` frame logic for ping-pong/reverse playback, variable
durations, velocity-linked timing, frame-dependent hitboxes, or independently
composed body parts.

## Validation workflow

Use all three evidence layers:

1. **Data:** `sprite atlas` checks sheet ownership, resolved frames, blank
   allocations, aliases/conflicts, and unused cells; `sprite dump` confirms exact indices and dimensions;
   `sprite export` confirms the editor-facing source image round-trips.
2. **Numbers:** `sprite lint` measures area, centroid/bbox drift, changed pixels,
   and one-frame-only colors.
3. **Vision:** render, strip, onion, diff, ghost, and GIF views expose actual
   shape/timing.

Recommended loop:

```bash
console sprite render game.cart hero --frame 0 \
  --zoom 12 --grid --indices --anchor -o /tmp/hero.png
console sprite atlas game.cart --zoom 4 --grid -o /tmp/sprite-atlas.png \
  > /tmp/sprite-atlas.json
console sprite strip game.cart hero.run \
  --zoom 12 --anchor -o /tmp/hero-run.png
console sprite onion game.cart hero.run --all \
  --zoom 12 --grid --anchor -o /tmp/hero-onion.png
console sprite lint game.cart hero.run \
  --max-drift 2 --max-area-var 20 --max-changed 100 \
  --no-unique-colors --summary
console sprite gif game.cart hero.run --zoom 8 --anchor \
  -o /tmp/hero-run.gif
```

Thresholds are starting points, not universal laws. A squash animation should
change area; teleport/smear frames should change many pixels. Explain deliberate
violations rather than weakening all gates.

Finally capture the sprite in gameplay. A clean isolated render can still fail
against its background, camera motion, effects, or nearby UI.

## Common failures

| Failure | Diagnosis / fix |
|---|---|
| Character jitters | Anchor or mass centroid drifts. Inspect onion/anchor and shift frames. |
| One-frame sparkle | Usually an accidental unique palette character. Run lint with `--no-unique-colors`. |
| Animation pops | Too many changed pixels or missing transition pose. Inspect diff/ghost. |
| Sprite invisible | Source color is transparent via `palt`, target is off-screen/clip, or frame rect is blank. |
| Wrong colors in tools | Add/fix `preview_palette`; remember it does not affect runtime. |
| Wrong colors at runtime | Inspect draw/display `pal` state and reset it between scenes. |
| Frame resolves elsewhere | Check `frames_rect`, explicit `tx:ty`, and the resolved `sprite_id` in lint. |
| Sheet corruption | Overlapping allocations or manual 128-column edits. Use named targets and poke/edit tools. |
| Looks good enlarged, unreadable on phone | Simplify silhouette, increase major feature size, and test packed 1× presentation. |

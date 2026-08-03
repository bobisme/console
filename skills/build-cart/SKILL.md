---
name: build-cart
description: How to build, test, and ship games (carts) for this fantasy console — cart format, Lua API, the headless dev loop with console-agent, sprite/animation authoring tools, music/sfx authoring, determinism rules, and single-file HTML packaging with console-pack. Use when creating or modifying a cart, authoring sprites/animations/music, debugging cart behavior, or packaging a game.
---

# Building carts for this console

A cart is ONE plain-text file (`*.cart`): Lua 5.4 code + sprites as palette text
grids + music as tracker text. The console is deterministic to the bit: the
same cart + seed + input sequence produces identical pixels and audio
samples on every platform. You develop headlessly through the
`console-agent` binary — never guess what something looks or sounds like;
render it and look.

Tools you need on PATH (from a release, or `cargo build --release` in the
platform repo → `target/release/`): **`console-agent`** (run, inspect,
author) and **`console-pack`** (package to HTML; needs the repo's
`web/engine.js` + `web/template.html` beside it or via `--engine`/
`--template`). The platform repo's `SPEC.md` is the authority on every
format; this skill is the working knowledge.

## Console facts

144×256 portrait, fixed 64-color Apollo64 palette (indices 0–63), 60fps
fixed timestep, 7 buttons (d-pad, A, B, menu), 6 audio channels, Lua 5.4 in
a sandbox. `_update()` then `_draw()` every frame.

## The dev loop

```bash
console-agent run game.cart \
  --frames 120 --input "30:,20:R,10:RA,60:" \
  --screenshot f120.png [--screenshot-zoom N] [--screen-text] [--eval "player.x"] [--wav out.wav] [--seed N]

# input spec: COUNT:BUTTONS segments; letters L R U D A B M; empty = idle
# printh() output arrives on stderr as [log] lines; nonzero exit = cart error
```

For interactive sessions (load once, step incrementally, inspect between
steps): `console-agent serve` — JSON-RPC 2.0, one request per line on
stdin. Verbs: `load_cart{path|text} reset{seed} step{frames,input}
screenshot{path} screen_text eval{code} get_global{name} logs
save_state{name} load_state{name} info audio_state audio_events
audio_stats spectrogram{path} wav{path}` plus the `sprite_*`, `map_*`
(read-only: `map_render`, `map_dump`, `map_lint`) and `music_*` (read-only:
`music_score`, `music_lint`, `music_piano_roll`) mirrors of the CLI tools
below.

Iterate in small steps: change one thing → step → screenshot/eval →
verify. `screen_text` returns the framebuffer as 256 rows of 144 palette
characters (`0-9a-zA-Z-_` maps to 0–63) —
cheap for asserting "pixel (x,y) is color c" without vision. Save states
are replays (`seed` + input log), so they reproduce everything, audio
included.

## Cart anatomy

```
__meta__          title=... author=... version=...
__lua__           the game (Lua 5.4, sandboxed)
__sprites__       128 lines x 128 palette chars (0-9a-zA-Z-_); 1 char = 1 pixel; sprite n at (n%16*8, n//16*8)
__map__           up to 64 lines x up to 128 cells; 2 hex chars = 1 tile id (00-ff); tile 00 = empty
__gfx_meta__      sprite <name> rect=tx,ty size=WxH [anchor=px,py]
                  anim <sprite>.<label> frames=f0,f1,... fps=N [loop] [frames_rect=tx,ty]
                  # played at runtime by aspr("<sprite>.<label>", x, y, [t0])
__instruments__   inst <name> wave=<0-7|w0-w7> [fm=ratio,index[,decay]] [env=a,d,s] [vib=cents,rate,delay] [trem=depth,rate[,delay]] [sweep=semis,frames] [duck=depth,release] [echo=0-8]
                  # waves: 0/1/2 pulse 12.5/25/50%, 3 tri, 4 saw, 5 white noise, 6 FM (needs fm=), 7 periodic noise, w0-w7 wavetables
                  wavetable <slot 0-7> <32 hex nibbles>   # custom single-cycle wave
                  master drive=0-8 [tone=0-8] [hiss=0-4]
                  echo delay=1-60 feedback=0-8 level=0-8
__sfx__           sfx <id> speed=<n|auto> [loop=start,end]  then rows: NOTE WAVE|INST VOL [FX]  or  ---
__music__         [bpm=N [rows_per_beat=R]]  then: pat <id> [stop|loop=<id>] : ch0 ch1 ch2 ch3 [ch4 ch5]
                  4 to 6 slots; a slot you leave off is silent, so old 4-slot lines still parse
```

Only `__lua__` is required. `#` starts a comment in the data sections.

## Lua API (essentials)

- Callbacks: `_init()`, `_update()`, `_draw()`.
- Draw: `cls([c]) pset pget line rect rectfill circ circfill
  spr(n,x,y,[w,h,flip_x,flip_y]) print(s,x,y,[c])`. Color 0 is transparent
  in `spr()`.
  - `sspr(sx,sy,sw,sh,dx,dy,[dw,dh],[flip_x,flip_y])` — blit any sheet
    rectangle into any screen rectangle, nearest-neighbor scaled (`dw,dh`
    default to `sw,sh`; any size <= 0 draws nothing, negatives do NOT mirror).
    Same camera/clip/pal/palt rules as `spr`; at 1:1 it *is* `spr`.
  - `aspr(name,x,y,[t0],[flip_x,flip_y])` — play an anim declared in
    `__gfx_meta__`. `(x,y)` is the sprite's **anchor**, not its top-left.
    Plus `anim_len(name)` and `anim_done(name,[t0])`. See "Sprite &
    animation authoring" below — this is how you animate.
- Tile map: `map([cel_x,cel_y,sx,sy,cel_w,cel_h])` draws a block of map cells as
  sprites (bare `map()` = the whole 128x64 map at 0,0; tile 0 cells are skipped;
  camera/clip/pal/palt apply exactly as to `spr`), `mget(cx,cy)` reads a tile id
  (0 off the map), `mset(cx,cy,v)` writes one.
- Draw state (all of it PERSISTS across frames — nothing auto-resets it):
  - `camera([x],[y])` — offset every later draw by `-(x,y)`; no args resets.
    `pget` and `cls` are unaffected.
  - `clip([x,y,w,h])` — clip rect in SCREEN space (after the camera); no args
    resets to full screen. `cls` respects it, so it clears the window.
  - `pal(c0,c1)` — draw-palette remap: pixels are actually written as `c1`.
  - `pal(c0,c1,1)` — DISPLAY remap, applied at scanout only. The framebuffer
    keeps its indices, so `for i=0,63 do pal(i,0,1) end` fades the whole
    screen to black with no redraw (and `screen_text` still shows the real
    pixels). Flashes: `pal(i,7,1)`. `pal()` with no args resets both maps
    AND `palt`.
  - `palt(c,flag)` — which colors `spr()` skips (default: only 0). Tested on
    the sprite's SOURCE color, before `pal()` remaps it. `palt()` resets.
  - `fillp(p,[secondary])` — 16-bit 4x4 dither pattern for SHAPES only (pset/line/rect/
    rectfill/circ/circfill; never spr/sspr/map/print/cls). Bit 15 = top-left,
    row-major; a clear bit draws the shape color and a set bit draws the
    explicit secondary color, or nothing when it is omitted. Anchored to SCREEN space, so shapes
    shimmer as the camera scrolls. `fillp()` = solid; `pal()` does NOT reset it.
  - `mosaic(f)` — end-of-frame pixelation: each f x f block of the finished
    frame becomes its top-left pixel (f 1-32, `mosaic()` = off). Unlike the
    display palette this really rewrites the framebuffer, so `screen_text` and
    screenshots show it.
  - `rshift(y,dx)` — end-of-frame per-scanline shift: line `y` (0-255) slides
    `dx` pixels, positive = RIGHT, WRAPPING around the 144-wide line (`dx` is
    taken mod 144, so -1 == 143 and any value is legal). Write-only:
    `rshift()` clears every line, `rshift(y)` clears one. Applied AFTER
    `mosaic`, and in the framebuffer like it.
- Input: `btn(i)` held / `btnp(i)` pressed-this-frame. 0=L 1=R 2=U 3=D
  4=A 5=B 6=menu (start/select-style; the web shell's triangle button).
- Audio: `sfx(n,[ch])` with `ch` 0–5 (`sfx(-1,ch)` stops a channel),
  `music(n)` / `music(-1)`, `master(drive,[tone],[hiss])` for saturation/tone
  at runtime, `echo(delay,[feedback],[level])` for the delay bus
  (`echo(0)` = off, and it flushes the tail).
- Math: `flr ceil abs min max mid sin cos rnd([n]) srand(seed) t()`.
  `sin/cos` take TURNS, not radians (`sin(x)` = `math.sin(x*2π)`), standard
  sign — NOT PICO-8's inverted sin. `t()` = frames/60, exact.
- `printh(s)` logs to the host; never draws.

### Effects worth reaching for

- **Scene transitions cost two lines.** `mosaic(f)` with `f` ramping
  `1,2,4,8,16,32` pixelates the screen out and the reverse pixelates the next
  scene in — no extra draw code, no shader, and it is in `screen_text` so you
  can assert on it. Pair it with a display-palette fade (`pal(i,0,1)`) for a
  dissolve to black that costs nothing either.
- **Raster effects with `rshift`.** One line in `_draw()` buys the whole SNES
  scanline-trick family, because the shift wraps instead of clipping:

  ```lua
  -- water / heat haze: a sine wave rolling down the screen
  for y = 0, 255 do rshift(y, 3 * sin(t() + y / 32)) end
  -- (sin takes TURNS here, so y/32 is 8 full waves down the 256 lines)

  -- horizontal parallax: each band scrolls at its own speed
  for y = 0, 255 do rshift(y, -t() * (10 + y / 8)) end

  -- reflection: only the bottom half wobbles
  rshift() for y = 160, 255 do rshift(y, 2 * sin(t() * 2 + y / 16)) end
  ```

  Clear it with `rshift()` before rebuilding the sweep if you only shift part
  of the screen — the table persists across frames like every other bit of draw
  state. It costs nothing to run and shows up in `screen_text`, so you can
  assert on it. Because it runs after `mosaic`, `mosaic(4)` plus a sine sweep
  gives chunky wobbling water rather than sliced-up blocks.
- **Dither shading with `fillp`.** Even with 64 colors, patterns add texture
  and shades without expanding the palette:
  `fillp(0x5a5a, light) rectfill(x0,y0,x1,y1, dark)` is a 50% blend of two
  colors; `0x8888` is 25% secondary, `0xeeee` 75%, `0x0f0f` horizontal stripes,
  `0x3333` vertical ones. Leave the secondary argument off and the pattern
  becomes a stencil instead — great for fog, water
  surfaces, damage flashes and "half-there" ghosts over whatever is behind.
  Remember it is anchored to the screen: a full-screen `rectfill` under a
  scrolling `camera` shimmers, which usually looks right.
- **`sspr` for zoom.** Title logos, boss intros, screen-shake punch-in and
  scaling pickups are all one `sspr` with a computed `dw,dh`. Integer scales
  (2x, 3x) stay crisp; fractional ones drop or repeat rows, which is the
  expected fantasy-console look.

## Determinism rules (non-negotiable)

- NEVER iterate string-keyed tables with `pairs()` — order is unstable
  across runs. Use `ipairs`, numeric `for`, or explicit key lists.
- There is no wall clock; animate from `t()` or frame counters.
- Randomness only via `rnd`/`srand` (seeded). `math.random` raises.
- The sandbox removes io/os/debug/require/load. Don't fight it.
- A Lua error halts the cart permanently (the harness surfaces the
  traceback; the web shell shows a crash overlay).

## Sprite & animation authoring

Draw with the `__sprites__` palette alphabet
`0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_` (one
character per pixel). Never hand-shift rows — use the transforms. Declare every sprite and anim in
`__gfx_meta__` (anchor at the feet/contact point for characters, visual
center for floaters) — that declaration is both what the tools below inspect
and what `aspr()` plays at runtime, so it is the single definition of an
animation. Anim frame `i` = the sprite's rect displaced `i`
widths rightward, wrapping down a row band. Leave tile 0 blank — sprite 0
is reserved as the empty tile by convention (color 0 is already `spr()`'s
transparent color, so an all-zero sprite 0 is a natural no-op/placeholder
id); nothing in the engine enforces this, but the demo cart follows it and
its test suite checks for it.

```bash
console-agent sprite render game.cart <sprite|anim|tx,ty,w,h> \
    [--frame N] [--zoom 12] [--grid] [--indices] [--anchor] -o out.png
console-agent sprite strip  game.cart <anim> --zoom 12 --anchor -o out.png  # frames baseline-aligned
console-agent sprite onion  game.cart <anim> --frame N [--grid] [--anchor] -o out.png  # red ghost=prev, green=next
console-agent sprite onion  game.cart <anim> --all [--grid] [--anchor] -o out.png      # contact sheet, every frame
console-agent sprite diff   game.cart <anim> A B -o out.png                 # magenta = changed pixels
console-agent sprite ghost  game.cart <anim> [--grid] [--anchor] -o out.png # motion accumulation
console-agent sprite gif    game.cart <anim> [--zoom 8] [--grid] [--anchor] -o out.gif  # animated preview at declared fps
console-agent sprite lint   game.cart [anim ...] \
    [--max-drift PX] [--max-area-var PCT] [--max-changed PX] [--no-unique-colors] [--summary]  # JSON quality numbers, or a CI gate
console-agent sprite edit   game.cart copy|shift|flip|rotate|clear ... [--dry-run]
console-agent sprite dump   game.cart <sprite|anim|tx,ty,w,h> [--frame N]    # print palette-character rows
console-agent sprite poke   game.cart <target> [--frame N] --rows r0,r1,... # write pixels back
console-agent sprite poke   game.cart <target> [--frame N] --stdin          # rows on stdin, one per line
```

Write pixels with `poke` instead of hand-editing rows: `dump` a region to see
its rows (a `#`-comment header plus one palette-character-per-pixel row per line,
same alphabet as `__sprites__`), edit the rows you got back, then `poke`
them — `--stdin` is the better fit for agents (`sprite dump ... | sprite
poke ... --stdin` round-trips cleanly; poke skips `#`-prefixed lines so the
dump header passes through harmlessly). `poke` validates row count and row
width against the target's region exactly and rejects characters outside the palette alphabet;
`--dry-run` previews the changed lines without writing.

Animation workflow: `copy` an existing frame → nudge pixels in the palette text →
`lint` until quiet → `onion`/`strip` for the visual pass. Quality gates:
zero/near-zero centroid drift relative to the anchor, silhouette area
steady within ~15%, no colors unique to a single frame (usually a typo'd
palette character), small `changed_pixels` between adjacent frames. Every frame
entry in `lint`'s JSON also carries `sprite_id`, the sheet tile `[tx, ty]`
that frame number actually resolved to — handy once an anim's frames don't
all sit in one contiguous run.

`lint` doubles as a CI gate: pass any of `--max-drift <px>`, `--max-area-var
<pct>`, `--max-changed <px>`, `--no-unique-colors` and it exits 1 (with a
`violations` array naming the anim/frame/metric/value/limit) the moment one
of those quality numbers crosses the line, instead of leaving the judgement
to whoever reads the report — the idiom for wiring this into a script or CI
step is `console-agent sprite lint game.cart --max-drift 3 --max-area-var 20
--max-changed 40 --no-unique-colors || fail "sprite lint gate failed"`.
`--summary` swaps the full JSON for one line per anim (frame count, worst
drift, worst changed-pixel count, unique-color count) — the shape you want
once the full per-frame dump is more noise than signal; it still combines
with the threshold flags above and still gates the exit code. Omit every
threshold flag and `lint` is exactly the old report-only tool (exit 0,
no `violations` key) — nothing here changes behavior for a bare `sprite
lint`.

### Playing anims at runtime: `aspr`

The runtime reads `__gfx_meta__`, so the frame list you authored *is* the one
that plays — never restate it in Lua.

```lua
aspr(name, x, y, [t0], [flip_x], [flip_y])   -- draw the anim's current frame
anim_len(name)                               -- frame count
anim_done(name, [t0])                        -- one-shot finished? (loops: never)
```

Two things to internalize:

- **`(x,y)` is the sprite's declared `anchor=`, not its top-left.** That is
  what anchors are for: position a walker by its feet, a floater by its
  centre, a 2x2 megatile by its thorax, and frames of different extent stop
  jittering against a corner. `spr()` is unchanged and still takes a
  top-left, so `aspr("p.walk", x+4, y+7, ...)` is the old
  `spr(id, x, y, ...)` for a sprite with `anchor=4,7`.
- **`aspr` is stateless.** The frame is a pure function of
  `frame_count - t0` — `floor(elapsed * fps / 60)`, wrapped for `loop` anims
  and clamped for one-shots. There is no animation object to tick, nothing to
  reset, and nothing new in the replay state. `t0` is the frame count you
  captured when the state changed; omit it and the anim phase-locks to the
  global clock, which is what ambient loops want.

```lua
-- Ambient loops: no t0, no bookkeeping at all.
aspr("star.twinkle", 24, 44)
aspr("moth.flap", mx, my, 0, facing_left, false)   -- pass t0=0 to reach the flips

-- Walk/idle: store one ORIGIN per state, captured the frame it changes.
function _update()
  local f = flr(t() * 60)
  local moving = btn(0) or btn(1)
  if moving ~= walking then
    walking = moving
    if moving then walk_t0 = f else idle_t0 = f end
  end
end
function _draw()
  if walking then aspr("player.walk", px, py, walk_t0, face_left, false)
  else            aspr("player.idle", px, py, idle_t0, face_left, false) end
end

-- One-shot attack: start it on the press, end it with anim_done.
if btnp(4) and state == "idle" then
  state, atk_t0 = "attack", flr(t() * 60)
end
if state == "attack" then
  aspr("player.slash", px, py, atk_t0)
  if anim_done("player.slash", atk_t0) then state = "idle" end
end
```

A typo'd anim name is a hard error (the cart halts and the message lists the
anims you did declare), so a mis-spelled cycle can never quietly draw nothing.
Camera, clip, `pal` and `palt` apply exactly as they do to `spr`, and
`frames_rect=`/explicit `tx:ty` frames resolve identically at runtime and in
the tools.

Hand-rolled frame tables are still legal and still the answer for the exotic
cases `aspr` deliberately does not cover — non-uniform frame durations, an
anim that ping-pongs or plays backwards, frame-dependent hitboxes, a cycle
whose speed follows the player's velocity. Those want your own index math and
plain `spr()`; everything else wants `aspr`.

An anim's frames don't have to be a contiguous run starting at its sprite's
own `rect` — that's just the default. `frames_rect=tx,ty` relocates where
INDEX frame `0` starts for that one anim (same `WxH`, same wrap rule), so a
second anim of the same sprite can live in its own sheet region instead of
fighting the first for contiguous tiles; a `frames=` entry can also be an
explicit `tx:ty` instead of an index, pinning that one frame to any tile on
the sheet regardless of `frames_rect`, which is how you skip a
damaged/reserved tile or give a megatile sprite frames that aren't one huge
contiguous block. The two compose: `anim boss.slam frames=0,1,7:2,3
frames_rect=4,9 fps=10` — frames 0, 1 and 3 count from `(4,9)`; frame `7:2`
is pinned at tile `(7,2)` regardless.

## Tile map authoring

The map remains hex text (**2 chars per cell**), unlike the sprite sheet's
64-character palette alphabet. The
number you write is a sprite index — `01` is sprite 1, `1f` is sprite 31. Count
in pairs, not characters: the screen is 18 cells wide (144/8) and 32 tall, so a
screenful is 36 characters per line. Rows can be short (they pad with tile 0)
and you can leave rows off entirely, so a ground plane is a few lines, not 64.

```
__map__
# 18 cells = one screen wide
000000000000000000000000000000000000
010101010101010101010101010101010101
```

Tile `00` is the empty cell — `map()` skips it, so it costs nothing and shows
nothing. That is why sprite 0 is left blank by convention: id 0 means "no tile"
in both systems. Keep terrain tiles at ids you can read at a glance (a 1x row of
ground, `1x` a row of decoration).

`map()` is the same pixel path as `spr()`, so a scrolling level is
`camera(cam_x, cam_y) map()` and the clip rect windows it for free. For dynamic
terrain — a block you break, a door that opens, a tile that burns — use
`mset(cx, cy, id)`; the change persists across frames and replays
deterministically, so it needs no save/restore of its own. `mget` is also your
collision lookup: `mget(flr(x/8), flr(y/8)) ~= 0` is "is there something here".

To check the map without vision, `screen_text` after a `map()` draw, or
`--eval "mget(3,4)"`.

### Map agent tooling

You don't have to hand-edit the hex grid blind — `console-agent map` mirrors
the sprite tools' shape (data, then numbers, then pictures) one level up,
cells instead of pixels:

```bash
console-agent map render <cart> [cx,cy,cw,ch] [--zoom 8] [--grid] [--ids] -o out.png
console-agent map dump   <cart> [cx,cy,cw,ch]                          # print cells as hex rows
console-agent map poke   <cart> [cx,cy,cw,ch] --rows <hex,hex,...>     # write cells back
console-agent map poke   <cart> [cx,cy,cw,ch] --stdin                 # rows on stdin
console-agent map lint   <cart>                                       # JSON quality numbers, whole map
console-agent map edit   <cart> copy|shift|fill|clear ... [--dry-run] # region transforms
```

`render`/`dump`/`poke` default their `[cx,cy,cw,ch]` region to the **used
extent** (the bounding box of non-zero cells) when omitted; `map edit`
always requires the region explicitly, since a region transform is
destructive. `map dump | map poke --stdin` round-trips cleanly, same as the
sprite tools (`--stdin` skips `#`-prefixed lines, so the dump header passes
through harmlessly). `fill`/`clear` and `poke`'s rows all use the `__map__`
alphabet directly — 1-2 hex digits, `00`-`ff`.

Workflow: `map lint` first — its `blank_sprite_tiles` list is the map's
version of "color unique to one frame": a tile id the map references whose
sprite-sheet region is entirely blank is almost always a typo'd digit. Then
`map render --ids` to see the grid's structure at a glance, each cell
labelled with its own tile id, before or after editing. Both `map edit` and
`map poke` rewrite only the changed lines of `__map__` and create the
section (positioned right after `__sprites__`) if the cart has none yet.

## Music & sfx authoring

You cannot hear. Work four layers, in order — and start at the cart, not at
a running console:

```bash
console-agent music score      game.cart [--song N]                  # the song as text
console-agent music lint       game.cart [--strict]                  # JSON diagnostics
console-agent music piano-roll game.cart [--song N | --patterns a,b] [--cell N] [--row-h N] -o roll.png
console-agent music render     game.cart [--song N] [--loops K | --frames F] -o out.wav
console-agent music edit       game.cart <verb> ...                  # rewrite __sfx__ in place
console-agent music import-abc game.cart tune.abc --sfx 0            # ABC notation -> sfx rows
```

1. **The score**: `music score` prints the whole song — the form chain
   (`pat 0 -> [pat 1 -> pat 2 ->] loop to 1`) then, per pattern, a tracker
   grid of `row | frame | NOTE VOICE VOL FX` per channel. This is the
   fastest way to see what you actually wrote, including which patterns are
   intro and which repeat. `--song N` follows the chain from pattern N,
   i.e. what `music(N)` would play.
2. **Numbers**: `music lint` — JSON, always exit 0 unless `--strict`. It
   catches the traps in the list below mechanically (env-sustain swell,
   vibrato and tremolo that never speak, no channel left for `sfx()`, notes
   swept off the note table, `music(n)`/`sfx(n)` calls naming ids the cart
   lacks, unreachable patterns, a chain with no `loop=`/`stop`, DC-offset
   wavetables, FM modulators past Nyquist) and reports every pattern's
   measured peak/RMS/clipped from a headless render of that pattern alone.
   Then `audio_stats` for the *running* mix (per-window RMS/peak/clipped)
   once the game is playing it.
3. **Running ground truth**: `audio_state` (per-channel sfx/row/note/vol/
   music ownership) and `audio_events` (note_on/row_change/note_off/
   pattern_change log with frame numbers). Reach for these when the score
   says one thing and the game does another — they show what the sequencer
   *did*, including sfx stealing channels from the music.
4. **Vision + humans**: `music piano-roll` (semitone × frame grid, one color
   per channel, brightness = velocity, pattern boundaries and the loop point
   marked) is the *score* seen at a glance; `spectrogram{path}` is the
   *signal* (what came out of the mixer, harmonics and all). Read them
   together when a note is not the note you expected. `music render` writes
   a WAV of a whole song for human ears without any input scripting — it
   boots the cart, calls `music(N)` and stops after the intro plus `--loops`
   (default 2) passes of the loop body.

`serve` mirrors the read-only three: `music_score{song?}`,
`music_lint{}`, `music_piano_roll{path, song?, patterns?, cell?, row_h?}`.
There is no `music_render` verb — in a session that is just
`eval{"music(n)"}` + `step` + `wav` — and no `music_edit`/`music_import_abc`
either: like `sprite edit` and `map edit`, rewriting a cart file is CLI-only.
Run the write verbs between sessions, then `load_cart` again.

Facts that bite (all but the last three are `music lint` rules):
- `env` sustain is an ABSOLUTE level — quiet rows on an env instrument
  swell UP to it. Voices needing per-row dynamics should carry no env.
- Vibrato `delay` must fit inside the row or it never speaks — same for `trem`.
- **Periodic noise (wave 7) is four octaves flat.** Write `A5` to hear `A1`.
  Its whole usable range is `C0`-`B3`, so it is a bass/tom/drone instrument.
- **A tremolo LFO restarts on every note-on.** A pad restating its chord each
  8-frame row never gets through a cycle; give it long rows.
- **Channel budget**: there are 6 channels and `sfx(n)` auto-allocation takes
  the lowest one music does not own, stealing **ch5** when all six are busy.
  So write songs for 4 or 5 slots and leave 1–2 channels free for blips;
  a 6-slot song works, but the first auto `sfx()` will eat channel 5.
- Six full-scale (vol 7) voices in phase sum past full scale and hard-clip —
  the mix gain is 0.25 per channel regardless of how many are sounding. Keep
  dense arrangements at vol 4–5, or set `master drive=1`, which soft-limits
  and cannot reach the clamp. Check with `audio_stats` (clipped count).
- Under `music()`, sfx `loop=` flags are ignored; a pattern lasts one pass
  of its longest slot.
- Song structure: intro patterns first, final pattern `loop=<id>` jumps
  back to the loop start; `stop` makes one-shot jingles. Multiple songs =
  id gaps (`music(0)` title, `music(8)` gameplay).
- Drums punch through via sidechain: give the kick `duck=3,8`.
- **FM without an index decay sounds like an organ.** `fm=<ratio>,<index>` on
  its own is a static timbre; the third number is what makes it a played note.
  Reach for `decay` 12-15 on basses and plucks, 6-9 on keys, 1-3 on bells.
- `master drive=1-3` is glue; 5+ is a distortion choice. `tone` darkens,
  `hiss` adds tape floor.
- **Echo** is one cart-global delay line plus a per-instrument send:
  `echo delay=24 feedback=5 level=6` in `__instruments__` and `echo=6` on the
  voices you want wet (0 = dry, the default). `delay` is in FRAMES, so it is
  tempo math you can do in your head — at `speed=8` a row is 8 frames, so
  `delay=8` is a row, `delay=16` a beat, `delay=24` the dotted-eighth. Repeats
  darken as they decay (a fixed 4.8 kHz lowpass in the loop) and feedback tops
  out at 7/8, so the tail always dies. `echo(d,f,l)` from Lua does the same at
  runtime; `echo(0)` kills it and flushes the tail.
- **Sparse notes make echo audible.** Echo only reads as echo in the gaps — a
  busy 16th-note part just smears into mush. Write the wet voice with long
  rests (one note every one or two beats), give it a short envelope so the note
  is gone before its repeat lands, and let the delay line fill the hole.
  Entry 15 of `carts/soundtest.cart` is four notes in 32 rows for exactly this
  reason. Send at 2–4 on a real mix; 6+ is an effect.
- Echo ADDS level (the return is post-fader and pre-mix-gain). Heavy sends plus
  high feedback can reach the clamp — check `audio_stats` for clipped samples,
  and `master drive=1` will soft-limit it for free.

### Score-level editing: never hand-shift tracker rows

`music edit` is the `sprite edit`/`map edit` of `__sfx__`: six verbs, all
CLI-only, all atomic, all with `--dry-run`, and all touching only the lines of
the cart they actually change.

```bash
console-agent music edit game.cart transpose  0-2 -12          # a whole part down an octave
console-agent music edit game.cart transpose  3 +7 --clamp     # clamp instead of erroring
console-agent music edit game.cart copy       0 8              # variate a groove without retyping it
console-agent music edit game.cart shift-rows 1 2              # rotate the phrase two rows later
console-agent music edit game.cart set-vol    2 -2             # duck a part against the mix
console-agent music edit game.cart set-inst   2 fmbass --where 3   # re-voice, optionally only some rows
console-agent music edit game.cart stretch    1 2              # half-time grid, same wall clock
```

- `<sfx-ids>` on `transpose` is an id, a range `0-5`, or a list `0,2,5-7`.
  Signed operands (`-12`, `+7`) are operands, not flags.
- **`transpose` refuses to fall off the note table** and tells you what would
  fit: `sfx 0 row 0: C4 -60 leaves the note table (C0-B7); the selection fits
  any shift in -48..=+40 — nearest to -60 is -48`. Take the number or pass
  `--clamp`.
- `set-vol` takes `0-7` absolute or `+n`/`-n` relative, clamped, and leaves
  rests as rests. `set-inst` validates the voice against the cart (wave digit,
  `w<slot>` or instrument name) and `--where <old>` matches the spelling
  `music score` prints.
- `stretch 2` inserts a `---` after every row and halves `speed=`; `stretch
  0.5` drops the odd rows (erroring if any carries a note — `--force` to mean
  it) and doubles `speed=`. Wall-clock length is preserved when the integer
  speed divides; when it does not, the summary prints the exact
  before/after frame count and the rounding delta. `speed=auto` is resolved to
  a number and a `loop=` range is rescaled with the rows.
- `copy` needs a free destination id (`--force` to overwrite), because
  `__sfx__` rejects duplicate ids.
- Your formatting survives: the single-column verbs swap one token and let the
  following whitespace absorb the width change, so a hand-aligned grid stays
  aligned and `sl+2`-style effect columns ride along untouched.

The loop is: **`music score` to read what you have → `music edit` to change it
→ `music score`/`music lint` to confirm → `music render` when you want ears on
it.** Everything is `--dry-run`-able first, and every rewrite is re-parsed with
`Cart::parse` before it reaches disk, so a bad edit fails instead of
corrupting the cart.

### ABC import: start from a real tune

`music import-abc` turns a monophonic ABC tune into consecutive `__sfx__`
entries. ABC is the format melodies actually travel in, so this is usually the
fastest way to get *something musical* into a cart and then bend it.

```bash
cat > tune.abc <<'ABC'
X:1
T:The Butterfly
M:9/8
L:1/8
Q:3/8=100
K:Em
|:B2E G2E FED|B2E G2E FED|B2d e2f g3-|g3 gfe dBA:|
ABC

console-agent music import-abc game.cart tune.abc --sfx 2 --inst lead --dry-run
console-agent music import-abc game.cart tune.abc --sfx 2 --inst lead
console-agent music score game.cart --song 0      # read what landed
console-agent music edit  game.cart transpose 2-3 -12   # ... and bend it
console-agent music edit  game.cart set-vol   2 -1     # (set-vol takes one id)
console-agent music lint  game.cart
console-agent music render game.cart -o tune.wav
```

The report tells you everything the mapping decided:

```
import-abc: "The Butterfly": 25 note(s), 0 rest(s) -> 36 row(s)
  key: E minor (F#) | meter: 9/8 | L:1/8 default note
  1 row = 1/8 note; speed=12 frames per row (Q:3/8=100)
  sfx ids: 2 (32 rows), 3 (4 rows)
  split at row 32 (the 32-row cap per sfx); a held note simply restates its row...
  suggested __music__ tempo header: bpm=150 rows_per_beat=2 (speed=auto then resolves to 12)
  suggested __music__ pattern(s):
    pat 0 : 2 - - -
    pat 1 stop : 3 - - -
  warning: repeat mark `|:` unrolled once: ...
```

`import-abc` writes `__sfx__` only — paste the suggested `pat` lines into
`__music__` yourself (consecutive pattern ids chain by the sequencer's "next
existing id" rule, so two sfx play back to back with no `loop=`).

Facts worth knowing before you import:
- **One row = the gcd of the tune's note lengths**, so every note is a whole
  number of rows. A held note **repeats its row** (the console has no
  note-off). On a wave digit or a flat instrument that is sample-identical to
  a sustain; on an `env`/`sweep`/`duck` instrument every repeat re-attacks —
  import with a flat voice if you want legato.
- **Repeats are played once.** `|: … :|` is a `__music__` `loop=`, not an sfx
  feature, and the importer warns and moves on.
- **Out-of-range notes are an error that names the ABC token and computes the
  transpose that fits** — copy the `--transpose <n>` out of the message.
- Tempo comes from `Q:` (with the rounding to whole frames reported), or
  `--speed` if you have a number in mind. No `Q:` means "assume quarter=120",
  said out loud.
- Supported: `M: L: Q: K: V:`, octave marks, accidentals with key-signature
  and bar-local memory, all seven modes, rests, length multipliers/divisors,
  ties, broken rhythm (`>` `<`), bars, repeats/endings, chords (first note
  kept), grace notes and decorations (dropped). Tuplets `(3` and voice
  overlays `&` are rejected by name — rewrite them at their true lengths or
  split the voices.
- Multi-voice files import **voice 1** and warn; run the command again per
  voice with a different `--sfx`, then put the parts in different channel
  slots of one pattern.

### Wavetables: your own waveforms

`wavetable <slot 0-7> <32 hex nibbles>` in `__instruments__` defines one
single-cycle wave, 32 samples of 4 bits. Play it with `inst lead wave=w0` or
straight from a sfx row (`A4 w0 6`, exactly like a bare wave digit). Eight
slots; the nibbles may be split into groups for readability.

Nibble `n` plays at `(2n-15)/15`: **`0` is −1.0, `f` is +1.0, `8` is +1/15**.
So `ffffffffffffffff0000000000000000` is literally the square wave, and 4 bits
have no code for zero — an all-`8` table is a quiet DC offset, not silence.
Pair every `8` with a `7` and the table is exactly DC-free. There is **no
interpolation**: the steps alias, and that crunch is the point (Game Boy /
VRC6 / N163 territory), so aim for character rather than a clean sine.

Writing one by hand — sketch the shape in hex, one digit per sample:

```
# a sine: up 8→f, back down through the middle, mirror below the line
wavetable 0 89acdeef ffeedca9 76532110 00112356
# hollow/reedy (odd harmonics only), the Game-Boy wave-channel sound
wavetable 1 8cefeede eedeefec 73101121 11211013
# organ drawbars: fundamental + octave/2 + twelfth/3
wavetable 2 8beffecb bbbaa988 77765544 44310014
# gritty: two rising ramps per cycle, offset
wavetable 3 78899aab bccddeef 01122334 45566778
```

Rules of thumb: start at `8`, rise to `f`, come back through `8`/`7`, fall to
`0`, return — one crossing each way per cycle is a fundamental-strong tone;
extra wiggles are extra harmonics. A ramp (`0011...eeff`) is a saw, a step is a
pulse of whatever width you put the edge at, and sharp corners are bright.
Everything else composes normally: `env`, `vib`, `sweep`, `duck`, `echo=` and
the fx column all work on a wavetable voice. `audio_state`/`audio_events`
report a wavetable voice's wave as `8 + slot` (so `w0` shows as 8).
Undefined slot, bad hex or a nibble count other than 32 is a parse error —
carts never silently fall back to another waveform. Entry 16 of
`carts/soundtest.cart` auditions three tables.

### 2-op FM: wave 6

`inst <name> wave=6 fm=<ratio>,<index>[,<decay>]` gives you one modulator
phase-modulating one carrier, both sine. It is the Genesis/YM2612 sound in one
line, and it is the only wave that *needs* an instrument: a bare `6` in a sfx
row is a parse error, because a digit cannot carry a ratio.

- **`ratio`** 0.5–15 in steps of 0.5 (`0.5 1 2 3.5 7 …`) — the modulator's
  pitch as a multiple of the note. **Integer = pitched** (every sideband lands
  on a harmonic); **half-integer = inharmonic**, which is what makes bells,
  tines and metal. Ratio 1 is the fattest bass there is; 7 and up is glassy.
- **`index`** 0–15 — how much modulation, i.e. how far up the harmonic series
  the energy reaches. `0` is a pure sine (the console has no other one). 1–3
  warm, 4–6 the bass/brass region, 7–10 glassy, 11–15 clangorous.
- **`decay`** 0–15, optional — the **index envelope**, halving the index every
  120 frames (`1`) down to every frame (`15`); `0` holds it flat. *This is the
  trick.* Real struck tones are bright at the attack and dull as they fade, and
  on FM that is the index falling, not the volume. Without a decay an FM patch
  sounds like an organ; with one it sounds played.

`decay` is completely separate from `env`, which is still just level — an
electric piano holds its level while its brightness dies, a bell does the
opposite. Everything else composes normally too (`vib`, `sweep`, `sl`, `arp`,
`duck`, `echo=`), and vibrato/sweeps bend **both** operators, so the timbre
transposes instead of detuning.

#### Recipes — paste these in and go

```
# FM BASS: ratio 1 doubles the fundamental, a big index makes it growl, and
# decay 13 (index halves every 3 frames) kills the growl in a fifth of a
# second. Play it low: A1-C3.
inst fm_bass  wave=6 fm=1,10,13   env=0,10,4

# ELECTRIC PIANO: a half-integer ratio puts the sidebands between the
# harmonics - that slightly-out tine ring. Medium index, medium decay = the
# hammer letting go. Play it mid: G3-C6.
inst fm_epian wave=6 fm=3.5,6,7   env=0,24,2

# BELL: wide ratio + big index puts energy up around the 7th partial, and a
# slow index decay (half-life 90 frames) keeps it ringing. Give it LONG rows -
# a rest cuts it off dead - and play it high: C5-B6.
inst fm_bell  wave=6 fm=7,11,2    env=0,56,1

# BRASS STAB: integer ratio, moderate index, fast-ish decay, and let `env`
# do the swell. Add vib for a section.
inst fm_brass wave=6 fm=2,7,9     env=4,12,4 vib=18,7,6

# WOOD BLOCK / TOM: FM percussion is a sweep plus a dying index.
inst fm_tom   wave=6 fm=3.5,12,15 env=0,6,0 sweep=-10,4
```

Entry 17 of `carts/soundtest.cart` plays the first three over an Am-F-C-G
phrase. `audio_state`/`audio_events` report an FM voice's wave as `6`.

### Periodic noise: wave 7 — and WRITE IT FOUR OCTAVES UP

Wave 5 is white noise. **Wave 7 is the PSG's other noise mode**: the same shift
register with rotate-only feedback, so one set bit circulates and the output is
a fixed `1000000000000000` pattern — a 1-in-16 pulse train. Buzzy, metallic and
completely **tonal**. It needs no parameters, so a bare `7` works in a sfx row
just like `0`-`5`.

**The one number to remember: periodic noise sounds at `note / 16`, which is
exactly `12·log2(16) = 48` semitones — FOUR OCTAVES — below what you write.**

```
A5 7 6      # heard as A1        C6 psg_bass 6   # heard as C2
```

So the playable range is `C0`-`B3` (the note table stops at `B7`, and
`B7/16 = B3`). That is a bass instrument: engine drones, robot voices, low toms.
Write the pitch you want, then transpose the row up four octaves.

Everything else composes (`env`, `vib`, `trem`, `sweep`, `duck`, `echo=`, the fx
column) — only `fm=` is rejected, since there are no operators to modulate. The
pattern is fixed, so unlike wave 5 there is nothing to seed: it renders
identically for every console seed, and two wave-7 voices on two channels hold
two independent pitches. It carries DC (mean −13/16, the same idiom as pulse
12.5%'s −3/4), so don't stack six of them.
`audio_state`/`audio_events` report a periodic-noise voice's wave as `7`.

```
# ENGINE DRONE: hold it, let vibrato do the wobbling. A4 -> heard as A0.
inst engine   wave=7 vib=35,3,0
# ROBOT VOICE: short buzzes, a slide per row, played as a "melody" up high.
inst robot    wave=7 env=0,3,3
# METALLIC TOM: a sweep on wave 7 is the classic falling clang. E6 -> E2.
inst psg_tom  wave=7 sweep=-10,6 env=0,9,0
```

### Tremolo: `trem=<depth>,<rate>[,<delay>]`

Vibrato's twin, on volume instead of pitch, and clocked from the same LFO — so
`vib=20,8,0` and `trem=6,8,0` on one instrument wobble in lockstep.

- **`depth`** 1-15, in **sixteenths of the level**. The gain swings between the
  authored volume and `1 - depth/16` of it — tremolo only ever attenuates, so it
  can never push a mix into the clamp. 2-4 is a shimmer, 6-8 a Leslie/organ
  pulse, 12-15 a gate.
- **`rate`** 1-16, the same units as `vib`: LFO phase units per frame out of 64,
  so one cycle is `64/rate` frames (rate 4 = 16 frames = 3.75 Hz, rate 8 =
  7.5 Hz). Divisors of 64 (1, 2, 4, 8, 16) give whole-frame periods.
- **`delay`** 0-255, optional — frames of flat gain before the LFO starts. The
  gain is exactly unity throughout the delay *and* on the frame it switches on,
  so the note fades into its wobble rather than stepping into it.

It multiplies **after** `env` and `fade` and before the duck and the echo send,
and it works on every wave source — builtins, both noise modes, FM, wavetables.
Two traps: like vibrato's, a `delay` longer than the row means it never speaks;
and the LFO clock starts at **note-on**, so a pad that restates its chord every
8-frame row restarts the wobble eight times a bar. Give tremolo pads long rows
(`speed=64`) or repeat the note less often.

```
# ORGAN / LESLIE PAD: half-depth wobble, one cycle every 16 frames.
inst trem_pad  wave=w1 trem=8,4
# STRING SHIMMER: shallow and fast, held under a lead.
inst strings   wave=4 env=6,10,4 trem=3,10,6
# HELICOPTER / GATE: deep and slow on a noise voice.
inst chopper   wave=5 trem=14,2
```

Entry 18 of `carts/soundtest.cart` plays a wave-7 bassline and tom next to
white-noise hats with a tremolo pad behind them.

## Packaging

```bash
console-pack game.cart -o game.html [--engine web/engine.js] [--template web/template.html]
```

One self-contained HTML file: works from `file://`, phone-ready (the shell
provides the handheld chassis, touch d-pad/A/B/menu, pause menu with
RESUME/RESET/pixel-scaling/volume), and the cart source remains readable,
editable text inside the HTML. In-browser debugging:
`window.__console.audioState()` reports audio pipeline health; a hidden tab
pauses the game (rAF suspension — not a bug).

## Checklist before calling a cart done

- [ ] `console-agent run` a full scripted playthrough: exits clean, no halt
- [ ] Screenshots at the moments that matter, actually looked at
- [ ] `sprite lint` quiet on every anim; strips/onions reviewed
- [ ] `music score` reads as the song you meant, form chain included;
      `music lint` quiet (or every warning deliberate); piano-roll eyeballed
      (`music edit` and `music import-abc` are how you get there without
      hand-shifting rows — every one of them re-parses before it writes)
- [ ] `audio_events` matches the score in the running game; `audio_stats`
      shows no clipping; spectrogram eyeballed
- [ ] Determinism: same seed + input script run twice ⇒ identical
      `screen_text` output
- [ ] Packed HTML loads, plays, and its cart section is still readable text

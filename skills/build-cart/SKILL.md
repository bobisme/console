---
name: build-cart
description: How to build, test, and ship games (carts) for this fantasy console — cart format, Lua API, the headless dev loop with console-agent, sprite/animation authoring tools, music/sfx authoring, determinism rules, and single-file HTML packaging with console-pack. Use when creating or modifying a cart, authoring sprites/animations/music, debugging cart behavior, or packaging a game.
---

# Building carts for this console

A cart is ONE plain-text file (`*.cart`): Lua 5.4 code + sprites as hex
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

144×256 portrait, fixed 16-color palette (Sweetie-16, indices 0–15), 60fps
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
audio_stats spectrogram{path} wav{path}` plus the `sprite_*` mirrors of the
CLI tools below.

Iterate in small steps: change one thing → step → screenshot/eval →
verify. `screen_text` returns the framebuffer as 256 rows of 144 hex chars —
cheap for asserting "pixel (x,y) is color c" without vision. Save states
are replays (`seed` + input log), so they reproduce everything, audio
included.

## Cart anatomy

```
__meta__          title=... author=... version=...
__lua__           the game (Lua 5.4, sandboxed)
__sprites__       128 lines x 128 hex chars; 1 char = 1 pixel; sprite n at (n%16*8, n//16*8)
__map__           up to 64 lines x up to 128 cells; 2 hex chars = 1 tile id (00-ff); tile 00 = empty
__gfx_meta__      sprite <name> rect=tx,ty size=WxH [anchor=px,py]
                  anim <sprite>.<label> frames=i0,i1,... fps=N [loop]
__instruments__   inst <name> wave=0-5 [env=a,d,s] [vib=cents,rate,delay] [sweep=semis,frames] [duck=depth,release] [echo=0-8]
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
- Tile map: `map([cel_x,cel_y,sx,sy,cel_w,cel_h])` draws a block of map cells as
  sprites (bare `map()` = the whole 128x64 map at 0,0; tile 0 cells are skipped;
  camera/clip/pal/palt apply exactly as to `spr`), `mget(cx,cy)` reads a tile id
  (0 off the map), `mset(cx,cy,v)` writes one.
- Draw state (all four PERSIST across frames — nothing auto-resets them):
  - `camera([x],[y])` — offset every later draw by `-(x,y)`; no args resets.
    `pget` and `cls` are unaffected.
  - `clip([x,y,w,h])` — clip rect in SCREEN space (after the camera); no args
    resets to full screen. `cls` respects it, so it clears the window.
  - `pal(c0,c1)` — draw-palette remap: pixels are actually written as `c1`.
  - `pal(c0,c1,1)` — DISPLAY remap, applied at scanout only. The framebuffer
    keeps its indices, so `for i=0,15 do pal(i,0,1) end` fades the whole
    screen to black with no redraw (and `screen_text` still shows the real
    pixels). Flashes: `pal(i,7,1)`. `pal()` with no args resets both maps
    AND `palt`.
  - `palt(c,flag)` — which colors `spr()` skips (default: only 0). Tested on
    the sprite's SOURCE color, before `pal()` remaps it. `palt()` resets.
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

## Determinism rules (non-negotiable)

- NEVER iterate string-keyed tables with `pairs()` — order is unstable
  across runs. Use `ipairs`, numeric `for`, or explicit key lists.
- There is no wall clock; animate from `t()` or frame counters.
- Randomness only via `rnd`/`srand` (seeded). `math.random` raises.
- The sandbox removes io/os/debug/require/load. Don't fight it.
- A Lua error halts the cart permanently (the harness surfaces the
  traceback; the web shell shows a crash overlay).

## Sprite & animation authoring

Draw by editing hex in `__sprites__` (one hex digit per pixel). Never
hand-shift hex — use the transforms. Declare every sprite and anim in
`__gfx_meta__` (anchor at the feet/contact point for characters, visual
center for floaters). Anim frame `i` = the sprite's rect displaced `i`
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
console-agent sprite lint   game.cart [anim ...]                            # JSON quality numbers
console-agent sprite edit   game.cart copy|shift|flip|rotate|clear ... [--dry-run]
console-agent sprite dump   game.cart <sprite|anim|tx,ty,w,h> [--frame N]    # print pixels as hex rows
console-agent sprite poke   game.cart <target> [--frame N] --rows r0,r1,... # write pixels back
console-agent sprite poke   game.cart <target> [--frame N] --stdin          # rows on stdin, one per line
```

Write pixels with `poke` instead of hand-editing hex: `dump` a region to see
its rows (a `#`-comment header plus one hex-digit-per-pixel row per line,
same alphabet as `__sprites__`), edit the rows you got back, then `poke`
them — `--stdin` is the better fit for agents (`sprite dump ... | sprite
poke ... --stdin` round-trips cleanly; poke skips `#`-prefixed lines so the
dump header passes through harmlessly). `poke` validates row count and row
width against the target's region exactly and rejects non-hex characters;
`--dry-run` previews the changed lines without writing.

Animation workflow: `copy` an existing frame → nudge pixels in the hex →
`lint` until quiet → `onion`/`strip` for the visual pass. Quality gates:
zero/near-zero centroid drift relative to the anchor, silhouette area
steady within ~15%, no colors unique to a single frame (usually a typo'd
hex digit), small `changed_pixels` between adjacent frames. At runtime,
drive frames from your own Lua table + `t()` (the runtime does not read
`__gfx_meta__`; keep the two in sync).

## Tile map authoring

The map is hex text like the sprite sheet, but **2 chars per cell**, and the
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
ground, `1x` a row of decoration) — you will be editing this by hand.

`map()` is the same pixel path as `spr()`, so a scrolling level is
`camera(cam_x, cam_y) map()` and the clip rect windows it for free. For dynamic
terrain — a block you break, a door that opens, a tile that burns — use
`mset(cx, cy, id)`; the change persists across frames and replays
deterministically, so it needs no save/restore of its own. `mget` is also your
collision lookup: `mget(flr(x/8), flr(y/8)) ~= 0` is "is there something here".

To check the map without vision, `screen_text` after a `map()` draw, or
`--eval "mget(3,4)"`.

## Music & sfx authoring

You cannot hear. Work three layers, in order:

1. **Ground truth**: `audio_state` (per-channel sfx/row/note/vol/music
   ownership) and `audio_events` (note_on/row_change/note_off log with
   frame numbers). Verify notes and timing here first — it reads like a
   score.
2. **Stats**: `audio_stats` — per-window RMS/peak/clipped. No clipping, no
   dead air, levels balanced.
3. **Vision + humans**: `spectrogram{path}` (semitone × time heatmap —
   melodies read as note blocks) and `--wav` for human ears.

Facts that bite:
- `env` sustain is an ABSOLUTE level — quiet rows on an env instrument
  swell UP to it. Voices needing per-row dynamics should carry no env.
- Vibrato `delay` must fit inside the row or it never speaks.
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
- [ ] `audio_events` matches the intended score; `audio_stats` shows no
      clipping; spectrogram eyeballed
- [ ] Determinism: same seed + input script run twice ⇒ identical
      `screen_text` output
- [ ] Packed HTML loads, plays, and its cart section is still readable text

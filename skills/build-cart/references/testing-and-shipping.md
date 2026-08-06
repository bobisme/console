# Testing and shipping guide

Use this guide to turn a cart from a plausible source file into a deterministic,
visually inspected, audio-checked, browser-verified single-file game.
Every `run`, `playtest`, `pack`, and `serve` example may replace `game.cart`
with a project directory or explicit `console.toml`; projects compile in
memory, and live serving recompiles on each GET/HEAD refresh.

## Contents

- [Evidence ladder](#evidence-ladder)
- [Fast smoke loop](#fast-smoke-loop)
- [Incremental JSON-RPC loop](#incremental-json-rpc-loop)
- [Developer hooks](#developer-hooks)
- [Playtest scenarios](#playtest-scenarios)
- [Determinism checks](#determinism-checks)
- [Visual acceptance](#visual-acceptance)
- [Audio acceptance](#audio-acceptance)
- [Package to HTML](#package-to-html)
- [Browser acceptance](#browser-acceptance)
- [Repository gates](#repository-gates)
- [Definition of done](#definition-of-done)

## Evidence ladder

Use the cheapest evidence that can disprove the current hypothesis, then climb:

1. Cart parses and initializes.
2. Lua state transitions match exact assertions.
3. Raw framebuffer/audio data matches expectations.
4. Rendered screenshots, sprite/map views, piano rolls, and spectrograms look
   correct.
5. A human plays/listens in the packed browser shell.
6. Repeatable scenarios and project checks guard the result.

A green process exit does not prove a fun/readable game. A screenshot alone does
not prove logic, collision, input, audio, or deterministic replay.

## Fast smoke loop

Run short, focused scripts while editing:

```bash
console run game.cart \
  --frames 180 \
  --input '30:,60:R,1:RA,30:R,59:' \
  --seed 0 \
  --screenshot /tmp/game-f180.png \
  --screenshot-zoom 2 \
  --eval-after 'return dev_status()' \
  --wav /tmp/game.wav \
  --audio-events \
  --audio-stats \
  --text-events
```

Use `--eval-before CODE` when a run needs deterministic setup that must exist
before frame 1, such as `dev_start()`, a warp, or a dense stress state. It runs
after cart top-level code and `_init`, but before input is latched. Use
`--eval-after CODE` for the one JSON inspection result after all frames:

```bash
console run game.cart --frames 180 --input '180:A' \
  --eval-before 'dev_stress()' \
  --eval-after 'return dev_status()' \
  --screenshot /tmp/stress.png
```

Flag order does not change lifecycle order. Screenshots, screen text, audio,
and event captures are collected after the post-frame eval. The setup return
value is discarded; only the post-frame result is serialized, last, on stdout.
`--eval` is an alias for `--eval-after`, but prefer the explicit name in new
automation.

Keep input segments at meaningful boundaries: start, hold direction, press an
action for one frame, release, observe recovery. `btnp` requires a transition,
so a long held `A` segment triggers it once.

Use `--screen-text` for exact pixel assertions or deterministic hashes, not as
a replacement for looking at the image. It emits raw draw-space colors after
`mosaic`/`rshift` but before display-palette remapping.

Use `--text-events` while building menus and HUDs. Every JSON line names the
source text, frame, `left|center|right` alignment, world and camera-adjusted
anchor, screen-space `x,y,width,height`, color, and whether the logical ink
envelope was visible or clipped. This turns a vague screenshot offset into an
exact layout diagnosis. Prefer `print(text,96,y,c,"center")` for full-screen
headings and `print(text,right,y,c,"right")` for numeric HUD columns.

## Incremental JSON-RPC loop

Start `console rpc` when several observations share one boot/session.
Send one JSON object per line:

```json
{"jsonrpc":"2.0","id":1,"method":"load_cart","params":{"path":"game.cart","seed":7}}
{"jsonrpc":"2.0","id":2,"method":"step","params":{"frames":30,"input":""}}
{"jsonrpc":"2.0","id":3,"method":"save_state","params":{"name":"before_jump"}}
{"jsonrpc":"2.0","id":4,"method":"step","params":{"frames":1,"input":"A"}}
{"jsonrpc":"2.0","id":5,"method":"step","params":{"frames":20,"input":"R"}}
{"jsonrpc":"2.0","id":6,"method":"eval","params":{"code":"return dev_status()"}}
{"jsonrpc":"2.0","id":7,"method":"screenshot","params":{"path":"/tmp/jump.png","zoom":2}}
{"jsonrpc":"2.0","id":8,"method":"text_events","params":{"from_frame":30}}
{"jsonrpc":"2.0","id":9,"method":"ecs_query","params":{"world":"arena","with":["enemy","pos"],"select":{"enemy":["kind","hp"],"pos":["x","y"]},"limit":32}}
{"jsonrpc":"2.0","id":10,"method":"load_state","params":{"name":"before_jump"}}
```

Use named replay states for alternative input branches. A state restores by
reset and replay, so it validates reproducibility rather than hiding VM state.
After a CLI command rewrites the cart, call `load_cart` again; the session does
not watch files.

For ECS-heavy games, use `ecs_query` instead of serializing the entire world
through `eval`. Ask only for the component fields needed to test the current
hypothesis, assert `alive`/`matched`/`returned`, and follow `next_after` while
`truncated` is true. This keeps inspection bounded even when hundreds of
bullets are live. Do not step between pages when they must describe the same
world snapshot. Pair it with a small `dev_status()` for semantic game state
such as phase, score, peak entity count, and dropped-spawn count.

## Developer hooks

Expose small Lua globals that report or arrange state through the real game
logic. Keep them deterministic and harmless in normal play:

```lua
function dev_status()
  return {
    scene=scene,
    player={x=player.x,y=player.y,vx=player.vx,vy=player.vy},
    grounded=player.grounded,
    collectibles=collectibles,
    won=won,
  }
end

function dev_start()
  if scene == "title" then start_game() end
end

function dev_warp(x,y)
  player.x,player.y=x,y
  player.vx,player.vy=0,0
end
```

Prefer calling shared functions (`start_game`) over mutating half of a state
transition. Hooks should help reach expensive states, while at least one
scenario still exercises the player-facing path to them.

## Playtest scenarios

Promote valuable smoke scripts into strict versioned JSON:

```json
{
  "version": 1,
  "seed": 0,
  "stages": [
    {"op":"eval","name":"start","code":"dev_start()"},
    {"op":"assert","code":"return dev_status().scene","equals":"play"},
    {"op":"input","name":"approach","frames":90,"buttons":"R"},
    {"op":"input","name":"jump","frames":1,"buttons":"RA"},
    {"op":"input","frames":30,"buttons":"R"},
    {"op":"assert","code":"return dev_status().grounded","equals":false},
    {
      "op":"capture",
      "name":"jump_apex",
      "screenshot":"jump-apex.png",
      "zoom":2,
      "screen_text":"jump-apex.txt",
      "text_events":"jump-text.json",
      "draw_trace":"jump-draws.json",
      "audio_events":"jump-events.json",
      "audio_stats":"jump-stats.json"
    }
  ]
}
```

Run it:

```bash
artifact_dir=$(mktemp -d)
console playtest game.cart --scenario playtests/game.json \
  --artifacts "$artifact_dir" --format json
```

Scenario principles:

- name semantic stages;
- assert after the state-changing input, not only at the end;
- capture transition moments, failure states, and victory;
- use `draw_tag` plus `draw_trace` when a screenshot does not reveal which
  system produced a stray primitive, sprite, or off-camera effect;
- compare exact JSON values and expose stable status values for floats when
  precision would otherwise make assertions brittle;
- keep paths relative/unique beneath one explicit artifact root;
- stop at the first failure and inspect its logs/artifacts before editing.

Cover at least title/start, core movement/action, one collision edge case, one
collectible or progression event, failure/recovery, and completion for a full
game.

## Determinism checks

Run the same cart, seed, and input twice and compare raw outputs:

```bash
tmp_a=$(mktemp -d)
tmp_b=$(mktemp -d)
console run game.cart --seed 9 --frames 300 \
  --input '60:R,1:RA,120:R,119:' --screen-text --wav "$tmp_a/audio.wav" \
  > "$tmp_a/screen.txt"
console run game.cart --seed 9 --frames 300 \
  --input '60:R,1:RA,120:R,119:' --screen-text --wav "$tmp_b/audio.wav" \
  > "$tmp_b/screen.txt"
cmp "$tmp_a/screen.txt" "$tmp_b/screen.txt"
cmp "$tmp_a/audio.wav" "$tmp_b/audio.wav"
```

Use task-specific variable names in automation and remove temporary directories
only when their exact paths are known. A different seed should affect only the
features designed to use randomness.

Investigate nondeterminism in this order:

1. `pairs`-dependent order;
2. unseeded/reseeded random calls;
3. state initialized outside `_init` assumptions;
4. input segment boundary mistakes;
5. logic depending on visual/audio inspection results or external state.

## Visual acceptance

Capture and actually inspect:

- title/first impression;
- neutral gameplay framing;
- core action at its strongest pose;
- dense/high-motion scene;
- damage/failure feedback;
- progression/victory;
- camera/world boundaries;
- pause/menu shell in the packed page.

Review at two scales:

- enlarged nearest-neighbor pixels for artifacts, palette mistakes, anchors,
  and seams;
- actual packed phone size for silhouette, text, touch obstruction, and visual
  hierarchy.

Use domain tools before screenshots: sprite strip/onion reveals motion more
clearly than one game frame, and map render with IDs reveals wrong cells more
clearly than gameplay.

For a runtime action, capture the whole motion deterministically in the
scenario instead of choosing one favorable screenshot:

```json
{"op":"sequence","name":"tongue swing","frames":24,"buttons":"B","every":3,
 "crop":{"x":24,"y":64,"w":144,"h":128},"zoom":2,"columns":4,
 "gif":"tongue.gif","strip":"tongue-strip.png","board":"tongue-board.png"}}
```

The GIF timing follows the 60 Hz sample cadence. The strip makes pose-to-pose
spacing obvious. The board labels every sampled frame, crop, and integer zoom;
its optional reference panel remains at native resolution and explicitly says
it is not pixel-aligned. Inspect the board as a qualitative composition target,
not as an automated similarity score.

For a reference-driven or visually dense game, follow the complete evidence
bundle, temporal checks, readability lint, and independent blind-review
protocol in
[visual-direction-and-review.md](visual-direction-and-review.md). Consolidate
that evidence in a final `review` stage rather than handing off a cherry-picked
still.

## Audio acceptance

Static cart checks:

```bash
console music score game.cart
console music lint game.cart --strict
console music piano-roll game.cart -o /tmp/music.png
console music render game.cart --loops 2 -o /tmp/music.wav
```

For a project with a native bundle, audition the lossless source first, then
inspect the compiled cart. The static music tools operate on a cart, not on a
standalone `.cmusic` file:

```bash
console music play my-game/audio/game.cmusic --song 0 --dry-run
console build my-game
console music play my-game --song 0 --dry-run
console music score my-game/build/game.cart
console music lint my-game/build/game.cart --strict
console music piano-roll my-game/build/game.cart --song 0 -o /tmp/music.png
console music render my-game/build/game.cart --song 0 --loops 1 -o /tmp/music.wav
console build my-game --check
```

If the bundle replaced audio in an older cart, inspect `audio_events` during a
gameplay action using the same seed/input as the old and new carts. This catches
ID collisions where a legacy gameplay `sfx(id)` now triggers a music phrase, and
remapped cues that steal all six music channels. A successful native bundle
dry-run alone cannot catch runtime integration errors.

Running checks:

- `audio_events`: correct trigger frame, pattern, row, and SFX stealing;
- `text_events`: intended alignment, camera-adjusted bounds, and no clipping;
- `audio_state`: channel ownership/current note;
- `audio_stats`: RMS/peak/clipped windows;
- spectrogram: pitch contour, transients, harmonics;
- WAV: human listening for musical feel, harshness, balance, and loop seam.

Test audio unlock in the browser with an actual trusted key/pointer gesture.
Autoplay restrictions mean correct native samples do not prove audible browser
playback.

## Package to HTML

From any directory after installing `console`:

```bash
console pack game.cart -o dist/game.html
```

The packer validates a cart, or compiles and validates a project, then embeds
engine/cart/template into one HTML
file. The result should have zero external requests and run directly from a
`file://` URL. The cart text remains editable inside
`<script type="text/cart">`. The default engine and template are embedded in
the binary; pass `--engine` or `--template` only to test an override.

For a live browser loop, run `console serve game.cart`. It prints the local URL
and recompiles/re-bundles saved changes on refresh. Keep its loopback default unless a
second device must connect; use `--port 0 --once` for deterministic scripts.

Confirm the output exists, is nonempty, contains the cart title/source, and has
no accidental external asset dependency. Do not modify the embedded cart while
also leaving the source cart divergent; repack from the source of truth.

## Browser acceptance

Open the exact packed output, not a development substitute. Verify:

1. URL is the packed file and status reaches ready.
2. Frames advance and framebuffer is nonuniform.
3. Keyboard and trusted touch/pointer controls work, including diagonals.
4. A/B/game-menu mappings match the cart.
5. Device pause halts stepping; resume has no catch-up burst.
6. RESET returns to a clean initialized game.
7. FIT/SHARP scaling works and preserves 3:5 aspect ratio.
8. Volume control and first-gesture audio unlock work.
9. No network requests occur.
10. Browser console has no errors; runtime crashes show the expected overlay.
11. The game is readable and playable at the target phone viewport.

Inside the platform repository, run the browser harness directly against a
specific packed cart when its normal start/action path matches the harness:

```bash
CONSOLE_BROWSER=/path/to/chromium \
  node web/browser-smoke.cjs dist/game.html \
  --artifacts out/game-browser-check
CONSOLE_BROWSER=/path/to/chromium \
  node web/diagnostics-smoke.cjs dist/game.html
```

The generic smoke expects frames to change during its probe and trusted A input
to produce nonzero audio. A static title, different start button, or delayed
audio needs a cart-specific browser interaction sequence; do not weaken the
game merely to satisfy those generic assumptions.

The frozen read-only diagnostic handle helps automation:

```javascript
window.__console.status()
window.__console.screenState()
window.__console.audioState()
```

`status()` reports lifecycle, frame count, input, pause, and fatal state.
`screenState()` reports logical/backing/CSS dimensions, palette/index health,
and framebuffer hash. `audioState()` reports output mode/errors, frames pushed,
nonzero audio, context state/sample rate, and volume. The handle intentionally
exposes no mutation methods.

## Repository gates

When working in the platform checkout:

```bash
just check
CONSOLE_BROWSER=/path/to/chromium just browser-check
```

`just check` covers formatting, Clippy, native tests, docs, and native/WASM
smokes. The opt-in browser check packs Lantern Leap, opens the exact `file://`
artifact, drives trusted controls, and checks lifecycle/framebuffer, palette,
audio, pause/resume/reset, network isolation, and browser errors. Inspect any
retained failure artifacts under `out/browser-check/`.

For a cart-only change, also run its specific playtest scenarios and inspect
new visuals/audio; platform gates cannot decide whether the game is good.

## Definition of done

- [ ] Cart parses, initializes, and completes representative scripted runs.
- [ ] Core game states have exact playtest assertions.
- [ ] Same seed/input reproduces pixels and audio.
- [ ] Representative screenshots were visually inspected at enlarged and phone scale.
- [ ] Dense action, camera boundaries, tagged layers, grayscale, and collision context were reviewed where visual readability matters.
- [ ] Effects begin at current rendered sockets in every facing and relevant pose.
- [ ] Sprite animations passed intentional lint thresholds and strip/onion review.
- [ ] Maps passed lint; blank tile IDs, seams, collision, and camera bounds were checked.
- [ ] Music score/form is intentional; lint findings are fixed or explained.
- [ ] Running audio events/stats match gameplay and do not unintentionally clip/steal.
- [ ] A human listening pass occurred when musical quality matters.
- [ ] Single-file HTML was produced from the current cart.
- [ ] Exact packed file passed keyboard/touch, pause/reset/scaling/audio, console, and zero-network checks.
- [ ] Cart source inside the packed HTML remains readable and editable.
- [ ] Repository/project checks pass when applicable.

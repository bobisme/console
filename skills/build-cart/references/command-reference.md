# Command and JSON-RPC reference

Use this reference for exact `console` CLI and JSON-RPC syntax.
Run the relevant `--help` in the active checkout if a locally installed binary
may be newer than this skill.

## Contents

- [`console` inventory](#console-inventory)
- [`run`](#run)
- [`playtest`](#playtest)
- [`rpc`](#rpc)
- [`build`](#build)
- [`scene compile`](#scene-compile)
- [`pack`](#pack)
- [`serve`](#serve)
- [`palette` commands](#palette-commands)
- [`sprite` commands](#sprite-commands)
- [`map` commands](#map-commands)
- [`music` commands](#music-commands)
- [JSON-RPC protocol](#json-rpc-protocol)
- [Session RPC methods](#session-rpc-methods)
- [Sprite RPC methods](#sprite-rpc-methods)
- [Map RPC methods](#map-rpc-methods)
- [Music RPC methods](#music-rpc-methods)
- [CLI-only operations](#cli-only-operations)

## `console` inventory

```text
console --help
console run ...
console playtest ...
console rpc
console build <project|console.toml> ...
console scene compile <scene.toml> --out <directory> ...
console pack <cart|project> -o <out.html> ...
console serve <cart|project> ...
console palette <show|quantize> ...
console sprite <render|atlas|strip|onion|diff|ghost|gif|lint|edit|dump|poke|export|import> ...
console map <render|dump|lint|edit|poke> ...
console music <score|lint|piano-roll|render|edit|import-abc|midi-to-abc|play> ...
```

`-h` and `--help` are accepted at the top level or anywhere after a top-level
subcommand. They print the relevant usage and exit 0; for `palette`, `sprite`,
`map`, and `music`, that usage covers the entire command family. Every
image/audio render command below accepts equivalent `-o` and `--out`
output-path forms.

Top-level/family help exits 0. Invalid CLI syntax generally exits 2; cart load,
runtime, assertion, or artifact failures generally exit 1.

## `build`

```text
console build <project|console.toml>
  [-o|--out out.cart]
  [--check]
  [--format text|pretty|json | --json]
```

Compile a version-1 `console.toml` project into the normal text cart format.
The manifest may repeat `[[sprites]]` to place tile-aligned PNGs explicitly;
build generates their sheet and named graphics metadata using exact, nearest,
or deterministic quantized Apollo64 conversion. `[audio].bundle` may name a
`console-music 1` `.cmusic` file; build validates and expands its native audio
sections. It cannot be combined with raw audio keys under `[sections]`.
The argument may be the project directory or its explicit manifest. With no
`-o`, the output is `[build].output`, defaulting to `build/game.cart` under the
project. An explicit output is interpreted as the supplied CLI path.

`--check` never writes and succeeds only if that output already matches the
deterministic compiled bytes. Normal builds validate the complete cart and
atomically replace the output. Manifest inputs must be relative UTF-8 files
confined to the project root, including after symlink resolution. See
[platform and cart format](platform-and-cart-format.md#multi-file-projects) for
the manifest schema and separated-section rules. Every report format includes
canonical inputs, Lua source provenance, and PNG sprite provenance. JSON
exposes `lua_sources` entries
with module/source names plus original and generated line ranges; text emits
one `lua_source=module|path|start-end` line per source. JSON `sprite_assets`
entries record source, placement, dimensions, anchor, conversion policy, color
budget and final palette indices; text emits one `sprite_asset=` line each.

## `scene compile`

```text
console scene compile <scene.toml> --out <directory>
  [--check]
  [--format text|pretty|json]
```

Compile a strict version-1 layered-scene manifest into `atlas.png`, `map.txt`,
`tile_classes.lua`, `decorative_layers.lua`, `objects.lua`,
`provenance.json`, and labeled `review/*.png` evidence. The declared atlas
rectangle is the only sprite-sheet region allocated. Tiles with identical
pixels and semantic class share an ID; different classes never deduplicate.

Inputs are relative, confined PNG/semantic-grid/play-grid paths. PNG dimensions
must be tile aligned and are never resized. Exact Apollo64 mapping is the
default; nearest and quantize are explicit and produce error evidence. Quantize
requires an atlas-wide color budget; the union of every layer output must fit
it, and reports preserve that budget plus the alpha threshold. The
manifest can declare tile edges, metatiles, N/E/S/W autotile mask lookups,
seeded weighted variants, stamps, overrides, and nonoverlapping anchored
objects. Invalid masks, capacity, bounds, or paths fail before publication.

`--out` is mandatory. A normal compile atomically replaces each finished
artifact after full validation. `--check` never writes and succeeds only when
every expected artifact is byte-identical and no obsolete managed lossy heatmap
remains. Scene layers retain at most 32,768 cells in aggregate. See the [maps
guide](maps-and-metatiles.md#compile-a-layered-scene) and the normative schema
in the repository `SPEC.md`.

## `run`

```text
console run <cart|project>
  [--frames N]
  [--input SPEC]
  [--screenshot out.png] [--screenshot-zoom N]
  [--screen-text]
  [--eval CODE]
  [--seed N]
  [--wav out.wav]
  [--spectrogram out.png]
  [--audio-events]
  [--audio-stats]
  [--text-events]
  [--draw-trace trace.json]
```

`SPEC` is comma-separated `COUNT:BUTTONS`, for example
`30:,10:R,5:RA,60:`. Letters are `L R U D A B M`; separators/whitespace inside
button strings are accepted. If `--frames` is omitted, the command runs the
sum of input-segment counts. If it exceeds the input length, remaining frames
are idle. An empty spec plus `--frames N` is an idle run.

| Option | Result |
|---|---|
| `--frames N` | Total fixed frames to step. |
| `--input SPEC` | Per-frame button masks. |
| `--screenshot FILE` | Final PNG after stepping and `--eval`. |
| `--screenshot-zoom N` | Integer nearest-neighbor PNG scale, at least 1; default 1. |
| `--screen-text` | Print 320 framebuffer rows of 192 palette characters. |
| `--eval CODE` | Evaluate after stepping; JSON-serialize the result to stdout. |
| `--seed N` | Initial deterministic seed; default 0. |
| `--wav FILE` | Write all retained audio as 16-bit mono PCM WAV. |
| `--spectrogram FILE` | Write the retained audio as a PNG, default cell 4. |
| `--audio-events` | Print one JSON sequencer event per line. |
| `--audio-stats` | Print JSON mix windows using 6 frames/window. |
| `--text-events` | Print one JSON text-draw event per line, including resolved bounds. |
| `--draw-trace FILE` | Write a bounded JSON draw-call trace for all stepped frames and the final eval. |

`printh` lines go to stderr as `[log] ...`. A readable cart that fails to load,
a project that fails to compile, a halted runtime, or a failed eval exits 1
after reporting the error. An unreadable/missing input path exits 2, as does
invalid CLI syntax.
`<project>` may be a directory containing `console.toml` or the manifest path;
it is compiled and validated in memory without writing `[build].output`.

## `playtest`

```text
console playtest <cart|project> --scenario <scenario.json>
  [--artifacts DIR]
  [--seed N]
  [--format text|pretty|json | --json]
```

- `--artifacts` is required if a stage writes files.
- `--seed` overrides the scenario seed.
- `--format` defaults to terminal-sensitive auto selection.
- `--json` is an alias for `--format json`.
- Exit 0: every stage passed. Exit 1: assertion/execution/capture failed.
  Exit 2: CLI or strict scenario schema invalid.

Version 1 schema:

```json
{
  "version": 1,
  "seed": 0,
  "stages": [
    {"op":"eval", "name":"setup", "code":"dev_warp(48,200)"},
    {"op":"input", "name":"jump", "frames":12, "buttons":"RA"},
    {"op":"assert", "code":"return dev_status().grounded", "equals":false},
    {
      "op":"sequence",
      "name":"jump arc",
      "frames":12,
      "buttons":"R",
      "every":3,
      "crop":{"x":32,"y":80,"w":128,"h":120},
      "zoom":2,
      "columns":2,
      "gif":"jump.gif",
      "strip":"jump-strip.png",
      "board":"jump-board.png",
      "reference":"jump-reference.png"
    },
    {
      "op":"capture",
      "screenshot":"jump.png",
      "zoom":2,
      "screen_text":"jump.txt",
      "text_events":"jump-text.json",
      "draw_trace":"jump-draws.json",
      "wav":"jump.wav",
      "spectrogram":"jump-spectrum.png",
      "audio_events":"jump-events.json",
      "audio_stats":"jump-stats.json",
      "map":{
        "source":"live",
        "png":"jump-map.png",
        "dump":"jump-map.txt",
        "lint":"jump-map.json",
        "region":"0,0,32,16",
        "zoom":4,
        "grid":true,
        "ids":true
      },
      "from_frame":0,
      "to_frame":120,
      "window_frames":6,
      "cell":4
    }
  ]
}
```

Every stage permits an optional unique `name`. `input.frames` must be at least
1; total input frames may not exceed 36,000. Capture paths must be unique,
relative descendants of `--artifacts`, and cannot traverse `.`/`..`, absolute
paths, or symlinks. Screenshot zoom is 1–16. Spectrogram cell is 1–8 and its
range at most 3,600 frames. Audio-stat windows are 1–36,000 frames.
Nested map captures accept `source: "authored"` (default) or `"live"` and one
or more output paths: `png`, `dump`, `lint`. Optional `region` uses
`cx,cy,cw,ch`; omitted regions use that snapshot's nonzero extent. Map zoom is
1–16 (default 4). `grid` and `ids` affect only the PNG.

`sequence` holds one input mask for `frames` and samples after every `every`
frames; the two values must divide exactly and yield at most 240 samples.
`crop` uses native 192x320 coordinates. `zoom` 1–16 is always nearest-neighbor,
and `columns` 1–16 lays out the board. Request one or more of `gif`, `strip`,
and `board`. An optional `reference` requires `board`, resolves relative to the
scenario file, and is embedded untouched at native size with a
`NOT PIXEL-ALIGNED` label rather than a similarity score.

## `rpc`

```text
console rpc
```

Read one JSON-RPC 2.0 object per stdin line and emit one response per stdout
line, flushed immediately. Blank input lines are ignored. Keep the process alive
to load/step/inspect one session incrementally.

## `palette` commands

```text
console palette show [-o|--out out.png] [--cell N]

console palette quantize <input.png> (-o|--out) <output.png>
  [--colors 1-63]
  [--alpha-threshold 0-255]
  [--dither none]
  [--format text|pretty|json]
```

`show` writes the exact Apollo64 colors as an 8x8 swatch grid; `--cell`
selects each swatch's pixel size and defaults to 16. With no output path it
writes `apollo64.png`.

`quantize` never resizes. It preserves transparent pixels, selects at most the
explicit `--colors` budget from opaque indices 1-63, maps pixels
deterministically, and reports selected indices plus color/error statistics.
Alpha below `--alpha-threshold` becomes transparent. `--dither` accepts only
`none`; unsupported modes fail rather than silently changing pixel clusters.
The default budget is 16 colors and the default alpha threshold is 128.
Report format precedence is explicit `--format`, then the `FORMAT` environment
variable, then TTY-aware pretty or piped text output.

## `sprite` commands

Targets are a declared sprite name, declared animation name, or raw tile rect
`tx,ty,w,h`. View zoom defaults to 8. `--grid` shows 8×8 boundaries;
`--indices` labels palette indices; `--anchor` draws the declared anchor.

### Inspect and render

```text
console sprite render <cart> <target> [--frame N] [--zoom Z]
  [--grid] [--indices] [--anchor] (-o|--out) out.png

console sprite atlas <cart> [--zoom Z] [--grid]
  (-o|--out) out.png

console sprite strip <cart> <anim> [--zoom Z] [--anchor]
  (-o|--out) out.png

console sprite onion <cart> <anim> --frame N [--zoom Z]
  [--grid] [--anchor] (-o|--out) out.png
console sprite onion <cart> <anim> --all [--zoom Z]
  [--grid] [--anchor] (-o|--out) out.png

console sprite diff <cart> <anim> <frameA> <frameB>
  [--zoom Z] (-o|--out) out.png

console sprite ghost <cart> <anim> [--zoom Z]
  [--grid] [--anchor] (-o|--out) out.png

console sprite gif <cart> <anim> [--zoom Z]
  [--grid] [--anchor] (-o|--out) out.gif

console sprite dump <cart> <target> [--frame N]

console sprite export <cart> <target> [--frame N]
  [--palette source] (-o|--out) out.png
```

- `render`: one resolved frame/rect.
- `atlas`: annotated full sheet; JSON on stdout inventories named allocations,
  anchors, resolved frames, palette counts, blank/unused cells, and classifies
  same-sprite aliases separately from cross-sprite conflicts.
- `strip`: all frames side by side and anchor/baseline aligned.
- `onion --frame`: current full opacity, previous red, next green; loop-aware.
- `onion --all`: contact sheet centered on every frame.
- `diff`: later frame dimmed with changed pixels magenta.
- `ghost`: motion accumulation over every frame.
- `gif`: declared animation timing in an animated GIF.
- `dump`: palette-character rows with a `#` header suitable for `poke --stdin`.
- `export`: exact-size source-index PNG; source color 0 is transparent.

### Lint

```text
console sprite lint <cart> [anim ...]
  [--max-drift PX]
  [--max-area-var PCT]
  [--max-changed PX]
  [--no-unique-colors]
  [--summary]
```

With no thresholds, emit a report and exit 0. Any threshold turns lint into a
gate: violations produce exit 1 and a `violations` array. `--summary` prints one
compact line per animation while preserving gate behavior.

### Write pixels and transform regions

```text
console sprite poke <cart> <target> [--frame N]
  --rows <pixels,pixels,...> [--dry-run]
console sprite poke <cart> <target> [--frame N]
  --stdin [--dry-run]

console sprite edit <cart> shift <target> [--frame N]
  [--dx N] [--dy N] [--wrap] [--dry-run]
console sprite edit <cart> flip <target> [--frame N]
  --horizontal|--vertical [--dry-run]
console sprite edit <cart> rotate <target> [--frame N]
  --cw|--ccw [--dry-run]
console sprite edit <cart> copy <src> <dst> [--dry-run]
console sprite edit <cart> clear <target> [--frame N] [--dry-run]

console sprite import <cart> <target> [--frame N]
  --input in.png
  [--mapping exact|nearest]
  [--alpha-threshold 0-255]
  [--max-colors 1-63]
  [--dry-run]
  [--format text|pretty|json]
```

`poke` requires exact height/width and valid palette characters; `--stdin`
skips `#` comment lines. Edit targets accept sprite/anim/raw rect; copy endpoints
accept `sprite[:frame]` or raw rect and must match size. Rotate requires a square
region. Shift fills vacated pixels with color 0 unless `--wrap`.

`import` also requires exact target dimensions. Exact mapping is the default
and rejects non-Apollo RGB; nearest mapping must be explicit. `--max-colors`
is a gate, not an implicit quantizer. Alpha below the threshold maps to source
color 0. Dry runs report changed pixels/rows and never write.

## `map` commands

Regions use cell coordinates `cx,cy,cw,ch`. Render/dump/poke default to the
used nonzero extent (or one origin cell on an empty map). Edit operations always
require an explicit region.

```text
console map render <cart> [cx,cy,cw,ch] [--zoom Z]
  [--grid] [--ids] (-o|--out) out.png
console map dump <cart> [cx,cy,cw,ch]
console map lint <cart>

console map poke <cart> [cx,cy,cw,ch]
  --rows <hex,hex,...> [--dry-run]
console map poke <cart> [cx,cy,cw,ch]
  --stdin [--dry-run]

console map edit <cart> copy <cx,cy,cw,ch> <dest_cx,dest_cy>
  [--dry-run]
console map edit <cart> shift <cx,cy,cw,ch>
  [--dx N] [--dy N] [--dry-run]
console map edit <cart> fill <cx,cy,cw,ch> <tile-hex> [--dry-run]
console map edit <cart> clear <cx,cy,cw,ch> [--dry-run]
```

`render --ids` labels nonempty cells. `dump` emits two hex digits/cell with a
pipeable comment header. `lint` reports extent, counts, fill, and map IDs whose
8×8 sheet tiles are blank. Poke rows must be exactly `2*cw` hex characters.
Shift drops cells leaving the named region and zero-fills the vacancy; it does
not wrap.

## `music` commands

### Inspect and render

```text
console music score <cart> [--song N]
console music lint <cart> [--strict]
console music piano-roll <cart>
  [--song N | --patterns a,b,c] [--cell N] [--row-h N]
  (-o|--out) out.png
console music render <cart>
  [--song N] [--loops K | --frames F] [--seed N]
  (-o|--out) out.wav
```

`--song N` follows the same pattern chain as `music(N)`; default is the lowest
defined pattern. `lint` emits JSON and exits 0 unless `--strict`. Piano-roll
renders pitch vs frame and may select an explicit pattern list. `render` boots
the cart and renders the intro plus 2 loop-body passes by default; `--frames`
overrides loop detection.

### Edit SFX rows

```text
console music edit <cart> transpose <sfx-ids> <semitones>
  [--clamp] [--dry-run]
console music edit <cart> copy <src-sfx> <dst-sfx>
  [--force] [--dry-run]
console music edit <cart> shift-rows <sfx-id> <n> [--dry-run]
console music edit <cart> set-vol <sfx-id> <0-7|+n|-n> [--dry-run]
console music edit <cart> set-inst <sfx-id> <inst|0-5|w0-w7>
  [--where old] [--dry-run]
console music edit <cart> stretch <sfx-id> <2|0.5>
  [--force] [--dry-run]
```

`<sfx-ids>` accepts one ID, an inclusive range, or comma combinations such as
`0,2,5-7`. Signed values like `-12` are operands. `transpose` errors before
leaving C0–B7 unless `--clamp`. `copy --force` may replace a destination.
`shift-rows` rotates. `set-vol` preserves rests and clamps relative changes.
`set-inst --where` rewrites only matching voices. `stretch 2` inserts rests and
reduces speed; `stretch 0.5` drops odd rows (requires `--force` if they contain
notes) and increases speed.

Only `transpose` accepts the range/list form `<sfx-ids>`. `copy`, `shift-rows`,
`set-vol`, `set-inst`, and `stretch` take the single IDs shown in their syntax;
invoke them once per target ID when changing several entries.

### Import ABC

```text
console music import-abc <cart> <file.abc|-> --sfx <start-id>
  [--inst <name|0-5|w0-w7>]
  [--vol 0-7]
  [--speed 1-255]
  [--transpose N]
  [--force]
  [--dry-run]
```

Import a monophonic tune into consecutive SFX IDs; `-` reads stdin. The command
prints grid/tempo decisions, split points, warnings, and suggested pattern lines.

### Convert source music and play native audio

```text
console music midi-to-abc <in-file.mid>
  [(-o|--out) out-file.abc]
console music play <file.abc|file.mid|file.cmusic|file.cart|project>
  [--song 0-63]
  [--seconds N]
  [--volume 0..1]
  [--repeat]
  [--dry-run]
```

`midi-to-abc` writes ABC to stdout when `--out` is absent, leaving warnings on
stderr so the result is safe to pipe. `-o` atomically replaces the named file.
It accepts Standard MIDI format 0/1 with PPQ timing, preserves absolute gaps
and note lengths, and splits simultaneous notes into sequential `V:` voices.
It rejects format 2 and SMPTE timing. Later MIDI tempo changes produce a
warning because the generated ABC header carries the initial tempo only. A
non-integer initial BPM is rounded to the nearest ABC `Q:` value and warns. Bad
command syntax exits 2 with usage; input, parse, and output-file failures exit
1 without polluting stdout or appending usage.

`play` accepts MIDI by `.mid`/`.midi` extension or `MThd` signature and parses
other non-native UTF-8 input as ABC. `.cmusic` carries the versioned native
audio sections; `.cart`, `console.toml`, and project directories supply those
sections through the ordinary cart/project loader. `--song` is native-only and
defaults to the lowest pattern. Native input is isolated from game Lua and
rendered through the exact instrument, effect, master, echo, and pattern-chain
runtime. An authored loop plays its intro once and loops its body; `--repeat`
restarts a one-shot. `--seconds` makes playback finite and, with `--repeat`,
selects the prefix to loop. Native device playback preserves runtime synth and
effect state across authored passes. One-shots drain the click-guard release
and taper its last 64 samples so echo reaches a silent seam; explicit time cuts
use the same taper before ending or restarting.

All forms play through the default host output. `--volume` is a linear host
gain from 0 to 1 and defaults to 0.5. `--dry-run` parses, validates, and plans
native input without opening a device, blocking, or allocating rendered PCM;
use it in CI and agent checks.
Decode/render/device failures exit 1, while bad CLI syntax exits 2.

ABC preview keeps the first `Q:` tempo and warns on later changes. It rejects
over-complex duration arithmetic instead of wrapping or panicking. Source
reads, shared ABC headers, voice counts, event counts, and preview duration are
all bounded. A final console-synth release frame makes the output end silent;
the command reports source duration and release frames separately.

## `pack`

```text
console pack <cart|project> -o <out.html>
  [--engine FILE]
  [--template FILE]
```

| Option | Meaning |
|---|---|
| `-o`, `--out`, `--output` | Required destination. |
| `--engine FILE` | Override the browser engine embedded in the executable. |
| `--template FILE` | Override the HTML template embedded in the executable. It must contain `{{TITLE}}`, `{{CART_TEXT}}`, and `{{ENGINE_JS}}`. |
| `-h`, `--help` | Print full help. |

The packer compiles project inputs in memory, validates the resulting cart, and
produces a zero-request HTML
file that works from `file://`. Because the default engine and template are
compiled into `console`, packing works from any current directory.

## `serve`

```text
console serve <cart|project>
  [--host HOST]
  [--port PORT]
  [--engine FILE]
  [--template FILE]
  [--once]
```

`serve` performs the same validation and in-memory bundle as `pack`, then
serves it at `/` and `/index.html`. It defaults to
`http://127.0.0.1:8000/`, prints the actual URL on stdout, sends
`Cache-Control: no-store`, and recompiles/re-bundles on each GET or HEAD so
saved cart or project-source edits appear immediately. A failed project build
returns an error response and never serves the previous page as fresh.
`--port 0` asks the OS for a free port. `--once`
exits after one connection and is useful for scripts and tests. Use `--host`
only when another device must reach the development server; the default is
intentionally loopback-only. Requests must use a `Host` authority matching the
configured host and actual port; wildcard binds accept IP-literal hosts only.

## JSON-RPC protocol

Request and success/error envelopes:

```json
{"jsonrpc":"2.0","id":1,"method":"info","params":{}}
{"jsonrpc":"2.0","id":1,"result":{}}
{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"..."}}
```

One line is one request. Important error codes: `-32700` parse error,
`-32601` unknown method, `-32602` bad params, `-32002` no cart loaded, and
`-32000` cart/runtime/halted errors (often with detail in `data`).

## Session RPC methods

| Method | Params | Result / behavior |
|---|---|---|
| `load_cart` | `{path, seed?}` or `{text, seed?}` | Load and run `_init`; `{ok,title,seed}`. `text` wins if both are supplied. |
| `reset` | `{seed?}` | Reload current cart, optionally replacing seed; clear input/audio/event logs while named save states survive. |
| `step` | `{frames?=1,input?=""}` | Input string or integer mask; return `{frame_count,halted,message}`. |
| `screenshot` | `{path,zoom?=1}` | Write PNG; zoom integer ≥1; return path/dimensions. |
| `screen_text` | `{}` | `{lines}`: 320 strings × 192 palette characters, raw draw-space indices. |
| `eval` | `{code}` | Execute chunk; `{result}` JSON conversion. |
| `get_global` | `{name}` | Return one global as `{result}`. |
| `ecs_query` | `{world,with?=[],select?={},limit?=64,after?=0}` | Read a bounded field projection from one named ECS world in stable creation order. |
| `logs` | `{}` | Drain `printh` lines as `{logs}`. |
| `save_state` | `{name}` | Save a replay checkpoint by name. |
| `load_state` | `{name}` | Reset/replay it; return frame/halt state. |
| `info` | `{}` | Frame, seed, halt, title/meta, input-log length, saved-state names. |
| `wav` | `{path,from_frame?,to_frame?}` | Write retained range; return frames/samples/duration. |
| `audio_state` | `{}` | Current music pattern and per-channel sequencer state. |
| `audio_events` | `{from_frame?}` | Sequencer events at/after the bound. |
| `audio_stats` | `{window_frames?=6}` | RMS/peak/clipped counts over mix windows. |
| `text_events` | `{from_frame?}` | `print` calls at/after the bound with anchors, bounds, alignment, visibility, and clipping. |
| `draw_trace` | `{enabled,clear?}` | Enable/disable bounded recording for later calls; mode changes or `clear:true` clear the trace. |
| `draw_events` | `{from_frame?,tag?,clear?}` | Return trace status and calls; optionally filter by frame/tag and clear after reading. |
| `spectrogram` | `{path,from_frame?,to_frame?,cell?=4}` | Write PNG; return windows/dimensions. |

Saved states are reset-plus-replay, so they reproduce pixels, map mutations,
audio samples, sequencer events, text events, and enabled draw traces rather
than serializing opaque VM memory.

`ecs_query` is the agent-facing ECS inspector. `with` is a dense array of at
most 16 required component names. `select` maps at most 8 component names to
dense arrays of at most 16 scalar fields; an empty field array includes a
scalar component value or an empty object for a table component. `limit` is
1–128 and `after` is the previous page's `next_after` entity ID. Example:

```json
{"jsonrpc":"2.0","id":7,"method":"ecs_query","params":{"world":"arena","with":["hostile","pos"],"select":{"hostile":["kind"],"pos":["x","y"]},"limit":32,"after":0}}
```

The result includes `frame_count`, world counts, capacity, registered
`component_type_count`, total `matched`,
page `returned`, `truncated`, `budget_exhausted`, `next_after`,
`component_counts`, and ordered `{id,components}` entries. Projection is capped
at 2048 scalar cells and 32768 string bytes (256 bytes per string); unsupported
Lua types become placeholders. This method uses a protected read-only
inspector retained by the host, so replacing the cart's public `ecs` table does
not disable it.

Draw traces distinguish primitives from `spr`/`sspr`/`aspr`/`map`, snapshot
camera, clip, non-identity palette remaps, transparency, and fill state, and
report world, screen, and visible bounds. Omitted palette indices are identity
mapped. The core cap is 4096 calls per frame; the session retains
the newest 65536 calls and reports all drops. Use Lua `draw_tag("actors")` (and
`draw_tag()` to clear) for stable layer/system filtering. Tracing does not alter
rendered pixels. It records subsequent frame-step and host-eval draws, not cart
top-level or `_init` calls that already ran while the console was loading.

## Sprite RPC methods

All operate on the loaded cart and do not step it.

| Method | Params |
|---|---|
| `sprite_render` | `{target,path,frame?,zoom?,grid?,indices?,anchor?}` |
| `sprite_atlas` | `{path,zoom?,grid?}` |
| `sprite_strip` | `{anim,path,zoom?,anchor?}` |
| `sprite_onion` | `{anim,path,frame?=0,all?,zoom?,grid?,anchor?}` |
| `sprite_diff` | `{anim,path,frame_a,frame_b,zoom?}` |
| `sprite_ghost` | `{anim,path,zoom?,grid?,anchor?}` |
| `sprite_lint` | `{anims?:[string],max_drift?,max_area_var?,max_changed?,no_unique_colors?,summary?}` |

Image methods return `{ok,path,width,height,frames}`. `sprite_atlas` instead
returns the semantic report plus an `image` object containing its path and
dimensions. `sprite_lint` returns
`violated` because RPC has no process exit code; `violations` appears when
thresholds are active. There is no RPC GIF, dump, poke, or edit method.

## Map RPC methods

| Method | Params | Result |
|---|---|---|
| `map_render` | `{path,source?:"authored"|"live",region?:"cx,cy,cw,ch",zoom?,grid?,ids?}` | Image result. |
| `map_dump` | `{source?:"authored"|"live",region?:"cx,cy,cw,ch"}` | `{text}` hex rows. |
| `map_lint` | `{source?:"authored"|"live"}` | Whole-map JSON lint object. |

The optional region defaults to used extent. Source defaults to the immutable
authored map; `live` snapshots mutations from the current session. There is no
RPC poke/edit method.

## Music RPC methods

| Method | Params | Result |
|---|---|---|
| `music_score` | `{song?}` | `{text}` song chain and score. |
| `music_lint` | `{}` | JSON diagnostics. |
| `music_piano_roll` | `{path,song?,patterns?,cell?,row_h?}` | Image result plus pattern order/loop pattern. |

There is no `music_render` RPC: use `eval` to call `music(n)`, then `step` and
`wav`. There is no RPC edit/import operation.

## CLI-only operations

Every operation that rewrites cart text is intentionally CLI-only:

```text
sprite edit, sprite poke, sprite import
map edit, map poke
music edit, music import-abc
```

Run them between RPC sessions and reload the cart. Static `sprite gif`, PNG
`sprite export`, palette commands, and raw `sprite dump` are also CLI-only.
This boundary prevents a running session from silently disagreeing with a
rewritten file.

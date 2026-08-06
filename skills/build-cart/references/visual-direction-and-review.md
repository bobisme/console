# Visual direction and strict review

Use this guide when a game is mechanically working but still looks confusing,
generic, inconsistent, or far below its reference art. It defines an
evidence-driven builder-reviewer loop for art direction and readability. The
tools expose facts and repeatable views; a reviewer still makes the aesthetic
judgment.

## Contents

- [Translate reference art into constraints](#translate-reference-art-into-constraints)
- [Establish the gameplay value hierarchy](#establish-the-gameplay-value-hierarchy)
- [Separate foreground from background](#separate-foreground-from-background)
- [Compose environments as continuous places](#compose-environments-as-continuous-places)
- [Keep actors and effects coherent](#keep-actors-and-effects-coherent)
- [Build the visual evidence bundle](#build-the-visual-evidence-bundle)
- [Use explicit temporal and semantic checks](#use-explicit-temporal-and-semantic-checks)
- [Run a strict builder-reviewer loop](#run-a-strict-builder-reviewer-loop)
- [Apply the blind review protocol](#apply-the-blind-review-protocol)
- [Know what automation cannot decide](#know-what-automation-cannot-decide)

## Translate reference art into constraints

A reference image is art direction, not a literal screen layout or a bitmap to
trace. Before drawing, write down what must transfer:

- value structure: which plane is darkest, which elements own the brightest
  accents, and where the eye lands first;
- depth grammar: sky, distant mass, middle architecture, playable terrain,
  actors/effects, and HUD;
- shape language: chunky or thin silhouettes, hard industrial rectangles,
  organic curves, repeated motifs, and edge treatment;
- density rhythm: where detail clusters and where negative space lets action
  breathe;
- material roles: which ramps mean steel, masonry, glass, foliage, water,
  hazards, or interactables;
- focal hierarchy: player first, immediate route second, threat or objective
  third, decoration last;
- atmosphere: time of day, temperature, contrast, and emotional tone.

Also record what must *not* transfer: portrait framing that conflicts with the
live camera, impossible sprite detail, decorative platforms that would lie
about collision, or colors that cannot survive Apollo64 reduction.

Do not treat a resized and quantized reference as the implementation target.
That preview is useful for palette feasibility and mood, but it does not solve
tile repetition, camera continuity, collision grammar, animation, or gameplay
readability.

## Establish the gameplay value hierarchy

Judge every live frame in grayscale before polishing hue. A strong default
hierarchy is:

1. player silhouette and active attack;
2. traversable edges, hazards, and immediate interaction target;
3. enemies, pickups, and route landmarks;
4. middle-ground structures;
5. distant skyline, clouds, stars, and texture.

This is not a demand that the player always be the brightest object. The
player may be mid-value with a bright face or outline, while hazards own a
different accent. What matters is stable separation from the local backdrop.

At native 192×320 resolution, ask:

- Can the player be located in under one second without motion?
- Can a new viewer point to every surface that is safe to stand on?
- Are hazards recognizable before reading their color?
- Does the intended objective beat decorative lights for attention?
- Does dense combat remain legible when effects overlap the actor?

If any answer is no, remove or darken competing detail before adding more
outline, bloom, or particles.

## Separate foreground from background

Use a collision-exclusive visual grammar. Playable terrain should own a small
set of cues that background architecture never borrows together:

- one continuous top-edge value or accent;
- a consistent cap thickness;
- a dark supporting face beneath the cap;
- contact shadows at feet or object bases;
- material seams aligned to the collision grid.

A background may echo the same world material, but it should not reproduce the
complete platform signature. Long bright background horizontals are especially
dangerous because they read as walkable ledges.

Prefer stable palette roles over screen-space separation effects. For example:

- author background ramps one or two luma steps below gameplay terrain;
- reserve the palest steel and warm hazard colors for collision/contact cues;
- wrap background draws in a constant plane-0 `pal(source,darker)` mapping,
  then deliberately restore the draw palette and transparency state before
  terrain/actors (`pal()` also resets `palt`);
- use plane-1 `pal(...,1)` only for intentional whole-screen fades/flashes.

Do not change the background palette abruptly by camera zone. Do not overlay a
screen-fixed haze, checker, or dimming rectangle on a scrolling world unless
that screen-space motion is itself the desired style; it will swim over the
scene. Keep any palette mapping stable across the level or interpolate it at a
deliberately tested transition.

Negative space is part of the collision grammar. Leave quieter cells around
the player route, jump apex, grapple arc, and enemy telegraphs. Concentrate
windows, pipes, antennae, and signs away from those envelopes.

## Compose environments as continuous places

Build each depth plane for the full traversable camera range, not one screenshot
at a time:

- distant plane: broad low-contrast masses and slow parallax;
- middle plane: fewer landmarks with readable silhouettes;
- gameplay plane: exclusive platform/hazard grammar and collision truth;
- foreground accents: sparse occluders that never hide required contact edges.

Use recurring motifs and a controlled landmark cadence to create place. Avoid
filling every cell with a unique prop. A detailed environment normally needs
large quiet shapes underneath its texture.

Capture both sides of every camera boundary. A camera crossing must scroll or
reveal one continuous composition; it must not trigger a full-screen scenery
swap unless the game explicitly communicates a room transition. Compare the
two frames outside an allowed actor/effect region and inspect a diff heatmap.

For platformers and grapple games, review the complete movement envelope:

- grounded neutral and run;
- jump rise, apex, and fall;
- grapple latch, low-tension tuck, high-speed extension, and release;
- fall/death at pits or water;
- dense combat and mutation effects;
- camera extremes and boss framing.

## Keep actors and effects coherent

An animation frame must look like the same character, not a replacement asset.
Keep these contracts stable across poses:

- head scale and defining facial features;
- torso mass and limb thickness;
- semantic palette ramps and outline language;
- anchor, collision contact, and facing convention;
- authored sockets for eyes, mouth, hands, feet, weapons, and effects.

Do not reuse a grounded idle frame for a suspended state merely because its
pixel quality is stronger. Preserve the design language while changing the
pose: tucked legs, shifted mass, tension, anticipation, and recovery must read
in silhouette.

Effects begin at rendered sockets, not remembered coordinates. When a sprite
changes, verify the live tongue, beam, flame, projectile, shadow, and hit flash
in both facings. Add a deterministic probe assertion and draw-trace evidence
that check the authored socket before the effect and its first rendered point.

Expose that relationship directly when the scenario cannot query draw-trace
JSON in-process. For example, have `dev_visual_probe()` return pose, facing,
rendered eye socket, beam root, and a derived `beam_socket_ok`, then assert it
after arranging each pose:

```json
{"op":"assert","name":"beam follows rendered eye",
 "code":"return dev_visual_probe().beam_socket_ok","equals":true}
```

Keep a `draw_trace` capture beside the assertion so a failure still names the
actual draw calls. The Boolean must compare the same socket/root values used by
the renderer; a duplicated hard-coded expectation merely recreates the bug.

Particles must communicate an event. Ground dust requires meaningful contact
velocity; smoke requires damage, heat, or an emitter. A continuously spewing
actor often means the trigger lacks a state/velocity threshold.

## Build the visual evidence bundle

Do not send a reviewer only the best screenshot. Produce a repeatable bundle:

- native-size actor-in-environment stills from representative districts;
- enlarged nearest-neighbor crops for pixel and socket inspection;
- complete action GIFs/strips, including direction changes and recovery;
- color, grayscale, luma-band, edge, and palette-index diagnostic views;
- isolated `draw_tag()` layers for background, terrain, actors, effects, HUD;
- live collision-map context from the same frame;
- dense-effects, damage, failure, boss, and victory frames;
- both sides of every camera/room boundary;
- the reference image labeled as qualitative, not pixel-aligned evidence;
- packed phone-viewport evidence and a manual play gate.

Use a final scenario `review` stage to consolidate the evidence. Keep stage and
tag names semantic and exact.

A single review board isolates tags from one exact `layers.stage`. For broader
coverage, make ordinary `capture` stages write layer PNGs at each important
district/pose, include those named stages as full composites in the final
board, and choose the densest or riskiest stage for its isolated layers and
lint. Use separate scenarios/final boards when several states each need a full
isolated-layer board; a `review` stage must be the final stage of its scenario.

## Use explicit temporal and semantic checks

The review stage supports game-authored mechanical checks. This example guards
a camera boundary, stationary scenery during a swing, and common readability
risks without assigning an art score:

```json
{
  "op": "review",
  "board": "review/visual-board.png",
  "report": "review/visual-report.json",
  "stages": ["boundary-left", "boundary-right", "swing"],
  "views": ["color", "grayscale", "luma_bands", "edges", "palette_index"],
  "motion_samples": 4,
  "layers": {
    "stage": "boundary-right",
    "tags": ["background", "terrain", "actor"]
  },
  "map": {
    "stage": "boundary-right",
    "source": "live",
    "region": "0,12,24,28",
    "zoom": 1,
    "grid": true,
    "ids": true
  },
  "temporal_checks": [
    {
      "kind": "boundary",
      "name": "continuous-background",
      "from": "boundary-left",
      "to": "boundary-right",
      "max_changed_fraction": 0.35,
      "allowed_regions": [{"x": 64, "y": 128, "w": 64, "h": 96}],
      "heatmap": "review/boundary-diff.png"
    },
    {
      "kind": "consecutive",
      "name": "no-static-shimmer",
      "stage": "swing",
      "max_changed_fraction": 0.08,
      "allowed_regions": [{"x": 32, "y": 72, "w": 128, "h": 176}],
      "heatmap": "review/swing-diff.png"
    }
  ],
  "lint": {
    "reserved_collision_colors": {
      "source_tag": "background",
      "indices": [59, 63]
    },
    "bright_background_horizontals": {
      "background_tag": "background",
      "min_luma": 192,
      "max_run": 48
    },
    "actor_background_luma": {
      "actor_tag": "actor",
      "background_tag": "background",
      "min_gap": 28
    },
    "traversal_corridor_edges": {
      "background_tag": "background",
      "region": {"x": 16, "y": 112, "w": 160, "h": 160},
      "min_luma_delta": 24,
      "max_edge_fraction": 0.22
    }
  }
}
```

The five listed `views` are also the default. The final board generates those
derived panels automatically; they do not need separate output filenames.
`map` renders authored or live tile topology captured at the named prior stage,
so use the live source when runtime collision or destruction can differ from
the authored map. Choose a region that covers the visible collision context.

Choose thresholds from the game and its intended motion. `allowed_regions`
use the compared source's coordinates; for a cropped sequence, that means the
crop's coordinate space rather than world or full-screen coordinates. They
exclude actor/effect motion from both changed count and denominator. Boundary
checks compare named still stages; consecutive checks report the worst retained
motion pair. Failed temporal checks preserve board, report, and heatmap
evidence. Lint findings are warnings for reviewer attention, not failures.

Tune a threshold only after inspecting the evidence. Never weaken a global
limit merely to hide an unexplained violation.

Boundary diffs compare screen coordinates and do not compensate for camera
translation or parallax. Capture the frames immediately before and after the
suspect threshold, measure ordinary adjacent scrolling pairs with the same
camera velocity away from the threshold, and set the authored limit just above
that non-boundary baseline. A discontinuity should exceed normal scrolling;
the check is not a promise that a moving camera leaves the screen unchanged.

## Run a strict builder-reviewer loop

1. Builder states one visual hypothesis, such as “platform caps need an
   exclusive value” or “the tuck frame needs folded legs.”
2. Builder changes the smallest coherent system—or performs a full overhaul if
   the art direction is structurally wrong.
3. Builder regenerates the complete evidence bundle with the same seed/input.
4. A fresh reviewer examines reference and live evidence without being told
   which implementation details to defend.
5. Reviewer records specific observable failures, severity, and evidence.
6. Builder fixes substantiated findings and adds a regression where the defect
   is mechanical.
7. Repeat until the review threshold is met and a manual play pass confirms the
   game still feels good in motion.

Keep technical and aesthetic verdicts separate. A technically correct asset
may still have weak composition; an attractive screenshot may still lie about
collision or break in motion.

## Apply the blind review protocol

Before reading implementation notes, ask the reviewer to perform these tasks on
native stills and motion:

1. Locate the player immediately.
2. Mark every traversable surface and hazard.
3. Name the primary attention target and intended route.
4. Identify depth planes and any element whose plane is ambiguous.
5. Compare silhouette, value hierarchy, density rhythm, and atmosphere to the
   supplied reference.
6. Watch the sequence for shimmer, scene swaps, anchor drift, detached effects,
   and UI/world motion conflicts.
7. Score or approve only after writing concrete evidence for the weakest area.

A useful review report includes:

- overall score or decision and the acceptance threshold;
- separate readability, composition, sprite coherence, motion, effects, and
  reference-fidelity assessments;
- top three blocking defects with exact frame/artifact locations;
- which defects are mechanical enough to automate;
- what improved since the previous pass.

Do not ask a single reviewer to infer fun, art direction, collision truth,
audio quality, and browser usability from one screenshot. Give each review a
clear scope, and use a stricter visual-only reviewer when sprites or scenery are
the remaining weakness.

## Know what automation cannot decide

Diagnostic metrics can reveal change, density, luma gaps, palette-role leaks,
and discontinuities. They cannot decide whether a skyline is evocative, a frog
pose is funny, a boss entrance feels dramatic, or a composition has taste.

Final acceptance still requires:

- human inspection of native and enlarged evidence;
- motion review, not isolated frames only;
- a manual play pass at packed phone size;
- explicit acknowledgment of remaining aesthetic compromises.

Use automation to prevent regressions and make critique precise. Keep the
creative verdict human and honest.

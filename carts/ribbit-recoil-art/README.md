# RIBBIT RECOIL art production spike

This directory proves a deterministic bridge from `paintop` into the console's
Apollo64 sprite sheet. `paintop` is an optional authoring companion, not a game
or console runtime dependency.

The checked-in specimen is a native 24x24 idle frog:

- source graph: `frog-idle.paintop.json`
- rendered PNG: `frog-idle-paintop.png`
- 43 typed nodes
- seven nontransparent Apollo64 indices: `8, 12, 14, 31, 38, 48, 63`
- zero partially transparent pixels
- `paintop` output content hash:
  `blake3:4e4b00ab519f2f2364dd27c0eb22752f3db27a31243f4f7830a62f5f6f8cdc9a`
- normalized plan semantic hash:
  `blake3:4ae47496968120ee0483319baf24b8983cdc4517bede9fe00bc997b49695cb7d`

This is a pipeline specimen and pose-language starting point, not the final
frog atlas. The authored-frog bone remains responsible for stronger anatomy,
compression, directional poses, mutation silhouettes, and animation timing.
Those assets must follow the shared palette, cluster, lighting, material, and
allocation contract in [`ART-BIBLE.md`](ART-BIBLE.md).

## Production frog atlas

`frog-atlas.pixels` is the compact exact-pixel source for the ten production
frog poses and six overlays allocated by the art bible. It uses the cart's
one-character Apollo64 index alphabet so it round-trips without color inference.
Rebuild those cells in a cart with:

```bash
bash carts/ribbit-recoil-art/build-frog-atlas.sh carts/ribbit-recoil.cart
```

The builder validates every full frame as exactly `24x24`, every overlay as
exactly `8x8`, and writes only the allocated sheet rectangles through
`console sprite poke`. `--extract` performs the inverse operation and is used
to refresh the checked-in source after an exact PNG import. The runtime scopes
these exact-index sprites to the identity draw palette, then restores the legacy
primitive palette for unmigrated art.

## Production enemy atlas

`enemy-atlas.pixels` stores the nine common-insect poses, ten `16x16`
Buzzkill modules, and two `24x24` pod cores allocated by the art bible. Rebuild
only those exact cells with:

```bash
bash carts/ribbit-recoil-art/build-enemy-atlas.sh carts/ribbit-recoil.cart
```

Use `--extract` after an exact PNG import to refresh the compact source. The
runtime draws every insect and boss module inside a scoped identity palette;
Buzzkill's green index `12` accent is the only phase-remapped material.

## Production environment atlas

`environment-atlas.pixels` stores the live `8x8` material kit and the three
generated-then-cleaned `16x16` prop clusters. It includes topology variants,
pipe and fence components, hazard lips, and paired cap/face families for rusted
loading roofs, damp concrete, violet lab panels, and mutagen pipeworks. Rebuild
only those exact cells with:

```bash
bash carts/ribbit-recoil-art/build-environment-atlas.sh carts/ribbit-recoil.cart
```

The runtime assigns collision-equivalent variants deterministically from level
topology and world zone, draws the map in an identity-palette scope, and applies
lamp, sign, coil, and moon highlights only to nearby exposed terrain edges.
`ribbit-recoil-environment-art.playtest.json` captures all seven zones at exact
nearest-neighbor `4x` for native-scale and seam review.

## Environment overhaul references

The continuous-city overhaul is reviewed against `../frog-inspo-console.png`,
the user's console-scale derivative of the original
`../frog-game-design-inspo.png`. The intended review reference is the
derivative: it is the `192x320` image that the strict judge opened beside every
native game capture. It guides hierarchy, atmosphere, lighting, and material
density; it is not shipped as a game background.

Reference provenance is intentionally explicit:

- original user image: `1024x1536`, BLAKE3
  `f4598180ab5ee35242844237b1e52d8f34212f06ae72c1fec2613d099aac7b43`
- console-scale review derivative: `192x320`, BLAKE3
  `a6f3d10145489d236c9e9e7e8997f4680eff8e29d8574fa84010a5714247b30f`
- the derivative was made by the user outside this tracked workspace after a
  suggested Console resize/quantize workflow; its exact crop, resize, and
  quantization invocation was not recorded, so rebuilding it is an explicit
  manual provenance gate rather than a falsely reproducible step

`environment-concept.png` is a generated environment-only composition guide,
not a runtime asset. It was generated in ImageGen reference/edit mode at
`1659x948` from `../frog-inspo-console.png`; the service did not expose a seed.
Its BLAKE3 is
`61a7ae1dbe165298df245e8733c34ef1123f31ffd870f9898309b4e09639fa73`.
The exact prompt was:

> Production seven-zone environment concept sheet directly inspired by the
> updated reference. Environment only; remove frog, enemies, UI, grapple
> targets, and text. Seven adjacent districts: loading roofs, waterworks, molt
> district, gene bar, croak tower, mutagen works, and arena. Dense moonlit
> industrial pixel art with layered night sky, irregular cloud banks, warm
> textured moon, distant city, foreground pipes, tanks, catwalks, vents, signs,
> fences, drains, puddles/canal water, localized cyan/magenta/green/orange
> lighting, coherent Apollo64-like palette, large authored structural masses,
> readable negative space and no full-screen wave band.

Because the generation seed is unavailable, the checked-in concept and hash
are the audit record; regeneration is a manual visual-composition gate. The
seven PNGs under `environment-concepts-native/` are nearest-neighbor crops
quantized through `console palette quantize --colors 32 --dither none`; they are
also composition guides, not runtime assets.

`environment-modules.paintop.lua` is the deterministic Paintop Lua study for a
moonlit sky, cloud bank, industrial facade, tank, catwalk, and sewer edge. Run:

```bash
paintop validate carts/ribbit-recoil-art/environment-modules.paintop.lua
paintop run carts/ribbit-recoil-art/environment-modules.paintop.lua \
  --bundle /tmp/ribbit-recoil-environment-modules
```

The checked-in `environment-modules-study.png` has Paintop output hash
`blake3:60299b0c6f15b9862acf43e3055288bd4b0b71ee40719f123fef26a1ebdc3599`;
the normalized plan hash is
`blake3:c210256e444535d5207c1de4499b3702b565c50a562991a4cc00f728d4bbed4d`.

`environment-cloud-strips.paintop.lua` authors two broad atmospheric banks and
one detached-wisp strip at exactly `96x24`. `environment-moon.paintop.lua`
authors the exact `48x48` broken-halo moon. Their checked-in Apollo64 PNGs are
`cloud-strip-a.png`, `cloud-strip-b.png`, `cloud-strip-c.png`, and `moon.png`.
The final Paintop runs reported:

- cloud output hash:
  `blake3:29d18f3ed6fe6d44140fe09f1c55a5ea668248838d99e6025ca3910a2b08d2e4`
- cloud plan hash:
  `blake3:985210fdfde3422b27e29d6c5775041f177bffbeb24ad3cdec96111e025df25e`
- moon output hash:
  `blake3:f8521bd872c60c1687e71eb34ea2dbf35dc58914d7e10416858459ea1b4fe607`
- moon plan hash:
  `blake3:d44e40141b87e142225170e07992706bd64e8d1d63b5d20520b5f9b8265007e6`

The cart preserves those exact pixels as compact horizontal spans. The live
sheet already occupies 252 of 256 cells, so this keeps the inspectable Paintop
assets exact without displacing gameplay sprites. The scene compiler remains
the preferred route for normal material/map authoring; its executable subset
still compiles with exact mapping and no warnings:

```bash
console scene compile carts/ribbit-recoil-scene/scene.toml \
  --out /tmp/ribbit-recoil-scene-compile --format json
```

That compile uses all five declared subset atlas cells, emits atlas/map/Lua,
review, and provenance artifacts, and reports zero palette error. The span path
is the deliberate exception for these larger backdrop images under the current
sheet budget.

Two native-resolution scenarios guard the result:

```bash
console playtest carts/ribbit-recoil.cart \
  --scenario carts/ribbit-recoil-environment-review.playtest.json \
  --artifacts /tmp/ribbit-recoil-environment-review

console playtest carts/ribbit-recoil.cart \
  --scenario carts/ribbit-recoil-environment-continuity.playtest.json \
  --artifacts /tmp/ribbit-recoil-environment-continuity
```

The first captures all seven districts plus reference comparison boards. The
second captures both sides of all six former camera-zone boundaries; clouds,
moon, skyline, landmarks, light accents, and terrain materials must scroll
through those pairs without a full-screen redraw. The Rust regression also
limits every two-pixel boundary crossing to less than 35 percent framebuffer
change and requires zero dropped draw-trace events. The final strict
environment-only judge pass scored the native captures `7.5/10`; the acceptance
threshold was `7.5`.

## Reproduce and inspect

Run from the console repository root:

```bash
paintop validate carts/ribbit-recoil-art/frog-idle.paintop.json

paintop run carts/ribbit-recoil-art/frog-idle.paintop.json \
  --bundle /tmp/ribbit-recoil-frog-a
paintop run carts/ribbit-recoil-art/frog-idle.paintop.json \
  --bundle /tmp/ribbit-recoil-frog-b

paintop diff \
  /tmp/ribbit-recoil-frog-a/outputs/frog-idle.png \
  /tmp/ribbit-recoil-frog-b/outputs/frog-idle.png

set -o pipefail
console sprite import carts/ribbit-recoil.cart 0,0,3,3 \
  --input /tmp/ribbit-recoil-frog-a/outputs/frog-idle.png \
  --mapping exact --max-colors 8 --dry-run --format json \
  | jq -e '
      .width == 24 and .height == 24 and .resized == false and
      .palette_indices == [8, 12, 14, 31, 38, 48, 63] and
      .partial_alpha_pixels == 0 and .written == false
    '
```

The two `paintop` runs are byte-identical. Exact console import succeeds at
24x24, reports all seven expected indices, and does not rewrite the cart during
the dry run. The `jq` predicate is part of the acceptance gate: it locks the
dimensions, no-resize policy, palette indices, binary alpha, and non-writing dry
run. Exact import can threshold partially transparent pixels, so checking its
report prevents a soft alpha edge from passing silently. `assert.alpha_valid`
only validates numeric alpha range; it does not enforce binary transparency. A
real import into a scratch cart followed by `sprite export` is pixel-identical
after decoding; the PNG files themselves differ because the two tools use
different encoders.

Evidence bundles are intentionally ignored. They contain the normalized plan,
execution trace, graph, assertions, masks, materialized intermediates, and
export. Keep them for a review run or CI artifact rather than duplicating them
in Git.

## Exact-pixel rules

- Create an `rgba`, `srgb`, display-referred, straight-alpha canvas with a
  transparent `[0, 0, 0, 0]` fill.
- Express Apollo colors as exact channel values divided by 255. Verify the
  encoded output with `console sprite import --mapping exact`; do not trust
  visual similarity alone.
- Keep every rectangle and staircase-polygon edge on integer pixel boundaries.
  Diagonal polygon edges, feathers, filters, and fractional masks can produce
  blended non-Apollo RGB values.
- Group disconnected clusters with `mask.union`, then apply one `paint.fill`
  per color role. Preserve source index 0 exclusively through transparent
  alpha.
- Use `console palette quantize` before exact import when a generated or edited
  source is not already Apollo64.

## Production decision

`paintop` is useful for reproducible composition, transformations, masks,
material/VFX synthesis, assertions, and evidence. Its current raw JSON graph is
too verbose for hand-authoring an entire character atlas: this one small frame
needs 43 nodes and a roughly 11 KB plan. Use it where graph reuse or procedural
construction pays for that ceremony; use a pixel editor, image generation, or a
compact authoring frontend for dense frame-by-frame cluster work, then cross the
strict PNG/import bridge. The paintop project accepted that same conclusion in
`bn-3tpc` and is implementing a bounded Lua-to-Plan frontend under `bn-v4fs`;
keep JSON as the canonical evidence/runtime representation and adopt the Lua
frontend when it lands. Do not add a console-specific image DSL to the runtime.

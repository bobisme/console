# RIBBIT RECOIL Apollo64 art bible

This is the production contract for the RIBBIT RECOIL visual rebuild. It turns
the supplied concept image into a native `192x320`, Apollo64-compatible visual
language; it is not a request to copy the reference pixel for pixel.

The reference succeeds because a tiny, bright frog and a few luminous grapple
coils sit inside a deep, layered industrial night. Construction detail supports
that hierarchy instead of competing with it. The rebuild must preserve that
relationship at phone scale.

## Non-negotiable hierarchy

1. **Interaction:** frog, tongue latch, enemies, weak points, mutation fronts.
2. **Route:** platform caps, hazards, doors, breakable material, pickups.
3. **World:** supports, pipes, vents, skyline, water, signs, grime.
4. **Atmosphere:** moon, clouds, haze, distant windows and reflections.

At native size, interaction silhouettes must read before surface detail. A
bright background pixel may never touch a critical eye, hand, tongue, weak
point, or platform contact edge without a dark separator.

The reference gives the moon the largest bright mass, then repeats much smaller
green-white grapple coils and the frog. The current game often gives truss
lines, signs, particles, and actors equal weight. The target keeps the moon broad
but low-frequency, reserves hard contrast for play, and pushes repeated world
detail into darker, quieter ramps.

## Light and depth

The global key comes from the **upper right**, toward the moon. On solid forms:

- top and right-facing edges receive the cool key;
- lower and left-facing planes carry the deepest shadow;
- cast shadows fall down-left and stay connected to the casting form;
- outlines open or brighten on a moon-facing edge, but remain dark on the
  shadow side;
- warm lamps may override the key locally, with a short amber falloff on nearby
  metal or brick instead of a flat triangular beam.

Use four depth bands:

| Band | Value and detail contract |
|---|---|
| Sky | Indices `48,49,50,1`; broad fields, sparse stars, no hard texture behind play. |
| Far city | `49,50,1,2`; one- to three-pixel window groups, merged silhouettes. |
| Mid structures | `49,1,2,3,40,41`; readable masses, about half foreground edge density. |
| Play plane | Material ramps below; darkest contact shadows and sharpest selective highlights. |

Index `63` is a scarce light, not a general white. Reserve it for eye whites,
the hottest mutation pixels, weak-point confirmation, a thin moon rim or crater
glint, and brief impact frames. The moon disk is mostly `61/62`; ordinary
moonlit edges stop at `58` or `61`.

Small actors that flip for facing are an explicit exception to the lateral key:
their permanent shading is top-weighted and nearly symmetric. Never bake a
right-side rim into a frame that will be flipped left. Non-flipped terrain and
separately authored left/right boss modules carry the full upper-right key.

## Semantic inks

Sprite index `0` remains transparent even though Apollo64 assigns it an RGB
color. Every authored PNG uses straight alpha and exact Apollo bytes. A normal
actor frame should use four to seven nontransparent indices.

| Role or material | Apollo64 indices, dark to light | Use |
|---|---|---|
| Night atmosphere | `48 #090a14`, `49 #10141f`, `50 #151d28`, `1 #253a5e`, `2 #3c5e8b`, `4 #4f8fba` | Sky, haze, far silhouettes; never use all six in one small prop. |
| Frog body | `48 #090a14`, `8 #19332d`, `12 #5d943b`, `14 #a8ca58`, `31 #e8c170`, `38 #cf573c`, `63 #ebede9` | Outline, green planes, iris/belly warmth, mouth, eyes. This is the default seven-color hero set. |
| Moonlit steel | `48 #090a14`, `49 #10141f`, `52 #2c3c43`, `54 #485e63`, `58 #94a6a4`, `61 #c7cfcc` | Platform caps, girders, vents, boss machinery. |
| Painted violet | `48 #090a14`, `40 #1e1d39`, `41 #402751`, `42 #5b2f68`, `43 #7a367b`, `46 #d46b9d` | Signs, painted panels, refinery casing; use `46` only on lit chips. |
| Rust and warm metal | `48 #090a14`, `24 #341c27`, `26 #753a2d`, `28 #a4602c`, `30 #de9e41`, `31 #e8c170` | Corrosion, lamps, crates, boss brass. |
| Sewer water and cyan tech | `48 #090a14`, `49 #10141f`, `1 #253a5e`, `3 #4576a3`, `5 #5fa7c7`, `6 #73bed3`, `7 #a4dddb` | Water edges, coils, powered machinery; bright cyan belongs at interaction points. |
| Tongue | `32 #241527`, `34 #5a2138`, `35 #752438`, `38 #cf573c`, `45 #c65197`, `47 #df84a5`, `63 #ebede9` | Tongue shadow/core and latch flash. Avoid pink scenery behind its travel path. |
| Damage and alarm | `32 #241527`, `24 #341c27`, `34 #5a2138`, `36 #a53030`, `38 #cf573c`, `31 #e8c170` | Wounds, refinery alarms, damage frames; no white except the instant of impact. |
| Fire | `24 #341c27`, `36 #a53030`, `38 #cf573c`, `39 #da863e`, `30 #de9e41`, `31 #e8c170`, `63 #ebede9` | Dark red outside, ochre core, white only at ignition/impact. |
| Laser | `48 #090a14`, `1 #253a5e`, `3 #4576a3`, `5 #5fa7c7`, `6 #73bed3`, `7 #a4dddb`, `63 #ebede9` | Dark casing, cyan fringe, one-pixel white core. It must not be confused with the magenta tongue. |

Do not substitute a nearby index because it looks acceptable in isolation.
Material identity comes from repeating these ramps across actors and scenery.

## Pixel-cluster grammar

- Start with a filled silhouette. It must communicate facing, action, and
  contact at `1x` in grayscale before internal detail is added.
- Prefer connected clusters at least two pixels across. On actors and props, a
  frame may use at most two pixels with no same-role neighbor in their eight
  surrounding cells; reserve them for an eye glint, droplet, spark, or similarly
  meaningful accent. Directional VFX debris and distant stars may exceed that
  limit, but must form a deliberate arc, trail, or depth rhythm.
- Use deliberate curve steps such as `1,1,2,2` or `1,2,2,3`. Do not build actors
  from perfect runtime circles or noisy one-pixel staircases.
- Important limbs are two pixels thick through their load-bearing section.
  Hands, feet, wings, mandibles, and toes need a one-pixel negative-space notch
  rather than an interior outline.
- The tongue reads as a two-value cable at normal extension: dark lower/left
  edge and bright upper/right edge. It may taper to one bright pixel only near
  the latch.
- Keep one transparent pixel between a moving limb and torso where the pose
  depends on that separation.
- No checkerboard dithering on actors, foreground platform caps, or VFX cores.
  Restrict atmospheric dithering to contiguous patches at or below 25 percent
  coverage.
- Surface wear follows construction: chips on exposed edges, rust below seams,
  grime in recesses, and highlights on intact caps. Random speckles are not
  material detail.

## Native silhouettes and animation poses

Dimensions are bounding boxes, not a mandate to fill every corner.

| Asset | Box | Occupancy and silhouette target |
|---|---:|---|
| Frog | `24x24` | Body occupies roughly `18-22x20-23`; readable head, torso, bent rear legs, two hands and toe contact. Anchor `12,23`. |
| Gnat | `16x16` | `12x10` body/wing read; bulb abdomen, separate wings, dangling legs. Center anchor. |
| Wasp | `16x16` | `16x12`; long abdomen, waist notch, swept wings and visible stinger. Center anchor. |
| Beetle | `16x16` | `16x14`; low shell, horn/mandibles, six-leg rhythm, broad ground contact. Anchor `8,14`. |
| Colonel Buzzkill | composed `92x58` weapon envelope | Two simultaneous `24x24` pod cores plus authored left/right `16x16` wing, armor, weak-point and weapon modules. Their union has the existing `50x52` contact-pod bounds; wings and weapons give the whole assembly the existing `92x58` damage bounds. |
| Egg bomb | `8x8` | Asymmetric shell, fuse/eye accent, readable against both metal and sky. |
| Terrain tile | `8x8` | Designed in `16x16` or `24x16` metatile groups; top cap remains readable with detail removed. |
| Major prop | `16x16` | Lamp, grapple coil and crate have distinct outer shapes before internal pixels. |
| Tongue/laser/fire/explosion | composed `16-32px` event | Small modular frames build a directional effect; no undirected stack of circles. |

The frog receives ten full poses. Geometry may flip at runtime, but the top-lit
base shading must remain credible in either facing direction:

| Pose | Purpose |
|---|---|
| `idle` | Fixed feet; selected-mutation and occasional blink overlays provide life without a rapid repeating blink. |
| `run_a`, `run_b` | The grounded live state: opposite leg diagonals, fixed footline, at 8-11 fps. |
| `rise` | Long diagonal from trailing toes to forward hands; narrow airborne mass. |
| `fall` | Rear legs open below torso and feet seek the next contact. |
| `swing_tuck`, `swing_extend` | Two readable tension states selected by speed/reel state; they do not pretend to rotate continuously with the rope. |
| `laser_brace` | Head pitches forward, eyes protrude, legs counter the recoil. |
| `fire_breath` | Throat expands 2-3 pixels and back rounds before the plume. |
| `hurt` | Broken diagonal with separated hands and splayed legs; never just a palette flash. |

The logical poses map to frame-origin tile IDs exactly: `idle=0`, `run_a=3`,
`run_b=6`, `rise=9`, `fall=12`, `swing_tuck=48`, `swing_extend=51`,
`laser_brace=54`, `fire_breath=57`, and `hurt=60`.

There is no `compress` frame until the runtime owns a visible anticipation state;
the current hop starts in update and is already airborne by draw. Six `8x8`
overlays supply blink, victory expression, persistent Laser Eyes, persistent
Fire Breath throat/back mutation, and the two pickup icons. Victory reuses
`rise`. Title art reuses a live pose at native or `2x` nearest-neighbor scale;
it does not redraw a smooth, procedural mascot.

Common insects each use three authored frames: neutral locomotion A/B plus a
clear attack or dive anticipation. Buzzkill draws both pod cores plus ten
left/right modules. Phase damage comes from omitted/displaced modules, the
closed/open weak-point shutter, and smoke rather than shrinking the interaction
envelopes invisibly.

## Material kit

The 24 terrain tiles preserve the five live gameplay IDs and add 19 variants:

- steel intact/seam/left end/right end and damaged cap;
- girder beam/diagonal brace/support junction/deep cavity;
- mud or masonry top/face/outside corner;
- pipe horizontal/vertical/elbow/cap-junction;
- vent/grille;
- fence post/wire/damaged wire;
- acid/runoff fill and lit hazard lip;
- one unmistakable breakable refinery face.

Never repeat one truss tile across an entire platform. Build a readable cap and
underside first, then alternate seam/damage/support modules at irregular but
designed intervals. Far-city windows appear in two- or three-pixel groups and
must not align with gameplay ledges.

The 16-tile prop block contains a lamp at IDs `224,225,240,241`, grapple coil at
`226,227,242,243`, and crate at `228,229,244,245`. The final four `8x8`
components are `sign_face=230`, `vent_or_barrel_face=231`, `antenna=246`, and
`cable_footing=247`. Repetition and paired components make larger props. Lamps
paint a warm rim into adjacent terrain. Powered coils keep a dark outer ring,
cyan mid ring, and small light center so they read as machinery attached to
scenery, not floating generic bullseyes.

## VFX shape language

VFX use 20 rotationally neutral `8x8` modules. The runtime places them along the
aim or blast vector; it does not rotate sprite pixels because the console only
supports flips.

- **Tongue latch (3 tiles):** hooked contact, compressed knot, saliva snap. The
  terrain supplies the anchor; no floating target sprite is added.
- **Laser (4 tiles):** one radial eye corona and three radial impacts. The cyan
  beam itself supplies direction and narrows to a one-pixel white core.
- **Fire (5 tiles):** one radial mouth base, three irregular plume clusters, one
  ember cluster. Placement along the aim vector supplies the taper.
- **Bomb/explosion (8 tiles):** one `8x8` egg bomb, two irregular nuclei, two
  debris clusters, and three smoke remnants. Compose them over `24-32px`; each
  later frame loses brightness and gains negative space.

Module IDs are explicit: tongue hook/knot/saliva `172,173,174`; laser
corona/impact A/B/C `175,188,189,190`; fire mouth/plume A/B/C/ember
`191,232,233,234,235`; egg bomb/nucleus A/B/debris A/B/smoke A/B/C
`236,237,248,249,250,251,252,253`.

An effect may occlude its source for one impact frame, never for the whole
animation. Nearby moonlit rims may briefly switch to the effect's hot ramp, but
background particles stay darker than actor outlines.

## Atlas allocation

The sheet remains `16x16` addressable tiles (`128x128` pixels). This plan uses
250 tiles and leaves six tiles unassigned, including a contiguous `2x2` repair
block. Coordinates below are tile coordinates; ID is `ty * 16 + tx`.

| Region | Tile coordinates / IDs | Tiles | Contents |
|---|---|---:|---|
| Frog poses | five `3x3` frames at `y=0`, `x=0,3,6,9,12`; five at `y=3` | 90 | `idle=0`, `run_a=3`, `run_b=6`, `rise=9`, `fall=12`, `swing_tuck=48`, `swing_extend=51`, `laser_brace=54`, `fire_breath=57`, `hurt=60`. |
| Frog/mutation overlays | IDs `15,31,47,63,79,95` | 6 | `blink=15`, `victory=31`, persistent `laser_eyes=47`, persistent `fire_throat=63`, `laser_pickup=79`, `fire_pickup=95`. |
| Common insects | eight `2x2` frames at `y=6`, `x=0..14 step 2`; one at `(0,8)` | 36 | Gnat origins `96,98,100`; wasp `102,104,106`; beetle `108,110,128`. |
| Boss modules | seven `2x2` assets at `y=8`, `x=2..14 step 2`; three at `y=10`, `x=0,2,4` | 40 | Upper wings L/R `130,132`; lower wings L/R `134,136`; side armor L/R `138,140`; weak point closed/open `142,160`; claw `162`; cannon `164`. |
| Boss pod cores | two `3x3` assets at `(6,10)` and `(9,10)` | 18 | Upper pod origin `166`, lower pod origin `169`; both draw simultaneously. |
| Terrain kit | IDs `192..196`, `204..222` | 24 | Exact semantic table below; current collision IDs remain stable. |
| Props | roots `(0,14)`, `(2,14)`, `(4,14)` plus component cells `(6,14)`, `(7,14)`, `(6,15)`, `(7,15)` | 16 | Lamp `224,225,240,241`; coil `226,227,242,243`; crate `228,229,244,245`; sign `230`; vent/barrel `231`; antenna `246`; cable footing `247`. |
| VFX | IDs `172..175`, `188..191`, `232..237`, `248..253` | 20 | Exact logical-to-physical order is defined in the VFX section. |
| Reserve | IDs `197,223,238,239,254,255` | 6 | Two single-tile repairs plus a contiguous `2x2` future module. |

Compact map:

```text
tile x -> 0123456789abcdef
y 00-05  FFFFFFFFFFFFFFFO
y 06-07  BBBBBBBBBBBBBBBB
y 08-09  BBMMMMMMMMMMMMMM
y 10-11  MMMMMMCCCCCCVVVV
y 12     TTTTTRCCCCCCTTTT
y 13     TTTTTTTTTTTTTTTR
y 14-15  PPPPPPPPVVVVVVRR

F frog  B common bug  M boss module  C boss core
O frog/mutation overlay  T terrain  P prop  V VFX  R reserve
```

The narrow overlay column alongside frog rows is deliberate: `24x24` frames
cannot pack evenly across 16 tiles. Do not silently absorb it into a `3x3`
frame. Any reallocation updates this table before pixels move. Until the art
bones land, this is a target allocation rather than a description of the
current sparse sheet.

## Terrain semantics and migration

The first five terrain IDs remain stable because live collision, hazards, laser
destruction, level construction, and playtests already depend on them. Every
additional tile has an explicit class:

| ID | Visual role | Class | Placement/replacement rule |
|---:|---|---|---|
| 192 | intact steel cap | solid, tongueable | Default steel platform cell. |
| 193 | girder beam | solid, tongueable | Underside/support runs. |
| 194 | mud/masonry or overgrown roof | solid, tongueable | Organic/rough platforms. |
| 195 | dark acid/runoff | hazard | Pit/runoff fill; contact remains instant death. |
| 196 | striped cracked refinery face | solid, tongueable, breakable | Laser/explosion replaces it with tile `0`. |
| 204 | steel seam | solid, tongueable | Interior cap, never at an exposed end. |
| 205 | steel left endcap | solid, tongueable | Left exposed edge only. |
| 206 | steel right endcap | solid, tongueable | Right exposed edge only. |
| 207 | damaged steel cap | solid, tongueable | Sparse variation, not adjacent to every seam. |
| 208 | diagonal girder brace | solid, tongueable | Alternates with ID `193` under long spans. |
| 209 | support junction | solid, tongueable | At column/platform intersections. |
| 210 | deep support cavity | decorative, passable | Behind or below collision cells. |
| 211 | masonry top | solid, tongueable | Top row of a masonry mass. |
| 212 | masonry face | solid, tongueable | Interior wall/column cell. |
| 213 | masonry outside corner | solid, tongueable | Exposed top-side corner. Flip only if its lighting is top-symmetric; if a lateral moon rim is necessary, spend a reserve tile on the opposite authored corner. |
| 214 | pipe horizontal | decorative, passable | Background/prop layer only. |
| 215 | pipe vertical | decorative, passable | Background/prop layer only. |
| 216 | pipe elbow | decorative, passable | Joins IDs `214/215`. |
| 217 | pipe cap/junction | decorative, passable | Terminates or branches a run. |
| 218 | vent/grille | decorative, passable | Flat wall detail. |
| 219 | fence post | decorative, passable | Paired with wire cells. |
| 220 | fence wire | decorative, passable | Between posts; do not imply a solid ledge. |
| 221 | damaged fence wire | decorative, passable | Sparse interrupted fence run. |
| 222 | lit acid/runoff lip | hazard | Bright cap over ID `195`; contact remains instant death. |

Every ID has one gameplay class. The environment bone replaces scalar
`solid()`/`hazardous()` comparisons with tables for solid, hazard, tongueable,
and breakable semantics, updates laser and explosion replacement through the
breakable table, and migrates level data, live-map construction, and authored
map data in one reviewed change.

## Runtime sockets and envelopes

All coordinates are relative to the named anchor and are part of the art gate.
Review renders show these sockets and rectangles over the native sprite.

| Asset | Anchor/socket | Relative contract |
|---|---|---|
| Frog | feet anchor | `(0,0)` maps to current `(player_x+7, player_y+15)`; frame anchor is `12,23`. |
| Frog | physics box | `x=-7, y=-15, w=14, h=16`; art may extend outside but contact limbs must agree visually. |
| Frog | mouth/tongue/fire | `(facing*5,-9)`, matching `player_mouth()`. |
| Frog | laser eyes | `(-6,-22)` and `(6,-22)`; beam begins at both eyes. |
| Frog | foot contacts | `(-5,0)` and `(5,0)`; grounded frames keep at least one planted. |
| Boss | assembly anchor | `(0,0)` maps to current `(e.x+12,e.y+38)`. |
| Boss | weapon envelope | Phases 1-2: `x=-46, y=-52, w=92, h=58`. Phase 3: `x=-46, y=-52, w=92, h=64`; the lowered claw reaches the new bottom while the surviving right upper wing reaches the top/right edges. |
| Boss | contact pod | Phases 1-2: `x=-25, y=-50, w=50, h=52`. Phase 3: `x=-25, y=-44, w=50, h=52`; the collider follows the visibly lowered pod. |
| Boss | weak-point aim | Phases 1-2: `(0,-23)`, matching the weapon-envelope center. Phase 3: `(0,-17)`, following the visibly lowered shutter rather than the taller damage-envelope center. |
| Boss | current projectile spawn | `(0,-26)`; moving it to the authored cannon muzzle requires the attack code and playtest expectations to change atomically. |
| Boss | pod-core centers | upper `(0,-38)`, lower `(0,-14)`; each `24x24` core uses center anchor `12,12`. |
| Boss | module centers | Phases 1-2: upper wings `(-38,-44)/(38,-44)`, lower wings `(-36,-24)/(36,-24)`, side armor `(-17,-6)/(17,-6)`, weak-point shutter `(0,-23)`, claw `(-38,-2)`, cannon `(38,-2)`. Phase-3 deltas are specified below. |

The phase 1-2 boss geometry is exact, using half-open rectangles. The upper core spans
`[-12,12) x [-50,-26)`; the lower spans `[-12,12) x [-26,-2)`;
the side armor spans to `x=-25/25` and `y=2`. Their union therefore has bounding
box `[-25,25) x [-50,2)`, the live `50x52` contact pod. Upper wings touch
`x=-46/46` and `y=-52`; claw/cannon touch `x=-46/46` and `y=6`, giving the full
assembly `[-46,46) x [-52,6)`, the live `92x58` weapon envelope. In phase 3,
the contact pod moves down six pixels to `[-25,25) x [-44,8)` while the right
upper wing remains at the top edge and the lowered left claw reaches the bottom,
giving `[-46,46) x [-52,12)`, the live `92x64` damage envelope. Opaque clusters
must visibly reach each stated edge even though the rounded silhouette leaves
transparent corner pixels.

Every `16x16` module uses center anchor `8,8`. Draw back to front: upper/lower
wings, upper/lower pod cores, side armor, closed/open weak-point shutter,
claw/cannon, then damage smoke and sparks. Do not substitute one core for the
other as animation frames.

Boss composition follows live state without inventing asset choices:

| Runtime state | Authored composition |
|---|---|
| dormant | Draw nothing. |
| phase 1 | Both cores, both side armors, four wings at base sockets, claw, cannon, closed shutter; scoped boss accent stays green index `12`. |
| phase 2 | Same envelope; lower wings move to `(-38,-20)/(34,-28)` and boss-only accent `12` remaps to cyan `6`, then identity is restored. |
| phase 3 | Lower the pod, side armors, shutter, and claw six pixels and cant the upper pod two pixels right; omit the entire left wing assembly; keep the right upper wing at `(38,-44)` and move the right lower wing to `(34,-30)`; replace the full cannon with a short sparking stump; remap boss-only accent `12` to red `36`; add smoke at the missing root. The surviving right upper wing and lowered left claw still touch every weapon-envelope edge, and collision follows the lowered pod. |
| vulnerable | Replace closed shutter ID `142` with open shutter ID `160`; shield VFX turns off and fallback targeting uses the visible phase socket: `(0,-23)` in phases 1-2 or `(0,-17)` in phase 3. |
| hurt | Preserve geometry and sockets; for two of five hurt frames, locally remap every boss-used index to the damage/white flash set, then restore the phase mapping. |
| facing | Never flip the assembly or its world lighting. `e.dir` changes projectile aim only; claw remains left and cannon right. |
| defeated | Disable contact first, force the open shutter, then move modules outward with deterministic offsets while explosion/smoke modules replace them; cores disappear last. |

The phase-3 left-wing loss and cannon break are persistent silhouette damage,
not a cosmetic overlay. The right upper wing, left claw, cores, and side armor
define the accepted dynamic weapon and contact bounds. The transient shield and
defeat blasts are VFX, not replacement pod geometry.

Every frog overlay is `8x8` with anchor `4,4`. Draw blink ID `15` or persistent
laser-eye ID `47` once at each eye socket; draw victory ID `31` centered at
`(0,-16)`; draw persistent fire-throat ID `63` centered at `(facing*2,-8)`.
Pickup IDs `79/95` use their own center anchor `4,4` in world space.

The two swing poses are selected by tension/speed, not rope angle. Eyes or a
small pupil offset may look toward the latch; limbs do not rotate. Laser impacts
and fire clusters are rotationally neutral, so vector placement remains honest
for horizontal, diagonal, and upward aim.

## Palette-state migration

The current cart is not identity-paletted: `reset_draw_state()` maps source
indices `0..15` through `INK_MAP`, and `preview_palette` publishes the same
legacy compact map. Exact Apollo sprites would otherwise render in the wrong
colors.

During incremental migration, every exact-index sprite draw must enter an
identity draw-palette scope, preserve source index `0` transparency, draw the
sprite, and restore the legacy mapping before old primitives render. Static
inspection must render the same identity colors; do not use the legacy
`preview_palette` to judge new assets. During the transition, inspect exported
source PNGs or a scratch cart with that preview mapping removed.

The final art migration removes `preview_palette` and the global `INK_MAP`,
converts remaining primitive colors to final Apollo indices, and leaves the draw
palette at identity. Hurt/invulnerability flash then remaps the complete set of
indices used by the affected authored sprite—not merely legacy indices `1..15`—
and restores identity afterward. Palette-state changes, actor migration, static
previews, and flash regressions are one atomic acceptance unit.

## Production and review gate

1. Author or compose an exact-size PNG. Use `paintop` when reusable masks,
   transforms, procedural material, assertions, or evidence justify a graph;
   use the bounded Lua frontend when available rather than expanding raw JSON
   by hand.
2. Gate each final PNG with exact import, expected dimensions/indices, zero
   partial alpha, and `written == false`, following
   [`README.md`](README.md#reproduce-and-inspect).
3. Import into a scratch cart first. Render every frame at `1x`, `4x` and over
   both a dark-sky and mid-value-metal background with an identity draw palette.
   Compare left/right flips and reject any mirrored lateral highlight.
4. Inspect loops with `console sprite strip`, `onion`, `ghost`, and `gif`.
   Idle feet may drift at most one pixel; insect center drift stays under two.
   Overlay the frog and boss socket tables, physics/contact rectangles, weapon
   envelopes, weak point, muzzle, eyes, mouth, and foot contacts.
5. Capture live gameplay. The sprite sheet is not accepted until the runtime
   draw path uses it, every visible body covers the relevant interaction
   envelope, and horizontal/diagonal/upward effects remain honest without image
   rotation.
6. Exercise palette state in normal, hurt, invulnerable, mutation, boss, title,
   and victory frames. Static inspection and live output must show the same exact
   Apollo colors, with no legacy `INK_MAP` leak.
7. The strict visual judge compares the same native captures against the
   reference hierarchy. It judges silhouette, material specificity, lighting,
   density control and motion—not merely palette compliance.

Automated palette, alpha, allocation and animation checks are necessary but do
not certify visual quality. The final gate remains a native-size art review.

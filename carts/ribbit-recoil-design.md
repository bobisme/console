# RIBBIT RECOIL campaign direction

`ribbit-recoil.cart` is the complete first-level vertical slice: a controller-playable
title-to-victory run through **The Mosquito Complex**, built around momentum hops,
terrain tongue grapples, insect combat, Laser Eyes, Fire Breath, checkpoints,
explosive egg bombs, and the Colonel Buzzkill boss.

The controller is intentionally two-button readable. **A** hops, and holding it gives
the hop more height. **B** fires and holds the tongue; directions aim, Up/Down reel a
latch, and releasing B converts the swing into launch velocity. **Down+A** uses the
selected mutation and **Down+B** swaps mutations after both are unlocked. Seven-frame
hop buffering and coyote time, preserved air momentum, a higher air-speed cap, and an
18-pixel close-reel limit make hop-to-swing and swing-to-hop routes practical rather
than decorative.

The cart keeps tongueable stage geometry, enemy placements, mutation vats, explosives,
the optional Executive Ventilation Shaft, checkpoints, boss spawn/arena-gate data,
trigger position, decorative props, and evac geometry together in `LEVELS`. The shared
movement, combat, effects, UI, and audio state machines do not depend on level-one
coordinates. There are no dedicated floating grapple targets: the tongue raycasts in
one-pixel substeps and sticks to steel, girders, mud, and breakable terrain.
That is the seam for later stages; if a later level exceeds the console's single
128x64 map, it can rebuild the live map from the next level's platform data during the
transition, just as level one does now.

`ribbit-recoil-traversal.playtest.json` is the controller-only route contract. It uses
no warps or state-changing developer hooks: the replay sticks to authored surfaces,
swings through the opening gaps, hops through a Laser-Eyes-cut aperture, collects both
mutations, reaches both checkpoints, detonates the refinery gate, survives one distinct
salvo in each of Colonel Buzzkill's three phases, hits three telegraphed Laser Eyes
weak-point windows, survives the post-defeat sequence, and walks into evac. The
companion `ribbit-recoil-secret-traversal.playtest.json` proves the hidden maintenance
lip and Golden Fly are physically reachable by hop and tongue with no state-changing
developer hooks. The faster effects scenario captures the hop, weapons, boss spectacle,
audio, and results screen for diagnosis.

The level now contains three deliberate solution shapes:

- Main-route ledges teach hop, hold, reel, pump, and release without a bespoke hook
  object. The mandatory first mutation now sits directly on the first checkpoint line,
  so a good landing accelerates progression instead of skipping it.
- The ventilation intake and higher shaft form an optional mastery branch; the hidden
  maintenance lip contains a Golden Fly that must be tongue-snatched while latched,
  preventing a fast main-route hop from collecting it accidentally. It grants a score
  reward and victory-screen secret tally.
- Red refinery walls can be cut with a frog-sized Laser Eyes aperture or demolished by
  igniting or tongue-striking an egg bomb; the weapons therefore change routing, not
  only DPS.

Music has independent six-pattern forms and timbres. The 16.10-second traversal loop
develops swamp-funk croak lead, sewer keys, and mutant bass through a moon-pad half-time
bridge before a denser nerve-pluck return. The separate 10.80-second boss loop uses
chromatic siren FM, war brass, toms, a tritone breakdown, and a more urgent bass
language. Both use four tracker channels, reserve two channels for action SFX, and lint
with zero warnings or clipping.

Runoff and bottomless pits are binary hazards: contact sets HP to zero and begins a
single death/respawn state immediately. The update returns before camera tracking, so
the view cannot drift downward while the frog is already dead. Ground contact uses a
one-pixel stability probe; idle movement therefore emits no particles, while only real
hops and meaningful landings create short motion streaks.

The visual rebuild will replace procedural actor primitives through the strict
PNG/Apollo64 bridge. [`ribbit-recoil-art/README.md`](ribbit-recoil-art/README.md)
records a reproducible `paintop` spike, exact-pixel rules, and the boundary between
deterministic graph composition and dense frame-by-frame sprite authoring. The
checked-in 24x24 frog is a pipeline specimen; the production atlas and runtime migration
still need distinct compressed, airborne, swinging, mutation, hurt, and victory
silhouettes.

Colonel Buzzkill's phase transitions reset and shield the next attack cycle, clear the
previous pattern, and require its new fan/bomb formation to fire before the next weak
point opens. Victory rank weighs bug kills, secrets, deaths, combo, and time; a run that
avoids every ordinary bug is capped at B even when it is fast and deathless.

## Gradual campaign progression

1. **The Mosquito Complex** — teaches latching, reeling, swinging, release momentum,
   tongue attacks, Laser Eyes, Fire Breath, explosive chains, and mutation swapping.
   Boss: Colonel Buzzkill.
2. **Canopy Conveyor** — adds moving crane beams, conveyor bark, armored beetles,
   collapsing tongueable bark, and the mutation **Toadally Radioactive Spit**. Existing
   mutations become alternate solutions rather than mandatory keys.
3. **The Royal Jelly Refinery** — introduces sticky surfaces, tongue-swing transfers,
   shield wasps, larva elevators, and the mutation **Sonic Croak**. The player starts
   combining one traversal mutation with one damage mutation.
4. **Orbital Bog Platform** — low gravity lengthens swing arcs; rotating hull surfaces,
   vacuum gusts, and electrified insects demand mid-air mutation swaps. The new
   **Magnetic Warts** mutation bends projectiles and moves metal structures.
5. **Hive Command: Extremely Final** — remixes every traversal surface and enemy rule in compact
   combat rooms before a multi-stage queen battle. Every prior mutation remains useful,
   and the final score rewards fast traversal, long bug combos, and minimal de-frogging.

Each level should add one traversal rule, one enemy interaction, and one mutation, then
recombine earlier lessons. Health and basic tongue physics stay stable so difficulty
comes from readable situations rather than numerical inflation.

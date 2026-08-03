# RIBBIT RECOIL campaign direction

`ribbit-recoil.cart` is the complete first-level vertical slice: a controller-playable
title-to-victory run through **The Mosquito Complex**, built around movement by tongue
grapple, insect combat, Laser Eyes, Fire Breath, checkpoints, explosive egg bombs, and
the Colonel Buzzkill boss.

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
swings through the opening gaps, collects both mutations, reaches both checkpoints,
detonates the refinery gate, survives one distinct salvo in each of Colonel Buzzkill's
three phases, hits three telegraphed Laser Eyes weak-point windows, survives the
post-defeat sequence, and walks into evac. The companion
`ribbit-recoil-secret-traversal.playtest.json` proves the ventilation shaft and Golden
Fly are physically reachable with controller input and no state-changing developer
hooks. The faster effects scenario captures the weapons, boss spectacle, audio, and
results screen for diagnosis.

The level now contains three deliberate solution shapes:

- Main-route ledges teach hold, reel, pump, and release without a bespoke hook object.
- The high ventilation shaft is an optional vertical mastery branch with a Golden Fly,
  score reward, and a victory-screen secret tally.
- Red refinery walls can be cut cell-by-cell with Laser Eyes or demolished by igniting
  or tongue-striking an egg bomb; the weapons therefore change routing, not only DPS.

Music has independent forms and timbres: the 9.70-second four-pattern traversal loop is
swamp-funk with croak lead and sewer keys, while the 6.53-second four-pattern boss loop
uses chromatic siren FM, war brass, toms, and a different bass language. Both reserve two
channels for action SFX and lint with zero warnings or clipping.

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

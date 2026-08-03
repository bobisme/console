# RIBBIT RECOIL campaign direction

`ribbit-recoil.cart` is the complete first-level vertical slice: a controller-playable
title-to-victory run through **The Mosquito Complex**, built around movement by tongue
grapple, insect combat, Laser Eyes, Fire Breath, checkpoints, explosive egg bombs, and
the Colonel Buzzkill boss.

The cart keeps stage geometry, grapple hooks, enemy placements, mutation vats,
explosives, checkpoints, boss spawn/arena-gate data, trigger position, and evac geometry
together in `LEVELS`. The shared movement, combat, effects, UI, and audio state machines
do not depend on level-one coordinates.
That is the seam for later stages; if a later level exceeds the console's single
128x64 map, it can rebuild the live map from the next level's platform data during the
transition, just as level one does now.

`ribbit-recoil-traversal.playtest.json` is the controller-only route contract. It uses
no warps or state-changing developer hooks: the replay swings through the opening
gaps, collects both mutations, reaches both checkpoints, detonates the refinery gate,
defeats Colonel Buzzkill with Laser Eyes, and walks into evac. The faster effects
scenario remains separate so individual weapons and audiovisual moments can still be
diagnosed directly.

## Gradual campaign progression

1. **The Mosquito Complex** — teaches latching, reeling, swinging, release momentum,
   tongue attacks, Laser Eyes, Fire Breath, explosive chains, and mutation swapping.
   Boss: Colonel Buzzkill.
2. **Canopy Conveyor** — adds moving dragonfly hooks, conveyor bark, armored beetles,
   breakable anchors, and the mutation **Toadally Radioactive Spit**. Existing mutations
   become alternate solutions rather than mandatory keys.
3. **The Royal Jelly Refinery** — introduces sticky surfaces, tongue-swing transfers,
   shield wasps, larva elevators, and the mutation **Sonic Croak**. The player starts
   combining one traversal mutation with one damage mutation.
4. **Orbital Bog Platform** — low gravity lengthens swing arcs; rotating hooks, vacuum
   gusts, and electrified insects demand mid-air mutation swaps. The new **Magnetic
   Warts** mutation bends projectiles and moves metal anchors.
5. **Hive Command: Extremely Final** — remixes every hook and enemy rule in compact
   combat rooms before a multi-stage queen battle. Every prior mutation remains useful,
   and the final score rewards fast traversal, long bug combos, and minimal de-frogging.

Each level should add one traversal rule, one enemy interaction, and one mutation, then
recombine earlier lessons. Health and basic tongue physics stay stable so difficulty
comes from readable situations rather than numerical inflation.

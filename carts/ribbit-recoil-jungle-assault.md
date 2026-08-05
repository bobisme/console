# Operation Lilybreaker

`ribbit-recoil-jungle-assault.abc` is an original 64-second, six-channel source
score for RIBBIT RECOIL. It treats the user-supplied
`stage-1-the-jungle.abc` as a complexity reference, not as melodic source
material.

## Console measurements

The reference and new score were decoded and rendered through the real Console
synth scheduler:

```bash
console music play stage-1-the-jungle.abc --dry-run
console music play carts/ribbit-recoil-jungle-assault.abc --dry-run
```

| Measurement | Jungle reference | First mix | Six-channel mix |
|---|---:|---:|---:|
| Tempo | 150 BPM | 150 BPM | 150 BPM |
| Duration | 199.98 s | 64.00 s | 64.00 s |
| Source voices | 4 | 4 | 6 |
| Note starts | 3,416 | 1,129 | 960 |
| Starts per second | 17.08 | 17.64 | 15.00 |
| Console channel steals | 0 | 0 | 0 |

The six-channel mix deliberately lowers aggregate density while distributing
each kind of motion to its own timbre and register:

| Voice | Preview waveform | Starts/s | Role |
|---|---|---:|---|
| 1 | square | 3.72 | croak lead |
| 2 | triangle | 2.94 | mud bass |
| 3 | saw | 1.88 | sparse canopy-brass stabs |
| 4 | narrow pulse | 1.47 | radio call-and-response |
| 5 | thin pulse | 3.00 | high rim pulse |
| 6 | square | 2.00 | low boot drum |

ABC source preview chooses waveform by voice order, so the ordering is part of
the arrangement. The first mix's four pitched lanes moved too continuously and
let G-sharp imply E-major color against D minor. This mix removes G-sharp,
reserves C-sharp for A-dominant bars, interlocks the lead and answer instead of
running them in parallel, and restricts the two percussion voices to chord
roots or fifths. It keeps the reference's broad D2-D6 register while using a
readable 1/16 grid instead of MIDI-derived `L:1/960` microtiming.

A static sixteenth-grid sanity check counts minor-second and tritone pitch
pairs while notes overlap. Those rough pairs fall from 234/3,360 (7.0%) in
the first mix to 14/3,293 (0.4%) here. This is not a perceptual quality score,
but it confirms that the rearrangement removes the unintended interval pileup
instead of merely making it quieter.

## Form

Forty bars are arranged as five eight-bar sections:

1. radio insertion — the lead signal emerges over a D-minor patrol pulse;
2. canopy fireline — the rim and drum double their attack rate;
3. mutation alarm — lead/answer exchanges move across a stable minor march;
4. moonlit breach — longer values create a half-time infiltration pocket;
5. extraction assault — all six roles return with a wider lead register.

Every voice contains exactly 40 complete 4/4 bars. Console emits no parser or
meter warnings, and the six-source arrangement uses the full preview allocator
without a channel steal. A runtime cart arrangement would need to choose
whether gameplay SFX may steal the sixth music channel; this source master
intentionally demonstrates the full musical space requested for listening.

Audition at a conservative host volume with:

```bash
console music play carts/ribbit-recoil-jungle-assault.abc --volume 0.25
```

This is the source-score master. `console music play` previews all six voices;
cart integration should arrange or import the voices into the existing tracker
ID budget rather than replacing the current gameplay loop implicitly. At that
stage the rim and boot parts can be assigned named noise/kick instruments while
the source preview keeps its deterministic voice-order waveforms.

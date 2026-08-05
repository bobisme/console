# Operation Lilybreaker

`ribbit-recoil-jungle-assault.abc` is an original 64-second, six-channel source
score for RIBBIT RECOIL. `ribbit-recoil-jungle-assault.cart` is its
tracker-native "Bog Funk Field Mix," with Console instruments and effects that
ABC cannot encode. Both treat the user-supplied `stage-1-the-jungle.abc` as a
complexity reference, not as melodic source material.

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

## Bog Funk Field Mix

The cart preserves all 40 bars while replacing the ABC preview's fixed voice
waveforms with eight named instruments:

| Channel | Instrument lane | Treatment |
|---|---|---|
| 0 | hollow wavetable croak lead | delayed vibrato, phrase-end scoops, selective chord arpeggios, echo send |
| 1 | FM mud bass | slow tremolo and an octave dive into each bar line |
| 2 | woody wavetable brass | short envelope, tremolo, echo and falling stabs |
| 3 | FM radio answer | pluck envelope, wider echo and upward answer-note slides |
| 4 | white-noise hats | low-level offbeat ticks with restrained accents |
| 5 | kick/snare/tom kit | pitch sweeps, noise snares and kick-triggered ducking |

`master drive=2` supplies mild saturation and safe limiting; the global echo is
a restrained two-row delay. Fifteen of the 20 patterns use all six channels.
Patterns 1, 5, 9, 13 and 17 drop the brass lane, creating a regular breath and
a real channel for auto-allocated gameplay SFX. SFX fired over the six-channel
patterns can still steal channel 5, so an integrated gameplay version should
either reserve a channel more aggressively or use explicit SFX allocation.

The tracker uses 56 of the 64 SFX IDs. Four two-bar patterns form the 12.8-second
insertion; the remaining 16 form a 51.2-second mission loop. One loop pass after
the intro is exactly 64 seconds. Strict lint reports zero errors, warnings or
info diagnostics, with no clipped samples; per-pattern peaks range from 0.517
to 0.652.

Regenerate and inspect the mix with:

```bash
node tools/lilybreaker-tracker.mjs
console music score carts/ribbit-recoil-jungle-assault.cart --song 0
console music lint carts/ribbit-recoil-jungle-assault.cart --strict
console music piano-roll carts/ribbit-recoil-jungle-assault.cart --song 0 \
  -o /tmp/operation-lilybreaker.png
console music render carts/ribbit-recoil-jungle-assault.cart --song 0 --loops 1 \
  --seed 1337 -o /tmp/operation-lilybreaker.wav
pw-play --volume 0.22 /tmp/operation-lilybreaker.wav
```

Audition the effects-free ABC source at a conservative host volume with:

```bash
console music play carts/ribbit-recoil-jungle-assault.abc --volume 0.25
```

The ABC remains the readable composition master and keeps deterministic
voice-order waveforms. The generated cart is a standalone listening mix; it
does not replace RIBBIT RECOIL's current gameplay loop implicitly.

## Build integration

The buildable game project at `carts/ribbit-recoil-project/` registers the
lossless `audio/game.cmusic` bundle through `[audio].bundle`. It keeps the
effects-rich song at music song 0, remaps the game's old combat cue calls to
seven dedicated SFX IDs (56–62), and preserves the sprite, graphics metadata,
map, and Lua sources extracted from the real game cart.

```bash
console music play carts/ribbit-recoil-project/audio/game.cmusic --song 0
console music play carts/ribbit-recoil-project --song 0 --dry-run
console build carts/ribbit-recoil-project
console build carts/ribbit-recoil-project --check
```

The `--check` command is a second pass after the normal build creates the
configured `build/ribbit-recoil.cart` output; that generated file is ignored
because the project sources and `.cmusic` bundle are authoritative.

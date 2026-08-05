# Operation Lilybreaker

`ribbit-recoil-jungle-assault.abc` is an original 64-second source score for
RIBBIT RECOIL. It treats the user-supplied `stage-1-the-jungle.abc` as a
complexity reference, not as melodic source material.

## Console measurements

The reference and new score were decoded and rendered through the real Console
synth scheduler:

```bash
console music play stage-1-the-jungle.abc --dry-run
console music play carts/ribbit-recoil-jungle-assault.abc --dry-run
```

| Measurement | Jungle reference | Operation Lilybreaker |
|---|---:|---:|
| Tempo | 150 BPM | 150 BPM |
| Duration | 199.98 s | 64.00 s |
| Source voices | 4 | 4 |
| Note starts | 3,416 | 1,129 |
| Starts per second | 17.08 | 17.64 |
| Console channel steals | 0 | 0 |

The new score is 3.3% denser by note starts per second. Its lane balance stays
close to the reference instead of concentrating the extra activity in one
part:

| Lane | Reference starts/s | New starts/s | New role |
|---|---:|---:|---|
| 1 | 4.90 | 5.08 | croak lead |
| 2 | 4.24 | 4.42 | canopy counterline |
| 3 | 4.40 | 4.50 | mud bass |
| 4 | 3.62 | 3.64 | field-kit percussion |

The reference combines staccato lead/counterpoint, near-continuous bass, and a
sparser low percussion lane across roughly F2-D6. The response preserves that
four-register orchestration and expands from D2-D6, but uses a readable 1/16
human-authored grid instead of retaining MIDI-derived `L:1/960` microtiming.

## Form

Forty bars are arranged as five eight-bar sections:

1. radio insertion — the lead signal emerges over a D-minor patrol pulse;
2. canopy fireline — shorter values and contrary motion increase pressure;
3. mutation alarm — chromatic G-sharp/C-sharp color destabilizes the march;
4. moonlit breach — longer values create a half-time infiltration pocket;
5. extraction assault — all four lanes return with a wider lead register.

Every voice contains exactly 40 complete 4/4 bars. Console emits no parser or
meter warnings, the four simultaneous voices stay below its six-channel limit,
and two channels remain conceptually available for action SFX.

Audition at a conservative host volume with:

```bash
console music play carts/ribbit-recoil-jungle-assault.abc --volume 0.25
```

This is the source-score master. `console music play` previews all four voices;
cart integration should arrange or import the voices into the existing tracker
ID budget rather than replacing the current gameplay loop implicitly.

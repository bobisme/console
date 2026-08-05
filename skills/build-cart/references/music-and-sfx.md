# Music and SFX guide

Use this guide to define timbres, write tracker rows, arrange song chains,
create responsive game SFX, mix safely, and inspect audio without guessing.

## Contents

- [Audio model](#audio-model)
- [Instrument grammar](#instrument-grammar)
- [Wave sources](#wave-sources)
- [SFX grammar and effects](#sfx-grammar-and-effects)
- [Music patterns and song form](#music-patterns-and-song-form)
- [Compose a good track](#compose-a-good-track)
- [Design good game SFX](#design-good-game-sfx)
- [Mixing, ducking, master, and echo](#mixing-ducking-master-and-echo)
- [Wavetable and FM recipes](#wavetable-and-fm-recipes)
- [Authoring tools](#authoring-tools)
- [MIDI and ABC source preview](#midi-and-abc-source-preview)
- [ABC import](#abc-import)
- [Inspection and acceptance](#inspection-and-acceptance)
- [Common failures](#common-failures)

## Audio model

| Resource | Contract |
|---|---|
| Output | 44,100 Hz mono f32, 735 samples per 60 Hz frame |
| Channels | 6 |
| Notes | C0–B7, A4=440 |
| SFX/pattern IDs | 0–63 |
| SFX rows | 1–32 |
| Row duration | 1–255 frames, or `speed=auto` from music tempo |
| Volume | 0–7 linear |
| Mix | each channel ×0.25, then master/echo path and final clamp |

The synth is deterministic across native and WASM. Oscillator phase is
continuous across note changes; changes ramp over 64 samples to avoid clicks.
A rest sets volume to zero.

Authoring has three levels:

1. `__instruments__` defines reusable timbres and global mix settings.
2. `__sfx__` defines short note/rest sequences and game sounds.
3. `__music__` assigns SFX IDs to channel slots in pattern chains.

## Instrument grammar

```text
__instruments__
inst <name> wave=<0-7|w0-w7>
  [fm=<ratio>,<index>[,<decay>]]
  [env=<attack>,<decay>,<sustain>]
  [vib=<cents>,<rate>,<delay>]
  [trem=<depth>,<rate>[,<delay>]]
  [sweep=<semitones>,<frames>]
  [duck=<depth>,<release>]
  [echo=<send>]
```

The grammar is wrapped above for readability; write each `inst` declaration on
one physical cart line.

Names match `[a-z0-9_]+`, are unique, and cannot look like a wave digit or
`w0`–`w7`.

| Field | Range | Meaning |
|---|---|---|
| `wave` | `0-7`, `w0-w7` | Source oscillator. Wave 6 requires `fm`. |
| `fm.ratio` | 0.5–15 in 0.5 steps | Modulator/carrier ratio. |
| `fm.index` | 0–15 | FM brightness/depth at note-on. |
| `fm.decay` | 0–15, default 0 | Index-envelope speed; 0 holds. |
| `env.attack` | 0–255 frames | Ramp from zero to row volume. |
| `env.decay` | 0–255 frames | Ramp toward absolute sustain. |
| `env.sustain` | 0–7 | Absolute held level, not a fraction of row volume. |
| `vib.cents` | 1–100 | Pitch depth. |
| `vib.rate` | 1–16 | Triangle LFO phase units/frame; cycle = `64/rate` frames. |
| `vib.delay` | 0–255 frames | Delay before pitch modulation. |
| `trem.depth` | 1–15 | Attenuation depth in sixteenths. |
| `trem.rate` | 1–16 | Same timing units as vibrato. |
| `trem.delay` | 0–255, default 0 | Delay before amplitude modulation. |
| `sweep.semitones` | -96–96 | Pitch change from note-on. |
| `sweep.frames` | 1–255 | Sweep duration. |
| `duck.depth` | 1–7 | Sidechain attenuation of other channels. |
| `duck.release` | 1–255 frames | Linear recovery. |
| `echo` | 0–8 | Per-instrument send; 0 dry, 8 unity send. |

An instrument with no `env` stays at row volume. Be careful: envelope sustain
is absolute, so a row at volume 2 with sustain 5 swells upward.

## Wave sources

| Source | Character / use |
|---|---|
| `0` | 12.5% pulse: thin lead, pluck, retro percussion |
| `1` | 25% pulse: nasal lead, arpeggio |
| `2` | 50% square: strong melody/bass |
| `3` | triangle: round bass, kick sweep |
| `4` | saw: bright bass, brass/string-like line |
| `5` | white noise: hats, snare, wind, impact |
| `6` | two-operator FM; instrument-only, must declare `fm=` |
| `7` | periodic noise: metallic pitched pulse train, sounds four octaves below written note |
| `w0-w7` | cart-defined 32-sample, 4-bit wavetables |

Periodic noise uses a 16-step sequence. To hear A1, write A5; useful audible
range is C0–B3 because written notes end at B7. Avoid stacking many wave-7
voices because its pulse has strong DC offset.

### Wavetables

```text
wavetable <slot 0-7> <exactly 32 hex nibbles>
```

Whitespace may group the nibbles. Sample nibble `n` maps to `(2n-15)/15`, so
`0=-1`, `f=+1`, `7=-1/15`, and `8=+1/15`; there is no exact zero. Pair values
around the center to avoid DC. Playback intentionally has no interpolation.

## SFX grammar and effects

```text
__sfx__
sfx <id 0-63> speed=<1-255|auto> [loop=<start-row>,<end-row>]
<NOTE> <WAVE|INSTRUMENT> <VOL 0-7> [FX]
---
```

Notes are C0–B7, with sharps such as `C#4`. A rest is a line containing `---`.
Each SFX must have 1–32 rows. Loop endpoints are zero-based and must lie within
the rows. `speed=auto` requires the first content line of `__music__` to declare
tempo.

One optional effect may appear on a note row:

| Effect | Range / behavior |
|---|---|
| `arp<a>,<b>` | cycle offsets 0,+a,+b; offsets 0–24 semitones; 2 frames each |
| `sl<n>` | slide -24..24 semitones across the row |
| `vib` | use named instrument's declared vibrato |
| `vib<cents>,<rate>` | row-local vibrato, cents 1–100, rate 1–16, no delay |
| `fade<n>` | ramp volume by -7..7 levels across the row |

Effects reset on the next row. A held note must be expressed as repeated note
rows; flat voices remain continuous, while envelope/sweep/duck instruments
retrigger on each repeated row.

## Music patterns and song form

```text
__music__
bpm=<1-1000> [rows_per_beat=<1-16>]
pat <id 0-63> [stop|loop=<pattern-id>] : ch0 ch1 ch2 ch3 [ch4 ch5]
```

Tempo must be the first content line. It resolves `speed=auto` as:

```text
round(3600 / (bpm * rows_per_beat)) frames per row
```

Each of the 4–6 slots is an SFX ID or `-`. Pattern duration is the maximum
`row_count * speed` among its slots. An SFX's own loop range is ignored under
music; the pattern plays one pass.

End behavior:

- `stop`: halt music;
- `loop=N`: jump to pattern N;
- no flag: play the next existing pattern ID, otherwise halt.

Build an intro followed by a loop:

```text
__music__
bpm=120 rows_per_beat=4
pat 0 : 0 1 2 3 -
pat 1 : 4 5 6 7 -
pat 2 loop=1 : 8 9 10 11 -
```

Starting `music(0)` yields `0 -> [1 -> 2 ->] loop to 1`. Use separate ID
ranges for distinct songs, for example title at 0 and gameplay at 8.

## Compose a good track

Start with form and rhythm, not timbre polish.

1. Choose mood, tempo, and a short loop target (often 8–16 seconds).
2. Write a kick/snare or pulse foundation and a bass line that establishes
   roots and syncopation.
3. Add a motif with a strong rhythmic identity; repeat it with one controlled
   change in contour, ending, or octave.
4. Add harmony/texture only if it creates contrast. Silence is a voice.
5. Create an intro or pickup that makes the loop seam feel intentional.
6. Revoice and mix after the score works as plain waves.

Chiptune arrangement heuristics:

- Let parts occupy different rhythmic registers. If melody is busy, simplify
  bass/harmony; if drums are dense, leave gaps elsewhere.
- Keep important notes on strong beats and use passing tones sparingly.
- Change one layer every 1–2 bars: mute a hat, raise melody, answer a phrase,
  or alter bass rhythm. Variation need not require new harmony.
- Use arpeggios as harmonic color, not continuously on every channel.
- Give echo-wet notes rests so repeats have space to speak.
- Leave one or two channels free for gameplay SFX. Four- or five-slot patterns
  are the normal target; a full six-channel pattern forces auto SFX to steal
  channel 5.

Use `music score` constantly. It time-aligns channels even when SFX speeds
differ and reveals the actual chain and loop.

## Design good game SFX

Game SFX must be short, distinct from music, and legible on phone speakers.

- **Jump:** quick rising `sl+5` pulse or triangle, 2–4 rows.
- **Land:** short downward triangle/noise sweep, stronger for high falls.
- **Pickup:** rising two- or three-note pulse/arpeggio with a bright final note.
- **Hit:** one noisy transient plus a fast low sweep; 2–5 rows.
- **UI move:** one tiny high pulse; reserve a different interval/timbre for
  confirm/cancel.
- **Explosion:** noise body plus falling triangle/FM layer on separate channels
  if channel budget permits.

Keep action latency at zero: the identifying transient belongs in row 0. Use
pitch contour and rhythm more than loudness. Assign critical SFX an explicit
channel only when stealing behavior matters; otherwise auto-allocation is
simpler.

## Mixing, ducking, master, and echo

Six volume-7 voices can hard-clip because every channel retains ×0.25 gain.
Dense arrangements should usually sit around volume 4–5. Confirm with
`audio_stats`; do not judge headroom from row values alone.

Cart-global master:

```text
master drive=<0-8> [tone=<0-8>] [hiss=<0-4>]
```

- drive 1–3: glue/soft limiting; 5+: deliberate distortion;
- tone 1–8: progressively darker low-pass;
- hiss 1–4: deterministic tape-like floor.

Sidechain a kick:

```text
inst kick wave=3 sweep=-14,5 env=0,6,0 duck=3,8
```

A kick note dips other channels, then they recover over 8 frames. The trigger
is not ducked itself.

Global echo plus per-voice sends:

```text
echo delay=24 feedback=5 level=6
inst lead wave=1 env=0,5,1 echo=3
```

Delay is whole frames (1–60), so align it to row speed: at speed 8, delay 8 is
one row, 16 two rows, 24 three rows. Feedback/return/send are 0–8; maximum
feedback is internally below unity. Echo adds energy: use sends 2–4 first,
leave rests, run stats, and consider master drive 1 on a wet mix.

Lua `master` and `echo` override cart settings immediately. `echo(0)` flushes
the tail so an old scene cannot leak into a later one.

## Wavetable and FM recipes

Paste a recipe, audition it, then change one parameter at a time.

### Wavetables

```text
# approximate sine
wavetable 0 89acdeef ffeedca9 76532110 00112356
# hollow/reedy
wavetable 1 8cefeede eedeefec 73101121 11211013
# gritty double ramp
wavetable 2 78899aab bccddeef 01122334 45566778
```

Sharp corners add upper harmonics; extra oscillations within the 32 samples add
harmonic complexity. Always run `music lint` for DC offset.

### FM

Integer ratios sound harmonic/pitched; half-integers are inharmonic/bell-like.
Index controls brightness. Index decay makes struck tones lose brightness.

```text
inst fm_bass  wave=6 fm=1,10,13   env=0,10,4
inst fm_epian wave=6 fm=3.5,6,7   env=0,24,2
inst fm_bell  wave=6 fm=7,11,2    env=0,56,1 echo=3
inst fm_brass wave=6 fm=2,7,9     env=4,12,4 vib=18,7,6
inst fm_tom   wave=6 fm=3.5,12,15 env=0,6,0 sweep=-10,4
```

Without FM index decay, a patch tends toward organ-like constancy. High notes
times high ratios may alias; lint reports this as information because it can be
an intentional metallic sound.

## Authoring tools

Read before changing:

```bash
console music score game.cart --song 0
console music lint game.cart --strict
console music piano-roll game.cart --song 0 -o /tmp/song.png
console music render game.cart --song 0 --loops 2 -o /tmp/song.wav
```

Use transforms instead of hand-editing many rows:

```bash
console music edit game.cart transpose 0-2 -12 --dry-run
console music edit game.cart copy 0 8 --dry-run
console music edit game.cart shift-rows 1 2 --dry-run
console music edit game.cart set-vol 2 -1 --dry-run
console music edit game.cart set-inst 2 fm_bass --where 2 --dry-run
console music edit game.cart stretch 1 2 --dry-run
```

Then run the chosen command without `--dry-run`, reread score, and lint again.
Every write operation preserves unrelated text and reparses before commit.
Only `transpose` accepts an ID/range/list selection; the other edit verbs take
the single IDs in their syntax, so repeat the command for multiple SFX.

## MIDI and ABC source preview

Audition source music before turning it into the console's row-limited cart
format:

```bash
console music play theme.mid
console music play theme.abc --seconds 20 --volume 0.35
console music play theme.abc --repeat
console music play theme.mid --seconds 5 --dry-run
```

Playback uses the real console oscillator, click guard, mixer, 44.1 kHz sample
path, and six-channel limit. MIDI program families are mapped to representative
pulse/square/triangle/saw voices and channel 10 percussion to noise; velocity
maps to volume. When polyphony exceeds six, playback reports channel steals.
MIDI tempo changes are honored. Notes outside C0-B7 are octave-folded with a
warning. `--dry-run` performs the complete parse and synth render without
opening a host device, so it is the right automated acceptance check. ABC
preview keeps its first `Q:` and warns if later changes occur. Playback adds a
single console release frame after active notes so the sample stream ends at
silence rather than clicking to zero. Host playback defaults to `--volume 0.5`;
set a linear output gain from 0 (silent) to 1 (full synth output) when auditioning.
`--repeat` loops the rendered track until Ctrl-C. With `--seconds`, the selected
prefix becomes the loop; with `--dry-run`, one pass is validated and the command exits.

Convert MIDI into editable, agent-readable ABC with either output mode:

```bash
console music midi-to-abc theme.mid > theme.abc
console music midi-to-abc theme.mid -o theme.abc
```

The converter supports format-0/1 PPQ MIDI. It preserves tick gaps and note
durations, writes explicit pitch accidentals, and splits overlapping parts into
monophonic `V:` lanes. Multiple tempo events warn because current ABC preview
and import use the initial header tempo; a non-integer initial BPM is rounded
to the nearest `Q:` BPM and warns. Keep the MIDI source for tempo-exact preview.
Format 2, SMPTE timing, and oversized inputs fail explicitly.

This conversion does **not** spend SFX IDs or choose cart loop structure. Once
the ABC is musically useful, select one monophonic voice (or split voices into
separate files), then use `music import-abc` and arrange the suggested pattern
lines. Treat that reduction as orchestration: the preview can play six live
voices, while each imported melody still consumes tracker rows and SFX IDs.

## ABC import

Use ABC as a melodic starting point, not a finished arrangement:

```bash
console music import-abc game.cart tune.abc --sfx 16 \
  --inst lead --vol 5 --dry-run
console music import-abc game.cart tune.abc --sfx 16 --inst lead --vol 5
```

The importer:

- imports one monophonic voice (voice 1 if several, with warning);
- derives a rational row grid from note-length GCD;
- splits beyond 32 rows into consecutive SFX IDs;
- uses `--speed`, otherwise ABC `Q:`, otherwise quarter=120;
- repeats held notes as rows;
- reports a fitting transpose when notes leave C0–B7;
- prints suggested `__music__` pattern lines.

Supported features include key signatures/modes, accidentals, rests, duration
fractions, ties, broken rhythm, bars, repeats/endings, chords (first note), and
grace/decorations (dropped with warnings). Tuplets and voice overlays are
rejected rather than guessed. Apply the printed pattern suggestion manually,
then edit/score/lint/render as usual.

## Inspection and acceptance

Use four evidence layers:

1. **Score:** `music score` verifies notes, voices, timing, form, and loop.
2. **Static diagnostics:** `music lint` checks unreachable/unterminated chains,
   undefined literal calls, envelope swell, ineffective modulation delays,
   pitch bounds, FM aliasing, wavetable DC, SFX channel headroom, and measured
   per-pattern clipping.
3. **Running truth:** `audio_state` and `audio_events` show the sequencer's
   actual ownership, note/row changes, and SFX stealing.
4. **Signal/human:** `audio_stats`, spectrogram, and WAV expose level, clipping,
   harmonic shape, and musical feel.

Minimum acceptance:

- form chain is intentional and loop/stop works;
- no unexamined lint errors/warnings;
- gameplay SFX remain audible without destroying required music channels;
- clipped sample count is zero unless deliberate distortion is documented;
- a human listens to representative WAVs when quality, not merely correctness,
  is the goal;
- native and WASM audio smoke/goldens remain green when engine code changed.

## Common failures

| Failure | Diagnosis / fix |
|---|---|
| Quiet row swells louder | Envelope sustain is absolute; remove env or lower sustain. |
| Vibrato/tremolo never appears | Delay is at least row length or note retriggers reset the LFO. Use longer rows. |
| Periodic noise pitch is wrong | Write four octaves above desired audible pitch. |
| FM sounds static | Add/tune index decay, not only level envelope. |
| Echo sounds muddy | Reduce activity/send/feedback and create rests before repeats. |
| SFX cuts music | Music filled all six slots; leave channel 4/5 free or choose explicit policy. |
| Loop has a click/gap | Inspect exact pattern lengths and the final-to-first transition in score/events. |
| Mix clips | Lower row volumes/sends or add low master drive; verify stats. |
| Wavetable hums | Nibble mean has DC; pair values around 7/8 and lint. |
| Imported melody attacks every held row | Use a flat voice or edit repeated rows; envelopes retrigger. |
| Song stops unexpectedly | Chain ran out without `loop=` or `stop`; inspect `music score`. |

#!/usr/bin/env node

// Reproducibly turn all 40 bars of Operation Lilybreaker's six ABC voices
// into a tracker-native intro + loop. The ABC remains the readable
// composition master; this file supplies the Console-only synthesis details
// that an ABC preview cannot encode.

"use strict";

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const sourcePath = `${repoRoot}/carts/ribbit-recoil-jungle-assault.abc`;
const outputPath = `${repoRoot}/carts/ribbit-recoil-jungle-assault.cart`;
const rowsPerBar = 16;
const barsInArrangement = 40;
const brassGapPatterns = new Set([1, 5, 9, 13, 17]);

function parseVoiceBars(source) {
  const voices = new Map();
  let currentVoice = null;

  for (const rawLine of source.split(/\r?\n/)) {
    const voiceMatch = rawLine.match(/^V:(\d+)/);
    if (voiceMatch) {
      currentVoice = Number.parseInt(voiceMatch[1], 10);
      voices.set(currentVoice, "");
      continue;
    }
    if (currentVoice === null) continue;

    const music = rawLine.replace(/%.*/, "").trim();
    if (music !== "") voices.set(currentVoice, `${voices.get(currentVoice)} ${music}`);
  }

  return [...voices.entries()]
    .sort(([a], [b]) => a - b)
    .map(([voice, body]) => {
      const bars = body.split("|").map((bar) => bar.trim()).filter(Boolean);
      if (bars.length !== 40) {
        throw new Error(`voice ${voice}: expected 40 bars, found ${bars.length}`);
      }
      return bars;
    });
}

const sharpNames = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const naturalPitch = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 };

function consoleNote(accidental, letter, marks) {
  const upper = letter.toUpperCase();
  let octave = letter === upper ? 4 : 5;
  octave += [...marks].filter((mark) => mark === "'").length;
  octave -= [...marks].filter((mark) => mark === ",").length;

  let index = octave * 12 + naturalPitch[upper];
  if (accidental === "^") index += 1;
  else if (accidental === "_") index -= 1;
  else if (accidental !== "=" && upper === "B") index -= 1; // K:Dm

  if (index < 0 || index > 95) throw new Error(`note ${accidental}${letter}${marks} is out of range`);
  return `${sharpNames[index % 12]}${Math.floor(index / 12)}`;
}

function expandBar(text, voice, barIndex) {
  const rows = [];
  for (const token of text.split(/\s+/).filter(Boolean)) {
    const match = token.match(/^(\^|_|=)?([A-Ga-gz])([,']*)(\d+)?$/);
    if (!match) throw new Error(`voice ${voice}, bar ${barIndex + 1}: bad ABC token ${token}`);

    const [, accidental = "", letter, marks, lengthText] = match;
    const length = lengthText === undefined ? 1 : Number.parseInt(lengthText, 10);
    const note = letter === "z" ? null : consoleNote(accidental, letter, marks);
    for (let offset = 0; offset < length; offset += 1) {
      rows.push({ note, onset: note !== null && offset === 0 });
    }
  }

  if (rows.length !== rowsPerBar) {
    throw new Error(`voice ${voice}, bar ${barIndex + 1}: expected 16 rows, found ${rows.length}`);
  }
  return rows;
}

function pitchClass(note) {
  return sharpNames.indexOf(note.replace(/\d$/, ""));
}

function chordFx(note) {
  switch (pitchClass(note)) {
    case 0: // C major
    case 5: // F major
      return "arp4,7";
    case 2: // D minor
    case 7: // G minor
      return "arp3,7";
    default:
      return null;
  }
}

function melodicRow(event, row, voice) {
  if (event.note === null) return "---";

  if (voice === 1) {
    let fx = null;
    if (event.onset && row % rowsPerBar === 6) fx = "vib40,10";
    if (event.onset && row % rowsPerBar === 12) fx = chordFx(event.note);
    if (event.onset && row % rowsPerBar === 14) fx = "sl+2";
    return `${event.note} croaklead 4${fx ? ` ${fx}` : ""}`;
  }

  if (voice === 2) {
    const fx = row % rowsPerBar === 15 ? " sl-12" : "";
    return `${event.note} mudbass 4${fx}`;
  }

  if (voice === 3) {
    if (!event.onset) return "---";
    const fx = row % rowsPerBar === 12 ? " sl-2" : "";
    return `${event.note} canopybrass 3${fx}`;
  }

  if (!event.onset) return "---";
  let fx = null;
  if (row % rowsPerBar === 6) fx = chordFx(event.note);
  if (row % rowsPerBar === 14) fx = "sl+3";
  return `${event.note} radioanswer 3${fx ? ` ${fx}` : ""}`;
}

function percussionRow(event, row, voice) {
  const rowInBar = row % rowsPerBar;
  if (voice === 5) {
    if (!event.onset) return "---";
    const volume = rowInBar % 8 === 7 ? 3 : 2;
    return `B6 hat ${volume}`;
  }

  // Add the backbeat that the pitched ABC placeholder could only imply. The
  // source's low onsets still decide the kick placement; rows 4 and 12 become
  // noise snares, giving the field mix a real two-limb rhythm section.
  if (rowInBar === 4 || rowInBar === 12) return `D5 snare ${rowInBar === 12 ? 5 : 4}`;
  if (!event.onset) return "---";
  if (rowInBar === 0) return "C3 kick 7";
  if (rowInBar === 8) return "C3 kick 6";
  return "C4 bogtom 4";
}

function renderPair(rows, voice) {
  return rows.map((event, row) => (
    voice <= 4 ? melodicRow(event, row, voice) : percussionRow(event, row, voice)
  ));
}

const voices = parseVoiceBars(readFileSync(sourcePath, "utf8"));
if (voices.length !== 6) throw new Error(`expected 6 voices, found ${voices.length}`);

const patterns = Array.from({ length: barsInArrangement / 2 }, () => Array(6));
const sfx = [];
const sfxByVoiceAndRows = new Map();

for (let voiceIndex = 0; voiceIndex < voices.length; voiceIndex += 1) {
  const voice = voiceIndex + 1;
  const expanded = voices[voiceIndex]
    .slice(0, barsInArrangement)
    .map((bar, barIndex) => expandBar(bar, voice, barIndex));

  for (let pair = 0; pair < patterns.length; pair += 1) {
    if (voice === 3 && brassGapPatterns.has(pair)) {
      patterns[pair][voiceIndex] = null;
      continue;
    }
    const rows = [...expanded[pair * 2], ...expanded[pair * 2 + 1]];
    const rendered = renderPair(rows, voice);
    const key = `${voice}:${rendered.join("/")}`;
    let id = sfxByVoiceAndRows.get(key);
    if (id === undefined) {
      id = sfx.length;
      sfxByVoiceAndRows.set(key, id);
      sfx.push({ id, voice, rows: rendered });
    }
    patterns[pair][voiceIndex] = id;
  }
}

if (sfx.length > 64) throw new Error(`arrangement needs ${sfx.length} sfx ids; limit is 64`);

const lines = [
  "__meta__",
  "title=Operation Lilybreaker - Bog Funk Field Mix",
  "author=OpenAI Codex",
  "version=0",
  "",
  "__lua__",
  "function _init()",
  "  music(0)",
  "end",
  "",
  "function _draw()",
  "  cls(1)",
  "  print(\"OPERATION LILYBREAKER\", 68, 60, 11)",
  "  print(\"BOG FUNK FIELD MIX\", 76, 72, 27)",
  "  print(\"SIX VOICES / ONE VERY BAD SWAMP\", 48, 96, 6)",
  "end",
  "",
  "__instruments__",
  "# Hollow croak, woody brass and FM radio colors split the harmony into lanes.",
  "wavetable 0 89acdeef ffeedca9 76532110 00112356",
  "wavetable 1 8cefeede eedeefec 73101121 11211013",
  "inst croaklead wave=w1 vib=13,3,3 echo=3",
  "inst mudbass wave=6 fm=1,7 trem=2,2",
  "inst canopybrass wave=w0 env=0,5,0 trem=3,4 echo=2",
  "inst radioanswer wave=6 fm=3.5,6,7 env=0,10,0 echo=4",
  "inst kick wave=3 sweep=-19,5 env=0,6,0 duck=4,9",
  "inst snare wave=5 sweep=-8,3 env=0,5,0 echo=1",
  "inst hat wave=5 env=0,2,0",
  "inst bogtom wave=7 sweep=-10,6 env=0,8,0 duck=2,7",
  "master drive=2 tone=1 hiss=0",
  "echo delay=12 feedback=4 level=3",
  "",
  "__sfx__",
  "# Each sfx is two 4/4 bars: 32 sixteenth rows at 150 BPM.",
];

for (const phrase of sfx) {
  lines.push("", `# voice ${phrase.voice}`, `sfx ${phrase.id} speed=auto`, ...phrase.rows);
}

lines.push(
  "",
  "__music__",
  "bpm=150 rows_per_beat=4",
  "# Four-pattern insertion, then the full sixteen-pattern mission loop.",
);

for (let pattern = 0; pattern < patterns.length; pattern += 1) {
  const slots = patterns[pattern].map((id) => id === null ? "-" : String(id));
  const loop = pattern === patterns.length - 1 ? " loop=4" : "";
  lines.push(`pat ${pattern}${loop} : ${slots.join(" ")}`);
}

writeFileSync(outputPath, `${lines.join("\n")}\n`);
console.log(`wrote ${outputPath}`);
console.log(`${sfx.length} unique two-bar sfx across ${patterns.length} patterns`);

#!/usr/bin/env node

// Materialize a buildable RIBBIT RECOIL project from the checked-in monolithic
// cart. The game owns its Lua/sprite/map sources here; the lossless native
// audio bundle comes from the effects-rich Lilybreaker music cart.

"use strict";

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const gameCart = `${root}/carts/ribbit-recoil.cart`;
const musicCart = `${root}/carts/ribbit-recoil-jungle-assault.cart`;
const project = `${root}/carts/ribbit-recoil-project`;

function sections(text) {
  const result = new Map();
  let name = null;
  for (const line of text.split(/\r?\n/)) {
    const header = line.match(/^__([a-z0-9_]+)__$/);
    if (header) {
      name = header[1];
      result.set(name, []);
    } else if (name !== null) {
      result.get(name).push(line);
    }
  }
  return new Map([...result].map(([key, lines]) => [key, `${lines.join("\n").trim()}\n`]));
}

const game = sections(readFileSync(gameCart, "utf8"));
const music = sections(readFileSync(musicCart, "utf8"));
for (const required of ["lua", "gfx_meta", "sprites", "map"]) {
  if (!game.has(required)) throw new Error(`game cart is missing __${required}__`);
}
for (const required of ["instruments", "sfx", "music"]) {
  if (!music.has(required)) throw new Error(`music cart is missing __${required}__`);
}

mkdirSync(`${project}/lua`, { recursive: true });
mkdirSync(`${project}/data`, { recursive: true });
mkdirSync(`${project}/audio`, { recursive: true });
mkdirSync(`${project}/build`, { recursive: true });

let lua = game.get("lua");
const sfxMap = new Map([
  [16, 56], [17, 57], [18, 58], [19, 59], [20, 60], [21, 61], [22, 62],
  [23, 60], [24, 61], [25, 59], [26, 57], [27, 62], [28, 60], [29, 61], [30, 62], [63, 56],
]);
for (const [oldId, newId] of sfxMap) lua = lua.replaceAll(`sfx(${oldId}`, `sfx(${newId}`);
lua = lua.replaceAll("sfx(kind==B_BOMB and 29 or 28", "sfx(kind==B_BOMB and 61 or 60");
lua = lua.replaceAll("music(boss_started and not boss_defeated and 8 or 0)", "music(0)");
lua = lua.replaceAll("music(8)", "music(0)");
lua = lua.replaceAll("music(16)", "music(0)");

const gameplaySfx = `
# Gameplay cues retained outside the music phrase ID range.
sfx 56 speed=4
C4 croaklead 4 sl+2
G4 croaklead 3

sfx 57 speed=4
C4 radioanswer 3 sl+4
E4 radioanswer 2

sfx 58 speed=3
C5 snare 5
C4 bogtom 4

sfx 59 speed=3
C5 canopybrass 3 sl+7
G5 radioanswer 3

sfx 60 speed=3
C3 kick 5
C4 snare 4

sfx 61 speed=4
C3 kick 7
D5 snare 5
C2 bogtom 4

sfx 62 speed=5
C4 radioanswer 4 arp4,7
G4 canopybrass 3
C5 radioanswer 2
`;

const cmusic = [
  "console-music 1",
  "",
  "__instruments__",
  music.get("instruments").trimEnd(),
  "",
  "__sfx__",
  music.get("sfx").trimEnd(),
  gameplaySfx.trim(),
  "",
  "__music__",
  music.get("music").trimEnd(),
  "",
].join("\n");

const manifest = `manifest_version = 1

[cart]
title = "RIBBIT RECOIL"
author = "OpenAI Codex"
version = "1"
preview_palette = [48,48,41,36,38,31,14,11,4,2,7,63,59,55,52,45]

[lua]
entry = "lua/main.lua"
root = "lua"

[audio]
bundle = "audio/game.cmusic"

[build]
output = "build/ribbit-recoil.cart"

[sections]
gfx_meta = "data/gfx-meta.txt"
sprites = "data/sprites.txt"
map = "data/map.txt"
`;

writeFileSync(`${project}/console.toml`, manifest);
writeFileSync(`${project}/lua/main.lua`, `${lua.trimEnd()}\n`);
writeFileSync(`${project}/data/gfx-meta.txt`, game.get("gfx_meta"));
writeFileSync(`${project}/data/sprites.txt`, game.get("sprites"));
writeFileSync(`${project}/data/map.txt`, game.get("map"));
writeFileSync(`${project}/audio/game.cmusic`, cmusic);
writeFileSync(`${project}/.gitignore`, "build/*.cart\n");
writeFileSync(`${project}/README.md`, [
  "# RIBBIT RECOIL build project",
  "",
  "This is the buildable multi-file form of the game. The Lua, sprite sheet, and map are extracted from carts/ribbit-recoil.cart; audio/game.cmusic supplies the six-channel Operation Lilybreaker mix plus remapped gameplay cues.",
  "",
  "console music play audio/game.cmusic --song 0 auditions the native bundle.",
  "",
  "console build . creates build/ribbit-recoil.cart from the project sources.",
  "console build . --check verifies that generated cart on a second pass; the output is ignored because the source files and bundle are authoritative.",
  "",
].join("\n"));

console.log(`wrote ${project}`);

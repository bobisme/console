#!/usr/bin/env node
// Headless smoke test for web/engine.js — the same C ABI the packed HTML uses,
// driven from Node (SINGLE_FILE means there is nothing else to load).
//
//   node web/smoke.cjs [path/to/engine.js] [path/to/cart]
//
// Exits non-zero on the first failure. Verifies: module loads, cart allocates
// and initialises, 120 frames run error-free with input applied mid-run, the
// framebuffer is both non-trivial and actually animating, and — the headline
// check — that the wasm synth renders audio bit-identical to the native build.

"use strict";

const fs = require("fs");
const path = require("path");

const repoRoot = path.resolve(__dirname, "..");
const enginePath = process.argv[2] || path.join(repoRoot, "web", "engine.js");
const cartPath = process.argv[3] || path.join(repoRoot, "carts", "demo.cart");

const W = 144, H = 256, FB_LEN = W * H;
const BTN_RIGHT = 2;
const FRAMES = 120;
const AUDIO_LEN = 735; // console_core::SAMPLES_PER_FRAME (44100 / 60)

// Golden hashes from the *native* console-core build, demo.cart, seed 0,
// 120 frames of input mask 0. Both must reproduce exactly under wasm — that
// cross-platform bit-exactness is the whole point of the determinism contract
// in SPEC.md ("Audio (PoC v1)").
//
//   AUDIO_GOLDEN: hash of the little-endian f32::to_bits stream of all
//     120 * 735 = 88200 samples. Same constant as DEMO_AUDIO_GOLDEN in
//     crates/console-core/tests/audio.rs — keep the two in sync.
//   FB_GOLDEN: FNV-1a-32 over the 144*256 palette indices at frame 120.
//     Audio must never perturb video; if the synth ever leaks into game logic
//     (RNG draws, frame counters) this is what catches it.
const AUDIO_GOLDEN = 0xbc2bd5e1f8c7f31en;
const FB_GOLDEN = 0x5e743aea;

// The 64-bit multiplier used by console-core's test hasher (tests/audio.rs,
// tests/determinism.rs). This is the canonical FNV-1a-64 prime; the golden
// value must always equal DEMO_AUDIO_GOLDEN in console-core's tests/audio.rs.
const CORE_HASH_PRIME = 0x100000001b3n;

let failures = 0;
function check(ok, label, detail) {
  if (ok) {
    console.log(`PASS ${label}`);
  } else {
    failures++;
    console.log(`FAIL ${label}${detail ? " — " + detail : ""}`);
  }
  return ok;
}
function fatal(label, detail) {
  check(false, label, detail);
  process.exit(1);
}

function distinct(buf) {
  const seen = new Set();
  for (let i = 0; i < buf.length; i++) seen.add(buf[i]);
  return seen.size;
}

function equalBytes(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/// FNV-1a, 32-bit, over raw bytes.
function fnv1a32(bytes) {
  let h = 0x811c9dc5;
  for (let i = 0; i < bytes.length; i++) {
    h = Math.imul(h ^ bytes[i], 0x01000193);
  }
  return h >>> 0;
}

// One BigInt per byte value, hoisted out of the hash loop (88200 samples ⇒
// 352800 byte steps; allocating a BigInt per step is measurably slower).
const BIG = Array.from({ length: 256 }, (_, i) => BigInt(i));

/// FNV-1a shape, 64-bit, over the little-endian `f32::to_bits()` byte stream
/// of each Float32Array in `chunks` — byte-for-byte the same input, and the
/// same multiplier, as console-core's `hash_samples` (see CORE_HASH_PRIME).
function hashSamples(chunks) {
  const PRIME = CORE_HASH_PRIME;
  const MASK = 0xffffffffffffffffn;
  const dv = new DataView(new ArrayBuffer(4));
  let h = 0xcbf29ce484222325n;
  for (const chunk of chunks) {
    for (let i = 0; i < chunk.length; i++) {
      dv.setFloat32(0, chunk[i], /* littleEndian */ true);
      for (let b = 0; b < 4; b++) {
        h = ((h ^ BIG[dv.getUint8(b)]) * PRIME) & MASK;
      }
    }
  }
  return h;
}

function hex64(v) {
  return "0x" + v.toString(16).padStart(16, "0");
}

(async function main() {
  const ConsoleEngine = require(enginePath);
  check(typeof ConsoleEngine === "function", "engine.js exports a ConsoleEngine factory");

  let Module;
  try {
    Module = await ConsoleEngine();
  } catch (e) {
    fatal("ConsoleEngine() resolves", String(e));
  }

  for (const m of ["cwrap", "UTF8ToString", "HEAPU8"]) {
    if (!(m in Module)) fatal(`runtime method ${m} exported`, "check -sEXPORTED_RUNTIME_METHODS");
  }
  check(true, "runtime methods cwrap/UTF8ToString/HEAPU8 exported");

  const con_alloc = Module.cwrap("con_alloc", "number", ["number"]);
  const con_free = Module.cwrap("con_free", null, ["number", "number"]);
  const con_init = Module.cwrap("con_init", "number", ["number", "number"]);
  const con_step = Module.cwrap("con_step", null, ["number"]);
  const con_fb = Module.cwrap("con_fb", "number", []);
  const con_palette = Module.cwrap("con_palette", "number", []);
  const con_error = Module.cwrap("con_error", "number", []);

  // web/template.html feature-detects the raw symbol before cwrap'ing it, so a
  // missing -sEXPORTED_FUNCTIONS entry degrades to a silently mute console
  // rather than an error. Assert the export itself, the way the shell sees it.
  if (!check(typeof Module["_con_audio"] === "function", "_con_audio is exported",
             "add _con_audio to -sEXPORTED_FUNCTIONS in web/build-engine.sh")) {
    process.exit(1);
  }
  const con_audio = Module.cwrap("con_audio", "number", []);
  check(true, "all eight con_* symbols cwrap'd");

  const currentError = () => {
    const p = con_error();
    return p ? Module.UTF8ToString(p) : null;
  };

  // A detached copy: ALLOW_MEMORY_GROWTH can swap the backing buffer out from
  // under a live view, so take the view fresh and slice immediately.
  const audioFrame = () =>
    new Float32Array(Module.HEAPU8.buffer, con_audio(), AUDIO_LEN).slice();

  // --- alloc + init ---
  const cartBytes = new TextEncoder().encode(fs.readFileSync(cartPath, "utf8"));
  const cartPtr = con_alloc(cartBytes.length);
  if (!cartPtr) fatal("con_alloc returns non-null", `len=${cartBytes.length}`);
  Module.HEAPU8.set(cartBytes, cartPtr);
  const rc = con_init(cartPtr, cartBytes.length);
  // con_init took ownership of cartPtr; do not free it here.
  if (!check(rc === 0, "con_init returns 0", `rc=${rc} err=${currentError()}`)) process.exit(1);
  check(currentError() === null, "con_error is null after init");

  // --- palette ---
  const pal = Module.HEAPU8.slice(con_palette(), con_palette() + 48);
  check(pal.length === 48 && distinct(pal) > 1, "con_palette gives 48 non-uniform RGB bytes");

  // --- audio: silence before the first step ---
  const audioPtrFirst = con_audio();
  check(audioPtrFirst !== 0, "con_audio returns non-null");
  check(audioPtrFirst % 4 === 0, "con_audio pointer is f32-aligned",
        `ptr=${audioPtrFirst}`);
  const audio0 = audioFrame();
  check(audio0.length === AUDIO_LEN, `con_audio gives ${AUDIO_LEN} samples`,
        `got ${audio0.length}`);
  check(audio0.every((s) => s === 0), "frame 0 (pre-step) is silence");

  // --- run 120 frames, RIGHT held for frames 30..90 ---
  const fbPtrFirst = con_fb();
  let frame1 = null, frameLast = null;
  let audio1 = null;
  let errAt = null;

  for (let f = 1; f <= FRAMES; f++) {
    const input = f >= 30 && f <= 90 ? BTN_RIGHT : 0;
    con_step(input);
    const err = currentError();
    if (err && errAt === null) errAt = `frame ${f}: ${err}`;
    if (f === 1) {
      frame1 = Module.HEAPU8.slice(con_fb(), con_fb() + FB_LEN);
      audio1 = audioFrame();
    }
    if (f === FRAMES) frameLast = Module.HEAPU8.slice(con_fb(), con_fb() + FB_LEN);
  }

  check(errAt === null, `con_error null across ${FRAMES} frames`, errAt);
  check(con_fb() === fbPtrFirst, "con_fb pointer is stable across calls");
  check(con_audio() === audioPtrFirst, "con_audio pointer is stable across calls");

  // The demo cart starts music in _init, so frame 1 already has sound.
  const peak1 = audio1.reduce((m, s) => Math.max(m, Math.abs(s)), 0);
  check(peak1 > 0, "frame 1 audio is nonzero (music starts in _init)",
        `peak=${peak1}`);
  check(audio1.every((s) => Math.abs(s) <= 1), "samples stay clamped to [-1, 1]",
        `peak=${peak1}`);

  check(
    frame1.length === FB_LEN && frameLast.length === FB_LEN,
    `framebuffer is ${FB_LEN} bytes (${W}x${H})`
  );
  const d1 = distinct(frame1), dN = distinct(frameLast);
  check(d1 >= 3, "frame 1 has >= 3 distinct palette values", `got ${d1}`);
  check(dN >= 3, `frame ${FRAMES} has >= 3 distinct palette values`, `got ${dN}`);
  check(!equalBytes(frame1, frameLast), `frame 1 differs from frame ${FRAMES}`);
  const maxIdx = Math.max(...frameLast);
  check(maxIdx <= 15, "all palette indices are in 0..15", `max=${maxIdx}`);

  // --- THE headline check: wasm audio is bit-identical to native ---
  // Fresh console, seed 0, 120 frames of input mask 0 — exactly the run that
  // console-core's native test suite hashes.
  {
    const bytes = new TextEncoder().encode(fs.readFileSync(cartPath, "utf8"));
    const p = con_alloc(bytes.length);
    Module.HEAPU8.set(bytes, p);
    if (!check(con_init(p, bytes.length) === 0, "fresh con_init for the golden run",
               currentError())) process.exit(1);
    check(audioFrame().every((s) => s === 0), "con_init resets audio to silence");

    const chunks = [];
    for (let f = 0; f < FRAMES; f++) {
      con_step(0);
      chunks.push(audioFrame());
    }
    const err = currentError();
    check(err === null, `golden run is error-free across ${FRAMES} frames`, err);

    const total = chunks.reduce((n, c) => n + c.length, 0);
    check(total === FRAMES * AUDIO_LEN, `collected ${FRAMES * AUDIO_LEN} samples`,
          `got ${total}`);

    const audioHash = hashSamples(chunks);
    check(
      audioHash === AUDIO_GOLDEN,
      `wasm audio hash == native golden ${hex64(AUDIO_GOLDEN)}`,
      `got ${hex64(audioHash)}`
    );
    console.log(`     audio hash: ${hex64(audioHash)} (expected ${hex64(AUDIO_GOLDEN)})`);

    // Audio must never feed back into game logic: the same run's framebuffer
    // still hashes to the value the native build produces.
    const fb = Module.HEAPU8.slice(con_fb(), con_fb() + FB_LEN);
    const fbHash = fnv1a32(fb);
    check(distinct(fb) >= 3, "golden-run framebuffer is non-uniform",
          `${distinct(fb)} distinct values`);
    check(
      fbHash === FB_GOLDEN,
      `wasm framebuffer FNV-1a-32 == native golden 0x${FB_GOLDEN.toString(16)}`,
      `got 0x${fbHash.toString(16)}`
    );
    console.log(`     fb hash:    0x${fbHash.toString(16).padStart(8, "0")} ` +
                `(expected 0x${FB_GOLDEN.toString(16).padStart(8, "0")})`);
  }

  // --- error path: a deliberately broken cart must report, not crash ---
  const bad = new TextEncoder().encode("__lua__\nfunction _update() error('boom') end\n");
  const badPtr = con_alloc(bad.length);
  Module.HEAPU8.set(bad, badPtr);
  check(con_init(badPtr, bad.length) === 0, "con_init accepts the error-cart");
  con_step(0);
  const runtimeErr = currentError();
  check(
    runtimeErr !== null && /boom/.test(runtimeErr),
    "runtime error surfaces through con_error",
    JSON.stringify(runtimeErr)
  );
  con_step(0);
  check(currentError() !== null, "console stays halted on subsequent steps");

  const junkPtr = con_alloc(4);
  Module.HEAPU8.set(new TextEncoder().encode("nope"), junkPtr);
  check(con_init(junkPtr, 4) !== 0, "con_init rejects an unparseable cart");
  check(currentError() !== null, "con_error is set after a failed init");

  // con_free is exported and usable on a buffer we never hand to con_init.
  const spare = con_alloc(16);
  con_free(spare, 16);
  check(true, "con_alloc/con_free round-trip");

  console.log(failures === 0 ? "\nSMOKE: PASS (0 failures)" : `\nSMOKE: FAIL (${failures} failures)`);
  process.exit(failures === 0 ? 0 : 1);
})().catch((e) => {
  console.log("FAIL unexpected exception — " + (e && e.stack ? e.stack : e));
  process.exit(1);
});

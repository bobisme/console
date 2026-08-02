#!/usr/bin/env node
// Headless smoke test for web/engine.js — the same C ABI the packed HTML uses,
// driven from Node (SINGLE_FILE means there is nothing else to load).
//
//   node web/smoke.cjs [path/to/engine.js] [path/to/cart]
//
// Exits non-zero on the first failure. Verifies: module loads, cart allocates
// and initialises, 120 frames run error-free with input applied mid-run, and
// the framebuffer is both non-trivial and actually animating.

"use strict";

const fs = require("fs");
const path = require("path");

const repoRoot = path.resolve(__dirname, "..");
const enginePath = process.argv[2] || path.join(repoRoot, "web", "engine.js");
const cartPath = process.argv[3] || path.join(repoRoot, "carts", "demo.cart");

const W = 144, H = 256, FB_LEN = W * H;
const BTN_RIGHT = 2;
const FRAMES = 120;

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
  check(true, "all seven con_* symbols cwrap'd");

  const currentError = () => {
    const p = con_error();
    return p ? Module.UTF8ToString(p) : null;
  };

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

  // --- run 120 frames, RIGHT held for frames 30..90 ---
  const fbPtrFirst = con_fb();
  let frame1 = null, frameLast = null;
  let errAt = null;

  for (let f = 1; f <= FRAMES; f++) {
    const input = f >= 30 && f <= 90 ? BTN_RIGHT : 0;
    con_step(input);
    const err = currentError();
    if (err && errAt === null) errAt = `frame ${f}: ${err}`;
    if (f === 1) frame1 = Module.HEAPU8.slice(con_fb(), con_fb() + FB_LEN);
    if (f === FRAMES) frameLast = Module.HEAPU8.slice(con_fb(), con_fb() + FB_LEN);
  }

  check(errAt === null, `con_error null across ${FRAMES} frames`, errAt);
  check(con_fb() === fbPtrFirst, "con_fb pointer is stable across calls");

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

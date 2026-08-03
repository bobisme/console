#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { spawnSync } = require("node:child_process");

function usage(message) {
  if (message) console.error(`browser-smoke: ${message}`);
  console.error(
    "usage: CONSOLE_BROWSER=/path/to/chromium node web/browser-smoke.cjs <packed.html> [--artifacts DIR]",
  );
  process.exit(2);
}

let html;
let artifactRoot = "out/browser-check";
for (let i = 2; i < process.argv.length; i++) {
  const arg = process.argv[i];
  if (arg === "--artifacts") {
    if (++i >= process.argv.length) usage("--artifacts requires a directory");
    artifactRoot = process.argv[i];
  } else if (arg.startsWith("-")) {
    usage(`unknown option ${arg}`);
  } else if (html) {
    usage("expected exactly one packed HTML path");
  } else {
    html = arg;
  }
}

if (!html) usage("missing packed HTML path");
const browser = process.env.CONSOLE_BROWSER;
if (!browser) usage("CONSOLE_BROWSER must name a Chromium executable");

function requireFile(label, value, executable = false) {
  let stat;
  try {
    stat = fs.statSync(value);
    if (executable) fs.accessSync(value, fs.constants.X_OK);
  } catch (error) {
    usage(`${label} is not an accessible${executable ? " executable" : ""} file: ${value}`);
  }
  if (!stat.isFile()) usage(`${label} is not a file: ${value}`);
  return fs.realpathSync(value);
}

const htmlPath = requireFile("packed HTML", html);
const browserPath = requireFile("CONSOLE_BROWSER", browser, true);
const driverVersion = spawnSync("agent-browser", ["--version"], { encoding: "utf8" });
if (driverVersion.error || driverVersion.status !== 0) {
  usage("agent-browser must be installed and available on PATH");
}

const session = `console-browser-smoke-${process.pid}`;
const pageUrl = pathToFileURL(htmlPath).href;
let opened = false;
let lastStatus = null;
let lastScreen = null;
let lastAudio = null;

function command(args, json = true) {
  const full = ["--session", session];
  if (json) full.push("--json");
  full.push(...args);
  const result = spawnSync("agent-browser", full, { encoding: "utf8" });
  if (result.error) throw new Error(`starting agent-browser: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(
      `agent-browser ${args[0]} failed (${result.status}): ${result.stderr || result.stdout}`,
    );
  }
  if (!json) return result.stdout;
  let envelope;
  try {
    envelope = JSON.parse(result.stdout);
  } catch (_) {
    throw new Error(`agent-browser ${args[0]} returned invalid JSON: ${result.stdout}`);
  }
  if (!envelope.success) throw new Error(envelope.error || `agent-browser ${args[0]} failed`);
  return envelope.data;
}

function evaluate(expression) {
  const encoded = Buffer.from(expression).toString("base64");
  const data = command(["eval", "-b", encoded]);
  return JSON.parse(data.result);
}

function snapshot(name) {
  const value = evaluate(`JSON.stringify(window.__console && window.__console.${name}())`);
  if (name === "status") lastStatus = value;
  if (name === "screenState") lastScreen = value;
  if (name === "audioState") lastAudio = value;
  return value;
}

function delay(ms) {
  command(["wait", String(ms)], false);
}

function assert(ok, message) {
  if (!ok) throw new Error(message);
  console.log(`PASS ${message}`);
}

function poll(read, accept, description, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  let value;
  do {
    value = read();
    if (accept(value)) return value;
    delay(100);
  } while (Date.now() < deadline);
  throw new Error(`${description}; last value: ${JSON.stringify(value)}`);
}

function elementCenter(selector, xFraction = 0.5, yFraction = 0.5) {
  return evaluate(`JSON.stringify((() => {
    const rect = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();
    return { x: rect.left + rect.width * ${xFraction}, y: rect.top + rect.height * ${yFraction} };
  })())`);
}

function canvasState() {
  return evaluate(`JSON.stringify((() => {
    const canvas = document.getElementById("screen");
    const pixels = canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height).data;
    const colors = new Set();
    let hash = 0x811c9dc5;
    for (let i = 0; i < pixels.length; i += 4) {
      const rgba = ((pixels[i] << 24) | (pixels[i + 1] << 16) |
        (pixels[i + 2] << 8) | pixels[i + 3]) >>> 0;
      colors.add(rgba);
      for (let channel = 0; channel < 4; channel++) {
        hash ^= pixels[i + channel];
        hash = Math.imul(hash, 0x01000193) >>> 0;
      }
    }
    return {
      width: canvas.width,
      height: canvas.height,
      hash: "0x" + hash.toString(16).padStart(8, "0"),
      distinctColors: colors.size
    };
  })())`);
}

function mouseMove(point) {
  command(["mouse", "move", String(Math.round(point.x)), String(Math.round(point.y))]);
}

function collectFailure(error) {
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  const dir = path.resolve(artifactRoot, `${stamp}-${process.pid}`);
  fs.mkdirSync(dir, { recursive: true });
  const packedCopy = path.join(dir, "packed.html");
  fs.copyFileSync(htmlPath, packedCopy);

  const bestEffort = (fn) => {
    try { return fn(); } catch (probeError) { return { probeError: String(probeError) }; }
  };
  if (opened) {
    lastStatus = bestEffort(() => snapshot("status"));
    lastScreen = bestEffort(() => snapshot("screenState"));
    lastAudio = bestEffort(() => snapshot("audioState"));
    bestEffort(() => command(["screenshot", path.join(dir, "failure.png")]));
  }
  const evidence = {
    error: error && error.stack ? error.stack : String(error),
    packedHtml: htmlPath,
    retainedHtml: packedCopy,
    pageUrl,
    browser: browserPath,
    agentBrowser: driverVersion.stdout.trim(),
    status: lastStatus,
    screen: lastScreen,
    audio: lastAudio,
    network: opened ? bestEffort(() => command(["network", "requests"])) : null,
    pageErrors: opened ? bestEffort(() => command(["errors"])) : null,
    console: opened ? bestEffort(() => command(["console"])) : null,
  };
  fs.writeFileSync(path.join(dir, "diagnostics.json"), `${JSON.stringify(evidence, null, 2)}\n`);
  return dir;
}

try {
  command([
    "--executable-path", browserPath,
    "--allow-file-access",
    "open", pageUrl,
  ]);
  opened = true;

  const ready = poll(
    () => snapshot("status"),
    (value) => value && value.phase !== "booting",
    "packed page did not finish booting",
  );
  assert(ready.phase === "ready", "packed file URL reaches ready state");
  assert(ready.error === null && ready.dead === false, "boot has no fatal shell or cart error");

  const surface = evaluate(`JSON.stringify((() => {
    const api = window.__console;
    const status = api.status();
    const screen = api.screenState();
    const audio = api.audioState();
    return {
      apiFrozen: Object.isFrozen(api),
      keys: Object.keys(api).sort(),
      statusFrozen: Object.isFrozen(status),
      screenFrozen: Object.isFrozen(screen),
      audioFrozen: Object.isFrozen(audio),
      paletteFrozen: Object.isFrozen(screen.displayPalette)
    };
  })())`);
  assert(surface.apiFrozen, "diagnostic handle is frozen");
  assert(
    JSON.stringify(surface.keys) === JSON.stringify(["audioState", "screenState", "status"]),
    "diagnostic handle exposes only snapshot readers",
  );
  assert(
    surface.statusFrozen && surface.screenFrozen && surface.audioFrozen && surface.paletteFrozen,
    "diagnostic snapshots and display palette are frozen",
  );

  const frameBefore = ready.successfulFrames;
  const screenBefore = snapshot("screenState");
  const canvasBefore = canvasState();
  delay(250);
  const running = snapshot("status");
  assert(running.successfulFrames > frameBefore, "animation frames advance after boot");

  const screen = snapshot("screenState");
  assert(screen.ready === true, "framebuffer diagnostic is ready");
  assert(
    screen.logicalWidth === 192 && screen.logicalHeight === 320 &&
      screen.backingWidth === 192 && screen.backingHeight === 320,
    "logical and backing canvas dimensions are 192x320",
  );
  assert(screen.cssWidth > 0 && screen.cssHeight > 0, "canvas has a visible CSS size");
  assert(screen.colorCount === 64, "packed engine reports the 64-color palette");
  assert(screen.distinctColors >= 2, "framebuffer contains a non-uniform image");
  assert(screen.invalidIndices === 0, "framebuffer palette indices are valid");
  assert(/^0x[0-9a-f]{8}$/.test(screen.framebufferHash), "framebuffer hash is well formed");
  assert(
    screen.framebufferHash !== screenBefore.framebufferHash,
    "Lantern Leap framebuffer changes across an animated interval",
  );
  assert(
    screen.displayPalette.length === 64 &&
      screen.displayPalette.every((value) => Number.isInteger(value) && value >= 0 && value < 64),
    "display palette is a valid 64-entry index map",
  );
  const canvas = canvasState();
  assert(
    canvas.width === 192 && canvas.height === 320 && canvas.distinctColors >= 2,
    "rendered canvas contains a non-uniform 192x320 image",
  );
  assert(
    /^0x[0-9a-f]{8}$/.test(canvas.hash) && canvas.hash !== canvasBefore.hash,
    "rendered canvas changes across the animated interval",
  );

  // Use independently issued mouse-down/up commands so a held, trusted input
  // survives long enough to be observed by both diagnostics and game frames.
  // agent-browser 0.24 emits trusted keyboard events without KeyboardEvent.code,
  // while this shell intentionally maps physical key codes.
  mouseMove(elementCenter("#dpad", 0.82));
  command(["mouse", "down", "left"]);
  assert((snapshot("status").inputMask & 2) !== 0, "trusted d-pad press reaches the input mask");
  delay(120);
  command(["mouse", "up", "left"]);
  assert((snapshot("status").inputMask & 2) === 0, "trusted d-pad release clears the input mask");

  mouseMove(elementCenter("#btnA"));
  command(["mouse", "down", "left"]);
  assert((snapshot("status").inputMask & 16) !== 0, "trusted A press reaches the input mask");
  delay(150);
  command(["mouse", "up", "left"]);
  assert((snapshot("status").inputMask & 16) === 0, "trusted A release clears the input mask");

  const audio = poll(
    () => snapshot("audioState"),
    (value) => value && value.ready && value.framesPushed > 0 && value.everNonzero,
    "audio did not unlock and produce nonzero samples",
  );
  assert(audio.supported === true, "packed engine exposes audio");
  assert(audio.ctx === "running", "audio context is running after a trusted pointer gesture");
  assert(
    ["worklet-data", "worklet-blob", "scriptprocessor"].includes(audio.mode),
    "audio uses a self-contained worklet or fallback",
  );
  assert(audio.sampleRate > 0, "audio reports a live sample rate");

  command(["click", "#devmenu"]);
  const pausedAt = snapshot("status");
  assert(pausedAt.paused === true, "trusted MENU click opens the pause menu");
  delay(300);
  const whilePaused = snapshot("status");
  assert(
    whilePaused.successfulFrames === pausedAt.successfulFrames,
    "game frames stop while paused",
  );
  command(["click", "#mresume"]);
  assert(snapshot("status").paused === false, "trusted RESUME click closes the pause menu");
  delay(200);
  assert(
    snapshot("status").successfulFrames > pausedAt.successfulFrames,
    "game frames resume without reload",
  );

  delay(250);
  const beforeReset = snapshot("status").successfulFrames;
  command(["click", "#devmenu"]);
  assert(snapshot("status").paused === true, "reset probe opens the real pause menu");
  command(["click", "#mreset"]);
  const reset = snapshot("status");
  assert(
    reset.phase === "ready" && reset.error === null && reset.dead === false && !reset.paused,
    "trusted RESET menu click returns to a healthy running state",
  );
  assert(reset.successfulFrames < beforeReset, "RESET restarts the frame counter");
  delay(200);
  assert(snapshot("status").successfulFrames > reset.successfulFrames, "frames advance after RESET");

  const network = command(["network", "requests"]);
  assert(network && Array.isArray(network.requests), "browser returns a network request list");
  const requests = network.requests;
  const documentRequest = requests.find(
    (request) => request && request.url === pageUrl && request.resourceType === "Document",
  );
  assert(
    documentRequest && documentRequest.status === 200,
    "network log contains the successful exact packed-page document request",
  );
  const unexpected = requests.filter((request) => {
    if (!request || typeof request.url !== "string") return true;
    return request.url !== pageUrl && !/^(?:data|blob):/i.test(request.url);
  });
  assert(
    unexpected.length === 0,
    "packed page requests only its exact file document and in-memory module URLs",
  );
  const pageErrors = command(["errors"]);
  assert(Array.isArray(pageErrors.errors) && pageErrors.errors.length === 0, "browser reports no page errors");
  assert(snapshot("status").phase === "ready", "shell remains ready after the full interaction smoke");

  console.log("\nBROWSER SMOKE: PASS");
} catch (error) {
  let artifactDir;
  try {
    artifactDir = collectFailure(error);
  } catch (artifactError) {
    console.error(`browser-smoke: could not retain failure artifacts: ${artifactError.stack || artifactError}`);
  }
  console.error(`\nBROWSER SMOKE: FAIL\n${error.stack || error}`);
  if (artifactDir) console.error(`Failure artifacts: ${artifactDir}`);
  process.exitCode = 1;
} finally {
  if (opened) {
    try { command(["close"], false); } catch (_) { /* best-effort cleanup */ }
  }
}

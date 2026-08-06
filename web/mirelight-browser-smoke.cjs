#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { spawn } = require("node:child_process");

function usage(message) {
  if (message) console.error(`mirelight-browser-smoke: ${message}`);
  console.error(
    "usage: CONSOLE_BROWSER=/path/to/chromium node web/mirelight-browser-smoke.cjs " +
      "<packed.html> [--frames 600] [--artifacts DIR]",
  );
  process.exit(2);
}

let html;
let requestedFrames = 600;
let artifactRoot = "out/mirelight-browser-check";
for (let i = 2; i < process.argv.length; i++) {
  const arg = process.argv[i];
  if (arg === "--frames") {
    if (++i >= process.argv.length) usage("--frames requires a positive integer");
    requestedFrames = Number(process.argv[i]);
    if (!Number.isSafeInteger(requestedFrames) || requestedFrames < 1) {
      usage("--frames requires a positive integer");
    }
  } else if (arg === "--artifacts") {
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
if (typeof WebSocket !== "function" || typeof fetch !== "function") {
  usage("Node.js with global WebSocket and fetch support is required");
}

function requireFile(label, value, executable = false) {
  let stat;
  try {
    stat = fs.statSync(value);
    if (executable) fs.accessSync(value, fs.constants.X_OK);
  } catch (_) {
    usage(`${label} is not an accessible${executable ? " executable" : ""} file: ${value}`);
  }
  if (!stat.isFile()) usage(`${label} is not a file: ${value}`);
  return fs.realpathSync(value);
}

const htmlPath = requireFile("packed HTML", html);
const browserPath = requireFile("CONSOLE_BROWSER", browser, true);
const pageUrl = pathToFileURL(htmlPath).href;
const stamp = new Date().toISOString().replace(/[:.]/g, "-");
const artifactDir = path.resolve(artifactRoot, `${stamp}-${process.pid}`);
fs.mkdirSync(artifactDir, { recursive: true });

const progressSamples = [];
const screenCheckpoints = [];
const networkRequests = new Map();
const pageExceptions = [];
const consoleMessages = [];
const browserLogEntries = [];
let lastStatus = null;
let lastScreen = null;
let lastAudio = null;
let chromium = null;
let cdp = null;
let browserVersion = null;
let targetId = null;

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function assert(ok, message) {
  if (!ok) throw new Error(message);
  console.log(`PASS ${message}`);
}

class CdpClient {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Map();

    socket.addEventListener("message", (event) => {
      let message;
      try {
        message = JSON.parse(String(event.data));
      } catch (_) {
        return;
      }
      if (message.id !== undefined) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        clearTimeout(pending.timer);
        if (message.error) {
          pending.reject(
            new Error(
              `CDP ${pending.method} failed (${message.error.code}): ${message.error.message}`,
            ),
          );
        } else {
          pending.resolve(message.result || {});
        }
        return;
      }
      const listeners = this.listeners.get(message.method) || [];
      for (const listener of listeners) listener(message.params || {});
    });

    socket.addEventListener("close", () => {
      for (const pending of this.pending.values()) {
        clearTimeout(pending.timer);
        pending.reject(new Error(`CDP socket closed while waiting for ${pending.method}`));
      }
      this.pending.clear();
    });
  }

  static connect(url, timeoutMs = 10000) {
    return new Promise((resolve, reject) => {
      const socket = new WebSocket(url);
      const timer = setTimeout(() => {
        socket.close();
        reject(new Error(`timed out connecting to CDP target ${url}`));
      }, timeoutMs);
      socket.addEventListener("open", () => {
        clearTimeout(timer);
        resolve(new CdpClient(socket));
      }, { once: true });
      socket.addEventListener("error", (event) => {
        clearTimeout(timer);
        reject(event.error || new Error(`failed connecting to CDP target ${url}`));
      }, { once: true });
    });
  }

  on(method, listener) {
    const listeners = this.listeners.get(method) || [];
    listeners.push(listener);
    this.listeners.set(method, listeners);
  }

  send(method, params = {}, timeoutMs = 10000) {
    if (this.socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error(`CDP socket is not open for ${method}`));
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`timed out waiting for CDP ${method}`));
      }, timeoutMs);
      this.pending.set(id, { method, resolve, reject, timer });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    if (
      this.socket.readyState === WebSocket.OPEN ||
      this.socket.readyState === WebSocket.CONNECTING
    ) {
      this.socket.close();
    }
  }
}

async function fetchJson(url, options = {}) {
  const response = await fetch(url, options);
  if (!response.ok) throw new Error(`${options.method || "GET"} ${url}: HTTP ${response.status}`);
  return response.json();
}

function chromiumStderr() {
  return chromium ? chromium.stderrText : "";
}

async function startChromium() {
  const prefix = path.join(os.tmpdir(), "console-mirelight-cdp-");
  const profile = fs.mkdtempSync(prefix);
  const args = [
    "--headless=new",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-background-networking",
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-breakpad",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-renderer-backgrounding",
    "--metrics-recording-only",
    "--allow-file-access-from-files",
    "--remote-allow-origins=*",
    "--remote-debugging-address=127.0.0.1",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "--window-size=900,900",
    "about:blank",
  ];
  const child = spawn(browserPath, args, {
    stdio: ["ignore", "ignore", "pipe"],
  });
  const runtime = { child, profile, port: null, stderrText: "" };
  let spawnError = null;
  child.on("error", (error) => {
    spawnError = error;
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    runtime.stderrText = (runtime.stderrText + chunk).slice(-32768);
  });

  try {
    const activePortFile = path.join(profile, "DevToolsActivePort");
    const deadline = Date.now() + 10000;
    while (Date.now() < deadline) {
      if (spawnError) throw new Error(`starting Chromium: ${spawnError.message}`);
      if (child.exitCode !== null) {
        throw new Error(
          `Chromium exited before CDP became ready (${child.exitCode}): ${runtime.stderrText}`,
        );
      }
      try {
        const [portLine] = fs.readFileSync(activePortFile, "utf8").trim().split(/\r?\n/);
        const port = Number(portLine);
        if (Number.isSafeInteger(port) && port > 0 && port <= 65535) {
          runtime.port = port;
          return runtime;
        }
      } catch (_) {
        // Chromium creates DevToolsActivePort after binding the requested port.
      }
      await delay(50);
    }
    throw new Error(`Chromium did not publish DevToolsActivePort: ${runtime.stderrText}`);
  } catch (error) {
    try {
      await stopChromium(runtime);
    } catch (cleanupError) {
      error.message += `; cleanup also failed: ${cleanupError}`;
    }
    throw error;
  }
}

async function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null || child.signalCode !== null) return true;
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(false), timeoutMs);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve(true);
    });
  });
}

async function stopChromium(runtime) {
  if (!runtime) return;
  if (
    runtime.child.pid && runtime.child.exitCode === null && runtime.child.signalCode === null
  ) {
    runtime.child.kill("SIGTERM");
    if (!(await waitForExit(runtime.child, 3000))) {
      runtime.child.kill("SIGKILL");
      await waitForExit(runtime.child, 3000);
    }
  }
  const expectedPrefix = path.join(os.tmpdir(), "console-mirelight-cdp-");
  if (!runtime.profile.startsWith(expectedPrefix) || runtime.profile === expectedPrefix) {
    throw new Error(`refusing to remove unexpected Chromium profile ${runtime.profile}`);
  }
  fs.rmSync(runtime.profile, { recursive: true, force: true });
}

function remoteValueText(value) {
  if (Object.prototype.hasOwnProperty.call(value, "value")) {
    if (typeof value.value === "string") return value.value;
    try {
      return JSON.stringify(value.value);
    } catch (_) {
      return String(value.value);
    }
  }
  return value.description || value.type || "";
}

function installEventCollectors(client) {
  client.on("Runtime.exceptionThrown", ({ exceptionDetails }) => {
    pageExceptions.push(exceptionDetails || {});
  });
  client.on("Runtime.consoleAPICalled", (event) => {
    consoleMessages.push({
      type: event.type,
      text: (event.args || []).map(remoteValueText).join(" "),
      timestamp: event.timestamp,
      stackTrace: event.stackTrace || null,
    });
  });
  client.on("Log.entryAdded", ({ entry }) => {
    browserLogEntries.push(entry || {});
  });
  client.on("Network.requestWillBeSent", (event) => {
    networkRequests.set(event.requestId, {
      requestId: event.requestId,
      url: event.request && event.request.url,
      method: event.request && event.request.method,
      resourceType: event.type,
      status: null,
      failed: null,
    });
  });
  client.on("Network.responseReceived", (event) => {
    const request = networkRequests.get(event.requestId) || {
      requestId: event.requestId,
      url: event.response && event.response.url,
      method: null,
      resourceType: event.type,
      failed: null,
    };
    request.status = event.response && event.response.status;
    request.resourceType = request.resourceType || event.type;
    networkRequests.set(event.requestId, request);
  });
  client.on("Network.loadingFailed", (event) => {
    const request = networkRequests.get(event.requestId) || {
      requestId: event.requestId,
      url: null,
      method: null,
      resourceType: event.type,
      status: null,
    };
    request.failed = event.errorText || "loading failed";
    networkRequests.set(event.requestId, request);
  });
}

function currentNetwork() {
  return { requests: Array.from(networkRequests.values()) };
}

function currentPageErrors() {
  return { errors: pageExceptions.slice() };
}

function currentConsole() {
  return { messages: consoleMessages.slice(), logEntries: browserLogEntries.slice() };
}

async function evaluate(expression) {
  const response = await cdp.send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (response.exceptionDetails) {
    throw new Error(
      `Runtime.evaluate failed: ${
        response.exceptionDetails.text || response.exceptionDetails.exception?.description ||
        JSON.stringify(response.exceptionDetails)
      }`,
    );
  }
  if (!response.result || !Object.prototype.hasOwnProperty.call(response.result, "value")) {
    throw new Error(`Runtime.evaluate returned no value for ${expression.slice(0, 120)}`);
  }
  return response.result.value;
}

async function evaluateJson(expression) {
  return JSON.parse(await evaluate(expression));
}

async function snapshot(name) {
  const value = await evaluateJson(
    `JSON.stringify(window.__console ? window.__console.${name}() : null)`,
  );
  if (name === "status") lastStatus = value;
  if (name === "screenState") lastScreen = value;
  if (name === "audioState") lastAudio = value;
  return value;
}

async function runtimeSample() {
  const value = await evaluateJson(`JSON.stringify((() => ({
    performanceNow: performance.now(),
    status: window.__console && window.__console.status()
  }))())`);
  lastStatus = value.status;
  return value;
}

async function poll(read, accept, description, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  let value;
  do {
    value = await read();
    if (accept(value)) return value;
    await delay(100);
  } while (Date.now() < deadline);
  throw new Error(`${description}; last value: ${JSON.stringify(value)}`);
}

async function elementCenter(selector) {
  return evaluateJson(`JSON.stringify((() => {
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) throw new Error("missing element " + ${JSON.stringify(selector)});
    const rect = element.getBoundingClientRect();
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  })())`);
}

async function pressButton(selector, bit, label) {
  const point = await elementCenter(selector);
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mouseMoved",
    x: point.x,
    y: point.y,
  });
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: point.x,
    y: point.y,
    button: "left",
    buttons: 1,
    clickCount: 1,
  });
  try {
    assert(
      ((await snapshot("status")).inputMask & bit) !== 0,
      `trusted ${label} press reaches input`,
    );
    await delay(150);
  } finally {
    await cdp.send("Input.dispatchMouseEvent", {
      type: "mouseReleased",
      x: point.x,
      y: point.y,
      button: "left",
      buttons: 0,
      clickCount: 1,
    });
  }
  assert(
    ((await snapshot("status")).inputMask & bit) === 0,
    `trusted ${label} release clears input`,
  );
  await delay(100);
}

async function canvasState() {
  return evaluateJson(`JSON.stringify((() => {
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

async function captureScreenshot(selector, file) {
  const clip = await evaluateJson(`JSON.stringify((() => {
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) throw new Error("missing screenshot element " + ${JSON.stringify(selector)});
    const rect = element.getBoundingClientRect();
    return {
      x: rect.left + window.scrollX,
      y: rect.top + window.scrollY,
      width: rect.width,
      height: rect.height,
      scale: 1
    };
  })())`);
  if (!(clip.width > 0 && clip.height > 0)) {
    throw new Error(`screenshot element has no area: ${JSON.stringify(clip)}`);
  }
  const result = await cdp.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: true,
    clip,
  }, 30000);
  fs.writeFileSync(file, Buffer.from(result.data, "base64"));
}

function ensureHealthy(sample, previousFrames) {
  const status = sample && sample.status;
  if (!status || status.phase !== "ready" || status.error !== null || status.dead !== false) {
    throw new Error(`packed runtime became unhealthy: ${JSON.stringify(status)}`);
  }
  if (!Number.isFinite(sample.performanceNow)) {
    throw new Error(`browser performance clock is invalid: ${sample.performanceNow}`);
  }
  if (!Number.isSafeInteger(status.successfulFrames) || status.successfulFrames < previousFrames) {
    throw new Error(
      `successful frame counter is not monotonic: ${previousFrames} -> ${status.successfulFrames}`,
    );
  }
}

function validateTelemetry(status) {
  for (const field of [
    "rafCallbacks",
    "droppedSimulationFrames",
    "wasmMemoryBytes",
    "peakWasmMemoryBytes",
    "wasmMemoryGrowthEvents",
  ]) {
    if (!Number.isSafeInteger(status[field]) || status[field] < 0) {
      throw new Error(`runtime telemetry ${field} is invalid: ${status[field]}`);
    }
  }
  for (const field of ["stepWallMs", "maxStepBatchMs"]) {
    if (!Number.isFinite(status[field]) || status[field] < 0) {
      throw new Error(`runtime telemetry ${field} is invalid: ${status[field]}`);
    }
  }
  if (status.wasmMemoryBytes === 0 || status.peakWasmMemoryBytes < status.wasmMemoryBytes) {
    throw new Error(
      `runtime telemetry has an invalid WASM memory envelope: ${JSON.stringify(status)}`,
    );
  }
}

function validateScreen(screen, label) {
  if (!screen || screen.ready !== true) throw new Error(`${label} framebuffer is not ready`);
  if (
    screen.logicalWidth !== 192 || screen.logicalHeight !== 320 ||
    screen.backingWidth !== 192 || screen.backingHeight !== 320
  ) {
    throw new Error(`${label} framebuffer dimensions are not 192x320: ${JSON.stringify(screen)}`);
  }
  if (screen.colorCount !== 64 || screen.distinctColors < 2 || screen.invalidIndices !== 0) {
    throw new Error(`${label} framebuffer palette invariants failed: ${JSON.stringify(screen)}`);
  }
  if (!/^0x[0-9a-f]{8}$/.test(screen.framebufferHash)) {
    throw new Error(`${label} framebuffer hash is malformed: ${screen.framebufferHash}`);
  }
  if (
    !Array.isArray(screen.displayPalette) || screen.displayPalette.length !== 64 ||
    !screen.displayPalette.every(
      (value) => Number.isInteger(value) && value >= 0 && value < 64,
    )
  ) {
    throw new Error(`${label} display palette is invalid`);
  }
}

function requirePng(file) {
  const bytes = fs.readFileSync(file);
  const signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  if (bytes.length <= 128 || !bytes.subarray(0, signature.length).equals(signature)) {
    throw new Error(`screenshot is not a non-trivial PNG: ${file}`);
  }
  return bytes.length;
}

async function bestEffort(fn) {
  try {
    return await fn();
  } catch (error) {
    return { probeError: error && error.stack ? error.stack : String(error) };
  }
}

async function collectFailure(error) {
  const packedCopy = path.join(artifactDir, "packed.html");
  fs.copyFileSync(htmlPath, packedCopy);
  if (cdp) {
    lastStatus = await bestEffort(() => snapshot("status"));
    lastScreen = await bestEffort(() => snapshot("screenState"));
    lastAudio = await bestEffort(() => snapshot("audioState"));
    await bestEffort(() =>
      captureScreenshot("#screenframe", path.join(artifactDir, "failure.png"))
    );
  }
  const evidence = {
    schemaVersion: 2,
    driver: "direct-cdp",
    error: error && error.stack ? error.stack : String(error),
    requestedFrames,
    packedHtml: htmlPath,
    retainedHtml: packedCopy,
    pageUrl,
    browser: browserPath,
    browserVersion,
    chromiumStderr: chromiumStderr(),
    status: lastStatus,
    screen: lastScreen,
    audio: lastAudio,
    progressSamples,
    screenCheckpoints,
    network: currentNetwork(),
    pageErrors: currentPageErrors(),
    console: currentConsole(),
  };
  fs.writeFileSync(
    path.join(artifactDir, "diagnostics.json"),
    `${JSON.stringify(evidence, null, 2)}\n`,
  );
}

async function main() {
  let succeeded = false;
  try {
    chromium = await startChromium();
    const endpoint = `http://127.0.0.1:${chromium.port}`;
    browserVersion = await fetchJson(`${endpoint}/json/version`);
    const target = await fetchJson(
      `${endpoint}/json/new?${encodeURIComponent("about:blank")}`,
      { method: "PUT" },
    );
    targetId = target.id;
    cdp = await CdpClient.connect(target.webSocketDebuggerUrl);
    installEventCollectors(cdp);

    await cdp.send("Runtime.enable");
    await cdp.send("Page.enable");
    await cdp.send("Network.enable");
    await cdp.send("Log.enable");
    await cdp.send("Page.bringToFront");
    const navigation = await cdp.send("Page.navigate", { url: pageUrl });
    if (navigation.errorText) throw new Error(`Page.navigate failed: ${navigation.errorText}`);

    const ready = await poll(
      () => snapshot("status"),
      (value) => value && value.phase !== "booting",
      "packed page did not finish booting",
    );
    assert(ready.phase === "ready", "packed file URL reaches ready state");
    assert(ready.error === null && ready.dead === false, "boot has no shell or cart error");
    validateTelemetry(ready);

    const surface = await evaluateJson(`JSON.stringify((() => {
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
      "diagnostic handle retains exactly three snapshot readers",
    );
    assert(
      surface.statusFrozen && surface.screenFrozen && surface.audioFrozen && surface.paletteFrozen,
      "diagnostic snapshots remain frozen",
    );

    // Mirelight's title uses B to toggle DENSE LOAD and A to start. CDP Input
    // events enter Chromium's trusted browser input path without mutating DOM.
    await pressButton("#btnB", 32, "B dense-mode toggle");
    await pressButton("#btnA", 16, "A start");

    const unlockedAudio = await poll(
      () => snapshot("audioState"),
      (value) => value && value.ready && value.framesPushed > 0 && value.everNonzero,
      "audio did not unlock and produce nonzero samples",
    );
    assert(unlockedAudio.supported === true, "packed engine exposes audio");
    assert(unlockedAudio.ctx === "running", "audio context runs after trusted pointer input");
    assert(
      ["worklet-data", "worklet-blob", "scriptprocessor"].includes(unlockedAudio.mode),
      "audio uses a self-contained worklet or fallback",
    );
    assert(unlockedAudio.sampleRate > 0, "audio reports a live sample rate");
    lastAudio = unlockedAudio;

    const start = await runtimeSample();
    ensureHealthy(start, 0);
    validateTelemetry(start.status);
    progressSamples.push(start);
    const startFrame = start.status.successfulFrames;
    let previousFrames = startFrame;

    const startScreen = await snapshot("screenState");
    validateScreen(startScreen, "start");
    screenCheckpoints.push({ label: "start", frame: startFrame, screen: startScreen });

    const deadline = Date.now() + 120000;
    let midpointCaptured = false;
    let end = start;
    while (end.status.successfulFrames - startFrame < requestedFrames) {
      if (Date.now() >= deadline) {
        throw new Error(
          `packed runtime did not complete ${requestedFrames} frames before the liveness watchdog`,
        );
      }
      await delay(250);
      end = await runtimeSample();
      ensureHealthy(end, previousFrames);
      validateTelemetry(end.status);
      progressSamples.push(end);
      previousFrames = end.status.successfulFrames;

      if (!midpointCaptured && previousFrames - startFrame >= Math.floor(requestedFrames / 2)) {
        const midpoint = await snapshot("screenState");
        validateScreen(midpoint, "midpoint");
        screenCheckpoints.push({ label: "midpoint", frame: previousFrames, screen: midpoint });
        midpointCaptured = true;
      }
    }

    assert(
      end.status.successfulFrames - startFrame >= requestedFrames,
      `packed runtime completes ${requestedFrames} successful dense-mode frames`,
    );
    const elapsedMs = end.performanceNow - start.performanceNow;
    const advancedFrames = end.status.successfulFrames - startFrame;
    const observedFps = advancedFrames * 1000 / elapsedMs;
    assert(Number.isFinite(observedFps) && observedFps > 0, "observed FPS is reportable evidence");

    const endScreen = await snapshot("screenState");
    validateScreen(endScreen, "end");
    screenCheckpoints.push({ label: "end", frame: end.status.successfulFrames, screen: endScreen });
    assert(
      new Set(screenCheckpoints.map((checkpoint) => checkpoint.screen.framebufferHash)).size >= 2,
      "framebuffer changes during the sustained run",
    );

    const canvas = await canvasState();
    assert(
      canvas.width === 192 && canvas.height === 320 && canvas.distinctColors >= 2 &&
        /^0x[0-9a-f]{8}$/.test(canvas.hash),
      "rendered canvas is a valid non-uniform 192x320 image",
    );

    const network = currentNetwork();
    assert(Array.isArray(network.requests), "browser returns a network request list");
    const documentRequest = network.requests.find(
      (request) => request && request.url === pageUrl && request.resourceType === "Document",
    );
    assert(
      documentRequest && documentRequest.status === 200 && documentRequest.failed === null,
      "network log contains the successful exact packed-page document",
    );
    const unexpectedRequests = network.requests.filter((request) => {
      if (!request || typeof request.url !== "string") return true;
      return request.url !== pageUrl && !/^(?:data|blob):/i.test(request.url);
    });
    assert(
      unexpectedRequests.length === 0,
      "packed load requests only its file document and in-memory module URLs",
    );

    const pageErrors = currentPageErrors();
    assert(pageErrors.errors.length === 0, "browser reports no page exceptions");
    const consoleLog = currentConsole();
    const consoleErrors = consoleLog.messages.filter(
      (message) => message && String(message.type).toLowerCase() === "error",
    );
    assert(consoleErrors.length === 0, "browser console contains no error messages");
    const logErrors = consoleLog.logEntries.filter(
      (entry) => entry && String(entry.level).toLowerCase() === "error",
    );
    assert(logErrors.length === 0, "browser log contains no error entries");

    const finalStatus = await snapshot("status");
    assert(
      finalStatus.phase === "ready" && finalStatus.error === null && finalStatus.dead === false,
      "shell remains healthy after sustained load",
    );
    lastAudio = await snapshot("audioState");

    const finalPng = path.join(artifactDir, "final.png");
    await captureScreenshot("#screenframe", finalPng);
    const screenshotBytes = requirePng(finalPng);
    assert(screenshotBytes > 128, "final screenshot is retained as a valid PNG");

    const metrics = {
      schemaVersion: 2,
      driver: "direct-cdp",
      requestedFrames,
      advancedFrames,
      startFrame,
      endFrame: end.status.successfulFrames,
      elapsedMs,
      observedFps,
      speedThresholdApplied: false,
      livenessWatchdogMs: 120000,
      browser: browserPath,
      browserVersion,
      packedHtml: htmlPath,
      pageUrl,
      screenshot: finalPng,
      screenshotBytes,
      runtimeDelta: {
        rafCallbacks: end.status.rafCallbacks - start.status.rafCallbacks,
        stepWallMs: end.status.stepWallMs - start.status.stepWallMs,
        maxStepBatchMs: end.status.maxStepBatchMs,
        droppedSimulationFrames:
          end.status.droppedSimulationFrames - start.status.droppedSimulationFrames,
        wasmMemoryStartBytes: start.status.wasmMemoryBytes,
        wasmMemoryEndBytes: end.status.wasmMemoryBytes,
        peakWasmMemoryBytes: end.status.peakWasmMemoryBytes,
        wasmMemoryGrowthEvents:
          end.status.wasmMemoryGrowthEvents - start.status.wasmMemoryGrowthEvents,
      },
      startStatus: start.status,
      endStatus: end.status,
      finalStatus,
      audio: lastAudio,
      progressSamples,
      screenCheckpoints,
      canvas,
      network,
      pageErrors,
      console: consoleLog,
    };
    fs.writeFileSync(
      path.join(artifactDir, "metrics.json"),
      `${JSON.stringify(metrics, null, 2)}\n`,
    );

    succeeded = true;
    console.log("\nMIRELIGHT BROWSER LOAD: PASS");
    console.log(
      `Observed ${advancedFrames} frames in ${elapsedMs.toFixed(1)} ms ` +
        `(${observedFps.toFixed(2)} FPS; evidence only)`,
    );
    console.log(`Artifacts: ${artifactDir}`);
  } catch (error) {
    try {
      await collectFailure(error);
    } catch (artifactError) {
      console.error(
        `mirelight-browser-smoke: could not retain failure artifacts: ${
          artifactError.stack || artifactError
        }`,
      );
    }
    console.error(`\nMIRELIGHT BROWSER LOAD: FAIL\n${error.stack || error}`);
    console.error(`Failure artifacts: ${artifactDir}`);
    process.exitCode = 1;
  } finally {
    if (cdp) cdp.close();
    try {
      await stopChromium(chromium);
    } catch (cleanupError) {
      console.error(`mirelight-browser-smoke: Chromium cleanup failed: ${cleanupError}`);
      if (succeeded) process.exitCode = 1;
    }
  }
}

main();

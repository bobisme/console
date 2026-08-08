#!/usr/bin/env node
"use strict";

// Real packed-page acceptance for the host-neutral platform event adapter.
// It builds tiny carts, injects a mock TipTap SDK before the console shell,
// and drives the exact file:// output through agent-browser.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { spawnSync } = require("node:child_process");

const repoRoot = path.resolve(__dirname, "..");
const browser = process.env.CONSOLE_BROWSER;
if (!browser) {
  console.error("platform-browser-smoke: CONSOLE_BROWSER must name a Chromium executable");
  process.exit(2);
}

function requireExecutable(label, value) {
  try {
    const stat = fs.statSync(value);
    fs.accessSync(value, fs.constants.X_OK);
    if (!stat.isFile()) throw new Error("not a file");
    return fs.realpathSync(value);
  } catch (error) {
    console.error(`platform-browser-smoke: ${label} is not executable: ${value}`);
    process.exit(2);
  }
}

const browserPath = requireExecutable("CONSOLE_BROWSER", browser);
const driver = spawnSync("agent-browser", ["--version"], { encoding: "utf8" });
if (driver.error || driver.status !== 0) {
  console.error("platform-browser-smoke: agent-browser must be available on PATH");
  process.exit(2);
}

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "console-platform-browser-"));
let openSession = null;

function assert(ok, message, detail) {
  if (!ok) throw new Error(`${message}${detail ? `; ${detail}` : ""}`);
  console.log(`PASS ${message}`);
}

function command(session, args, json = true) {
  const full = ["--session", session];
  if (json) full.push("--json");
  full.push(...args);
  const result = spawnSync("agent-browser", full, { encoding: "utf8" });
  if (result.error || result.status !== 0) {
    throw new Error(`agent-browser ${args[0]} failed: ${result.stderr || result.stdout}`);
  }
  if (!json) return result.stdout;
  const envelope = JSON.parse(result.stdout);
  if (!envelope.success) throw new Error(envelope.error || `agent-browser ${args[0]} failed`);
  return envelope.data;
}

function evaluate(session, expression) {
  const encoded = Buffer.from(expression).toString("base64");
  const data = command(session, ["eval", "-b", encoded]);
  return JSON.parse(data.result);
}

function delay(session, ms) {
  command(session, ["wait", String(ms)], false);
}

function poll(session, read, accept, description, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  let value;
  do {
    value = read();
    if (accept(value)) return value;
    delay(session, 50);
  } while (Date.now() < deadline);
  throw new Error(`${description}; last value: ${JSON.stringify(value)}`);
}

function packCase(name, cartText, setupScript = "") {
  const cart = path.join(scratch, `${name}.cart`);
  const html = path.join(scratch, `${name}.html`);
  fs.writeFileSync(cart, cartText);
  const packed = spawnSync(
    "cargo",
    ["run", "-q", "-p", "console", "--", "pack", cart, "-o", html],
    { cwd: repoRoot, encoding: "utf8" },
  );
  if (packed.error || packed.status !== 0) {
    throw new Error(`packing ${name}: ${packed.stderr || packed.stdout}`);
  }
  if (setupScript) {
    const source = fs.readFileSync(html, "utf8");
    fs.writeFileSync(html, source.replace("</head>", `<script>${setupScript}</script></head>`));
  }
  return html;
}

function openCase(name, html) {
  const session = `console-platform-${name}-${process.pid}`;
  command(session, [
    "--executable-path", browserPath,
    "--allow-file-access",
    "open", pathToFileURL(html).href,
  ]);
  openSession = session;
  return session;
}

function closeCase(session) {
  try { command(session, ["close"], false); } finally { openSession = null; }
}

function status(session) {
  return evaluate(session, "JSON.stringify(window.__console.status())");
}

const callsMock = `
window.__tipTapCalls=[];
window.TipTap={
  updateScore:function(score){window.__tipTapCalls.push(["updateScore",score]);},
  submitScore:function(score){window.__tipTapCalls.push(["submitScore",score]);return Promise.resolve();},
  showLeaderboard:function(){window.__tipTapCalls.push(["showLeaderboard"]);}
};`;

try {
  {
    const html = packCase(
      "exact",
      "__lua__\nscore_update(11) score_update(11) score_submit() score_submit() " +
        "score_update(11) score_submit() leaderboard_show()\n",
      callsMock,
    );
    const session = openCase("exact", html);
    const ready = poll(session, () => status(session), (value) => value.phase !== "booting", "exact case did not boot");
    assert(
      ready.phase === "ready" && ready.error === null,
      "mock TipTap page remains ready",
      JSON.stringify(ready),
    );
    const calls = evaluate(session, "JSON.stringify(window.__tipTapCalls)");
    assert(
      JSON.stringify(calls) === JSON.stringify([
        ["updateScore", 11],
        ["submitScore", 11],
        ["updateScore", 11],
        ["submitScore", 11],
        ["showLeaderboard"],
      ]),
      "TipTap receives exact ordered methods and submit score arguments",
      JSON.stringify(calls),
    );
    assert(ready.platformBackend === "tiptap" && ready.platformEvents === 5, "TipTap diagnostics count retained events");
    assert(ready.platformFailures === 0 && ready.platformUnavailableCalls === 0, "successful TipTap calls report no failures");
    closeCase(session);
  }

  {
    const failingMock = `
window.TipTap={
  updateScore:function(){throw new Error("sync update failure");},
  submitScore:function(){return Promise.reject(new Error("async submit failure"));}
};`;
    const html = packCase(
      "failure",
      "__lua__\nscore_update(12) score_submit() leaderboard_show()\n",
      failingMock,
    );
    const session = openCase("failure", html);
    const observed = poll(
      session,
      () => status(session),
      (value) => value.phase === "ready" && value.platformFailures >= 2,
      "throw/rejection diagnostics were not observed",
    );
    assert(observed.error === null && observed.dead === false, "throwing/rejecting TipTap never halts the cart");
    assert(observed.platformUnavailableCalls === 1, "missing TipTap method is feature-detected");
    closeCase(session);
  }

  {
    const html = packCase(
      "crash",
      "__lua__\nfunction _update() score_update(44) score_submit() error('boom') end\n",
      callsMock,
    );
    const session = openCase("crash", html);
    const halted = poll(session, () => status(session), (value) => value.phase === "halted", "crash case did not halt");
    const calls = evaluate(session, "JSON.stringify(window.__tipTapCalls)");
    assert(calls.length === 0, "failed frame leaks no TipTap side effects", JSON.stringify(calls));
    assert(halted.platformEvents === 0, "failed frame commits no platform events");
    closeCase(session);
  }

  {
    const html = packCase(
      "local",
      "__meta__\nsave_id=org.example.platform-smoke\nsave_version=1\n" +
        "__lua__\nscore_update(8) score_submit() leaderboard_show()\n",
    );
    const session = openCase("local", html);
    const ready = poll(session, () => status(session), (value) => value.phase !== "booting", "local case did not boot");
    assert(ready.phase === "ready" && ready.platformBackend === "local", "missing TipTap selects ordinary browser adapter");
    assert(ready.maxSubmittedScore === 8 && ready.platformEvents === 3, "ordinary adapter retains submitted maximum");

    // Repack at the same URL with a lower result, then navigate in the same
    // browser profile. The host-only maximum must survive independently of
    // the cart save envelope.
    packCase(
      "local",
      "__meta__\nsave_id=org.example.platform-smoke\nsave_version=1\n" +
        "__lua__\nscore_update(3) score_submit() leaderboard_show() leaderboard_show()\n",
    );
    command(session, ["open", pathToFileURL(html).href]);
    const reloaded = poll(
      session,
      () => status(session),
      (value) => value.phase === "ready" && value.platformEvents === 4,
      "local persisted-score case did not reload",
    );
    assert(reloaded.maxSubmittedScore === 8, "ordinary adapter persists the maximum by stable cart identity");
    closeCase(session);
  }

  console.log("\nPLATFORM BROWSER SMOKE: PASS");
} catch (error) {
  console.error(`\nPLATFORM BROWSER SMOKE: FAIL\n${error.stack || error}`);
  process.exitCode = 1;
} finally {
  if (openSession) {
    try { command(openSession, ["close"], false); } catch (_) { /* best effort */ }
  }
  fs.rmSync(scratch, { recursive: true, force: true });
}

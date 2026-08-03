#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const { pathToFileURL } = require("node:url");
const { spawnSync } = require("node:child_process");

const html = process.argv[2];
const browser = process.env.CONSOLE_BROWSER;
if (!html || process.argv.length !== 3) {
  console.error("usage: CONSOLE_BROWSER=/path/to/chromium node web/diagnostics-smoke.cjs <packed.html>");
  process.exit(2);
}
if (!browser) {
  console.error("diagnostics-smoke: CONSOLE_BROWSER must name a Chromium executable");
  process.exit(2);
}
if (!fs.statSync(html).isFile() || !fs.statSync(browser).isFile()) {
  console.error("diagnostics-smoke: packed HTML and CONSOLE_BROWSER must both be files");
  process.exit(2);
}

const session = `console-diagnostics-${process.pid}`;
function command(args, json = true) {
  const full = ["--session", session];
  if (json) full.push("--json");
  full.push(...args);
  const result = spawnSync("agent-browser", full, { encoding: "utf8" });
  if (result.error) throw new Error(`starting agent-browser: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(`agent-browser ${args[0]} failed (${result.status}): ${result.stderr || result.stdout}`);
  }
  if (!json) return result.stdout;
  const envelope = JSON.parse(result.stdout);
  if (!envelope.success) throw new Error(envelope.error || `agent-browser ${args[0]} failed`);
  return envelope.data;
}

function evaluate(expression) {
  const data = command(["eval", expression]);
  return JSON.parse(data.result);
}

function delay(ms) {
  command(["wait", String(ms)], false);
}

function assert(ok, message) {
  if (!ok) throw new Error(message);
  console.log(`PASS ${message}`);
}

function waitReady() {
  let status;
  for (let tries = 0; tries < 40; tries++) {
    status = evaluate("JSON.stringify(window.__console && window.__console.status())");
    if (status && status.phase !== "booting") break;
    delay(100);
  }
  assert(status && status.phase === "ready", "packed page reaches diagnostic ready state");
  return status;
}

const INJECT_RENDER_FAILURE = `(() => {
  const original = CanvasRenderingContext2D.prototype.putImageData;
  CanvasRenderingContext2D.prototype.putImageData = function () {
    CanvasRenderingContext2D.prototype.putImageData = original;
    throw new Error("diagnostic smoke injected render failure");
  };
  return JSON.stringify(true);
})()`;

try {
  command([
    "--executable-path", browser,
    "--allow-file-access",
    "open", pathToFileURL(fs.realpathSync(html)).href,
  ]);

  waitReady();
  evaluate(INJECT_RENDER_FAILURE);
  delay(250);
  const failed = evaluate("JSON.stringify(window.__console.status())");
  assert(failed.phase === "failed", "unexpected render exceptions transition to failed");
  assert(failed.dead === true, "unexpected render exceptions latch dead state");
  assert(
    failed.error && failed.error.includes("diagnostic smoke injected render failure"),
    "unexpected render exceptions surface actionable text",
  );
  const stoppedAt = failed.successfulFrames;
  delay(250);
  const stopped = evaluate("JSON.stringify(window.__console.status())");
  assert(stopped.successfulFrames === stoppedAt, "failed render loop stays stopped");

  command(["reload"]);
  waitReady();
  command(["press", "Escape"]);
  const paused = evaluate("JSON.stringify(window.__console.status())");
  assert(paused.paused === true, "reset fault probe pauses the render path first");
  evaluate(INJECT_RENDER_FAILURE);
  evaluate(`(() => {
    document.getElementById("mreset").click();
    return JSON.stringify(true);
  })()`);
  const resetFailed = evaluate("JSON.stringify(window.__console.status())");
  assert(resetFailed.phase === "failed", "unexpected reset exceptions transition to failed");
  assert(resetFailed.dead === true, "unexpected reset exceptions latch dead state");
  assert(
    resetFailed.error && resetFailed.error.includes("diagnostic smoke injected render failure"),
    "unexpected reset exceptions surface actionable text",
  );
  console.log("\nDIAGNOSTICS SMOKE: PASS");
} catch (error) {
  console.error(`\nDIAGNOSTICS SMOKE: FAIL\n${error.stack || error}`);
  process.exitCode = 1;
} finally {
  try { command(["close"], false); } catch (_) { /* best-effort cleanup */ }
}

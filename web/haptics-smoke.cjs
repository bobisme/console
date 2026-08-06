#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const { pathToFileURL } = require("node:url");
const { spawnSync } = require("node:child_process");

const html = process.argv[2];
const browser = process.env.CONSOLE_BROWSER;
if (!html || process.argv.length !== 3) {
  console.error("usage: CONSOLE_BROWSER=/path/to/chromium node web/haptics-smoke.cjs <packed.html>");
  process.exit(2);
}
if (!browser) {
  console.error("haptics-smoke: CONSOLE_BROWSER must name a Chromium executable");
  process.exit(2);
}

function requireFile(label, value, executable = false) {
  let stat;
  try {
    stat = fs.statSync(value);
    if (executable) fs.accessSync(value, fs.constants.X_OK);
  } catch (_) {
    console.error(`haptics-smoke: ${label} must be an accessible${executable ? " executable" : ""} file`);
    process.exit(2);
  }
  if (!stat.isFile()) {
    console.error(`haptics-smoke: ${label} must be a file`);
    process.exit(2);
  }
  return fs.realpathSync(value);
}

const htmlPath = requireFile("packed HTML", html);
const browserPath = requireFile("CONSOLE_BROWSER", browser, true);
const driverVersion = spawnSync("agent-browser", ["--version"], { encoding: "utf8" });
if (driverVersion.error || driverVersion.status !== 0) {
  console.error("haptics-smoke: agent-browser must be installed and available on PATH");
  process.exit(2);
}

const session = `console-haptics-${process.pid}`;
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
  const encoded = Buffer.from(expression).toString("base64");
  const data = command(["eval", "-b", encoded]);
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
  for (let tries = 0; tries < 50; tries++) {
    status = evaluate("JSON.stringify(window.__console && window.__console.status())");
    if (status && status.phase !== "booting") break;
    delay(100);
  }
  assert(status && status.phase === "ready", "packed page reaches haptic test ready state");
}

const HAPTIC_PROBE = `(() => {
  const calls = [];
  const report = {};
  const realAudioContext = window.AudioContext;
  const realWebkitAudioContext = window.webkitAudioContext;
  // Synthetic pointer events are not user gestures. Keep this focused probe
  // out of the asynchronous audio initialization path exercised by the main
  // browser smoke.
  window.AudioContext = undefined;
  window.webkitAudioContext = undefined;

  const spy = function (duration) {
    calls.push(duration);
    return true;
  };
  Object.defineProperty(navigator, "vibrate", {
    configurable: true,
    writable: true,
    value: spy
  });

  function snap() {
    const status = window.__console.status();
    return {
      inputMask: status.inputMask,
      paused: status.paused,
      calls: calls.slice()
    };
  }

  function pointer(type, selector, pointerId, xFraction, yFraction, pointerType) {
    const target = document.querySelector(selector);
    const rect = target.getBoundingClientRect();
    target.dispatchEvent(new PointerEvent(type, {
      bubbles: true,
      cancelable: true,
      pointerId,
      pointerType: pointerType || "touch",
      isPrimary: true,
      button: 0,
      buttons: type === "pointerup" ? 0 : 1,
      clientX: rect.left + rect.width * (xFraction === undefined ? 0.5 : xFraction),
      clientY: rect.top + rect.height * (yFraction === undefined ? 0.5 : yFraction)
    }));
    return snap();
  }

  report.initial = snap();
  report.mouseDown = pointer("pointerdown", "#btnA", 90, 0.5, 0.5, "mouse");
  report.mouseUp = pointer("pointerup", "#btnA", 90, 0.5, 0.5, "mouse");
  window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, code: "KeyZ" }));
  report.keyboardDown = snap();
  window.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, code: "KeyZ" }));
  report.keyboardUp = snap();

  report.actionDown = pointer("pointerdown", "#btnA", 101);
  report.actionHold = pointer("pointermove", "#btnA", 101);
  report.actionSlide = pointer("pointermove", "#btnB", 101);
  report.actionUp = pointer("pointerup", "#btnB", 101);

  report.dpadRight = pointer("pointerdown", "#dpad", 102, 0.82, 0.5);
  report.dpadRightHold = pointer("pointermove", "#dpad", 102, 0.9, 0.5);
  report.dpadUp = pointer("pointermove", "#dpad", 102, 0.5, 0.1);
  report.dpadUpRelease = pointer("pointerup", "#dpad", 102, 0.5, 0.1);

  report.gameMenuDown = pointer("pointerdown", "#gmenu", 103);
  report.gameMenuUp = pointer("pointerup", "#gmenu", 103);

  Object.defineProperty(navigator, "vibrate", {
    configurable: true,
    writable: true,
    value: undefined
  });
  report.unsupportedDown = pointer("pointerdown", "#btnA", 104);
  report.unsupportedUp = pointer("pointerup", "#btnA", 104);

  Object.defineProperty(navigator, "vibrate", {
    configurable: true,
    writable: true,
    value: function () { throw new Error("injected vibration failure"); }
  });
  report.throwingDown = pointer("pointerdown", "#btnB", 105);
  report.throwingUp = pointer("pointerup", "#btnB", 105);

  Object.defineProperty(navigator, "vibrate", {
    configurable: true,
    writable: true,
    value: spy
  });
  report.deviceMenu = pointer("pointerdown", "#devmenu", 106);
  window.AudioContext = realAudioContext;
  window.webkitAudioContext = realWebkitAudioContext;
  return JSON.stringify(report);
})()`;

function same(actual, expected) {
  return JSON.stringify(actual) === JSON.stringify(expected);
}

try {
  command([
    "--executable-path", browserPath,
    "--allow-file-access",
    "open", pathToFileURL(htmlPath).href,
  ]);
  waitReady();
  const report = evaluate(HAPTIC_PROBE);

  assert(same(report.initial.calls, []), "haptic log starts empty");
  assert((report.mouseDown.inputMask & 16) !== 0 && same(report.mouseDown.calls, []),
         "mouse action input remains silent");
  assert((report.keyboardDown.inputMask & 16) !== 0 && same(report.keyboardDown.calls, []),
         "keyboard action input remains silent");

  assert((report.actionDown.inputMask & 16) !== 0 && same(report.actionDown.calls, [12]),
         "new touch action requests one 12ms tap");
  assert(same(report.actionHold.calls, [12]), "holding a touch action does not repeat haptics");
  assert((report.actionSlide.inputMask & 32) !== 0 && same(report.actionSlide.calls, [12, 12]),
         "sliding onto another action requests one new tap");
  assert((report.actionUp.inputMask & 48) === 0 && same(report.actionUp.calls, [12, 12]),
         "releasing a touch action is silent");

  assert((report.dpadRight.inputMask & 2) !== 0 && same(report.dpadRight.calls, [12, 12, 8]),
         "new d-pad direction requests one 8ms tap");
  assert(same(report.dpadRightHold.calls, [12, 12, 8]),
         "moving within one d-pad direction does not repeat haptics");
  assert((report.dpadUp.inputMask & 4) !== 0 && same(report.dpadUp.calls, [12, 12, 8, 8]),
         "sliding into another d-pad direction requests one new tap");
  assert(same(report.dpadUpRelease.calls, [12, 12, 8, 8]),
         "releasing the d-pad is silent");

  assert((report.gameMenuDown.inputMask & 64) !== 0 &&
         same(report.gameMenuDown.calls, [12, 12, 8, 8, 8]),
         "touch game-menu input requests one 8ms tap");
  assert((report.gameMenuUp.inputMask & 64) === 0, "releasing game-menu clears its input bit");

  assert((report.unsupportedDown.inputMask & 16) !== 0 &&
         same(report.unsupportedUp.calls, [12, 12, 8, 8, 8]),
         "touch input survives an unsupported Vibration API");
  assert((report.throwingDown.inputMask & 32) !== 0 &&
         same(report.throwingUp.calls, [12, 12, 8, 8, 8]),
         "touch input survives a throwing Vibration API");

  assert(report.deviceMenu.paused === true &&
         same(report.deviceMenu.calls, [12, 12, 8, 8, 8, 8]),
         "touch device-menu opens the pause menu with one 8ms tap");
  console.log("\nHAPTICS SMOKE: PASS");
} catch (error) {
  console.error(`\nHAPTICS SMOKE: FAIL\n${error.stack || error}`);
  process.exitCode = 1;
} finally {
  try { command(["close"], false); } catch (_) { /* best-effort cleanup */ }
}

#!/usr/bin/env node
"use strict";

// Real-browser mock of the TipTap SDK 2.2 persistence boundary. It proves
// loadState resolves before `_init`, the parsed object (not a quoted JSON
// string) reaches saveState, and the TipTap artifact contains no browser-
// storage API token.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { spawnSync } = require("node:child_process");

const consoleBin = process.env.CONSOLE_BIN || process.argv[2];
const browser = process.env.CONSOLE_BROWSER;
if (!consoleBin || !browser) {
  console.error("usage: CONSOLE_BROWSER=/path/chrome CONSOLE_BIN=/path/console node web/tiptap-persistence-smoke.cjs");
  process.exit(2);
}
const dir = fs.mkdtempSync(path.join(os.tmpdir(), "console-tiptap-smoke-"));
const cart = path.join(dir, "save.cart");
const packed = path.join(dir, "save.html");
fs.writeFileSync(cart, `__meta__
title=TipTap Persistence Smoke
save_id=org.example.tiptap-smoke
save_version=2
__lua__
function _init()
  local data,version=save_load()
  assert(version==1 and data.count==41 and data.label=="蛙")
  assert(save_store({count=data.count+1,label=data.label}))
end
function _draw() cls(9) end
`);
const built = spawnSync(consoleBin, ["pack", cart, "--target", "tiptap", "-o", packed], { encoding: "utf8" });
if (built.status !== 0) throw new Error(built.stderr || built.stdout);
let html = fs.readFileSync(packed, "utf8");
if (html.toLowerCase().includes("localstorage")) throw new Error("TipTap artifact contains localStorage");
const mock = `<script>
window.__tiptapMock={calls:[],saved:null,errorHandlerRegistered:false};
window.TipTap={
  onStateError:function(cb){window.__tiptapMock.errorHandlerRegistered=typeof cb==="function";},
  loadState:function(cb){window.__tiptapMock.calls.push("load");setTimeout(function(){cb({data:{count:41,label:"蛙"},id:"org.example.tiptap-smoke",version:1});},20);},
  saveState:function(value){window.__tiptapMock.calls.push("save");window.__tiptapMock.saved=value;}
};
</script>`;
html = html.replace('<script type="text/cart"', `${mock}\n<script type="text/cart"`);
fs.writeFileSync(packed, html);

const session = `console-tiptap-persistence-${process.pid}`;
function command(args) {
  const result = spawnSync("agent-browser", ["--session", session, "--json", ...args], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(result.stderr || result.stdout);
  const envelope = JSON.parse(result.stdout);
  if (!envelope.success) throw new Error(envelope.error);
  return envelope.data;
}
function evaluate(source) {
  return JSON.parse(command(["eval", "-b", Buffer.from(source).toString("base64")]).result);
}
try {
  command(["--executable-path", browser, "--allow-file-access", "open", pathToFileURL(packed).href]);
  let result;
  for (let i = 0; i < 80; i++) {
    result = evaluate("JSON.stringify({status:window.__console&&window.__console.status(),mock:window.__tiptapMock})");
    if (result.status && result.status.phase !== "booting") break;
    command(["wait", "50"]);
  }
  if (result.status.phase !== "ready") throw new Error(JSON.stringify(result));
  if (result.status.persistence.backend !== "tiptap" || result.status.persistence.revision !== 1) {
    throw new Error(`bad persistence status ${JSON.stringify(result.status.persistence)}`);
  }
  if (JSON.stringify(result.mock.calls) !== JSON.stringify(["load", "save"])) {
    throw new Error(`bad SDK order ${JSON.stringify(result.mock.calls)}`);
  }
  if (typeof result.mock.saved !== "object" || result.mock.saved.data.count !== 42 || result.mock.saved.data.label !== "蛙") {
    throw new Error(`bad saved object ${JSON.stringify(result.mock.saved)}`);
  }
  if (!result.mock.errorHandlerRegistered) throw new Error("onStateError was not registered");
  console.log("TIPTAP PERSISTENCE SMOKE: PASS");
} finally {
  spawnSync("agent-browser", ["--session", session, "close"], { encoding: "utf8" });
  fs.rmSync(dir, { recursive: true, force: true });
}

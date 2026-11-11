import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import ts from "typescript";

// Run the actual controller/renderer with deterministic Tauri events and DOM
// controls. No real receiver, audio capture, timers, or network are involved.
class Element {
  children = [];
  listeners = new Map();
  textContent = "";
  value = "";
  hidden = false;
  checked = false;
  classList = { add() {}, remove() {}, toggle() {} };
  set innerHTML(value) { this.children = []; }
  get options() { return this.children; }
  append(...items) { this.children.push(...items); }
  add(item) { this.append(item); }
  appendChild(item) { this.append(item); return item; }
  replaceChildren(...items) { this.children = items; }
  addEventListener(name, callback) { this.listeners.set(name, callback); }
  setAttribute() {}
  contains() { return false; }
  click() { this.listeners.get("click")?.(); }
}

const settle = () => new Promise(resolve => setImmediate(resolve));
async function harness() {
  const controls = new Map();
  const events = new Map();
  const calls = [];
  let fail = true;
  const document = {
    getElementById(id) {
      if (!controls.has(id)) controls.set(id, new Element());
      return controls.get(id);
    },
    querySelectorAll: () => [],
    createElement: () => new Element(),
    createTextNode: text => text,
  };
  document.getElementById("toast").hidden = true;
  document.getElementById("volume").value = "20";
  document.getElementById("latency").value = "3000";
  const api = {
    async invoke(command) {
      calls.push(command);
      if (command === "connect_device") {
        if (fail) throw new Error("receiver capabilities: invalid UUID: compound");
        return { ip: "192.0.2.10", port: 7000, name: "Office" };
      }
      if (command === "get_audio_options") return { device_id: null, mute_local: false };
      if (command === "list_audio_outputs") return [];
      if (command === "get_receiver_preferences") return { volume: 0.2, latency_ms: 3000, reconnect: false };
      return null;
    },
  };
  const i18n = {
    applyStaticTranslations() {}, getLang: () => "en", onLangChange() {}, toggleLang() {},
    t: (key, params) => params?.err ? `Error: ${params.err}` : key,
  };
  function load(source, imports = {}, globals = {}) {
    const exports = {};
    const code = ts.transpileModule(source, {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2021 },
    }).outputText;
    vm.runInNewContext(code, { exports, require: name => {
      assert.ok(name in imports, `Unexpected import: ${name}`);
      return imports[name];
    }, ...globals });
    return exports;
  }
  const devices = load(await readFile(new URL("./devices.ts", import.meta.url), "utf8"));
  load(await readFile(new URL("./main.ts", import.meta.url), "utf8"), {
    "@tauri-apps/api/core": api,
    "@tauri-apps/api/event": { async listen(name, callback) {
      events.set(name, callback); return () => events.delete(name);
    } },
    "./devices": devices, "./i18n": i18n,
  }, {
    document, console, queueMicrotask,
    window: { setInterval: () => 0, setTimeout: () => 0 },
    clearTimeout() {}, clearInterval() {},
    Option: class extends Element { constructor(text, value) { super(); this.textContent = text; this.value = value; } },
  });
  await settle();
  const receiver = { id: "Office._airplay._tcp.local.", hardware_id: "aabbccddeeff",
    name: "Office", host: "office.local.", addresses: ["192.0.2.10"], port: 7000,
    kind: "homepod", model: "AudioAccessory11,1", features: null, supports_airplay2: true };
  const announce = () => events.get("airplay://device")({ payload: receiver });
  const clickConnect = () => document.getElementById("devices").children[0].children[1].click();
  announce();
  return { document, calls, announce, clickConnect, succeed: () => { fail = false; } };
}

test("a connection rejection stays visible after the final render and discovery updates", async () => {
  const app = await harness();
  app.clickConnect();
  await settle();
  assert.ok(app.calls.includes("connect_device"));
  assert.match(app.document.getElementById("status").textContent, /invalid UUID/);
  app.announce();
  assert.match(app.document.getElementById("status").textContent, /invalid UUID/);
});

test("a successful retry clears the old failure and exposes the player", async () => {
  const app = await harness();
  app.clickConnect();
  await settle();
  app.succeed();
  app.clickConnect();
  await settle();
  assert.doesNotMatch(app.document.getElementById("status").textContent, /invalid UUID/);
  assert.equal(app.document.getElementById("player").hidden, false);
});

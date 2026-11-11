import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import ts from "typescript";

// Exercise the real event handlers and renderer with a rejected Tauri command.
const source = await readFile(new URL("./main.ts", import.meta.url), "utf8");
const ast = ts.createSourceFile("main.ts", source, ts.ScriptTarget.Latest, true);
const functions = ast.statements.filter(node => ts.isFunctionDeclaration(node)
  && ["connect", "disconnect", "render", "escape"].includes(node.name?.text));
const script = ts.transpileModule(functions.map(node => node.getText(ast)).join("\n"), {
  compilerOptions: { target: ts.ScriptTarget.ES2020 },
}).outputText;

function harness() {
  const element = () => ({ appendChild() {}, addEventListener() {}, textContent: "" });
  const device = { id: "speaker", name: "HomePod", kind: "homepod", addresses: ["192.0.2.27"], port: 7000 };
  const context = vm.createContext({
    actionBusy: false, latencyApplying: false, playing: false, multiToggle: {},
    connectedId: null, connectingId: null, connectedRoute: null, connectionError: null,
    synchronizedPlaying: false, intentSequence: 0, known: new Map([[device.id, device]]),
    discovered: new Map(), activeRoutes: new Map(), KIND_LABEL: { homepod: "HomePod" },
    devicesList: element(), statusEl: element(), document: { createElement: element },
    t: (key, args) => args?.err ?? key, updatePlayerUi() {}, refreshLatencyUi() {},
    showToast(message) { context.toast = message; },
    routesFor: () => [device], loadReceiverPreferences: async () => {},
    invoke: async () => { throw new Error("receiver capabilities: invalid UUID"); },
  });
  vm.runInContext(script, context);
  return { context, device };
}

test("connection failure survives finally-render and later discovery renders, then clears on retry", async () => {
  const { context, device } = harness();
  await context.connect(device);
  assert.match(context.statusEl.textContent, /invalid UUID/);
  assert.match(context.toast, /invalid UUID/);
  assert.equal(context.actionBusy, false);
  assert.equal(context.connectingId, null);
  context.render();
  assert.match(context.statusEl.textContent, /invalid UUID/);
  context.invoke = async () => ({});
  await context.connect(device);
  assert.equal(context.connectedId, device.id);
  assert.equal(context.connectionError, null);
  assert.equal(context.statusEl.textContent, "devices_count_one");
});

test("disconnect failure remains visible after rendering", async () => {
  const { context, device } = harness();
  context.connectedId = device.id;
  await context.disconnect(device);
  context.render();
  assert.match(context.statusEl.textContent, /invalid UUID/);
  assert.equal(context.connectedId, device.id);
});

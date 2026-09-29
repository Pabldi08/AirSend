import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const source = await readFile(new URL("./devices.ts", import.meta.url), "utf8");
const javascript = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 },
}).outputText;
const { groupDevices, routesFor } = await import(`data:text/javascript;base64,${Buffer.from(javascript).toString("base64")}`);

function device(id, hardware_id, name, port, kind = "otherairplay") {
  return {
    id, hardware_id, name, host: "speaker.local.", addresses: ["192.168.1.20"],
    port, kind, model: null, features: null, supports_airplay2: true,
  };
}

test("AirPlay and RAOP advertisements show once and retain a working route", () => {
  const airplay = device("Living Room._airplay._tcp.local.", "aabbccddeeff", "Living Room", 7000, "homepod");
  const raop = device("AABBCCDDEEFF@Living Room._raop._tcp.local.", "aabbccddeeff", "AABBCCDDEEFF@Living Room", 7453);
  const discovered = new Map([[raop.id, raop], [airplay.id, airplay]]);
  const grouped = groupDevices(discovered);
  assert.equal(grouped.size, 1);
  const shown = [...grouped.values()][0];
  assert.equal(shown.name, "Living Room");
  assert.equal(shown.kind, "homepod");
  assert.deepEqual(routesFor(discovered, shown).map((item) => item.port), [7000, 7453]);
  assert.deepEqual(routesFor(discovered, shown, raop).map((item) => item.port), [7453, 7000]);
});

test("separate hardware and manually added devices stay separate", () => {
  const first = device("Room._airplay._tcp.local.", "aabbccddeeff", "Room", 7000);
  const second = device("AABBCCDDEEFF@Room._raop._tcp.local.", "aabbccddeeff", "AABBCCDDEEFF@Room", 7001);
  const third = device("Other._airplay._tcp.local.", "112233445566", "Room", 7002);
  const manual = device("manual://192.168.1.21:7000", null, "Room", 7000);
  const discovered = new Map([first, second, third, manual].map((item) => [item.id, item]));
  assert.equal(groupDevices(discovered).size, 3);
});

test("a third-party receiver uses its RAOP route and AirPlay label", () => {
  const airplay = device("Denon._airplay._tcp.local.", "000678aabbcc", "Denon", 7000);
  const raop = device("000678AABBCC@Denon._raop._tcp.local.", "000678aabbcc", "000678AABBCC@Denon", 7001);
  const discovered = new Map([[airplay.id, airplay], [raop.id, raop]]);
  const shown = [...groupDevices(discovered).values()][0];
  assert.equal(shown.name, "Denon");
  assert.equal(shown.port, 7001);
  assert.deepEqual(routesFor(discovered, shown).map((item) => item.port), [7001, 7000]);
});

test("configured stereo pairs use two hardware identities and preserve their leader", () => {
  const left = { ...device("left._airplay._tcp.local.", "aabbccddeeff", "Left", 7000, "homepod"), tight_sync_id: "pair-1", group_name: "Living room", is_group_leader: true };
  const leftRaop = { ...left, id: "AABBCCDDEEFF@Left._raop._tcp.local.", port: 7001 };
  const right = { ...device("right._airplay._tcp.local.", "112233445566", "Right", 7000, "homepod"), tight_sync_id: "pair-1" };
  const discovered = new Map([right, leftRaop, left].map(d => [d.id, d]));
  assert.equal(groupDevices(discovered).size, 2);
  const pairs = groupDevices(discovered, true);
  assert.equal(pairs.size, 1);
  assert.deepEqual(pairs.get("stereo:pair-1").member_ids, [left.id, right.id]);
  assert.equal(pairs.get("stereo:pair-1").name, "Living room");
  assert.equal(pairs.get("stereo:pair-1").pair_complete, true);
});

test("one physical member plus its RAOP advertisement cannot form a complete pair", () => {
  const left = { ...device("left._airplay._tcp.local.", "aabbccddeeff", "Left", 7000, "homepod"), tight_sync_id: "pair-1" };
  const raop = { ...left, id: "AABBCCDDEEFF@Left._raop._tcp.local.", port: 7001 };
  const discovered = new Map([left, raop].map(d => [d.id, d]));
  assert.equal(groupDevices(discovered, true).get("stereo:pair-1").pair_complete, false);
});

test("a room group alone does not imply a stereo pair, and unavailable routes come last", () => {
  const first = { ...device("first._airplay._tcp.local.", "aabbccddeeff", "First", 7000, "homepod"), group_id: "room", available: false };
  const second = { ...device("second._airplay._tcp.local.", "112233445566", "Second", 7000, "homepod"), group_id: "room" };
  const raop = { ...first, id: "AABBCCDDEEFF@First._raop._tcp.local.", port: 7001, available: true };
  const discovered = new Map([first, raop, second].map(d => [d.id, d]));
  const known = groupDevices(discovered, true);
  assert.equal(known.size, 2);
  assert.equal(known.get("hardware:aabbccddeeff").available, true);
  assert.deepEqual(routesFor(discovered, known.get("hardware:aabbccddeeff")).map(d => d.port), [7001, 7000]);
  assert.deepEqual(routesFor(discovered, known.get("hardware:aabbccddeeff"), first).map(d => d.port), [7001, 7000]);
});

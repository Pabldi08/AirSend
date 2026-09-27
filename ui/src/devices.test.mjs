import assert from "node:assert/strict";
import test from "node:test";
import { groupDevices, routesFor } from "./devices.ts";

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

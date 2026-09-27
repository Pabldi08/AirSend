export type DeviceKind = "homepod" | "appletv" | "airportexpress" | "otherairplay";

export interface Device {
  id: string;
  hardware_id: string | null;
  name: string;
  host: string;
  addresses: string[];
  port: number;
  kind: DeviceKind;
  model: string | null;
  features: string | null;
  supports_airplay2: boolean;
}

function isRaop(device: Device): boolean {
  return device.id.toLowerCase().includes("._raop._tcp.");
}

function isAirPlay(device: Device): boolean {
  return device.id.toLowerCase().includes("._airplay._tcp.");
}

function displayName(device: Device): string {
  return isRaop(device) ? device.name.replace(/^[0-9a-f:-]{12,17}@/i, "") : device.name;
}

export function deviceGroupKey(device: Device): string {
  if (!isRaop(device) && !isAirPlay(device)) return device.id;
  if (device.hardware_id) return `hardware:${device.hardware_id}`;
  return `host:${device.host.toLowerCase()}|${displayName(device).toLowerCase()}`;
}

export function routesFor(discovered: Map<string, Device>, device: Device, first: Device | null = null): Device[] {
  const routes = [...discovered.values()].filter((candidate) => deviceGroupKey(candidate) === device.id);
  if (routes.length === 0) return [device];
  routes.sort((a, b) => {
    const preference = device.kind === "homepod" ? isAirPlay : isRaop;
    return Number(preference(b)) - Number(preference(a));
  });
  if (first) routes.sort((a, b) => Number(b.id === first.id) - Number(a.id === first.id));
  return routes;
}

export function groupDevices(discovered: Map<string, Device>): Map<string, Device> {
  const groups = new Map<string, Device[]>();
  for (const device of discovered.values()) {
    const key = deviceGroupKey(device);
    const group = groups.get(key) ?? [];
    group.push(device);
    groups.set(key, group);
  }
  const known = new Map<string, Device>();
  for (const [key, group] of groups) {
    const presentation = group.find(isAirPlay) ?? group[0];
    const transport = presentation.kind === "homepod"
      ? group.find(isAirPlay) ?? group[0]
      : group.find(isRaop) ?? group[0];
    known.set(key, {
      ...transport,
      id: key,
      name: displayName(presentation),
      kind: presentation.kind,
      model: presentation.model ?? transport.model,
      features: transport.features ?? presentation.features,
      supports_airplay2: group.some((device) => device.supports_airplay2),
    });
  }
  return known;
}

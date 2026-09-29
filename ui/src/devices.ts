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
  available?: boolean;
  manual?: boolean;
  group_id?: string | null;
  tight_sync_id?: string | null;
  group_name?: string | null;
  is_group_leader?: boolean;
  member_ids?: string[];
  member_names?: string[];
  pair_complete?: boolean;
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
    if ((a.available === false) !== (b.available === false)) return Number(a.available === false) - Number(b.available === false);
    const preference = device.kind === "homepod" ? isAirPlay : isRaop;
    return Number(preference(b)) - Number(preference(a));
  });
  if (first && routes.some(d => d.id === first.id && d.available !== false))
    routes.sort((a, b) => Number(b.id === first.id) - Number(a.id === first.id));
  return routes;
}

export function groupDevices(discovered: Map<string, Device>, stereo = false): Map<string, Device> {
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
      available: group.some((device) => device.available !== false),
    });
  }
  if (stereo) {
    const pairs = new Map<string, Device[]>();
    for (const device of known.values()) {
      if (device.kind !== "homepod" || !device.tight_sync_id) continue;
      const members = pairs.get(device.tight_sync_id) ?? [];
      members.push(device); pairs.set(device.tight_sync_id, members);
    }
    for (const [id, members] of pairs) {
      members.sort((a,b) => Number(Boolean(b.is_group_leader)) - Number(Boolean(a.is_group_leader)) || a.id.localeCompare(b.id));
      const member_ids = members.map((d) => routesFor(discovered, d)[0].id);
      for (const member of members) known.delete(member.id);
      known.set(`stereo:${id}`, { ...members[0], id: `stereo:${id}`, name: members.find(d => d.group_name)?.group_name ?? members.map(d => d.name).join(" + "),
        member_ids, member_names: members.map(d => d.name), pair_complete: members.length === 2 && members.every(d => d.available !== false) });
    }
  }
  return known;
}

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { groupDevices, routesFor, type Device, type DeviceKind } from "./devices";
import {
  applyStaticTranslations,
  getLang,
  onLangChange,
  t,
  toggleLang,
} from "./i18n";

const KIND_LABEL: Record<DeviceKind, string> = {
  homepod: "HomePod",
  appletv: "Apple TV",
  airportexpress: "AirPort Express",
  otherairplay: "AirPlay",
};

const devicesList = document.getElementById("devices") as HTMLUListElement;
const scanBtn = document.getElementById("scan") as HTMLButtonElement;
const statusEl = document.getElementById("status") as HTMLSpanElement;
const toastEl = document.getElementById("toast") as HTMLDivElement;

let toastTimer: number | null = null;
function showToast(msg: string, durationMs = 5000) {
  toastEl.textContent = msg;
  toastEl.hidden = false;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toastEl.hidden = true;
    toastTimer = null;
  }, durationMs);
}

// Errores asíncronos del backend (pump muriendo, heartbeat fallando reiteradamente).
// Los errores síncronos de invoke se siguen mostrando junto a su botón asociado.
// La función se llama al final del archivo, una vez declarados `playing` y demás.
function setupAsyncErrorListener() {
  void listen<{generation: number; code: string}>("airplay://error", async (event) => {
    if (event.payload.generation !== activeGeneration) return;
    const message = t(event.payload.code);
    showToast(message);
    playing = false; activeRoutes.clear();
    playerStatus.textContent = t("error_prefix", { err: message });
    updatePlayerUi(); render();
  });
  void listen<{generation: number; code: string}>("airplay://session", (event) => {
    if (event.payload.generation < activeGeneration) return;
    activeGeneration = event.payload.generation;
    if (event.payload.code === "recovering") { playing = true; playerStatus.textContent = t("session_recovering"); }
    if (event.payload.code === "streaming") { playing = true; playerStatus.textContent = t("player_playing"); }
    if (event.payload.code === "disconnected" && !actionBusy) { playing = false; activeRoutes.clear(); }
    updatePlayerUi();
  });
}

const known = new Map<string, Device>();
const discovered = new Map<string, Device>();
let unlisten: UnlistenFn | null = null;
let unlistenRemoved: UnlistenFn | null = null;
let connectedId: string | null = null;
let connectingId: string | null = null;
let connectedRoute: Device | null = null;
const activeRoutes = new Map<string, Device>();
let actionBusy = false;
let activeGeneration = 0;
let intentSequence = 0;
let stopping = false;
let volumeRevision = 0;

const multiToggle = document.getElementById("multi-device") as HTMLInputElement;
const confirmDialog = document.getElementById("confirm-dialog") as HTMLDialogElement;
const confirmTitle = document.getElementById("confirm-title") as HTMLHeadingElement;
const confirmMessage = document.getElementById("confirm-message") as HTMLParagraphElement;
const confirmCancel = document.getElementById("confirm-cancel") as HTMLButtonElement;
const confirmAccept = document.getElementById("confirm-accept") as HTMLButtonElement;

function askConfirmation(title: string, message: string, acceptLabel: string): Promise<boolean> {
  confirmTitle.textContent = title;
  confirmMessage.textContent = message;
  confirmAccept.textContent = acceptLabel;
  return new Promise((resolve) => {
    confirmDialog.returnValue = "";
    confirmAccept.onclick = () => confirmDialog.close("accept");
    confirmCancel.onclick = () => confirmDialog.close("cancel");
    confirmDialog.addEventListener("close", () => resolve(confirmDialog.returnValue === "accept"), { once: true });
    confirmDialog.showModal();
  });
}

interface ConnectionInfo {
  ip: string;
  port: number;
  name: string;
}

async function streamTo(device: Device, command: "start_streaming" | "add_streaming", preferred?: Device | null): Promise<{ route: Device; ip: string }> {
  if (device.member_ids) {
    if (command === "add_streaming") throw new Error(t("stop_before_group_change"));
    if (!device.pair_complete) throw new Error(t("incomplete_pair"));
    await invoke("start_group_streaming", { deviceIds: device.member_ids, stereoPair: true, latencyMs: confirmedLatencyMs });
    activeGeneration = (await invoke<{generation: number}>("get_session_status")).generation;
    synchronizedPlaying = true;
    return { route: device, ip: device.addresses[0] };
  }
  const preferences = command === "add_streaming" || (playing && connectedId !== device.id)
    ? await invoke<ReceiverPreferences>("get_receiver_preferences", { receiverId: device.id }) : null;
  let lastError: unknown = new Error("dispositivo sin dirección IP");
  for (const route of routesFor(discovered, device, preferred ?? null)) {
    const ip = route.addresses.find((a) => !a.includes(":")) ?? route.addresses[0];
    if (!ip) continue;
    try {
      await invoke(command, {
        deviceId: route.id,
        ip,
        port: route.port,
        name: device.name,
        volume: preferences?.volume ?? Number(volumeSlider.value) / 100,
        latencyMs: command === "add_streaming" ? confirmedLatencyMs : preferences?.latency_ms ?? confirmedLatencyMs,
      });
      activeGeneration = (await invoke<{generation: number}>("get_session_status")).generation;
      if (command === "start_streaming") synchronizedPlaying = false;
      if (preferences && command === "start_streaming") { confirmedLatencyMs = preferences.latency_ms; latencySlider.value = String(confirmedLatencyMs); volumeSlider.value = String(Math.round(preferences.volume * 100)); volumeOut.value = `${volumeSlider.value}%`; autoReconnect.checked = preferences.reconnect; }
      return { route, ip };
    } catch (err) {
      lastError = String(err) === "capture_interrupted" ? t("capture_interrupted") : err;
      if (!String(err).startsWith("stream:")) break;
    }
  }
  throw lastError;
}

async function connect(device: Device) {
  if (actionBusy || latencyApplying) return;
  if (playing && !multiToggle.checked) {
    const current = connectedId ? known.get(connectedId)?.name ?? t("player_playing") : t("player_playing");
    const confirmed = await askConfirmation(
      t("switch_title"),
      t("switch_message", { from: current, to: device.name }),
      t("switch_confirm", { name: device.name }),
    );
    if (!confirmed || actionBusy) return;
  }
  const intent = ++intentSequence;
  actionBusy = true;
  connectingId = device.id;
  render();
  try {
    if (playing) {
      const { route, ip } = await streamTo(device, multiToggle.checked ? "add_streaming" : "start_streaming");
      if (intent !== intentSequence) return;
      if (!multiToggle.checked) {
        activeRoutes.clear();
        connectedId = device.id;
        connectedRoute = route;
        void invoke("save_last_device", { ip, port: route.port, name: device.name });
      }
      activeRoutes.set(device.id, route);
      return;
    }
    if (device.member_ids) { connectedId = device.id; connectedRoute = device; await loadReceiverPreferences(device.id); return; }
    let lastError: unknown = new Error("dispositivo sin dirección IP");
    let route: Device | null = null;
    for (const candidate of routesFor(discovered, device)) {
      const ip = candidate.addresses.find((a) => !a.includes(":")) ?? candidate.addresses[0];
      if (!ip) continue;
      try {
        await invoke<ConnectionInfo>("connect_device", {
          deviceId: candidate.id,
          ip,
          port: candidate.port,
          name: device.name,
        });
        route = candidate;
        break;
      } catch (err) {
        lastError = err;
        if (String(err) === "session_cancelled" || intent !== intentSequence) return;
      }
    }
    if (!route) throw lastError;
    if (intent !== intentSequence) return;
    connectedRoute = route;
    connectedId = device.id;
    await loadReceiverPreferences(device.id);
  } catch (err) {
    statusEl.textContent = t("error_prefix", { err: String(err) });
  } finally {
    actionBusy = false;
    connectingId = null;
    render();
  }
}

async function disconnect(device: Device) {
  if (actionBusy || latencyApplying) return;
  actionBusy = true;
  render();
  try {
    if (playing && synchronizedPlaying) { await stopPlay(); return; }
    if (playing && activeRoutes.size > 1) {
      const route = activeRoutes.get(device.id);
      const ip = route?.addresses.find((a) => !a.includes(":")) ?? route?.addresses[0];
      if (!ip || !route) throw new Error("dispositivo sin dirección IP");
      await invoke("remove_streaming", { ip, port: route.port });
      activeRoutes.delete(device.id);
      if (connectedId === device.id) {
        const next = activeRoutes.entries().next().value as [string, Device];
        connectedId = next[0];
        connectedRoute = next[1];
        const nextIp = next[1].addresses.find((a) => !a.includes(":")) ?? next[1].addresses[0];
        if (nextIp) void invoke("save_last_device", { ip: nextIp, port: next[1].port, name: known.get(next[0])?.name ?? next[1].name });
      }
      return;
    }
    if (playing) {
      await invoke("stop_streaming");
      playing = false;
      activeRoutes.clear();
      playerStatus.textContent = "";
    }
    await invoke("disconnect_device");
    connectedId = null;
    connectedRoute = null;
  } catch (err) {
    statusEl.textContent = t("error_prefix", { err: String(err) });
  } finally {
    actionBusy = false;
    render();
  }
}

const playerEl = document.getElementById("player") as HTMLDivElement;
const playBtn = document.getElementById("play-stop") as HTMLButtonElement;
const playerStatus = document.getElementById("player-status") as HTMLSpanElement;
const volumeSlider = document.getElementById("volume") as HTMLInputElement;
const volumeOut = document.getElementById("vol-out") as HTMLOutputElement;
const latencySlider = document.getElementById("latency") as HTMLInputElement;
const latencyOut = document.getElementById("latency-out") as HTMLOutputElement;
const latencyConfirm = document.getElementById("latency-confirm") as HTMLButtonElement;
const localBufferToggle = document.getElementById("experimental-local-buffer") as HTMLInputElement;
const diagnosticsExport = document.getElementById("diagnostics-export") as HTMLButtonElement;
const diagnosticsStatus = document.getElementById("diagnostics-status") as HTMLSpanElement;
let savedLocalBuffer = false;

localBufferToggle.addEventListener("change", async () => {
  const enabled = localBufferToggle.checked;
  if (playing || actionBusy || latencyApplying) {
    localBufferToggle.checked = savedLocalBuffer;
    return;
  }
  localBufferToggle.disabled = true;
  try {
    await invoke("save_experimental_local_buffer", { enabled });
    savedLocalBuffer = enabled;
  } catch (err) {
    localBufferToggle.checked = savedLocalBuffer;
    const message = String(err) === "stop_before_buffer_change" ? t("local_buffer_stop") : t("local_buffer_error", { err: String(err) });
    showToast(message);
  } finally {
    updatePlayerUi();
  }
});

diagnosticsExport.addEventListener("click", async () => {
  diagnosticsExport.disabled = true;
  try {
    const report = await invoke<string>("export_audio_diagnostics");
    const url = URL.createObjectURL(new Blob([report], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = `AirSend-diagnostics-${new Date().toISOString().replaceAll(":", "-")}.json`;
    document.body.append(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 60_000);
    diagnosticsStatus.textContent = t("diagnostics_ready");
  } catch (err) {
    diagnosticsStatus.textContent = t("diagnostics_error", { err: String(err) });
  } finally {
    diagnosticsExport.disabled = false;
  }
});

const latencyStatus = document.getElementById("latency-status") as HTMLSpanElement;

let playing = false;
let synchronizedPlaying = false;
let windowsAudioAvailable = false;
let volumeDebounce: number | null = null;
let confirmedLatencyMs = 3000;
let latencyApplying = false;
let latencyCooldownUntil = 0;
let latencyTimer: number | null = null;

function updatePlayerUi() {
  const dev = connectedId ? known.get(connectedId) : null;
  playerEl.hidden = !dev;
  playBtn.disabled = stopping || ((latencyApplying || actionBusy) && !playing);
  cancelOperation.hidden = !actionBusy && !latencyApplying;
  audioSource.disabled = playing || actionBusy || latencyApplying;
  muteLocal.disabled = !windowsAudioAvailable || playing || actionBusy || latencyApplying;
  stereoPairs.disabled = playing || actionBusy || latencyApplying;
  autoReconnect.disabled = actionBusy || latencyApplying;
  if (!playing) document.getElementById("receiver-volumes")!.replaceChildren();
  groupPlay.disabled = playing || actionBusy || latencyApplying;
  groupFirst.disabled = groupSecond.disabled = playing || actionBusy || latencyApplying;
  localBufferToggle.disabled = playing || actionBusy || latencyApplying;
  if (!dev) return;
  if (playing) {
    playBtn.textContent = t("stop");
    playBtn.classList.add("playing");
  } else {
    playBtn.textContent = t("play_to", { name: dev.name });
    playBtn.classList.remove("playing");
  }
}

async function startPlay() {
  if (actionBusy || latencyApplying) return;
  const dev = connectedId ? known.get(connectedId) : null;
  if (!dev) return;
  const intent = ++intentSequence;
  actionBusy = true;
  playBtn.disabled = true;
  cancelOperation.hidden = false;
  playerStatus.textContent = t("player_starting");
  try {
    const vol = Number(volumeSlider.value) / 100;
    const { route, ip } = await streamTo(dev, "start_streaming", connectedRoute);
    if (intent !== intentSequence) return;
    connectedRoute = route;
    activeRoutes.clear();
    activeRoutes.set(dev.id, route);
    playing = true;
    playerStatus.textContent = t("player_playing");
    void invoke("save_last_device", { ip, port: route.port, name: dev.name });
    // Persistimos para auto-reconnect (C2) y carga rápida en futuros arranques.
    void invoke("save_volume", { volume: vol });
  } catch (err) {
    playerStatus.textContent = t("error_prefix", { err: String(err) });
  } finally {
    actionBusy = false;
    updatePlayerUi();
    render();
  }
}

async function stopPlay() {
  if (stopping) return;
  ++intentSequence;
  stopping = true;
  actionBusy = true;
  playBtn.disabled = true;
  try {
    await invoke("stop_streaming");
    await invoke("disconnect_device");
    playing = false; synchronizedPlaying = false;
    activeGeneration = (await invoke<{generation: number}>("get_session_status")).generation;
    activeRoutes.clear();
    playerStatus.textContent = "";
  } catch (error) { showToast(t("error_prefix", { err: String(error) })); } finally {
    stopping = false;
    actionBusy = false;
    updatePlayerUi();
    render();
  }
}

playBtn.addEventListener("click", () => {
  if (playing) void stopPlay();
  else void startPlay();
});

multiToggle.addEventListener("change", async () => {
  const enabled = multiToggle.checked;
  if (actionBusy || latencyApplying) {
    multiToggle.checked = !enabled;
    return;
  }
  if (!enabled && playing && activeRoutes.size > 1) {
    const keepId = connectedId && activeRoutes.has(connectedId)
      ? connectedId
      : activeRoutes.keys().next().value as string;
    const keepName = known.get(keepId)?.name ?? t("player_playing");
    const confirmed = await askConfirmation(
      t("multi_off_title"),
      t("multi_off_message", { name: keepName }),
      t("multi_off_confirm", { name: keepName }),
    );
    if (!confirmed) {
      multiToggle.checked = true;
      return;
    }
    actionBusy = true;
    render();
    try {
      for (const [id, route] of [...activeRoutes]) {
        if (id === keepId) continue;
        const ip = route.addresses.find((a) => !a.includes(":")) ?? route.addresses[0];
        if (!ip) throw new Error("dispositivo sin dirección IP");
        await invoke("remove_streaming", { ip, port: route.port });
        activeRoutes.delete(id);
      }
    } catch (err) {
      multiToggle.checked = true;
      showToast(t("error_prefix", { err: String(err) }));
      return;
    } finally {
      actionBusy = false;
      render();
    }
  }
  try {
    await invoke("save_multi_device", { enabled });
  } catch (err) {
    multiToggle.checked = !enabled;
    showToast(t("error_prefix", { err: String(err) }));
  }
});

volumeSlider.addEventListener("input", () => {
  volumeRevision++;
  volumeOut.textContent = `${volumeSlider.value}%`;
  if (volumeDebounce) clearTimeout(volumeDebounce);
  volumeDebounce = window.setTimeout(async () => {
    const vol = Number(volumeSlider.value) / 100;
    // Persistimos siempre (aunque no haya streaming activo) para que el próximo
    // arranque recuerde la preferencia del usuario.
    void invoke("save_volume", { volume: vol });
    if (!playing) { if (connectedId) void patchReceiverPreferences(connectedId, { volume: vol }).catch(e => showToast(String(e))); return; }
    try {
      await invoke("set_stream_volume", { volume: vol });
      if (connectedId) await patchReceiverPreferences(connectedId, { volume: vol });
    } catch (err) {
      playerStatus.textContent = t("vol_error_prefix", { err: String(err) });
    }
  }, 120);
});

function refreshLatencyUi() {
  const pending = Number(latencySlider.value);
  const cooldownMs = Math.max(0, latencyCooldownUntil - Date.now());
  latencyOut.value = `${pending} ms`;
  latencyConfirm.disabled = actionBusy || latencyApplying || cooldownMs > 0 || pending === confirmedLatencyMs;
  if (latencyApplying) latencyStatus.textContent = t("latency_applying");
  else if (cooldownMs > 0)
    latencyStatus.textContent = t("latency_cooldown", { seconds: Math.ceil(cooldownMs / 1000) });
  else latencyStatus.textContent = t(pending === confirmedLatencyMs ? "latency_current" : "latency_pending");
}

function setLatencyCooldown(ms: number) {
  latencyCooldownUntil = Date.now() + ms;
  if (latencyTimer !== null) clearInterval(latencyTimer);
  latencyTimer = window.setInterval(() => {
    refreshLatencyUi();
    if (Date.now() >= latencyCooldownUntil && latencyTimer !== null) {
      clearInterval(latencyTimer);
      latencyTimer = null;
    }
  }, 250);
  refreshLatencyUi();
}

latencySlider.addEventListener("input", refreshLatencyUi);
latencyConfirm.addEventListener("click", async () => {
  const latencyMs = Number(latencySlider.value);
  if (actionBusy || latencyApplying || Date.now() < latencyCooldownUntil || latencyMs === confirmedLatencyMs) return;
  latencyApplying = true;
  playBtn.disabled = true;
  refreshLatencyUi();
  try {
    const restarted = await invoke<boolean>("confirm_latency", { latencyMs });
    activeGeneration = (await invoke<{generation: number}>("get_session_status")).generation;
    confirmedLatencyMs = latencyMs;
    if (connectedId) await patchReceiverPreferences(connectedId, { latency_ms: latencyMs });
    if (restarted && !stopping) {
      playing = true;
      playerStatus.textContent = t("player_playing");
    }
    setLatencyCooldown(10_000);
  } catch (err) {
    const message = String(err);
    const cooldown = /^latency_cooldown:(\d+)$/.exec(message);
    if (cooldown) setLatencyCooldown(Number(cooldown[1]));
    else showToast(t("latency_error", { err: message }));
    try {
      playing = (await invoke<ConnectionInfo | null>("is_streaming")) !== null;
      playerStatus.textContent = playing ? t("player_playing") : "";
    } catch {
      // Keep the previous UI state if the status query itself fails.
    }
  } finally {
    latencyApplying = false;
    refreshLatencyUi();
    updatePlayerUi();
    render();
  }
});

function render() {
  devicesList.innerHTML = "";
  const sorted = [...known.values()].sort((a, b) => {
    if (a.kind === "homepod" && b.kind !== "homepod") return -1;
    if (b.kind === "homepod" && a.kind !== "homepod") return 1;
    return a.name.localeCompare(b.name);
  });
  for (const d of sorted) {
    const li = document.createElement("li");
    const isConnected = playing ? activeRoutes.has(d.id) : connectedId === d.id;
    const isConnecting = connectingId === d.id;
    li.className = `device ${d.kind}${isConnected ? " connected" : ""}`;
    const addr = d.addresses.find((a) => !a.includes(":")) ?? d.addresses[0] ?? d.host;

    const info = document.createElement("div");
    info.className = "info";
    info.innerHTML = `
      <span class="name">${escape(d.name)}</span>
      <span class="meta">${d.member_ids ? `${escape(t("stereo_pair"))} · ${escape(d.member_names?.join(" + ") ?? "")}` : `${KIND_LABEL[d.kind]} · ${escape(addr)}:${d.port}${d.supports_airplay2 ? " · AirPlay 2" : ""}`}</span>
    `;
    li.appendChild(info);

    const btn = document.createElement("button");
    btn.className = "connect";
    if (isConnecting) {
      btn.textContent = t("connecting");
      btn.disabled = true;
    } else if (isConnected) {
      btn.textContent = t("disconnect");
      btn.disabled = actionBusy || latencyApplying;
      btn.addEventListener("click", () => void disconnect(d));
    } else {
      btn.textContent = t("connect");
      btn.disabled = actionBusy || latencyApplying || d.available === false || (Boolean(d.member_ids) && !d.pair_complete);
      if (d.member_ids && !d.pair_complete) btn.textContent = t("incomplete_pair");
      btn.addEventListener("click", () => void connect(d));
    }
    li.appendChild(btn);

    devicesList.appendChild(li);
  }
  statusEl.textContent =
    known.size === 1
      ? t("devices_count_one")
      : t("devices_count_other", { n: known.size });
  updatePlayerUi();
  multiToggle.disabled = synchronizedPlaying || actionBusy || latencyApplying;
  refreshLatencyUi();
}

function refreshKnownDevices() {
  const previous = [...known.values()];
  known.clear();
  for (const [key, device] of groupDevices(discovered, stereoPairs.checked)) known.set(key, device);
  for (const device of previous) { if (!known.has(device.id) && (connectedId === device.id || activeRoutes.has(device.id))) known.set(device.id, {...device, available: false}); }
  refreshGroupChoices();
}

function escape(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

async function startScan() {
  scanBtn.disabled = true;
  statusEl.textContent = t("scan_searching");
  discovered.clear();
  refreshKnownDevices();
  render();

  if (unlisten) {
    unlisten();
    unlisten = null;
  }

  unlistenRemoved?.();
  unlistenRemoved = await listen<string>("airplay://device-removed", event => { discovered.delete(event.payload); refreshKnownDevices(); render(); });
  unlisten = await listen<Device>("airplay://device", (event) => {
    discovered.set(event.payload.id, event.payload);
    refreshKnownDevices();
    render();
  });

  try {
    await invoke("start_discovery_stream");
  } catch (err) {
    statusEl.textContent = t("error_prefix", { err: String(err) });
  }

  scanBtn.disabled = false;
}

scanBtn.addEventListener("click", () => {
  void startScan();
});

const manualIpInput = document.getElementById("manual-ip") as HTMLInputElement;
const manualNameInput = document.getElementById("manual-name") as HTMLInputElement;
const manualBtn = document.getElementById("manual-add") as HTMLButtonElement;
const manualStatus = document.getElementById("manual-status") as HTMLSpanElement;

async function addManual() {
  const ip = manualIpInput.value.trim();
  const name = manualNameInput.value.trim() || null;
  if (!ip) {
    manualStatus.textContent = t("manual_need_ip");
    return;
  }
  manualBtn.disabled = true;
  manualStatus.textContent = t("manual_checking");
  try {
    const device = await invoke<Device>("add_manual_device", {
      ip,
      port: null,
      name,
    });
    manualStatus.textContent = t("manual_ok", { name: device.name });
    manualIpInput.value = "";
    manualNameInput.value = "";
  } catch (err) {
    manualStatus.textContent = String(err);
  } finally {
    manualBtn.disabled = false;
  }
}

manualBtn.addEventListener("click", () => {
  void addManual();
});
manualIpInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") void addManual();
});

setupAsyncErrorListener();
setupLangToggle();
// Initialize after all controls below have been created.
queueMicrotask(() => void initialize());

async function initialize() {
  await Promise.all([preloadSavedVolume(), preloadSavedLatency(), preloadMultiDevice(), preloadLocalBuffer(), loadAudioOptions().catch(e => showToast(String(e)))]);
  await bootstrap();
}

async function preloadMultiDevice() {
  try {
    multiToggle.checked = await invoke<boolean>("get_multi_device");
  } catch {
    multiToggle.checked = false;
  }
}

function setupLangToggle() {
  const btn = document.getElementById("lang-toggle") as HTMLButtonElement | null;
  applyStaticTranslations();
  refreshLangToggleLabel();
  syncTrayLanguage(getLang());
  onLangChange(() => {
    refreshLangToggleLabel();
    syncTrayLanguage(getLang());
    // Re-render lo que tiene texto dinámico generado por JS.
    render();
    updatePlayerUi();
    refreshLatencyUi();
    void refreshAudioOutputs().catch(e => showToast(String(e)));
  });
  if (btn) btn.addEventListener("click", () => toggleLang());
}

function syncTrayLanguage(lang: "es" | "en") {
  void invoke("set_tray_language", { lang }).catch((err) => {
    console.error("Could not update tray language", err);
  });
}

function refreshLangToggleLabel() {
  const btn = document.getElementById("lang-toggle") as HTMLButtonElement | null;
  if (!btn) return;
  btn.textContent = t(getLang() === "es" ? "lang_toggle_to_en" : "lang_toggle_to_es");
  const title = t("lang_toggle_title");
  btn.title = title;
  btn.setAttribute("aria-label", title);
}

async function preloadSavedVolume() {
  try {
    const v = await invoke<number | null>("get_volume");
    if (v !== null && v !== undefined) {
      const pct = Math.round(Math.max(0, Math.min(1, v)) * 100);
      volumeSlider.value = String(pct);
      volumeOut.textContent = `${pct}%`;
    }
  } catch {
    // sin volumen guardado todavía, se queda el default del HTML.
  }
}

async function preloadLocalBuffer() {
  try {
    savedLocalBuffer = await invoke<boolean>("get_experimental_local_buffer");
    localBufferToggle.checked = savedLocalBuffer;
  } catch {
    savedLocalBuffer = false;
    localBufferToggle.checked = false;
  }
}

async function preloadSavedLatency() {
  try {
    const [saved, cooldownMs] = await Promise.all([
      invoke<number | null>("get_latency"),
      invoke<number>("get_latency_cooldown_ms"),
    ]);
    if (saved !== null && saved >= 0 && saved <= 3000) {
      confirmedLatencyMs = saved;
      latencySlider.value = String(saved);
    }
    if (cooldownMs > 0) setLatencyCooldown(cooldownMs);
  } catch {
    // Keep the safe default when no setting has been stored yet.
  }
  refreshLatencyUi();
}

interface PersistedDevice {
  ip: string;
  port: number;
  name: string;
}

/// Bootstrap: arranca discovery y, si hay un último HomePod guardado, intenta
/// reconectarse a él automáticamente (C2). Si mDNS lo descubre en <RECONNECT_TIMEOUT_MS,
/// streamea directo. Si no, prueba añadirlo manualmente por la IP que tenemos
/// guardada (típico en routers Movistar HGU donde mDNS no se propaga).
const RECONNECT_TIMEOUT_MS = 6000;

async function bootstrap() {
  // Lanzamos el discovery en paralelo a la lectura de la store: la mayor parte
  // del tiempo el HomePod ya estará en `known` antes de que el promise de la
  // store resuelva.
  void startScan();

  let last: PersistedDevice | null = null;
  try {
    last = await invoke<PersistedDevice | null>("get_last_device");
  } catch {
    last = null;
  }
  if (!last) return;

  statusEl.textContent = t("reconnecting", { name: last.name });

  const matchByIp = (): Device | null => {
    for (const d of known.values()) {
      if (d.addresses.some((a) => a === last!.ip)) return d;
    }
    return null;
  };

  // Esperar hasta RECONNECT_TIMEOUT_MS a que mDNS lo encuentre.
  const deadline = Date.now() + RECONNECT_TIMEOUT_MS;
  let found = matchByIp();
  while (!found && Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 250));
    found = matchByIp();
  }

  if (!found) {
    // Fallback: probe manual al puerto 7000 y, si responde, lo añadimos a la
    // lista (mismo flujo que el botón "Añadir" de la sección manual).
    try {
      const dev = await invoke<Device>("add_manual_device", {
        ip: last.ip,
        port: last.port,
        name: last.name,
      });
      discovered.set(dev.id, dev);
      refreshKnownDevices();
      found = known.get(dev.id) ?? dev;
      render();
    } catch (err) {
      showToast(t("cant_find", { name: last.name, err: String(err) }), 6000);
      return;
    }
  }

  // Tenemos el device. Saltamos `connect_device` (que haría pair-setup adicional
  // sin uso real) y vamos directo a streaming, marcando connectedId para que
  // startPlay y el resto de la UI lo traten como activo.
  connectedId = found.id;
  connectedRoute = routesFor(discovered, found).find((route) =>
    route.port === last!.port && route.addresses.includes(last!.ip)
  ) ?? null;
  await loadReceiverPreferences(found.id);
  render();
  await startPlay();
}

interface ReceiverPreferences { volume: number; latency_ms: number; reconnect: boolean }
async function patchReceiverPreferences(id: string, patch: Partial<ReceiverPreferences>) {
  await invoke("patch_receiver_preferences", { receiverId: id, patch });
}
async function loadReceiverPreferences(id: string) {
  const preferences = await invoke<ReceiverPreferences>("get_receiver_preferences", { receiverId: id });
  if (connectedId !== id) return;
  volumeSlider.value = String(Math.round(preferences.volume * 100)); volumeOut.value = `${volumeSlider.value}%`;
  if (!playing) { confirmedLatencyMs = preferences.latency_ms; latencySlider.value = String(confirmedLatencyMs); refreshLatencyUi(); }
  autoReconnect.checked = preferences.reconnect;
}
const cancelOperation = document.getElementById("operation-cancel") as HTMLButtonElement;
cancelOperation.addEventListener("click", () => void stopPlay());
const audioSource = document.getElementById("audio-source") as HTMLSelectElement;
const muteLocal = document.getElementById("mute-local") as HTMLInputElement;
const audioStatus = document.getElementById("audio-options-status") as HTMLSpanElement;
const autoReconnect = document.getElementById("auto-reconnect") as HTMLInputElement;
autoReconnect.addEventListener("change", () => { if (connectedId) void patchReceiverPreferences(connectedId, { reconnect: autoReconnect.checked }).catch(e => showToast(String(e))); });
interface AudioOptions { device_id: string | null; mute_local: boolean }
let savedAudioOptions: AudioOptions = { device_id: null, mute_local: false };
async function loadAudioOptions() {
  savedAudioOptions = await invoke<AudioOptions>("get_audio_options");
  muteLocal.checked = savedAudioOptions.mute_local;
  await refreshAudioOutputs();
}
async function refreshAudioOutputs() {
  const devices = await invoke<{id: string; name: string; is_default: boolean}[]>("list_audio_outputs");
  windowsAudioAvailable = devices.length > 0;
  audioSource.replaceChildren(new Option(t("audio_default"), ""));
  for (const device of devices) audioSource.add(new Option(device.name + (device.is_default ? ` (${t("audio_default")})` : ""), device.id));
  if (savedAudioOptions.device_id && !devices.some(d => d.id === savedAudioOptions.device_id)) audioSource.add(new Option(t("receiver_unavailable"), savedAudioOptions.device_id));
  audioSource.value = savedAudioOptions.device_id ?? "";
  updatePlayerUi();
}
async function saveAudioOptions() {
  try {
    const options = { device_id: audioSource.value || null, mute_local: muteLocal.checked };
    await invoke("save_audio_options", { options }); savedAudioOptions = options; audioStatus.textContent = "";
  } catch (error) {
    audioSource.value = savedAudioOptions.device_id ?? ""; muteLocal.checked = savedAudioOptions.mute_local;
    audioStatus.textContent = t(String(error));
  }
}
audioSource.addEventListener("change", () => void saveAudioOptions());
muteLocal.addEventListener("change", () => void saveAudioOptions());
document.getElementById("audio-refresh")!.addEventListener("click", () => void refreshAudioOutputs().catch(e => showToast(String(e))));
document.getElementById("check-updates")!.addEventListener("click", () => void invoke("open_latest_release").catch(e => showToast(String(e))));
const stereoPairs = document.getElementById("stereo-pairs") as HTMLInputElement;
const groupFirst = document.getElementById("group-first") as HTMLSelectElement;
const groupSecond = document.getElementById("group-second") as HTMLSelectElement;
const groupPlay = document.getElementById("group-play") as HTMLButtonElement;
const groupStatus = document.getElementById("group-status") as HTMLSpanElement;
stereoPairs.addEventListener("change", () => { if (playing) { stereoPairs.checked = !stereoPairs.checked; return; } refreshKnownDevices(); render(); });
function refreshGroupChoices() {
  const first = groupFirst.value, second = groupSecond.value;
  groupFirst.replaceChildren(); groupSecond.replaceChildren();
  for (const device of groupDevices(discovered).values()) {
    if (device.kind !== "homepod") continue;
    const route = routesFor(discovered, device)[0];
    groupFirst.add(new Option(device.name, route.id)); groupSecond.add(new Option(device.name, route.id));
  }
  if (Array.from(groupFirst.options).some(o => o.value === first)) groupFirst.value = first;
  if (second !== groupFirst.value && Array.from(groupSecond.options).some(o => o.value === second)) groupSecond.value = second;
  else if (groupSecond.options.length > 1) groupSecond.selectedIndex = Array.from(groupSecond.options).findIndex(o => o.value !== groupFirst.value);
}
groupPlay.addEventListener("click", async () => {
  if (playing || actionBusy || latencyApplying) return;
  if (!groupFirst.value || groupFirst.value === groupSecond.value) { groupStatus.textContent = t("group_requires_two_receivers"); return; }
  const selected = [groupFirst.value, groupSecond.value].map(id => discovered.get(id)!);
  actionBusy = true; const intent = ++intentSequence; cancelOperation.hidden = false; render();
  try {
    await invoke("start_group_streaming", { deviceIds: selected.map(d => d.id), stereoPair: false, latencyMs: confirmedLatencyMs });
    if (intent !== intentSequence) return;
    activeGeneration = (await invoke<{generation: number}>("get_session_status")).generation;
    activeRoutes.clear();
    for (const route of selected) { activeRoutes.set([...groupDevices(discovered)].find(([,d]) => d.hardware_id === route.hardware_id)?.[0] ?? route.id, route); }
    connectedId = activeRoutes.keys().next().value ?? null;
    connectedRoute = connectedId ? activeRoutes.get(connectedId)! : null;
    playing = true; synchronizedPlaying = true; multiToggle.checked = false;
    if (connectedId) await loadReceiverPreferences(connectedId);
    playerStatus.textContent = t("player_playing"); groupStatus.textContent = "";
  } catch (error) { groupStatus.textContent = t("error_prefix", { err: String(error) }); }
  finally { actionBusy = false; updatePlayerUi(); render(); }
});
interface ReceiverVolume { ip: string; port: number; volume: number; readable: boolean }
let polling = false;
window.setInterval(async () => {
  if (polling) return;
  polling = true;
  try {
    const status = await invoke<{generation: number; mute_status: string; group_mode?: boolean}>("get_session_status");
    audioStatus.textContent = t(`mute_${status.mute_status}`);
    synchronizedPlaying = status.group_mode ?? synchronizedPlaying;
    if (!playing || actionBusy || latencyApplying) return;
    const revision = volumeRevision;
    const values = await invoke<ReceiverVolume[]>("get_receiver_volumes");
    if (revision !== volumeRevision || !playing || status.generation !== activeGeneration) return;
    const container = document.getElementById("receiver-volumes")!;
    if (container.contains(document.activeElement)) return;
    container.replaceChildren();
    for (const value of values) {
      const route = [...activeRoutes.values(), ...discovered.values()].find(d => d.port === value.port && d.addresses.includes(value.ip));
      const row = document.createElement("div"); row.className = "receiver-volume";
      const label = document.createElement("label"); label.append(document.createTextNode(route?.name ?? `${value.ip}:${value.port}`));
      const slider = document.createElement("input"); slider.type = "range"; slider.min = "0"; slider.max = "100"; slider.value = String(Math.round(value.volume * 100));
      const output = document.createElement("output"); output.textContent = `${slider.value}%`;
      let timer: number | undefined;
      slider.addEventListener("input", () => { ++volumeRevision; output.textContent = `${slider.value}%`; clearTimeout(timer); timer = window.setTimeout(() => void invoke("set_receiver_volume", { ip: value.ip, port: value.port, volume: Number(slider.value)/100 }).catch(e => showToast(String(e))), 120); });
      label.append(slider, output); row.append(label);
      if (!value.readable) { const hint = document.createElement("small"); hint.textContent = t("volume_unreadable"); row.append(hint); }
      container.append(row);
    }
  } catch { /* The next poll retries; session errors are reported by the backend. */ }
  finally { polling = false; }
}, 3000);

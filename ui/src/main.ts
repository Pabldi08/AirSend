import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  applyStaticTranslations,
  getLang,
  onLangChange,
  t,
  toggleLang,
} from "./i18n";

type DeviceKind = "homepod" | "appletv" | "airportexpress" | "otherairplay";

interface Device {
  id: string;
  name: string;
  host: string;
  addresses: string[];
  port: number;
  kind: DeviceKind;
  model: string | null;
  features: string | null;
  supports_airplay2: boolean;
}

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
  void listen<string>("airplay://error", (event) => {
    showToast(event.payload);
    if (playing) {
      playing = false;
      playerStatus.textContent = t("error_prefix", { err: event.payload });
      updatePlayerUi();
    }
  });
}

const known = new Map<string, Device>();
let unlisten: UnlistenFn | null = null;
let connectedId: string | null = null;
let connectingId: string | null = null;

interface ConnectionInfo {
  ip: string;
  port: number;
  name: string;
}

async function connect(device: Device) {
  if (connectingId) return;
  connectingId = device.id;
  render();
  try {
    const ip = device.addresses.find((a) => !a.includes(":")) ?? device.addresses[0];
    if (!ip) throw new Error("dispositivo sin dirección IP");
    await invoke<ConnectionInfo>("connect_device", {
      ip,
      port: device.port,
      name: device.name,
    });
    connectedId = device.id;
  } catch (err) {
    statusEl.textContent = t("error_prefix", { err: String(err) });
  } finally {
    connectingId = null;
    render();
  }
}

async function disconnect() {
  if (playing) await stopPlay();
  try {
    await invoke("disconnect_device");
  } finally {
    connectedId = null;
    render();
  }
}

const playerEl = document.getElementById("player") as HTMLDivElement;
const playBtn = document.getElementById("play-stop") as HTMLButtonElement;
const playerStatus = document.getElementById("player-status") as HTMLSpanElement;
const volumeSlider = document.getElementById("volume") as HTMLInputElement;
const volumeOut = document.getElementById("vol-out") as HTMLOutputElement;
const latencySelect = document.getElementById("latency") as HTMLSelectElement;

let playing = false;
let volumeDebounce: number | null = null;

function updatePlayerUi() {
  const dev = connectedId ? known.get(connectedId) : null;
  playerEl.hidden = !dev;
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
  const dev = connectedId ? known.get(connectedId) : null;
  if (!dev) return;
  const ip = dev.addresses.find((a) => !a.includes(":")) ?? dev.addresses[0];
  if (!ip) return;
  playBtn.disabled = true;
  playerStatus.textContent = t("player_starting");
  try {
    const vol = Number(volumeSlider.value) / 100;
    const latency = latencySelect.value;
    await invoke("start_streaming", {
      ip,
      port: dev.port,
      name: dev.name,
      volume: vol,
      latency,
    });
    playing = true;
    playerStatus.textContent = t("player_playing");
    // Persistimos para auto-reconnect (C2) y carga rápida en futuros arranques.
    void invoke("save_last_device", { ip, port: dev.port, name: dev.name });
    void invoke("save_volume", { volume: vol });
    void invoke("save_latency", { latency });
  } catch (err) {
    playerStatus.textContent = t("error_prefix", { err: String(err) });
  } finally {
    playBtn.disabled = false;
    updatePlayerUi();
  }
}

async function stopPlay() {
  playBtn.disabled = true;
  try {
    await invoke("stop_streaming");
    playing = false;
    playerStatus.textContent = "";
  } finally {
    playBtn.disabled = false;
    updatePlayerUi();
  }
}

playBtn.addEventListener("click", () => {
  if (playing) void stopPlay();
  else void startPlay();
});

volumeSlider.addEventListener("input", () => {
  volumeOut.textContent = `${volumeSlider.value}%`;
  if (volumeDebounce) clearTimeout(volumeDebounce);
  volumeDebounce = window.setTimeout(async () => {
    const vol = Number(volumeSlider.value) / 100;
    // Persistimos siempre (aunque no haya streaming activo) para que el próximo
    // arranque recuerde la preferencia del usuario.
    void invoke("save_volume", { volume: vol });
    if (!playing) return;
    try {
      await invoke("set_stream_volume", { volume: vol });
    } catch (err) {
      playerStatus.textContent = t("vol_error_prefix", { err: String(err) });
    }
  }, 120);
});

latencySelect.addEventListener("change", () => {
  const latency = latencySelect.value;
  // Persistimos siempre para recordar la preferencia en el próximo arranque.
  void invoke("save_latency", { latency });
  // La latencia se negocia con el HomePod al abrir el stream, así que un cambio
  // en caliente no surte efecto hasta el próximo Play. Avisamos al usuario.
  if (playing) showToast(t("latency_hint"));
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
    const isConnected = connectedId === d.id;
    const isConnecting = connectingId === d.id;
    li.className = `device ${d.kind}${isConnected ? " connected" : ""}`;
    const addr = d.addresses.find((a) => !a.includes(":")) ?? d.addresses[0] ?? d.host;

    const info = document.createElement("div");
    info.className = "info";
    info.innerHTML = `
      <span class="name">${escape(d.name)}</span>
      <span class="meta">${KIND_LABEL[d.kind]} · ${escape(addr)}:${d.port}${d.supports_airplay2 ? " · AirPlay 2" : ""}</span>
    `;
    li.appendChild(info);

    const btn = document.createElement("button");
    btn.className = "connect";
    if (isConnecting) {
      btn.textContent = t("connecting");
      btn.disabled = true;
    } else if (isConnected) {
      btn.textContent = t("disconnect");
      btn.addEventListener("click", () => void disconnect());
    } else {
      btn.textContent = t("connect");
      btn.disabled = connectingId !== null;
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
}

function escape(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

async function startScan() {
  scanBtn.disabled = true;
  statusEl.textContent = t("scan_searching");
  known.clear();
  render();

  if (unlisten) {
    unlisten();
    unlisten = null;
  }

  unlisten = await listen<Device>("airplay://device", (event) => {
    known.set(event.payload.id, event.payload);
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
void preloadSavedVolume();
void preloadSavedLatency();
void bootstrap();

function setupLangToggle() {
  const btn = document.getElementById("lang-toggle") as HTMLButtonElement | null;
  applyStaticTranslations();
  refreshLangToggleLabel();
  onLangChange(() => {
    refreshLangToggleLabel();
    // Re-render lo que tiene texto dinámico generado por JS.
    render();
    updatePlayerUi();
  });
  if (btn) btn.addEventListener("click", () => toggleLang());
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

async function preloadSavedLatency() {
  try {
    const v = await invoke<string | null>("get_latency");
    if (v === "music" || v === "video" || v === "gaming") {
      latencySelect.value = v;
    }
  } catch {
    // sin preferencia guardada, se queda el default del HTML ("music").
  }
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
      known.set(dev.id, dev);
      found = dev;
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
  render();
  await startPlay();
}

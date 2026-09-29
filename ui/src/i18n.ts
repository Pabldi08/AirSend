export type Lang = "es" | "en";

type Dict = Record<string, string>;

const ES: Dict = {
  audio_source: "Fuente de audio de Windows",
  audio_default: "Salida predeterminada",
  refresh: "Actualizar",
  audio_source_hint: "Captura las aplicaciones que ya reproducen en esta salida; no las redirige. Para cambiarla, para la reproducción.",
  mute_local: "Silenciar salida local (experimental)",
  mute_hint: "Espera a que llegue audio y comprueba que la captura continúa. Si se corta, restaura la salida. También la restaura al parar o fallar.",
  mute_off: "Salida local sin modificar",
  mute_waiting_signal: "Esperando audio para probar el silencio",
  mute_checking: "Comprobando captura con salida silenciada…",
  mute_muted: "Salida local silenciada; captura con señal",
  mute_unsupported: "El silencio no conserva la captura en esta salida. Se ha restaurado.",
  mute_user_changed: "Se respeta tu cambio manual de volumen o silencio",
  group_first: "Primer HomePod",
  stereo_pair: "Pareja estéreo (experimental)",
  group_second: "Segundo HomePod",
  mute_unavailable: "Silencio local disponible en Windows",
  group_summary: "Parejas y sincronización (experimental)",
  stereo_pairs: "Mostrar parejas configuradas en Apple Home",
  group_hint: "Prueba la reproducción simultánea en dos HomePods. Si se pierde un miembro, se para todo el grupo. Pendiente de validar con dispositivos reales.",
  group_play: "Probar reproducción sincronizada",
  incomplete_pair: "Pareja incompleta",
  auto_reconnect: "Reconectar si falla este receptor (hasta 3 intentos)",
  check_updates: "Buscar actualizaciones",
  volume_unreadable: "Lectura del receptor no disponible; ajuste solicitado",
  feedback_failed: "El receptor dejó de responder",
  transport_failed: "Falló el envío de audio",
  receiver_unavailable: "El receptor ya no está disponible",
  session_cancelled: "Operación cancelada",
  session_recovering: "Reconectando…",
  stop_before_source_change: "Para la reproducción antes de cambiar la fuente",
  group_requires_two_receivers: "Selecciona dos HomePods distintos",
  stop_before_group_change: "Para el grupo antes de cambiar sus miembros",

  subtitle: "Envía el audio de Windows a tu HomePod",
  scan: "Buscar dispositivos",
  scan_searching: "buscando…",
  devices_count_one: "1 dispositivo",
  devices_count_other: "{n} dispositivos",
  connect: "Conectar",
  connecting: "Conectando…",
  disconnect: "Desconectar",
  play_generic: "▶ Reproducir audio del PC",
  play_to: "▶ Enviar audio del PC a {name}",
  stop: "⏸ Parar",
  player_starting: "iniciando…",
  player_playing: "reproduciendo",
  multi_device: "Reproducir en varios dispositivos",
  multi_device_hint: "Experimental: al conectar otro receptor durante la reproducción, se añadirá al audio actual. Los receptores pueden no estar perfectamente sincronizados.",
  switch_title: "Cambiar dispositivo de audio",
  switch_message: "El audio se está enviando a {from}. Con varios dispositivos desactivado, conectar {to} cambiará la reproducción y desconectará {from}.",
  switch_confirm: "Cambiar a {name}",
  multi_off_title: "Desactivar varios dispositivos",
  multi_off_message: "Se detendrá la reproducción en los demás dispositivos y continuará en {name}.",
  multi_off_confirm: "Continuar solo en {name}",
  cancel: "Cancelar",
  local_buffer: "Búfer local reducido (experimental)",
  local_buffer_hint: "Puede reducir el retardo, pero también causar cortes. Para cambiarlo, para la reproducción. El HomePod puede añadir su propio retardo.",
  local_buffer_error: "No se pudo cambiar el búfer local: {err}",
  local_buffer_stop: "Para la reproducción antes de cambiar el búfer local.",
  diagnostics_summary: "Diagnóstico de audio",
  diagnostics_hint: "Descarga las métricas de las últimas sesiones para comparar retardo y cortes. No incluye nombres, direcciones IP ni credenciales. Las medidas locales no representan el retardo audible del HomePod.",
  diagnostics_export: "Descargar diagnóstico",
  diagnostics_ready: "Diagnóstico preparado",
  diagnostics_error: "No se pudo exportar el diagnóstico: {err}",
  volume: "Volumen",
  latency: "Límite de búfer solicitado",
  latency_lower: "Menos retardo",
  latency_safer: "Más estabilidad",
  latency_hint:
    "Confirma el cambio para aplicarlo. Solo puedes cambiarlo una vez cada 10 segundos. Si se está reproduciendo audio, la conexión se reinicia brevemente. 0 ms solicita ningún búfer adicional, pero es experimental: puede fallar o causar cortes. El receptor puede añadir más retardo.",
  latency_confirm: "Confirmar",
  latency_current: "Valor aplicado",
  latency_pending: "Pendiente de confirmar",
  latency_applying: "Aplicando…",
  latency_cooldown: "Disponible en {seconds} s",
  latency_error: "No se pudo aplicar la latencia: {err}",
  capture_interrupted: "La captura de audio se interrumpió (dispositivo desconectado o fallo del controlador)",
  manual_summary: "¿No aparece tu HomePod? Añade su IP y puerto manualmente",
  manual_hint:
    "Puedes usar una IP o IP:puerto, por ejemplo 192.168.1.50:7453. Para IPv6 con puerto usa [dirección]:puerto.",
  manual_endpoint_placeholder: "192.168.1.50[:7000]",
  manual_name_placeholder: "Nombre (opcional)",
  manual_add: "Añadir",
  manual_checking: "verificando…",
  manual_ok: "OK: {name}",
  manual_need_ip: "introduce una IP",
  reconnecting: "reconectando a {name}…",
  cant_find: "no encuentro {name}: {err}",
  error_prefix: "error: {err}",
  vol_error_prefix: "vol err: {err}",
  lang_toggle_to_en: "EN",
  lang_toggle_to_es: "ES",
  lang_toggle_title: "Cambiar idioma",
};

const EN: Dict = {
  audio_source: "Windows audio source",
  audio_default: "Default output",
  refresh: "Refresh",
  audio_source_hint: "Captures apps already playing on this output; it does not redirect them. Stop playback before changing it.",
  mute_local: "Mute local output (experimental)",
  mute_hint: "Waits for audio and checks that capture keeps its signal. Restores the output if capture stops, and when playback stops or fails.",
  mute_off: "Local output unchanged",
  mute_waiting_signal: "Waiting for audio to test mute",
  mute_checking: "Checking capture while output is muted…",
  mute_muted: "Local output muted; capture has signal",
  mute_unsupported: "Mute does not preserve capture on this output. It has been restored.",
  mute_user_changed: "Your manual volume or mute change is respected",
  group_first: "First HomePod",
  stereo_pair: "Stereo pair (experimental)",
  group_second: "Second HomePod",
  mute_unavailable: "Local mute is available on Windows",
  group_summary: "Pairs and synchronization (experimental)",
  stereo_pairs: "Show pairs configured in Apple Home",
  group_hint: "Try simultaneous playback on two HomePods. Losing a member stops the whole group. Validation with real devices is pending.",
  group_play: "Test synchronized playback",
  incomplete_pair: "Incomplete pair",
  auto_reconnect: "Reconnect this receiver after failure (up to 3 attempts)",
  check_updates: "Check for updates",
  volume_unreadable: "Receiver reading unavailable; requested setting",
  feedback_failed: "The receiver stopped responding",
  transport_failed: "Audio transport failed",
  receiver_unavailable: "The receiver is no longer available",
  session_cancelled: "Operation cancelled",
  session_recovering: "Reconnecting…",
  stop_before_source_change: "Stop playback before changing the source",
  group_requires_two_receivers: "Select two different HomePods",
  stop_before_group_change: "Stop the group before changing its members",

  subtitle: "Send your Windows audio to your HomePod",
  scan: "Scan devices",
  scan_searching: "scanning…",
  devices_count_one: "1 device",
  devices_count_other: "{n} devices",
  connect: "Connect",
  connecting: "Connecting…",
  disconnect: "Disconnect",
  play_generic: "▶ Play PC audio",
  play_to: "▶ Send PC audio to {name}",
  stop: "⏸ Stop",
  player_starting: "starting…",
  player_playing: "playing",
  multi_device: "Play on multiple devices",
  multi_device_hint: "Experimental: connecting another receiver during playback adds it to the current audio. Receivers may not be perfectly synchronized.",
  switch_title: "Switch audio device",
  switch_message: "Audio is playing on {from}. With multiple devices off, connecting {to} will switch playback and disconnect {from}.",
  switch_confirm: "Switch to {name}",
  multi_off_title: "Turn off multiple devices",
  multi_off_message: "Playback will stop on the other devices and continue on {name}.",
  multi_off_confirm: "Keep only {name}",
  cancel: "Cancel",
  local_buffer: "Reduced local buffer (experimental)",
  local_buffer_hint: "May reduce delay, but may also cause dropouts. Stop playback before changing it. The HomePod can add its own delay.",
  local_buffer_error: "Could not change the local buffer: {err}",
  local_buffer_stop: "Stop playback before changing the local buffer.",
  diagnostics_summary: "Audio diagnostics",
  diagnostics_hint: "Download recent session metrics to compare delay and dropouts. Device names, IP addresses and credentials are excluded. Local measurements do not represent audible HomePod delay.",
  diagnostics_export: "Download diagnostics",
  diagnostics_ready: "Diagnostics prepared",
  diagnostics_error: "Could not export diagnostics: {err}",
  volume: "Volume",
  latency: "Requested buffer limit",
  latency_lower: "Less delay",
  latency_safer: "More stable",
  latency_hint:
    "Confirm to apply a change. You can change it only once every 10 seconds. If audio is playing, the connection briefly restarts. 0 ms requests no extra buffer, but is experimental: it may fail or stutter. The receiver may add more delay.",
  latency_confirm: "Confirm",
  latency_current: "Applied value",
  latency_pending: "Awaiting confirmation",
  latency_applying: "Applying…",
  latency_cooldown: "Available in {seconds} s",
  latency_error: "Could not apply latency: {err}",
  capture_interrupted: "Audio capture stopped (device disconnected or driver failed)",
  manual_summary: "Can't see your HomePod? Add its IP and port manually",
  manual_hint:
    "Enter an IP or IP:port, for example 192.168.1.50:7453. Use [address]:port for IPv6.",
  manual_endpoint_placeholder: "192.168.1.50[:7000]",
  manual_name_placeholder: "Name (optional)",
  manual_add: "Add",
  manual_checking: "checking…",
  manual_ok: "OK: {name}",
  manual_need_ip: "enter an IP",
  reconnecting: "reconnecting to {name}…",
  cant_find: "can't find {name}: {err}",
  error_prefix: "error: {err}",
  vol_error_prefix: "vol err: {err}",
  lang_toggle_to_en: "EN",
  lang_toggle_to_es: "ES",
  lang_toggle_title: "Change language",
};

const DICTS: Record<Lang, Dict> = { es: ES, en: EN };

const STORAGE_KEY = "airsend.lang";

let current: Lang = detectInitial();

function detectInitial(): Lang {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "es" || saved === "en") return saved;
  } catch {
    // localStorage no disponible (no debería pasar en webview Tauri).
  }
  const nav = (navigator.language || "es").toLowerCase();
  return nav.startsWith("es") ? "es" : "en";
}

export function getLang(): Lang {
  return current;
}

export function t(key: string, params?: Record<string, string | number>): string {
  const dict = DICTS[current];
  let s = dict[key] ?? DICTS.es[key] ?? key;
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      s = s.replaceAll(`{${k}}`, String(v));
    }
  }
  return s;
}

type Listener = (lang: Lang) => void;
const listeners = new Set<Listener>();

export function onLangChange(cb: Listener): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function setLang(lang: Lang): void {
  if (lang === current) return;
  current = lang;
  try {
    localStorage.setItem(STORAGE_KEY, lang);
  } catch {
    // ignore
  }
  document.documentElement.lang = lang;
  applyStaticTranslations();
  listeners.forEach((cb) => cb(lang));
}

export function toggleLang(): void {
  setLang(current === "es" ? "en" : "es");
}

// Aplica traducciones a todos los nodos con data-i18n / data-i18n-attr.
// data-i18n="key"           -> reemplaza textContent
// data-i18n-attr="attr:key" -> reemplaza el atributo (p.ej. "placeholder:manual_name_placeholder")
export function applyStaticTranslations(): void {
  document.documentElement.lang = current;
  document.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    const key = el.dataset.i18n;
    if (key) el.textContent = t(key);
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-attr]").forEach((el) => {
    const spec = el.dataset.i18nAttr;
    if (!spec) return;
    for (const pair of spec.split(",")) {
      const [attr, key] = pair.split(":").map((s: string) => s.trim());
      if (attr && key) el.setAttribute(attr, t(key));
    }
  });
}

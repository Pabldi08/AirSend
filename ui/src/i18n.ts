export type Lang = "es" | "en";

type Dict = Record<string, string>;

const ES: Dict = {
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

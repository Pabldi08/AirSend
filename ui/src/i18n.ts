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
  latency: "Latencia",
  latency_music: "Música (segura)",
  latency_video: "Vídeo",
  latency_gaming: "Juegos (mínima)",
  latency_hint:
    "Menos latencia es más reactivo, pero sobre Wi-Fi flojo el audio puede entrecortarse. Si oyes cortes, sube a Música.",
  manual_summary: "¿No aparece tu HomePod? Añade su IP manualmente",
  manual_hint:
    "Algunos routers (Movistar HGU, redes con VLANs) no propagan mDNS entre wifi 2.4 y 5 GHz. Mira la IP del HomePod en la app Casa del iPhone.",
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
  latency: "Latency",
  latency_music: "Music (safe)",
  latency_video: "Video",
  latency_gaming: "Gaming (lowest)",
  latency_hint:
    "Lower latency is more responsive, but on weak Wi-Fi the audio may stutter. If you hear drops, switch back to Music.",
  manual_summary: "Can't see your HomePod? Add its IP manually",
  manual_hint:
    "Some routers (Movistar HGU, VLAN-segmented networks) don't propagate mDNS between 2.4 and 5 GHz Wi-Fi. Check the HomePod's IP in the iPhone Home app.",
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

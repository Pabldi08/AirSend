# AirSend
<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.es.md">Español</a>
</p>


App de escritorio para Windows que captura el audio del sistema y lo envía a un HomePod por AirPlay 2 con latencia mínima.

> ⚠️ **Estado**: pre-alfa.



## Instalación

Descarga la última versión estable en:

<a href="https://github.com/Pabldi08/AirSend/releases/latest">
  <img src="https://img.shields.io/badge/Download-Latest%20Release-blue?style=for-the-badge" alt="Descargar última versión">
</a>

> Aviso de SmartScreen: al no firmar con certificado EV de código (≈200 €/año), la primera ejecución mostrará un aviso. Workaround: "Más información" → "Ejecutar igualmente". Considerar firma EV en post-MVP.

## Comparativa con alternativas

Esta tabla resume las diferencias principales entre AirSend y algunas alternativas populares.
AirSend prioriza ser gratuito, auditable y compatible con HomePod mediante AirPlay 2, aunque todavía es un proyecto joven y no pretende cubrir funciones como mirroring de pantalla o multi-destino.

| Característica | AirSend | TuneBlade | AirParrot |
|---|---|---|---|
| Plataforma | Windows | Windows | Windows + macOS |
| Licencia / precio | GPL-2.0, gratis | Freeware, closed-source | Comercial (~15 €) |
| AirPlay 2 con HomePod | ✅ | ⚠️ Inestable / no pairea en muchos casos | ✅ |
| Pair-setup transient + verify | ✅ | ❌, sólo AirPlay 1 fiable | ✅ |
| Codec | ALAC + ChaCha20-Poly1305 | ALAC | ALAC |
| Captura | WASAPI loopback, audio del sistema | WASAPI loopback | WASAPI loopback + cherry-pick de apps |
| Tray + close-to-tray | ✅ | ✅ | ✅ |
| Auto-reconnect al último device | ✅ | ✅ | ✅ |
| IP manual si mDNS falla | ✅ | ❌ | ⚠️ Limitado |
| Latencia HomePod estable | 500 ms – 3 s configurable | Variable | Alta, desync ocasional |
| Jitter Windows en release | ✅ MMCSS Pro Audio + HIGH_PRIORITY | ❌ | ✅ |
| Desarrollo activo | ✅ | ❌, parado desde hace años | ✅ |
| Código auditable | ✅ | ❌ | ❌ |
| Probado en hardware | ⚠️ 1 setup (HomePod gen 2) | ✅ Años, mucho hw | ✅ Años, mucho hw |

## Estado del proyecto

AirSend está en una fase inicial de desarrollo. La versión actual ya permite enviar audio del sistema desde Windows a un HomePod mediante AirPlay 2, pero todavía no pretende sustituir completamente a soluciones comerciales más maduras.

El objetivo principal del proyecto es ofrecer una alternativa gratuita, abierta y auditable para usuarios que quieran enviar audio desde Windows a dispositivos AirPlay compatibles.

## Roadmap

### Versión actual: 0.1.4

- [x] Captura de audio del sistema con WASAPI loopback
- [x] Envío AirPlay 2 (ALAC + ChaCha20-Poly1305 + RTSP/RTP)
- [x] Pair-setup transient + pair-verify contra HomePod
- [x] Heartbeat RTSP para mantener la sesión viva (~10 s sin él el HomePod la cierra)
- [x] Descubrimiento mDNS con cache replay al re-escanear
- [x] Añadir dispositivo por IP manual (redes con mDNS roto: Movistar HGU, VLANs)
- [x] Reconexión automática al último dispositivo al arrancar
- [x] Control de volumen persistente entre sesiones
- [x] Perfiles de latencia seleccionables (Música / Vídeo / Gaming), persistentes entre sesiones
- [x] Bandeja del sistema + cerrar al tray (la app sigue activa de fondo)
- [x] Logs rotados por día + toast de errores en la UI
- [x] Auto-update firmado con minisign vía GitHub Releases
- [x] Jitter Windows resuelto: `HIGH_PRIORITY_CLASS` + MMCSS "Pro Audio" + `THREAD_PRIORITY_TIME_CRITICAL`

### Próximas mejoras

- [ ] Release notes automáticas desde `git log` en el workflow
- [ ] Tests unitarios en `crates/airplay-core` (probe parser, stream config builder, smoke del pump loop)
- [ ] Probar en más hardware (HomePod gen 1, mini, Apple TV, AirPort Express)
- [ ] Documentación técnica de AirPlay 2 y pairing en `/docs`
- [ ] Cert EV de code signing para eliminar el aviso de SmartScreen (~200 €/año, decisión económica)

### Futuro

- [ ] Multi-destino simultáneo (group streaming a varios HomePods)
- [ ] Selección de audio por aplicación (WASAPI process-loopback, Win10 1903+)
- [ ] PR upstream a `lmcgartland/airplay2-rs` con los dos patches Windows
- [ ] Soporte estable para otros receptores AirPlay 2 (Apple TV, AirPort Express, altavoces de terceros)

## Stack

- **Rust** + **Tauri v2** (UI con webview nativo de Windows, sin Electron)
- **mdns-sd** para descubrir dispositivos AirPlay en la red local
- **WASAPI loopback** para capturar el audio del sistema (sólo Windows)
- **ALAC** + **RTP cifrado** (ChaCha20-Poly1305) para enviar al HomePod
- Instalador **NSIS** y auto-updater firmado con **minisign** vía **GitHub Releases**

## Contribuir

Las instrucciones de release y notas para mantenedores están en
[CONTRIBUTING.md](CONTRIBUTING.md).

## Licencia

GPL-2.0 (compatible con `airplay2-rs`, usado como base).

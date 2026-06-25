# AirSend
<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.es.md">Español</a>
</p>


Windows desktop app that captures system audio and sends it to a HomePod over AirPlay 2 with low latency.

> ⚠️ **Status**: pre-alpha.



## Installation

Download the latest release:

<a href="https://github.com/Pabldi08/AirSend/releases/latest">
  <img src="https://img.shields.io/badge/Download-Latest%20Release-blue?style=for-the-badge" alt="Download latest release">
</a>

> SmartScreen warning: the app is not signed with an EV code-signing
> certificate (~€200/year), so the first run shows a warning.
> Workaround: "More info" → "Run anyway". EV signing is under
> consideration post-MVP.

## Comparison with alternatives

This table summarizes the main differences between AirSend and some
popular alternatives. AirSend prioritizes being free, auditable and
compatible with HomePod over AirPlay 2 — it is still a young project
and does not aim to cover screen mirroring or multi-target streaming.

| Feature | AirSend | TuneBlade | AirParrot |
|---|---|---|---|
| Platform | Windows | Windows | Windows + macOS |
| License / price | GPL-2.0, free | Freeware, closed-source | Commercial (~€15) |
| AirPlay 2 with HomePod | ✅ | ⚠️ Unstable / often fails to pair | ✅ |
| Pair-setup transient + verify | ✅ | ❌, reliable only on AirPlay 1 | ✅ |
| Codec | ALAC + ChaCha20-Poly1305 | ALAC | ALAC |
| Capture | WASAPI loopback (system audio) | WASAPI loopback | WASAPI loopback + per-app picker |
| Tray + close-to-tray | ✅ | ✅ | ✅ |
| Auto-reconnect to last device | ✅ | ✅ | ✅ |
| Manual IP when mDNS fails | ✅ | ❌ | ⚠️ Limited |
| Stable HomePod latency | 500 ms – 3 s, configurable | Variable | High, occasional desync |
| Windows jitter in release builds | ✅ MMCSS Pro Audio + HIGH_PRIORITY | ❌ | ✅ |
| Active development | ✅ | ❌, stalled for years | ✅ |
| Auditable source | ✅ | ❌ | ❌ |
| Tested on hardware | ⚠️ 1 setup (HomePod gen 2) | ✅ Years, broad hw | ✅ Years, broad hw |

## Project status

AirSend is in an early stage. The current version already streams system
audio from Windows to a HomePod over AirPlay 2, but does not yet aim to
fully replace more mature commercial solutions.

The main goal of the project is to offer a free, open and auditable
alternative for users who want to stream audio from Windows to
AirPlay-compatible devices.

## Roadmap

### Current version: 0.1.4

- [x] System audio capture via WASAPI loopback
- [x] AirPlay 2 streaming (ALAC + ChaCha20-Poly1305 + RTSP/RTP)
- [x] HomePod pair-setup transient + pair-verify
- [x] RTSP heartbeat to keep the session alive (~10 s without it, the HomePod drops it)
- [x] mDNS discovery with cache replay on rescan
- [x] Add device by manual IP (networks with broken mDNS: Movistar HGU routers, VLANs)
- [x] Auto-reconnect to the last device on startup
- [x] Persistent volume across sessions
- [x] Selectable latency profiles (Music / Video / Gaming), persisted across sessions
- [x] System tray + close-to-tray (app keeps running in the background)
- [x] Daily-rotated log files + UI toast for async errors
- [x] Signed auto-update via minisign + GitHub Releases
- [x] Windows jitter fix: `HIGH_PRIORITY_CLASS` + MMCSS "Pro Audio" + `THREAD_PRIORITY_TIME_CRITICAL`

### Next improvements

- [ ] Auto-generated release notes from `git log` in the workflow
- [ ] Minimal unit tests in `crates/airplay-core` (probe parser, stream config builder, pump-loop smoke)
- [ ] Validation on more hardware (HomePod gen 1, mini, Apple TV, AirPort Express)
- [ ] Technical docs on AirPlay 2 and pairing under `/docs`
- [ ] EV code-signing certificate to drop the SmartScreen warning (~€200/year, budget decision)

### Future

- [ ] Simultaneous multi-target (group streaming to multiple HomePods)
- [ ] Per-application audio capture (WASAPI process-loopback, Win10 1903+)
- [ ] Upstream PR to `lmcgartland/airplay2-rs` with the two Windows patches
- [ ] Stable support for other AirPlay 2 receivers (Apple TV, AirPort Express, third-party speakers)

## Stack

- **Rust** + **Tauri v2** (native Windows webview UI, no Electron)
- **mdns-sd** for AirPlay device discovery on the local network
- **WASAPI loopback** for system audio capture (Windows only)
- **ALAC** + **encrypted RTP** (ChaCha20-Poly1305) for HomePod streaming
- **NSIS** installer and signed auto-updater (minisign) via **GitHub Releases**

## Contributing

Release instructions and maintainer notes live in
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-2.0 (compatible with `airplay2-rs`, used as upstream base).

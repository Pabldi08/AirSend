<h1 align="center">AirSend</h1>

<p align="center">Send audio from your Windows PC to a HomePod over AirPlay 2.</p>

<p align="center">
  <a href="https://github.com/Pabldi08/AirSend/releases/latest"><img src="https://img.shields.io/badge/Download-Windows%20x64-blue?style=for-the-badge" alt="Download AirSend for Windows x64"></a>
</p>

<p align="center"><a href="README.md">English</a> · <a href="README.es.md">Español</a></p>

## Current release

**AirSend 0.3.0** includes the native WinUI interface and a Windows x64 installer. AirSend is free and open source.

1. Download and run the **Setup-x64.exe** installer from [GitHub Releases](https://github.com/Pabldi08/AirSend/releases/latest). If AirSend is already running, quit it from the system tray first.
2. Open AirSend, choose your HomePod and select **Send PC audio**.

## What you can do

| Feature | How it works |
| --- | --- |
| System or app audio | Send the entire system mix or select an app on Windows 11, such as Chrome or Spotify. |
| Speaker discovery | Find speakers on your local network or enter an IP address manually. |
| Synchronized playback | Use a HomePod stereo pair or two compatible HomePods with a shared PTP clock. |
| Playback controls | Adjust volume and the requested receiver buffer from **100 to 3,000 ms**. The audible delay also depends on the receiver and network. |
| Local mute and standby | Mute local playback where supported. Optionally release the speaker after sustained silence and resume when you choose. |
| System tray | Close the window to keep listening, reopen it from the tray, or stop playback from the tray menu. |
| Diagnostics | Review audio issues and retransmissions, mark audible distortion and export a report without speaker names, network addresses or credentials. |

The Windows app uses **WinUI 3**, a **Rust** engine and **WASAPI** capture. For build instructions and hardware checks, see the [Windows developer guide](native/README.md).

## Compare AirSend, TuneBlade and AirParrot

| Feature | AirSend | TuneBlade | AirParrot 3 |
| --- | --- | --- | --- |
| Sender platform | Windows x64 | Windows | Windows and macOS |
| Audio source | Entire system or one app on Windows 11 | System-wide audio | System audio, media files and single-app mirroring on Windows |
| HomePod | AirPlay 2 audio, with a shared PTP clock for compatible pairs | HomePod firmware compatibility unverified here | HomePod audio documented for Windows and macOS |
| Buffer control | Requested receiver buffer of 100–3,000 ms | — | — |
| Troubleshooting | Recent audio issues, retransmissions and exportable distortion markers | — | — |
| Screen mirroring | Audio only | Audio streaming | Screen and app mirroring |

The dashes indicate features that were not verified for this comparison. Sources: [TuneBlade integration documentation](https://github.com/spycle/tuneblade/blob/main/README.md), [AirParrot 3 feature overview](https://blog.airsquirrels.com/introducing-airparrot-3-the-most-powerful-screen-mirroring-and-media-streaming-software-yet), and [AirParrot for Windows release notes](https://www.airsquirrels.com/airparrot/release-notes/win).

## Report a problem

[Open a GitHub issue](https://github.com/Pabldi08/AirSend/issues/new) with your AirSend version, Windows version, speaker model and software version, and steps to reproduce the issue. If you hear distortion, use **Mark distortion** when it happens and attach the exported report from **Diagnostics**.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for maintainer notes.

## License

AirSend is free software licensed under the
[GNU General Public License v3.0 or later](LICENSE).

The vendored `airplay2-rs` snapshot uses the same SPDX identifier,
`GPL-3.0-or-later`. Its exact revision, fork ancestry, and licensing basis
are recorded in [PROVENANCE.json](vendor/airplay2-rs/PROVENANCE.json).

The project incorporates and modifies code from
[`lmcgartland/airplay2-rs`](https://github.com/lmcgartland/airplay2-rs),
which is also licensed under GPL-3.0-or-later. See the `LICENSE` file for
the complete terms.

## Support the project

If you find AirSend useful, you can support the project here:

<a href="https://ko-fi.com/E3E226XAYR" target="_blank" rel="noopener noreferrer">
  <img
    src="https://storage.ko-fi.com/cdn/kofi3.png?v=6"
    alt="Buy Me a Coffee"
    width="140"
    height="36"
  />
</a>

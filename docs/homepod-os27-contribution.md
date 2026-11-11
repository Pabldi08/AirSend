# HomePod OS 27 audio candidate (draft)

This audio-only draft includes upstream `main` at `05d2118`. A HomePod
running software 27.2 would connect but had excessive delay or crackling when
lower buffers were requested with the previous timing path. In the tested
Windows setup, automatic PTP timing and capture/timeline recovery allowed clean
playback with acceptable video sync at a **250 ms requested receiver buffer**.
That setting is not a measured end-to-end acoustic delay or a gaming latency
guarantee. Results need validation on other receivers and networks.

## Relationship to existing upstream work

Merged upstream [PR #26](https://github.com/Pabldi08/AirSend/pull/26) already
addresses HomePod OS 27 connection/timing and idle audio. This contribution
overlaps those fixes and adds WASAPI packet draining,
legacy timing override, timing diagnostics and further regression coverage.
Its automatic PTP choice is narrower (supported HomePod models on OS 27+)
than #26's feature-based selection for all PTP-capable receivers. The maintainer
should reconcile those policies and dependency changes rather than merge both
implementations blindly.

Merged upstream [PR #27](https://github.com/Pabldi08/AirSend/pull/27) separately
removes the zero receiver-buffer option. The current draft preserves that migration for global and per-receiver settings
and rejects requests below 100 ms. The 50 ms step size still allows values such
as 150 and 250 ms; it does not allow a 50 ms requested buffer.

## Audio changes

- Select PTP for PTP-capable `AudioAccessory` receivers advertising software
  version 27 or newer. Older, unknown and other receivers retain NTP. A saved
  legacy-timing option allows single-speaker playback to force NTP.
- Retain receiver metadata needed for that choice. Diagnostics report the actual
  timing protocol rather than assuming NTP.
- Validate PTP exchanges by sequence, peer identity, domain, age and advancing
  timestamps; retain one-way clock tracking if two-way replies stop.
- Recover the RTP timeline after capture gaps rather than retaining a stale
  playback schedule.
- Drain all available WASAPI packets per notification, explicitly zero silent
  packets, and catch up idle silence after timer wakeups. Capture diagnostics
  include discontinuities, timestamp errors and observed frame rate.
- Allow 50 ms receiver-buffer steps and keep the requested minimum no greater
  than the maximum at small settings. The stable default and opt-in reduced
  local-buffer policy are retained.

The sender is vendored from
[DavidTIBO/airplay2-rs](https://github.com/DavidTIBO/airplay2-rs) revision
`9edef3287ae1ade1248ea4f18f1bc41a8780005b`, based on the project's existing
`Pabldi08/airplay2-rs` revision `dc66e2b`. Credit for the receiver compatibility
and initial PTP fixes belongs to DavidTIBO. Additional exchange validation and
RTP regression cases are included here. The original license and exact
provenance are retained in `vendor/airplay2-rs/PROVENANCE.json` and `LICENSE`.
The Unix socket-QoS guard and Windows sender priority support remain present.
Vendoring makes the tested combination reproducible; maintainers may prefer to
land those changes in their sender fork and restore a pinned Git dependency.

## Pending before audio review/merge

The maintainer requested the following in PR #28. The negotiation and licensing items remain open; incorporating the merged
minimum/migration fixes does not resolve them:

- Handle PTP negotiation failures explicitly. Do not leave a failed preparation
  looking like successful streaming; agree the fallback/error policy with #26.
- Preserve the merged upstream fixes during further timing work. The current
  branch includes #27's global/per-receiver zero migration, 100 ms minimum and
  pre-state-change validation. Regression checks cover these cases.
- Resolve the sender licensing discrepancy identified by the maintainer before
  distributing an installer with the updated dependency. Exact source and
  existing license files are preserved here; they are not a licensing conclusion.

Desktop navigation/layout and anonymized screenshots have been moved to a
separate PR [#29](https://github.com/Pabldi08/AirSend/pull/29) based on main. This candidate retains the original narrow
layout and only adds the legacy-timing control needed by its audio preference.

## Validation

The Windows alpha.6 candidate from which this contribution was prepared passed:

- 33 application/workspace Rust tests on Linux and Windows.
- 254 sender clock/audio Rust tests on Linux and Windows, with two existing
  ignored tests.
- Eight UI regression tests, TypeScript/Vite build and Chromium desktop smoke
  coverage for navigation, playback state, source locking, legacy timing,
  localization and sizing.
- Windows release build and NSIS installation, preserving existing preferences.
- Eleven microphone-calibration helper tests on Windows.

Listening tests on the HomePod 27.2 setup reported near sync with faint crackling
at 200 ms, then clean sound, acceptable sync and clean resume after pausing at
250 ms, with reduced local buffers enabled. Volume/mute worked with the existing
VB-CABLE route. No acoustic latency figure was established for the final profile.
Those hardware checks used the earlier combined alpha.6 candidate, with the
same audio backend and the wider layout. The separated audio branch reruns
application Rust and frontend unit/build checks; it does not include the
navigation smoke harness. The prior combined contribution passed all 33
application tests, 254 sender tests (two ignored), eight UI tests, the frontend
build and Chromium smoke checks on Linux.
GitHub CI results should be consulted on the PR before merging.

The optional `tools/audio-latency` microphone helper saves numerical results,
not recordings. Its README describes setup and measurement limitations.

## Limits and review choices

- This remains realtime ALAC/PTP. It does not implement Apple's newer responsive
  audio transport and does not promise zero latency or universal gaming sync.
- Windows output selection captures the chosen output; it does not install or
  bundle a virtual audio driver or automatically reroute applications. A working
  separately installed virtual cable can avoid simultaneous physical playback.
- Multi-receiver synchronization, older HomePods, Apple TV and third-party
  receivers still need hardware testing. The legacy override only applies to
  single-speaker playback.
- Repository release versions are left unchanged for the maintainer to choose.
  Independent AAC/OpenAir research and local settings/logs are excluded.
- Desktop navigation is submitted separately. The vendored sender accounts for
  most of the added source lines in this audio candidate.

After merging upstream `05d2118`, the separated audio branch passed 37 application
Rust tests, eight frontend tests and the production frontend build on Linux.
This includes upstream saved-zero migration, invalid-buffer-before-state-change
and compound receiver-group metadata regression cases. No new hardware run was
performed for this draft update.

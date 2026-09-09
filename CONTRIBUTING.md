# Contributing to AirSend

## Publishing a release

1. Bump the version in `src-tauri/tauri.conf.json` (`version`) and in
   `Cargo.toml` (`workspace.package.version`).
2. Commit and tag:

   ```sh
   git commit -am "release: bump version to 0.1.x"
   git tag v0.1.x
   git push origin main v0.1.x
   ```

3. GitHub Actions (`.github/workflows/release.yml`) builds on
   `windows-latest` and creates a **draft release** with the NSIS `.exe`.
4. Once the draft is validated, **publish it manually** from GitHub
   (`gh release edit vX.Y.Z --draft=false --latest`). Users install new
   versions by downloading and running the installer; the application does
   not check for or install updates automatically.

## Notes for maintainers

- The `airplay2-rs` dependency is pinned to a fork
  (`Pabldi08/airplay2-rs`) that gates `set_socket_qos` behind
  `cfg(unix)` and adds MMCSS "Pro Audio" + `THREAD_PRIORITY_TIME_CRITICAL`
  for the sender thread on Windows. Any rev bump must keep both patches.
- The product `identifier` (`com.pablodiaz.conexionairplay`) must stay
  stable — it drives the NSIS upgrade path on Windows and the location
  of the user store (`%APPDATA%/<identifier>/settings.json`).
- `productName` is "AirSend" (visible). The internal crate names still
  use `conexion-airplay` for historical reasons; renaming them is
  cosmetic and can break Cargo cache, so leave them alone unless there
  is a reason.

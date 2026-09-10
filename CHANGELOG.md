# Changelog

## 0.3.0 - 2026-09-10

### Fixed

- Release executables could open the Vite development URL at localhost. Plain `cargo build --release` omitted Tauri's `custom-protocol` feature, so the executable did not serve the embedded frontend. The feature is now enabled by default, and a release compile guard rejects builds that omit it.
- Dictation cleanup previously sent every observed vocabulary entry, including whole phrases from old dictations. The correction step could then apply text from dictation history to a new utterance. Cleanup now sends only matching manual or learned vocabulary entries, and it rejects expansions beyond the current transcript.
- Update checks now read signed updater assets from public GitHub releases without requiring a GitHub token. Private release sources may still use one.

### Release packaging

- The signed build verifies the production executable and its updater trust key before checking the installer signature.
- Release scripts select the installer for the configured version and write its exact Tauri signature to `latest.json` beside the NSIS package.

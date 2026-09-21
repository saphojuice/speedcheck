# Changelog

All notable changes to the probe and the install scripts. Dates are the tag date.

The version numbers refer to `sj-probe`. The browser probe in `browser/fit.js` ships with the
website and is not versioned separately.

---

## v0.1.1 — 21 September 2026

### Fixed

- **Windows recorded the wrong power mode on every receipt.** Windows 11 has two power layers and
  they disagree. `powercfg /getactivescheme` reads the legacy plan, which says "Balanced" even
  when the Settings power mode slider is on Best performance. The mode is read from
  `ActiveOverlayAcPowerScheme` in the registry instead, because `powercfg` has no working flag
  for it on current builds. A machine set to Best performance now reports Best performance.
- **The same machine measured 20.3 GB/s and 34.6 GB/s an hour apart.** The first bandwidth pass
  after an idle period runs before the CPU has boosted and while the pages are still cold, and
  reads roughly a third low. Each thread count now does a discarded warm-up pass followed by the
  best of three. On one machine this took the run-to-run spread from 56% to 2%.
- **A result submitted from the command line could not be deleted.** The probe printed the share
  link but discarded the delete token, and `class_hash` is correctly refused as proof of
  ownership, so there was no way to remove it. The token is now printed once, with the exact
  command to use it.
- Nothing is written to disk unless `--out` is passed. It previously dropped
  `sj_receipt.json` into whatever directory you happened to be in, which contradicted the promise
  that nothing is left behind.

### Added

- Every receipt records the conditions it was measured under: power source, power mode, and CPU
  load at the start. Two results cannot be compared without them.
- The output states those conditions, and says plainly when a run is not comparable.
- `--no-share` to measure and send nothing, and `--show-payload` to print the exact JSON that
  would be sent before being asked.

### Changed

- The assumed efficiency factor `k` moved from 0.6 to 0.8, matching the browser test and the
  share pages. The same machine was being told ~13.7 tok/s by the command line and 18 by its own
  share page. 0.8 is also closer to the 0.83 measured on the reference machine.
- Windows builds use SChannel through `native-tls` rather than rustls, so the build needs only
  MSVC. rustls pulls in `ring`, whose aarch64 Windows assembly requires clang.
- Consent wording matches the website, and end of input is no longer treated as consent: a run
  with no terminal attached sends nothing.

---

## v0.1.0 — 21 September 2026

First release. Five platforms, built in public CI from this repository.

### Added

- `sj-probe`: measures memory bandwidth per working-set tier and per thread count, f32 GEMM and
  int8 throughput, a sustained throttle curve, storage, and the CPU and instruction-set
  inventory. Predicts decode speed for a fixed model catalogue from those measurements.
- Install scripts for macOS, Linux and Windows that download the binary for the platform, verify
  its SHA-256 against the published `SHA256SUMS`, run it, and delete it. Nothing is installed.
- Release workflow building `macos-arm64`, `macos-x64`, `windows-x64`, `windows-arm64` and
  `linux-x64`, publishing the binaries with `SHA256SUMS`.

### Known limitations

- **tok/s is predicted, not measured.** No inference engine is bundled, so the figures come from
  `k x bandwidth / bytes per token` with `k` assumed. A measured `k` needs a real generation run,
  which is the largest planned change.
- No `linux-arm64` build.
- The predictions have been checked against real generation on a small number of machines. That
  is what [CONTRIBUTING.md](CONTRIBUTING.md) asks for help with.

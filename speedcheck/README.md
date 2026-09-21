# sj-probe v0.1

Measures the physical constants that decide local LLM speed on a machine, then predicts decode tok/s for a reference model table. Zero external crates. One small binary.

What it measures:
1. Memory bandwidth per tier (STREAM triad across working sets from 192 KB to 1.5 GB), multi and single thread
2. f32 GEMM throughput and int8 dot-product throughput
3. Sustained bandwidth and compute over time (throttle curve)
4. Storage sequential write/read
5. Inventory: CPU, ISA features (dotprod, i8mm, sve, avx512...), RAM modules, GPU, power plan, battery

Then, unless `--offline` is passed: downloads the pinned reference model (Qwen3-0.6B Q4_K_M, see `manifest.json`) to a temp dir, verifies its SHA-256, and runs a ~30 s generation against it with a prebuilt llama.cpp engine if one is present (see "Calibrate with a live run" below). Finally it asks before sending anything to the public table.

Output: a human summary on stdout and `sj_receipt.json` (receipt v0.1).

## Build on the Surface (Windows on ARM)

1. Install Rust: https://rustup.rs (pick the ARM64 installer; `aarch64-pc-windows-msvc` is a Tier 1 target).
   Rustup will ask for Visual Studio Build Tools; install "Desktop development with C++" with the ARM64 build tools selected.
2. Set Windows power plan to Best Performance and plug in. Snapdragon machines clock down hard otherwise.
3. In this folder:

```
cargo build --release
.\target\release\sj-probe.exe
```

`.cargo/config.toml` sets `target-cpu=native`, so the binary uses dotprod/i8mm on ARM and AVX2/AVX512 on x86.

Flags: `--quick` (short run, ~30 s), `--bw-seconds 60`, `--gemm-seconds 20`, `--skip-storage`, `--offline` (skip the model download and generation run), `--out file.json`.

Building requires the ARM64 MSVC target and toolset (`aarch64-pc-windows-msvc`), plus `clang.exe` on PATH — `ring` (rustls's crypto backend) needs clang specifically to assemble its ARM64 code; MSVC's own `cl.exe`/`armasm64.exe` can't do it. `x86_64-pc-windows-msvc` only needs the ordinary x64 MSVC toolset (no clang required there).

macOS / Linux: same `cargo build --release`.

## Calibrate with a live run

By default (no `--offline`), after the physical measurements the probe:

1. Downloads the model pinned in `manifest.json` (Hugging Face URL pinned to a specific revision, plus its SHA-256) to a temp dir, verifying the hash before use.
2. Looks for a prebuilt `llama-bench` at `engines/<target>/llama-bench(.exe)` next to the binary (or under `speedcheck/engines/<target>/` in this checkout), where `<target>` is a Rust target triple, e.g. `aarch64-pc-windows-msvc`. If none is found, it says so and skips straight to the reference-model fit list already printed above.
3. If found, runs a short sizing pass then a ~30 s generation, and reports measured tok/s and `k = measured ÷ (bandwidth ÷ model_GB)`. This is written into `sj_receipt.json` under `"calibration"`, and `derived.k_used`/`k_calibrated` switch to the measured value.
4. Asks `Add this anonymous result to the public table? (y/N)`; on `y`, POSTs a receipt (class hash + machine constants + this run's numbers, no prompts/files/names) to `https://saphojuice.com/v1/receipt`.

To build your own engine: build llama.cpp for the target ISA and place `llama-bench(.exe)` at `speedcheck/engines/<target>/`. The temp download directory is removed when the probe exits.

You can still calibrate manually against a different model:

```
llama-bench -m qwen3-1.7b-q4_k_m.gguf -t <physical cores> -p 512 -n 128
sj-probe k --tokps <tg128> --model-gb <size> --bw <dram_sustained_gbps from receipt>
```

k is this machine's efficiency factor. Expect 0.4 to 0.6 on small models and up to 0.9 on large ones. Under 0.3 means a misconfigured build, thread count, power plan, or RAM spill.

## Notes

- Prediction uses k=0.8 until calibrated, the same value the browser test and the share pages
  assume, so one machine gets one answer whichever way you measure it. The receipt marks
  `k_calibrated: false` until a live generation run measures the real value.
- Storage read may be page-cached; treat it as an upper bound.
- No prompts, files, or names are collected. Hostname is stored as a hash only. The public-table submission is opt-in per run.

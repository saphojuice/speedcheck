# Why SAPHOJUICE Speedtest?

I had a spare ThinkPad X13s sitting around. Snapdragon, 16 GB. It's been a workhorse.

I wanted to run a local AI model on it, or at least figure out what it could actually run before downloading a bunch of multi-gigabyte models.

That turned out to be much harder than it should be.

LM Studio terminated. The official llama.cpp Windows ARM64 build crashed the moment it tried to compute because the chip lacks an instruction the build assumed was there. Ollama ran, at about five tokens a second, but told me nothing about why.

Three tools, and none could answer the question I had from the beginning:

What AI can this machine actually run, and how fast? So I stopped guessing and measured it.

Memory bandwidth, compute, free memory, then a real model generating real tokens. The answer turned out to be one line of arithmetic: a model reads its whole weight file to produce each token, so speed is your memory bandwidth divided by the bytes it has to read. Predicted 24, 13 and 6 tokens a second for three model sizes. Measured 21.3, 9.6 and 5.5. Within fifteen percent across a fourfold range, on the machine everything else failed on.

That is the whole idea. Answer the question honestly, before the download, with a number from your own hardware instead of somebody's guess. This repo is the measurement half, public so anyone can check the numbers or tell me I'm wrong.

SAPHOJUICE. Set your mind in motion.

## The physics

Two equations. That is the entire model.

**Speed.** Generating one token reads the model's weights once, so decode rate is bandwidth divided by bytes read:

```
tok/s = k x bandwidth / active bytes per token
```

`k` is the machine's efficiency factor, the fraction of theoretical bandwidth the inference engine actually reaches. It is measured, not assumed. For a dense model, active bytes per token is the weight file size. For a mixture of experts it is the active parameters, which is why a 30B MoE can outrun a 14B dense model.

**Fit.** A model runs only if it and its working memory have somewhere to live:

```
file + KV(context) + reserve <= free memory
```

Free memory, not installed memory. That distinction is most of the story on Windows.

### Measured on a ThinkPad X13s Gen 1

| Quantity | Measured |
|---|---|
| Memory bandwidth, single thread | 33 GB/s |
| Memory bandwidth, 8 threads | 26.7 GB/s |
| k | 0.83 |
| Free memory under Windows | 6.4 GB of 16.5 GB |

| Model | Predicted | Measured |
|---|---|---|
| qwen3:1.7b | 24 tok/s | 21.3 tok/s |
| qwen3:4b | 13 tok/s | 9.6 tok/s |
| qwen3:8b | 6 tok/s | 5.48 tok/s |

Bandwidth peaks single threaded and drops at 8 threads. More threads do not help a workload that is waiting on memory.

### Two tiers

**Browser estimate** (`browser/fit.js`). Runs in a page, no install. Measures what the web platform exposes, using a WebAssembly threads kernel for bandwidth, then applies the same two equations. It carries a ±10% band and is labelled an estimate. A browser cannot reach the bandwidth a native program can, so it reads low more often than high, which makes a verdict that a model fits the conservative one.

**Native measurement** (`speedcheck/`). A Rust binary, no external crates. Measures memory bandwidth per working-set tier, f32 GEMM and int8 dot-product throughput, the sustained throttle curve, storage, and the CPU/ISA inventory. Then it downloads a pinned reference model, verifies its SHA-256, generates real tokens against it, and derives `k` from that run instead of guessing. Output is a human summary plus `sj_receipt.json`. Nothing is uploaded unless you answer yes to a prompt.

See `speedcheck/README.md` for build and usage. The receipt table shape is `docs/receipt-schema.sql`.

## What is proven and what is not

**Proven:**

- The probe runs, and its predictions matched measurement within 15 percent across a 4x range of model sizes, on one machine.

**Not proven:**

- One machine is one data point. These predictions have not been checked across a range of hardware, and `k` is a per-machine number that has to be measured, not carried over.
- The browser tier's ±10% band is an operating assumption from a small number of runs, not a number derived from a population of measurements.

## Layout

```
speedcheck/              native probe and speed check (Rust)
browser/fit.js           in-browser estimate
docs/receipt-schema.sql  receipt table shape
```

## Creator & Maintainer

[Agim Lolovic](https://github.com/agimlolovic)

## Contributing

SAPHOJUICE is open source. If you have a better way to benchmark local AI performance, estimate model compatibility, improve hardware detection, or add support for additional models or hardware, contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for how to share a hardware result, report a wrong prediction, or submit a pull request.

## License

MIT. See `LICENSE`.

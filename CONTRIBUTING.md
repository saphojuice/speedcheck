# Contributing

Three ways to help, easiest first. All contributions are under the [MIT license](LICENSE).

---

## 1. Share your hardware result

**This is the most useful thing anyone can do.** The whole project rests on one claim: that decode
speed can be predicted from measured memory bandwidth. That claim has been checked on a handful of
machines. It needs checking on yours.

Run either test:

```
# macOS and Linux
curl -fsSL https://saphojuice.com/check | sh

# Windows, in PowerShell
irm https://saphojuice.com/check.ps1 | iex
```

or press the face at [saphojuice.com](https://saphojuice.com).

Then open an issue with the
[**Hardware result**](https://github.com/saphojuice/speedcheck/issues/new?template=hardware-result.yml)
template.

**If you have Ollama, please include a real measurement.** It is worth more than any number of
predictions, because it is what the predictions get checked against:

```
ollama run qwen3:4b --verbose
```

and copy the `eval rate` line. That single number tells us this machine's true efficiency factor.

---

## 2. Report a wrong prediction

If the test said one thing and your machine did another, that is a finding, not a complaint. Use
the [**Wrong prediction**](https://github.com/saphojuice/speedcheck/issues/new?template=wrong-prediction.yml)
template: what was predicted, what you measured, and how you measured it.

A prediction that is wrong in a reproducible way is the fastest route to a better formula.

---

## 3. Improve the code

### Building locally

The native probe, in `speedcheck/`:

```
cd speedcheck
cargo build --release
./target/release/sj-probe --quick --offline --no-share
```

Rust 1.98.0 is what CI pins. `--offline` skips the model download, `--no-share` sends nothing, and
`--show-payload` prints exactly what would be sent.

The browser probe is a single file, `browser/fit.js`, with no build step. Drop it into any page
served with these two headers, or it will silently fall back to a much slower measurement:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

Then call `await SJ.measure(phase => console.log(phase))`.

### Before you open a pull request

- **Open an issue first for anything large.** A refactor nobody asked for is hard to accept.
- **Measurement changes must come with numbers.** If you touch anything that measures, include
  before and after results on named hardware: the CPU, how much memory, the OS, and whether it was
  plugged in. "Faster" without a machine attached cannot be reviewed.
- Match the surrounding style. There is no formatter config and no linter to satisfy.
- Keep dependencies few. The probe currently has one direct dependency, and that is deliberate.

### What is most wanted

- **Results from hardware nobody has tested.** AMD, Intel with and without AVX-512, Apple M4 and
  later, Snapdragon X, anything with unified memory.
- **A bundled inference engine**, so tok/s can be measured rather than predicted. This is the
  single biggest gap in the project: everything in v0.1.x is a prediction with an assumed
  efficiency factor.
- **Better GPU detection.** WebGPU adapter limits are a poor proxy for real VRAM bandwidth.
- **linux-arm64 builds.** The release matrix does not cover it yet.

---

## Reporting security problems

Not through issues. See [SECURITY.md](SECURITY.md).

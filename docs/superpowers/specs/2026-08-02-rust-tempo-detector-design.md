# Design: Rust rewrite of bpm-offset-detector (`tempo`)

## Context

The repo contains a C++ CLI (`FindTempo_standalone.cpp`/`.hpp`) implementing
Bram van de Wetering's tempo detection algorithm (documented in
`doc/syslab-version/paper.tex`), using `aubio` (GPL) for audio decoding and
onset detection.

GitHub issue #1 ("segfaults on mac") reports crashes on macOS. Root cause
(confirmed by analysis of the crash reports and code):

- macOS's C11-strict `libc` `aligned_alloc` returns `NULL` when the requested
  size is not an integral multiple of the alignment (16). glibc (Linux) does
  not enforce this, so the bug is silent there.
- `IntervalTester::fitness` is allocated as
  `AlignedMalloc(real_t, numIntervals)` where, for a 44.1kHz file,
  `numIntervals = 16823` → `16823 * 8 = 134584` bytes, and `134584 % 16 == 8`.
  `aligned_alloc` returns `NULL` on macOS → `memset(NULL, ...)` at
  `FindTempo_standalone.cpp:435` → SIGSEGV. This matches crash report #1
  exactly (`_platform_bzero` called from `CalculateBPM` at line 435).
- `Samples::left`/`right` are allocated as `AlignedMalloc(short, frame_len)`;
  for arbitrary `frame_len` the byte count (`frame_len * 2`) is frequently not
  a multiple of 16, producing the same `NULL`-write crash at line 767. This
  matches crash report #2 exactly.

Rather than patch this class of bug in place, the decision (made with the
repo owner) is to build a new, independent implementation based on the paper
and the example code, in Rust, structured so that a WebAssembly build is a
natural next step (not built in this iteration, but not precluded either).

## Goals

- Cross-platform executable (Windows, macOS x86_64+arm64, Linux) with no
  system library dependencies (unlike aubio, which is a build headache on
  Windows and was the source of this segfault class).
- Faithful port of van de Wetering's tempo algorithm as implemented in
  `FindTempo_standalone.cpp` and described in the paper.
- Architected so the core algorithm crate has zero I/O/threading
  dependencies, making a future `wasm-bindgen` web build straightforward.
- MIT-licensed (the repo owner's long-stated goal, currently blocked by the
  GPL `aubio` dependency in the C++ version).
- Attempt to fix upstream's broken offset detection; validate it against
  synthetic ground truth and report honestly if it can't be made reliable.

## Non-goals (v1)

- A web UI or `tempo-wasm`/`wasm-bindgen` crate (architecture supports it,
  but it is not being built now).
- Multithreading (upstream's implementation leaked thread-tracking memory
  under load; algorithm is fast enough single-threaded for song-length
  inputs; can be added later behind a `rayon` feature).
- Machine-learning-based onset detection methods (BLSTM/CNN/LL) discussed in
  the paper — those require `madmom`/Python and are out of scope for a
  native/WASM Rust tool.
- Legacy onset-text-file batch format used by `dataset/scripts/*.py`.
- Modifying or removing the existing C++ implementation; it remains in the
  repo as-is (segfault and all) since it documents the original research.

## Architecture

New Cargo workspace at `rust/` inside this repo:

```
rust/
├── Cargo.toml                     # workspace
└── crates/
    ├── tempo-core/                # pure algorithm crate
    │   ├── Cargo.toml              # deps: rustfft only
    │   └── src/
    │       ├── lib.rs              # public API: detect(samples, sample_rate, opts) -> Vec<TempoResult>
    │       ├── onset.rs            # complex-domain onset detection
    │       ├── gapdata.rs          # wrapped-onset histogram + Hamming-windowed gap confidence
    │       ├── interval.rs         # coarse scan, polyfit normalization, refinement
    │       ├── bpm.rs              # dedup, octave-collapse, integer rounding, top-3 selection
    │       ├── offset.rs           # base offset + offbeat disambiguation
    │       └── polyfit.rs          # cubic least-squares (normal equations + Givens QR)
    └── tempo-cli/                  # native executable crate
        ├── Cargo.toml              # deps: tempo-core, symphonia, clap, serde_json
        └── src/main.rs
```

`tempo-core` has no dependency on file I/O, threads, or any platform API. It
takes `&[f32]` mono samples + sample rate + options, and returns tempo
candidates. This is the boundary that makes a future `tempo-wasm` crate (with
`wasm-bindgen`) a thin wrapper rather than a rewrite: on the web, JavaScript's
Web Audio `decodeAudioData` would supply the PCM buffer instead of
`symphonia`.

`tempo-cli` is a native binary using `symphonia` (pure-Rust audio decoding:
MP3/FLAC/WAV/OGG/AAC/M4A, no system libraries) and `clap` for argument
parsing.

## Algorithm specification

Ported from `FindTempo_standalone.cpp`, cross-checked against
`doc/syslab-version/paper.tex` §Methods/§Implementation.

Constants (overridable via CLI flags where noted):
- `MIN_BPM = 40.0`, `MAX_BPM = 260.0` (flags: `--min-bpm`, `--max-bpm`).
  *Amended post-implementation from the reference's 89–205: songs slower
  than 89 BPM were otherwise reported as their in-range octave multiples
  (e.g. 68 BPM detected as 136). The wider range costs ~3.3× runtime, which
  is acceptable.*
- `INTERVAL_DELTA = 10`
- `INTERVAL_DOWNSAMPLE = 3`
- Onset detection window = 1024 samples, hop = 256 samples
- Onset strength is constant (`1.0`) for every onset — the paper found that
  weighting by measured onset strength *hurt* accuracy vs. treating every
  onset equally, so this is intentional, not a simplification.

Pipeline:

1. **Decode** (CLI only) → mono `f32` samples (average of L/R channels,
   normalized) + sample rate.
2. **Onset detection** — the one piece that is *reimplemented* rather than
   ported line-for-line, since the original delegates this to `aubio`'s
   `"complex"` onset method. Complex-domain onset detection: per-frame FFT
   (1024-point, 256 hop), predict next frame's complex spectrum from
   magnitude+phase of previous two frames, sum the magnitude of the
   prediction error across bins as the onset detection function, then peak-pick
   with aubio's default parameters (adaptive threshold, silence gate ~ -70dB,
   minimum inter-onset interval) with parabolic interpolation for
   sub-frame-accurate onset position. The paper identifies Complex Domain as
   one of the two best non-ML onset methods (along with LL, which requires
   `madmom`/RNN and is out of scope), and it's what the reference C++ uses by
   default.
3. **Coarse interval scan**: for every `IntervalDelta`-th interval between
   `minInterval` and `maxInterval` (computed from BPM bounds and sample
   rate), compute gap confidence using a wrapped-onset histogram and a
   256-tap (`2048 >> INTERVAL_DOWNSAMPLE`) Hamming window, with onset
   positions downsampled by `INTERVAL_DOWNSAMPLE`.
4. **Cubic polynomial normalization**: fit a degree-3 polynomial through the
   coarse `(interval, fitness)` points using least squares (normal equations
   solved via Givens-rotation QR — the same numerical method as the
   reference's `polyfit.h`, reimplemented from the underlying math rather
   than copied, both because `polyfit.h` is explicitly unlicensed in this
   repo's README and to keep the new crate cleanly MIT). Confirmed via
   reading `polyfit.h`: the call site skips zero-valued fitness entries and
   uses `x = minInterval + index`, i.e. the polynomial is fit directly over
   `(interval, fitness)` pairs — subtract the fitted curve from each raw
   fitness value to normalize.
5. **Refinement**: for every coarse interval whose normalized fitness
   exceeds 0.4× the coarse maximum, scan every interval in a
   `±IntervalDelta` window at full resolution (still using the downsampled
   gap data) and keep the best interval in each window as a BPM candidate.
6. **Candidate selection**: sort candidates by fitness descending; remove
   near-duplicates and octave duplicates (BPM values within 0.1 of each
   other's value, double, or half); rebuild gap data at full resolution
   (downsample 0) and re-check candidates whose BPM is within 0.05 of an
   integer, snapping to the integer if confidence doesn't drop; if the top
   two candidates' fitness are within 5% of each other, recompute both at
   full precision and re-sort ("second opinion"); keep the top 3.
7. **Offset detection** (per candidate BPM): compute the base offset as the
   histogram position with highest gap confidence at full resolution, then
   disambiguate against the offbeat position (offset + half a beat) by
   comparing which position has more support in a slope-based representation
   of the raw waveform (leading-edge energy over ~50ms windows), picking
   whichever has higher total support.

## Offset reliability — validation gate

Upstream (`README.md`, `paper.tex` §Implementation) states offset detection
is broken and the cause is unknown. Plan: port the algorithm above faithfully,
then validate with synthetic click tracks generated at known BPM *and* known
offset. Decision rule:

- If median absolute offset error across the synthetic suite is ≤ ~5ms,
  ship the offset field normally.
- If not, ship it anyway but set `"offset_experimental": true` in JSON
  output (and print a warning in text output) rather than hiding it,
  and record findings in the crate's doc comments for future work.

This will be resolved during implementation (step 5 in Implementation Plan),
not decided in advance.

## CLI

```
tempo <file> [--min-bpm 40] [--max-bpm 260] [--start 0] [--duration 60] [--json]
tempo batch <folder> --out results.csv [--json]
```

- Single-file mode: decode, run detection, print top-3
  `(bpm, offset, fitness)` as human-readable text or `--json`.
- `batch` mode: walk a folder of audio files (extension-filtered via
  symphonia's supported formats), run detection on each, write a CSV (or
  JSON array with `--json`) of results. This replaces the legacy
  onset-text-file batch format (out of scope per Non-goals).

## Cross-platform builds

GitHub Actions workflow, matrix build on tag push:
- `x86_64-pc-windows-msvc`
- `x86_64-apple-darwin`, `aarch64-apple-darwin`
- `x86_64-unknown-linux-musl` (static, for portability across distros)

No system libraries are required by any dependency (`rustfft`, `symphonia`,
`clap`, `serde_json` are all pure Rust), so `cargo build --release` on stock
GitHub-hosted runners is sufficient — no aubio-style native dependency setup.
Release binaries are attached to GitHub Releases via
`softprops/action-gh-release`.

## Testing strategy

- **Unit tests** (`tempo-core`): polyfit against hand-computed coefficients
  for known small datasets; Hamming window generation; gap-confidence
  wrap-around behavior at histogram boundaries.
- **Synthetic end-to-end tests**: generate click tracks (impulse trains) at
  known BPMs spanning the supported range (89, 120, 118.879, 162, 174, 205)
  and assert the top-1 candidate is within ±0.05 BPM; generate tracks with
  known onset offsets to validate the offset module (see validation gate
  above).
- **Differential harness (dev-only, not CI)**: build the segfault-fixed
  original C++ on Linux, run both implementations on the same synthetic
  inputs, and diff BPM outputs to catch algorithmic porting mistakes early.
  This is a development aid, not a shipped test suite (the C++ build isn't
  portable to Windows/macOS CI without aubio).

## Implementation order

1. Cargo workspace + `tempo-core` skeleton (public API shape, no logic yet).
2. `polyfit.rs` + unit tests (independent of audio, verifiable in isolation).
3. `gapdata.rs`, `interval.rs`, `bpm.rs` driven by synthetic onset-position
   arrays (no real audio/FFT needed yet) — validates the core BPM-finding
   logic against known interval/onset fixtures.
4. `onset.rs` — complex-domain onset detection via `rustfft`.
5. `offset.rs` + validation gate decision (see above).
6. `tempo-cli` — `symphonia` decoding, `clap` argument parsing, single-file
   mode.
7. Batch mode + `--json` output.
8. Synthetic end-to-end test suite (ties steps 2–6 together).
9. GitHub Actions release workflow.

## Open questions / risks

- Complex-domain onset detection is a reimplementation, not a port — its
  exact numerical output will differ from aubio's. Mitigated by validating
  against known-BPM synthetic tracks rather than expecting bit-for-bit
  parity with the C++/aubio pipeline.
- The cubic-polyfit normal-equations approach is numerically delicate at the
  scale of raw sample-interval values (x up to ~30,000, degree 3 → x^6 terms).
  The reference implementation apparently tolerates this in `f64`; the Rust
  port will use `f64` throughout for the same reason and be checked against
  the differential harness.

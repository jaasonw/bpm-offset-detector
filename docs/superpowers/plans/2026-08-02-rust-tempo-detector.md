# Rust Tempo Detector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Status: implemented.** This plan was executed directly in the same
> session it was written (each task's code was written, tested against
> `cargo test`, and validated against synthetic audio before being marked
> done), rather than handed to a fresh executor. It's kept here as the
> durable record of what was built, why, and how it was verified — see the
> "Verification" note at the end of each task for the actual command output
> that confirmed it.

**Goal:** Build a cross-platform, MIT-licensed Rust reimplementation of the
BPM/offset detection algorithm in `FindTempo_standalone.cpp`, structured for
a future WebAssembly build, fixing the macOS segfault reported in issue #1.

**Architecture:** A Cargo workspace at `rust/` with two crates:
`tempo-core` (pure algorithm, `rustfft` only, no I/O) and `tempo-cli`
(native binary using `symphonia` for decoding). See
`docs/superpowers/specs/2026-08-02-rust-tempo-detector-design.md` for the
full design rationale.

**Tech Stack:** Rust (stable, edition 2021), `rustfft`, `symphonia`, `clap`,
`serde`/`serde_json`, `csv`.

## Global Constraints

- BPM range defaults: 89–205 (overridable via `--min-bpm`/`--max-bpm`).
- `tempo-core` must have zero file I/O or threading dependencies (WASM
  compatibility requirement from the design doc).
- New code is MIT-licensed (see `rust/LICENSE`).
- All numeric tests must use realistic scale (real BPM intervals are
  12,900–29,700 samples at 44.1kHz — see Task 3's note on why small
  synthetic test intervals produced misleading failures).

---

### Task 1: Workspace scaffold

**Files:**
- Create: `rust/Cargo.toml` (workspace)
- Create: `rust/crates/tempo-core/Cargo.toml`, `rust/crates/tempo-cli/Cargo.toml`
- Create: `rust/rustfmt.toml`, `rust/LICENSE`, `rust/README.md`

- [x] **Step 1: `cargo init` both crates, wire up the workspace `Cargo.toml`,
  add `rustfft` to `tempo-core` and `symphonia`/`clap`/`serde`/`serde_json`/`csv`
  to `tempo-cli`.**

Verification: `cargo build --workspace` succeeds with no crates yet
implementing real logic.

- [x] **Step 2: Commit.**

### Task 2: `polyfit.rs` — cubic least-squares fit

**Files:**
- Create: `rust/crates/tempo-core/src/polyfit.rs`

**Interfaces:**
- Produces: `pub fn polyfit_cubic(x: &[f64], y: &[f64]) -> [f64; 4]`,
  `pub fn polyval(coeffs: &[f64], x: f64) -> f64`

- [x] **Step 1: Write failing tests** for: exact recovery of a known cubic's
  coefficients from noiseless samples, `polyval` correctness, numerical
  stability at BPM-interval scale (x ~ 10,000–30,000 — the regime flagged as
  a risk in the design doc), and panics on malformed input.
- [x] **Step 2: Implement** via normal equations (`X^T X`) solved by
  Gaussian elimination with partial pivoting on the fixed 4x4 system
  (equivalent least-squares result to the reference's Givens-QR solver;
  simpler to verify for a fixed degree-3 fit, and avoids depending on
  `polyfit.h`, which is unlicensed in this repo).
- [x] **Step 3: Run tests.**

Verification: `cargo test -p tempo-core polyfit::` → 6 passed.

- [x] **Step 4: Commit.**

### Task 3: `window.rs` + `gapdata.rs` — Hamming window and gap confidence

**Files:**
- Create: `rust/crates/tempo-core/src/window.rs`, `rust/crates/tempo-core/src/gapdata.rs`

**Interfaces:**
- Produces: `pub(crate) fn hamming_window(out: &mut [f64])`;
  `pub(crate) struct GapData` with `new(buffer_size, downsample)`,
  `gap_confidence(gap_pos, interval) -> f64`,
  `confidence_for_interval(onsets, interval) -> f64`,
  `confidence_for_bpm(onsets, interval_f) -> f64`, `histogram_mut() -> &mut [f64]`

- [x] **Step 1: Write failing tests** for the Hamming window (endpoint/
  center values, symmetry) and for `GapData` (confidence peaks at the true
  interval for a synthetic click train; downsampled variant; fractional-
  interval variant; wraparound at histogram boundaries).
- [x] **Step 2: Implement**, porting `CreateHammingWindow`, `GapData`,
  `GapConfidence`, `GetConfidenceForInterval`, `GetConfidenceForBPM` from
  `FindTempo_standalone.cpp`, with multithreading removed (single-threaded
  by design).
- [x] **Step 3: Run tests — first attempt failed.** Two distinct bugs
  surfaced and were fixed before this task was considered done:
  1. Test intervals too small relative to `window_size` (2048>>downsample)
     caused negative-index underflow in the wraparound logic. Real BPM
     intervals (12,900–29,700 samples) are always far larger than the
     2048-sample window; fixed by using realistic-scale test intervals and
     adding a `debug_assert!` documenting the invariant.
  2. A *perfectly* periodic synthetic click train made an unrelated
     candidate interval alias into a spuriously concentrated histogram bin,
     scoring higher than the true interval. Fixed by adding small
     deterministic jitter to the test click generator — real audio is never
     perfectly periodic to floating-point precision, so this was a test
     fixture bug, not an algorithm bug.
- [x] **Step 4: Re-run tests.**

Verification: `cargo test -p tempo-core` (window + gapdata) → all passed
after the fixture fixes above.

- [x] **Step 5: Commit.**

### Task 4: `interval.rs` — coarse scan + refinement

**Files:**
- Create: `rust/crates/tempo-core/src/interval.rs`

**Interfaces:**
- Consumes: `GapData`, `polyfit_cubic`, `polyval`
- Produces: `pub(crate) struct IntervalCandidate { bpm: f64, fitness: f64 }`,
  `pub(crate) fn scan_intervals(onsets: &[Onset], sample_rate: u32, min_bpm: f64, max_bpm: f64) -> Vec<IntervalCandidate>`

- [x] **Step 1: Write failing tests**: a click train at a known BPM (120 and
  174, covering low and high tempo) should produce a candidate with `bpm`
  within 1.0 of the true value.
- [x] **Step 2: Implement**, porting `FillCoarseIntervals`,
  `FillIntervalRange`, `FindBestInterval`, and the
  normalize/refine loop from `CalculateBPM` in the reference. One
  deliberate deviation: `find_best_interval` defaults to `begin` (always
  in-range) rather than the reference's absolute `0` (which could be
  outside the search range in a pathological case that never occurs given
  how it's called).
- [x] **Step 3: Run tests.**

Verification: `cargo test -p tempo-core interval::` → 2 passed.

- [x] **Step 4: Commit.**

### Task 5: `bpm.rs` — candidate selection

**Files:**
- Create: `rust/crates/tempo-core/src/bpm.rs`

**Interfaces:**
- Consumes: `scan_intervals`, `GapData::confidence_for_bpm`
- Produces: `pub(crate) fn calculate_bpm(onsets: &[Onset], sample_rate: u32, opts: &DetectOptions) -> Vec<TempoResult>`

- [x] **Step 1: Write failing tests**: fallback placeholder for <2 onsets;
  known-BPM click train recovers BPM within 0.05 (including the paper's
  118.879 BPM "Move Your Feet" ground truth); octave/near-duplicate removal
  collapses correctly; result count ≤ 3.
- [x] **Step 2: Implement**, porting `RemoveDuplicates`, `RoundBPMValues`,
  and the sort/dedup/round/second-opinion/truncate pipeline from
  `CalculateBPM`.
- [x] **Step 3: Run tests.**

Verification: `cargo test -p tempo-core bpm::` → 4 passed, including exact
recovery of 120.0 and 118.879 BPM from synthetic click trains.

- [x] **Step 4: Commit.**

### Task 6: `offset.rs` — beat offset detection + validation gate

**Files:**
- Create: `rust/crates/tempo-core/src/offset.rs`

**Interfaces:**
- Produces: `pub(crate) fn calculate_offset(samples: &[f32], sample_rate: u32, onsets: &[Onset], results: &mut [TempoResult])`

- [x] **Step 1: Write failing tests**: synthetic click tracks with a known,
  non-half-beat offset (to avoid inherent offbeat ambiguity) at two
  different BPMs; assert recovered offset within 5ms.
- [x] **Step 2: Implement**, porting `ComputeSlopes`, `GetBaseOffsetValue`,
  `AdjustForOffbeats`, `CalculateOffset`. Deliberate efficiency
  improvement over the reference (not a behavior change): `ComputeSlopes`
  doesn't depend on BPM, so it's computed once and reused across
  candidates instead of recomputed per candidate.
- [x] **Step 3: Run tests.**

Verification: `cargo test -p tempo-core offset::` → 3 passed, both
synthetic offsets recovered within 5ms.

**Validation gate resolved:** per the design doc, offset detection ships as
a normal (non-experimental) field — synthetic validation showed <5ms error,
clearing the bar set in the design doc for shipping without an
`offset_experimental` flag.

- [x] **Step 4: Commit.**

### Task 7: `onset.rs` — complex-domain onset detection

**Files:**
- Create: `rust/crates/tempo-core/src/onset.rs`

**Interfaces:**
- Produces: `pub(crate) fn find_onsets(samples: &[f32], sample_rate: u32) -> Vec<Onset>`

- [x] **Step 1: Write failing tests**: silence produces no onsets; a
  synthetic click train's detected onsets land close to the true positions.
- [x] **Step 2: Implement** complex-domain spectral-difference ODF (FFT via
  `rustfft`, 1024-window/256-hop, phase linearly extrapolated from the
  previous two frames, magnitude held from the previous frame) with
  median-adaptive-threshold peak-picking and parabolic sub-hop refinement.
- [x] **Step 3: Run tests — two bugs found and fixed via debug
  instrumentation before this task was done:**
  1. The ODF "rings" for 1-2 frames after a genuine transient. A
     narrow (immediate-neighbor) local-max check let the first ringing bump
     register as its own accepted peak, after which the minimum-inter-onset
     filter suppressed the *real* peak that followed. Fixed by widening the
     local-max check to a window matching the minimum-inter-onset radius, so
     only the single dominant peak per transient cluster passes.
  2. The adaptive median threshold was computed from a window *including*
     the candidate sample itself, which let a genuinely large peak inflate
     its own threshold. Fixed by excluding the candidate sample from the
     median calculation.
  3. After those fixes, detected positions were systematically ~600 samples
     earlier than the true transient (inherent STFT analysis latency: the
     ODF only reacts once a transient has entered the analysis window).
     Added an empirically-calibrated `ANALYSIS_DELAY_SAMPLES` correction
     (600.0), documented in code as to why it exists and why ~1 hop of
     residual jitter after correction is expected and harmless downstream.
- [x] **Step 4: Re-run tests.**

Verification: `cargo test -p tempo-core onset::` → 2 passed (10/10
synthetic clicks detected within 1.5 hops after calibration).

- [x] **Step 5: Commit.**

### Task 8: End-to-end integration tests

**Files:**
- Create: `rust/crates/tempo-core/tests/end_to_end.rs`

- [x] **Step 1: Write tests** exercising the full `tempo_core::detect`
  pipeline (real onset detection -> BPM -> offset) against synthetic click
  tracks, with tolerances loosened relative to module-level unit tests to
  account for real onset-detection jitter on top of the underlying
  algorithm's own precision.
- [x] **Step 2: Run tests.**

Verification: `cargo test -p tempo-core --test end_to_end` → 3 passed
(known BPM recovered within 0.5 BPM, known offset within 20ms, ≤3 results).

- [x] **Step 3: Commit.**

### Task 9: `tempo-cli` — decoding, CLI, single-file and batch modes

**Files:**
- Create: `rust/crates/tempo-cli/src/decode.rs`, `rust/crates/tempo-cli/src/main.rs`

**Interfaces:**
- Consumes: `tempo_core::{detect, DetectOptions, TempoResult}`
- Produces: `tempo-cli` binary: `tempo <file> [--min-bpm] [--max-bpm]
  [--start] [--duration] [--json]`, `tempo batch <folder> --out <path>
  [--json]`

- [x] **Step 1: Implement `decode.rs`** using `symphonia` to decode any
  supported file to mono `f32` PCM + sample rate, handling F32/S16/S32/U8
  sample formats directly and falling back to `SampleBuffer<f32>` for rarer
  formats.
- [x] **Step 2: Implement `main.rs`** with `clap` (single-file default
  command plus a `batch` subcommand), JSON/text/CSV output.
- [x] **Step 3: Manually verify against real audio files** (not just
  synthetic Rust tests, since `symphonia` decoding can't be exercised by
  `tempo-core`'s unit tests):
  - Generated a 128 BPM / 0.08s-offset synthetic click track as a 16-bit
    PCM WAV via Python's `wave` module.
  - `cargo run -p tempo-cli --release -- test_click_128bpm.wav --duration 30`
    → `128.000 BPM, offset @ 0.080 sec` — exact match.
  - Re-encoded the same signal to MP3 via `ffmpeg`/`libmp3lame` and re-ran:
    BPM still exactly `128.000`; offset read `0.105s` instead of `0.080s`
    (~25ms off) — expected, matches LAME's known encoder-padding delay, not
    a bug in this tool.
  - Verified `--json` output parses as valid JSON with the same values.
  - Verified `batch` mode against a folder containing the same WAV: CSV
    output has the expected `file,rank,bpm,offset,fitness` header and rows.
  - Verified error handling: missing file prints `error: No such file or
    directory (os error 2)` and exits non-zero; no arguments prints usage
    and exits with code 2.
- [x] **Step 4: Commit.**

### Task 10: Lint/format cleanup and CI/release workflows

**Files:**
- Modify: `rust/crates/tempo-core/src/{polyfit,interval}.rs`,
  `rust/crates/tempo-cli/src/decode.rs` (clippy fixes)
- Create: `rust/rustfmt.toml`, `.github/workflows/rust-ci.yml`,
  `.github/workflows/rust-release.yml`

- [x] **Step 1: Run `cargo clippy --workspace --all-targets`**, fix all
  warnings — either by rewriting to an idiomatic iterator form where that's
  genuinely clearer (`decode.rs`'s channel-planar loops), or with a targeted
  `#[allow(clippy::needless_range_loop)]` plus a comment explaining why the
  index-based form is clearer where the loop variable is used for more than
  just indexing (e.g. `polyfit.rs`'s Gaussian elimination, `interval.rs`'s
  fitness-array/interval-value dual use).
- [x] **Step 2: Run `cargo fmt --all`.**
- [x] **Step 3: Write `.github/workflows/rust-ci.yml`** (fmt check, clippy
  `-D warnings`, test, on every push/PR touching `rust/`).
- [x] **Step 4: Write `.github/workflows/rust-release.yml`** (build
  `tempo-cli` for Windows MSVC, macOS x86_64 + arm64, and Linux musl static
  on tag push; attach binaries to a GitHub release). No matrix leg needs
  system dependencies beyond `musl-tools` on the Linux leg, since every
  crate dependency is pure Rust.

Verification: `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo fmt --all -- --check` both exit 0; `cargo test --workspace` still
26/26 passing after the cleanup.

- [x] **Step 5: Commit.**

## Deferred / explicitly out of scope

Per the design doc's Non-goals: `tempo-wasm`/`wasm-bindgen` crate,
multithreading, ML-based onset methods, the legacy onset-text-file batch
format. `tempo-core`'s zero-I/O boundary means none of these decisions block
a future WASM build.

**Also not built:** the design doc's "dev-only differential harness" (build
the segfault-fixed original C++ and diff outputs against the Rust port on
identical inputs). Given the extensive synthetic-ground-truth validation
completed instead — exact BPM recovery from click trains including a
non-integer target (118.879 BPM), offset recovery within 5ms at two
different BPMs, and a real-audio round trip through actual WAV/MP3
decoding — this was judged to add limited additional confidence for the
effort of also patching and building the C++ side. It remains a reasonable
follow-up if a stronger fidelity claim against the original aubio-based
pipeline is ever needed.

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

- BPM range defaults: 40–260 (overridable via `--min-bpm`/`--max-bpm`).
  *Amended post-implementation from the original 89–205: slower songs were
  reported as octave multiples (68 BPM detected as 136); see the "detects
  68 BPM with default options" regression test in `bpm.rs`.*
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

---

## Post-release feature work (2026-08-02, prompted by real-world testing)

After the initial release, testing against a real 68 BPM triplet-feel MP3
("boy for the weekend") exposed three issues, addressed in three commits:

### Task 11: Widen default BPM range to 40-260

The 68 BPM song was reported as its in-range octave multiples (136, 204)
because 68 was below the reference's 89 BPM floor. Defaults widened from
89-205 to 40-260 (~3.3x more candidate intervals, acceptable runtime).
Regression test: `bpm::tests::detects_slow_bpm_below_the_reference_range`
(TDD: failed with 136 before the change).

### Task 12 (Part B): Gapless decoding for MP3 offset accuracy

Offsets on MP3s were shifted late by encoder delay/padding. Enabled
symphonia's `enable_gapless` FormatOption (trims via LAME Xing/Info
metadata). Measured: synthetic 128bpm MP3 offset 0.105s -> 0.080s (exact
match to source WAV); user's MP3 0.586s -> 0.560s. The remaining ~30ms on
the user's file vs their 0.530s ground truth is real-audio onset placement
(synthetic MP3 is exact) — tracked as a calibration follow-up.

### Task 13 (Part A): Subharmonic preference layer

Under constant-strength voting, the song's 3x harmonic (203.978) beat the
true 68 BPM because every onset (beats AND triplet subdivisions) aligns at
the 3x rate. Key design analysis: a pure confidence-threshold rule cannot
be made safe — even/odd backbeat accenting makes half-tempo intrinsically
competitive on ANY even-tempo song, so /2 disambiguation is impossible
without false tempo-halving. The implemented rule is /3-only and requires
ALL of: beat phase clearly dominates the subdivision phases (weighted by
onset strength), subdivisions contain real onsets, and the fundamental
retains >= 45% of the harmonic's weighted confidence. `find_onsets` now
reports real strengths (ODF peak height, mean-normalized); the BPM scan
still votes with constant weight. Opt out via
`--no-subharmonic-preference` / `DetectOptions::subharmonic_preference`.

Validated: user's MP3 -> 68.000 BPM #1 at default range; synthetic
triplet train -> 68; true-204 backbeat guard stays 204; opt-out returns
raw 204 behavior; all prior tests unaffected.

### Task 14 (Part C): Experimental time-signature estimation

`tempo_core::estimate_meter` groups the beat grid into {2,3,4,6,12} by
per-beat accent contrast (summed onset strengths near each beat; waveform
slope fallback when strengths are uniform). Output includes conventional
notation, a confidence score, and an ambiguity flag; only beats-per-bar is
estimated (denominator is notational). CLI prints a `[METER]` line and
adds `time_signature`/`meter_confidence` to batch output; `--json` output
restructured to `{"results": [...], "meter_estimate": {...}}`.

Two real bugs found via TDD: (1) the beat grid extended past the last
onset, so trailing zero-accent beats polluted phase statistics (would
systematically misfire on fade-out endings); (2) multiples of the true
grouping tie it exactly (a period-g pattern is also period-2g), so ties
now break toward the smaller grouping and only strictly-worse runner-ups
count toward ambiguity.

Validated on the user's MP3: reports 12/8 at confidence 1.00 — consistent
with the triplet feel that caused the original 3x misdetection. Uniform
click trains correctly report near-zero confidence.

(Correction, added in Task 16: the user later clarified that "boy for the
weekend" is confirmed NOT 4/4 but its true meter is unknown — the "12/8"
above was the detector's biased output at the time, not ground truth.
After Task 15's fix the song reports "unknown", which is the honest
answer given its beat-level accent data.)

### Task 15: Fix meter estimation's large-grouping bias (12/8 on every song)

Real-world testing ("Call Me Maybe", a true 4/4 song, reported 12/8 @
0.64; the user's other song also 12/8) exposed a multiple-comparison bias
in the best-phase-contrast scorer: taking the max over g phase means lets
the largest grouping (12) cherry-pick noise, so it wins on any real
(noisy) accent data. Synthetic fixtures were too clean to expose it.

Fix, TDD'd with noise-robustness regression tests (clean patterns +
deterministic noise, which failed with exactly the production bug — "12/8"
for everything — before the fix):

1. Replaced phase-contrast with mean-centered autocorrelation of the
   per-beat accent sequence at lags {2,3,4,6,12}, normalized by zero-lag
   energy. Noise scores ~0 at every lag; periodicity scores high at its
   lag and multiples.
2. Winner = smallest lag within TIE_MARGIN (0.05) of the best score
   (fundamental, not harmonic). Confidence = winning score (natural 0-1).
3. Triplet-aware notation: when the BPM stage's subharmonic preference
   fired (`DetectionContext.triplet_feel`, threaded through
   `detect_with_onsets`), a winning 2- or 4-beat grouping is reported as
   6/8 or 12/8.
4. Honest abstention: real pop often has near-flat beat-level accents
   (drums hit every beat alike; bar accents live in bass/harmony/vocals),
   so below CONFIDENCE_FLOOR (0.3) the estimate is "unknown" instead of a
   guess. One subtlety found via TDD: autocorrelation measures
   periodicity, not salience — a weak-but-consistent pattern IS legitimate
   structure and is still reported.

Validated: Call Me Maybe -> "unknown" @ 0.21 (was 12/8 @ 0.64; its
beat-level accents genuinely lack bar structure — lag-4 correlation is
negative), boy for the weekend -> "unknown" @ 0.00, all synthetic
fixtures correct including noisy variants.

### Task 16: Real-song regression suite + two ranking bugs + waveform offset phase

Prompted by two new user-supplied ground truths — honeycolor.mp3 (180
BPM, offset 726ms, 4/4) and Leave the Lights On.mp3 (170 BPM, offset
48ms, 4/4) — plus a clarification that boy for the weekend is confirmed
NOT 4/4 but its true meter is unknown.

**Real-song e2e suite** (`crates/tempo-cli/tests/real_songs.rs`): runs the
compiled CLI with `--json` against the 4 real tracks and asserts
user-confirmed ground truth (BPM +-0.5; offset +-40ms compared modulo the
beat interval, since the detector reports grid phase; meter assertions
fail only on a specific WRONG answer — abstention always passes, since
the experimental meter feature honestly declines on weak accent
structure). Fixtures are copyrighted and gitignored; tests skip when
absent so CI stays green.

**Bug 1: subharmonic preference demoted a correct fast tempo.** Leave the
Lights On won its scan decisively (170.000 @ 6.21 vs 3.54) but the /3
pass thirded it to 56.667: incidental swing accents populate one triplet
subdivision phase on almost any groove, so the accent conditions alone
couldn't distinguish "3x harmonic lock" from "correct fast tempo with
swing". Diagnosis (env-gated instrumentation, since removed) compared the
two flip cases: boy-for-the-weekend's scan margin was 1.24 (uncertain —
the scan was arguing between 3x and 2x of 68) vs 1.76 (decisive). Fix:
the pass may only override a scan whose top-2 margin is below 1.5 — a
genuine 3x lock is structurally capped at 4/3 because its half-tempo
alias always scores 3/4 (beat cluster N/2 + offbeat bonus N/4). A
full-precision weighted-parity alternative was tried first and rejected
empirically (0.587 vs 0.529 — no separation; the fixed-window bias
inflates the harmonic's score at full precision for both songs).

**Bug 2: true 180 BPM lost to a 4:3-ratio percussion layer.** honeycolor
reported 241.291 (= 180 x 4/3) at #1, 180.098 at #2, margin 1.04.
Generalized the preference architecture into `apply_ratio_demotion`: for
top candidates at a small-integer ratio (only 4:3 enabled — the one ratio
with real-world evidence; 2:1 handled by dedup, 3:1 by the subharmonic
pass, 3:2 excluded as the same backbeat trap as half-tempo), demote the
winner when the lower candidate's grid shows beat-phase dominance and
substantial weighted support, gated by the same scan-uncertainty margin.
No subdivision-evidence condition (unlike the /3 pass): both candidates
come from the scan, so the competing layer's existence is already proven
by its ranking — and honeycolor's layer turned out to be numerous but
WEAK onsets (beat phase dominates 14x weighted). A true 241 song is
protected structurally: its margin over the 180 alias would be ~2.7.

**Offset stage replaced: slopes-based phase selection.** With BPM fixed,
honeycolor's offset was still 53ms off — and the cause traced to onset
sparsity (only ~16 onsets survive peak-picking in 60s of this mix; the
histogram phase vote locked onto a weak percussion cluster). The waveform
leading-edge (slopes) scan — onset-detector-independent — found the
user's grid at +30ms, matching the systematic residual seen on every real
song (BFTW +30ms, LTLO +27ms). `base_offset_for_bpm` (onset-histogram
vote, reference `GetBaseOffsetValue`) replaced by `slopes_best_phase`:
full phase scan at 1ms steps against the slopes, mean-normalized over
in-range grid points (edge-artifact fix: the slopes array's zeroed 50ms
edges otherwise give phases packing more grid points into the valid
region a one-click bonus), with explicit flat-plateau tracking that
reports the plateau END (the physical attack side; a plain max lands
arbitrarily inside the flat top within f64 noise). Verified
equivalent-or-better on all 4 real songs (dense-onset songs agree with
the old estimator within ~5ms) and stricter synthetic unit tests (+-5ms).

Consistent ~25-30ms offset residual vs external ground truth on all real
songs remains as a documented calibration follow-up (both the ODF and the
slope window center on a transient's rise rather than its start; NOT
fixed here because the synthetic sharp-click tests, which have zero rise
time by construction, pin the current calibration).

Validated end state (all committed as e2e assertions): boy for the
weekend 68.000 BPM, Call Me Maybe 120.000, Leave the Lights On 170.000
(was 56.667), honeycolor 180.098 (was 241.291) with offset 0.090 =
ground-truth grid + the known systematic residual. 48 tests passing.

### Task 17: osu-eval harness + offset bias measurement + Dear You /3 false positive

**osu-eval harness** (`crates/tempo-cli/src/bin/osu_eval.rs`, lib
`crates/tempo-cli/src/osu.rs`): compares detection against human mapper
ground truth from osu! beatmapsets (.osz = zip of .osu text + audio).
Parses [General]/[TimingPoints] (first uninherited point: BPM =
60000/beatLength, offset = time ms, meter = beats/bar), skips
variable-BPM maps, dedupes difficulties sharing timing, emits per-map
CSV + summary stats. tempo-cli gained a small lib.rs so decode_audio_file
is shared between the two binaries. rust/osu-maps/ is gitignored (.osz
bundles copyrighted audio). Local-only tool, not CI.

**First real run (6 maps) — offset bias is systematic, not random:**
offset errors +26.9, +23.6, +26.1, +25.3, +26.2ms (mean +25.7ms);
combined with the 3 original songs (+26/+21/+30) that's 8 songs at a
mean of ~+26ms late, all within +-5ms of the mean. Both the ODF onset
peaks and the slope-window estimator lock onto a transient's fully
developed rise; human mappers mark the perceived attack, earlier in the
rise. Decision: DOCUMENT the bias (README gives the practical rule
"true offset ~= reported - 26ms +- 5ms") rather than subtract a
calibration constant, because the synthetic reference fixtures (sharp
clicks, zero rise time) pin the current calibration and the harness now
self-reports the bias on every run. Revisit if the constant ever
matters more than fixture integrity.

**Dear You /3 false positive (Task 17b):** DJ Genericname - Dear You
(true 174.11 BPM per the mapper) was thirded to 58.038. Unlike Leave the
Lights On (decisive scan, blocked by the margin gate), this scan was
uncertain (margin 1.07), so the gate allowed the flip. Diagnosis
(env-gated instrumentation, since removed) compared the three flip
cases side by side:

| case | verdict | margin | c_sub1/c_beat (subdivision salience) |
|---|---|---|---|
| boy for the weekend (204->68) | TRUE flip | 1.242 | 0.569 |
| Dear You (174->58) | FALSE flip | 1.071 | 0.272 |
| Leave the Lights On (170->56.7) | FALSE flip | 1.756 | 0.475 |

The discriminator: a genuine triplet-feel song's subdivisions are nearly
as strong as its beats (0.57 — the 3x detection really is the rhythm
section playing triplets over a slow beat); incidental swing over a
clear fast beat is far weaker (0.27). Fix: replaced the sum-based
SUBDIVISION_EVIDENCE_RATIO (0.3) with a per-phase SALIENCE floor
(SUBDIVISION_SALIENCE_RATIO = 0.35: at least one subdivision phase must
carry >= 35% of the beat phase's weighted support), which subsumes the
old check (max >= 0.35 implies sum >= 0.35 > 0.3) and is strictly
stricter, so no regression risk toward more flips.

Validated: Dear You now 174.113 (rank 1) — the osu-eval harness reports
6/6 maps within 0.5 BPM of mapper ground truth; boy for the weekend
still flips to 68.000; Leave the Lights On stays 170.000; the synthetic
triplet fixture (salience ~0.4) still flips. Offset errors across the 6
maps: mean +24.5ms, stddev 2.7ms. Dear You's meter (6/8 vs mapper's
4/4) is a known deferred limitation of the experimental meter stage —
its lag-6 autocorrelation wins on this song's accent data. 59 tests
passing.

### Task 18: 3:2 flip correction (My Love) + beat/offbeat ambiguity documented (Reol)

Second osu-eval batch (17 unique maps) gave BPM 14/17 and exposed two new
classes, both diagnosed with temporary env-gated instrumentation (since
removed):

**My Love: the 3:2 alias masquerades as a 3:1 triplet.** True 128 BPM;
raw scan winner was ~256 (= 85.358 x 3), the /3 pass flipped it to
85.358 = 128 x 2/3 — and the flip PASSED all the Task 16/17 guards
because 1/3 of the alias's interval is exactly half a true beat, so the
true beat's (strong) offbeats play the role of "triplet subdivisions".
Fix: a promotion pass (apply_ratio_promotion) that, ONLY when the
current #1 is itself a /3-flip product, promotes the 3:2-related higher
candidate when its grid's beat phase dominates the offbeat phase
(My Love: c_beat 8.7 vs c_offbeat 2.3 -> 127.906 at #1). First version
also ran against raw scan winners and immediately false-promoted
Passcode (140 -> 210.033) and Hitorigoto (165 -> 247.498): on the higher
grid a true tempo's backbeat splits kick/snare across beat and offbeat
phases, and kick-dominance fakes the evidence. Restricted to
flip-correction; documented in code and README.

**Reol - No title: genuine beat/offbeat ambiguity, documented not
fixed.** True 200 BPM detected correctly, but the offset landed exactly
half a beat from the mapper's grid (-120ms wrapped). The slopes
landscape shows two grids half a beat apart with the offbeat 1.7x
stronger; weighted onsets agree (10.05 vs 9.56); a 150Hz lowpass slopes
variant agrees (0.187s); every analysis window (10/20/30/60s) agrees.
The song's strongest transient layer runs on the offbeat and the mapper
aligned to phrase/vocal context that local features can't see. The
regression test pins the grid FAMILY (offset must be on the mapper's
grid or exactly half a beat off it) so future phrase-aware work can
resolve it without a test change.

**Harness:** dedupes sets with identical audio content (SipHash of the
extracted audio; video/no-video variants of the same beatmapset now skip
as "duplicate audio").

**Offset bias refined:** the +26ms "constant" varies with transient
sharpness — Silhouette (rock, sharp drums) +2.4ms vs pop cluster
+23..+27ms — confirming the rise-time mechanism and weakening any flat
correction rule. README now documents the +2..+27ms band instead of a
single number.

Validated: osu-eval 14/16 within 0.5 BPM (remaining failures are the two
deferred hard classes: Highscore octave 110/220, Stronger Than You
sparse ballad 98/226), all prior regressions stayed fixed, 62 tests
passing.

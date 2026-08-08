# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A cross-platform, GPL-3.0-licensed Rust reimplementation of Bram van de
Wetering's tempo/BPM and beat-offset detection algorithm, documented in
`original/doc/syslab-version/paper.tex` and originally implemented in
`original/FindTempo_standalone.cpp` (also GPL, depends on `aubio`). It exists to
fix a macOS segfault in the reference and to drop the `aubio` dependency
(hard to build cross-platform), by reimplementing onset detection directly.

Every part of the algorithm is a faithful port of the reference C++,
**except** onset detection (`tempo-core/src/onset.rs`, since the reference
delegates to `aubio`) and the cubic polyfit solver (`tempo-core/src/polyfit.rs`,
same math, different solve method). See the "Notes on fidelity" section of
`README.md` for details before touching either module.

## Commands

```sh
# Build the CLI (release)
cargo build --release -p tempo-cli

# Run all tests (unit + end-to-end + real-song regression)
cargo test --workspace

# Run a single test
cargo test -p tempo-core some_test_name
cargo test -p tempo-cli some_test_name

# What CI runs (rust-ci.yml) — run these before considering work done
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Analyze a file
cargo run --release -p tempo-cli -- <file> [--min-bpm 40] [--max-bpm 260] [--start 0] [--duration 60] [--json] [--no-subharmonic-preference]
cargo run --release -p tempo-cli -- batch <folder> --out results.csv [--json]

# osu!-map ground-truth evaluation (local tool, not in CI — needs .osz files in osu-maps/, gitignored)
cargo run --release --bin osu_eval -- osu-maps --out report.csv
cargo run --release --bin osu_eval -- osu-maps --table
```

`cargo fmt` uses `rustfmt.toml` (2021 edition default settings, no
overrides beyond that). Real-song regression tests
(`crates/tempo-cli/tests/real_songs.rs`) reference copyrighted MP3
fixtures that aren't committed; they skip automatically when absent, so a
green `cargo test` locally without the fixtures doesn't exercise that net.

## Architecture

Two-crate workspace, split specifically so the algorithm can later target
WebAssembly:

- **`crates/tempo-core`**: the pure algorithm. No file I/O, no threads, no
  platform dependencies beyond `rustfft`. Takes mono `f32` PCM + sample
  rate, returns BPM/offset/fitness candidates. A future web version would
  decode with the Web Audio API in JS and hand PCM to this crate compiled
  to WASM — don't add I/O or threading here even if it would simplify
  something in `tempo-cli`.
- **`crates/tempo-cli`**: native CLI. Decodes audio with `symphonia` (pure
  Rust, no system libs) and calls into `tempo-core`. Also hosts the
  `osu_eval` second binary (`src/bin/osu_eval.rs`) used for ground-truth
  calibration against osu! beatmapsets.

### `tempo-core` pipeline (`src/lib.rs` orchestrates; each stage is a module)

1. **`onset.rs`** — complex-domain onset detection: FFT-based phase
   prediction per bin, prediction-error sum as the onset detection
   function (ODF), median-adaptive peak-picking with sub-hop parabolic
   refinement. This is the one stage NOT ported from the reference (which
   delegated to `aubio`); validated against synthetic click tracks, not
   against `aubio`'s numeric output.
2. **`interval.rs`** — coarse-to-fine scan over the interval range implied
   by `[min_bpm, max_bpm]`: coarse pass every 10th interval, then refines
   around promising peaks. Calls into `gapdata.rs` to score each candidate
   interval and `polyfit.rs` to refine the peak location.
3. **`gapdata.rs`** — gap-confidence scoring: histograms onset strengths
   wrapped modulo a candidate interval, scores concentration via a
   Hamming-windowed area-under-peak. This is the core fitness metric the
   whole scan is built on.
4. **`bpm.rs`** — top-level BPM detection: runs the interval scan,
   deduplicates near-identical/octave candidates, snaps near-integer BPMs,
   re-checks close top-2 candidates at full precision, returns top 3. Also
   where the beyond-reference passes live: subharmonic preference (1/3
   triplet-feel correction), harmonic-ratio demotion (4:3 secondary
   percussion), 3:2 flip correction, and octave preference (1/2, for when
   the scan locked the subdivision layer) — see `README.md` for the specific
   failure modes each one fixes and why they're gated the way they are
   (these gates exist because naive versions caused regressions on other
   songs; don't loosen them without re-running the real-song and osu-eval
   suites). `OCTAVE_DOMINANCE_RATIO` in particular looks absurdly high at
   2.6 — it is high because the two classes it separates were measured to
   OVERLAP (a song needing correction at 1.575, one that must not be
   touched at 1.585), so the pass is designed to abstain on everything but
   the unambiguous tail. Lowering it will wrongly halve correct tempos; the
   comment on the constant carries the full measurement table.
5. **`offset.rs`** — beat-offset detection per BPM candidate: waveform
   leading-edge energy (not onset voting) determines beat phase, which is
   what keeps offset correct on dense/quiet mixes where few onsets survive
   peak-picking.
6. **`meter.rs`** — experimental time-signature estimation (2/4, 3/4, 4/4,
   6/8, 12/8) from per-beat accent periodicity over the beat grid. Reports
   `"unknown"` rather than guessing when confidence < 0.3. See
   `docs/meter-estimation.md` for the full heuristic walkthrough and known
   failure modes.
7. **`window.rs`** — shared Hamming window helper used by `gapdata.rs`.

`DetectionContext` (from `detect_with_onsets`) threads whether the
subharmonic pass fired through to meter estimation, so a triplet-feel song
gets 6/8/12-8 notation instead of simple-meter notation.

### Known, documented limitations (don't "fix" without reading the README first)

- Offset has a systematic bias vs. human mapper ground truth (~+2ms sharp
  transients to ~+27ms soft transients) — both estimators center on a
  transient's full rise, humans mark perceived attack.
- Beat/offbeat ambiguity when the strongest transient layer runs on the
  offbeat — every local estimator locks the offbeat grid instead.
- These are tracked/quantified via `osu_eval`, not synthetic tests — the
  offset error distribution across many real maps is what calibrates
  confidence, not a handful of fixtures.

## Testing strategy (why three layers exist)

- **Unit tests** per module (polyfit, gap confidence, interval scanning,
  BPM candidate selection, offset detection, onset detection) — fast,
  deterministic, test the math in isolation.
- **`crates/tempo-core/tests/end_to_end.rs`** — synthetic click tracks with
  known BPM/offset, run through the full `detect()` pipeline. Tolerances
  are looser than unit tests because onset-detection jitter is included.
- **`crates/tempo-cli/tests/real_songs.rs`** — runs the compiled CLI binary
  against real tracks with user-confirmed ground truth (from external
  knowledge, not derived from this tool). This is the primary regression
  net for algorithm changes: synthetic fixtures can't reproduce real-music
  pathologies (triplet swing, busy secondary percussion, near-flat
  beat-level accents). Fixtures are gitignored (copyrighted); tests skip
  when absent rather than failing, so treat a pass on a fixture-less
  checkout as "didn't run," not "passed."

When changing anything in `bpm.rs`'s post-processing passes or
`offset.rs`, run both the real-song suite (if you have local fixtures) and
`osu_eval` against `osu-maps/` before considering the change validated —
unit and end-to-end tests alone have historically not been enough to catch
regressions on real music.

# tempo (Rust implementation)

A cross-platform, MIT-licensed reimplementation of the tempo/BPM and beat-
offset detection algorithm documented in
[`doc/syslab-version/paper.tex`](../doc/syslab-version/paper.tex) and
originally implemented in [`FindTempo_standalone.cpp`](../FindTempo_standalone.cpp)
(GPL, depends on `aubio`). This is an independent implementation, not a
translation of that GPL code, built to fix
[the macOS segfault](https://github.com/nathanstep55/bpm-offset-detector/issues/1)
(root cause: macOS's strict `aligned_alloc` returning `NULL` for
non-16-byte-aligned sizes, which the C++ never checks for) and to remove the
`aubio` dependency, which is difficult to build cross-platform.

See [`docs/superpowers/specs/2026-08-02-rust-tempo-detector-design.md`](../docs/superpowers/specs/2026-08-02-rust-tempo-detector-design.md)
for the full design.

## Structure

- **`crates/tempo-core`**: the pure algorithm. No file I/O, no threads, no
  platform dependencies beyond `rustfft` — takes mono `f32` PCM samples and a
  sample rate, returns BPM/offset/fitness candidates. This boundary is what
  makes a future WebAssembly build (via `wasm-bindgen`) straightforward: the
  web version would decode audio with the Web Audio API in JavaScript and
  hand the PCM buffer to this crate compiled to WASM.
- **`crates/tempo-cli`**: the native command-line tool. Decodes audio files
  with `symphonia` (pure Rust — no system libraries, unlike `aubio`) and
  calls into `tempo-core`.

## Building

```sh
cargo build --release -p tempo-cli
```

The resulting binary is at `target/release/tempo-cli` (`tempo-cli.exe` on
Windows). No system libraries are required on any platform.

## Usage

```sh
tempo-cli <file> [--min-bpm 40] [--max-bpm 260] [--start 0] [--duration 60] [--json]
                 [--no-subharmonic-preference]
tempo-cli batch <folder> --out results.csv [--json]
```

Features beyond the reference C++ implementation:

- **Wider default BPM range** (40–260 vs the reference's 89–205), so slow
  songs aren't reported as their in-range octave multiples.
- **Subharmonic preference**: triplet-feel songs (12/8, shuffle, slow jams)
  whose beats all align at 3x the true tempo are re-labeled to the true
  tempo when the accent evidence supports it (e.g. 68 BPM instead of
  203.978). The pass only overrides an uncertain scan (top-2 fitness
  margin < 1.5): a genuine 3x lock is structurally capped at a 4/3 margin
  by its half-tempo alias, so a decisive scan winner is trusted — this
  keeps true fast tempos with swing accents (e.g. a real 170 BPM song)
  from being thirded. Disable with `--no-subharmonic-preference` for raw
  reference behavior.
- **Harmonic-ratio demotion**: when the scan winner and another top
  candidate sit at a small-integer ratio (currently 4:3) and the accent
  evidence identifies the lower one as the beat, the lower candidate is
  promoted — fixes dense secondary percussion layers (e.g. a loop at 4
  hits per 3 beats, detected as 241 BPM) outscoring the true tempo
  (180 BPM) under the scan's accent-blind voting.
- **Waveform-based offset phase**: the beat offset is chosen by scanning
  the beat grid against raw waveform leading-edge energy rather than by
  voting detected onsets, so the offset stays correct even when onset
  detection is sparse (dense/quiet mixes where few onsets survive
  peak-picking). Offsets run ~25-30ms behind externally-measured ground
  truth on real audio (both estimators center on the transient's rise
  rather than its start) — a known, consistent calibration residual.
- **Experimental time-signature estimation**: each result includes a
  `time_signature` estimate (2/4, 3/4, 4/4, 6/8, 12/8) with a confidence
  score, derived from per-beat accent periodicity (mean-centered
  autocorrelation over the beat grid, with triplet-aware 6/8/12-8 notation
  when the BPM stage found compound-meter evidence). When the accent
  structure is too weak (confidence < 0.3 — common in pop, where drums hit
  every beat alike and bar accents live in other layers), the estimate is
  reported as `"unknown"` rather than guessing. Only the beats-per-bar
  grouping is estimated (4/4 vs 2/2 is notational, not acoustic).
- **Gapless decoding**: MP3 encoder delay/padding is trimmed, so reported
  offsets line up with the original source audio instead of being shifted
  late by 25-50ms.

## Testing

```sh
cargo test --workspace
```

Tests include unit tests per module (polyfit, gap confidence, interval
scanning, BPM candidate selection, offset detection, onset detection),
end-to-end tests (`crates/tempo-core/tests/end_to_end.rs`) that run the full
pipeline against synthetic click tracks with known BPM and offset, and a
real-song regression suite (`crates/tempo-cli/tests/real_songs.rs`) that
runs the compiled CLI against real tracks with user-confirmed ground
truth. The real-song fixtures are copyrighted and not committed; those
tests skip automatically when the files are absent (e.g. on CI).

## Notes on fidelity to the reference implementation

Every part of the algorithm is a faithful (if idiomatically-Rust) port of
`FindTempo_standalone.cpp`, **except**:

- **Onset detection** (`tempo-core/src/onset.rs`): the reference delegates
  this entirely to `aubio`'s `"complex"` onset method. This crate
  reimplements complex-domain onset detection directly (FFT-based phase
  prediction + adaptive peak-picking via `rustfft`), since porting `aubio`
  itself was explicitly out of scope. Its numerical output will not match
  `aubio` exactly; correctness is validated against synthetic click tracks
  with known onset positions instead.
- **Cubic polynomial fitting** (`tempo-core/src/polyfit.rs`): solves the same
  least-squares normal equations as the reference's `polyfit.h`, but via
  Gaussian elimination on a fixed 4x4 system instead of a general
  Givens-rotation QR decomposition (simpler to implement/verify for a fixed
  degree-3 fit; `polyfit.h` is also explicitly unlicensed in this
  repository's `README.md`, so this module is written from the underlying
  math rather than translated from it).

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
- **3:2 flip correction**: a 2/3-rate alias (e.g. 85.358 for a true 128
  BPM) can pass the /3 subharmonic checks because 1/3 of its interval is
  exactly half a true beat, making the true beat's offbeats look like
  triplet subdivisions. When such a flip produced the current #1, a
  promotion pass replaces it with the 3:2-related higher candidate if
  that grid's beat phase dominates its offbeat phase. Restricted to
  flip-correction because the same evidence is unsafe against raw scan
  winners (backbeat kick/snare splits fake it).
- **Waveform-based offset phase**: the beat offset is chosen by scanning
  the beat grid against raw waveform leading-edge energy rather than by
  voting detected onsets, so the offset stays correct even when onset
  detection is sparse (dense/quiet mixes where few onsets survive
  peak-picking).
- **Known offset limitations**: (a) systematic bias — reported offsets run
  late vs human mapper ground truth, varying with transient sharpness
  (≈ +2ms for sharp rock drums to ≈ +27ms for soft pop transients; both
  estimators center on a transient's full rise while humans mark the
  perceived attack); (b) beat/offbeat ambiguity — when a song's strongest
  transient layer runs on the offbeat, every local estimator (broadband
  slopes, weighted onsets, low-band slopes) locks the offbeat grid, half
  a beat away from the mapper's choice (observed on Reol - No title);
  resolving it needs phrase-level context we don't model.
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

## Evaluating against osu! maps (local tool)

`osu-eval` is a second binary that compares the detector against human
mapper ground truth from osu! beatmapsets. Download `.osz` files from
[osu.ppy.sh](https://osu.ppy.sh/beatmapsets) and drop them into
`rust/osu-maps/` (gitignored — `.osz` bundles copyrighted audio, so it
stays local-only, exactly like the MP3 fixtures):

```sh
cargo run --release --bin osu_eval -- osu-maps --out report.csv
# or, for terminal reading instead of a CSV file:
cargo run --release --bin osu_eval -- osu-maps --table
```

For each `.osz` it extracts the mapper's timing (first uninherited timing
point: BPM = 60000/beatLength, offset = time in ms), analyzes the bundled
audio, and writes a per-map CSV row (true vs detected BPM/offset/meter,
signed offset error wrapped to ±half a beat) plus summary statistics on
stderr. Maps with variable BPM (multiple uninherited timing points) are
skipped and logged, as are sets whose audio content duplicates an
already-analyzed set (e.g. video and no-video variants of the same map).
The offset-error distribution across many maps is how the systematic
offset bias gets calibrated with statistical confidence instead of 3
data points.

Report columns (CSV and `--table`; the table view drops `audio` and merges
the two meter columns into `true→detected`):

- `map` / `osz`: the beatmapset filename (table view truncates at 45 chars).
- `status`: `ok` = analyzed; `skipped: <reason>` = unusable (variable BPM,
  missing audio, decode failure).
- `true_bpm` / `detected_bpm` / `bpm_error`: mapper's BPM vs our #1
  candidate, and their difference.
- `true_bpm_rank` (`r` in the table): which position the true BPM holds in
  our top-3 (1 = correct outright); empty when it's absent from the top 3.
- `true_offset_ms`: mapper's first timing-point time, in ms (can exceed
  one beat interval).
- `detected_offset_ms`: our beat-grid phase, in ms, always modulo one beat
  interval — so it is NOT directly comparable to `true_offset_ms` by eye.
- `offset_error_ms` (`off_err`): signed error in ms, wrapped to ±half a
  beat. Empty when the BPM is wrong (an offset on the wrong grid is
  meaningless). Expect ≈ +26 (the documented systematic bias); a good row
  is `r=1`, `bpm_error` ≈ 0, `off_err` in the +21..+30 band.
- `meter_true` / `meter_detected` (`meter` as `true→detected` in the
  table): mapper's beats-per-bar vs our estimate; `-` = no estimate
  produced, `unknown` = abstained (confidence < 0.3).

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

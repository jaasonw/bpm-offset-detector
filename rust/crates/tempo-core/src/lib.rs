//! `tempo-core`: a pure, dependency-light implementation of Bram van de
//! Wetering's tempo (BPM) and beat-offset detection algorithm, as documented
//! in `doc/syslab-version/paper.tex` and originally implemented in
//! `FindTempo_standalone.cpp` (this crate is an independent Rust port, not a
//! translation of that GPL-licensed code).
//!
//! This crate has no file I/O and no threading dependency, so it compiles
//! cleanly to WebAssembly: callers supply already-decoded mono `f32` PCM
//! samples (e.g. from `symphonia` natively, or from the Web Audio API's
//! `decodeAudioData` in a future WASM build) and get back tempo candidates.

mod bpm;
mod gapdata;
mod interval;
mod offset;
mod onset;
mod polyfit;
mod window;

pub use polyfit::{polyfit_cubic, polyval};

/// A single detected onset (the start of a sound), at a sample position
/// with a strength weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Onset {
    /// Position of the onset, in samples from the start of the audio.
    pub pos: usize,
    /// Relative accent/salience of the onset (mean 1.0 across a detection
    /// run, from ODF peak height). The BPM scan deliberately ignores this
    /// and votes with constant weight (the paper found constant strengths
    /// more accurate for the scan); strengths are used only by the
    /// subharmonic preference pass and meter estimation, where accent
    /// structure is the relevant signal.
    pub strength: f64,
}

impl Onset {
    pub fn new(pos: usize, strength: f64) -> Self {
        Onset { pos, strength }
    }
}

/// One candidate tempo result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoResult {
    /// Beats per minute.
    pub bpm: f64,
    /// Position of the first beat, in seconds from the start of the audio.
    pub offset: f64,
    /// Relative confidence score (higher is better; not normalized to any
    /// fixed range).
    pub fitness: f64,
}

/// Options controlling tempo detection.
#[derive(Debug, Clone, Copy)]
pub struct DetectOptions {
    /// Slowest BPM considered. Default: 40.0 (widened from the reference
    /// implementation's 89.0 — slower songs otherwise get reported as their
    /// in-range octave multiples, e.g. a 68 BPM song detected as 136).
    pub min_bpm: f64,
    /// Fastest BPM considered. Default: 260.0 (reference used 205.0; 260
    /// keeps the shortest beat interval far above the 2048-sample analysis
    /// window, so there's no algorithmic constraint anywhere near this).
    pub max_bpm: f64,
    /// When true (default), a post-processing pass re-labels a winning
    /// candidate as its 1/3 subharmonic when the evidence supports it
    /// (accented beat phase dominates the subharmonic's phase clusters AND
    /// the 1/3 and 2/3 subdivision phases contain real onsets). This fixes
    /// triplet-feel songs being reported at 3x their true tempo. Only /3 is
    /// considered — /2 is deliberately excluded because even/odd backbeat
    /// accenting makes half-tempo intrinsically competitive on any
    /// even-tempo song, so a /2 rule cannot avoid false tempo-halving.
    pub subharmonic_preference: bool,
}

impl Default for DetectOptions {
    fn default() -> Self {
        DetectOptions {
            min_bpm: 40.0,
            max_bpm: 260.0,
            subharmonic_preference: true,
        }
    }
}

/// Detects tempo candidates (BPM + fitness only, `offset` left at `0.0`)
/// directly from a precomputed list of onsets. Useful for testing the
/// BPM-finding logic without decoding or analyzing real audio.
pub fn calculate_bpm(onsets: &[Onset], sample_rate: u32, opts: &DetectOptions) -> Vec<TempoResult> {
    bpm::calculate_bpm(onsets, sample_rate, opts)
}

/// Fills in the `offset` field of each candidate in `results`, given the
/// same onsets used to compute them and the raw mono samples they came
/// from (needed for offbeat disambiguation via waveform slope).
pub fn calculate_offset(
    samples: &[f32],
    sample_rate: u32,
    onsets: &[Onset],
    results: &mut [TempoResult],
) {
    offset::calculate_offset(samples, sample_rate, onsets, results);
}

/// Runs the full pipeline: onset detection (complex-domain) over `samples`,
/// then BPM detection, then offset detection. This is the primary entry
/// point for real audio.
pub fn detect(samples: &[f32], sample_rate: u32, opts: &DetectOptions) -> Vec<TempoResult> {
    let onsets = onset::find_onsets(samples, sample_rate);
    let mut results = calculate_bpm(&onsets, sample_rate, opts);
    calculate_offset(samples, sample_rate, &onsets, &mut results);
    results
}

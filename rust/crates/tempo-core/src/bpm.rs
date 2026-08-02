//! Top-level BPM detection: runs the coarse/refine interval scan, then
//! deduplicates near-identical and octave (half/double) candidates, snaps
//! near-integer BPMs to the integer when it doesn't hurt confidence, takes
//! a "second opinion" full-precision re-check when the top two candidates
//! are close, and returns the top 3.
//!
//! Ported from `CalculateBPM`, `RemoveDuplicates`, and `RoundBPMValues` in
//! `FindTempo_standalone.cpp`.

use crate::gapdata::GapData;
use crate::interval::scan_intervals;
use crate::{DetectOptions, Onset, TempoResult};

pub(crate) fn calculate_bpm(
    onsets: &[Onset],
    sample_rate: u32,
    opts: &DetectOptions,
) -> Vec<TempoResult> {
    // In order to determine the BPM, we need at least two onsets. Matches
    // the reference's fallback behavior rather than erroring.
    if onsets.len() < 2 {
        return vec![TempoResult {
            bpm: 100.0,
            offset: 0.0,
            fitness: 1.0,
        }];
    }

    let candidates = scan_intervals(onsets, sample_rate, opts.min_bpm, opts.max_bpm);
    let mut tempo: Vec<TempoResult> = candidates
        .into_iter()
        .map(|c| TempoResult {
            bpm: c.bpm,
            offset: 0.0,
            fitness: c.fitness,
        })
        .collect();

    tempo.sort_by(|a, b| b.fitness.total_cmp(&a.fitness));
    remove_duplicates(&mut tempo);

    // At this point we stop downsampling and upgrade to a full-precision
    // gap window for rounding / second-opinion checks.
    let max_interval = (sample_rate as f64 * 60.0 / opts.min_bpm + 0.5) as usize;
    let mut gapdata = GapData::new(max_interval, 0);
    round_bpm_values(&mut gapdata, onsets, sample_rate, &mut tempo);

    // If the fitness of the first and second option is very close, ask for
    // a second opinion at full precision.
    if tempo.len() >= 2 && tempo[0].fitness / tempo[1].fitness < 1.05 {
        for t in tempo.iter_mut() {
            let interval_f = sample_rate as f64 * 60.0 / t.bpm;
            t.fitness = gapdata.confidence_for_bpm(onsets, interval_f);
        }
        tempo.sort_by(|a, b| b.fitness.total_cmp(&a.fitness));
    }

    // In all 300 test cases in the original research the correct BPM value
    // was part of the top 3 choices, so anything beyond that is discarded.
    tempo.truncate(3);
    tempo
}

/// Removes BPM values that are near-duplicates or octave multiples (double
/// or half) of a higher-fitness candidate already kept.
fn remove_duplicates(tempo: &mut Vec<TempoResult>) {
    let mut i = 0;
    while i < tempo.len() {
        let bpm = tempo[i].bpm;
        let doubled = bpm * 2.0;
        let halved = bpm * 0.5;

        let mut j = tempo.len();
        while j > i + 1 {
            j -= 1;
            let v = tempo[j].bpm;
            let min_diff = (v - bpm)
                .abs()
                .min((v - doubled).abs())
                .min((v - halved).abs());
            if min_diff < 0.1 {
                tempo.remove(j);
            }
        }
        i += 1;
    }
}

/// Snaps BPM values that are close to an integer to that integer, when
/// doing so doesn't meaningfully reduce confidence.
fn round_bpm_values(
    gapdata: &mut GapData,
    onsets: &[Onset],
    sample_rate: u32,
    tempo: &mut [TempoResult],
) {
    for t in tempo.iter_mut() {
        let round_bpm = t.bpm.round();
        let diff = (t.bpm - round_bpm).abs();
        if diff < 0.01 {
            t.bpm = round_bpm;
        } else if diff < 0.05 {
            let old_interval = sample_rate as f64 * 60.0 / t.bpm;
            let new_interval = sample_rate as f64 * 60.0 / round_bpm;
            let old = gapdata.confidence_for_bpm(onsets, old_interval);
            let cur = gapdata.confidence_for_bpm(onsets, new_interval);
            if cur > old * 0.99 {
                t.bpm = round_bpm;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_train(interval: usize, count: usize) -> Vec<Onset> {
        (0..count)
            .map(|i| {
                let jitter = (i % 7) as i64 - 3;
                let pos = (i * interval) as i64 + jitter;
                Onset::new(pos.max(0) as usize, 1.0)
            })
            .collect()
    }

    #[test]
    fn too_few_onsets_falls_back_to_placeholder() {
        let opts = DetectOptions::default();
        let result = calculate_bpm(&[Onset::new(0, 1.0)], 44100, &opts);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].bpm, 100.0);
    }

    #[test]
    fn detects_known_bpm_from_click_train() {
        let sample_rate = 44100u32;
        let true_bpm = 120.0;
        let interval = (sample_rate as f64 * 60.0 / true_bpm).round() as usize;
        let onsets = click_train(interval, 60);
        let opts = DetectOptions::default();

        let results = calculate_bpm(&onsets, sample_rate, &opts);

        assert!(!results.is_empty());
        assert!(
            (results[0].bpm - true_bpm).abs() < 0.05,
            "top candidate bpm = {}, expected close to {true_bpm}",
            results[0].bpm
        );
        assert!(results.len() <= 3);
    }

    #[test]
    fn detects_known_fractional_bpm() {
        // 118.879 BPM is the "Move Your Feet" ground truth from the paper's
        // dataset (doc/syslab-version/paper.tex, Table tab:onsetmethods).
        let sample_rate = 44100u32;
        let true_bpm = 118.879;
        let interval = (sample_rate as f64 * 60.0 / true_bpm).round() as usize;
        let onsets = click_train(interval, 60);
        let opts = DetectOptions::default();

        let results = calculate_bpm(&onsets, sample_rate, &opts);

        assert!(
            (results[0].bpm - true_bpm).abs() < 0.1,
            "top candidate bpm = {}, expected close to {true_bpm}",
            results[0].bpm
        );
    }

    #[test]
    fn remove_duplicates_collapses_octave_and_near_duplicates() {
        let mut tempo = vec![
            TempoResult {
                bpm: 120.0,
                offset: 0.0,
                fitness: 10.0,
            },
            TempoResult {
                bpm: 240.0,
                offset: 0.0,
                fitness: 8.0,
            }, // double
            TempoResult {
                bpm: 60.0,
                offset: 0.0,
                fitness: 7.0,
            }, // half
            TempoResult {
                bpm: 120.05,
                offset: 0.0,
                fitness: 6.0,
            }, // near-duplicate
            TempoResult {
                bpm: 174.0,
                offset: 0.0,
                fitness: 5.0,
            }, // unrelated, kept
        ];
        remove_duplicates(&mut tempo);
        assert_eq!(tempo.len(), 2);
        assert_eq!(tempo[0].bpm, 120.0);
        assert_eq!(tempo[1].bpm, 174.0);
    }
}

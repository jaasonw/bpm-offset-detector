//! Coarse-to-fine interval scanning: finds candidate beat intervals (and
//! thus BPM values) by evaluating gap confidence across the full interval
//! range implied by `[min_bpm, max_bpm]`, first coarsely (every 10th
//! interval), then refining around promising coarse peaks.
//!
//! Ported from `FillCoarseIntervals`, `FillIntervalRange`,
//! `FindBestInterval`, and the scanning/normalization loop in
//! `CalculateBPM` in `FindTempo_standalone.cpp`.

use crate::gapdata::GapData;
use crate::polyfit::{polyfit_cubic, polyval};
use crate::Onset;

const INTERVAL_DELTA: usize = 10;
const INTERVAL_DOWNSAMPLE: u32 = 3;

/// One BPM candidate found during the coarse/refine scan, before
/// deduplication or rounding.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IntervalCandidate {
    pub(crate) bpm: f64,
    pub(crate) fitness: f64,
}

/// Scans the interval range implied by `[min_bpm, max_bpm]` for candidate
/// beat intervals: a coarse scan every `INTERVAL_DELTA`-th interval,
/// cubic-polyfit normalization of the coarse fitness curve, then
/// full-resolution refinement around every coarse peak that clears 40% of
/// the coarse maximum.
pub(crate) fn scan_intervals(
    onsets: &[Onset],
    sample_rate: u32,
    min_bpm: f64,
    max_bpm: f64,
) -> Vec<IntervalCandidate> {
    let min_interval = (sample_rate as f64 * 60.0 / max_bpm + 0.5) as usize;
    let max_interval = (sample_rate as f64 * 60.0 / min_bpm + 0.5) as usize;
    let num_intervals = max_interval - min_interval;

    let mut fitness = vec![0.0f64; num_intervals];
    let mut gapdata = GapData::new(max_interval, INTERVAL_DOWNSAMPLE);

    // Coarse scan: every INTERVAL_DELTA-th interval, raw (unnormalized)
    // confidence, floored at 0.001 so "unfilled" (0.0) is distinguishable
    // from "filled but low".
    let mut coarse_indices = Vec::new();
    let mut i = 0;
    while i < num_intervals {
        let interval = min_interval + i;
        let confidence = gapdata.confidence_for_interval(onsets, interval).max(0.001);
        fitness[i] = confidence;
        coarse_indices.push(i);
        i += INTERVAL_DELTA;
    }

    // Fit a cubic through the coarse (interval, raw fitness) points, then
    // subtract it from every coarse fitness value in place so comparisons
    // across the BPM range aren't biased by the curve's overall shape.
    let xs: Vec<f64> = coarse_indices
        .iter()
        .map(|&idx| (min_interval + idx) as f64)
        .collect();
    let ys: Vec<f64> = coarse_indices.iter().map(|&idx| fitness[idx]).collect();
    let coefs = polyfit_cubic(&xs, &ys);

    let mut max_fitness = 0.001f64;
    for &idx in &coarse_indices {
        let interval = (min_interval + idx) as f64;
        fitness[idx] -= polyval(&coefs, interval);
        max_fitness = max_fitness.max(fitness[idx]);
    }

    // Refine around every coarse peak whose normalized fitness clears 40%
    // of the coarse maximum.
    let fitness_threshold = max_fitness * 0.4;
    let mut candidates = Vec::new();
    for &idx in &coarse_indices {
        if fitness[idx] > fitness_threshold {
            let begin = idx.saturating_sub(INTERVAL_DELTA);
            let end = (idx + INTERVAL_DELTA).min(num_intervals);
            fill_interval_range(
                &mut fitness,
                &mut gapdata,
                onsets,
                min_interval,
                &coefs,
                begin,
                end,
            );
            let best = find_best_interval(&fitness, begin, end);
            candidates.push(IntervalCandidate {
                bpm: interval_to_bpm(sample_rate, min_interval, best),
                fitness: fitness[best],
            });
        }
    }

    candidates
}

/// Fills in any not-yet-computed (`== 0.0`) fitness entries in
/// `[begin, end)` at full resolution, normalizing each with the same cubic
/// fit used for the coarse scan and flooring at 0.1.
fn fill_interval_range(
    fitness: &mut [f64],
    gapdata: &mut GapData,
    onsets: &[Onset],
    min_interval: usize,
    coefs: &[f64],
    begin: usize,
    end: usize,
) {
    // `i` does double duty as both the fitness-array index and (via
    // `min_interval + i`) the interval value itself, so an
    // enumerate()-based rewrite wouldn't be clearer here.
    #[allow(clippy::needless_range_loop)]
    for i in begin..end {
        if fitness[i] == 0.0 {
            let interval = min_interval + i;
            let mut f = gapdata.confidence_for_interval(onsets, interval);
            f -= polyval(coefs, interval as f64);
            fitness[i] = f.max(0.1);
        }
    }
}

/// Returns the index in `[begin, end)` with the highest fitness. Defaults
/// to `begin` if every value in range is `<= 0.0` (this never happens in
/// practice, since `fill_interval_range`'s caller only refines around
/// indices already known to exceed a positive threshold, but `begin` is a
/// safe in-range default rather than the reference's `0`, which could be
/// outside `[begin, end)`).
fn find_best_interval(fitness: &[f64], begin: usize, end: usize) -> usize {
    let mut best = begin;
    let mut highest = 0.0f64;
    // `i` is returned as the result (the best index), not just used to
    // index `fitness`, so an enumerate()-based rewrite wouldn't be clearer.
    #[allow(clippy::needless_range_loop)]
    for i in begin..end {
        if fitness[i] > highest {
            highest = fitness[i];
            best = i;
        }
    }
    best
}

fn interval_to_bpm(sample_rate: u32, min_interval: usize, index: usize) -> f64 {
    (sample_rate as f64 * 60.0) / (index + min_interval) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_train(interval: usize, count: usize) -> Vec<Onset> {
        (0..count)
            .map(|i| {
                // Small deterministic jitter so the train isn't perfectly
                // periodic (see gapdata.rs tests for why this matters).
                let jitter = (i % 7) as i64 - 3;
                let pos = (i * interval) as i64 + jitter;
                Onset::new(pos.max(0) as usize, 1.0)
            })
            .collect()
    }

    #[test]
    fn finds_candidate_near_true_bpm() {
        let sample_rate = 44100u32;
        let true_bpm = 120.0;
        let interval = (sample_rate as f64 * 60.0 / true_bpm).round() as usize;
        let onsets = click_train(interval, 60);

        let candidates = scan_intervals(&onsets, sample_rate, 89.0, 205.0);

        assert!(!candidates.is_empty(), "expected at least one candidate");
        let best = candidates
            .iter()
            .max_by(|a, b| a.fitness.total_cmp(&b.fitness))
            .unwrap();
        assert!(
            (best.bpm - true_bpm).abs() < 1.0,
            "best candidate bpm = {}, expected close to {true_bpm}",
            best.bpm
        );
    }

    #[test]
    fn finds_candidate_near_true_bpm_at_high_tempo() {
        let sample_rate = 44100u32;
        let true_bpm = 174.0;
        let interval = (sample_rate as f64 * 60.0 / true_bpm).round() as usize;
        let onsets = click_train(interval, 60);

        let candidates = scan_intervals(&onsets, sample_rate, 89.0, 205.0);

        let best = candidates
            .iter()
            .max_by(|a, b| a.fitness.total_cmp(&b.fitness))
            .unwrap();
        assert!(
            (best.bpm - true_bpm).abs() < 1.0,
            "best candidate bpm = {}, expected close to {true_bpm}",
            best.bpm
        );
    }
}

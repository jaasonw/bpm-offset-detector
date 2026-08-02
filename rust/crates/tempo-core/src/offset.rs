//! Beat-offset detection: given BPM candidates and the onsets/raw samples
//! they were derived from, finds the position of the first beat for each
//! candidate.
//!
//! Ported from `GetBaseOffsetValue`, `ComputeSlopes`, `AdjustForOffbeats`,
//! and `CalculateOffset` in `FindTempo_standalone.cpp`. Per the design
//! doc's validation gate, this port is faithful to the reference algorithm;
//! whether it's reliable enough to ship as non-experimental is decided by
//! the synthetic end-to-end tests in `lib.rs`, since upstream's own offset
//! detection is documented as broken and of unknown cause.
//!
//! One deliberate deviation from the reference for efficiency (not
//! behavior): `ComputeSlopes` doesn't depend on BPM, so this port computes
//! it once and reuses it across every candidate instead of recomputing it
//! per candidate.

use crate::gapdata::GapData;
use crate::{Onset, TempoResult};

pub(crate) fn calculate_offset(
    samples: &[f32],
    sample_rate: u32,
    onsets: &[Onset],
    results: &mut [TempoResult],
) {
    if results.is_empty() {
        return;
    }

    let max_interval = results
        .iter()
        .map(|t| sample_rate as f64 * 60.0 / t.bpm)
        .fold(0.0f64, f64::max);
    let mut gapdata = GapData::new((max_interval + 1.0) as usize, 1);

    for t in results.iter_mut() {
        t.offset = base_offset_for_bpm(&mut gapdata, sample_rate, onsets, t.bpm);
    }

    let slopes = compute_slopes(samples, sample_rate);
    for t in results.iter_mut() {
        t.offset = adjust_for_offbeats(sample_rate, &slopes, t.offset, t.bpm);
    }
}

/// Finds the most-supported beat phase for `bpm`: builds a histogram of
/// onset counts (unweighted — every onset counts as `1.0` regardless of its
/// `strength`) wrapped by the BPM's fractional sample interval, and returns
/// the position with the highest Hamming-windowed gap confidence, in
/// seconds.
fn base_offset_for_bpm(gapdata: &mut GapData, sample_rate: u32, onsets: &[Onset], bpm: f64) -> f64 {
    let interval_f = sample_rate as f64 * 60.0 / bpm;
    let interval = (interval_f + 0.5) as usize;

    let histogram = gapdata.histogram_mut();
    histogram[..interval].fill(0.0);

    let mut wrapped_pos = Vec::with_capacity(onsets.len());
    for onset in onsets {
        let pos = (onset.pos as f64).rem_euclid(interval_f) as usize;
        let pos = pos.min(interval - 1);
        wrapped_pos.push(pos);
        histogram[pos] += 1.0;
    }

    let mut highest = 0.0f64;
    let mut offset_pos = 0usize;
    for &pos in &wrapped_pos {
        let mut confidence = gapdata.gap_confidence(pos, interval);
        let offbeat_pos = (pos + interval / 2) % interval;
        confidence += gapdata.gap_confidence(offbeat_pos, interval) * 0.5;
        if confidence > highest {
            highest = confidence;
            offset_pos = pos;
        }
    }

    offset_pos as f64 / sample_rate as f64
}

/// Creates a "leading-edge energy" representation of the waveform: at each
/// sample, the (clamped-to-nonnegative) difference between the average
/// absolute amplitude in a 50ms window just after it and a 50ms window just
/// before it. Onsets (sudden increases in energy) show up as positive
/// spikes; this is used to disambiguate a beat position from its offbeat
/// (half a beat later) by checking which one lands on more leading edges.
fn compute_slopes(samples: &[f32], sample_rate: u32) -> Vec<f64> {
    let num_frames = samples.len();
    let mut out = vec![0.0f64; num_frames];

    let half_window = (sample_rate / 20) as usize; // 50ms
    if half_window == 0 || num_frames < half_window * 2 {
        return out;
    }

    let mut sum_l = 0.0f64;
    let mut sum_r = 0.0f64;
    for i in 0..half_window {
        sum_l += (samples[i] as f64).abs();
        sum_r += (samples[i + half_window] as f64).abs();
    }

    let scalar = 1.0 / half_window as f64;
    let end = num_frames - half_window;
    for i in half_window..end {
        out[i] = ((sum_r - sum_l) * scalar).max(0.0);

        let cur = (samples[i] as f64).abs();
        sum_l -= (samples[i - half_window] as f64).abs();
        sum_l += cur;
        sum_r -= cur;
        sum_r += (samples[i + half_window] as f64).abs();
    }

    out
}

/// Compares a candidate offset to its offbeat (offset + half a beat,
/// wrapped) by summing the leading-edge energy (`slopes`) at every beat
/// position each implies, and returns whichever has more total support.
fn adjust_for_offbeats(sample_rate: u32, slopes: &[f64], offset: f64, bpm: f64) -> f64 {
    let seconds_per_beat = 60.0 / bpm;
    let mut offbeat = offset + seconds_per_beat * 0.5;
    if offbeat > seconds_per_beat {
        offbeat -= seconds_per_beat;
    }

    let end = slopes.len() as f64;
    let interval = seconds_per_beat * sample_rate as f64;
    let mut pos_a = offset * sample_rate as f64;
    let mut pos_b = offbeat * sample_rate as f64;
    let mut sum_a = 0.0f64;
    let mut sum_b = 0.0f64;

    while pos_a < end && pos_b < end {
        sum_a += slopes[pos_a as usize];
        sum_b += slopes[pos_b as usize];
        pos_a += interval;
        pos_b += interval;
    }

    if sum_a >= sum_b {
        offset
    } else {
        offbeat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a synthetic click track: silence with a short decaying pulse
    /// at each beat (so `compute_slopes` has real leading edges to find),
    /// starting at `offset_samples` and repeating every `interval` samples,
    /// with the same small deterministic jitter used elsewhere so the
    /// onsets aren't perfectly periodic.
    fn synthetic_click_signal(
        num_frames: usize,
        offset_samples: usize,
        interval: usize,
        click_len: usize,
    ) -> (Vec<f32>, Vec<Onset>) {
        let mut samples = vec![0.0f32; num_frames];
        let mut onsets = Vec::new();

        let mut k = 0i64;
        loop {
            let jitter = (k % 7) - 3;
            let pos = offset_samples as i64 + k * interval as i64 + jitter;
            if pos < 0 {
                k += 1;
                continue;
            }
            let pos = pos as usize;
            if pos + click_len >= num_frames {
                break;
            }
            for i in 0..click_len {
                samples[pos + i] = (1.0 - i as f32 / click_len as f32).max(0.0);
            }
            onsets.push(Onset::new(pos, 1.0));
            k += 1;
        }

        (samples, onsets)
    }

    #[test]
    fn recovers_known_offset() {
        let sample_rate = 44100u32;
        let bpm = 120.0;
        let interval = (sample_rate as f64 * 60.0 / bpm).round() as usize;
        let true_offset_secs = 0.15; // deliberately not half the beat period (0.25s), to avoid offbeat ambiguity
        let offset_samples = (true_offset_secs * sample_rate as f64).round() as usize;
        let num_frames = interval * 60 + offset_samples + 1000;

        let (samples, onsets) = synthetic_click_signal(num_frames, offset_samples, interval, 500);

        let mut results = vec![TempoResult {
            bpm,
            offset: 0.0,
            fitness: 1.0,
        }];
        calculate_offset(&samples, sample_rate, &onsets, &mut results);

        let error = (results[0].offset - true_offset_secs).abs();
        assert!(
            error < 0.005,
            "offset = {}, expected close to {true_offset_secs} (error {error}s)",
            results[0].offset
        );
    }

    #[test]
    fn recovers_known_offset_at_different_bpm() {
        let sample_rate = 44100u32;
        let bpm = 162.0;
        let interval = (sample_rate as f64 * 60.0 / bpm).round() as usize;
        let true_offset_secs = 0.09;
        let offset_samples = (true_offset_secs * sample_rate as f64).round() as usize;
        let num_frames = interval * 60 + offset_samples + 1000;

        let (samples, onsets) = synthetic_click_signal(num_frames, offset_samples, interval, 500);

        let mut results = vec![TempoResult {
            bpm,
            offset: 0.0,
            fitness: 1.0,
        }];
        calculate_offset(&samples, sample_rate, &onsets, &mut results);

        let error = (results[0].offset - true_offset_secs).abs();
        assert!(
            error < 0.005,
            "offset = {}, expected close to {true_offset_secs} (error {error}s)",
            results[0].offset
        );
    }

    #[test]
    fn empty_results_is_a_noop() {
        let mut results: Vec<TempoResult> = Vec::new();
        calculate_offset(&[0.0; 1000], 44100, &[], &mut results);
        assert!(results.is_empty());
    }
}

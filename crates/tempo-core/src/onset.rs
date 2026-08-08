//! Complex-domain onset detection.
//!
//! Reimplements (not a line-for-line port of, since the reference C++
//! delegates this to `aubio`) the "complex domain" onset detection method:
//! per FFT bin, predicts the next frame's complex spectrum from the
//! magnitude of the previous frame and a linear extrapolation of the
//! previous two frames' phase, and sums the magnitude of the prediction
//! error across bins as the onset detection function (ODF). Peaks in a
//! median-adaptive-thresholded ODF, subject to a minimum inter-onset
//! interval, become onsets, with sub-hop position refinement via parabolic
//! interpolation of the ODF around each peak.

use crate::Onset;
use rustfft::num_complex::Complex64;
use rustfft::FftPlanner;

const WINDOW_SIZE: usize = 1024;
const HOP_SIZE: usize = WINDOW_SIZE / 4; // 256
const MEDIAN_RADIUS: usize = 3; // 7-frame window for the adaptive median threshold
const THRESHOLD_MULTIPLIER: f64 = 1.5;
const ODF_FLOOR: f64 = 1e-6; // avoids picking noise as "peaks" in near-silent audio
const MIN_INTER_ONSET_SECONDS: f64 = 0.02; // 20ms, matches aubio's default minimum inter-onset interval
/// STFT-based onset detection has inherent analysis latency: the ODF only
/// reacts once a transient has entered the (1024-sample) analysis window,
/// so the frame that wins the peak-picking competition is systematically
/// *earlier*, in raw `frame_index * HOP_SIZE` terms, than the transient
/// itself (the window covering the transient starts before the transient
/// occurs). Empirically calibrated against synthetic click transients (see
/// `onset::tests::detects_onsets_near_true_click_positions`): reported
/// positions are corrected forward by this many samples. Residual jitter
/// of about one hop (256 samples, ~5.8ms at 44.1kHz) after correction is
/// expected and is well within what the downstream Hamming-windowed gap
/// confidence (window half-width up to 1024 samples) tolerates.
const ANALYSIS_DELAY_SAMPLES: f64 = 600.0;

pub(crate) fn find_onsets(samples: &[f32], sample_rate: u32) -> Vec<Onset> {
    let odf = onset_detection_function(samples);
    let mut onsets = pick_peaks(&odf, sample_rate);

    // Normalize ODF peak heights to a mean strength of 1.0, so strengths
    // express relative accent (this onset vs typical onsets in the same
    // audio) rather than an absolute spectral magnitude. The main pipeline
    // deliberately uses constant weights (see gapdata.rs); strengths exist
    // for the subharmonic preference pass and meter estimation.
    if !onsets.is_empty() {
        let mean = onsets.iter().map(|o| o.strength).sum::<f64>() / onsets.len() as f64;
        if mean > 0.0 {
            for o in onsets.iter_mut() {
                o.strength /= mean;
            }
        }
    }

    onsets
}

/// Computes the complex-domain onset detection function, one value per hop.
fn onset_detection_function(samples: &[f32]) -> Vec<f64> {
    let num_frames = samples.len();
    if num_frames < WINDOW_SIZE {
        return Vec::new();
    }

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(WINDOW_SIZE);

    let mut window = vec![0.0f64; WINDOW_SIZE];
    hann_window(&mut window);

    let num_bins = WINDOW_SIZE / 2 + 1;
    let mut prev_mag = vec![0.0f64; num_bins];
    let mut prev_phase = vec![0.0f64; num_bins];
    let mut prev_prev_phase = vec![0.0f64; num_bins];

    let mut odf = Vec::new();
    let mut pos = 0usize;
    let mut frame_index = 0usize;
    while pos + WINDOW_SIZE <= num_frames {
        let mut buffer: Vec<Complex64> = (0..WINDOW_SIZE)
            .map(|i| Complex64::new(samples[pos + i] as f64 * window[i], 0.0))
            .collect();
        fft.process(&mut buffer);

        let mut mag = vec![0.0f64; num_bins];
        let mut phase = vec![0.0f64; num_bins];
        for k in 0..num_bins {
            mag[k] = buffer[k].norm();
            phase[k] = buffer[k].arg();
        }

        // Need two prior frames (n-1, n-2) to linearly extrapolate phase,
        // so the first two frames contribute no ODF value.
        let value = if frame_index >= 2 {
            let mut sum = 0.0f64;
            for k in 0..num_bins {
                let predicted_phase = 2.0 * prev_phase[k] - prev_prev_phase[k];
                let predicted = Complex64::from_polar(prev_mag[k], predicted_phase);
                let actual = Complex64::from_polar(mag[k], phase[k]);
                sum += (actual - predicted).norm();
            }
            sum
        } else {
            0.0
        };
        odf.push(value);

        // Shift frame history: this frame becomes "n-1" and the old "n-1"
        // becomes "n-2" for the next iteration.
        prev_prev_phase = std::mem::replace(&mut prev_phase, phase);
        prev_mag = mag;

        pos += HOP_SIZE;
        frame_index += 1;
    }

    odf
}

fn hann_window(out: &mut [f64]) {
    let n = out.len();
    for (i, v) in out.iter_mut().enumerate() {
        *v = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
    }
}

/// Picks peaks in `odf` using a median-filtered adaptive threshold, a
/// wide-window local maximum check, and minimum inter-onset interval,
/// converting frame indices to sample positions (with parabolic sub-hop
/// refinement).
///
/// The local-max window matches the minimum inter-onset radius rather than
/// just the immediate neighbors: the complex-domain ODF "rings" for 1-2
/// frames after a genuine transient (a second, smaller bump right after
/// the true peak), and a narrow immediate-neighbor local-max check can let
/// that secondary bump register as its own local max, get accepted first,
/// and then have the minimum-inter-onset filter suppress the *real* peak
/// that follows. Requiring `odf[n]` to be the maximum over a wider window
/// makes only the single dominant peak per transient pass, independent of
/// the minimum-inter-onset filter (which still guards against separate,
/// genuinely close-together onsets from different transients).
fn pick_peaks(odf: &[f64], sample_rate: u32) -> Vec<Onset> {
    if odf.len() < MEDIAN_RADIUS * 2 + 3 {
        return Vec::new();
    }

    let min_inter_onset_hops = ((sample_rate as f64 * MIN_INTER_ONSET_SECONDS) / HOP_SIZE as f64)
        .round()
        .max(1.0) as usize;
    let peak_radius = min_inter_onset_hops;

    let mut onsets = Vec::new();
    let mut last_onset_frame: Option<usize> = None;

    for n in 1..odf.len() - 1 {
        // The adaptive threshold is computed from the *surrounding* context,
        // excluding `odf[n]` itself: including the candidate sample would
        // let a genuinely large peak inflate its own threshold (especially
        // with the ODF's post-transient ringing keeping nearby frames
        // elevated too), causing real onsets to fail their own threshold
        // check.
        let median_lo = n.saturating_sub(MEDIAN_RADIUS);
        let median_hi = (n + MEDIAN_RADIUS + 1).min(odf.len());
        let mut context: Vec<f64> = odf[median_lo..median_hi].to_vec();
        context.remove(n - median_lo);
        let threshold = median(&context) * THRESHOLD_MULTIPLIER + ODF_FLOOR;

        let peak_lo = n.saturating_sub(peak_radius);
        let peak_hi = (n + peak_radius + 1).min(odf.len());
        let is_local_max = odf[peak_lo..peak_hi].iter().all(|&v| v <= odf[n]);

        let clears_threshold = odf[n] > threshold;
        let clears_min_ioi = last_onset_frame.is_none_or(|last| n - last >= min_inter_onset_hops);

        if is_local_max && clears_threshold && clears_min_ioi {
            let frac = parabolic_offset(odf[n - 1], odf[n], odf[n + 1]);
            let frame_pos = (n as f64 + frac) * HOP_SIZE as f64 + ANALYSIS_DELAY_SAMPLES;
            // Strength = ODF peak height (normalized later by find_onsets);
            // it approximates the onset's accent/perceptual salience.
            onsets.push(Onset::new(frame_pos.max(0.0) as usize, odf[n]));
            last_onset_frame = Some(n);
        }
    }

    onsets
}

fn median(values: &[f64]) -> f64 {
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

/// Estimates the sub-frame peak offset (roughly in `[-0.5, 0.5]` hops) via
/// a parabola fit through three consecutive ODF values centered on a local
/// maximum.
fn parabolic_offset(y_prev: f64, y_curr: f64, y_next: f64) -> f64 {
    let denom = y_prev - 2.0 * y_curr + y_next;
    if denom.abs() < 1e-12 {
        0.0
    } else {
        0.5 * (y_prev - y_next) / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Silence with a short decaying pulse (sharp attack, smooth linear
    /// decay) at each position in `positions` — a strong broadband
    /// transient at the attack, with no secondary transient at the
    /// (gradual) release.
    fn click_signal(num_frames: usize, positions: &[usize], click_len: usize) -> Vec<f32> {
        let mut samples = vec![0.0f32; num_frames];
        for &pos in positions {
            for i in 0..click_len {
                if pos + i < num_frames {
                    samples[pos + i] = (1.0 - i as f32 / click_len as f32).max(0.0);
                }
            }
        }
        samples
    }

    #[test]
    fn detects_no_onsets_in_silence() {
        let samples = vec![0.0f32; 44100];
        let onsets = find_onsets(&samples, 44100);
        assert!(
            onsets.is_empty(),
            "expected no onsets in silence, got {}",
            onsets.len()
        );
    }

    #[test]
    fn detects_onsets_near_true_click_positions() {
        let sample_rate = 44100u32;
        let interval = 20000usize;
        let true_positions: Vec<usize> = (0..10).map(|i| 5000 + i * interval).collect();
        let num_frames = true_positions.last().unwrap() + interval;
        let samples = click_signal(num_frames, &true_positions, 400);

        let onsets = find_onsets(&samples, sample_rate);

        assert!(
            onsets.len() >= true_positions.len() - 1 && onsets.len() <= true_positions.len() + 1,
            "expected close to {} onsets, got {}",
            true_positions.len(),
            onsets.len()
        );

        // Every true click position should have a detected onset within
        // about 1.5 hops (384 samples, ~8.7ms at 44.1kHz) after analysis-
        // delay correction — see `ANALYSIS_DELAY_SAMPLES`'s doc comment for
        // why some residual jitter on this order is expected.
        for &true_pos in &true_positions {
            let closest = onsets
                .iter()
                .map(|o| (o.pos as i64 - true_pos as i64).abs())
                .min()
                .unwrap();
            assert!(
                closest <= (HOP_SIZE as i64 * 3 / 2),
                "no detected onset within 1.5 hops of true position {true_pos} (closest = {closest})"
            );
        }
    }
}

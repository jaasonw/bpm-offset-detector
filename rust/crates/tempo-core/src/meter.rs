//! Experimental time signature (meter) estimation.
//!
//! Given the beat grid (from BPM + offset) and the onsets, estimates how
//! beats group into bars by measuring per-beat accent (summed onset
//! strengths near each beat, falling back to waveform leading-edge energy
//! when onset strengths carry no information) and scoring grouping
//! hypotheses g in {2, 3, 4, 6, 12} by how strongly one phase of the
//! g-cycle stands out above the rest.
//!
//! This is inherently heuristic — meter estimation from audio is an open
//! research problem, and even good systems make mistakes on syncopated or
//! weakly-accented music. Only the beats-per-bar grouping is estimated:
//! the denominator (4 vs 8) is notational rather than acoustic, and 6/8 vs
//! 3/4 is reported as ambiguous when the two score too close to separate.
//! Treat the output as a hint, not ground truth.

use crate::offset::compute_slopes;
use crate::Onset;

/// Candidate beat groupings evaluated (simple meters 2/3/4 and compound
/// 6/8, 12/8). Odd meters (5/4, 7/8) are out of scope for v1.
const GROUPINGS: [usize; 5] = [2, 3, 4, 6, 12];

/// Minimum number of complete bars required before a grouping is scored.
const MIN_BARS: usize = 4;

/// Fraction of the beat interval on either side of a beat position within
/// which onsets count toward that beat's accent.
const ACCENT_WINDOW_FRACTION: f64 = 0.08;

/// A grouping is flagged ambiguous when the runner-up scores at least this
/// fraction of the winner (covers the classic 6/8-vs-3/4 case, which a
/// plain strong-weak-weak accent pattern cannot distinguish).
const AMBIGUITY_RATIO: f64 = 0.8;

/// An estimated time signature. Experimental — see module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterEstimate {
    /// Estimated beats per bar (2, 3, 4, 6, or 12).
    pub beats_per_bar: usize,
    /// Conventional notation for the estimate (e.g. "3/4", "6/8"). The
    /// denominator is a convention, not a measured quantity.
    pub notation: String,
    /// Heuristic confidence in [0, 1]: the normalized contrast of the
    /// winning phase. Below ~0.3 the estimate is unreliable.
    pub confidence: f64,
    /// True when the runner-up grouping scored nearly as well (>= 80%),
    /// most notably for 6/8 vs 3/4.
    pub ambiguous: bool,
}

/// Estimates the time signature from the beat grid implied by `bpm` and
/// `offset` over the given audio. Returns `None` when there's too little
/// data (few beats, or no measurable accent energy at all).
pub(crate) fn estimate_meter(
    onsets: &[Onset],
    samples: &[f32],
    sample_rate: u32,
    bpm: f64,
    offset: f64,
) -> Option<MeterEstimate> {
    if bpm <= 0.0 {
        return None;
    }
    let interval = sample_rate as f64 * 60.0 / bpm;
    let first_beat = offset * sample_rate as f64;

    // Build the beat grid over the audio.
    let mut beats = Vec::new();
    let mut pos = first_beat;
    while pos >= interval {
        pos -= interval; // include beats before the offset if any fit
    }
    while pos < 0.0 {
        pos += interval;
    }
    while (pos as usize) < samples.len() {
        beats.push(pos as usize);
        pos += interval;
    }

    // Restrict the grid to the region actually covered by onsets: beats
    // before the first or after the last detected onset have no accent
    // data, and their zero scores would pollute the phase statistics
    // (e.g. a fade-out ending would systematically dilute the true
    // grouping's best phase while a multiple grouping finds a clean one).
    if !onsets.is_empty() {
        let first_pos = onsets.iter().map(|o| o.pos).min().unwrap();
        let last_pos = onsets.iter().map(|o| o.pos).max().unwrap();
        beats.retain(|&b| b >= first_pos && b <= last_pos);
    }

    // Need enough beats for at least the smallest grouping to have
    // MIN_BARS complete bars.
    if beats.len() < MIN_BARS * GROUPINGS[0] {
        return None;
    }

    let accents = beat_accents(onsets, samples, sample_rate, &beats, interval);

    let overall_mean = accents.iter().sum::<f64>() / accents.len() as f64;
    if overall_mean <= 1e-9 {
        return None; // no accent energy anywhere — nothing to group
    }

    // Score each grouping by normalized contrast of its strongest phase.
    // A pattern with period g is mathematically also periodic at every
    // multiple of g, so multiples can TIE the true grouping exactly; ties
    // are broken toward the smaller grouping (Occam), and only strictly
    // worse runner-ups count toward the ambiguity flag.
    let mut best: Option<(usize, f64)> = None;
    let mut second: Option<(usize, f64)> = None;
    for &g in &GROUPINGS {
        if beats.len() < MIN_BARS * g {
            continue;
        }
        let mut phase_sums = vec![0.0f64; g];
        let mut phase_counts = vec![0usize; g];
        for (k, &a) in accents.iter().enumerate() {
            phase_sums[k % g] += a;
            phase_counts[k % g] += 1;
        }
        let max_phase_mean = (0..g)
            .map(|p| phase_sums[p] / phase_counts[p].max(1) as f64)
            .fold(0.0f64, f64::max);
        let contrast = (max_phase_mean - overall_mean) / overall_mean;

        if contrast <= 0.0 {
            continue;
        }
        match best {
            None => best = Some((g, contrast)),
            Some((bg, bc)) => {
                if contrast > bc + 1e-9 {
                    second = Some((bg, bc));
                    best = Some((g, contrast));
                } else if contrast < bc - 1e-9 && second.is_none_or(|(_, sc)| contrast > sc) {
                    second = Some((g, contrast));
                }
                // Exact tie with the best: keep the earlier (smaller)
                // grouping and don't record it as a runner-up.
            }
        }
    }

    let (g, contrast) = best?;
    let ambiguous = second.is_some_and(|(_, c)| c >= AMBIGUITY_RATIO * contrast);

    Some(MeterEstimate {
        beats_per_bar: g,
        notation: notation_for(g),
        confidence: contrast.clamp(0.0, 1.0),
        ambiguous,
    })
}

/// Computes an accent score per beat: summed onset strengths within
/// +/- ACCENT_WINDOW_FRACTION of the beat interval around each beat
/// position. If onset strengths carry no information (all equal, e.g.
/// hand-built onset lists), falls back to waveform leading-edge energy at
/// the beat positions.
fn beat_accents(
    onsets: &[Onset],
    samples: &[f32],
    sample_rate: u32,
    beats: &[usize],
    interval: f64,
) -> Vec<f64> {
    let strengths_uniform = onsets
        .iter()
        .map(|o| o.strength)
        .fold(None::<(f64, f64)>, |acc, s| {
            Some(match acc {
                None => (s, s),
                Some((lo, hi)) => (lo.min(s), hi.max(s)),
            })
        })
        .is_none_or(|(lo, hi)| hi - lo < 0.05);

    if strengths_uniform && !samples.is_empty() {
        let slopes = compute_slopes(samples, sample_rate);
        return beats
            .iter()
            .map(|&b| if b < slopes.len() { slopes[b] } else { 0.0 })
            .collect();
    }

    let window = interval * ACCENT_WINDOW_FRACTION;
    beats
        .iter()
        .map(|&b| {
            onsets
                .iter()
                .filter(|o| (o.pos as f64 - b as f64).abs() <= window)
                .map(|o| o.strength)
                .sum()
        })
        .collect()
}

fn notation_for(beats_per_bar: usize) -> String {
    match beats_per_bar {
        2 => "2/4",
        3 => "3/4",
        4 => "4/4",
        6 => "6/8",
        12 => "12/8",
        _ => "unknown",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44100;

    /// Builds an onset train at exactly `bpm` with strengths following
    /// `pattern` cyclically (e.g. [2,1,1] = waltz), plus matching audio for
    /// the fallback path.
    fn accent_train(
        bpm: f64,
        num_beats: usize,
        pattern: &[f64],
    ) -> (Vec<Onset>, Vec<f32>, f64, f64) {
        let interval_f = SR as f64 * 60.0 / bpm;
        let offset = 0.1; // seconds
        let num_frames = (offset * SR as f64 + num_beats as f64 * interval_f + interval_f) as usize;
        let mut samples = vec![0.0f32; num_frames];
        let mut onsets = Vec::new();

        for k in 0..num_beats {
            let jitter = (k % 7) as i64 - 3;
            let pos = (offset * SR as f64 + k as f64 * interval_f).round() as i64 + jitter;
            let pos = pos.max(0) as usize;
            let strength = pattern[k % pattern.len()];
            onsets.push(Onset::new(pos, strength));
            // Short decaying click scaled by the accent, for the fallback path.
            let click_len = 300;
            for i in 0..click_len {
                if pos + i < num_frames {
                    samples[pos + i] =
                        (strength as f32) * (1.0 - i as f32 / click_len as f32).max(0.0);
                }
            }
        }

        (onsets, samples, bpm, offset)
    }

    #[test]
    fn four_four_accent_pattern_reports_four_four() {
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[2.0, 1.0, 1.5, 1.0]);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset).expect("expected an estimate");
        assert_eq!(est.notation, "4/4");
        assert!(est.confidence > 0.3, "confidence {}", est.confidence);
        assert!(!est.ambiguous);
    }

    #[test]
    fn three_four_accent_pattern_reports_three_four() {
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[2.0, 1.0, 1.0]);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset).expect("expected an estimate");
        assert_eq!(est.notation, "3/4");
        assert!(est.confidence > 0.3, "confidence {}", est.confidence);
        // A plain strong-weak-weak pattern is ALSO consistent with 6/8, so
        // ambiguity is acceptable (and honest) here.
    }

    #[test]
    fn two_four_accent_pattern_reports_two_four() {
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[2.0, 1.0]);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset).expect("expected an estimate");
        assert_eq!(est.notation, "2/4");
    }

    #[test]
    fn six_eight_accent_pattern_reports_six_eight() {
        // [S w w s w w]: two groups of three with a secondary accent —
        // distinguishable from a plain 3/4 waltz pattern.
        let (onsets, samples, bpm, offset) =
            accent_train(120.0, 48, &[2.0, 1.0, 1.0, 1.4, 1.0, 1.0]);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset).expect("expected an estimate");
        assert_eq!(est.notation, "6/8");
        assert!(!est.ambiguous);
    }

    #[test]
    fn uniform_accents_report_low_confidence() {
        // Equal-strength onsets and equal-energy clicks: no metrical
        // structure, so the estimate must not be confident.
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[1.0]);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset);
        match est {
            None => {} // declining to estimate is also acceptable
            Some(est) => assert!(
                est.confidence < 0.3,
                "uniform accents should not produce a confident estimate, got {}",
                est.confidence
            ),
        }
    }

    #[test]
    fn too_few_beats_returns_none() {
        let (onsets, samples, bpm, offset) = accent_train(120.0, 4, &[2.0, 1.0]);
        assert!(estimate_meter(&onsets, &samples, SR, bpm, offset).is_none());
    }
}

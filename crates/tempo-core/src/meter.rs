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

/// The winning grouping is the smallest lag whose autocorrelation score is
/// within this margin of the best score — a period-g pattern also
/// correlates at multiples of g (tying exactly for clean patterns, landing
/// within noise of the max for real ones), and the fundamental is the
/// musically meaningful one.
const TIE_MARGIN: f64 = 0.05;

/// Below this autocorrelation score there is no meaningful metrical
/// structure in the beat-level accents, so the estimate is reported as
/// "unknown" instead of guessing (real pop often has near-flat beat-level
/// accents: drums hit every beat alike, and bar accents live in the bass/
/// harmony/vocals rather than in onset strength).
const CONFIDENCE_FLOOR: f64 = 0.3;

/// An estimated time signature. Experimental — see module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterEstimate {
    /// Estimated beats per bar (2, 3, 4, 6, or 12), or 0 when the accent
    /// structure was too weak to group ("unknown").
    pub beats_per_bar: usize,
    /// Conventional notation for the estimate (e.g. "3/4", "6/8"), or
    /// "unknown" when `confidence` is below the reliability floor. The
    /// denominator is a convention, not a measured quantity.
    pub notation: String,
    /// Heuristic confidence in [0, 1]: the autocorrelation score of the
    /// winning grouping. Below ~0.3 the estimate is "unknown".
    pub confidence: f64,
    /// True when the runner-up grouping scored nearly as well (>= 80%),
    /// most notably for 6/8 vs 3/4.
    pub ambiguous: bool,
}

/// Estimates the time signature from the beat grid implied by `bpm` and
/// `offset` over the given audio. Returns `None` when there's too little
/// data (few beats, or no measurable accent energy at all).
///
/// `triplet_feel` should come from the BPM stage's subharmonic preference
/// pass (see `DetectionContext`): when the song is known to have triplet
/// subdivisions, a winning 2-beat grouping is reported as 6/8 and a
/// winning 4-beat grouping as 12/8, since the beat unit is compound.
pub(crate) fn estimate_meter(
    onsets: &[Onset],
    samples: &[f32],
    sample_rate: u32,
    bpm: f64,
    offset: f64,
    triplet_feel: bool,
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

    // Mean-center the accent sequence and compute its zero-lag energy, for
    // autocorrelation scoring. Pure noise scores ~0 at every lag, so no
    // grouping can win by cherry-picking (see the regression tests).
    let mean = accents.iter().sum::<f64>() / accents.len() as f64;
    let centered: Vec<f64> = accents.iter().map(|a| a - mean).collect();
    let energy: f64 = centered.iter().map(|a| a * a).sum();
    if energy <= 1e-9 {
        return None; // uniform accents: no metrical structure to find
    }

    // Score each candidate grouping by mean-centered autocorrelation at
    // that lag, normalized by the zero-lag energy (score in [-1, 1]). This
    // replaces the original best-phase-contrast scorer, which took the max
    // over g phase means — a multiple-comparison contest that the largest
    // grouping (12) almost always won on real (noisy) accent data,
    // reporting 12/8 for every song regardless of true meter.
    let mut scores: Vec<(usize, f64)> = Vec::new();
    for &g in &GROUPINGS {
        if beats.len() < MIN_BARS * g {
            continue;
        }
        let mut numerator = 0.0f64;
        for k in 0..centered.len() - g {
            numerator += centered[k] * centered[k + g];
        }
        scores.push((g, numerator / energy));
    }

    // The winner is the SMALLEST lag within TIE_MARGIN of the best score:
    // a pattern with period g also correlates at multiples of g (which tie
    // it exactly for clean patterns, and land within noise of it for real
    // ones), and the fundamental is the musically meaningful grouping.
    let max_score = scores.iter().map(|&(_, s)| s).fold(0.0f64, f64::max);
    if max_score <= 0.0 {
        return None;
    }
    let (g, winner_score) = scores
        .iter()
        .find(|&&(_, s)| s >= max_score - TIE_MARGIN)
        .map(|&(g, s)| (g, s))?;

    // Decline rather than guess: on real pop audio, beat-level accent
    // structure is often simply absent (drums hit every beat with similar
    // strength, and bar-level accents live in other musical layers), in
    // which case the best score is low and reporting a grouping would be
    // printing noise. Below the floor, report "unknown" and let the
    // confidence number speak for itself.
    if winner_score < CONFIDENCE_FLOOR {
        return Some(MeterEstimate {
            beats_per_bar: 0,
            notation: "unknown".to_string(),
            confidence: winner_score.clamp(0.0, 1.0),
            ambiguous: false,
        });
    }

    // Ambiguity: a runner-up on a DIFFERENT grouping family (not a multiple
    // of the winner) that scored nearly as well. Multiples of the winner
    // are expected to correlate too and aren't evidence of ambiguity.
    let ambiguous = scores
        .iter()
        .any(|&(sg, s)| sg != g && sg % g != 0 && s >= AMBIGUITY_RATIO * winner_score);

    // Triplet-aware notation: when the BPM stage found triplet-subdivision
    // evidence, the beat unit is compound, so 2 beats = 6/8 and 4 beats =
    // 12/8 (4 dotted-quarter beats per bar).
    let notation = if triplet_feel && g == 2 {
        "6/8".to_string()
    } else if triplet_feel && g == 4 {
        "12/8".to_string()
    } else {
        notation_for(g)
    };

    Some(MeterEstimate {
        beats_per_bar: g,
        notation,
        confidence: winner_score.clamp(0.0, 1.0),
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

    /// Deterministic pseudo-noise in [-0.5, 0.5), used to simulate the
    /// accent variability of real performances/recordings.
    fn noise(k: usize) -> f64 {
        (k as f64 * 0.618_033_988_749_894_9).fract() - 0.5
    }

    /// Builds an onset train at exactly `bpm` with strengths following
    /// `pattern` cyclically (e.g. [2,1,1] = waltz), plus matching audio for
    /// the fallback path. `noise_amp` adds `noise_amp * noise(k)` to each
    /// strength (0.0 = clean pattern).
    fn accent_train(
        bpm: f64,
        num_beats: usize,
        pattern: &[f64],
        noise_amp: f64,
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
            let strength = (pattern[k % pattern.len()] + noise_amp * noise(k)).max(0.05);
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
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[2.0, 1.0, 1.5, 1.0], 0.0);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false)
            .expect("expected an estimate");
        assert_eq!(est.notation, "4/4");
        assert!(est.confidence > 0.3, "confidence {}", est.confidence);
        assert!(!est.ambiguous);
    }

    #[test]
    fn three_four_accent_pattern_reports_three_four() {
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[2.0, 1.0, 1.0], 0.0);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false)
            .expect("expected an estimate");
        assert_eq!(est.notation, "3/4");
        assert!(est.confidence > 0.3, "confidence {}", est.confidence);
        // A plain strong-weak-weak pattern is ALSO consistent with 6/8, so
        // ambiguity is acceptable (and honest) here.
    }

    #[test]
    fn two_four_accent_pattern_reports_two_four() {
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[2.0, 1.0], 0.0);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false)
            .expect("expected an estimate");
        assert_eq!(est.notation, "2/4");
    }

    #[test]
    fn six_eight_accent_pattern_reports_six_eight() {
        // [S w w s w w]: two groups of three with a secondary accent —
        // distinguishable from a plain 3/4 waltz pattern.
        let (onsets, samples, bpm, offset) =
            accent_train(120.0, 48, &[2.0, 1.0, 1.0, 1.4, 1.0, 1.0], 0.0);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false)
            .expect("expected an estimate");
        assert_eq!(est.notation, "6/8");
        assert!(!est.ambiguous);
    }

    #[test]
    fn uniform_accents_report_low_confidence() {
        // Equal-strength onsets and equal-energy clicks: no metrical
        // structure, so the estimate must not be confident.
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[1.0], 0.0);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false);
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
        let (onsets, samples, bpm, offset) = accent_train(120.0, 4, &[2.0, 1.0], 0.0);
        assert!(estimate_meter(&onsets, &samples, SR, bpm, offset, false).is_none());
    }

    // --- Noise-robustness regression tests ---------------------------------
    // The original best-phase-contrast scorer failed these: taking the max
    // over g phase means is a multiple-comparison contest that the largest
    // grouping (12) almost always wins on noisy accent data — which is why
    // every real song was reported as 12/8.

    #[test]
    fn noisy_four_four_still_reports_four_four() {
        // 4/4 pattern with +-0.5 noise on strengths (strong beats overlap
        // weak ones at the edges), like real pop drumming.
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[2.0, 1.0, 1.5, 1.0], 1.0);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false)
            .expect("expected an estimate");
        assert_eq!(est.notation, "4/4");
    }

    #[test]
    fn noisy_waltz_still_reports_three_four() {
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[2.0, 1.0, 1.0], 1.0);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false)
            .expect("expected an estimate");
        assert_eq!(est.notation, "3/4");
    }

    #[test]
    fn noisy_six_eight_still_reports_six_eight() {
        // Noise amp 0.6 (+-0.3) keeps the secondary accent (1.4 vs 1.0) above
        // the noise floor. With noise LARGER than the secondary accent gap,
        // period-3 legitimately wins — the 6/8 structure genuinely isn't in
        // the data anymore, so there's nothing to "correctly" detect.
        let (onsets, samples, bpm, offset) =
            accent_train(120.0, 48, &[2.0, 1.0, 1.0, 1.4, 1.0, 1.0], 0.6);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false)
            .expect("expected an estimate");
        assert_eq!(est.notation, "6/8");
    }

    #[test]
    fn pure_noise_reports_low_confidence() {
        // Random strengths, no metrical structure at all: the estimate
        // must not be confident (the old scorer over-claimed here by
        // cherry-picking the best of 12 phase means).
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[1.0], 1.0);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false);
        match est {
            None => {}
            Some(est) => assert!(
                est.confidence < 0.3,
                "pure noise should not produce a confident estimate, got {}",
                est.confidence
            ),
        }
    }

    #[test]
    fn weak_accent_structure_reports_unknown() {
        // A barely-there accent pattern buried under noise: the periodic
        // structure is below the noise floor, so the estimate should
        // decline with "unknown" rather than guess a grouping. (Without
        // noise, even a weak-but-consistent pattern IS real periodic
        // structure and is legitimately detected.)
        let (onsets, samples, bpm, offset) = accent_train(120.0, 48, &[1.1, 1.0, 1.05, 1.0], 0.6);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false)
            .expect("expected an estimate");
        assert_eq!(est.notation, "unknown");
        assert_eq!(est.beats_per_bar, 0);
        assert!(est.confidence < 0.3);
    }

    // --- Triplet-aware notation refinement ---------------------------------

    #[test]
    fn triplet_feel_maps_four_beats_to_twelve_eight() {
        // When the BPM stage already found triplet-subdivision evidence, a
        // 4-beat grouping means 4 dotted-quarter beats per bar: 12/8.
        let (onsets, samples, bpm, offset) = accent_train(68.0, 48, &[2.0, 1.0, 1.5, 1.0], 0.0);
        let est =
            estimate_meter(&onsets, &samples, SR, bpm, offset, true).expect("expected an estimate");
        assert_eq!(est.notation, "12/8");
        assert_eq!(est.beats_per_bar, 4);
    }

    #[test]
    fn triplet_feel_maps_two_beats_to_six_eight() {
        let (onsets, samples, bpm, offset) = accent_train(68.0, 48, &[2.0, 1.0], 0.0);
        let est =
            estimate_meter(&onsets, &samples, SR, bpm, offset, true).expect("expected an estimate");
        assert_eq!(est.notation, "6/8");
        assert_eq!(est.beats_per_bar, 2);
    }

    #[test]
    fn no_triplet_feel_keeps_simple_notation() {
        let (onsets, samples, bpm, offset) = accent_train(68.0, 48, &[2.0, 1.0, 1.5, 1.0], 0.0);
        let est = estimate_meter(&onsets, &samples, SR, bpm, offset, false)
            .expect("expected an estimate");
        assert_eq!(est.notation, "4/4");
    }
}

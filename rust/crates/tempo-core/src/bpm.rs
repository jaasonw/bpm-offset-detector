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
    calculate_bpm_with_context(onsets, sample_rate, opts).0
}

/// Like `calculate_bpm`, but also reports whether the subharmonic
/// preference pass re-labeled the top candidate as its 1/3 subharmonic
/// (i.e. the song shows triplet-subdivision evidence), for downstream
/// analysis like meter estimation.
pub(crate) fn calculate_bpm_with_context(
    onsets: &[Onset],
    sample_rate: u32,
    opts: &DetectOptions,
) -> (Vec<TempoResult>, bool) {
    // In order to determine the BPM, we need at least two onsets. Matches
    // the reference's fallback behavior rather than erroring.
    if onsets.len() < 2 {
        return (
            vec![TempoResult {
                bpm: 100.0,
                offset: 0.0,
                fitness: 1.0,
            }],
            false,
        );
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

    let mut triplet_feel = false;
    if opts.subharmonic_preference {
        triplet_feel =
            apply_subharmonic_preference(&mut tempo, &mut gapdata, onsets, sample_rate, opts);
    }

    (tempo, triplet_feel)
}

// Subharmonic preference rule thresholds (see apply_subharmonic_preference).
/// The beat phase must dominate the two subdivision phases by this ratio.
const BEAT_DOMINANCE_RATIO: f64 = 1.25;
/// The subdivision phases must contain at least this fraction of the beat
/// phase's support (i.e. the subdivisions are real events, not noise).
const SUBDIVISION_EVIDENCE_RATIO: f64 = 0.3;
/// The fundamental's weighted confidence must retain at least this fraction
/// of the harmonic's (the harmonic naturally collects all subdivision votes,
/// so the fundamental always has less; this just ensures it's substantial).
const FUNDAMENTAL_SUPPORT_RATIO: f64 = 0.45;
/// The pass may only override an UNCERTAIN scan: the top-2 fitness margin
/// must be below this factor. A true 3x-harmonic lock has a structural
/// margin ceiling — its half-tempo alias always scores 3/4 of the lock
/// (beat cluster N/2 plus the offbeat bonus N/4), capping the margin at
/// 4/3 — so a threshold of 1.5 admits every genuine harmonic lock while
/// protecting songs whose fast tempo won decisively on its own structure
/// (e.g. a true 170 BPM winning 1.76x over its runner-up must not be
/// thirded to 56.667 just because incidental swing accents alias into the
/// subharmonic's grid).
const SCAN_UNCERTAINTY_MARGIN: f64 = 1.5;

/// Re-labels the winning candidate as its 1/3 subharmonic when the accent
/// evidence supports it, fixing triplet-feel songs being reported at 3x
/// their true tempo (e.g. a 68 BPM song with triplet subdivisions detected
/// as 203.978 because every onset aligns at the 3x rate under
/// constant-strength voting).
///
/// Only /3 is considered, never /2: even/odd backbeat accenting makes the
/// half-tempo intrinsically competitive on ANY even-tempo song, so a /2
/// rule cannot avoid false tempo-halving (this distinction is what keeps
/// the pass safe on regular rock/pop).
///
/// The flip requires ALL of:
/// - beat phase dominance: the fundamental's best phase has clearly more
///   weighted support than either triplet subdivision phase (accents fall
///   on the beat), and
/// - subdivision evidence: the subdivision phases contain real onsets (the
///   song genuinely has a triplet feel — without this, plain songs whose
///   beats merely alias into the subharmonic's grid would flip), and
/// - substantial support: the fundamental's total weighted confidence is a
///   reasonable fraction of the harmonic's, and
/// - scan uncertainty: the scan's top-2 margin is below
///   `SCAN_UNCERTAINTY_MARGIN` — a genuine 3x lock is structurally capped
///   at a 4/3 margin by its half-tempo alias, so a decisive margin means
///   the winner earned its ranking and must not be thirded (regression:
///   a true 170 BPM 4/4 song with swing accents was demoted to 56.667).
///
/// When the rule fires, the fundamental replaces the #1 entry (position
/// signals the preference decision); its `fitness` reports the fundamental's
/// unweighted full-precision confidence for transparency, which is typically
/// LOWER than the harmonic's — that's expected and is why position, not
/// fitness, encodes the decision.
fn apply_subharmonic_preference(
    tempo: &mut [TempoResult],
    gapdata: &mut GapData,
    onsets: &[Onset],
    sample_rate: u32,
    opts: &DetectOptions,
) -> bool {
    let Some(seed) = tempo.first() else {
        return false;
    };
    let seed_bpm = seed.bpm;
    let sub_bpm = seed_bpm / 3.0;
    if sub_bpm < opts.min_bpm {
        return false;
    }

    let seed_interval = sample_rate as f64 * 60.0 / seed_bpm;
    let sub_interval = sample_rate as f64 * 60.0 / sub_bpm;

    let w_seed = gapdata.confidence_for_bpm_weighted(onsets, seed_interval);
    let w_sub = gapdata.confidence_for_bpm_weighted(onsets, sub_interval);

    // Phase supports on the fundamental's grid: the beat phase vs the two
    // triplet subdivision phases.
    let (interval, beat_pos) = gapdata.weighted_best_phase(onsets, sub_interval);
    let c_beat = gapdata.gap_confidence(beat_pos, interval);
    let c_sub1 = gapdata.gap_confidence((beat_pos + interval / 3) % interval, interval);
    let c_sub2 = gapdata.gap_confidence((beat_pos + interval * 2 / 3) % interval, interval);

    let beat_dominant = c_beat >= BEAT_DOMINANCE_RATIO * c_sub1.max(c_sub2);
    let subdivisions_real = c_sub1 + c_sub2 >= SUBDIVISION_EVIDENCE_RATIO * c_beat;
    let substantial_support = w_sub >= FUNDAMENTAL_SUPPORT_RATIO * w_seed;

    // Only second-guess an uncertain scan. A single surviving candidate is
    // decisive by definition (everything else fell below the scan's coarse
    // threshold), and a decisive top-2 margin means the winner's support
    // comes from its own structure, not the window bias this pass exists
    // to correct.
    let scan_uncertain =
        tempo.len() >= 2 && tempo[0].fitness / tempo[1].fitness < SCAN_UNCERTAINTY_MARGIN;

    if beat_dominant && subdivisions_real && substantial_support && scan_uncertain {
        let fitness = gapdata.confidence_for_bpm(onsets, sub_interval);
        tempo[0] = TempoResult {
            bpm: sub_bpm,
            offset: 0.0,
            fitness,
        };
        true
    } else {
        false
    }
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
    fn detects_slow_bpm_below_the_reference_range() {
        // 68 BPM is below the reference implementation's 89 BPM floor, so
        // the reference (and this crate before the defaults were widened)
        // would report its in-range octave multiples (136, 204) instead of
        // the true tempo. The widened default range must find 68 directly.
        let sample_rate = 44100u32;
        let true_bpm = 68.0;
        let interval = (sample_rate as f64 * 60.0 / true_bpm).round() as usize;
        let onsets = click_train(interval, 60);
        let opts = DetectOptions::default();

        let results = calculate_bpm(&onsets, sample_rate, &opts);

        assert!(
            (results[0].bpm - true_bpm).abs() < 0.5,
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

    /// Builds a triplet-feel onset train: onsets at every 1/3-beat of the
    /// given BPM, with every 3rd onset (the ones landing on the beat)
    /// accented. Reproduces the real-world failure mode of a 68 BPM
    /// triplet-feel song being reported as 203.978/136.
    fn triplet_train(bpm: f64, num_subdivisions: i64, accent: f64, sample_rate: u32) -> Vec<Onset> {
        let interval_f = sample_rate as f64 * 60.0 / bpm;
        (0..num_subdivisions)
            .map(|i| {
                let jitter = (i % 7) - 3;
                let pos = (i as f64 * interval_f / 3.0).round() as i64 + jitter;
                let strength = if i % 3 == 0 { accent } else { 1.0 };
                Onset::new(pos.max(0) as usize, strength)
            })
            .collect()
    }

    #[test]
    fn triplet_subdivision_song_prefers_fundamental() {
        // A 68 BPM song with accented beats and triplet subdivisions. Under
        // constant-strength voting the 3x harmonic (204) wins because all
        // onsets align there; with subharmonic preference the accented
        // fundamental must win instead.
        let sample_rate = 44100u32;
        let onsets = triplet_train(68.0, 120, 2.5, sample_rate);

        let results = calculate_bpm(&onsets, sample_rate, &DetectOptions::default());

        assert!(
            (results[0].bpm - 68.0).abs() < 0.5,
            "top candidate bpm = {}, expected close to 68.0",
            results[0].bpm
        );
    }

    #[test]
    fn fast_song_with_backbeat_is_not_thirded() {
        // Guard against false subharmonic flips: a true 204 BPM song with
        // an even/odd backbeat accent pattern must NOT be re-labeled as 68
        // (its /3 subharmonic). Its beat accents are spread uniformly
        // across the subharmonic's three phase clusters, so the beat-phase
        // dominance check must reject the flip.
        let sample_rate = 44100u32;
        let true_bpm = 204.0;
        let interval_f = sample_rate as f64 * 60.0 / true_bpm;
        let onsets: Vec<Onset> = (0..120i64)
            .map(|i| {
                let jitter = (i % 7) - 3;
                let pos = (i as f64 * interval_f).round() as i64 + jitter;
                let strength = if i % 2 == 0 { 2.5 } else { 1.0 };
                Onset::new(pos.max(0) as usize, strength)
            })
            .collect();

        let results = calculate_bpm(&onsets, sample_rate, &DetectOptions::default());

        assert!(
            (results[0].bpm - true_bpm).abs() < 1.0,
            "top candidate bpm = {}, expected close to {true_bpm}",
            results[0].bpm
        );
    }

    #[test]
    fn subharmonic_preference_can_be_disabled() {
        // With the preference layer off, the triplet train falls back to
        // raw reference behavior: the 3x harmonic wins.
        let sample_rate = 44100u32;
        let onsets = triplet_train(68.0, 120, 2.5, sample_rate);
        let opts = DetectOptions {
            subharmonic_preference: false,
            ..DetectOptions::default()
        };

        let results = calculate_bpm(&onsets, sample_rate, &opts);

        assert!(
            (results[0].bpm - 204.0).abs() < 1.0,
            "top candidate bpm = {}, expected close to 204.0",
            results[0].bpm
        );
    }
}

//! End-to-end integration tests: exercise the full `tempo_core::detect`
//! pipeline (complex-domain onset detection -> BPM scan -> offset
//! detection) against synthetic click tracks with known ground truth,
//! rather than testing individual modules in isolation as the unit tests
//! do. Tolerances here are looser than the module-level unit tests because
//! they include real onset-detection jitter on top of the underlying
//! algorithm's own precision.

use tempo_core::{detect, DetectOptions};

/// A synthetic click track: silence with a short decaying pulse (sharp
/// attack, smooth linear decay -- a strong broadband transient with no
/// secondary transient at the release) at each beat, starting at
/// `offset_samples` and repeating every `interval` samples.
fn synthetic_click_track(
    num_frames: usize,
    offset_samples: usize,
    interval: usize,
    click_len: usize,
) -> Vec<f32> {
    let mut samples = vec![0.0f32; num_frames];
    let mut k = 0usize;
    loop {
        let pos = offset_samples + k * interval;
        if pos + click_len >= num_frames {
            break;
        }
        for i in 0..click_len {
            samples[pos + i] = (1.0 - i as f32 / click_len as f32).max(0.0);
        }
        k += 1;
    }
    samples
}

#[test]
fn detects_known_bpm_end_to_end() {
    let sample_rate = 44100u32;
    let true_bpm = 120.0;
    let interval = (sample_rate as f64 * 60.0 / true_bpm).round() as usize;
    let num_frames = interval * 60;
    let samples = synthetic_click_track(num_frames, 5000, interval, 400);

    let results = detect(&samples, sample_rate, &DetectOptions::default());

    assert!(!results.is_empty(), "expected at least one candidate");
    assert!(
        (results[0].bpm - true_bpm).abs() < 0.5,
        "top candidate bpm = {}, expected close to {true_bpm}",
        results[0].bpm
    );
}

#[test]
fn detects_known_bpm_and_offset_end_to_end() {
    let sample_rate = 44100u32;
    let true_bpm = 162.0;
    let interval = (sample_rate as f64 * 60.0 / true_bpm).round() as usize;
    let true_offset_secs = 0.12;
    let offset_samples = (true_offset_secs * sample_rate as f64).round() as usize;
    let num_frames = interval * 60 + offset_samples;
    let samples = synthetic_click_track(num_frames, offset_samples, interval, 400);

    let results = detect(&samples, sample_rate, &DetectOptions::default());

    assert!(!results.is_empty());
    assert!(
        (results[0].bpm - true_bpm).abs() < 0.5,
        "top candidate bpm = {}, expected close to {true_bpm}",
        results[0].bpm
    );
    let offset_error = (results[0].offset - true_offset_secs).abs();
    assert!(
        offset_error < 0.02,
        "offset = {}, expected close to {true_offset_secs} (error {offset_error}s)",
        results[0].offset
    );
}

#[test]
fn returns_no_more_than_three_candidates() {
    let sample_rate = 44100u32;
    let interval = (sample_rate as f64 * 60.0 / 140.0).round() as usize;
    let samples = synthetic_click_track(interval * 60, 3000, interval, 400);

    let results = detect(&samples, sample_rate, &DetectOptions::default());

    assert!(results.len() <= 3);
}

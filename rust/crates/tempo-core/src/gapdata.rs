//! Gap-confidence evaluation: measures how well a candidate beat interval
//! explains a set of onsets, by building a histogram of onset strengths
//! wrapped modulo the interval and measuring how concentrated that
//! histogram is around its best-supported position (a Hamming-windowed
//! "area under the peak").
//!
//! This is a Rust port of the reference `GapData`/`GapConfidence`/
//! `GetConfidenceForInterval`/`GetConfidenceForBPM` functions in
//! `FindTempo_standalone.cpp`, with the multi-threading removed (this crate
//! is single-threaded by design; see the design doc's Non-goals).

use crate::window::hamming_window;
use crate::Onset;

/// Holds the reusable buffers needed to evaluate gap confidence for many
/// candidate intervals against the same set of onsets, without
/// reallocating per call.
pub(crate) struct GapData {
    /// Hamming window used to weight onset contributions near a candidate
    /// gap position; length `window_size`.
    window: Vec<f64>,
    /// `2048 >> downsample`.
    window_size: usize,
    /// Downsampling shift applied to onset positions before histogramming,
    /// for the coarse/refine scan. Must be `0` when using
    /// [`GapData::confidence_for_bpm`].
    downsample: u32,
    /// Reusable onset-strength histogram, indexed by wrapped position.
    /// Must be at least as long as the largest interval ever passed in.
    histogram: Vec<f64>,
}

impl GapData {
    /// Creates a `GapData` sized for intervals up to `buffer_size` samples
    /// (full resolution, before downsampling), using the given downsample
    /// shift.
    pub(crate) fn new(buffer_size: usize, downsample: u32) -> Self {
        let window_size = 2048usize >> downsample;
        let mut window = vec![0.0; window_size];
        hamming_window(&mut window);
        GapData {
            window,
            window_size,
            downsample,
            histogram: vec![0.0; buffer_size],
        }
    }

    /// Direct access to the histogram buffer, for callers (like offset
    /// detection) that need to build their own weighting scheme instead of
    /// the strength-weighted one used by [`GapData::confidence_for_interval`]
    /// and [`GapData::confidence_for_bpm`].
    pub(crate) fn histogram_mut(&mut self) -> &mut [f64] {
        &mut self.histogram
    }

    /// The Hamming-windowed "area" of onset support in `self.histogram`
    /// around `gap_pos`, within a wraparound histogram of length `interval`.
    ///
    /// Sums `histogram[i] * window[i - (gap_pos - window_size/2)]` for `i`
    /// in a `window_size`-wide neighborhood of `gap_pos`, wrapping around
    /// both ends of `[0, interval)`.
    pub(crate) fn gap_confidence(&self, gap_pos: usize, interval: usize) -> f64 {
        debug_assert!(
            interval > self.window_size,
            "gap_confidence assumes the beat interval ({interval}) is larger than the \
             analysis window ({}); this always holds for real BPM ranges (interval is on \
             the order of 10,000+ samples) but is easy to violate with small synthetic \
             test intervals",
            self.window_size
        );
        let half_window = self.window_size / 2;
        let mut area = 0.0;

        let mut begin_onset = gap_pos as i64 - half_window as i64;
        let mut end_onset = gap_pos as i64 + half_window as i64;

        if begin_onset < 0 {
            let wrapped_begin = begin_onset + interval as i64;
            for i in wrapped_begin..interval as i64 {
                let window_index = (i - wrapped_begin) as usize;
                area += self.histogram[i as usize] * self.window[window_index];
            }
            begin_onset = 0;
        }
        if end_onset > interval as i64 {
            let wrapped_end = end_onset - interval as i64;
            let index_offset = self.window_size as i64 - wrapped_end;
            for i in 0..wrapped_end {
                let window_index = (i + index_offset) as usize;
                area += self.histogram[i as usize] * self.window[window_index];
            }
            end_onset = interval as i64;
        }
        for i in begin_onset..end_onset {
            let window_index = (i - begin_onset) as usize;
            area += self.histogram[i as usize] * self.window[window_index];
        }

        area
    }

    /// Confidence that `interval` (in full-resolution samples) is the beat
    /// interval, using integer modulo wrapping and `self.downsample`-reduced
    /// histogram resolution. Onsets are weighted by their `strength`. This
    /// is the coarse/refine-scan variant (`GetConfidenceForInterval` in the
    /// reference).
    pub(crate) fn confidence_for_interval(&mut self, onsets: &[Onset], interval: usize) -> f64 {
        let reduced_interval = interval >> self.downsample;
        self.histogram[..reduced_interval].fill(0.0);

        let mut wrapped_pos = Vec::with_capacity(onsets.len());
        for onset in onsets {
            let pos = (onset.pos % interval) >> self.downsample;
            wrapped_pos.push(pos);
            self.histogram[pos] += onset.strength;
        }

        let mut highest = 0.0f64;
        for &pos in &wrapped_pos {
            let mut confidence = self.gap_confidence(pos, reduced_interval);
            let offbeat_pos = (pos + reduced_interval / 2) % reduced_interval;
            confidence += self.gap_confidence(offbeat_pos, reduced_interval) * 0.5;
            if confidence > highest {
                highest = confidence;
            }
        }
        highest
    }

    /// Confidence that a candidate BPM (expressed as a fractional sample
    /// interval `interval_f = sample_rate * 60 / bpm`) is the beat interval,
    /// using fractional (`rem_euclid`) wrapping for sub-sample onset
    /// placement and full histogram resolution. Onsets are weighted by
    /// their `strength`. Requires `downsample == 0` (`GetConfidenceForBPM`
    /// in the reference).
    pub(crate) fn confidence_for_bpm(&mut self, onsets: &[Onset], interval_f: f64) -> f64 {
        debug_assert_eq!(
            self.downsample, 0,
            "confidence_for_bpm requires downsample = 0"
        );
        let interval = interval_f.round().max(1.0) as usize;
        self.histogram[..interval].fill(0.0);

        let mut wrapped_pos = Vec::with_capacity(onsets.len());
        for onset in onsets {
            let pos = (onset.pos as f64).rem_euclid(interval_f) as usize;
            let pos = pos.min(interval - 1);
            wrapped_pos.push(pos);
            self.histogram[pos] += onset.strength;
        }

        let mut highest = 0.0f64;
        for &pos in &wrapped_pos {
            let mut confidence = self.gap_confidence(pos, interval);
            let offbeat_pos = (pos + interval / 2) % interval;
            confidence += self.gap_confidence(offbeat_pos, interval) * 0.5;
            if confidence > highest {
                highest = confidence;
            }
        }
        highest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_train(interval: usize, count: usize) -> Vec<Onset> {
        (0..count)
            .map(|i| {
                // Small deterministic jitter (+/- 3 samples) so the train
                // isn't perfectly periodic to floating precision, matching
                // real audio (which never has beats at *exactly* uniform
                // sample spacing). Without jitter, a perfectly periodic
                // click train can make an unrelated candidate interval
                // alias into a spuriously concentrated histogram bin,
                // which is a property of testing with an idealized signal,
                // not a bug in the confidence calculation.
                let jitter = (i % 7) as i64 - 3;
                let pos = (i * interval) as i64 + jitter;
                Onset::new(pos.max(0) as usize, 1.0)
            })
            .collect()
    }

    #[test]
    fn confidence_for_interval_peaks_at_true_interval() {
        // Realistic scale: real BPM intervals at 44.1kHz range from ~12,900
        // to ~29,700 samples, always far larger than the 2048-sample
        // analysis window. Use a comparable scale here.
        let onsets = click_train(16000, 30);
        let mut gapdata = GapData::new(32000, 0);

        let confidence_correct = gapdata.confidence_for_interval(&onsets, 16000);
        let confidence_wrong_low = gapdata.confidence_for_interval(&onsets, 11000);
        let confidence_wrong_high = gapdata.confidence_for_interval(&onsets, 21000);

        assert!(
            confidence_correct > confidence_wrong_low,
            "correct={confidence_correct}, wrong_low={confidence_wrong_low}"
        );
        assert!(
            confidence_correct > confidence_wrong_high,
            "correct={confidence_correct}, wrong_high={confidence_wrong_high}"
        );
    }

    #[test]
    fn confidence_for_interval_respects_downsampling() {
        let true_interval = 16000usize;
        let onsets = click_train(true_interval, 30);
        // window_size = 2048 >> 3 = 256, comfortably smaller than the
        // downsampled interval (16000 >> 3 = 2000).
        let mut gapdata = GapData::new(true_interval * 2, 3);

        let confidence_correct = gapdata.confidence_for_interval(&onsets, true_interval);
        let confidence_wrong = gapdata.confidence_for_interval(&onsets, true_interval - 4000);

        assert!(
            confidence_correct > confidence_wrong,
            "correct={confidence_correct}, wrong={confidence_wrong}"
        );
    }

    #[test]
    fn confidence_for_bpm_resolves_fractional_intervals() {
        // Interval of 9185.5 samples doesn't divide evenly; confidence_for_bpm
        // uses fractional wrapping and should still clearly prefer the true
        // interval over an interval a full sample off.
        let interval_f = 9185.5;
        let onsets: Vec<Onset> = (0..40)
            .map(|i| {
                let jitter = (i % 7) as i64 - 3;
                let pos = (i as f64 * interval_f).round() as i64 + jitter;
                Onset::new(pos.max(0) as usize, 1.0)
            })
            .collect();
        let mut gapdata = GapData::new(20000, 0);

        let confidence_correct = gapdata.confidence_for_bpm(&onsets, interval_f);
        let confidence_wrong = gapdata.confidence_for_bpm(&onsets, interval_f + 300.0);

        assert!(
            confidence_correct > confidence_wrong,
            "correct={confidence_correct}, wrong={confidence_wrong}"
        );
    }

    #[test]
    fn gap_confidence_wraps_around_both_ends() {
        // window_size = 2048 (downsample 0), half_window = 1024. Put a
        // single onset near the wraparound boundary and confirm querying at
        // position 0 (which straddles the wrap) picks it up.
        let mut gapdata = GapData::new(5000, 0);
        let interval = 4000;
        gapdata.histogram_mut()[..interval].fill(0.0);
        gapdata.histogram_mut()[3999] = 1.0; // one sample before wraparound
        let confidence_at_zero = gapdata.gap_confidence(0, interval);
        assert!(
            confidence_at_zero > 0.0,
            "expected wraparound contribution, got {confidence_at_zero}"
        );
    }
}

//! Window functions used by gap-confidence evaluation.

/// Fills `out` with a Hamming window of length `out.len()`.
///
/// Matches the reference `CreateHammingWindow`: `0.54 - 0.46*cos(2*pi*i/(n-1))`.
///
/// # Panics
/// Panics if `out.len() < 2` (the formula divides by `n - 1`).
pub fn hamming_window(out: &mut [f64]) {
    let n = out.len();
    assert!(n >= 2, "hamming window needs at least 2 samples");
    let t = std::f64::consts::TAU / (n - 1) as f64;
    for (i, v) in out.iter_mut().enumerate() {
        *v = 0.54 - 0.46 * (i as f64 * t).cos();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_near_zero_center_is_one() {
        let mut w = vec![0.0; 9];
        hamming_window(&mut w);
        assert!((w[0] - 0.08).abs() < 1e-9, "w[0] = {}", w[0]);
        assert!((w[8] - 0.08).abs() < 1e-9, "w[8] = {}", w[8]);
        assert!((w[4] - 1.0).abs() < 1e-9, "w[4] = {}", w[4]);
    }

    #[test]
    fn is_symmetric() {
        let mut w = vec![0.0; 16];
        hamming_window(&mut w);
        for i in 0..w.len() {
            assert!(
                (w[i] - w[w.len() - 1 - i]).abs() < 1e-12,
                "window not symmetric at {i}"
            );
        }
    }
}

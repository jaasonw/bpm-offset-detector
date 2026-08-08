//! Cubic least-squares polynomial fit.
//!
//! Used to normalize the fitness curve over the BPM-interval search space:
//! the reference algorithm fits a degree-3 polynomial through coarse
//! `(interval, fitness)` samples and subtracts it, so that fitness
//! comparisons aren't biased by the overall shape of the curve across the
//! BPM range.
//!
//! This solves the normal equations `(X^T X) c = X^T y` for
//! `y = c0 + c1*x + c2*x^2 + c3*x^3` via Gaussian elimination with partial
//! pivoting. The reference C++ (`polyfit.h`) solves the same normal
//! equations via Givens-rotation QR; both produce the same least-squares
//! fit (up to floating point rounding) since they solve the same linear
//! system. Gaussian elimination on a fixed 4x4 system is simpler to
//! implement and verify than a general QR decomposition, and `polyfit.h` is
//! explicitly unlicensed in this repository, so this module is written from
//! the underlying least-squares math rather than translated from it.

/// Fits a cubic polynomial `y = c[0] + c[1]*x + c[2]*x^2 + c[3]*x^3` to the
/// given points in a least-squares sense. Returns the four coefficients,
/// constant term first (matching `polyval`'s expected order).
///
/// # Panics
/// Panics if `x.len() != y.len()` or `x.len() < 4` (a cubic needs at least
/// 4 points to be well-determined).
pub fn polyfit_cubic(x: &[f64], y: &[f64]) -> [f64; 4] {
    assert_eq!(x.len(), y.len(), "x and y must have the same length");
    assert!(x.len() >= 4, "need at least 4 points to fit a cubic");

    // Accumulate X^T X (4x4) and X^T y (4x1) directly, without materializing
    // the full n x 4 design matrix.
    let mut xtx = [[0.0f64; 4]; 4];
    let mut xty = [0.0f64; 4];

    for (&xi, &yi) in x.iter().zip(y.iter()) {
        let powers = [1.0, xi, xi * xi, xi * xi * xi];
        for r in 0..4 {
            for c in 0..4 {
                xtx[r][c] += powers[r] * powers[c];
            }
            xty[r] += powers[r] * yi;
        }
    }

    solve_4x4(xtx, xty)
}

/// Evaluates a polynomial with the given coefficients (constant term first)
/// at `x`.
pub fn polyval(coeffs: &[f64], x: f64) -> f64 {
    let mut result = 0.0;
    let mut xp = 1.0;
    for &c in coeffs {
        result += c * xp;
        xp *= x;
    }
    result
}

/// Solves the linear system `a * result = b` for a 4x4 matrix `a`, via
/// Gaussian elimination with partial pivoting.
fn solve_4x4(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> [f64; 4] {
    for col in 0..4 {
        // Partial pivot: swap in the row with the largest magnitude in this
        // column, for numerical stability.
        let mut pivot_row = col;
        let mut pivot_val = a[col][col].abs();
        // `row` is also tracked as `pivot_row` (the result), not just used
        // to index `a`, so an enumerate()-based rewrite wouldn't be clearer.
        #[allow(clippy::needless_range_loop)]
        for row in (col + 1)..4 {
            if a[row][col].abs() > pivot_val {
                pivot_row = row;
                pivot_val = a[row][col].abs();
            }
        }
        if pivot_row != col {
            a.swap(col, pivot_row);
            b.swap(col, pivot_row);
        }

        let diag = a[col][col];
        assert!(diag.abs() > 1e-300, "singular matrix in polyfit_cubic");

        // Gaussian elimination inherently indexes both `a` and `b` by row
        // number while also using that row number in arithmetic (`factor`),
        // so an iterator-based rewrite wouldn't be clearer.
        #[allow(clippy::needless_range_loop)]
        for row in (col + 1)..4 {
            let factor = a[row][col] / diag;
            if factor == 0.0 {
                continue;
            }
            #[allow(clippy::needless_range_loop)]
            for k in col..4 {
                a[row][k] -= factor * a[col][k];
            }
            b[row] -= factor * b[col];
        }
    }

    // Back-substitution.
    let mut result = [0.0f64; 4];
    for row in (0..4).rev() {
        let mut sum = b[row];
        for col in (row + 1)..4 {
            sum -= a[row][col] * result[col];
        }
        result[row] = sum / a[row][row];
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_exact_cubic_with_no_noise() {
        // y = 2 + 3x - x^2 + 0.5x^3, sampled exactly: the least-squares fit
        // of a cubic to points that lie exactly on a cubic must recover the
        // exact coefficients (up to floating point error).
        let f = |x: f64| 2.0 + 3.0 * x - x * x + 0.5 * x * x * x;
        let xs: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|&x| f(x)).collect();

        let coeffs = polyfit_cubic(&xs, &ys);

        assert!((coeffs[0] - 2.0).abs() < 1e-6, "c0 = {}", coeffs[0]);
        assert!((coeffs[1] - 3.0).abs() < 1e-6, "c1 = {}", coeffs[1]);
        assert!((coeffs[2] - (-1.0)).abs() < 1e-6, "c2 = {}", coeffs[2]);
        assert!((coeffs[3] - 0.5).abs() < 1e-6, "c3 = {}", coeffs[3]);
    }

    #[test]
    fn polyval_evaluates_constant() {
        assert_eq!(polyval(&[5.0], 100.0), 5.0);
    }

    #[test]
    fn polyval_evaluates_cubic() {
        // 2 + 3x - x^2 + 0.5x^3 at x=2 => 2 + 6 - 4 + 4 = 8
        let coeffs = [2.0, 3.0, -1.0, 0.5];
        assert!((polyval(&coeffs, 2.0) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn fits_at_bpm_interval_scale_without_blowing_up() {
        // x values in the thousands, matching the scale used for BPM
        // interval fitness normalization (x ~ 10,000-30,000 samples). This
        // is the numerically delicate regime flagged as a risk in the
        // design doc; verify it stays well-conditioned in f64.
        let xs: Vec<f64> = (0..20).map(|i| 10000.0 + i as f64 * 500.0).collect();
        let ys: Vec<f64> = xs.iter().map(|&x| 0.001 * x + 3.0).collect();

        let coeffs = polyfit_cubic(&xs, &ys);
        for (&x, &y) in xs.iter().zip(ys.iter()) {
            let predicted = polyval(&coeffs, x);
            assert!(
                (predicted - y).abs() < 1.0,
                "at x={x}, predicted={predicted}, actual={y}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "same length")]
    fn panics_on_mismatched_lengths() {
        polyfit_cubic(&[1.0, 2.0, 3.0, 4.0], &[1.0, 2.0, 3.0]);
    }

    #[test]
    #[should_panic(expected = "at least 4 points")]
    fn panics_on_too_few_points() {
        polyfit_cubic(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]);
    }
}

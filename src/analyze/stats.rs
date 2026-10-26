//! Mean and 95% interval across replicates.
//!
//! The interval is the empirical 2.5th to 97.5th percentile of the per-replicate
//! values (linear interpolation between order statistics), not a bootstrap.

use serde::Serialize;

#[derive(Serialize, Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub mean: f64,
    pub ci_low: f64,
    pub ci_high: f64,
}

pub fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        None
    } else {
        Some(xs.iter().sum::<f64>() / xs.len() as f64)
    }
}

/// `q` in 0..=1 of already-sorted values.
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    let pos = q * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

pub fn interval(xs: &[f64]) -> Option<Interval> {
    let m = mean(xs)?;
    let mut sorted = xs.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    Some(Interval {
        mean: m,
        ci_low: percentile(&sorted, 0.025),
        ci_high: percentile(&sorted, 0.975),
    })
}

pub fn round(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_interpolates_between_order_statistics() {
        let xs = [10.0, 20.0, 30.0, 40.0, 50.0];
        assert_eq!(percentile(&xs, 0.0), 10.0);
        assert_eq!(percentile(&xs, 1.0), 50.0);
        assert_eq!(percentile(&xs, 0.5), 30.0);
        assert_eq!(percentile(&xs, 0.125), 15.0);
    }

    #[test]
    fn interval_of_one_hundred_values() {
        let xs: Vec<f64> = (1..=100).map(f64::from).collect();
        let iv = interval(&xs).unwrap();
        assert_eq!(iv.mean, 50.5);
        assert!((iv.ci_low - 3.475).abs() < 1e-9, "{}", iv.ci_low);
        assert!((iv.ci_high - 97.525).abs() < 1e-9, "{}", iv.ci_high);
    }

    #[test]
    fn interval_is_order_independent_and_none_when_empty() {
        assert_eq!(interval(&[3.0, 1.0, 2.0]), interval(&[1.0, 2.0, 3.0]));
        assert_eq!(interval(&[]), None);
    }

    #[test]
    fn rounds_to_places() {
        assert_eq!(round(1.23456, 2), 1.23);
        assert_eq!(round(0.00005, 4), 0.0001);
    }
}

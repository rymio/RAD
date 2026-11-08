//! Small numeric helpers shared across modules.

/// Percentile (0..=100) by linear interpolation. Returns `None` for empty input.
pub fn percentile(values: &[f32], p: f32) -> Option<f32> {
    let mut v: Vec<f32> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let rank = (p.clamp(0.0, 100.0) / 100.0) * (v.len() - 1) as f32;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f32;
    Some(v[lo] + (v[hi] - v[lo]) * frac)
}

pub fn median(values: &[f32]) -> Option<f32> {
    percentile(values, 50.0)
}

pub fn mean(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f32>() / values.len() as f32)
    }
}

pub fn std_dev(values: &[f32]) -> Option<f32> {
    let m = mean(values)?;
    if values.len() < 2 {
        return Some(0.0);
    }
    let var = values.iter().map(|x| (x - m).powi(2)).sum::<f32>() / (values.len() - 1) as f32;
    Some(var.sqrt())
}

/// Pearson correlation of two equal-length series. `None` if either is constant.
pub fn pearson(a: &[f32], b: &[f32]) -> Option<f32> {
    let n = a.len().min(b.len());
    if n < 3 {
        return None;
    }
    let (a, b) = (&a[..n], &b[..n]);
    let ma = mean(a)?;
    let mb = mean(b)?;
    let (mut sab, mut saa, mut sbb) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let da = (a[i] - ma) as f64;
        let db = (b[i] - mb) as f64;
        sab += da * db;
        saa += da * da;
        sbb += db * db;
    }
    if saa <= 1e-12 || sbb <= 1e-12 {
        return None;
    }
    Some((sab / (saa.sqrt() * sbb.sqrt())) as f32)
}

/// Least-squares slope of `y` against `x`.
pub fn slope(x: &[f32], y: &[f32]) -> Option<f32> {
    let n = x.len().min(y.len());
    if n < 2 {
        return None;
    }
    let mx = mean(&x[..n])?;
    let my = mean(&y[..n])?;
    let (mut sxy, mut sxx) = (0.0f32, 0.0f32);
    for i in 0..n {
        sxy += (x[i] - mx) * (y[i] - my);
        sxx += (x[i] - mx).powi(2);
    }
    if sxx <= f32::EPSILON {
        None
    } else {
        Some(sxy / sxx)
    }
}

/// Running median filter with an odd window; edges use a shrunken window.
pub fn median_filter(values: &[f32], window: usize) -> Vec<f32> {
    let half = window / 2;
    (0..values.len())
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(values.len());
            median(&values[lo..hi]).unwrap_or(values[i])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(median(&v), Some(3.0));
        assert_eq!(percentile(&v, 0.0), Some(1.0));
        assert_eq!(percentile(&v, 100.0), Some(5.0));
        assert_eq!(percentile(&v, 25.0), Some(2.0));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn correlation_and_slope() {
        let x: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let y: Vec<f32> = x.iter().map(|v| 3.0 * v + 1.0).collect();
        let neg: Vec<f32> = x.iter().map(|v| -v).collect();
        assert!((pearson(&x, &y).unwrap() - 1.0).abs() < 1e-5);
        assert!((pearson(&x, &neg).unwrap() + 1.0).abs() < 1e-5);
        assert!((slope(&x, &y).unwrap() - 3.0).abs() < 1e-5);
        assert_eq!(pearson(&x, &[1.0; 10]), None);
    }

    #[test]
    fn median_filter_removes_spikes() {
        let v = [1.0, 1.0, 9.0, 1.0, 1.0];
        assert_eq!(median_filter(&v, 3), vec![1.0, 1.0, 1.0, 1.0, 1.0]);
    }
}

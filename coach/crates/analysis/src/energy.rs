//! Intensity helpers. Intensity frames share the pitch tracker's alignment
//! (25 ms window, 10 ms hop), so frame `i` of each always describe the same audio.

/// Floor used for digital silence, in dBFS.
pub const SILENCE_DB: f32 = -100.0;

/// Linear RMS to dBFS (full-scale sine ≈ −3 dBFS).
pub fn rms_to_db(rms: f32) -> f32 {
    if rms <= 1e-5 {
        SILENCE_DB
    } else {
        (20.0 * rms.log10()).max(SILENCE_DB)
    }
}

/// Centred moving average over `window` frames.
pub fn smooth(values: &[f32], window: usize) -> Vec<f32> {
    let half = window / 2;
    (0..values.len())
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(values.len());
            values[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_conversion() {
        assert!((rms_to_db(1.0)).abs() < 1e-6);
        assert!((rms_to_db(0.1) + 20.0).abs() < 1e-4);
        assert_eq!(rms_to_db(0.0), SILENCE_DB);
    }

    #[test]
    fn smoothing_preserves_constant() {
        assert_eq!(smooth(&[2.0; 6], 3), vec![2.0; 6]);
    }
}

//! Intonation contours in semitones relative to the speaker's own median F0,
//! so a deep voice and a high voice saying the same tune produce the same shape.

use crate::{stats, FRAME_RATE};

/// Hz to semitones relative to `reference_hz`.
pub fn semitones(hz: f32, reference_hz: f32) -> f32 {
    12.0 * (hz / reference_hz).log2()
}

/// Convert a cleaned F0 track to semitones around its own median.
/// Returns the contour and the median Hz used as reference.
pub fn to_semitones(f0: &[Option<f32>]) -> Option<(Vec<Option<f32>>, f32)> {
    let voiced: Vec<f32> = f0.iter().flatten().copied().collect();
    let med = stats::median(&voiced)?;
    Some((f0.iter().map(|v| v.map(|hz| semitones(hz, med))).collect(), med))
}

/// Fill unvoiced gaps by linear interpolation; leading and trailing gaps hold the nearest value.
/// Returns `None` if nothing is voiced.
pub fn interpolate(track: &[Option<f32>]) -> Option<Vec<f32>> {
    let first = track.iter().position(Option::is_some)?;
    let last = track.iter().rposition(Option::is_some)?;
    let mut out = vec![0.0f32; track.len()];
    let mut prev = (first, track[first].unwrap());
    for (i, slot) in out.iter_mut().enumerate() {
        if i <= first {
            *slot = track[first].unwrap();
        } else if i >= last {
            *slot = track[last].unwrap();
        } else if let Some(v) = track[i] {
            *slot = v;
            prev = (i, v);
        } else {
            let next = (i..=last).find(|&k| track[k].is_some()).unwrap();
            let nv = track[next].unwrap();
            let t = (i - prev.0) as f32 / (next - prev.0) as f32;
            *slot = prev.1 + (nv - prev.1) * t;
        }
    }
    Some(out)
}

/// Pitch range as the 10th–90th percentile spread, in semitones.
pub fn range(st: &[Option<f32>]) -> f32 {
    let v: Vec<f32> = st.iter().flatten().copied().collect();
    match (stats::percentile(&v, 10.0), stats::percentile(&v, 90.0)) {
        (Some(lo), Some(hi)) => hi - lo,
        _ => 0.0,
    }
}

/// Pitch movement over the last `window_s` of voiced speech, in semitones
/// (positive = rising). This is what distinguishes the Australian rising
/// terminal on a statement from a falling one.
pub fn final_movement(st: &[Option<f32>], window_s: f32) -> Option<f32> {
    let need = (window_s * FRAME_RATE).round() as usize;
    let voiced: Vec<(f32, f32)> = st
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|s| (i as f32 / FRAME_RATE, s)))
        .collect();
    if voiced.len() < need.max(5) {
        return None;
    }
    let tail = &voiced[voiced.len() - need..];
    let x: Vec<f32> = tail.iter().map(|p| p.0).collect();
    let y: Vec<f32> = tail.iter().map(|p| p.1).collect();
    let duration = x.last()? - x.first()?;
    Some(stats::slope(&x, &y)? * duration)
}

/// A final movement above this many semitones counts as a rising terminal.
pub const RISE_THRESHOLD_ST: f32 = 1.5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octave_is_twelve_semitones() {
        assert!((semitones(220.0, 110.0) - 12.0).abs() < 1e-4);
        assert!((semitones(110.0, 110.0)).abs() < 1e-6);
    }

    #[test]
    fn interpolation_fills_gaps() {
        let t = [None, Some(1.0), None, None, Some(4.0), None];
        assert_eq!(interpolate(&t).unwrap(), vec![1.0, 1.0, 2.0, 3.0, 4.0, 4.0]);
        assert!(interpolate(&[None, None]).is_none());
    }

    #[test]
    fn detects_rise_and_fall() {
        let rising: Vec<Option<f32>> = (0..100).map(|i| Some(if i > 70 { (i - 70) as f32 * 0.15 } else { 0.0 })).collect();
        let falling: Vec<Option<f32>> = rising.iter().map(|v| v.map(|x| -x)).collect();
        let up = final_movement(&rising, 0.3).unwrap();
        let down = final_movement(&falling, 0.3).unwrap();
        assert!(up > RISE_THRESHOLD_ST, "{up}");
        assert!(down < -RISE_THRESHOLD_ST, "{down}");
    }

    #[test]
    fn contour_is_voice_independent() {
        let low: Vec<Option<f32>> = [100.0, 110.0, 120.0].iter().map(|&v| Some(v)).collect();
        let high: Vec<Option<f32>> = [200.0, 220.0, 240.0].iter().map(|&v| Some(v)).collect();
        let (a, _) = to_semitones(&low).unwrap();
        let (b, _) = to_semitones(&high).unwrap();
        for (x, y) in a.iter().zip(&b) {
            assert!((x.unwrap() - y.unwrap()).abs() < 1e-4);
        }
    }
}

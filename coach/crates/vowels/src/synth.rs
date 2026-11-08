//! Test-only formant synthesiser: a glottal pulse train through a cascade of
//! time-varying resonators, with a raised-cosine amplitude envelope.

use std::f32::consts::PI;

const SR: f32 = 16_000.0;

pub fn silence(seconds: f32) -> Vec<f32> {
    vec![0.0; (seconds * SR) as usize]
}

/// A vowel whose F1–F3 move linearly from `targets[0]` (at 20%) to `targets[1]` (at 80%).
pub fn vowel(f0: f32, targets: [(f32, f32, f32); 2], seconds: f32, amp: f32) -> Vec<f32> {
    let n = (seconds * SR) as usize;
    let period = (SR / f0) as usize;
    let mut src = vec![0.0f32; n];
    for i in (0..n).step_by(period.max(1)) {
        src[i] = 1.0;
    }
    // Glottal spectral tilt.
    let (mut y1, mut y2) = (0.0f32, 0.0f32);
    for s in src.iter_mut() {
        let y = *s + 1.6 * y1 - 0.64 * y2;
        y2 = y1;
        y1 = y;
        *s = y;
    }
    let bws = [80.0f32, 100.0, 140.0];
    let mut sig = src;
    for k in 0..5 {
        let (mut p1, mut p2) = (0.0f32, 0.0f32);
        for (i, x) in sig.iter_mut().enumerate() {
            let pos = ((i as f32 / n as f32 - 0.2) / 0.6).clamp(0.0, 1.0);
            let (f, bw) = match k {
                0 => (targets[0].0 + (targets[1].0 - targets[0].0) * pos, bws[0]),
                1 => (targets[0].1 + (targets[1].1 - targets[0].1) * pos, bws[1]),
                2 => (targets[0].2 + (targets[1].2 - targets[0].2) * pos, bws[2]),
                3 => (3500.0, 200.0),
                _ => (4500.0, 250.0),
            };
            let t = 1.0 / SR;
            let c = -(-2.0 * PI * bw * t).exp();
            let b = 2.0 * (-PI * bw * t).exp() * (2.0 * PI * f * t).cos();
            let a = 1.0 - b - c;
            let y = a * *x + b * p1 + c * p2;
            p2 = p1;
            p1 = y;
            *x = y;
        }
    }
    let peak = sig.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    let ramp = (0.03 * SR) as usize;
    sig.iter()
        .enumerate()
        .map(|(i, v)| {
            let edge = i.min(n - 1 - i);
            let env = if edge < ramp { 0.5 - 0.5 * (PI * edge as f32 / ramp as f32).cos() } else { 1.0 };
            amp * env * v / peak
        })
        .collect()
}

/// Intensity in dB per 10 ms frame, aligned like the analysis crate.
pub fn intensity(audio: &[f32]) -> (Vec<f32>, f32) {
    let frames = analysis::pitch::track(audio, &analysis::pitch::PitchConfig::default());
    let offset = frames.first().map(|f| f.time).unwrap_or(0.0);
    (frames.iter().map(|f| analysis::energy::rms_to_db(f.rms)).collect(), offset)
}

//! Band-limited resampling by windowed-sinc interpolation, usable as a stream.
//!
//! Quality is far beyond what speech analysis needs (passband to ~0.45 × the lower
//! rate, Blackman-windowed, 16 zero crossings), and cost is small: about 100 MACs per
//! output sample when downsampling 48 kHz to 16 kHz.

use std::f64::consts::PI;

/// Average interleaved channels to mono.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect()
}

#[derive(Debug, Clone)]
pub struct Resampler {
    in_rate: f64,
    out_rate: f64,
    /// Input samples per output sample.
    step: f64,
    /// Cutoff relative to the input Nyquist (≤ 1).
    cutoff: f64,
    /// Kernel half-width in input samples.
    half_width: usize,
    history: Vec<f32>,
    /// Absolute input index of `history[0]`.
    history_start: u64,
    /// Absolute (fractional) input position of the next output sample.
    next_pos: f64,
}

const ZERO_CROSSINGS: f64 = 16.0;

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let step = in_rate as f64 / out_rate as f64;
        let cutoff = (out_rate as f64 / in_rate as f64).min(1.0) * 0.95;
        let half_width = (ZERO_CROSSINGS / cutoff).ceil() as usize;
        Self {
            in_rate: in_rate as f64,
            out_rate: out_rate as f64,
            step,
            cutoff,
            half_width,
            history: Vec::new(),
            history_start: 0,
            next_pos: 0.0,
        }
    }

    pub fn is_passthrough(&self) -> bool {
        (self.in_rate - self.out_rate).abs() < f64::EPSILON
    }

    fn kernel(&self, x: f64) -> f64 {
        let w = self.half_width as f64;
        if x.abs() >= w {
            return 0.0;
        }
        let arg = x * self.cutoff;
        let sinc = if arg.abs() < 1e-12 { 1.0 } else { (PI * arg).sin() / (PI * arg) };
        // Blackman window over [-w, w].
        let n = (x + w) / (2.0 * w);
        let window = 0.42 - 0.5 * (2.0 * PI * n).cos() + 0.08 * (4.0 * PI * n).cos();
        self.cutoff * sinc * window
    }

    /// Feed input; returns every output sample whose kernel is fully covered.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.is_passthrough() {
            return input.to_vec();
        }
        self.history.extend_from_slice(input);
        let available_end = self.history_start + self.history.len() as u64;
        let hw = self.half_width as f64;
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 2);
        while self.next_pos + hw < available_end as f64 {
            let centre = self.next_pos;
            let lo = (centre - hw).floor().max(0.0) as u64;
            let hi = (centre + hw).ceil() as u64;
            let mut acc = 0.0f64;
            for k in lo..hi {
                if k < self.history_start {
                    continue;
                }
                let s = self.history[(k - self.history_start) as usize] as f64;
                acc += s * self.kernel(centre - k as f64);
            }
            out.push(acc as f32);
            self.next_pos += self.step;
        }
        // Drop history the next kernel can no longer reach.
        let keep_from = (self.next_pos - hw).floor().max(0.0) as u64;
        if keep_from > self.history_start {
            let drop = ((keep_from - self.history_start) as usize).min(self.history.len());
            self.history.drain(..drop);
            self.history_start += drop as u64;
        }
        out
    }

    /// Flush the tail by padding with silence.
    pub fn finish(&mut self) -> Vec<f32> {
        if self.is_passthrough() {
            return Vec::new();
        }
        self.process(&vec![0.0; self.half_width * 2 + 1])
    }
}

/// Resample a complete clip.
pub fn resample(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    let mut r = Resampler::new(in_rate, out_rate);
    let mut out = r.process(input);
    out.extend(r.finish());
    let expected = (input.len() as f64 * out_rate as f64 / in_rate as f64).round() as usize;
    out.truncate(expected);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, rate: u32, seconds: f32) -> Vec<f32> {
        let n = (seconds * rate as f32) as usize;
        (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn keeps_passband_tone() {
        for in_rate in [44_100, 48_000, 22_050, 8_000] {
            let input = tone(440.0, in_rate, 0.5);
            let out = resample(&input, in_rate, 16_000);
            assert_eq!(out.len(), 8000, "{in_rate}");
            let expected = tone(440.0, 16_000, 0.5);
            // Skip edges; compare the middle.
            let err: f32 = out[400..7600].iter().zip(&expected[400..7600]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(err < 0.01, "{in_rate}: max error {err}");
        }
    }

    #[test]
    fn rejects_aliasing_tone() {
        // 12 kHz at 48 kHz would alias to 4 kHz at 16 kHz without filtering.
        let out = resample(&tone(12_000.0, 48_000, 0.5), 48_000, 16_000);
        assert!(rms(&out[400..7600]) < 0.005, "alias rms {}", rms(&out[400..7600]));
    }

    #[test]
    fn streaming_equals_one_shot() {
        let input = tone(300.0, 48_000, 0.3);
        let whole = resample(&input, 48_000, 16_000);
        let mut r = Resampler::new(48_000, 16_000);
        let mut streamed = Vec::new();
        for chunk in input.chunks(333) {
            streamed.extend(r.process(chunk));
        }
        streamed.extend(r.finish());
        streamed.truncate(whole.len());
        for (a, b) in streamed.iter().zip(&whole) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn downmix_averages() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
    }
}

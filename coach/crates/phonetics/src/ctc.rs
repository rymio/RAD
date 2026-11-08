//! CTC posteriors, greedy decoding and Viterbi forced alignment.

use serde::{Deserialize, Serialize};

/// Frame-level log-probabilities from a CTC acoustic model, row-major `[frames × vocab]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Posteriors {
    pub frames: usize,
    pub vocab: usize,
    /// Natural-log probabilities.
    pub log_probs: Vec<f32>,
    /// Seconds per frame (wav2vec2: 0.02).
    pub frame_s: f32,
}

impl Posteriors {
    /// Build from raw logits by log-softmax over each frame.
    pub fn from_logits(logits: &[f32], frames: usize, vocab: usize, frame_s: f32) -> Self {
        assert_eq!(logits.len(), frames * vocab, "logits shape mismatch");
        let mut log_probs = Vec::with_capacity(logits.len());
        for row in logits.chunks_exact(vocab) {
            let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let sum: f32 = row.iter().map(|v| (v - max).exp()).sum();
            let lse = max + sum.ln();
            log_probs.extend(row.iter().map(|v| v - lse));
        }
        Self { frames, vocab, log_probs, frame_s }
    }

    #[inline]
    pub fn row(&self, t: usize) -> &[f32] {
        &self.log_probs[t * self.vocab..(t + 1) * self.vocab]
    }

    #[inline]
    pub fn at(&self, t: usize, label: usize) -> f32 {
        self.log_probs[t * self.vocab + label]
    }

    pub fn time(&self, frame: usize) -> f32 {
        frame as f32 * self.frame_s
    }
}

/// A label occupying frames `start..end` (end exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelSpan {
    pub label: usize,
    pub start: usize,
    pub end: usize,
}

/// Best-path decoding: argmax per frame, collapse repeats, drop blanks.
/// Labels listed in `ignore` (sentence markers, unknown) are dropped too.
pub fn greedy(p: &Posteriors, blank: usize, ignore: &[usize]) -> Vec<LabelSpan> {
    let mut out: Vec<LabelSpan> = Vec::new();
    let mut prev: Option<usize> = None;
    for t in 0..p.frames {
        let row = p.row(t);
        let best = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap_or(blank);
        if best == blank || ignore.contains(&best) {
            prev = None;
            continue;
        }
        match (prev, out.last_mut()) {
            (Some(pl), Some(last)) if pl == best && last.end == t => last.end = t + 1,
            _ => out.push(LabelSpan { label: best, start: t, end: t + 1 }),
        }
        prev = Some(best);
    }
    out
}

/// Viterbi forced alignment of `labels` to the posteriors under the CTC topology
/// (optional blanks between labels, a repeated label needs a blank in between).
/// Returns one span per label, or `None` if the audio is too short for the sequence.
pub fn force_align(p: &Posteriors, labels: &[usize], blank: usize) -> Option<Vec<LabelSpan>> {
    if labels.is_empty() || p.frames == 0 {
        return None;
    }
    // Extended sequence: blank, l1, blank, l2, …, lN, blank.
    let s_len = 2 * labels.len() + 1;
    let ext = |s: usize| if s.is_multiple_of(2) { blank } else { labels[s / 2] };
    let t_len = p.frames;
    let neg = f32::NEG_INFINITY;
    let mut score = vec![neg; t_len * s_len];
    let mut back = vec![0u8; t_len * s_len]; // 0 = stay, 1 = from s-1, 2 = from s-2
    let idx = |t: usize, s: usize| t * s_len + s;

    score[idx(0, 0)] = p.at(0, blank);
    if s_len > 1 {
        score[idx(0, 1)] = p.at(0, ext(1));
    }
    for t in 1..t_len {
        // States reachable at t must still be able to finish by T: s ≥ s_len − 2(T − t) − 1 roughly;
        // the full sweep is cheap enough (labels ≤ ~200, frames ≤ ~1500) to skip pruning.
        for s in 0..s_len {
            let mut best = score[idx(t - 1, s)];
            let mut from = 0u8;
            if s >= 1 && score[idx(t - 1, s - 1)] > best {
                best = score[idx(t - 1, s - 1)];
                from = 1;
            }
            if s >= 2 && ext(s) != blank && ext(s) != ext(s - 2) && score[idx(t - 1, s - 2)] > best {
                best = score[idx(t - 1, s - 2)];
                from = 2;
            }
            if best > neg {
                score[idx(t, s)] = best + p.at(t, ext(s));
                back[idx(t, s)] = from;
            }
        }
    }

    // End in the final blank or the final label, whichever scores higher.
    let last = t_len - 1;
    let (mut s, end_score) = [s_len - 1, s_len - 2]
        .into_iter()
        .map(|s| (s, score[idx(last, s)]))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if !end_score.is_finite() {
        return None;
    }

    // Backtrack the state per frame.
    let mut states = vec![0usize; t_len];
    for t in (0..t_len).rev() {
        states[t] = s;
        if t > 0 {
            s -= back[idx(t, s)] as usize;
        }
    }

    let mut spans: Vec<Option<LabelSpan>> = vec![None; labels.len()];
    for (t, &st) in states.iter().enumerate() {
        if st % 2 == 1 {
            let k = st / 2;
            match &mut spans[k] {
                Some(span) => span.end = t + 1,
                None => spans[k] = Some(LabelSpan { label: labels[k], start: t, end: t + 1 }),
            }
        }
    }
    spans.into_iter().collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Posteriors where frame `t` strongly predicts `path[t]` (others share the rest).
    pub fn peaked(path: &[usize], vocab: usize, confidence: f32) -> Posteriors {
        let mut logits = Vec::with_capacity(path.len() * vocab);
        for &l in path {
            for v in 0..vocab {
                logits.push(if v == l { confidence } else { 0.0 });
            }
        }
        Posteriors::from_logits(&logits, path.len(), vocab, 0.02)
    }

    #[test]
    fn log_softmax_rows_sum_to_one() {
        let p = peaked(&[1, 2, 0], 4, 3.0);
        for t in 0..3 {
            let s: f32 = p.row(t).iter().map(|v| v.exp()).sum();
            assert!((s - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn greedy_collapses_repeats_and_blanks() {
        // blank=0: "_ a a _ b _ b c c"
        let p = peaked(&[0, 1, 1, 0, 2, 0, 2, 3, 3], 5, 6.0);
        let spans = greedy(&p, 0, &[]);
        let labels: Vec<usize> = spans.iter().map(|s| s.label).collect();
        assert_eq!(labels, vec![1, 2, 2, 3]);
        assert_eq!((spans[0].start, spans[0].end), (1, 3));
        assert_eq!((spans[3].start, spans[3].end), (7, 9));
    }

    #[test]
    fn greedy_skips_ignored_labels() {
        let p = peaked(&[4, 1, 4, 2], 5, 6.0);
        assert_eq!(greedy(&p, 0, &[4]).iter().map(|s| s.label).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn forced_alignment_finds_label_positions() {
        let p = peaked(&[0, 0, 1, 1, 0, 2, 2, 2, 0, 3, 0], 5, 5.0);
        let spans = force_align(&p, &[1, 2, 3], 0).unwrap();
        assert_eq!(spans.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(), vec![(2, 4), (5, 8), (9, 10)]);
    }

    #[test]
    fn forced_alignment_handles_repeated_labels_and_mismatch() {
        // The model hears 1 1 (with a blank between) but weakly; we force 1 1 anyway.
        let p = peaked(&[1, 0, 1, 0], 3, 2.0);
        let spans = force_align(&p, &[1, 1], 0).unwrap();
        assert_eq!(spans.len(), 2);
        assert!(spans[0].end <= spans[1].start);
        // Forcing a label the audio does not contain still yields a monotonic path.
        let q = peaked(&[0, 1, 0, 1, 0], 4, 4.0);
        let forced = force_align(&q, &[1, 3, 1], 0).unwrap();
        assert!(forced.windows(2).all(|w| w[0].end <= w[1].start));
    }

    #[test]
    fn too_short_is_none() {
        let p = peaked(&[1], 3, 2.0);
        assert!(force_align(&p, &[1, 1], 0).is_none()); // needs a blank between
        assert!(force_align(&p, &[], 0).is_none());
    }
}

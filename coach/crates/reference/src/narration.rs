//! A person's recording of the whole text, cut into one clip per sentence.
//!
//! Until word alignment lands (M2), sentences are found from pauses: with N sentences
//! the N − 1 longest pauses in the narration become the sentence boundaries. Narrators
//! pause longer between sentences than inside them, so this is right for read speech.

use analysis::{energy, pitch, vad};
use serde::{Deserialize, Serialize};

use crate::{ReferenceError, ReferenceSource, RenderedAudio, Result, SourceKind};

const MARGIN_S: f32 = 0.15;
const RATE: f32 = analysis::SAMPLE_RATE as f32;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub start: f32,
    pub end: f32,
}

#[derive(Debug, Clone)]
pub struct NarrationSource {
    id: String,
    audio: Vec<f32>,
    spans: Vec<Span>,
}

impl NarrationSource {
    /// Split `audio` (16 kHz mono) into `sentence_count` clips.
    pub fn new(id: impl Into<String>, audio: Vec<f32>, sentence_count: usize) -> Result<Self> {
        let spans = split(&audio, sentence_count)?;
        Ok(Self { id: id.into(), audio, spans })
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    pub fn duration_s(&self) -> f32 {
        self.audio.len() as f32 / RATE
    }

    /// The span for sentence `index`, if the narration has one.
    pub fn clip(&self, index: usize) -> Option<Vec<f32>> {
        let span = self.spans.get(index)?;
        let lo = (((span.start - MARGIN_S).max(0.0)) * RATE) as usize;
        let hi = (((span.end + MARGIN_S) * RATE) as usize).min(self.audio.len());
        (lo < hi).then(|| self.audio[lo..hi].to_vec())
    }
}

/// Sentence spans from speech segments: the `count − 1` longest pauses are boundaries.
pub fn split(audio: &[f32], count: usize) -> Result<Vec<Span>> {
    let frames = pitch::track(audio, &pitch::PitchConfig::default());
    let offset = frames.first().map(|f| f.time).unwrap_or(0.0);
    let db: Vec<f32> = frames.iter().map(|f| energy::rms_to_db(f.rms)).collect();
    let segments = vad::segments(&db, offset, &vad::VadConfig::default());
    if segments.is_empty() {
        return Err(analysis::AnalysisError::NoSpeech.into());
    }
    let count = count.max(1);
    let mut gaps: Vec<(usize, f32)> = segments.windows(2).enumerate().map(|(i, w)| (i, w[1].start - w[0].end)).collect();
    gaps.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut cuts: Vec<usize> = gaps.iter().take(count - 1).map(|(i, _)| *i).collect();
    cuts.sort_unstable();

    let mut spans = Vec::with_capacity(count);
    let mut first = 0;
    for cut in cuts.into_iter().chain(std::iter::once(segments.len() - 1)) {
        spans.push(Span { start: segments[first].start, end: segments[cut].end });
        first = cut + 1;
    }
    Ok(spans)
}

impl ReferenceSource for NarrationSource {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Narration
    }
    fn render(&mut self, index: usize, _text: &str) -> Result<RenderedAudio> {
        let samples = self.clip(index).ok_or(ReferenceError::NoAudioForSentence(index))?;
        Ok(RenderedAudio { samples, word_timings: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32) -> Vec<f32> {
        (0..(seconds * RATE) as usize)
            .map(|i| {
                let t = i as f32 / RATE;
                0.3 * (2.0 * std::f32::consts::PI * 150.0 * t).sin() + 0.1 * (2.0 * std::f32::consts::PI * 300.0 * t).sin()
            })
            .collect()
    }
    fn quiet(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * RATE) as usize]
    }

    #[test]
    fn longest_pauses_become_sentence_boundaries() {
        // Sentence 1 has an internal pause of 0.3 s; sentence breaks are 0.8 s and 0.7 s.
        let mut audio = quiet(0.3);
        for (dur, gap) in [(0.6, 0.3), (0.5, 0.8), (0.9, 0.7), (0.4, 0.3), (0.5, 0.0)] {
            audio.extend(tone(dur));
            audio.extend(quiet(gap));
        }
        audio.extend(quiet(0.3));
        let spans = split(&audio, 3).unwrap();
        assert_eq!(spans.len(), 3, "{spans:?}");
        assert!((spans[0].start - 0.3).abs() < 0.05);
        assert!((spans[0].end - 1.7).abs() < 0.05, "{spans:?}"); // 0.3 + 0.6 + 0.3 + 0.5
        assert!((spans[1].end - 3.4).abs() < 0.05, "{spans:?}"); // + 0.8 + 0.9
        let mut src = NarrationSource::new("n", audio, 3).unwrap();
        assert!(src.render(2, "x").is_ok());
        assert!(matches!(src.render(3, "x"), Err(ReferenceError::NoAudioForSentence(3))));
    }

    #[test]
    fn fewer_pauses_than_sentences_leaves_later_sentences_empty() {
        let mut audio = quiet(0.3);
        audio.extend(tone(1.0));
        audio.extend(quiet(0.3));
        let spans = split(&audio, 4).unwrap();
        assert_eq!(spans.len(), 1);
    }
}

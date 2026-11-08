//! M2 speech engines: Whisper word timings, Silero VAD, wav2vec2 phoneme scoring.
//!
//! Each engine is optional. Without its model on disk (or the `rad-coach-asr` helper, which runs
//! Whisper and Silero), Coach keeps working with prosody-only feedback and says so in the UI.
//! In RAD, whisper.cpp runs in that helper process (see `asr::remote`): its ggml can't share a
//! process with RAD's llama.cpp. wav2vec2 runs here, on ONNX Runtime loaded at run time.

use asr::words::{self, SentenceWord, WordTiming};
use phonetics::{AttemptScore, ReferencePhones};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum EngineState {
    /// This build has no support (rebuild with `--features speech`).
    NotBuilt,
    /// Supported, but the model is not on disk yet.
    MissingModel,
    Ready,
    Failed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeechStatus {
    pub words: EngineState,
    pub vad: EngineState,
    pub phonemes: EngineState,
}

/// What the speech engines learned about a reference sentence.
#[derive(Debug, Clone, Default)]
pub struct ReferenceSpeech {
    pub words: Vec<SentenceWord>,
    /// Each sentence word's span in the reference clip.
    pub spans: Vec<Option<(f32, f32)>>,
    pub phones: Option<ReferencePhones>,
}

impl ReferenceSpeech {
    /// Words with spans and phones, for vowel measurement.
    pub fn word_inputs(&self) -> Vec<vowels::WordInput> {
        self.words
            .iter()
            .enumerate()
            .map(|(i, w)| vowels::WordInput {
                index: i,
                text: w.display.clone(),
                span: self.span(i),
                phones: self
                    .phones
                    .as_ref()
                    .map(|p| {
                        p.word_of
                            .iter()
                            .zip(p.phones.iter().zip(&p.spans_s))
                            .filter(|(wi, _)| **wi == Some(i))
                            .map(|(_, (ph, s))| vowels::PhoneSpan { phone: ph.clone(), start_s: s.0, end_s: s.1 })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// Reference span of a word: from its phones when scored, else from Whisper.
    pub fn span(&self, index: usize) -> Option<(f32, f32)> {
        self.phones.as_ref().and_then(|p| p.word_span(index)).or_else(|| self.spans.get(index).copied().flatten())
    }
}

#[derive(Debug, Clone, Default)]
pub struct AttemptSpeech {
    pub heard: Vec<WordTiming>,
    /// Sentence word indices the recogniser did not hear (empty when Whisper is unavailable).
    pub missed: Vec<usize>,
    pub score: Option<AttemptScore>,
    /// Each sentence word's span in the attempt.
    pub spans: Vec<Option<(f32, f32)>>,
}

impl AttemptSpeech {
    /// Words with this attempt's spans and phones, for vowel measurement.
    pub fn word_inputs(&self, words: &[SentenceWord]) -> Vec<vowels::WordInput> {
        words
            .iter()
            .enumerate()
            .map(|(i, w)| vowels::WordInput {
                index: i,
                text: w.display.clone(),
                span: self.spans.get(i).copied().flatten(),
                phones: self
                    .score
                    .as_ref()
                    .and_then(|s| s.words.iter().find(|ws| ws.index == i))
                    .map(|ws| {
                        ws.phones
                            .iter()
                            .map(|p| vowels::PhoneSpan { phone: p.phone.clone(), start_s: p.start_s, end_s: p.end_s })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }
}

pub struct Speech {
    transcriber: Option<asr::remote::Transcriber>,
    vad: Option<asr::remote::SileroVad>,
    phonemes: Option<phonetics::model::PhonemeModel>,
    pub status: SpeechStatus,
    score_cfg: phonetics::ScoreConfig,
}

impl Speech {
    /// `helper`: the `rad-coach-asr` executable, which runs Whisper and Silero.
    pub fn load(models: &models::ModelStore, models_dir: &std::path::Path, helper: Option<&std::path::Path>) -> Self {
        use models::Role;

        let ready = |role: Role| -> Option<std::path::PathBuf> {
            let m = models.manifest();
            let mut candidates: Vec<&models::ModelEntry> = m.models.iter().filter(|e| e.role == role).collect();
            candidates.sort_by_key(|e| !e.default);
            candidates.into_iter().find_map(|e| models.ready_path(&e.id))
        };

        const NO_HELPER: &str = "the speech helper isn't built; run ./scripts/build-coach.sh";
        let (transcriber, words_state) = match (ready(Role::Asr), helper) {
            (None, _) => (None, EngineState::MissingModel),
            (Some(_), None) => (None, EngineState::Failed(NO_HELPER.into())),
            (Some(p), Some(h)) => match asr::remote::Transcriber::load(h, &p) {
                Ok(t) => (Some(t), EngineState::Ready),
                Err(e) => (None, EngineState::Failed(e.to_string())),
            },
        };
        let (vad, vad_state) = match (ready(Role::Vad), helper) {
            (None, _) => (None, EngineState::MissingModel),
            (Some(_), None) => (None, EngineState::Failed(NO_HELPER.into())),
            (Some(p), Some(h)) => match asr::remote::SileroVad::load(h, &p) {
                Ok(v) => (Some(v), EngineState::Ready),
                Err(e) => (None, EngineState::Failed(e.to_string())),
            },
        };
        let (phonemes, phon_state) = match ready(Role::Phonemes) {
            None => (None, EngineState::MissingModel),
            Some(p) => {
                let vocab = models_dir.join(phonetics::VOCAB_FILE);
                match phonetics::model::PhonemeModel::load(&p, &vocab, models_dir) {
                    Ok(m) => (Some(m), EngineState::Ready),
                    Err(e) => (None, EngineState::Failed(e.to_string())),
                }
            }
        };
        let status = SpeechStatus { words: words_state, vad: vad_state, phonemes: phon_state };
        tracing::info!(?status, "speech engines");
        Self { transcriber, vad, phonemes, status, score_cfg: Default::default() }
    }

    /// True when word timings are available (needed for vowel measurement).
    pub fn has_words(&self) -> bool {
        self.status.words == EngineState::Ready
    }

    /// Silero speech segments, or `None` to use the energy detector.
    pub fn segments(&mut self, _samples: &[f32]) -> Option<Vec<analysis::vad::Segment>> {
        if let Some(vad) = self.vad.as_mut() {
            match vad.segments(_samples) {
                Ok(segs) if !segs.is_empty() => {
                    return Some(segs.into_iter().map(|(start, end)| analysis::vad::Segment { start, end }).collect());
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("Silero VAD failed, using energy VAD: {e}"),
            }
        }
        None
    }

    pub fn reference(&mut self, _samples: &[f32], sentence: &str) -> ReferenceSpeech {
        let words = words::sentence_words(sentence);
        #[allow(unused_mut)]
        let mut out = ReferenceSpeech { spans: vec![None; words.len()], words, phones: None };
        {
            if let Some(t) = self.transcriber.as_ref() {
                match t.words(_samples, Some(sentence)) {
                    Ok(heard) => out.spans = words::word_spans(&out.words, &heard),
                    Err(e) => tracing::warn!("reference transcription failed: {e}"),
                }
            }
            if let Some(m) = self.phonemes.as_mut() {
                match m.posteriors(_samples) {
                    Ok(post) => {
                        let keys: Vec<String> = out.words.iter().map(|w| clean_word(&w.display)).collect();
                        out.phones = ReferencePhones::build(&post, &m.vocab, &keys, &out.spans);
                    }
                    Err(e) => tracing::warn!("reference phoneme analysis failed: {e}"),
                }
            }
        }
        out
    }

    /// Words heard in a whole recording (a page read in one go). Needs Whisper.
    ///
    /// Whisper mistimes and drops words across long silences in one long clip, so the
    /// recording is cut at pauses and each chunk is transcribed on its own.
    pub fn transcribe(&mut self, _samples: &[f32]) -> Result<Vec<WordTiming>, String> {
        if self.transcriber.is_some() {
            let segments = self.segments(_samples).unwrap_or_else(|| energy_segments(_samples));
            let rate = analysis::SAMPLE_RATE as f32;
            let t = self.transcriber.as_ref().expect("checked above");
            let mut heard = Vec::new();
            for (start, end) in page_chunks(&segments, _samples.len() as f32 / rate) {
                let lo = ((start - CHUNK_PAD_S).max(0.0) * rate) as usize;
                let hi = (((end + CHUNK_PAD_S) * rate) as usize).min(_samples.len());
                if hi <= lo {
                    continue;
                }
                let offset = lo as f32 / rate;
                for mut w in t.words(&_samples[lo..hi], None).map_err(|e| e.to_string())? {
                    w.start_s += offset;
                    w.end_s += offset;
                    heard.push(w);
                }
            }
            return Ok(heard);
        }
        Err("Reading a whole page needs Whisper word timings. Download Whisper small.en in Models, then choose Reload speech models.".into())
    }

    /// `heard`: words already recognised in this clip (from a whole-page transcript), so
    /// Whisper is not run again.
    pub fn attempt(&mut self, _samples: &[f32], _reference: &ReferenceSpeech, _heard: Option<Vec<WordTiming>>) -> AttemptSpeech {
        #[allow(unused_mut)]
        let mut out = AttemptSpeech { spans: vec![None; _reference.words.len()], ..Default::default() };
        {
            let heard = match _heard {
                Some(h) => Some(h),
                None => self.transcriber.as_ref().and_then(|t| {
                    t.words(_samples, None).map_err(|e| tracing::warn!("attempt transcription failed: {e}")).ok()
                }),
            };
            if let Some(heard) = heard {
                out.missed = words::missed(&_reference.words, &heard);
                out.spans = words::word_spans(&_reference.words, &heard);
                out.heard = heard;
            }
            if let (Some(m), Some(ref_phones)) = (self.phonemes.as_mut(), _reference.phones.as_ref()) {
                match m.posteriors(_samples) {
                    Ok(post) => out.score = ref_phones.score(&post, &m.vocab, &self.score_cfg),
                    Err(e) => tracing::warn!("attempt phoneme analysis failed: {e}"),
                }
            }
            // Phone alignment gives tighter word spans than Whisper.
            if let Some(score) = &out.score {
                for w in &score.words {
                    if let Some(slot) = out.spans.get_mut(w.index) {
                        *slot = Some((w.start_s, w.end_s));
                    }
                }
            }
        }
        out
    }
}

/// Air kept around each chunk of a whole-page recording.
const CHUNK_PAD_S: f32 = 0.2;
/// Speech runs closer than this are transcribed together.
const CHUNK_JOIN_GAP_S: f32 = 0.6;
/// Chunks grow up to this long; longer unbroken speech is split evenly below Whisper's 30 s window.
const CHUNK_MAX_S: f32 = 20.0;
const CHUNK_HARD_MAX_S: f32 = 25.0;

/// Group speech segments into chunks to transcribe one at a time.
pub fn page_chunks(segments: &[analysis::vad::Segment], duration_s: f32) -> Vec<(f32, f32)> {
    let mut chunks: Vec<(f32, f32)> = Vec::new();
    for s in segments {
        let (start, end) = (s.start.max(0.0), s.end.min(duration_s));
        if end <= start {
            continue;
        }
        match chunks.last_mut() {
            Some(last) if start - last.1 < CHUNK_JOIN_GAP_S && end - last.0 <= CHUNK_MAX_S => last.1 = end,
            _ => chunks.push((start, end)),
        }
    }
    chunks
        .into_iter()
        .flat_map(|(a, b)| {
            let parts = ((b - a) / CHUNK_HARD_MAX_S).ceil().max(1.0) as usize;
            let step = (b - a) / parts as f32;
            (0..parts).map(move |k| (a + step * k as f32, a + step * (k + 1) as f32))
        })
        .collect()
}

/// Energy speech segments, for when Silero is not available.
fn energy_segments(samples: &[f32]) -> Vec<analysis::vad::Segment> {
    const WINDOW: usize = 400;
    let db: Vec<f32> = (0..samples.len().saturating_sub(WINDOW))
        .step_by(analysis::HOP)
        .map(|i| {
            let w = &samples[i..i + WINDOW];
            analysis::energy::rms_to_db((w.iter().map(|x| x * x).sum::<f32>() / WINDOW as f32).sqrt())
        })
        .collect();
    let cfg = analysis::vad::VadConfig { min_pause_s: 0.3, ..Default::default() };
    analysis::vad::segments(&db, WINDOW as f32 / 2.0 / analysis::SAMPLE_RATE as f32, &cfg)
}

/// A sentence word without surrounding punctuation, as the coach and UI quote it ("mate," → "mate").
pub fn clean_word(display: &str) -> String {
    display.trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f32, end: f32) -> analysis::vad::Segment {
        analysis::vad::Segment { start, end }
    }

    #[test]
    fn page_chunks_join_close_speech_and_split_long_runs() {
        // Two runs 0.3 s apart join; a 3 s pause starts a new chunk.
        assert_eq!(page_chunks(&[seg(0.0, 2.0), seg(2.3, 4.0), seg(7.0, 9.0)], 10.0), vec![(0.0, 4.0), (7.0, 9.0)]);
        // 60 s of unbroken speech is split into pieces Whisper handles well.
        let long = page_chunks(&[seg(0.0, 60.0)], 60.0);
        assert_eq!(long.len(), 3);
        assert!(long.iter().all(|(a, b)| b - a <= CHUNK_HARD_MAX_S));
    }

    #[test]
    fn energy_segments_find_speech_between_silences() {
        let mut audio = vec![0.0f32; 16_000];
        audio.extend((0..16_000).map(|i| 0.3 * (i as f32 * 0.05).sin()));
        audio.extend(vec![0.0f32; 16_000]);
        let segs = energy_segments(&audio);
        assert_eq!(segs.len(), 1);
        assert!((segs[0].start - 1.0).abs() < 0.1 && (segs[0].end - 2.0).abs() < 0.1, "{segs:?}");
    }

    #[test]
    fn clean_word_keeps_inner_apostrophes() {
        assert_eq!(clean_word("\"G'day"), "G'day");
        assert_eq!(clean_word("mate,"), "mate");
        assert_eq!(clean_word("going?"), "going");
    }
}

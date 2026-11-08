//! M3: calibration, the reference speaker's vowel space, vowel and R comparison, vowel chart.

use std::sync::Arc;

use analysis::UtteranceAnalysis;
use asr::words::sentence_words;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use vowels::calibration::{self, Coverage, PASSAGE};
use vowels::sets::{is_stopword, word_vowels};
use vowels::{Cloud, Grade, RhoticComparison, SharedNorms, Token, VowelComparison, VowelSpace};

use crate::accent_coach::commands::reference_clip;
use crate::accent_coach::state::{AppState, CalibrationTake, RefClip};

type Shared<'a> = State<'a, Mutex<AppState>>;
type CmdResult<T> = Result<T, String>;

/// Why vowel feedback is or is not available.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VowelStatus {
    Ready,
    /// Whisper word timings are not available, so vowels cannot be located.
    NeedsWords,
    /// The learner has not calibrated their voice.
    NeedsCalibration,
    /// The reference has not shown enough different vowels yet.
    NeedsReference { shared: Vec<String>, missing: Vec<String>, prepared: bool },
}

#[derive(Debug, Clone, Serialize)]
pub struct VowelReport {
    pub status: VowelStatus,
    pub vowels: Vec<VowelComparison>,
    pub rhotic: Vec<RhoticComparison>,
}

impl VowelReport {
    fn unavailable(status: VowelStatus) -> Self {
        Self { status, vowels: Vec::new(), rhotic: Vec::new() }
    }
}

pub fn save_learner(st: &AppState) {
    match serde_json::to_string(&st.learner) {
        Ok(json) => {
            if let Err(e) = st.store.set_learner_profile(&json) {
                tracing::warn!("could not save learner profile: {e}");
            }
        }
        Err(e) => tracing::warn!("could not serialise learner profile: {e}"),
    }
}

/// Measured vowels for a recording, given its words.
pub fn tokens_for(samples: &[f32], analysis: &UtteranceAnalysis, words: &[vowels::WordInput]) -> Vec<Token> {
    if words.iter().all(|w| w.span.is_none()) {
        return Vec::new();
    }
    let intensity = vowels::Intensity { db: &analysis.intensity_db, offset_s: analysis.frame_offset_s };
    vowels::tokens(samples, intensity, words)
}

/// The reference speaker's vowels across every sentence of the text analysed so far.
fn reference_space(st: &AppState) -> VowelSpace {
    let mut space = VowelSpace::default();
    if let Some(t) = &st.text {
        let mut clips: Vec<(&usize, &Arc<RefClip>)> = t.cache.iter().collect();
        clips.sort_by_key(|(i, _)| **i);
        for (_, clip) in clips {
            space.add(clip.vowels.iter().cloned());
        }
    }
    space
}

fn all_prepared(st: &AppState) -> bool {
    st.text.as_ref().is_some_and(|t| (0..t.sentences.len()).all(|i| t.cache.contains_key(&i)))
}

fn norms(st: &AppState) -> Result<SharedNorms, VowelStatus> {
    if !st.speech.has_words() {
        return Err(VowelStatus::NeedsWords);
    }
    if st.learner.calibrated_at.is_none() {
        return Err(VowelStatus::NeedsCalibration);
    }
    vowels::shared_norms(&reference_space(st), &st.learner.vowels).map_err(|s| VowelStatus::NeedsReference {
        shared: s.shared.iter().map(|x| x.keyword().to_string()).collect(),
        missing: s.missing_corners.iter().map(|x| x.to_string()).collect(),
        prepared: all_prepared(st),
    })
}

/// Compare an attempt's vowels with the reference sentence's.
pub fn report(st: &AppState, clip: &RefClip, attempt: &[Token]) -> VowelReport {
    match norms(st) {
        Err(status) => VowelReport::unavailable(status),
        Ok(n) => {
            let sv = vowels::compare(&clip.vowels, attempt, &n.reference, &n.learner);
            VowelReport { status: VowelStatus::Ready, vowels: sv.vowels, rhotic: sv.rhotic }
        }
    }
}

/// Coach findings: target vowels that are not close, and pronounced R's.
pub fn findings(r: &VowelReport) -> Vec<coach::issues::VowelFinding> {
    let mut out = Vec::new();
    for c in r.vowels.iter().filter(|c| c.target && c.grade != Grade::Good) {
        out.push(coach::issues::VowelFinding {
            feature: c.set.code().to_string(),
            word: c.word.clone(),
            severity: ((c.distance - vowels::score::GOOD) * 0.6 + 0.35).clamp(0.3, 1.0),
            detail: c.detail.clone(),
            hint: c.hint.clone().unwrap_or_default(),
        });
    }
    for x in r.rhotic.iter().filter(|x| x.flagged) {
        out.push(coach::issues::VowelFinding {
            feature: coach::schema::feature::NON_RHOTIC.to_string(),
            word: x.word.clone(),
            severity: (0.35 + (x.reference_ratio - x.you_ratio - vowels::score::RHOTIC_MARGIN) * 2.5).clamp(0.35, 1.0),
            detail: x.detail.clone(),
            hint: x.hint.clone().unwrap_or_default(),
        });
    }
    out
}

// ---------- vowel chart ----------

#[derive(Debug, Clone, Serialize)]
pub struct ChartWord {
    word_index: usize,
    word: String,
    set: vowels::Set,
    keyword: &'static str,
    target: bool,
    /// Normalised points: onset and offset for diphthongs, the middle otherwise.
    reference: Vec<[f32; 2]>,
    you: Option<Vec<[f32; 2]>>,
    grade: Option<Grade>,
    hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VowelChart {
    status: VowelStatus,
    /// The Australian reference speaker's vowel categories across the text.
    reference: Vec<Cloud>,
    /// The learner's vowels from calibration.
    yours: Vec<Cloud>,
    /// This sentence's measured vowels.
    words: Vec<ChartWord>,
    rhotic: Vec<RhoticComparison>,
    analysed: usize,
    sentences: usize,
}

fn chart(st: &mut AppState) -> CmdResult<VowelChart> {
    let current = st.current;
    let clip = reference_clip(st, current)?;
    let status = match norms(st) {
        Ok(_) => VowelStatus::Ready,
        Err(s) => s,
    };
    let ref_space = reference_space(st);
    let shared = norms(st).ok();
    // Without a calibration the reference can still be drawn, normalised on its own.
    let ref_norm = shared.as_ref().map(|n| n.reference).or_else(|| vowels::shared_norms(&ref_space, &ref_space).ok().map(|n| n.reference));
    let Some(ref_norm) = ref_norm else {
        return Ok(VowelChart {
            status,
            reference: Vec::new(),
            yours: Vec::new(),
            words: Vec::new(),
            rhotic: Vec::new(),
            analysed: st.text.as_ref().map(|t| t.cache.len()).unwrap_or(0),
            sentences: st.text.as_ref().map(|t| t.sentences.len()).unwrap_or(0),
        });
    };
    let last = st.last_attempt.as_ref().filter(|a| a.sentence == current);
    let words = clip
        .vowels
        .iter()
        .map(|t| {
            let cmp = last.and_then(|a| a.vowels.vowels.iter().find(|c| c.word_index == t.word_index && c.set == t.set));
            let reference = if t.set.is_diphthong() {
                vec![ref_norm.z(t.measure.onset), ref_norm.z(t.measure.offset)]
            } else {
                vec![ref_norm.z(t.measure.mid)]
            };
            ChartWord {
                word_index: t.word_index,
                word: t.word.clone(),
                set: t.set,
                keyword: t.set.keyword(),
                target: t.set.is_target(),
                reference: cmp.map(|c| c.reference.clone()).unwrap_or(reference),
                you: cmp.map(|c| c.you.clone()),
                grade: cmp.map(|c| c.grade),
                hint: cmp.and_then(|c| c.hint.clone()),
            }
        })
        .collect();
    Ok(VowelChart {
        status,
        reference: vowels::clouds(&ref_space, &ref_norm),
        yours: shared.as_ref().map(|n| vowels::clouds(&st.learner.vowels, &n.learner)).unwrap_or_default(),
        words,
        rhotic: last.map(|a| a.vowels.rhotic.clone()).unwrap_or_default(),
        analysed: st.text.as_ref().map(|t| t.cache.len()).unwrap_or(0),
        sentences: st.text.as_ref().map(|t| t.sentences.len()).unwrap_or(0),
    })
}

#[tauri::command]
pub async fn coach_vowel_chart(state: Shared<'_>) -> CmdResult<VowelChart> {
    chart(&mut state.lock())
}

#[derive(Serialize, Clone)]
struct PrepareProgress {
    done: usize,
    total: usize,
}

/// Analyse every reference sentence so the reference's vowel space covers the whole text.
/// Locks per sentence so the UI stays responsive.
#[tauri::command]
pub async fn coach_prepare_reference(app: AppHandle, state: Shared<'_>) -> CmdResult<VowelChart> {
    let total = state.lock().text.as_ref().map(|t| t.sentences.len()).ok_or("Open a text first.")?;
    for i in 0..total {
        {
            let mut st = state.lock();
            if let Err(e) = reference_clip(&mut st, i) {
                tracing::warn!("reference sentence {} not analysed: {e}", i + 1);
            }
        }
        let _ = app.emit("coach-vowel-progress", PrepareProgress { done: i + 1, total });
    }
    chart(&mut state.lock())
}

// ---------- calibration ----------

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationInfo {
    passage: Vec<&'static str>,
    /// Passage indices read so far in this calibration.
    taken: Vec<usize>,
    coverage: Coverage,
    calibrated: bool,
    /// Whisper is ready, so calibration can run.
    available: bool,
}

fn draft_space(st: &AppState) -> VowelSpace {
    VowelSpace::new(st.calibration.values().flat_map(|t| t.tokens.iter().cloned()).collect())
}

fn info(st: &AppState) -> CalibrationInfo {
    let space = if st.calibration.is_empty() { st.learner.vowels.clone() } else { draft_space(st) };
    CalibrationInfo {
        passage: PASSAGE.to_vec(),
        taken: st.calibration.keys().copied().collect(),
        coverage: calibration::coverage(&space),
        calibrated: st.learner.calibrated_at.is_some(),
        available: st.speech.has_words(),
    }
}

#[tauri::command]
pub async fn coach_calibration_info(state: Shared<'_>) -> CmdResult<CalibrationInfo> {
    Ok(info(&state.lock()))
}

#[derive(Debug, Clone, Serialize)]
pub struct TakeWord {
    word: String,
    keyword: &'static str,
    measured: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TakeResult {
    index: usize,
    words: Vec<TakeWord>,
    info: CalibrationInfo,
}

/// Finish recording one calibration sentence and measure its vowels.
#[tauri::command]
pub async fn coach_calibration_take(state: Shared<'_>, index: usize) -> CmdResult<TakeResult> {
    let mut st = state.lock();
    let rec = st.recording.take().ok_or("Not recording.")?;
    rec.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let samples = rec.handle.join().map_err(|_| "recording thread panicked".to_string())??;
    let sentence = *PASSAGE.get(index).ok_or("No such calibration sentence.")?;
    if !st.speech.has_words() {
        return Err("Calibration needs Whisper word timings. Download Whisper small.en in Models, then choose Reload speech models.".into());
    }
    let segments = st.speech.segments(&samples);
    let analysis = analysis::analyse_with_segments(&samples, segments)
        .map_err(|_| "I didn't catch any speech. Check the microphone level and read the sentence again.".to_string())?;
    let sp = st.speech.reference(&samples, sentence);
    let tokens = tokens_for(&samples, &analysis, &sp.word_inputs());

    let words = sentence_words(sentence)
        .iter()
        .enumerate()
        .filter(|(_, w)| !is_stopword(&w.display))
        .filter_map(|(i, w)| {
            let set = word_vowels(&w.display)?.primary()?.set;
            Some(TakeWord {
                word: crate::accent_coach::speech::clean_word(&w.display),
                keyword: set.keyword(),
                measured: tokens.iter().any(|t| t.word_index == i),
            })
        })
        .collect();
    st.calibration.insert(index, CalibrationTake { tokens, median_f0_hz: analysis.median_f0_hz });
    Ok(TakeResult { index, words, info: info(&st) })
}

/// Save the calibration if it covers enough of the vowel space.
#[tauri::command]
pub async fn coach_calibration_save(state: Shared<'_>) -> CmdResult<CalibrationInfo> {
    let mut st = state.lock();
    let space = draft_space(&st);
    let cov = calibration::coverage(&space);
    if !cov.ready {
        let missing: Vec<String> = cov.missing.iter().map(|(_, k)| format!("'{k}'")).collect();
        return Err(format!(
            "Not enough vowels measured yet. Still missing the vowels in {}. Read the sentences with those vowels again, clearly and not too fast.",
            missing.join(", ")
        ));
    }
    let f0: Vec<f32> = st.calibration.values().map(|t| t.median_f0_hz).collect();
    st.learner.vowels = space;
    st.learner.calibrated_at = Some(store::now());
    if let Some(m) = analysis::stats::median(&f0) {
        st.learner.median_f0_hz = Some(m);
    }
    st.calibration.clear();
    save_learner(&st);
    st.last_attempt = None; // its vowel report used the old (or no) calibration
    Ok(info(&st))
}

#[tauri::command]
pub async fn coach_calibration_clear(state: Shared<'_>) -> CmdResult<CalibrationInfo> {
    let mut st = state.lock();
    st.calibration.clear();
    Ok(info(&st))
}

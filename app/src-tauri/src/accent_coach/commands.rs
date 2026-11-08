//! Tauri commands and events. Requests are commands; live data flows as events:
//! `live` (pitch/intensity frames while recording), `auto-stop` (speech ended),
//! `playback` (position while playing) and `model-progress` (downloads).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use analysis::{LiveAnalyzer, LiveFrame};
use audio_io::capture::Capture;
use audio_io::playback::Playback;
use coach::{CoachReply, Issue};
use crossbeam_channel::{bounded, RecvTimeoutError};
use parking_lot::Mutex;
use reference::narration::NarrationSource;
use reference::cloned::ClonedVoice;
use reference::system_voice::{self, SystemVoice};
use reference::{ReferenceSource, SourceKind};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::accent_coach::speech::{clean_word, Speech, SpeechStatus};
use crate::accent_coach::state::{ActiveText, AppState, DocInfo, LastAttempt, PageAttempt, PlaybackGen, RefClip, Recording};

type Shared<'a> = State<'a, Mutex<AppState>>;
type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Stop listening after this much silence once speech has started.
const AUTO_STOP_SILENCE_S: f32 = 1.3;
const MAX_RECORDING: Duration = Duration::from_secs(60);
/// Reading a whole page: allow breathing pauses, stop after a long silence.
const PAGE_AUTO_STOP_SILENCE_S: f32 = 6.0;
const MAX_PAGE_RECORDING: Duration = Duration::from_secs(15 * 60);

// ---------- payloads ----------

#[derive(Serialize)]
pub struct TextSummary {
    id: i64,
    title: String,
    sentence_count: usize,
    practised_at: Option<i64>,
}

#[derive(Serialize, Clone)]
pub struct RefInfo {
    kind: SourceKind,
    label: String,
}

#[derive(Serialize)]
pub struct TextDetail {
    id: i64,
    title: String,
    sentences: Vec<String>,
    reference: Option<RefInfo>,
    /// Set for a page of an imported document.
    document: Option<DocInfo>,
}

#[derive(Serialize)]
pub struct DocSummary {
    id: i64,
    title: String,
    page_count: usize,
    practised_at: Option<i64>,
}

#[derive(Serialize)]
pub struct ModelInfo {
    id: String,
    name: String,
    role: models::Role,
    size_mb: u64,
    milestone: String,
    status: models::Status,
    note: String,
}

#[derive(Serialize)]
pub struct Bootstrap {
    texts: Vec<TextSummary>,
    documents: Vec<DocSummary>,
    voices: Vec<system_voice::VoiceInfo>,
    /// The learner's chosen voice (also used to read coach feedback aloud).
    voice: String,
    neural_voices: Vec<NeuralVoice>,
    /// The neural voice model is set up.
    neural_ready: bool,
    coach: String,
    llm_supported: bool,
    learner_hz: Option<f32>,
    calibrated: bool,
    data_dir: String,
    session: i64,
    speech: SpeechStatus,
}

#[derive(Serialize)]
pub struct SentenceRef {
    index: usize,
    text: String,
    /// Speech span length, seconds.
    duration_s: f32,
    /// Where speech starts inside the playable clip, seconds (for the playhead).
    onset_s: f32,
    /// Reference pitch in semitones around its own median, one value per 10 ms, speech span only.
    contour: Vec<Option<f32>>,
    /// Intensity (dB below the clip's loudest frame), same frames — drawn as the syllable "terrain".
    intensity: Vec<f32>,
    articulation_sps: f32,
    final_rise: bool,
    words: Vec<RefWord>,
    /// True when phoneme scoring is available for this sentence.
    scoring: bool,
}

#[derive(Serialize)]
pub struct RefWord {
    index: usize,
    display: String,
    /// The reference's phones for this word (IPA), when the phoneme model is loaded.
    ipa: Option<String>,
    /// Whether the word can be played on its own.
    playable: bool,
}

#[derive(Serialize, Clone)]
pub struct WordResult {
    index: usize,
    display: String,
    score: Option<f32>,
    grade: Option<phonetics::Grade>,
    missed: bool,
    phones: Vec<phonetics::PhoneScore>,
    /// Both the reference and your version can be played.
    comparable: bool,
}

#[derive(Serialize, Clone)]
struct LivePayload {
    frames: Vec<LiveFrame>,
    onset: Option<f32>,
    rate_sps: Option<f32>,
}

#[derive(Serialize, Clone)]
struct PlaybackPayload {
    which: String,
    position_s: f32,
    duration_s: f32,
    done: bool,
}

#[derive(Serialize)]
pub struct AttemptResult {
    sentence: usize,
    comparison: analysis::Comparison,
    issues: Vec<Issue>,
    coach: CoachReply,
    /// Per-word results; empty when no speech engine is loaded.
    words: Vec<WordResult>,
    /// What the recogniser heard, for display.
    heard: Option<String>,
    /// Vowel and R comparison (M3), or why it is unavailable.
    vowels: crate::accent_coach::vowel_cmds::VowelReport,
}

#[derive(Serialize, Clone)]
struct ModelProgress {
    id: String,
    downloaded: u64,
    total: Option<u64>,
    done: bool,
    error: Option<String>,
}

// ---------- library ----------

fn doc_summaries(st: &AppState) -> CmdResult<Vec<DocSummary>> {
    Ok(st
        .store
        .documents()
        .map_err(err)?
        .into_iter()
        .map(|d| DocSummary { id: d.id, title: d.title, page_count: d.pages.len(), practised_at: d.practised_at })
        .collect())
}

fn summaries(st: &AppState) -> CmdResult<Vec<TextSummary>> {
    Ok(st
        .store
        .texts()
        .map_err(err)?
        .into_iter()
        .map(|t| TextSummary { id: t.id, title: t.title, sentence_count: t.sentences.len(), practised_at: t.practised_at })
        .collect())
}

#[tauri::command]
pub async fn coach_bootstrap(state: Shared<'_>) -> CmdResult<Bootstrap> {
    let st = state.lock();
    Ok(Bootstrap {
        texts: summaries(&st)?,
        documents: doc_summaries(&st)?,
        voices: voices(),
        voice: st.coach_voice.clone(),
        neural_voices: neural_voices(&st)?,
        neural_ready: st.neural.is_installed(),
        coach: st.coach.name().to_string(),
        llm_supported: true,
        learner_hz: st.learner.median_f0_hz,
        calibrated: st.learner.calibrated_at.is_some(),
        data_dir: st.dirs.data.display().to_string(),
        session: st.session,
        speech: st.speech.status.clone(),
    })
}

fn voices() -> Vec<system_voice::VoiceInfo> {
    let mut voices = system_voice::australian_voices();
    if voices.is_empty() {
        voices.push(system_voice::VoiceInfo::from_name(system_voice::DEFAULT_VOICE));
    }
    voices
}

/// Re-read the installed Australian voices (after downloading new ones).
#[tauri::command]
pub async fn coach_list_voices() -> CmdResult<Vec<system_voice::VoiceInfo>> {
    Ok(voices())
}

#[tauri::command]
pub async fn coach_open_voice_settings() -> CmdResult<()> {
    system_voice::open_voice_settings().map_err(err)
}

/// Speak a short sample in `voice` without changing anything.
#[tauri::command]
pub async fn coach_preview_voice(state: Shared<'_>, voice: String) -> CmdResult<()> {
    let mut st = state.lock();
    if let Some(mut child) = st.speaking.take() {
        let _ = child.kill();
    }
    st.speaking = Some(system_voice::speak(&voice, system_voice::PREVIEW_TEXT).map_err(err)?);
    Ok(())
}

// ---------- neural voices ----------

#[derive(Serialize)]
pub struct NeuralVoice {
    id: i64,
    name: String,
}

/// Shortest and longest clip worth cloning from.
const CLIP_MIN_S: f32 = 5.0;
const CLIP_MAX_S: f32 = 20.0;

fn neural_voices(st: &AppState) -> CmdResult<Vec<NeuralVoice>> {
    Ok(st.store.cloned_voices().map_err(err)?.into_iter().map(|(id, name, _)| NeuralVoice { id, name }).collect())
}

/// `(name, clip path)` of a neural voice.
fn neural_voice(st: &AppState, id: i64) -> CmdResult<(String, PathBuf)> {
    let (_, name, data) = st
        .store
        .cloned_voices()
        .map_err(err)?
        .into_iter()
        .find(|(i, _, _)| *i == id)
        .ok_or("That neural voice no longer exists.")?;
    let clip = serde_json::from_str::<serde_json::Value>(&data)
        .ok()
        .and_then(|v| v["clip"].as_str().map(PathBuf::from))
        .ok_or("That neural voice has no clip.")?;
    Ok((name, clip))
}

/// Save a clip of an Australian speaker as a neural voice. `consent` records their permission.
#[tauri::command]
pub async fn coach_add_neural_voice(state: Shared<'_>, name: String, path: String, consent: String) -> CmdResult<Vec<NeuralVoice>> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the voice a name.".into());
    }
    if consent.trim().is_empty() {
        return Err("Confirm you have the speaker's permission to copy their voice.".into());
    }
    let mut audio = audio_io::decode::decode_file(&PathBuf::from(&path)).map_err(err)?;
    let rate = audio_io::ENGINE_RATE as f32;
    if (audio.len() as f32) < CLIP_MIN_S * rate {
        return Err(format!("The clip is too short; use {CLIP_MIN_S:.0}–{CLIP_MAX_S:.0} seconds of clear speech."));
    }
    audio.truncate((CLIP_MAX_S * rate) as usize);
    let st = state.lock();
    let dir = st.dirs.data.join("voices");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let clip = dir.join(format!("voice-{}.wav", store::now()));
    audio_io::write_wav(&clip, &audio).map_err(err)?;
    let data = serde_json::json!({ "clip": clip, "source": path }).to_string();
    st.store.add_cloned_voice(name, &data, consent.trim()).map_err(err)?;
    neural_voices(&st)
}

#[tauri::command]
pub async fn coach_delete_neural_voice(state: Shared<'_>, id: i64) -> CmdResult<Vec<NeuralVoice>> {
    let mut st = state.lock();
    if let Ok((_, clip)) = neural_voice(&st, id) {
        let _ = std::fs::remove_file(clip);
    }
    st.store.delete_cloned_voice(id).map_err(err)?;
    if st.coach_voice == format!("neural:{id}") {
        st.coach_voice = system_voice::DEFAULT_VOICE.to_string();
        st.store.set_setting("voice", &st.coach_voice).map_err(err)?;
    }
    neural_voices(&st)
}

#[tauri::command]
pub async fn coach_use_neural_voice(state: Shared<'_>, id: i64) -> CmdResult<TextDetail> {
    let mut st = state.lock();
    let st = &mut *st;
    let (name, clip) = neural_voice(st, id)?;
    let active = st.text.as_mut().ok_or("Open a text first.")?;
    st.store.set_reference(active.id, "cloned", &id.to_string(), None).map_err(err)?;
    set_source(active, Box::new(ClonedVoice::new(id, clip, st.neural.clone())), format!("{name} (neural voice)"));
    st.coach_voice = format!("neural:{id}");
    st.store.set_setting("voice", &st.coach_voice).map_err(err)?;
    Ok(detail(active))
}

/// Speak a short sample in a neural voice. The first call loads the model, which takes a while.
#[tauri::command]
pub async fn coach_preview_neural_voice(state: Shared<'_>, gen: State<'_, PlaybackGen>, id: i64) -> CmdResult<()> {
    let (engine, clip) = {
        let st = state.lock();
        (st.neural.clone(), neural_voice(&st, id)?.1)
    };
    let samples = engine.synthesise(&clip, system_voice::PREVIEW_TEXT).map_err(err)?;
    let mut st = state.lock();
    gen.0.fetch_add(1, Ordering::SeqCst);
    st.playback = Some(Playback::start(&samples).map_err(err)?);
    Ok(())
}

#[tauri::command]
pub async fn coach_create_text(state: Shared<'_>, title: String, body: String) -> CmdResult<TextDetail> {
    let sentences = reference::text::split_sentences(&body);
    if sentences.is_empty() {
        return Err("Add at least one sentence to practise.".into());
    }
    let title = if title.trim().is_empty() { sentences[0].chars().take(40).collect() } else { title.trim().to_string() };
    let id = state.lock().store.add_text(&title, &body, &sentences).map_err(err)?;
    coach_open_text(state, id).await
}

#[tauri::command]
pub async fn coach_open_text(state: Shared<'_>, id: i64) -> CmdResult<TextDetail> {
    let mut st = state.lock();
    let row = st.store.text(id).map_err(err)?;
    let mut active = ActiveText {
        id,
        title: row.title.clone(),
        sentences: row.sentences.clone(),
        source: None,
        source_kind: None,
        source_label: None,
        cache: HashMap::new(),
        document: None,
    };
    if let Some(doc_id) = row.doc_id {
        let doc = st.store.document(doc_id).map_err(err)?;
        let index = doc.pages.iter().position(|p| p.1 == id).unwrap_or(0);
        active.document = Some(DocInfo {
            id: doc_id,
            title: doc.title.clone(),
            page_no: row.page_no.unwrap_or(index as i64 + 1),
            index,
            count: doc.pages.len(),
            prev: index.checked_sub(1).map(|i| doc.pages[i].1),
            next: doc.pages.get(index + 1).map(|p| p.1),
        });
        st.store.set_document_page(doc_id, id).map_err(err)?;
    }
    // Restore the saved reference source.
    match st.store.reference(id).map_err(err)? {
        Some((kind, source_id)) if kind == "narration" => {
            match audio_io::decode::decode_file(&PathBuf::from(&source_id))
                .map_err(err)
                .and_then(|audio| NarrationSource::new(source_id.clone(), audio, row.sentences.len()).map_err(err))
            {
                Ok(src) => set_source(&mut active, Box::new(src), file_label(&source_id)),
                Err(e) => tracing::warn!("could not reopen narration {source_id}: {e}"),
            }
        }
        Some((kind, voice)) if kind == "system_voice" => {
            set_source(&mut active, Box::new(SystemVoice::new(voice.clone())), format!("{voice} (macOS voice)"))
        }
        Some((kind, id)) if kind == "cloned" => match id.parse().map_err(err).and_then(|id| Ok((id, neural_voice(&st, id)?))) {
            Ok((id, (name, clip))) => {
                set_source(&mut active, Box::new(ClonedVoice::new(id, clip, st.neural.clone())), format!("{name} (neural voice)"))
            }
            Err(e) => tracing::warn!("could not reopen neural voice {id}: {e}"),
        },
        _ => {}
    }
    // Document pages read in the chosen voice without asking on every page.
    if active.source.is_none() && active.document.is_some() {
        let voice = st.coach_voice.clone();
        match voice.strip_prefix("neural:").and_then(|v| v.parse::<i64>().ok()) {
            Some(voice_id) => {
                if let Ok((name, clip)) = neural_voice(&st, voice_id) {
                    st.store.set_reference(id, "cloned", &voice_id.to_string(), None).map_err(err)?;
                    set_source(&mut active, Box::new(ClonedVoice::new(voice_id, clip, st.neural.clone())), format!("{name} (neural voice)"));
                }
            }
            None => {
                st.store.set_reference(id, "system_voice", &voice, None).map_err(err)?;
                set_source(&mut active, Box::new(SystemVoice::new(voice.clone())), format!("{voice} (macOS voice)"));
            }
        }
    }
    let detail = detail(&active);
    st.text = Some(active);
    st.page = None;
    st.current = 0;
    Ok(detail)
}

#[tauri::command]
pub async fn coach_delete_text(state: Shared<'_>, id: i64) -> CmdResult<Vec<TextSummary>> {
    let mut st = state.lock();
    st.store.delete_text(id).map_err(err)?;
    if st.text.as_ref().is_some_and(|t| t.id == id) {
        st.text = None;
    }
    summaries(&st)
}

fn file_label(path: &str) -> String {
    PathBuf::from(path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

fn set_source(active: &mut ActiveText, src: Box<dyn ReferenceSource>, label: String) {
    active.source_kind = Some(src.kind());
    active.source = Some(src);
    active.source_label = Some(label);
    active.cache.clear();
}

fn detail(active: &ActiveText) -> TextDetail {
    TextDetail {
        id: active.id,
        title: active.title.clone(),
        sentences: active.sentences.clone(),
        reference: match (active.source_kind, &active.source_label) {
            (Some(kind), Some(label)) => Some(RefInfo { kind, label: label.clone() }),
            _ => None,
        },
        document: active.document.clone(),
    }
}

// ---------- documents (PDF) ----------

/// Import a PDF: each page with text becomes a page to practise. Opens the first page.
#[tauri::command]
pub async fn coach_import_pdf(state: Shared<'_>, path: String) -> CmdResult<TextDetail> {
    let file = PathBuf::from(&path);
    let pages = crate::accent_coach::pdf::pages(&file)?;
    let title = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Document".into());
    let first = {
        let mut st = state.lock();
        let doc = st.store.add_document(&title, &path, &pages).map_err(err)?;
        st.store.document(doc).map_err(err)?.pages[0].1
    };
    coach_open_text(state, first).await
}

/// Open a document at the page last opened (or its first page).
#[tauri::command]
pub async fn coach_open_document(state: Shared<'_>, id: i64) -> CmdResult<TextDetail> {
    let page = {
        let st = state.lock();
        let doc = st.store.document(id).map_err(err)?;
        doc.last_text.filter(|t| doc.pages.iter().any(|p| p.1 == *t)).or(doc.pages.first().map(|p| p.1))
    };
    coach_open_text(state, page.ok_or("This document has no pages with text.")?).await
}

#[tauri::command]
pub async fn coach_delete_document(state: Shared<'_>, id: i64) -> CmdResult<Vec<DocSummary>> {
    let mut st = state.lock();
    st.store.delete_document(id).map_err(err)?;
    if st.text.as_ref().and_then(|t| t.document.as_ref()).is_some_and(|d| d.id == id) {
        st.text = None;
        st.page = None;
    }
    doc_summaries(&st)
}

#[tauri::command]
pub async fn coach_attach_narration(state: Shared<'_>, path: String) -> CmdResult<TextDetail> {
    let audio = audio_io::decode::decode_file(&PathBuf::from(&path)).map_err(err)?;
    let mut st = state.lock();
    let st = &mut *st;
    let active = st.text.as_mut().ok_or("Open a text first.")?;
    let src = NarrationSource::new(path.clone(), audio, active.sentences.len()).map_err(err)?;
    let covered = src.spans().len();
    let spans = serde_json::to_string(src.spans()).map_err(err)?;
    st.store.set_reference(active.id, "narration", &path, Some(&spans)).map_err(err)?;
    set_source(active, Box::new(src), file_label(&path));
    if covered < active.sentences.len() {
        tracing::warn!(covered, sentences = active.sentences.len(), "narration has fewer phrases than sentences");
    }
    Ok(detail(active))
}

#[tauri::command]
pub async fn coach_use_system_voice(state: Shared<'_>, voice: String) -> CmdResult<TextDetail> {
    let mut st = state.lock();
    let st = &mut *st;
    let active = st.text.as_mut().ok_or("Open a text first.")?;
    st.store.set_reference(active.id, "system_voice", &voice, None).map_err(err)?;
    set_source(active, Box::new(SystemVoice::new(voice.clone())), format!("{voice} (macOS voice)"));
    st.store.set_setting("voice", &voice).map_err(err)?;
    st.coach_voice = voice;
    Ok(detail(active))
}

// ---------- reference sentences ----------

pub(crate) fn reference_clip(st: &mut AppState, index: usize) -> CmdResult<Arc<RefClip>> {
    let AppState { text, speech, .. } = st;
    let active = text.as_mut().ok_or("Open a text first.")?;
    if let Some(c) = active.cache.get(&index) {
        return Ok(c.clone());
    }
    let text = active.sentences.get(index).ok_or("No such sentence.")?.clone();
    let source = active
        .source
        .as_mut()
        .ok_or("Add a reference first: attach an Australian narration or choose a macOS voice.")?;
    let rendered = source.render(index, &text).map_err(err)?;
    let segments = speech.segments(&rendered.samples);
    let analysis = analysis::analyse_with_segments(&rendered.samples, segments)
        .map_err(|e| format!("Reference sentence {}: {e}", index + 1))?;
    let started = Instant::now();
    let ref_speech = speech.reference(&rendered.samples, &text);
    tracing::debug!(ms = started.elapsed().as_millis() as u64, "reference speech analysis");
    let vowel_tokens = crate::accent_coach::vowel_cmds::tokens_for(&rendered.samples, &analysis, &ref_speech.word_inputs());
    let clip = Arc::new(RefClip { samples: Arc::new(rendered.samples), analysis, speech: ref_speech, vowels: vowel_tokens });
    active.cache.insert(index, clip.clone());
    Ok(clip)
}

#[tauri::command]
pub async fn coach_load_sentence(state: Shared<'_>, index: usize) -> CmdResult<SentenceRef> {
    let mut st = state.lock();
    let clip = reference_clip(&mut st, index)?;
    st.current = index;
    let a = &clip.analysis;
    let range = a.speech_frames();
    let peak = a.intensity_db[range.clone()].iter().copied().fold(f32::MIN, f32::max);
    let text = st.text.as_ref().map(|t| t.sentences[index].clone()).unwrap_or_default();
    let rate = analysis::compare::with_syllables(&a.rate, reference::text::count_syllables(&text));
    Ok(SentenceRef {
        index,
        text,
        duration_s: range.len() as f32 / analysis::FRAME_RATE,
        onset_s: a.time_of(range.start),
        contour: a.semitones[range.clone()].to_vec(),
        intensity: a.intensity_db[range].iter().map(|d| (d - peak).max(-40.0)).collect(),
        articulation_sps: rate.articulation_sps,
        final_rise: a.final_movement_st.is_some_and(|m| m > analysis::contour::RISE_THRESHOLD_ST),
        words: ref_words(&clip),
        scoring: clip.speech.phones.is_some(),
    })
}

fn ref_words(clip: &RefClip) -> Vec<RefWord> {
    let sp = &clip.speech;
    sp.words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let ipa = sp.phones.as_ref().and_then(|p| {
                let phones: Vec<&str> =
                    p.word_of.iter().zip(&p.phones).filter(|(wi, _)| **wi == Some(i)).map(|(_, ph)| ph.as_str()).collect();
                (!phones.is_empty()).then(|| phones.join(" "))
            });
            RefWord { index: i, display: w.display.clone(), ipa, playable: sp.span(i).is_some() }
        })
        .collect()
}

// ---------- playback ----------

#[tauri::command]
pub async fn coach_play(
    app: AppHandle,
    state: Shared<'_>,
    gen: State<'_, PlaybackGen>,
    which: String,
    sentence: Option<usize>,
) -> CmdResult<f32> {
    let mut st = state.lock();
    st.playback = None;
    let samples: Arc<Vec<f32>> = match (which.as_str(), sentence) {
        ("reference", _) => {
            let index = sentence.unwrap_or(st.current);
            reference_clip(&mut st, index)?.samples.clone()
        }
        // One sentence of a whole-page reading.
        ("attempt", Some(i)) => st
            .page
            .as_ref()
            .and_then(|p| p.attempts.get(&i))
            .map(|a| a.samples.clone())
            .ok_or("That sentence was not found in your reading.")?,
        ("attempt", None) => st.last_attempt.as_ref().map(|a| a.samples.clone()).ok_or("Record an attempt first.")?,
        ("page", _) => st.page.as_ref().map(|p| p.samples.clone()).ok_or("Read the page first.")?,
        _ => return Err(format!("unknown clip `{which}`")),
    };
    start_playback(app, &mut st, &gen, which, &samples)
}

/// Play `samples`, reporting progress as `playback` events tagged `which`.
fn start_playback(app: AppHandle, st: &mut AppState, gen: &PlaybackGen, which: String, samples: &[f32]) -> CmdResult<f32> {
    let playback = Playback::start(samples).map_err(err)?;
    let monitor = playback.monitor();
    let duration_s = playback.duration_s();
    st.playback = Some(playback);
    let my_gen = gen.0.fetch_add(1, Ordering::SeqCst) + 1;
    let gen = gen.0.clone();
    std::thread::spawn(move || loop {
        let stale = gen.load(Ordering::SeqCst) != my_gen;
        let done = monitor.is_finished() || stale;
        let _ = app.emit(
            "coach-playback",
            PlaybackPayload { which: which.clone(), position_s: monitor.position_s(), duration_s, done },
        );
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(33));
    });
    Ok(duration_s)
}

#[derive(Serialize)]
pub struct PageListen {
    duration_s: f32,
    /// Where each sentence plays, seconds.
    sentences: Vec<(f32, f32)>,
}

/// Play the reference reading of the whole page, sentence after sentence.
#[tauri::command]
pub async fn coach_play_page_reference(app: AppHandle, state: Shared<'_>, gen: State<'_, PlaybackGen>) -> CmdResult<PageListen> {
    const GAP_S: f32 = 0.45;
    let mut st = state.lock();
    st.playback = None;
    let count = st.text.as_ref().ok_or("Open a text first.")?.sentences.len();
    let rate = audio_io::ENGINE_RATE as f32;
    let mut audio = Vec::new();
    let mut sentences = Vec::with_capacity(count);
    for i in 0..count {
        let clip = reference_clip(&mut st, i)?;
        let start = audio.len() as f32 / rate;
        audio.extend_from_slice(&clip.samples);
        sentences.push((start, audio.len() as f32 / rate));
        audio.resize(audio.len() + (GAP_S * rate) as usize, 0.0);
    }
    let duration_s = start_playback(app, &mut st, &gen, "page_reference".into(), &audio)?;
    Ok(PageListen { duration_s, sentences })
}

#[tauri::command]
pub async fn coach_stop_playback(state: Shared<'_>, gen: State<'_, PlaybackGen>) -> CmdResult<()> {
    gen.0.fetch_add(1, Ordering::SeqCst);
    state.lock().playback = None;
    Ok(())
}

// ---------- recording ----------

#[tauri::command]
pub async fn coach_start_recording(
    app: AppHandle,
    state: Shared<'_>,
    gen: State<'_, PlaybackGen>,
    page: Option<bool>,
) -> CmdResult<String> {
    let page = page.unwrap_or(false);
    let (silence_s, max_len) =
        if page { (PAGE_AUTO_STOP_SILENCE_S, MAX_PAGE_RECORDING) } else { (AUTO_STOP_SILENCE_S, MAX_RECORDING) };
    let mut st = state.lock();
    if st.recording.is_some() {
        return Err("Already recording.".into());
    }
    gen.0.fetch_add(1, Ordering::SeqCst);
    st.playback = None;
    if let Some(mut child) = st.speaking.take() {
        let _ = child.kill();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let learner_hz = st.learner.median_f0_hz;
    // Live rate counts intensity peaks; scale it by how the reference's peak count relates to
    // the sentence's syllable count so the live number lands on the same scale as the result.
    // (No text or reference, e.g. during calibration: no scaling.)
    let index = st.current;
    let live_scale = match reference_clip(&mut st, index) {
        Ok(clip) => {
            let syllables =
                st.text.as_ref().map(|t| reference::text::count_syllables(&t.sentences[index])).unwrap_or(0);
            if clip.analysis.nuclei.is_empty() || syllables == 0 {
                1.0
            } else {
                (syllables as f32 / clip.analysis.nuclei.len() as f32).clamp(0.5, 2.5)
            }
        }
        Err(_) => 1.0,
    };
    let (ready_tx, ready_rx) = bounded::<Result<String, String>>(1);

    let handle = std::thread::Builder::new()
        .name("recording".into())
        .spawn(move || -> Result<Vec<f32>, String> {
            let (mut capture, rx) = match Capture::start() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return Err(e.to_string());
                }
            };
            let _ = ready_tx.send(Ok(capture.device_name.clone()));
            let mut live = LiveAnalyzer::new(learner_hz);
            let mut all: Vec<f32> = Vec::with_capacity(16_000 * 10);
            let mut pending: Vec<LiveFrame> = Vec::new();
            let mut last_emit = Instant::now();
            let started = Instant::now();
            while !stop_thread.load(Ordering::SeqCst) {
                match rx.recv_timeout(Duration::from_millis(15)) {
                    Ok(chunk) => {
                        all.extend_from_slice(&chunk);
                        pending.extend(live.push(&chunk));
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if last_emit.elapsed() >= Duration::from_millis(33) && !pending.is_empty() {
                    let payload =
                        LivePayload { frames: std::mem::take(&mut pending), onset: live.onset(), rate_sps: live.rate_sps().map(|r| r * live_scale) };
                    let _ = app.emit("coach-live", payload);
                    last_emit = Instant::now();
                }
                let now_s = all.len() as f32 / 16_000.0;
                let silent_after_speech = live.last_speech().is_some_and(|last| now_s - last > silence_s);
                if silent_after_speech || started.elapsed() > max_len {
                    let _ = app.emit("coach-auto-stop", ());
                    break;
                }
            }
            capture.stop();
            Ok(all)
        })
        .map_err(err)?;

    let device = ready_rx.recv().map_err(err)??;
    st.recording = Some(Recording { stop, handle, sentence: st.current });
    Ok(device)
}

#[tauri::command]
pub async fn coach_stop_recording(state: Shared<'_>) -> CmdResult<AttemptResult> {
    let mut st = state.lock();
    let rec = st.recording.take().ok_or("Not recording.")?;
    rec.stop.store(true, Ordering::SeqCst);
    let samples = rec.handle.join().map_err(|_| "recording thread panicked".to_string())??;
    st.last_attempt = None;
    let (result, last) = assess(&mut st, rec.sentence, Arc::new(samples), None, false)?;
    st.last_attempt = Some(last);
    Ok(result)
}

/// Compare a recording of one sentence with its reference, coach it, and save it.
///
/// `heard`: words already recognised in this clip (whole-page reading), so Whisper is not rerun.
/// `rules_only`: use the fast rule coach (a page has many sentences; the LLM would be slow).
fn assess(
    st: &mut AppState,
    sentence_idx: usize,
    samples: Arc<Vec<f32>>,
    heard: Option<Vec<asr::WordTiming>>,
    rules_only: bool,
) -> CmdResult<(AttemptResult, LastAttempt)> {
    let segments = st.speech.segments(&samples);
    let attempt = analysis::analyse_with_segments(&samples, segments).map_err(|e| match e {
        analysis::AnalysisError::NoSpeech | analysis::AnalysisError::TooShort(_) => {
            "I didn't catch any speech. Check the microphone level and read the sentence again.".to_string()
        }
        other => other.to_string(),
    })?;
    let clip = reference_clip(st, sentence_idx)?;
    let sentence = st.text.as_ref().map(|t| t.sentences[sentence_idx].clone()).unwrap_or_default();
    let syllables = reference::text::count_syllables(&sentence);
    let comparison = analysis::compare(&attempt, &clip.analysis, Some(syllables)).map_err(err)?;

    let started = Instant::now();
    let att_speech = st.speech.attempt(&samples, &clip.speech, heard);
    tracing::debug!(ms = started.elapsed().as_millis() as u64, "attempt speech analysis");
    let words = word_results(&clip, &att_speech);

    let history = st.store.coach_history().unwrap_or_default();
    let mut input = coach::issues::build_input(&sentence, &comparison, history);
    let findings: Vec<coach::issues::WordFinding> = words
        .iter()
        .filter(|w| !w.missed)
        .filter_map(|w| w.score.map(|score| coach::issues::WordFinding { word: clean_word(&w.display), score }))
        .collect();
    let missed: Vec<String> = words.iter().filter(|w| w.missed).map(|w| clean_word(&w.display)).collect();
    if !findings.is_empty() || !missed.is_empty() {
        coach::issues::add_word_issues(&mut input, &findings, &missed);
    }
    let attempt_vowels = crate::accent_coach::vowel_cmds::tokens_for(&samples, &attempt, &att_speech.word_inputs(&clip.speech.words));
    let vowel_report = crate::accent_coach::vowel_cmds::report(st, &clip, &attempt_vowels);
    let vowel_findings = crate::accent_coach::vowel_cmds::findings(&vowel_report);
    if !vowel_findings.is_empty() {
        coach::issues::add_vowel_issues(&mut input, &vowel_findings);
    }
    let reply = if rules_only { coach::Coach::reply(&mut coach::RuleCoach, &input) } else { st.coach.reply(&input) };
    let heard = (!att_speech.heard.is_empty())
        .then(|| att_speech.heard.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "));

    // Keep the learner's pitch baseline current (slow average across attempts).
    let hz = match st.learner.median_f0_hz {
        Some(old) => old * 0.8 + attempt.median_f0_hz * 0.2,
        None => attempt.median_f0_hz,
    };
    st.learner.median_f0_hz = Some(hz);
    crate::accent_coach::vowel_cmds::save_learner(st);

    // Save the attempt.
    let file = st.dirs.attempts.join(format!("s{}-{}-{}.wav", st.session, sentence_idx, store::now()));
    let audio_path = match audio_io::write_wav(&file, &samples) {
        Ok(()) => Some(file.display().to_string()),
        Err(e) => {
            tracing::warn!("could not save attempt audio: {e}");
            None
        }
    };
    let issues: Vec<(String, f32)> = input.issues.iter().map(|i| (i.feature.clone(), i.severity)).collect();
    let analysis_json = serde_json::to_string(&input).map_err(err)?;
    let coach_json = serde_json::to_string(&reply).map_err(err)?;
    let text_id = st.text.as_ref().map(|t| t.id);
    st.store
        .add_attempt(&store::NewAttempt {
            text_id,
            sentence_idx,
            sentence: &sentence,
            audio_path: audio_path.as_deref(),
            analysis_json: &analysis_json,
            coach_json: &coach_json,
            session: st.session,
            issues: &issues,
        })
        .map_err(err)?;

    let last = LastAttempt { sentence: sentence_idx, samples, speech: att_speech, vowels: vowel_report.clone() };
    let result =
        AttemptResult { sentence: sentence_idx, comparison, issues: input.issues, coach: reply, words, heard, vowels: vowel_report };
    Ok((result, last))
}

// ---------- whole page ----------

#[derive(Serialize)]
pub struct PageSentence {
    index: usize,
    /// Where this sentence was read in the whole recording, seconds; `None` when skipped.
    span: Option<(f32, f32)>,
    /// Each word's span in the whole recording, for following along during playback.
    word_spans: Vec<Option<(f32, f32)>>,
    result: Option<AttemptResult>,
    /// Words when there is no result (skipped sentence, or it could not be assessed).
    words: Vec<WordResult>,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct PageResult {
    duration_s: f32,
    sentences: Vec<PageSentence>,
    heard: String,
}

#[derive(Serialize, Clone)]
struct PageProgress {
    stage: String,
    done: usize,
    total: usize,
}

/// Words of a sentence that could not be scored; all marked missed when it was skipped.
fn unscored(sentence: &str, missed: bool) -> Vec<WordResult> {
    asr::words::sentence_words(sentence)
        .into_iter()
        .enumerate()
        .map(|(index, w)| WordResult {
            index,
            display: w.display,
            score: None,
            grade: None,
            missed,
            phones: Vec::new(),
            comparable: false,
        })
        .collect()
}

/// Stop a whole-page recording, find each sentence in it, and assess them one by one.
#[tauri::command]
pub async fn coach_stop_page_recording(app: AppHandle, state: Shared<'_>) -> CmdResult<PageResult> {
    let mut st = state.lock();
    let rec = st.recording.take().ok_or("Not recording.")?;
    rec.stop.store(true, Ordering::SeqCst);
    let samples = rec.handle.join().map_err(|_| "recording thread panicked".to_string())??;
    st.page = None;
    let sentences = st.text.as_ref().ok_or("Open a text first.")?.sentences.clone();
    let total = sentences.len();
    let progress = |stage: &str, done: usize| {
        let _ = app.emit("coach-page-progress", PageProgress { stage: stage.into(), done, total });
    };

    progress("Recognising your reading", 0);
    let heard = st.speech.transcribe(&samples)?;
    if heard.is_empty() {
        return Err("I didn't catch any speech. Check the microphone level and read the page again.".into());
    }
    let page_words: Vec<_> = sentences.iter().map(|s| asr::words::sentence_words(s)).collect();
    let ranges = asr::words::split_page(&page_words, &heard);
    let rate = audio_io::ENGINE_RATE as f32;
    let duration_s = samples.len() as f32 / rate;
    let spans: Vec<Option<(f32, f32)>> = ranges.iter().map(|r| r.map(|(a, b)| (heard[a].start_s, heard[b].end_s))).collect();

    let mut out = Vec::with_capacity(total);
    let mut attempts = HashMap::new();
    for (i, sentence) in sentences.iter().enumerate() {
        progress("Assessing", i);
        let (Some((a, b)), Some((start, end))) = (ranges[i], spans[i]) else {
            out.push(PageSentence { index: i, span: None, word_spans: vec![], result: None, words: unscored(sentence, true), error: None });
            continue;
        };
        // Cut with a little air, without running into the neighbouring sentences.
        let prev_end = spans[..i].iter().rev().flatten().next().map_or(0.0, |s| s.1);
        let next_start = spans[i + 1..].iter().flatten().next().map_or(duration_s, |s| s.0);
        let lo = (start - 0.3).max(prev_end).max(0.0);
        let hi = (end + 0.4).min(next_start).min(duration_s).max(lo);
        let clip = samples[(lo * rate) as usize..((hi * rate) as usize).min(samples.len())].to_vec();
        let local: Vec<asr::WordTiming> = heard[a..=b]
            .iter()
            .map(|w| asr::WordTiming { start_s: w.start_s - lo, end_s: w.end_s - lo, ..w.clone() })
            .collect();
        match assess(&mut st, i, Arc::new(clip), Some(local), true) {
            Ok((result, last)) => {
                let word_spans = last.speech.spans.iter().map(|s| s.map(|(x, y)| (x + lo, y + lo))).collect();
                out.push(PageSentence { index: i, span: Some((lo, hi)), word_spans, result: Some(result), words: vec![], error: None });
                attempts.insert(i, last);
            }
            Err(e) => out.push(PageSentence {
                index: i,
                span: Some((lo, hi)),
                word_spans: vec![],
                result: None,
                words: unscored(sentence, false),
                error: Some(e),
            }),
        }
    }
    progress("Done", total);
    let heard_text = heard.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    st.page = Some(PageAttempt { samples: Arc::new(samples), attempts });
    Ok(PageResult { duration_s, sentences: out, heard: heard_text })
}

fn word_results(clip: &RefClip, att: &crate::accent_coach::speech::AttemptSpeech) -> Vec<WordResult> {
    clip.speech
        .words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let scored = att.score.as_ref().and_then(|s| s.words.iter().find(|ws| ws.index == i));
            WordResult {
                index: i,
                display: w.display.clone(),
                score: scored.map(|s| s.score),
                grade: scored.map(|s| s.grade),
                missed: att.missed.contains(&i),
                phones: scored.map(|s| s.phones.clone()).unwrap_or_default(),
                comparable: clip.speech.span(i).is_some() && att.spans.get(i).copied().flatten().is_some(),
            }
        })
        .collect()
}

/// A clip of `samples` covering `span` plus a little air either side.
fn cut(samples: &[f32], span: (f32, f32)) -> Vec<f32> {
    const PAD_S: f32 = 0.08;
    let lo = (((span.0 - PAD_S).max(0.0)) * 16_000.0) as usize;
    let hi = (((span.1 + PAD_S) * 16_000.0) as usize).min(samples.len());
    let mut clip = if lo < hi { samples[lo..hi].to_vec() } else { Vec::new() };
    // 10 ms fades so cuts do not click.
    let fade = 160.min(clip.len() / 2);
    for k in 0..fade {
        let g = k as f32 / fade as f32;
        clip[k] *= g;
        let n = clip.len();
        clip[n - 1 - k] *= g;
    }
    clip
}

/// Play one word: `compare` = reference, yours, reference (A/B/A); `reference` = reference only.
#[tauri::command]
pub async fn coach_play_word(
    app: AppHandle,
    state: Shared<'_>,
    gen: State<'_, PlaybackGen>,
    index: usize,
    mode: String,
    sentence: Option<usize>,
) -> CmdResult<f32> {
    let mut st = state.lock();
    st.playback = None;
    let current = sentence.unwrap_or(st.current);
    let clip = reference_clip(&mut st, current)?;
    let ref_span = clip.speech.span(index).ok_or("That word could not be located in the reference.")?;
    let reference_part = cut(&clip.samples, ref_span);
    let gap = vec![0.0f32; 16_000 * 35 / 100];
    let audio = if mode == "compare" {
        let page_attempt = sentence.and_then(|i| st.page.as_ref().and_then(|p| p.attempts.get(&i)));
        let attempt = page_attempt
            .or(st.last_attempt.as_ref().filter(|a| a.sentence == current))
            .ok_or("Record this sentence first.")?;
        let span = attempt.speech.spans.get(index).copied().flatten().ok_or("That word was not found in your attempt.")?;
        let yours = cut(&attempt.samples, span);
        [reference_part.clone(), gap.clone(), yours, gap, reference_part].concat()
    } else {
        reference_part
    };
    let playback = Playback::start(&audio).map_err(err)?;
    let monitor = playback.monitor();
    let duration_s = playback.duration_s();
    st.playback = Some(playback);
    let my_gen = gen.0.fetch_add(1, Ordering::SeqCst) + 1;
    let gen = gen.0.clone();
    std::thread::spawn(move || loop {
        let done = monitor.is_finished() || gen.load(Ordering::SeqCst) != my_gen;
        let _ = app.emit("coach-playback", PlaybackPayload { which: "word".into(), position_s: monitor.position_s(), duration_s, done });
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    });
    Ok(duration_s)
}

/// Reload speech models after a download or running the phoneme setup script.
#[tauri::command]
pub async fn coach_reload_speech(state: Shared<'_>) -> CmdResult<SpeechStatus> {
    let mut st = state.lock();
    let st = &mut *st;
    st.dirs.helper = super::helper_path(); // it may have been built since
    st.speech = Speech::load(&st.models, &st.dirs.models, st.dirs.helper.as_deref());
    if let Some(t) = st.text.as_mut() {
        t.cache.clear(); // re-analyse references with the new engines
    }
    st.last_attempt = None;
    Ok(st.speech.status.clone())
}

#[tauri::command]
pub async fn coach_speak(state: Shared<'_>, gen: State<'_, PlaybackGen>, text: String) -> CmdResult<()> {
    // A neural coach voice ("neural:<id>") is synthesised, then played; a macOS voice speaks directly.
    let neural = {
        let mut st = state.lock();
        if let Some(mut child) = st.speaking.take() {
            let _ = child.kill();
        }
        match st.coach_voice.strip_prefix("neural:").map(|id| id.parse().map_err(err).and_then(|id| neural_voice(&st, id))) {
            Some(Ok((_, clip))) => Some((st.neural.clone(), clip)),
            Some(Err(_)) => {
                st.coach_voice = system_voice::DEFAULT_VOICE.to_string();
                None
            }
            None => None,
        }
    };
    if let Some((engine, clip)) = neural {
        let samples = engine.synthesise(&clip, &text).map_err(err)?;
        let mut st = state.lock();
        gen.0.fetch_add(1, Ordering::SeqCst);
        st.playback = Some(Playback::start(&samples).map_err(err)?);
        return Ok(());
    }
    let mut st = state.lock();
    let voice = st.coach_voice.clone();
    st.speaking = Some(system_voice::speak(&voice, &text).map_err(err)?);
    Ok(())
}

// ---------- models ----------

#[tauri::command]
pub async fn coach_models_status(state: Shared<'_>) -> CmdResult<Vec<ModelInfo>> {
    let st = state.lock();
    Ok(st
        .models
        .manifest()
        .models
        .iter()
        .map(|m| ModelInfo {
            id: m.id.clone(),
            name: m.name.clone(),
            role: m.role.clone(),
            size_mb: m.size_mb,
            milestone: m.milestone.clone(),
            status: st.models.status(m),
            note: m.note.clone(),
        })
        .collect())
}

#[tauri::command]
pub async fn coach_download_model(app: AppHandle, state: Shared<'_>, id: String) -> CmdResult<()> {
    let (dir, manifest) = {
        let st = state.lock();
        (st.dirs.models.clone(), st.models.manifest().clone())
    };
    std::thread::spawn(move || {
        let result = models::ModelStore::new(dir, manifest).and_then(|store| {
            let mut last = Instant::now();
            store.download(&id, |p| {
                if last.elapsed() > Duration::from_millis(250) {
                    let _ = app.emit(
                        "coach-model-progress",
                        ModelProgress { id: id.clone(), downloaded: p.downloaded, total: p.total, done: false, error: None },
                    );
                    last = Instant::now();
                }
                true
            })
        });
        let error = result.err().map(|e| e.to_string());
        let _ = app.emit("coach-model-progress", ModelProgress { id, downloaded: 0, total: None, done: true, error });
    });
    Ok(())
}

#[tauri::command]
pub async fn coach_load_coach(state: Shared<'_>, id: String) -> CmdResult<String> {
    // RAD links llama.cpp already (rad-local); the coach model shares its backend.
    let path = state.lock().models.ready_path(&id).ok_or("Download the model first.")?;
    let backend = rad_local::llama_backend()?;
    let coach = tauri::async_runtime::spawn_blocking(move || coach::llm::LlamaCoach::load(backend, &path))
        .await
        .map_err(err)?
        .map_err(err)?;
    let mut st = state.lock();
    st.coach = Box::new(coach);
    Ok(st.coach.name().to_string())
}

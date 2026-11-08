//! Coach (Media → Coach): Rob's Aussie Accent Coach inside RAD. Read a sentence aloud and it
//! compares your pitch, pace, pauses, ending, word sounds and Australian vowels with an
//! Australian reference reading, and gives short, grounded feedback.
//!
//! The engine is in `coach/` (its own workspace); this is the Tauri glue from the original app,
//! with every command prefixed `coach_` and every event `coach-`. Differences from the
//! standalone app:
//! - It starts the first time Coach is opened (`coach_open`), not with RAD.
//! - Whisper and Silero run in the `rad-coach-asr` helper (built by scripts/build-coach.sh): their
//!   ggml can't share RAD's process with llama.cpp.
//! - The LLM coach uses RAD's llama.cpp (rad-local's backend).
//!
//! Data stays where the standalone app kept it, `~/Library/Application Support/AussieAccentCoach/`
//! (texts, attempts, voice calibration, downloaded models), so both see the same history.

pub mod commands;
mod pdf;
mod speech;
mod state;
pub mod vowel_cmds;

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use state::{AppState, Dirs, PlaybackGen};

const MANIFEST: &str = include_str!("../../../../coach/models.toml");

const SAMPLE_TITLE: &str = "Arvo at the beach";
const SAMPLE_TEXT: &str = "G'day mate, how's it going?\n\
Not too bad, thanks. Reckon it'll be a scorcher today.\n\
We're heading down to the beach this arvo.\n\
Do you want to come for a swim?\n\
No worries, I'll grab the esky and meet you there by four.";

const HELPER: &str = if cfg!(windows) { "rad-coach-asr.exe" } else { "rad-coach-asr" };

/// `rad-coach-asr`: `RAD_COACH_ASR` if set, next to the RAD executable (bundled sidecar), or — in
/// development — in coach/target.
pub(crate) fn helper_path() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("RAD_COACH_ASR") {
        candidates.push(p.into());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        candidates.push(dir.join(HELPER));
    }
    if cfg!(debug_assertions) {
        let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../coach/target");
        candidates.push(target.join("release").join(HELPER));
        candidates.push(target.join("debug").join(HELPER));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Coach's data folder: the standalone app's, so its texts, history and models carry over.
fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().data_dir().map_err(|e| e.to_string())?.join("AussieAccentCoach"))
}

fn build(app: &AppHandle) -> Result<AppState, String> {
    build_in(data_dir(app)?)
}

fn build_in(data: PathBuf) -> Result<AppState, String> {
    let e = |e: &dyn std::fmt::Display| e.to_string();
    let dirs = Dirs { attempts: data.join("audio"), models: data.join("models"), data, helper: helper_path() };
    std::fs::create_dir_all(&dirs.attempts).map_err(|x| e(&x))?;
    let store = store::Store::open(&dirs.data.join("coach.sqlite")).map_err(|x| e(&x))?;
    if store.texts().map_err(|x| e(&x))?.is_empty() {
        let sentences = reference::text::split_sentences(SAMPLE_TEXT);
        store.add_text(SAMPLE_TITLE, SAMPLE_TEXT, &sentences).map_err(|x| e(&x))?;
    }
    let session = store.new_session().map_err(|x| e(&x))?;
    let learner: state::LearnerProfile = store
        .learner_profile()
        .map_err(|x| e(&x))?
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    let manifest = models::Manifest::parse(MANIFEST).map_err(|x| e(&x))?;
    let models = models::ModelStore::new(&dirs.models, manifest).map_err(|x| e(&x))?;
    let speech = speech::Speech::load(&models, &dirs.models, dirs.helper.as_deref());
    let coach_voice = store
        .setting("voice")
        .map_err(|x| e(&x))?
        .unwrap_or_else(|| reference::system_voice::DEFAULT_VOICE.to_string());
    let neural = reference::cloned::Engine::new(&dirs.models);
    tracing::info!(data = %dirs.data.display(), session, helper = ?dirs.helper, "Coach starting");
    Ok(AppState {
        store,
        session,
        dirs,
        models,
        text: None,
        current: 0,
        recording: None,
        last_attempt: None,
        page: None,
        speech,
        playback: None,
        coach: Box::new(coach::RuleCoach),
        coach_voice,
        neural,
        speaking: None,
        learner,
        calibration: Default::default(),
    })
}

/// Starts Coach the first time it's opened: its database, a practice session and the speech
/// engines (Whisper's model takes a moment to load). Every other `coach_*` command needs this first.
#[tauri::command]
pub async fn coach_open(app: AppHandle) -> Result<(), String> {
    static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = OPENING.lock().await;
    if app.try_state::<Mutex<AppState>>().is_some() {
        return Ok(());
    }
    let handle = app.clone();
    let st = tauri::async_runtime::spawn_blocking(move || build(&handle)).await.map_err(|e| e.to_string())??;
    app.manage(Mutex::new(st));
    app.manage(PlaybackGen(Arc::new(AtomicU64::new(0))));
    Ok(())
}

/// On quitting RAD: stop listening and playing (the helpers end when their pipes close).
pub fn stop(app: &AppHandle) {
    if let Some(state) = app.try_state::<Mutex<AppState>>() {
        let mut st = state.lock();
        if let Some(r) = st.recording.take() {
            r.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        st.playback = None;
        if let Some(mut child) = st.speaking.take() {
            let _ = child.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use speech::EngineState;

    /// Coach on the owner's real data and models, with Whisper and Silero in `rad-coach-asr`.
    /// Needs `./scripts/build-coach.sh` and the models downloaded in Coach. The database is copied,
    /// never touched. `cargo test -p rad-app --lib accent_coach -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn starts_on_real_data_and_hears_the_words() {
        let home = PathBuf::from(std::env::var("HOME").unwrap()).join("Library/Application Support/AussieAccentCoach");
        let tmp = tempfile::tempdir().unwrap();
        for f in ["coach.sqlite", "coach.sqlite-wal", "coach.sqlite-shm"] {
            if home.join(f).exists() {
                std::fs::copy(home.join(f), tmp.path().join(f)).unwrap();
            }
        }
        std::os::unix::fs::symlink(home.join("models"), tmp.path().join("models")).unwrap();

        let mut st = build_in(tmp.path().to_path_buf()).expect("Coach starts");
        let texts = st.store.texts().unwrap();
        println!("texts: {:?}", texts.iter().map(|t| &t.title).collect::<Vec<_>>());
        assert!(!texts.is_empty());
        println!("speech: {:?}", st.speech.status);
        assert_eq!(st.speech.status.words, EngineState::Ready);
        assert_eq!(st.speech.status.vad, EngineState::Ready);

        // Karen reads the sentence (the reference); then an "attempt" that drops a word.
        let sentence = "We're heading down to the beach this arvo.";
        let karen = reference::system_voice::SystemVoice::new("Karen");
        let reference_audio = karen.synthesise(sentence).unwrap();
        let rs = st.speech.reference(&reference_audio, sentence);
        println!("reference spans: {:?}", rs.spans);
        assert!(rs.spans.iter().filter(|s| s.is_some()).count() >= 6, "Whisper placed the words");

        let attempt_audio = karen.synthesise("We're heading down to the beach.").unwrap();
        let at = st.speech.attempt(&attempt_audio, &rs, None);
        let missed: Vec<_> = at.missed.iter().map(|&i| rs.words[i].display.clone()).collect();
        println!("heard: {:?}; missed: {missed:?}", at.heard.iter().map(|w| &w.text).collect::<Vec<_>>());
        assert!(missed.iter().any(|w| w.contains("arvo")), "the skipped word is noticed");

        // Silero segments feed the analysis.
        let segs = st.speech.segments(&attempt_audio).expect("Silero VAD");
        assert!(!segs.is_empty());
        let a = analysis::analyse_with_segments(&reference_audio, st.speech.segments(&reference_audio)).unwrap();
        let b = analysis::analyse_with_segments(&attempt_audio, Some(segs)).unwrap();
        let cmp = analysis::compare(&b, &a, None).unwrap();
        println!("tune match {:?}, rate ratio {:.2}", cmp.contour_corr, cmp.articulation_ratio);

        // The local LLM coach on RAD's llama.cpp backend (when the owner has downloaded one).
        if let Some(path) = st.models.ready_path("coach-qwen2.5-7b") {
            use coach::Coach;
            let backend = rad_local::llama_backend().unwrap();
            let mut llm = coach::llm::LlamaCoach::load(backend, &path).expect("coach model loads");
            let input = coach::issues::build_input(sentence, &cmp, Default::default());
            let reply = llm.reply(&input);
            println!("LLM coach ({:?}): {:?} rejected={:?}", reply.source, reply.output.spoken, reply.rejected_reason);
            // A second load shares the same backend (llama.cpp can be started only once).
            assert!(rad_local::llama_backend().is_ok());
        }
    }
}

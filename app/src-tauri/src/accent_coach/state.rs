//! Application state shared by the Tauri commands.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::thread::JoinHandle;

use analysis::UtteranceAnalysis;
use audio_io::playback::Playback;
use coach::Coach;
use models::ModelStore;
use reference::{ReferenceSource, SourceKind};
use store::Store;

use crate::accent_coach::speech::{AttemptSpeech, ReferenceSpeech, Speech};

pub struct Dirs {
    pub data: PathBuf,
    pub attempts: PathBuf,
    pub models: PathBuf,
    /// `rad-coach-asr`, which runs Whisper and Silero (None when it isn't built).
    pub helper: Option<PathBuf>,
}

/// A rendered and analysed reference sentence.
pub struct RefClip {
    pub samples: Arc<Vec<f32>>,
    pub analysis: UtteranceAnalysis,
    pub speech: ReferenceSpeech,
    /// Measured stressed vowels of the reference sentence (M3).
    pub vowels: Vec<vowels::Token>,
}

/// The most recent attempt, for word playback.
pub struct LastAttempt {
    pub sentence: usize,
    pub samples: Arc<Vec<f32>>,
    pub speech: AttemptSpeech,
    pub vowels: crate::accent_coach::vowel_cmds::VowelReport,
}

/// What the app knows about the learner's voice; stored as JSON in `voice_profiles`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LearnerProfile {
    #[serde(default)]
    pub median_f0_hz: Option<f32>,
    /// Vowels measured during calibration.
    #[serde(default)]
    pub vowels: vowels::VowelSpace,
    #[serde(default)]
    pub calibrated_at: Option<i64>,
}

/// One calibration sentence as read, before the calibration is saved.
#[derive(Debug, Clone)]
pub struct CalibrationTake {
    pub tokens: Vec<vowels::Token>,
    pub median_f0_hz: f32,
}

/// Where a page of an imported document sits in it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DocInfo {
    pub id: i64,
    pub title: String,
    /// The PDF's page number.
    pub page_no: i64,
    /// Position among the pages with text, 0-based.
    pub index: usize,
    pub count: usize,
    /// Text ids of the neighbouring pages.
    pub prev: Option<i64>,
    pub next: Option<i64>,
}

pub struct ActiveText {
    pub id: i64,
    pub title: String,
    pub sentences: Vec<String>,
    pub source: Option<Box<dyn ReferenceSource>>,
    pub source_kind: Option<SourceKind>,
    pub source_label: Option<String>,
    pub cache: HashMap<usize, Arc<RefClip>>,
    pub document: Option<DocInfo>,
}

pub struct Recording {
    pub stop: Arc<AtomicBool>,
    pub handle: JoinHandle<Result<Vec<f32>, String>>,
    pub sentence: usize,
}

/// The latest whole-page reading: the full recording and each sentence's assessed attempt.
pub struct PageAttempt {
    pub samples: Arc<Vec<f32>>,
    pub attempts: HashMap<usize, LastAttempt>,
}

pub struct AppState {
    pub store: Store,
    pub session: i64,
    pub dirs: Dirs,
    pub models: ModelStore,
    pub text: Option<ActiveText>,
    pub current: usize,
    pub recording: Option<Recording>,
    pub last_attempt: Option<LastAttempt>,
    pub page: Option<PageAttempt>,
    pub speech: Speech,
    pub playback: Option<Playback>,
    pub coach: Box<dyn Coach>,
    pub coach_voice: String,
    /// Neural voice sidecar, shared by every cloned voice.
    pub neural: reference::cloned::Engine,
    pub speaking: Option<Child>,
    pub learner: LearnerProfile,
    /// Calibration sentences read so far, by passage index.
    pub calibration: std::collections::BTreeMap<usize, CalibrationTake>,
}

/// Bumped on every playback start/stop so stale watcher threads exit.
pub struct PlaybackGen(pub Arc<AtomicU64>);

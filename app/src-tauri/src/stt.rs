//! Speech to text for talking with Gilgamesh, running on this computer.
//!
//! The microphone is read with cpal, Silero VAD finds speech, and NVIDIA Parakeet TDT 0.6B v3
//! (sherpa-onnx, int8, 25 European languages, with punctuation) turns it into text. While the
//! user talks, the growing utterance is decoded again every ~0.7 s for live captions; the turn
//! ends after a pause, which is shorter when the words so far end a sentence. Audio is never
//! written to disk. Models (~670 MB) are downloaded once into the app's `models/stt` folder and
//! verified against pinned checksums.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sherpa_onnx::{LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, VadModelConfig, VoiceActivityDetector};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;

use crate::client::{CmdError, CmdResult};

const PARAKEET: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/2bda32ec70b097a55adaa07d9a7173915b43cc78";
const SILERO: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";
const VAD_FILE: &str = "silero_vad.onnx";

struct ModelFile {
    name: &'static str,
    base: &'static str,
    sha256: &'static str,
    bytes: u64,
}

const FILES: &[ModelFile] = &[
    ModelFile { name: "encoder.int8.onnx", base: PARAKEET, sha256: "acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247", bytes: 652_184_281 },
    ModelFile { name: "decoder.int8.onnx", base: PARAKEET, sha256: "179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e", bytes: 11_845_275 },
    ModelFile { name: "joiner.int8.onnx", base: PARAKEET, sha256: "3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3", bytes: 6_355_277 },
    ModelFile { name: "tokens.txt", base: PARAKEET, sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d", bytes: 93_939 },
    ModelFile { name: VAD_FILE, base: SILERO, sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6", bytes: 643_854 },
];

const RATE: usize = 16_000;
/// Silero's window at 16 kHz.
const WINDOW: usize = 512;
/// Audio kept from just before speech is detected (the VAD needs a moment to be sure).
const PREROLL: usize = RATE / 2;
/// Live captions: decode the utterance again after this much new audio.
const PARTIAL_EVERY: usize = RATE * 7 / 10;
/// Pause that ends a turn whose words end a sentence, and one that doesn't.
const END_SENTENCE: Duration = Duration::from_millis(550);
const END_OPEN: Duration = Duration::from_millis(1100);
/// A turn longer than this is ended anyway.
const MAX_TURN: usize = RATE * 30;

struct Session {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    finish: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Stt {
    downloading: Mutex<Option<Value>>,
    recognizer: tokio::sync::Mutex<Option<Arc<OfflineRecognizer>>>,
    session: Mutex<Option<Session>>,
}

impl Stt {
    pub fn stop(&self) {
        if let Some(s) = self.session.lock().unwrap().take() {
            s.stop.store(true, Ordering::SeqCst);
        }
    }
}

fn dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().map(|d| d.join("models").join("stt")).unwrap_or_else(|_| "models/stt".into())
}

fn present(d: &Path, f: &ModelFile) -> bool {
    std::fs::metadata(d.join(f.name)).is_ok_and(|m| m.len() == f.bytes)
}

fn installed(app: &AppHandle) -> bool {
    let d = dir(app);
    FILES.iter().all(|f| present(&d, f))
}

fn path_str(p: PathBuf) -> Option<String> {
    Some(p.to_string_lossy().into_owned())
}

/// Loads Parakeet once (about half a second) and keeps it for later conversations.
async fn recognizer(app: &AppHandle, s: &Stt) -> CmdResult<Arc<OfflineRecognizer>> {
    let mut g = s.recognizer.lock().await;
    if let Some(r) = g.as_ref() {
        return Ok(r.clone());
    }
    let d = dir(app);
    let r = tokio::task::spawn_blocking(move || {
        let mut c = OfflineRecognizerConfig::default();
        c.model_config.transducer = OfflineTransducerModelConfig {
            encoder: path_str(d.join("encoder.int8.onnx")),
            decoder: path_str(d.join("decoder.int8.onnx")),
            joiner: path_str(d.join("joiner.int8.onnx")),
        };
        c.model_config.tokens = path_str(d.join("tokens.txt"));
        c.model_config.model_type = Some("nemo_transducer".into());
        c.model_config.num_threads = std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(2, 4) as i32).unwrap_or(2);
        OfflineRecognizer::create(&c)
    })
    .await
    .map_err(|e| CmdError::new("stt", e.to_string()))?
    .ok_or_else(|| CmdError::new("stt", "Couldn't load the speech recognition model — try downloading it again"))?;
    let r = Arc::new(r);
    *g = Some(r.clone());
    Ok(r)
}

fn transcribe(rec: &OfflineRecognizer, audio: &[f32]) -> String {
    let s = rec.create_stream();
    s.accept_waveform(RATE as i32, audio);
    rec.decode(&s);
    s.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default()
}

/// Words, not just noise the model wrote down as punctuation.
fn has_words(t: &str) -> bool {
    t.chars().any(char::is_alphanumeric)
}

fn ends_sentence(t: &str) -> bool {
    t.trim_end_matches(['"', '\'', ')', '”', '’']).ends_with(['.', '?', '!', '…'])
}

// ------------------------------------------------------------------ the microphone

/// Opens the default microphone; its samples arrive as mono chunks on the returned channel.
fn open_mic(tx: mpsc::Sender<Vec<f32>>) -> Result<(cpal::Stream, u32), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let dev = cpal::default_host().default_input_device().ok_or("No microphone found")?;
    let cfg = dev.default_input_config().map_err(|e| format!("Couldn't open the microphone: {e}"))?;
    let format = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.config();
    let channels = config.channels.max(1) as usize;
    let rate = config.sample_rate;
    let err = |e: cpal::Error| eprintln!("microphone: {e}");
    let stream = match format {
        cpal::SampleFormat::F32 => dev.build_input_stream(
            config,
            move |d: &[f32], _: &cpal::InputCallbackInfo| {
                let _ = tx.send(d.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect());
            },
            err,
            None,
        ),
        cpal::SampleFormat::I16 => dev.build_input_stream(
            config,
            move |d: &[i16], _: &cpal::InputCallbackInfo| {
                let _ = tx.send(d.chunks(channels).map(|f| f.iter().map(|&x| x as f32 / 32768.0).sum::<f32>() / channels as f32).collect());
            },
            err,
            None,
        ),
        other => return Err(format!("The microphone's sample format ({other}) isn't supported")),
    }
    .map_err(|e| format!("Couldn't open the microphone: {e}"))?;
    stream.play().map_err(|e| format!("Couldn't start the microphone: {e}"))?;
    Ok((stream, rate))
}

// ------------------------------------------------------------------ the listening loop

enum Turn {
    Idle,
    Speaking {
        audio: Vec<f32>,
        decoded_at: usize,
        /// Where the current pause began (in samples), and whether the words so far end a sentence.
        pause: Option<(usize, bool)>,
    },
}

fn vad_config(model: &Path) -> VadModelConfig {
    let mut vc = VadModelConfig::default();
    vc.silero_vad.model = path_str(model.to_path_buf());
    vc.silero_vad.threshold = 0.5;
    vc.silero_vad.min_silence_duration = 0.15;
    vc.silero_vad.min_speech_duration = 0.15;
    vc.silero_vad.window_size = WINDOW as i32;
    vc.silero_vad.max_speech_duration = 60.0;
    vc.sample_rate = RATE as i32;
    vc.num_threads = 1;
    vc
}

/// Turns 16 kHz audio into turn events. Time is counted in samples, so it behaves the same on
/// a recording as on the microphone.
struct Listener {
    rec: Arc<OfflineRecognizer>,
    vad: VoiceActivityDetector,
    pending: Vec<f32>,
    preroll: VecDeque<f32>,
    turn: Turn,
}

impl Listener {
    fn new(rec: Arc<OfflineRecognizer>, vad_model: &Path) -> Option<Self> {
        let vad = VoiceActivityDetector::create(&vad_config(vad_model), 60.0)?;
        Some(Listener { rec, vad, pending: Vec::new(), preroll: VecDeque::with_capacity(PREROLL + WINDOW), turn: Turn::Idle })
    }

    /// Forgets everything heard so far; true if a turn was under way.
    fn reset(&mut self) -> bool {
        self.vad.reset();
        self.pending.clear();
        self.preroll.clear();
        matches!(std::mem::replace(&mut self.turn, Turn::Idle), Turn::Speaking { .. })
    }

    /// Ends the turn now (push to talk: the key was let go).
    fn finish(&mut self, out: &mut impl FnMut(Value)) {
        if let Turn::Speaking { audio, .. } = std::mem::replace(&mut self.turn, Turn::Idle) {
            let t = transcribe(&self.rec, &audio);
            out(if has_words(&t) { json!({"type": "final", "text": t}) } else { json!({"type": "cancelled"}) });
        }
        self.reset();
    }

    fn feed(&mut self, samples: &[f32], out: &mut impl FnMut(Value)) {
        self.pending.extend_from_slice(samples);
        let mut used = 0;
        while self.pending.len() - used >= WINDOW {
            let win = &self.pending[used..used + WINDOW];
            used += WINDOW;
            self.vad.accept_waveform(win);
            // Only "is someone talking now" is used; finished segments aren't needed.
            while !self.vad.is_empty() {
                self.vad.pop();
            }
            let speech = self.vad.detected();
            let rec = &self.rec;
            let partial = |audio: &[f32], out: &mut dyn FnMut(Value)| {
                let t = transcribe(rec, audio);
                if has_words(&t) {
                    out(json!({"type": "partial", "text": t}));
                }
                t
            };
            match &mut self.turn {
                Turn::Idle => {
                    self.preroll.extend(win.iter().copied());
                    while self.preroll.len() > PREROLL {
                        self.preroll.pop_front();
                    }
                    if speech {
                        let audio: Vec<f32> = self.preroll.drain(..).collect();
                        self.turn = Turn::Speaking { audio, decoded_at: 0, pause: None };
                        out(json!({"type": "speech_start"}));
                    }
                }
                Turn::Speaking { audio, decoded_at, pause } => {
                    audio.extend_from_slice(win);
                    let mut end = audio.len() >= MAX_TURN;
                    if speech {
                        *pause = None;
                        if audio.len() - *decoded_at >= PARTIAL_EVERY && audio.len() < RATE * 15 {
                            *decoded_at = audio.len();
                            partial(audio, out);
                        }
                    } else {
                        match pause {
                            None => {
                                // A pause begins: what's been said so far decides how long to wait.
                                *decoded_at = audio.len();
                                let t = partial(audio, out);
                                *pause = Some((audio.len(), ends_sentence(&t)));
                            }
                            Some((at, sentence)) => {
                                let wait = if *sentence { END_SENTENCE } else { END_OPEN };
                                end |= audio.len() - *at >= (wait.as_millis() as usize) * RATE / 1000;
                            }
                        }
                    }
                    if end {
                        let t = transcribe(rec, audio);
                        out(if has_words(&t) { json!({"type": "final", "text": t}) } else { json!({"type": "cancelled"}) });
                        self.turn = Turn::Idle;
                        self.vad.reset();
                    }
                }
            }
        }
        self.pending.drain(..used);
    }
}

fn listen(rec: Arc<OfflineRecognizer>, vad_model: PathBuf, ch: Channel<Value>, stop: Arc<AtomicBool>, paused: Arc<AtomicBool>, finish: Arc<AtomicBool>) {
    let mut send = |v: Value| {
        let _ = ch.send(v);
    };
    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    // The stream stops when it's dropped at the end of this function.
    let (_stream, mic_rate) = match open_mic(tx) {
        Ok(s) => s,
        Err(e) => return send(json!({"type": "error", "message": e})),
    };
    let Some(mut l) = Listener::new(rec, &vad_model) else {
        return send(json!({"type": "error", "message": "Couldn't load the voice activity model"}));
    };
    let resampler = (mic_rate as usize != RATE).then(|| LinearResampler::create(mic_rate as i32, RATE as i32)).flatten();
    send(json!({"type": "ready"}));
    let mut was_paused = false;
    let mut level = 0f32;
    let mut level_at = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        let got = rx.recv_timeout(Duration::from_millis(100));
        if finish.swap(false, Ordering::SeqCst) {
            l.finish(&mut send);
        }
        let chunk = match got {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return send(json!({"type": "error", "message": "The microphone stopped"})),
        };
        // Paused (Gilgamesh is talking through the speakers): ignore what the microphone hears.
        if paused.load(Ordering::SeqCst) {
            if !was_paused {
                was_paused = true;
                if l.reset() {
                    send(json!({"type": "cancelled"}));
                }
            }
            continue;
        }
        was_paused = false;
        let chunk = match &resampler {
            Some(r) => r.resample(&chunk, false),
            None => chunk,
        };
        // The level meter, ~15 times a second.
        level = level.max((chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len().max(1) as f32).sqrt());
        if level_at.elapsed() >= Duration::from_millis(66) {
            send(json!({"type": "level", "rms": level}));
            level = 0.0;
            level_at = Instant::now();
        }
        l.feed(&chunk, &mut send);
    }
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn stt_status(app: AppHandle, s: State<'_, Stt>) -> Value {
    json!({
        "installed": installed(&app),
        "bytes": FILES.iter().map(|f| f.bytes).sum::<u64>(),
        "dir": dir(&app),
        "downloading": s.downloading.lock().unwrap().clone(),
        "listening": s.session.lock().unwrap().is_some(),
    })
}

#[tauri::command]
pub async fn stt_download(app: AppHandle, s: State<'_, Stt>) -> CmdResult<()> {
    {
        let mut d = s.downloading.lock().unwrap();
        if d.is_some() {
            return Err(CmdError::new("busy", "A download is already running"));
        }
        *d = Some(json!({"done": 0, "total": 0}));
    }
    let r = download(&app, &s).await;
    *s.downloading.lock().unwrap() = None;
    let _ = app.emit("stt-progress", json!({"done": true, "error": r.as_ref().err().map(|e| e.message.clone())}));
    r
}

async fn download(app: &AppHandle, s: &Stt) -> CmdResult<()> {
    let d = dir(app);
    tokio::fs::create_dir_all(&d).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    let todo: Vec<&ModelFile> = FILES.iter().filter(|f| !present(&d, f)).collect();
    let total: u64 = todo.iter().map(|f| f.bytes).sum();
    let mut done = 0u64;
    let http = reqwest::Client::builder().proxy(rad_tunnel::route::reqwest_proxy()).connect_timeout(Duration::from_secs(15)).build().map_err(|e| CmdError::new("stt", e.to_string()))?;
    for f in todo {
        let part = d.join(format!("{}.part", f.name));
        let resp = http.get(format!("{}/{}", f.base, f.name)).send().await.map_err(|e| CmdError::new("offline", format!("Download failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(CmdError::new("stt", format!("Download of {} failed: HTTP {}", f.name, resp.status())));
        }
        let mut file = tokio::fs::File::create(&part).await.map_err(|e| CmdError::new("io", e.to_string()))?;
        let mut hasher = Sha256::new();
        let mut stream = resp.bytes_stream();
        let mut last = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| CmdError::new("offline", format!("Download interrupted: {e}")))?;
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| CmdError::new("io", e.to_string()))?;
            done += chunk.len() as u64;
            if done - last > 2_000_000 {
                last = done;
                let p = json!({"file": f.name, "done": done, "total": total});
                *s.downloading.lock().unwrap() = Some(p.clone());
                let _ = app.emit("stt-progress", p);
            }
        }
        file.flush().await.map_err(|e| CmdError::new("io", e.to_string()))?;
        drop(file);
        let hash: String = hasher.finalize().iter().map(|x| format!("{x:02x}")).collect();
        if hash != f.sha256 {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(CmdError::new("stt", format!("{} didn't match its checksum — try again", f.name)));
        }
        tokio::fs::rename(&part, d.join(f.name)).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn stt_remove(app: AppHandle, s: State<'_, Stt>) -> CmdResult<()> {
    s.stop();
    *s.recognizer.lock().await = None;
    let d = dir(&app);
    for f in FILES {
        let _ = tokio::fs::remove_file(d.join(f.name)).await;
    }
    Ok(())
}

/// Starts listening to the microphone (or, with `paused`, opens it but ignores it for now). Events on `on_event`: `ready`, `level {rms}`,
/// `speech_start`, `partial {text}`, `final {text}` (the turn ended), `cancelled` (the turn was
/// only noise, or listening was paused) and `error {message}`.
#[tauri::command]
pub async fn stt_listen(app: AppHandle, s: State<'_, Stt>, on_event: Channel<Value>, paused: bool) -> CmdResult<()> {
    if !installed(&app) {
        return Err(CmdError::new("stt_models", "Download the speech recognition model first"));
    }
    s.stop();
    let rec = recognizer(&app, &s).await?;
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(paused));
    let finish = Arc::new(AtomicBool::new(false));
    *s.session.lock().unwrap() = Some(Session { stop: stop.clone(), paused: paused.clone(), finish: finish.clone() });
    let vad = dir(&app).join(VAD_FILE);
    std::thread::Builder::new()
        .name("rad-stt".into())
        .spawn(move || listen(rec, vad, on_event, stop, paused, finish))
        .map_err(|e| CmdError::new("stt", e.to_string()))?;
    Ok(())
}

/// Stops listening and releases the microphone.
#[tauri::command]
pub fn stt_stop(s: State<'_, Stt>) {
    s.stop();
}

/// While paused, what the microphone hears is ignored (used while Gilgamesh talks through the
/// speakers, so it doesn't hear itself).
#[tauri::command]
pub fn stt_pause(s: State<'_, Stt>, paused: bool) {
    if let Some(sess) = s.session.lock().unwrap().as_ref() {
        sess.paused.store(paused, Ordering::SeqCst);
    }
}

/// Ends the current turn now, as if the user had paused (push to talk).
#[tauri::command]
pub fn stt_finish(s: State<'_, Stt>) {
    if let Some(sess) = s.session.lock().unwrap().as_ref() {
        sess.finish.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the models: RAD_STT_MODELS=<folder with the model files and en.wav> cargo test stt
    #[test]
    fn a_recording_becomes_one_turn() {
        let Ok(d) = std::env::var("RAD_STT_MODELS") else { return };
        let d = PathBuf::from(d);
        let mut c = OfflineRecognizerConfig::default();
        c.model_config.transducer = OfflineTransducerModelConfig {
            encoder: path_str(d.join("encoder.int8.onnx")),
            decoder: path_str(d.join("decoder.int8.onnx")),
            joiner: path_str(d.join("joiner.int8.onnx")),
        };
        c.model_config.tokens = path_str(d.join("tokens.txt"));
        c.model_config.model_type = Some("nemo_transducer".into());
        let rec = Arc::new(OfflineRecognizer::create(&c).unwrap());
        let w = sherpa_onnx::Wave::read(&d.join("en.wav").to_string_lossy()).unwrap();
        let r = LinearResampler::create(w.sample_rate(), RATE as i32).unwrap();
        let mut audio = vec![0f32; RATE];
        audio.extend(r.resample(w.samples(), true));
        audio.extend(vec![0f32; RATE * 2]);
        let mut l = Listener::new(rec, &d.join(VAD_FILE)).unwrap();
        let mut ev = Vec::new();
        for c in audio.chunks(1600) {
            l.feed(c, &mut |v| ev.push(v));
        }
        let kinds: Vec<&str> = ev.iter().map(|v| v["type"].as_str().unwrap()).collect();
        eprintln!("{ev:?}");
        assert_eq!(kinds.first(), Some(&"speech_start"));
        assert_eq!(kinds.iter().filter(|k| **k == "final").count(), 1);
        let fin = ev.iter().find(|v| v["type"] == "final").unwrap()["text"].as_str().unwrap().to_lowercase();
        assert!(fin.contains("ask not what your country can do for you"), "{fin}");
    }

    #[test]
    fn sentence_endings() {
        assert!(ends_sentence("Open the terminal."));
        assert!(ends_sentence("What time is it?”"));
        assert!(!ends_sentence("I was thinking about"));
        assert!(!has_words(" . "));
    }
}

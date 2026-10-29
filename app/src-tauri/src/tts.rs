//! Text to speech with MioTTS-0.1B (English and Japanese), running on this computer.
//!
//! The engine is `mio-tts-server` (mio-tts-cpp: llama.cpp + ggml), built by
//! `scripts/build-tts.sh` and started on demand on 127.0.0.1. It runs the MioTTS language model
//! (text → speech tokens) and the MioCodec decoder (tokens + a voice → audio). Audio is returned
//! in memory and never written to disk. Models (~520 MB) are downloaded once into the app's
//! `models/miotts` folder and verified against pinned checksums.
//!
//! The decoder is MioCodec-25Hz-44.1kHz-v2: it reads the same speech tokens as the original
//! 24 kHz decoder but renders them with clearer harmonics (the 24 kHz one sounds breathy).
//! Installs that only have the 24 kHz decoder keep working until they upgrade.
//!
//! A voice can be made from a recording chosen on disk or recorded here (up to 30 seconds).
//! The in-app recording is written to the private `tmp` folder only until the voice is made.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use sha2::{Digest, Sha256};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;

use crate::client::{CmdError, CmdResult};

const BIN: &str = if cfg!(windows) { "mio-tts-server.exe" } else { "mio-tts-server" };
const MODELS: &str = "https://huggingface.co/mmnga-o/miotts-cpp-gguf/resolve/6ffcbb61be839a7f2b7acf52dde534efa9e1d125";
const WAVLM: &str = "https://huggingface.co/mmnga-o/wavlm-base-plus-gguf/resolve/a6c76be37081877942ab07069c5296b7c03ffadc";
const LLM: &str = "MioTTS-0.1B-Q8_0.gguf";
/// The original 24 kHz decoder. No longer downloaded; used only until the clearer one is installed.
const CODEC_24K: &str = "miocodec.gguf";
const CODEC: &str = "miocodec-25hz-44k-v2.gguf";
const WAVLM_FILE: &str = "wavlm_base_plus_2l_f32.gguf";

struct ModelFile {
    name: &'static str,
    base: &'static str,
    sha256: &'static str,
    bytes: u64,
    /// Only needed to make a voice from a recording.
    cloning: bool,
}

const FILES: &[ModelFile] = &[
    ModelFile { name: LLM, base: MODELS, sha256: "f11afe7b878bf2ac99bd6551b95f34dd3aa9c1af263d186ccff2d0e57055219b", bytes: 124_956_192, cloning: false },
    ModelFile { name: CODEC, base: MODELS, sha256: "444b710dc5b654486ac771ac8e3956d31ac4d6f34accbb209685d866afa11112", bytes: 392_700_224, cloning: false },
    ModelFile { name: "en_female.emb.gguf", base: MODELS, sha256: "aa8e5efd699eee3648d60eda4c954e31a3d0eebf99bcd09453178b38d78e6ccb", bytes: 768, cloning: false },
    ModelFile { name: "en_male.emb.gguf", base: MODELS, sha256: "4b8e7b748d20cb463863ddff718aa9a078651c1ab34003d555d12c8f36f6ad43", bytes: 768, cloning: false },
    ModelFile { name: "jp_female.emb.gguf", base: MODELS, sha256: "1a99490f54aedefc98010ae53101f1cd49fd73a2cfc043e2009e3e2795a1ffdb", bytes: 768, cloning: false },
    ModelFile { name: "jp_male.emb.gguf", base: MODELS, sha256: "6365e075530afba927e0158c7b834cef987e1a9757b2a5df5080f5022ca68ea6", bytes: 768, cloning: false },
    ModelFile { name: WAVLM_FILE, base: WAVLM, sha256: "900d8ddff3e3feb6a63d08d93c56c3bc7280caa2838baf92ce14c4b1b3367e18", bytes: 93_992_128, cloning: true },
];

/// The longest recording made in the app, and the most of any recording the engine uses.
const MAX_RECORDING_SECS: u32 = 30;

const PRESETS: &[(&str, &str)] = &[("en_female", "English — female"), ("en_male", "English — male"), ("jp_female", "Japanese — female"), ("jp_male", "Japanese — male")];

struct Engine {
    child: Child,
    port: u16,
    cloning: bool,
}

#[derive(Default)]
pub struct Tts {
    engine: tokio::sync::Mutex<Option<Engine>>,
    downloading: Mutex<Option<Value>>,
    http: reqwest::Client,
    /// Set to stop the recording under way.
    recording: Mutex<Option<Arc<AtomicBool>>>,
}

impl Tts {
    pub fn stop(&self) {
        if let Ok(mut g) = self.engine.try_lock() {
            if let Some(mut e) = g.take() {
                let _ = e.child.kill();
            }
        }
    }
}

fn dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().map(|d| d.join("models").join("miotts")).unwrap_or_else(|_| "models/miotts".into())
}

fn voices_dir(app: &AppHandle) -> PathBuf {
    dir(app).join("voices")
}

fn recording_path(app: &AppHandle) -> PathBuf {
    dir(app).join("tmp").join("recording.wav")
}

fn present(d: &Path, f: &ModelFile) -> bool {
    std::fs::metadata(d.join(f.name)).is_ok_and(|m| m.len() == f.bytes)
}

/// The decoder to run: the clearer one when downloaded, else an older install's 24 kHz one.
fn codec_file(app: &AppHandle) -> Option<&'static str> {
    let d = dir(app);
    if FILES.iter().any(|f| f.name == CODEC && present(&d, f)) {
        Some(CODEC)
    } else if std::fs::metadata(d.join(CODEC_24K)).is_ok_and(|m| m.len() == 387_687_136) {
        Some(CODEC_24K)
    } else {
        None
    }
}

fn installed(app: &AppHandle, cloning: bool) -> bool {
    let d = dir(app);
    if cloning {
        return FILES.iter().filter(|f| f.cloning).all(|f| present(&d, f));
    }
    FILES.iter().filter(|f| !f.cloning && f.name != CODEC).all(|f| present(&d, f)) && codec_file(app).is_some()
}

fn engine_path(app: &AppHandle) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("RAD_TTS_BIN") {
        c.push(p.into());
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        c.push(d.join(BIN));
    }
    if let Ok(d) = app.path().resource_dir() {
        c.push(d.join(BIN));
    }
    if cfg!(debug_assertions) {
        c.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tts/bin").join(BIN));
    }
    c.into_iter().find(|p| p.is_file())
}

#[derive(Serialize)]
struct Voice {
    key: String,
    name: String,
    custom: bool,
}

fn voices(app: &AppHandle) -> Vec<Voice> {
    let mut v: Vec<Voice> = PRESETS.iter().map(|(k, n)| Voice { key: k.to_string(), name: n.to_string(), custom: false }).collect();
    if let Ok(rd) = std::fs::read_dir(voices_dir(app)) {
        let mut custom: Vec<Voice> = rd
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".emb.gguf").map(String::from))
            .map(|k| Voice { name: k.replace(['_', '-'], " "), key: k, custom: true })
            .collect();
        custom.sort_by(|a, b| a.key.cmp(&b.key));
        v.extend(custom);
    }
    v
}

fn free_port() -> CmdResult<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| CmdError::new("tts", e.to_string()))?;
    Ok(l.local_addr().map_err(|e| CmdError::new("tts", e.to_string()))?.port())
}

/// The running engine's port, starting it if needed (with cloning support when asked for).
async fn ensure_engine(app: &AppHandle, t: &Tts, need_cloning: bool) -> CmdResult<u16> {
    let mut g = t.engine.lock().await;
    if let Some(e) = g.as_mut() {
        let alive = matches!(e.child.try_wait(), Ok(None));
        if alive && (e.cloning || !need_cloning) {
            return Ok(e.port);
        }
        let _ = e.child.kill();
        *g = None;
    }
    if !installed(app, false) {
        return Err(CmdError::new("tts_models", "Download the voice model first (Settings → Voice)"));
    }
    let bin = engine_path(app).ok_or_else(|| {
        CmdError::new(
            "tts_missing",
            if cfg!(debug_assertions) {
                "The speech engine isn't built yet. Run ./scripts/build-tts.sh."
            } else {
                "This copy of RAD was built without the speech engine. Rebuild it with ./scripts/build-app.sh and reinstall."
            },
        )
    })?;
    let d = dir(app);
    let vd = voices_dir(app);
    std::fs::create_dir_all(&vd).map_err(|e| CmdError::new("tts", e.to_string()))?;
    let refs: Vec<Value> = voices(app)
        .iter()
        .map(|v| json!({"key": v.key, "path": if v.custom { vd.join(format!("{}.emb.gguf", v.key)) } else { d.join(format!("{}.emb.gguf", v.key)) }}))
        .collect();
    let port = free_port()?;
    let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(2, 6)).unwrap_or(2);
    let cloning = installed(app, true);
    let codec = codec_file(app).unwrap_or(CODEC);
    let mut cmd = Command::new(&bin);
    cmd.arg("-m").arg(d.join(LLM)).arg("-mv").arg(d.join(codec));
    // Prompt (up to ~80 tokens for a 220-character piece) + up to 700 speech tokens; the
    // engine's default 700 makes long pieces fail with "requires n_ctx=…".
    cmd.args(["--host", "127.0.0.1", "--port", &port.to_string(), "--threads", &threads.to_string(), "--ctx-size", "1024"]);
    cmd.arg("--reference-file-json").arg(Value::Array(refs).to_string());
    cmd.arg("--reference-added-output-dir").arg(&vd);
    // Anything the engine would write goes to a private folder, not /tmp.
    let scratch = d.join("tmp");
    let _ = std::fs::create_dir_all(&scratch);
    cmd.arg("--output-dir").arg(&scratch);
    cmd.args(["--tts-max-reference-seconds", &MAX_RECORDING_SECS.to_string()]);
    if cloning {
        cmd.arg("--tts-wavlm-model").arg(d.join(WAVLM_FILE));
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let child = cmd.spawn().map_err(|e| CmdError::new("tts_missing", format!("Couldn't start the speech engine: {e}")))?;
    *g = Some(Engine { child, port, cloning });
    drop(g);
    // Loading takes a second or two.
    for _ in 0..120 {
        if t.http.get(format!("http://127.0.0.1:{port}/health")).timeout(Duration::from_secs(1)).send().await.is_ok_and(|r| r.status().is_success()) {
            return Ok(port);
        }
        if let Some(e) = t.engine.lock().await.as_mut() {
            if let Ok(Some(st)) = e.child.try_wait() {
                return Err(CmdError::new("tts", format!("The speech engine stopped while starting ({st})")));
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(CmdError::new("tts", "The speech engine didn't start in time"))
}

async fn restart(t: &Tts) {
    if let Some(mut e) = t.engine.lock().await.take() {
        let _ = e.child.kill();
    }
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn tts_status(app: AppHandle, t: State<'_, Tts>) -> Value {
    let total: u64 = FILES.iter().filter(|f| !f.cloning).map(|f| f.bytes).sum();
    let cloning_bytes: u64 = FILES.iter().filter(|f| f.cloning).map(|f| f.bytes).sum();
    json!({
        "installed": installed(&app, false),
        // false: an older install still on the breathy 24 kHz decoder (offer the upgrade).
        "hd": codec_file(&app) == Some(CODEC),
        "hd_bytes": FILES.iter().find(|f| f.name == CODEC).map(|f| f.bytes).unwrap_or(0),
        "cloning_installed": installed(&app, true),
        "engine": engine_path(&app).is_some(),
        // An installed app only finds engines bundled inside it; a dev build also finds tts/bin.
        "dev_build": cfg!(debug_assertions),
        "bytes": total,
        "cloning_bytes": cloning_bytes,
        "dir": dir(&app),
        "downloading": t.downloading.lock().unwrap().clone(),
        "voices": voices(&app),
    })
}

/// Downloads the model files (or, with `cloning`, the extra model for custom voices).
#[tauri::command]
pub async fn tts_download(app: AppHandle, t: State<'_, Tts>, cloning: bool) -> CmdResult<()> {
    {
        let mut d = t.downloading.lock().unwrap();
        if d.is_some() {
            return Err(CmdError::new("busy", "A download is already running"));
        }
        *d = Some(json!({"done": 0, "total": 0}));
    }
    let r = download(&app, &t, cloning).await;
    *t.downloading.lock().unwrap() = None;
    let _ = app.emit("tts-progress", json!({"done": true, "error": r.as_ref().err().map(|e| e.message.clone())}));
    if r.is_ok() {
        // Pick up a newly downloaded decoder (or cloning model) on the next request.
        restart(&t).await;
        if codec_file(&app) == Some(CODEC) {
            let _ = tokio::fs::remove_file(dir(&app).join(CODEC_24K)).await;
        }
    }
    r
}

async fn download(app: &AppHandle, t: &Tts, cloning: bool) -> CmdResult<()> {
    let d = dir(app);
    tokio::fs::create_dir_all(&d).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    let todo: Vec<&ModelFile> = FILES.iter().filter(|f| f.cloning == cloning).filter(|f| !std::fs::metadata(d.join(f.name)).is_ok_and(|m| m.len() == f.bytes)).collect();
    let total: u64 = todo.iter().map(|f| f.bytes).sum();
    let mut done = 0u64;
    let http = reqwest::Client::builder().proxy(rad_tunnel::route::reqwest_proxy()).connect_timeout(Duration::from_secs(15)).build().map_err(|e| CmdError::new("tts", e.to_string()))?;
    for f in todo {
        let part = d.join(format!("{}.part", f.name));
        let resp = http.get(format!("{}/{}", f.base, f.name)).send().await.map_err(|e| CmdError::new("offline", format!("Download failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(CmdError::new("tts", format!("Download of {} failed: HTTP {}", f.name, resp.status())));
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
                *t.downloading.lock().unwrap() = Some(p.clone());
                let _ = app.emit("tts-progress", p);
            }
        }
        file.flush().await.map_err(|e| CmdError::new("io", e.to_string()))?;
        drop(file);
        if hex(&hasher.finalize()) != f.sha256 {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(CmdError::new("tts", format!("{} didn't match its checksum — try again", f.name)));
        }
        tokio::fs::rename(&part, d.join(f.name)).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    }
    Ok(())
}

/// Removes the downloaded models (custom voices are kept).
#[tauri::command]
pub async fn tts_remove(app: AppHandle, t: State<'_, Tts>) -> CmdResult<()> {
    restart(&t).await;
    let d = dir(&app);
    for f in FILES {
        let _ = tokio::fs::remove_file(d.join(f.name)).await;
    }
    let _ = tokio::fs::remove_file(d.join(CODEC_24K)).await;
    Ok(())
}

/// One sentence or short paragraph → a WAV (44.1 kHz, or 24 kHz on older installs), in memory.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn tts_speak(
    app: AppHandle,
    t: State<'_, Tts>,
    text: String,
    voice: String,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u32>,
    repetition_penalty: Option<f32>,
) -> CmdResult<Response> {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err(CmdError::new("bad_request", "Nothing to say"));
    }
    let chars = text.chars().count() as u32;
    let port = ensure_engine(&app, &t, false).await?;
    // Speech runs at ~25 tokens a second; a sentence never needs more than this. The cap stops
    // the rare sample that doesn't end, and an overlong result is tried once more.
    let n_predict = (chars * 3 + 80).clamp(120, 700);
    let expected_secs = chars as f64 / 9.0 + 3.0;
    let mut best: Option<(f64, Vec<u8>)> = None;
    for _ in 0..2 {
        let body = json!({
            "text": text,
            "reference_key": voice,
            "temp": temperature.unwrap_or(0.8).clamp(0.1, 1.5),
            "top_p": top_p.unwrap_or(1.0).clamp(0.1, 1.0),
            "top_k": top_k.unwrap_or(50).min(200),
            "repeat_penalty": repetition_penalty.unwrap_or(1.0).clamp(1.0, 1.5),
            "seed": rand_seed(),
            "n_predict": n_predict,
        });
        let resp = t
            .http
            .post(format!("http://127.0.0.1:{port}/mio/tts/stream"))
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| CmdError::new("tts", format!("The speech engine didn't answer: {e}")))?;
        let ok = resp.status().is_success() && resp.headers().get("content-type").is_some_and(|v| v.as_bytes().starts_with(b"audio/"));
        if !ok {
            let msg = resp.json::<Value>().await.ok().and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).map(String::from));
            return Err(CmdError::new("tts", msg.unwrap_or_else(|| "Speech synthesis failed".into())));
        }
        let wav = resp.bytes().await.map_err(|e| CmdError::new("tts", e.to_string()))?.to_vec();
        // Byte rate from the WAV header: the decoder decides the sample rate.
        let byte_rate = wav.get(28..32).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).filter(|r| *r > 0).unwrap_or(48_000);
        let secs = wav.len().saturating_sub(44) as f64 / byte_rate as f64;
        let better = best.as_ref().is_none_or(|(s, _)| secs < *s);
        if better {
            best = Some((secs, wav));
        }
        if secs <= expected_secs {
            break;
        }
    }
    Ok(Response::new(best.map(|b| b.1).unwrap_or_default()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn rand_seed() -> u32 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() ^ (d.as_secs() as u32)).unwrap_or(1);
    n.wrapping_mul(2_654_435_761) >> 1
}

/// Makes a voice from a short recording (WAV, MP3 or FLAC; up to 30 seconds are used).
#[tauri::command]
pub async fn tts_add_voice(app: AppHandle, t: State<'_, Tts>, name: String, path: String) -> CmdResult<Value> {
    if !installed(&app, true) {
        return Err(CmdError::new("tts_models", "Download the voice-cloning model first"));
    }
    let key: String = name.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '_' }).collect::<String>().trim_matches('_').chars().take(40).collect();
    if key.is_empty() || PRESETS.iter().any(|(k, _)| *k == key) {
        return Err(CmdError::new("bad_request", "Choose another name for the voice"));
    }
    if !Path::new(&path).is_file() {
        return Err(CmdError::new("bad_request", "That recording can't be found"));
    }
    let port = ensure_engine(&app, &t, true).await?;
    let resp = t
        .http
        .post(format!("http://127.0.0.1:{port}/mio/generate_reference"))
        .json(&json!({"reference_key": key, "reference_audio": path}))
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| CmdError::new("tts", e.to_string()))?;
    if !resp.status().is_success() {
        let v: Value = resp.json().await.unwrap_or_default();
        return Err(CmdError::new("tts", v["error"]["message"].as_str().or(v["error"].as_str()).unwrap_or("Couldn't make a voice from that recording").to_string()));
    }
    if Path::new(&path) == recording_path(&app) {
        let _ = std::fs::remove_file(&path);
    }
    Ok(json!({"key": key}))
}

/// 16-bit mono WAV.
fn wav(rate: u32, samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    b
}

/// Records the microphone for a new voice, until `tts_record_stop` or 30 seconds. Events on
/// `on_event`: `level {rms, seconds}` (~15 a second), then `done {path, seconds}` or
/// `error {message}`.
#[tauri::command]
pub fn tts_record_start(app: AppHandle, t: State<'_, Tts>, on_event: Channel<Value>) -> CmdResult<()> {
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(old) = t.recording.lock().unwrap().replace(stop.clone()) {
        old.store(true, Ordering::SeqCst);
    }
    let path = recording_path(&app);
    std::thread::Builder::new()
        .name("rad-voice-recording".into())
        .spawn(move || {
            let send = |v: Value| {
                let _ = on_event.send(v);
            };
            let (tx, rx) = mpsc::channel::<Vec<f32>>();
            // The stream stops when it's dropped at the end of this closure.
            let (_stream, rate) = match crate::stt::open_mic(tx) {
                Ok(s) => s,
                Err(e) => return send(json!({"type": "error", "message": e})),
            };
            let max = (rate * MAX_RECORDING_SECS) as usize;
            let mut audio: Vec<f32> = Vec::with_capacity(max);
            let mut level = 0f32;
            let mut level_at = Instant::now();
            while !stop.load(Ordering::SeqCst) && audio.len() < max {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(c) => {
                        level = level.max((c.iter().map(|x| x * x).sum::<f32>() / c.len().max(1) as f32).sqrt());
                        audio.extend_from_slice(&c[..c.len().min(max - audio.len())]);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return send(json!({"type": "error", "message": "The microphone stopped"})),
                }
                if level_at.elapsed() >= Duration::from_millis(66) {
                    send(json!({"type": "level", "rms": level, "seconds": audio.len() as f64 / rate as f64}));
                    level = 0.0;
                    level_at = Instant::now();
                }
            }
            let seconds = audio.len() as f64 / rate as f64;
            let saved = path.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&path, wav(rate, &audio)));
            match saved {
                Ok(()) => send(json!({"type": "done", "path": path, "seconds": seconds})),
                Err(e) => send(json!({"type": "error", "message": format!("Couldn't keep the recording: {e}")})),
            }
        })
        .map_err(|e| CmdError::new("tts", e.to_string()))?;
    Ok(())
}

/// Ends the recording under way (it is kept, and `done` follows on its channel).
#[tauri::command]
pub fn tts_record_stop(t: State<'_, Tts>) {
    if let Some(stop) = t.recording.lock().unwrap().take() {
        stop.store(true, Ordering::SeqCst);
    }
}

/// The last recording made in the app, to listen to before making a voice from it.
#[tauri::command]
pub fn tts_recording(app: AppHandle) -> CmdResult<Response> {
    std::fs::read(recording_path(&app)).map(Response::new).map_err(|_| CmdError::new("bad_request", "There's no recording yet"))
}

#[tauri::command]
pub async fn tts_delete_voice(app: AppHandle, t: State<'_, Tts>, key: String) -> CmdResult<()> {
    if key.contains(['/', '\\', '.']) || PRESETS.iter().any(|(k, _)| *k == key) {
        return Err(CmdError::new("bad_request", "That voice can't be removed"));
    }
    let _ = tokio::fs::remove_file(voices_dir(&app).join(format!("{key}.emb.gguf"))).await;
    restart(&t).await;
    Ok(())
}

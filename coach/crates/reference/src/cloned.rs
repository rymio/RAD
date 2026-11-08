//! Neural voice (M5): Chatterbox, run as a Python sidecar, speaks any text in the voice
//! and accent of a short clip. Set up with `scripts/setup_voice_model.py`.
//!
//! The sidecar is started on first use and kept running so the model stays loaded.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::{ReferenceError, ReferenceSource, RenderedAudio, Result, SourceKind};

/// Written by the setup script into the models folder; says how to run the sidecar.
pub const READY_FILE: &str = "chatterbox.json";

#[derive(Debug, Deserialize)]
struct ReadyInfo {
    python: PathBuf,
    server: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
struct Reply {
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    ok: bool,
    error: Option<String>,
}

pub struct Sidecar {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Sidecar {
    /// Start the sidecar described by `<models dir>/chatterbox.json` and wait for the model to load.
    pub fn start(dir: &Path) -> Result<Self> {
        let info: ReadyInfo = std::fs::read_to_string(dir.join(READY_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .ok_or_else(|| fail("not set up; run coach/scripts/setup_voice_model.py (in the RAD folder)"))?;
        let mut child = Command::new(&info.python)
            .arg(&info.server)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| fail(format!("could not start {} ({e})", info.python.display())))?;
        let stdin = child.stdin.take().ok_or_else(|| fail("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| fail("no stdout"))?);
        let mut sidecar = Self { child, stdin, stdout };
        let hello = sidecar.read_reply()?;
        if !hello.ready {
            return Err(fail(hello.error.unwrap_or_else(|| "the model did not load".into())));
        }
        Ok(sidecar)
    }

    /// Speak `text` in the voice of `clip`; 16 kHz mono.
    pub fn synthesise(&mut self, clip: &Path, text: &str) -> Result<Vec<f32>> {
        let out = std::env::temp_dir().join(format!(
            "aussie-coach-neural-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        let req = serde_json::json!({ "text": text, "voice": clip, "out": out });
        writeln!(self.stdin, "{req}").and_then(|_| self.stdin.flush()).map_err(|e| fail(e.to_string()))?;
        let reply = self.read_reply()?;
        if !reply.ok {
            return Err(fail(reply.error.unwrap_or_else(|| "synthesis failed".into())));
        }
        let audio = audio_io::decode::decode_file(&out);
        let _ = std::fs::remove_file(&out);
        Ok(audio?)
    }

    fn read_reply(&mut self) -> Result<Reply> {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).map_err(|e| fail(e.to_string()))?;
        if n == 0 {
            return Err(fail("the voice sidecar stopped (check the Python environment)"));
        }
        serde_json::from_str(&line).map_err(|e| fail(format!("bad reply from sidecar ({e})")))
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One sidecar shared by every neural voice; started lazily.
#[derive(Clone)]
pub struct Engine {
    dir: PathBuf,
    sidecar: Arc<Mutex<Option<Sidecar>>>,
}

impl Engine {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), sidecar: Arc::new(Mutex::new(None)) }
    }

    pub fn is_installed(&self) -> bool {
        self.dir.join(READY_FILE).exists()
    }

    pub fn synthesise(&self, clip: &Path, text: &str) -> Result<Vec<f32>> {
        let mut guard = self.sidecar.lock().map_err(|_| fail("sidecar lock poisoned"))?;
        if guard.is_none() {
            *guard = Some(Sidecar::start(&self.dir)?);
        }
        let result = guard.as_mut().map(|s| s.synthesise(clip, text)).unwrap_or_else(|| Err(fail("not started")));
        if result.is_err() {
            // Restart on next use in case the process died.
            *guard = None;
        }
        result
    }
}

/// A neural voice cloned from `clip`.
pub struct ClonedVoice {
    id: String,
    clip: PathBuf,
    engine: Engine,
}

impl ClonedVoice {
    pub fn new(profile_id: i64, clip: PathBuf, engine: Engine) -> Self {
        Self { id: format!("cloned:{profile_id}"), clip, engine }
    }
}

impl ReferenceSource for ClonedVoice {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Cloned
    }
    fn render(&mut self, _index: usize, text: &str) -> Result<RenderedAudio> {
        Ok(RenderedAudio { samples: self.engine.synthesise(&self.clip, text)?, word_timings: None })
    }
}

fn fail(msg: impl Into<String>) -> ReferenceError {
    ReferenceError::Cloned(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_setup_is_reported() {
        let engine = Engine::new(std::env::temp_dir().join("aussie-coach-no-such-model"));
        assert!(!engine.is_installed());
        let err = engine.synthesise(Path::new("/x.wav"), "hi").unwrap_err();
        assert!(err.to_string().contains("setup_voice_model"));
    }

    #[test]
    fn replies_parse() {
        let r: Reply = serde_json::from_str(r#"{"ready": true, "device": "mps"}"#).unwrap();
        assert!(r.ready && !r.ok);
        let r: Reply = serde_json::from_str(r#"{"error": "boom"}"#).unwrap();
        assert_eq!(r.error.as_deref(), Some("boom"));
    }
}

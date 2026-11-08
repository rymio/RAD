//! Whisper word timings and Silero VAD in a helper process (`rad-coach-asr`), with the same
//! API as [`crate::whisper`].
//!
//! RAD links llama.cpp; whisper.cpp brings its own build of ggml, and two different ggml builds
//! can't share one process. So inside RAD, whisper.cpp runs next door in `rad-coach-asr`, which is
//! this crate with the `whisper` feature on.
//!
//! The protocol, over the helper's stdin/stdout:
//! - start: `rad-coach-asr whisper <model>` or `rad-coach-asr vad <model>`; it loads the model and
//!   answers one line, `{"ready":true}` or `{"error":"…"}`.
//! - request: one JSON line `{"n":<samples>,"prompt":<string|null>}`, then `n` little-endian f32s
//!   (16 kHz mono).
//! - reply: one JSON line, `{"words":[…]}` (whisper), `{"segments":[[start_s,end_s],…]}` (vad) or
//!   `{"error":"…"}`.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::words::WordTiming;
use crate::AsrError;

/// One request to the helper.
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub n: usize,
    #[serde(default)]
    pub prompt: Option<String>,
}

/// Any line the helper writes.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<WordTiming>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<(f32, f32)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct Helper {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Helper {
    fn start(program: &Path, kind: &str, model: &Path) -> Result<Self, AsrError> {
        let mut child = Command::new(program)
            .arg(kind)
            .arg(model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AsrError::Model(format!("could not start {} ({e})", program.display())))?;
        let stdin = child.stdin.take().ok_or_else(|| AsrError::Model("no stdin".into()))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| AsrError::Model("no stdout".into()))?);
        let mut helper = Self { child, stdin, stdout };
        let hello = helper.read().map_err(|e| AsrError::Model(e.to_string()))?;
        if !hello.ready {
            return Err(AsrError::Model(hello.error.unwrap_or_else(|| format!("could not load {}", model.display()))));
        }
        Ok(helper)
    }

    fn read(&mut self) -> Result<Reply, AsrError> {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).map_err(|e| AsrError::Run(e.to_string()))?;
        if n == 0 {
            return Err(AsrError::Run("the speech recognition helper stopped".into()));
        }
        serde_json::from_str(&line).map_err(|e| AsrError::Run(format!("bad reply from the helper ({e})")))
    }

    fn ask(&mut self, samples: &[f32], prompt: Option<&str>) -> Result<Reply, AsrError> {
        let req = Request { n: samples.len(), prompt: prompt.map(str::to_string) };
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let io = |e: std::io::Error| AsrError::Run(format!("the speech recognition helper: {e}"));
        let header = serde_json::to_string(&req).map_err(|e| AsrError::Run(e.to_string()))?;
        writeln!(self.stdin, "{header}").map_err(io)?;
        self.stdin.write_all(&bytes).map_err(io)?;
        self.stdin.flush().map_err(io)?;
        let reply = self.read()?;
        match reply.error {
            Some(e) => Err(AsrError::Run(e)),
            None => Ok(reply),
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whisper word timings, run by the helper.
pub struct Transcriber {
    helper: Mutex<Helper>,
}

impl Transcriber {
    /// Start the helper `program` with a Whisper model and wait for it to load.
    pub fn load(program: &Path, model: &Path) -> Result<Self, AsrError> {
        Ok(Self { helper: Mutex::new(Helper::start(program, "whisper", model)?) })
    }

    /// Words with timings for 16 kHz mono audio. See [`crate::whisper::Transcriber::words`].
    pub fn words(&self, samples: &[f32], prompt: Option<&str>) -> Result<Vec<WordTiming>, AsrError> {
        let mut helper = self.helper.lock().map_err(|_| AsrError::Run("helper lock poisoned".into()))?;
        Ok(helper.ask(samples, prompt)?.words.unwrap_or_default())
    }
}

/// Silero VAD, run by the helper.
pub struct SileroVad {
    helper: Helper,
}

impl SileroVad {
    pub fn load(program: &Path, model: &Path) -> Result<Self, AsrError> {
        Ok(Self { helper: Helper::start(program, "vad", model)? })
    }

    /// Speech segments `(start_s, end_s)` in 16 kHz mono audio.
    pub fn segments(&mut self, samples: &[f32]) -> Result<Vec<(f32, f32)>, AsrError> {
        Ok(self.helper.ask(samples, None)?.segments.unwrap_or_default())
    }
}

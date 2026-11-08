//! macOS built-in speech synthesis through the `say` command.
//!
//! "Karen" (en_AU) ships with macOS; "Lee", "Matilda" and their Enhanced and Premium
//! (neural) versions can be downloaded in System Settings › Accessibility ›
//! Spoken Content › System Voice › Manage Voices. Only Australian voices are used.
//! Synthetic speech is a fallback reference only — prefer a person's narration.

use std::path::PathBuf;
use std::process::{Child, Command};

use serde::Serialize;

use crate::{ReferenceError, ReferenceSource, RenderedAudio, Result, SourceKind};

pub const DEFAULT_VOICE: &str = "Karen";

/// Spoken when previewing a voice.
pub const PREVIEW_TEXT: &str = "G'day! I'm heading down to the beach this arvo, no worries.";

/// Opens System Settings at Accessibility, where Spoken Content › Manage Voices lives.
pub const VOICE_SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.Accessibility-Settings.extension";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    /// The small built-in voice; sounds robotic.
    Compact,
    Enhanced,
    /// Neural voice; the most natural.
    Premium,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceInfo {
    /// Exactly as `say -v` expects it, e.g. "Lee (Premium)".
    pub name: String,
    /// "male" or "female"; `None` for voices we do not recognise.
    pub gender: Option<&'static str>,
    pub quality: Quality,
}

impl VoiceInfo {
    pub fn from_name(name: &str) -> Self {
        let quality = if name.contains("(Premium)") {
            Quality::Premium
        } else if name.contains("(Enhanced)") {
            Quality::Enhanced
        } else {
            Quality::Compact
        };
        let base = name.split(" (").next().unwrap_or(name);
        let gender = match base {
            "Lee" | "Gordon" => Some("male"),
            "Karen" | "Matilda" | "Catherine" => Some("female"),
            _ => None,
        };
        Self { name: name.to_string(), gender, quality }
    }
}

#[derive(Debug, Clone)]
pub struct SystemVoice {
    voice: String,
    id: String,
}

impl SystemVoice {
    pub fn new(voice: impl Into<String>) -> Self {
        let voice = voice.into();
        Self { id: format!("system:{voice}"), voice }
    }

    pub fn voice(&self) -> &str {
        &self.voice
    }

    /// Synthesise `text` to 16 kHz mono.
    pub fn synthesise(&self, text: &str) -> Result<Vec<f32>> {
        let path = temp_path("wav");
        let status = Command::new("say")
            .args(["-v", &self.voice, "--file-format=WAVE", "--data-format=LEI16@16000", "-o"])
            .arg(&path)
            .arg(safe_text(text))
            .status()
            .map_err(|e| ReferenceError::SystemVoice(format!("could not run `say` ({e}); this needs macOS")))?;
        if !status.success() {
            return Err(ReferenceError::SystemVoice(format!("`say -v {}` failed — is the voice installed?", self.voice)));
        }
        let audio = audio_io::decode::decode_file(&path);
        let _ = std::fs::remove_file(&path);
        Ok(audio?)
    }
}

impl Default for SystemVoice {
    fn default() -> Self {
        Self::new(DEFAULT_VOICE)
    }
}

impl ReferenceSource for SystemVoice {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> SourceKind {
        SourceKind::SystemVoice
    }
    fn render(&mut self, _index: usize, text: &str) -> Result<RenderedAudio> {
        Ok(RenderedAudio { samples: self.synthesise(text)?, word_timings: None })
    }
}

/// Speak text aloud without blocking (coach feedback). Returns the child so it can be stopped.
pub fn speak(voice: &str, text: &str) -> Result<Child> {
    Command::new("say")
        .args(["-v", voice])
        .arg(safe_text(text))
        .spawn()
        .map_err(|e| ReferenceError::SystemVoice(e.to_string()))
}

/// Installed English (Australia) voices, from `say -v ?`, most natural first.
pub fn australian_voices() -> Vec<VoiceInfo> {
    let Ok(out) = Command::new("say").args(["-v", "?"]).output() else {
        return Vec::new();
    };
    let mut voices: Vec<VoiceInfo> =
        parse_voice_list(&String::from_utf8_lossy(&out.stdout)).iter().map(|n| VoiceInfo::from_name(n)).collect();
    voices.sort_by(|a, b| b.quality.cmp(&a.quality).then_with(|| a.name.cmp(&b.name)));
    voices.dedup_by(|a, b| a.name == b.name);
    voices
}

/// Open System Settings so the learner can download better Australian voices.
pub fn open_voice_settings() -> Result<()> {
    Command::new("open").arg(VOICE_SETTINGS_URL).spawn().map_err(|e| ReferenceError::SystemVoice(e.to_string()))?;
    Ok(())
}

/// Lines look like `Karen               en_AU    # Hello! My name is Karen.`
pub fn parse_voice_list(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| {
            let before_comment = line.split('#').next()?;
            let mut parts: Vec<&str> = before_comment.split_whitespace().collect();
            let locale = parts.pop()?;
            (locale == "en_AU" && !parts.is_empty()).then(|| parts.join(" "))
        })
        .collect()
}

/// `say` treats a leading '-' as an option; strip it from the text.
fn safe_text(text: &str) -> &str {
    text.trim_start_matches(|c: char| c == '-' || c.is_whitespace())
}

fn temp_path(ext: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("aussie-coach-{}-{nanos}.{ext}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_australian_voices() {
        let listing = "Karen               en_AU    # Hello! My name is Karen.\n\
                       Daniel              en_GB    # Hello! My name is Daniel.\n\
                       Lee (Enhanced)      en_AU    # Hello! My name is Lee.\n";
        assert_eq!(parse_voice_list(listing), vec!["Karen".to_string(), "Lee (Enhanced)".to_string()]);
    }

    #[test]
    fn describes_voices() {
        let lee = VoiceInfo::from_name("Lee (Premium)");
        assert_eq!((lee.gender, lee.quality), (Some("male"), Quality::Premium));
        let karen = VoiceInfo::from_name("Karen");
        assert_eq!((karen.gender, karen.quality), (Some("female"), Quality::Compact));
        assert!(Quality::Premium > Quality::Enhanced && Quality::Enhanced > Quality::Compact);
    }
}

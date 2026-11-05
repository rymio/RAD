//! Talking with RAD over stdin/stdout, one JSON object per line. What the world says starts with
//! `@rad ` (the engine writes its own log lines to stdout too). Without RAD (in the editor, or
//! run on its own) nothing comes in and the world just lives.

use std::sync::mpsc::{channel, Receiver};

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    /// Who the owner is, and Gilgamesh's face (a PNG or JPEG file).
    Hello {
        #[serde(default)]
        player: Option<String>,
        #[serde(default)]
        face: Option<String>,
        #[serde(default)]
        hour: Option<f32>,
    },
    Face { path: String },
    /// The local time of day, 0–24.
    Clock { hour: f32 },
    /// Gilgamesh's state in RAD: idle, listening, thinking, speaking (and his mood).
    Phase {
        phase: String,
        #[serde(default)]
        mood: Option<String>,
    },
    /// A sentence Gilgamesh says (shown in his bubble and the chat).
    Say { text: String },
    /// Something the owner said to him by voice (for the chat log).
    Heard { text: String },
    /// Gilgamesh builds something: what, where (a place or thing's name, "here", "you"), and a name.
    Build {
        what: String,
        #[serde(default)]
        near: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        by: Option<String>,
    },
    /// Gilgamesh walks somewhere.
    Goto { place: String },
    /// Takes a built thing away (by name).
    Remove { name: String },
    /// What's on the clay tablet (the owner's tasks), shown when they stand by it.
    Tasks { items: Vec<String> },
    /// Saves a picture of the scene to a PNG (tests) after a few frames.
    Shot { path: String },
    /// Tests: a key press (by its name, e.g. "Enter", "KeyB"), typing into the chat box (as if
    /// Enter was pressed in it), and a report of what the UI shows.
    Key { code: String },
    Type { text: String },
    Ui,
    Save,
    Quit,
}

pub fn listen() -> Receiver<Cmd> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let t = line.trim();
                    if t.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Cmd>(t) {
                        Ok(c) => {
                            if tx.send(c).is_err() {
                                break;
                            }
                        }
                        Err(e) => say(&serde_json::json!({ "type": "error", "message": format!("bad command: {e}") })),
                    }
                }
            }
        }
    });
    rx
}

pub fn say(v: &Value) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "@rad {v}");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let c: Cmd = serde_json::from_str(r#"{"cmd":"build","what":"palm","near":"house"}"#).unwrap();
        assert!(matches!(c, Cmd::Build { ref what, .. } if what == "palm"));
        let c: Cmd = serde_json::from_str(r#"{"cmd":"hello","player":"Rob"}"#).unwrap();
        assert!(matches!(c, Cmd::Hello { player: Some(ref p), .. } if p == "Rob"));
        assert!(serde_json::from_str::<Cmd>(r#"{"cmd":"nope"}"#).is_err());
    }
}

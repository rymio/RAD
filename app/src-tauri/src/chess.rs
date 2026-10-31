//! Chess (Games → Chess): the engine Gilgamesh plays with, `rad-chess` (see `chess/rad-chess`,
//! built by `scripts/build-chess.sh`). Each move is one run of it: the game so far in, his move
//! and what he saw out (JSON). One search at a time; a new one, or `chess_stop`, ends the last.
//! Games themselves are kept by the UI in the encrypted Games database.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use crate::client::{CmdError, CmdResult};

const BIN: &str = if cfg!(windows) { "rad-chess.exe" } else { "rad-chess" };

#[derive(Default)]
pub struct Chess {
    /// Ends the search running now.
    cancel: Mutex<Option<oneshot::Sender<()>>>,
}

/// Next to the app (bundled sidecar), in its resources, or — in development — in the engine's
/// own build folder. `RAD_CHESS_BIN` overrides these.
fn engine_path(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("RAD_CHESS_BIN") {
        candidates.push(p.into());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        candidates.push(dir.join(BIN));
    }
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join(BIN));
    }
    if cfg!(debug_assertions) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chess/rad-chess/target");
        candidates.push(root.join("release").join(BIN));
        candidates.push(root.join("debug").join(BIN));
    }
    candidates.into_iter().find(|p| p.is_file())
}

#[tauri::command]
pub fn chess_status(app: AppHandle) -> Value {
    json!({ "installed": engine_path(&app).is_some() })
}

/// Gilgamesh's move for a position: `{fen?, moves: [uci…], depth, margin, explain}` in, the
/// engine's answer out. A search that's replaced or stopped fails with code "stopped".
#[tauri::command]
pub async fn chess_think(app: AppHandle, state: State<'_, Chess>, request: Value) -> CmdResult<Value> {
    let bin = engine_path(&app).ok_or_else(|| CmdError::new("missing", "The chess engine isn't built. Run ./scripts/build-chess.sh"))?;
    let (tx, rx) = oneshot::channel();
    if let Some(old) = state.cancel.lock().unwrap().replace(tx) {
        let _ = old.send(());
    }
    let mut cmd = tokio::process::Command::new(bin);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| CmdError::new("chess", format!("Couldn't start the chess engine: {e}")))?;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(request.to_string().as_bytes()).await?;
    drop(stdin);
    // Dropping the child (when stopped) kills it.
    let out = tokio::select! {
        out = child.wait_with_output() => out?,
        _ = rx => return Err(CmdError::new("stopped", "Stopped")),
    };
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| CmdError::new("chess", "The chess engine gave no answer"))?;
    match v.get("error").and_then(Value::as_str) {
        Some(e) => Err(CmdError::new("chess", e)),
        None => Ok(v),
    }
}

#[tauri::command]
pub fn chess_stop(state: State<'_, Chess>) {
    if let Some(c) = state.cancel.lock().unwrap().take() {
        let _ = c.send(());
    }
}

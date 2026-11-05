//! Gilgamesh's world: the Fyrox game in `fyrox/world` (`rad-world`), run as its own window. RAD
//! talks with it over stdin/stdout, one JSON object per line (the world's lines start with
//! `@rad `): RAD passes on what Gilgamesh says and decides ("build a palm by the well"), the
//! world tells RAD what the owner says to him there, what's been built, and how things stand.
//!
//! The world is a Fyrox scene kept in the app's data folder (`fyrox/world/world.rgs`). The
//! world and the Fyrox editor both run in that folder (Fyrox only reads scenes inside its working
//! directory), so the editor opens the same file; the world is closed while the editor has it.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::client::{CmdError, CmdResult};
use crate::fyrox::{data_dir, Fyrox};

const EXE: &str = if cfg!(windows) { "rad-world.exe" } else { "rad-world" };
const EDITOR: &str = if cfg!(windows) { "rad-world-editor.exe" } else { "rad-world-editor" };
const WORLD_FILE: &str = "world.rgs";

#[derive(Default)]
pub struct World {
    game: Mutex<Option<(Child, ChildStdin)>>,
    editor: Mutex<Option<Child>>,
    /// The world's latest `state` report.
    state: Arc<Mutex<Value>>,
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

fn world_dir(app: &AppHandle) -> PathBuf {
    data_dir(app).join("world")
}

/// The world's two programs: bundled next to RAD, or — in development — where
/// `scripts/build-fyrox.sh` builds them. `RAD_WORLD_BIN_DIR` overrides both.
fn bin(app: &AppHandle, editor: bool) -> Option<PathBuf> {
    let (bundled, dev) = if editor { (EDITOR, "editor") } else { (EXE, "executor") };
    let dev = if cfg!(windows) { format!("{dev}.exe") } else { dev.to_string() };
    let mut c: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("RAD_WORLD_BIN_DIR") {
        c.push(Path::new(&d).join(&dev));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        c.push(dir.join(bundled));
    }
    if let Ok(dir) = app.path().resource_dir() {
        c.push(dir.join(bundled));
    }
    if cfg!(debug_assertions) {
        c.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fyrox/world/target/release").join(&dev));
    }
    c.into_iter().find(|p| p.is_file())
}

fn missing() -> CmdError {
    CmdError::new(
        "world_missing",
        if cfg!(debug_assertions) { "Gilgamesh's world isn't built yet. Run ./scripts/build-fyrox.sh (a few minutes the first time), then try again." } else { "This copy of RAD was built without Gilgamesh's world. Rebuild it with ./scripts/build-app.sh." },
    )
}

fn alive(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

impl World {
    fn running(&self) -> bool {
        let mut g = self.game.lock().unwrap();
        let ok = g.as_mut().is_some_and(|(c, _)| alive(c));
        if !ok {
            *g = None;
        }
        ok
    }

    fn editing(&self) -> bool {
        let mut g = self.editor.lock().unwrap();
        let ok = g.as_mut().is_some_and(alive);
        if !ok {
            *g = None;
        }
        ok
    }

    fn send(&self, v: &Value) -> CmdResult<()> {
        let mut g = self.game.lock().unwrap();
        let (_, stdin) = g.as_mut().ok_or_else(|| CmdError::new("world_stopped", "Gilgamesh's world isn't open"))?;
        let mut line = serde_json::to_vec(v).map_err(io)?;
        line.push(b'\n');
        stdin.write_all(&line).and_then(|_| stdin.flush()).map_err(|e| CmdError::new("world_stopped", format!("The world closed: {e}")))
    }

    pub fn stop(&self) {
        if let Some((mut c, mut stdin)) = self.game.lock().unwrap().take() {
            let _ = stdin.write_all(b"{\"cmd\":\"quit\"}\n");
            let _ = stdin.flush();
            // It saves, then closes; if it doesn't, it's stopped.
            for _ in 0..30 {
                if !alive(&mut c) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Makes a new world file (no window) if there isn't one yet.
fn ensure_world(app: &AppHandle) -> CmdResult<()> {
    let dir = world_dir(app);
    if dir.join(WORLD_FILE).exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(io)?;
    let exe = bin(app, false).ok_or_else(missing)?;
    let out = Command::new(exe).args(["--generate", "--world", WORLD_FILE]).current_dir(&dir).stdin(Stdio::null()).output().map_err(io)?;
    if dir.join(WORLD_FILE).exists() {
        Ok(())
    } else {
        Err(CmdError::new("world", format!("Couldn't make the world: {}", String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains("rror") || l.contains("panic")).collect::<Vec<_>>().join("; "))))
    }
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn world_status(app: AppHandle, st: State<'_, Fyrox>) -> Value {
    let w = &st.world;
    json!({
        "built": bin(&app, false).is_some(),
        "editor_built": bin(&app, true).is_some(),
        "running": w.running(),
        "editing": w.editing(),
        "exists": world_dir(&app).join(WORLD_FILE).exists(),
        "dir": world_dir(&app).to_string_lossy(),
        "state": w.state.lock().unwrap().clone(),
    })
}

/// Opens the world (a window of its own). `hello` is sent first (the owner's name, the time).
#[tauri::command]
pub async fn world_start(app: AppHandle, hello: Value) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<Fyrox>();
        let w = &st.world;
        if w.running() {
            return w.send(&hello);
        }
        if w.editing() {
            return Err(CmdError::new("world_busy", "The Fyrox editor has the world open. Save and close it first."));
        }
        ensure_world(&app)?;
        let exe = bin(&app, false).ok_or_else(missing)?;
        let mut child = Command::new(exe)
            .args(["--world", WORLD_FILE])
            .current_dir(world_dir(&app))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| CmdError::new("world_missing", format!("Couldn't open the world: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let mut line = serde_json::to_vec(&hello).map_err(io)?;
        line.push(b'\n');
        let _ = stdin.write_all(&line).and_then(|_| stdin.flush());
        let state = w.state.clone();
        let app2 = app.clone();
        std::thread::spawn(move || {
            for l in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some(j) = l.strip_prefix("@rad ") else { continue };
                let Ok(v) = serde_json::from_str::<Value>(j) else { continue };
                if v["type"] == "state" {
                    *state.lock().unwrap() = v.clone();
                }
                let _ = app2.emit_to("main", "world-event", v);
            }
            let _ = app2.emit_to("main", "world-event", json!({ "type": "exited" }));
        });
        *w.game.lock().unwrap() = Some((child, stdin));
        Ok(())
    })
    .await
    .map_err(io)?
}

#[tauri::command]
pub fn world_send(st: State<'_, Fyrox>, cmd: Value) -> CmdResult<()> {
    st.world.send(&cmd)
}

#[tauri::command]
pub async fn world_stop(app: AppHandle) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || app.state::<Fyrox>().world.stop()).await.map_err(io)
}

/// Opens the world in the Fyrox editor (the world closes first, saving, so both don't change it).
#[tauri::command]
pub async fn world_edit(app: AppHandle) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<Fyrox>();
        if st.world.editing() {
            return Ok(());
        }
        st.world.stop();
        ensure_world(&app)?;
        let exe = bin(&app, true).ok_or_else(missing)?;
        let child = Command::new(exe).args(["--scene", WORLD_FILE]).current_dir(world_dir(&app)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| CmdError::new("world_missing", format!("Couldn't open the editor: {e}")))?;
        *st.world.editor.lock().unwrap() = Some(child);
        let _ = app.emit_to("main", "world-event", json!({ "type": "editing" }));
        Ok(())
    })
    .await
    .map_err(io)?
}

/// Gilgamesh's face for the world (an image data URL), saved as `face.png` next to the world.
#[tauri::command]
pub fn world_face(app: AppHandle, data_url: String) -> CmdResult<String> {
    let b64 = data_url.split_once(',').map(|(_, d)| d).unwrap_or(&data_url);
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| CmdError::new("bad_request", e.to_string()))?;
    let dir = world_dir(&app);
    std::fs::create_dir_all(&dir).map_err(io)?;
    let path = dir.join("face.png");
    std::fs::write(&path, bytes).map_err(io)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Starts the world over (the old one is kept as `world-<time>.rgs`).
#[tauri::command]
pub async fn world_reset(app: AppHandle) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<Fyrox>();
        st.world.stop();
        let dir = world_dir(&app);
        let f = dir.join(WORLD_FILE);
        if f.exists() {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            std::fs::rename(&f, dir.join(format!("world-{stamp}.rgs"))).map_err(io)?;
        }
        ensure_world(&app)
    })
    .await
    .map_err(io)?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the world built (`./scripts/build-fyrox.sh`):
    /// `cargo test -p rad-app --lib fyrox_world -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn makes_a_world_with_the_real_binary() {
        let d = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("RAD_FYROX_DIR", d.path());
            std::env::set_var("RAD_WORLD_BIN_DIR", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fyrox/world/target/release"));
        }
        let app = tauri::test::mock_app();
        let h = app.handle();
        // ensure_world takes an AppHandle<Wry>; the mock runtime's handle has the same paths.
        let dir = data_dir(h).join("world");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fyrox/world/target/release/executor");
        let out = Command::new(exe).args(["--generate", "--world", WORLD_FILE]).current_dir(&dir).stdin(Stdio::null()).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("@rad {") && text.contains("\"generated\""), "{text}");
        let scene = std::fs::read_to_string(dir.join(WORLD_FILE)).unwrap();
        assert!(scene.contains("Gilgamesh's house") && scene.contains("character:you"));
        println!("world: {} KB", scene.len() / 1024);
    }
}

//! SideX, the code editor: VS Code's workbench on Tauri (https://github.com/Sidenai/sidex),
//! built by scripts/build-sidex.sh. It is its own app (its own Rust backend and windows), so
//! RAD launches it rather than embedding it: `SideX --folder PATH`. RAD's patch makes SideX a
//! single instance, so opening another folder while it runs opens a window in the running one.
//! RAD keeps the folders opened from here as a recents list.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

use crate::client::{CmdError, CmdResult};

const MAX_RECENT: usize = 12;

#[derive(Default)]
pub struct Sidex {
    child: Mutex<Option<std::process::Child>>,
}

/// SideX.app: inside RAD's bundle (`Contents/Helpers`) or, in development, its build folder.
fn app_dir(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        candidates.push(dir.join("../Helpers/SideX.app"));
    }
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join("SideX.app"));
    }
    if cfg!(debug_assertions) {
        candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sidex/target/release/bundle/macos/SideX.app"));
    }
    candidates.into_iter().find(|d| d.join("Contents/MacOS/SideX").is_file())
}

fn recents_file(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("sidex-recent.json")
}

fn recents(app: &AppHandle) -> Vec<String> {
    std::fs::read(recents_file(app)).ok().and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok()).unwrap_or_default()
}

fn remember(app: &AppHandle, folder: &str) {
    let mut r = recents(app);
    r.retain(|f| f != folder);
    r.insert(0, folder.to_string());
    r.truncate(MAX_RECENT);
    let _ = std::fs::write(recents_file(app), serde_json::to_vec(&r).unwrap_or_default());
}

fn running(s: &Sidex) -> bool {
    let mut c = s.child.lock().unwrap();
    match c.as_mut() {
        Some(ch) => matches!(ch.try_wait(), Ok(None)),
        None => false,
    }
}

#[tauri::command]
pub fn sidex_status(app: AppHandle, s: State<'_, Sidex>) -> Value {
    let list: Vec<Value> = recents(&app)
        .into_iter()
        .map(|f| {
            let name = Path::new(&f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| f.clone());
            json!({"path": f, "name": name, "exists": Path::new(&f).is_dir()})
        })
        .collect();
    json!({
        "supported": cfg!(target_os = "macos"),
        "available": app_dir(&app).is_some(),
        "dev_build": cfg!(debug_assertions),
        "running": running(&s),
        "recent": list,
    })
}

/// Opens SideX, on `folder` if given. SideX keeps running when RAD quits (it may have unsaved work).
#[tauri::command]
pub fn sidex_open(app: AppHandle, s: State<'_, Sidex>, folder: Option<String>) -> CmdResult<()> {
    let dir = app_dir(&app).ok_or_else(|| {
        CmdError::new(
            "sidex_missing",
            if cfg!(debug_assertions) { "SideX isn't built yet. Run ./scripts/build-sidex.sh." } else { "This copy of RAD was built without SideX. Rebuild it with ./scripts/build-app.sh." },
        )
    })?;
    let folder = match folder.map(|f| f.trim().to_string()).filter(|f| !f.is_empty()) {
        Some(f) => {
            let p = PathBuf::from(&f);
            if !p.is_dir() {
                return Err(CmdError::new("not_found", "That folder doesn't exist any more"));
            }
            Some(p.canonicalize().unwrap_or(p).display().to_string())
        }
        None => None,
    };
    let mut cmd = Command::new(dir.join("Contents/MacOS/SideX"));
    if let Some(f) = &folder {
        cmd.args(["--folder", f]);
    }
    if let Some(home) = std::env::var_os("HOME") {
        cmd.current_dir(home);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let child = cmd.spawn().map_err(|e| CmdError::new("sidex", format!("Couldn't start SideX: {e}")))?;
    // With SideX already running, this launch hands the folder over and exits at once; keep
    // the handle of the one that stays.
    {
        let mut c = s.child.lock().unwrap();
        let keep_old = c.as_mut().is_some_and(|ch| matches!(ch.try_wait(), Ok(None)));
        if !keep_old {
            *c = Some(child);
        } else {
            std::thread::spawn(move || {
                let mut child = child;
                let _ = child.wait();
            });
        }
    }
    if let Some(f) = folder {
        remember(&app, &f);
    }
    Ok(())
}

#[tauri::command]
pub fn sidex_forget(app: AppHandle, folder: Option<String>) {
    let mut r = recents(&app);
    match folder {
        Some(f) => r.retain(|x| *x != f),
        None => r.clear(),
    }
    let _ = std::fs::write(recents_file(&app), serde_json::to_vec(&r).unwrap_or_default());
}

//! The DOS view: a DOS PC (DOSBox Pure, see dos/rad-dos and scripts/build-dos.sh) running in its
//! own process, like the media engine (media.rs) — DOSBox Pure is GPL, so RAD never links it.
//!
//! - The PC's hard disk, drive C:, is a folder in RAD's data (`dos/c`): programs and games are
//!   installed there from DOS, or added from this Mac (ZIPs, folders, files). RAD keeps a
//!   `DOSBOX.BAT` there that DOSBox runs at start (it calls the user's own `AUTOEXEC.BAT`).
//! - Floppy and CD images (IMG, IMA, ISO, CUE) go into drives A: and D: while it runs.
//! - Pictures: the engine sends each changed picture; the page is told once ("dos-frame") and
//!   pulls the latest with `dos_frame`, so a busy page never falls behind. Sound plays in the
//!   engine itself. Events (ready, messages, exit) arrive as "dos-event".

use std::io::{BufRead, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::client::{CmdError, CmdResult};

const BIN: &str = if cfg!(windows) { "rad-dos.exe" } else { "rad-dos" };
const CORE: &str = if cfg!(target_os = "macos") { "dosbox_pure_libretro.dylib" } else if cfg!(windows) { "dosbox_pure_libretro.dll" } else { "dosbox_pure_libretro.so" };

struct Engine {
    child: Child,
    stdin: ChildStdin,
}

#[derive(Default)]
pub struct Dos {
    engine: Mutex<Option<Engine>>,
    frame: Arc<Mutex<(Vec<u8>, bool)>>,
    signalled: Arc<AtomicBool>,
}

impl Dos {
    pub fn stop(&self) {
        if let Some(mut e) = self.engine.lock().unwrap().take() {
            let _ = e.stdin.write_all(b"{\"cmd\":\"quit\"}\n");
            let _ = e.stdin.flush();
            // A moment to close cleanly (files the DOS program had open), then make sure.
            for _ in 0..20 {
                if matches!(e.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let _ = e.child.kill();
            let _ = e.child.wait();
        }
    }

    fn running(&self) -> bool {
        let mut g = self.engine.lock().unwrap();
        let alive = g.as_mut().is_some_and(|e| matches!(e.child.try_wait(), Ok(None)));
        if !alive {
            *g = None;
        }
        alive
    }

    fn send(&self, cmd: &Value) -> CmdResult<()> {
        let mut g = self.engine.lock().unwrap();
        let e = g.as_mut().ok_or_else(|| CmdError::new("dos_stopped", "The PC is off"))?;
        let mut line = serde_json::to_vec(cmd)?;
        line.push(b'\n');
        e.stdin.write_all(&line).and_then(|_| e.stdin.flush()).map_err(|e| CmdError::new("dos_stopped", format!("The PC stopped: {e}")))
    }
}

fn data_dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("dos")
}

/// Drive C: — the PC's hard disk.
fn hdd(app: &AppHandle) -> PathBuf {
    data_dir(app).join("c")
}

/// The engine's folder (rad-dos with DOSBox Pure next to it): in the app bundle or, in
/// development, where scripts/build-dos.sh lays it out. `RAD_DOS_DIR` overrides these.
fn engine_dir(app: &AppHandle) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("RAD_DOS_DIR") {
        c.push(p.into());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        c.push(dir.join("../Helpers/rad-dos"));
        c.push(dir.join("rad-dos"));
    }
    if let Ok(d) = app.path().resource_dir() {
        c.push(d.join("rad-dos"));
    }
    if cfg!(debug_assertions) {
        c.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dos/rad-dos/target/bundle/rad-dos"));
    }
    c.into_iter().find(|d| d.join(BIN).is_file() && d.join(CORE).is_file())
}

/// What DOSBox runs at start (RAD's; the user's own start-up commands go in AUTOEXEC.BAT).
const DOSBOX_BAT: &str = "@echo off\r\n\
rem RAD starts the PC with this file (it's rewritten each time). Put your own start-up\r\n\
rem commands in C:\\AUTOEXEC.BAT.\r\n\
path Z:\\;C:\\;C:\\DOS;C:\\UTILS\r\n\
if exist C:\\AUTOEXEC.BAT call C:\\AUTOEXEC.BAT\r\n\
echo.\r\n\
echo RAD DOS - drive C: is your hard disk. Type DIR to look around,\r\n\
echo CD \\FOLDER to go into a folder, and a program's name to run it.\r\n\
echo.\r\n";

fn prepare_hdd(app: &AppHandle) -> CmdResult<PathBuf> {
    let c = hdd(app);
    std::fs::create_dir_all(&c).map_err(|e| CmdError::new("io", e.to_string()))?;
    std::fs::write(c.join("DOSBOX.BAT"), DOSBOX_BAT).map_err(|e| CmdError::new("io", e.to_string()))?;
    for d in ["system", "save"] {
        let _ = std::fs::create_dir_all(data_dir(app).join(d));
    }
    Ok(c)
}

/// The DOSBox Pure options for the PC the page asked for (memory, speed, sound, graphics).
fn core_options(settings: &Value) -> Value {
    let pick = |k: &str, allowed: &[&str], default: &str| -> String {
        settings.get(k).and_then(Value::as_str).filter(|v| allowed.contains(v)).unwrap_or(default).to_string()
    };
    json!({
        "dosbox_pure_memory_size": pick("memory", &["4", "8", "16", "32", "64"], "8"),
        "dosbox_pure_cycles": pick("speed", &["auto", "max", "315", "1320", "2750", "4720", "7800", "13400", "26800", "77000"], "auto"),
        "dosbox_pure_sblaster_type": pick("sound", &["sb16", "sbpro2", "sbpro1", "sb2", "sb1", "none"], "sb16"),
        "dosbox_pure_machine": pick("graphics", &["svga", "vga", "ega", "cga", "tandy", "hercules"], "svga"),
        "dosbox_pure_svga": pick("svga", &["svga_s3", "vesa_nolfb", "vesa_oldvbe", "svga_et4000", "svga_et3000", "svga_paradise"], "svga_s3"),
        "dosbox_pure_svgamem": pick("svgamem", &["1", "2", "4", "8"], "2"),
        "dosbox_pure_gus": if settings.get("gus").and_then(Value::as_bool).unwrap_or(false) { "true" } else { "false" },
        "dosbox_pure_keyboard_layout": pick("keyboard", &["us", "uk", "gr", "fr", "sp", "it", "sv", "dk", "nl", "no", "su", "pl", "ru"], "us"),
        // Fixed for RAD: on Apple Silicon Macs the interpreter (DOSBox Pure's recompiler needs
        // JIT memory macOS doesn't give it there; the interpreter is fast enough for DOOM and its
        // peers), elsewhere the recompiler for protected-mode games; the SB at its usual
        // settings, high-quality OPL3, no Voodoo (it needs OpenGL), no on-screen keyboard or
        // save states (C: is outside them), the mouse as a mouse.
        "dosbox_pure_cpu_core": if cfg!(all(target_os = "macos", target_arch = "aarch64")) { "normal" } else { "auto" },
        "dosbox_pure_cpu_type": "auto",
        "dosbox_pure_sblaster_conf": "A220 I7 D1 H5",
        "dosbox_pure_sblaster_adlib_mode": "auto",
        "dosbox_pure_sblaster_adlib_emu": "nuked",
        "dosbox_pure_voodoo": "off",
        "dosbox_pure_on_screen_keyboard": "false",
        "dosbox_pure_savestate": "disabled",
        "dosbox_pure_perfstats": "none",
        "dosbox_pure_mouse_input": "true",
        "dosbox_pure_auto_mapping": "false",
        "dosbox_pure_aspect_correction": "false",
        "dosbox_pure_conf": "false",
    })
}

/// Whether the engine is built, if the PC is on, and the hard disk's folder and size.
#[tauri::command]
pub fn dos_status(app: AppHandle, dos: State<'_, Dos>) -> Value {
    let c = hdd(&app);
    json!({"engine": engine_dir(&app).is_some(), "running": dos.running(), "hdd": c.to_string_lossy(), "used": dir_size(&c)})
}

fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten().map(|e| match e.metadata() {
        Ok(m) if m.is_dir() => dir_size(&e.path()),
        Ok(m) => m.len(),
        Err(_) => 0,
    }).sum()
}

/// Turns the PC on (off first if it's on) with these settings (see `core_options`).
#[tauri::command]
pub fn dos_start(app: AppHandle, dos: State<'_, Dos>, settings: Option<Value>) -> CmdResult<()> {
    dos.stop();
    let dir = engine_dir(&app).ok_or_else(|| {
        CmdError::new(
            "dos_missing",
            if cfg!(debug_assertions) { "The DOS engine isn't built yet. Run ./scripts/build-dos.sh, then try again." } else { "This copy of RAD was built without the DOS engine." },
        )
    })?;
    let c = prepare_hdd(&app)?;
    let options = core_options(&settings.unwrap_or(Value::Null));
    let mut child = crate::helper_command(dir.join(BIN))
        // RAD's patch to DOSBox Pure: C: is the folder, written directly. Set here, not in the
        // engine, so the core's C runtime sees it from the start (Windows).
        .env("RAD_DOS_DIRECT_C", "1")
        .arg("--core")
        .arg(dir.join(CORE))
        .arg("--hdd")
        .arg(&c)
        .arg("--system")
        .arg(data_dir(&app).join("system"))
        .arg("--save")
        .arg(data_dir(&app).join("save"))
        .arg("--options")
        .arg(options.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CmdError::new("dos_missing", format!("Couldn't start the DOS engine: {e}")))?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            tracing::debug!(target: "rad_dos", "{line}");
        }
    });
    *dos.frame.lock().unwrap() = (Vec::new(), false);
    dos.signalled.store(false, Ordering::Release);
    let (frame, signalled) = (dos.frame.clone(), dos.signalled.clone());
    std::thread::spawn(move || read_engine(app, stdout, frame, signalled));
    *dos.engine.lock().unwrap() = Some(Engine { child, stdin });
    Ok(())
}

fn read_engine(app: AppHandle, stdout: std::process::ChildStdout, frame: Arc<Mutex<(Vec<u8>, bool)>>, signalled: Arc<AtomicBool>) {
    let mut r = std::io::BufReader::with_capacity(1 << 20, stdout);
    let mut head = [0u8; 5];
    let mut buf = Vec::new();
    while r.read_exact(&mut head).is_ok() {
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        buf.resize(len, 0);
        if r.read_exact(&mut buf).is_err() {
            break;
        }
        match head[0] {
            1 => {
                if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                    let _ = app.emit_to("main", "dos-event", v);
                }
            }
            2 if buf.len() >= 12 => {
                {
                    let mut f = frame.lock().unwrap();
                    std::mem::swap(&mut f.0, &mut buf);
                    f.1 = true;
                }
                // One notification until the page pulls; pictures in between are dropped.
                if !signalled.swap(true, Ordering::AcqRel) {
                    let _ = app.emit_to("main", "dos-frame", ());
                }
            }
            _ => {}
        }
    }
    let _ = app.emit_to("main", "dos-event", json!({"event": "exit"}));
}

/// The latest picture: `[w u32][h u32][format u8 + 3]` then (format 1) a 768-byte palette and
/// a byte per pixel, or (format 4) RGBA. Just a header of zeros when there's nothing new.
#[tauri::command]
pub fn dos_frame(dos: State<'_, Dos>) -> Response {
    let mut f = dos.frame.lock().unwrap();
    dos.signalled.store(false, Ordering::Release);
    if !f.1 || f.0.is_empty() {
        return Response::new(vec![0u8; 12]);
    }
    f.1 = false;
    Response::new(f.0.clone())
}

/// Keys, the mouse, the joystick and the rest, passed to the engine (see dos/rad-dos):
/// key {code, down} · release_all · mouse {dx, dy, buttons, wheel} · joy {bits} · type {text} ·
/// pause {on} · reset · volume {value} · insert {path} · eject.
#[tauri::command]
pub fn dos_send(dos: State<'_, Dos>, cmd: Value) -> CmdResult<()> {
    let name = cmd.get("cmd").and_then(Value::as_str).unwrap_or("");
    if !matches!(name, "key" | "release_all" | "mouse" | "joy" | "type" | "pause" | "reset" | "volume" | "insert" | "eject") {
        return Err(CmdError::new("bad_request", "unknown command"));
    }
    if name == "insert" {
        let p = cmd.get("path").and_then(Value::as_str).unwrap_or("");
        let ext = p.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !Path::new(p).is_file() || !matches!(ext.as_str(), "img" | "ima" | "vfd" | "iso" | "cue" | "chd") {
            return Err(CmdError::new("bad_request", "Use a floppy (IMG, IMA) or CD (ISO, CUE, CHD) image"));
        }
    }
    dos.send(&cmd)
}

#[tauri::command]
pub fn dos_stop(dos: State<'_, Dos>) {
    dos.stop();
}

// ------------------------------------------------------------------ the hard disk

/// A path inside drive C: from "GAMES\DOOM" (no way out of the folder).
fn inside(app: &AppHandle, rel: &str) -> CmdResult<PathBuf> {
    let rel = rel.replace('\\', "/");
    let rel = rel.trim_start_matches(['/', '\\']).trim_start_matches("C:").trim_start_matches(['/', '\\']);
    let p = Path::new(rel);
    if !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(CmdError::new("bad_request", "that path isn't on drive C:"));
    }
    Ok(hdd(app).join(p))
}

/// What's in a folder of drive C: (folders first).
#[tauri::command]
pub fn dos_files(app: AppHandle, path: Option<String>) -> CmdResult<Vec<Value>> {
    let dir = inside(&app, path.as_deref().unwrap_or(""))?;
    let mut out: Vec<(bool, String, u64)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let m = e.metadata().ok();
            let is_dir = m.as_ref().is_some_and(|m| m.is_dir());
            out.push((is_dir, name, if is_dir { dir_size(&e.path()) } else { m.map(|m| m.len()).unwrap_or(0) }));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    Ok(out.into_iter().map(|(d, n, s)| json!({"name": n, "dir": d, "size": s})).collect())
}

/// The programs on drive C: (EXE, COM, BAT), up to three folders deep, as DOS paths.
#[tauri::command]
pub fn dos_programs(app: AppHandle) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, depth: u32, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                if depth < 3 {
                    walk(base, &p, depth + 1, out);
                }
                continue;
            }
            let ext = p.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if matches!(ext.as_str(), "exe" | "com" | "bat") && out.len() < 500 {
                let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('/', "\\");
                if rel != "DOSBOX.BAT" {
                    out.push(rel.to_uppercase());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&hdd(&app), &hdd(&app), 0, &mut out);
    out
}

/// A DOS-friendly folder name from a file name: letters and digits, at most 8.
fn dos_name(s: &str) -> String {
    let stem = Path::new(s).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    let n: String = stem.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect::<String>().to_uppercase();
    if n.is_empty() { "PROGRAM".into() } else { n }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to.join(&name))?;
        } else {
            std::fs::copy(e.path(), to.join(&name))?;
        }
    }
    Ok(())
}

/// Unpacks a ZIP into `to`; when everything in it is inside one folder, that folder's contents.
fn unzip(file: &Path, to: &Path) -> Result<(), String> {
    let f = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("not a ZIP file: {e}"))?;
    // Mac junk (__MACOSX/, ._files) isn't part of the program.
    let junk = |n: &str| n.split('/').any(|c| c.starts_with("__MACOSX") || c.starts_with('.'));
    let names: Vec<String> = (0..z.len())
        .filter_map(|i| z.by_index(i).ok().and_then(|e| e.enclosed_name()).map(|p| p.to_string_lossy().replace('\\', "/")))
        .filter(|n| !junk(n))
        .collect();
    let top = names.first().and_then(|n| n.split('/').next()).map(String::from);
    let strip = top.filter(|t| names.iter().all(|n| n.starts_with(&format!("{t}/")) || n == t)).map(|t| format!("{t}/"));
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = e.enclosed_name() else { continue };
        let name = name.to_string_lossy().replace('\\', "/");
        let rel = match &strip {
            Some(s) => match name.strip_prefix(s.as_str()) {
                Some(r) if !r.is_empty() => r.to_string(),
                _ => continue,
            },
            None => name,
        };
        if junk(&rel) {
            continue;
        }
        let out = to.join(&rel);
        if e.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut e, &mut w).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Puts things from this Mac onto drive C: — a ZIP is unpacked and a folder copied into
/// `C:\<into>\<NAME>`; other files are copied into `C:\<into>`. Returns the DOS folders made.
#[tauri::command]
pub async fn dos_import(app: AppHandle, paths: Vec<String>, into: Option<String>) -> CmdResult<Vec<String>> {
    let into = into.unwrap_or_else(|| "GAMES".into());
    let base = inside(&app, &into)?;
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&base).map_err(|e| CmdError::new("io", e.to_string()))?;
        let mut made = Vec::new();
        for p in paths {
            let src = PathBuf::from(&p);
            let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let is_zip = src.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
            if src.is_dir() || is_zip {
                let mut folder = dos_name(&name);
                let mut n = 1;
                while base.join(&folder).exists() {
                    n += 1;
                    folder = format!("{}{n}", &dos_name(&name)[..dos_name(&name).len().min(6)]);
                }
                let dest = base.join(&folder);
                if is_zip {
                    unzip(&src, &dest).map_err(|e| CmdError::new("io", format!("{name}: {e}")))?;
                } else {
                    copy_dir(&src, &dest).map_err(|e| CmdError::new("io", format!("{name}: {e}")))?;
                }
                made.push(format!("C:\\{}\\{folder}", into.trim_matches('\\').to_uppercase()));
            } else if src.is_file() {
                std::fs::copy(&src, base.join(&name)).map_err(|e| CmdError::new("io", format!("{name}: {e}")))?;
                made.push(format!("C:\\{}", into.trim_matches('\\').to_uppercase()));
            }
        }
        Ok(made)
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?
}

/// Deletes a file or folder on drive C:.
#[tauri::command]
pub fn dos_delete(app: AppHandle, path: String) -> CmdResult<()> {
    let p = inside(&app, &path)?;
    if p == hdd(&app) {
        return Err(CmdError::new("bad_request", "that's the whole drive"));
    }
    if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) }.map_err(|e| CmdError::new("io", e.to_string()))
}

/// Makes a new folder on drive C:.
#[tauri::command]
pub fn dos_mkdir(app: AppHandle, path: String) -> CmdResult<()> {
    std::fs::create_dir_all(inside(&app, &path)?).map_err(|e| CmdError::new("io", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_names() {
        assert_eq!(dos_name("doom19s.zip"), "DOOM19S");
        assert_eq!(dos_name("Commander Keen 4.zip"), "COMMANDE");
        assert_eq!(dos_name("…"), "PROGRAM");
    }

    #[test]
    fn unzips_one_folder_flat() {
        let td = tempfile::tempdir().unwrap();
        let zp = td.path().join("g.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("KEEN/KEEN4.EXE", o).unwrap();
            z.write_all(b"MZ").unwrap();
            z.start_file("KEEN/DATA/LEVEL1.DAT", o).unwrap();
            z.write_all(b"x").unwrap();
            z.start_file("__MACOSX/KEEN/._KEEN4.EXE", o).unwrap();
            z.finish().unwrap();
        }
        let out = td.path().join("out");
        unzip(&zp, &out).unwrap();
        assert!(out.join("KEEN4.EXE").is_file());
        assert!(out.join("DATA/LEVEL1.DAT").is_file());
        assert!(!out.join("__MACOSX").exists());
    }

    #[test]
    fn options_stay_in_bounds() {
        let o = core_options(&json!({"memory": "8", "speed": "max", "sound": "nonsense"}));
        assert_eq!(o["dosbox_pure_memory_size"], "8");
        assert_eq!(o["dosbox_pure_cycles"], "max");
        assert_eq!(o["dosbox_pure_sblaster_type"], "sb16");
        assert_eq!(o["dosbox_pure_cpu_core"], if cfg!(all(target_os = "macos", target_arch = "aarch64")) { "normal" } else { "auto" });
    }
}

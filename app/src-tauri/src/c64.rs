//! The Commodore 64 view: a C64 (crates/rad-c64) running on its own thread at 50 frames a
//! second, like the ZX Spectrum view (zx.rs): the picture goes to the page over a channel
//! (one byte per pixel, colour 0–15, only when it changed), the sound to this Mac's audio
//! output (zx.rs's), and the page sends key and joystick states.
//!
//! ROMs: the page reads the user's own C64 ROMs from the encrypted Games database and hands
//! them over (`c64_set_roms`); without them RAD's C64 uses the free Open ROMs. The set in use
//! is kept here for the view, the Games view's checks and the lab.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use rad_c64::{Loaded, Roms, C64};
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::client::{CmdError, CmdResult};

enum Cmd {
    Attach(Channel<InvokeResponseBody>),
    Keys(Vec<u8>),
    Joy(u8),
    JoyPort(u8),
    Load(String, Vec<u8>, Sender<Result<Loaded, String>>),
    Disk(String, Vec<u8>, Sender<Result<Vec<String>, String>>),
    SaveState(Sender<Result<Vec<u8>, String>>),
    Roms(Arc<Roms>),
    Reset,
    Pause(bool),
    Tape(&'static str),
    FastLoad(bool),
    Volume(f32),
    Memory(Sender<Vec<u8>>),
    Poke(u16, u8),
    Lock(u16, Option<u8>),
    Quit,
}

pub struct C64State {
    tx: Mutex<Option<Sender<Cmd>>>,
    roms: Mutex<Arc<Roms>>,
}

impl Default for C64State {
    fn default() -> Self {
        C64State { tx: Mutex::new(None), roms: Mutex::new(Arc::new(Roms::open_roms())) }
    }
}

impl C64State {
    fn send(&self, c: Cmd) -> CmdResult<()> {
        let tx = self.tx.lock().unwrap();
        tx.as_ref().ok_or_else(|| CmdError::new("c64", "The C64 isn't on"))?.send(c).map_err(|_| CmdError::new("c64", "The C64 stopped"))
    }

    /// The ROMs in use (the user's, or the free Open ROMs).
    pub fn roms(&self) -> Arc<Roms> {
        self.roms.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        if let Some(tx) = self.tx.lock().unwrap().take() {
            let _ = tx.send(Cmd::Quit);
        }
    }
}

fn b64(d: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(d)
}

fn unb64(s: &str) -> CmdResult<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).map_err(|_| CmdError::new("bad_request", "That file's data is damaged"))
}

/// The emulator thread: one frame every ~20 ms (faster while starting a program or fast
/// loading a tape).
fn run(rx: Receiver<Cmd>, roms: Arc<Roms>) {
    let mut c = C64::new(roms);
    let sound = crate::zx::open_sound();
    c.set_sample_rate(sound.rate);
    let (w, h) = c.size();
    let mut frame = vec![0u8; w * h];
    let mut last = vec![255u8; w * h];
    let mut out: Option<Channel<InvokeResponseBody>> = None;
    let mut paused = false;
    let mut samples = Vec::new();
    let mut next = Instant::now();
    let mut status_at = Instant::now();
    let mut last_status = Value::Null;
    const FRAME: Duration = Duration::from_micros(rad_c64::FRAME_US);

    'main: loop {
        loop {
            let cmd = if paused {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(c) => c,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(_) => break 'main,
                }
            } else {
                match rx.try_recv() {
                    Ok(c) => c,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(_) => break 'main,
                }
            };
            match cmd {
                Cmd::Attach(ch) => {
                    let _ = ch.send(InvokeResponseBody::Json(json!({"size": [w, h], "palette": rad_c64::PALETTE}).to_string()));
                    last.fill(255);
                    last_status = Value::Null;
                    out = Some(ch);
                }
                Cmd::Keys(k) => c.set_keys(&k),
                Cmd::Joy(j) => c.set_joystick(j),
                Cmd::JoyPort(p) => c.set_joystick_port(p),
                Cmd::Load(name, data, reply) => {
                    let _ = reply.send(c.load(&name, &data).map_err(|e| e.to_string()));
                    paused = false;
                }
                Cmd::Disk(name, data, reply) => {
                    let _ = reply.send(c.insert_disk(&name, &data).map_err(|e| e.to_string()));
                }
                Cmd::SaveState(reply) => {
                    let _ = reply.send(c.save_state().map_err(|e| e.to_string()));
                }
                Cmd::Roms(r) => {
                    if !Arc::ptr_eq(c.roms(), &r) {
                        c.set_roms(r);
                        c.set_sample_rate(sound.rate);
                    }
                }
                Cmd::Reset => c.reset(),
                Cmd::Pause(p) => paused = p,
                Cmd::Tape(a) => match a {
                    "play" => c.tape_play(true),
                    "stop" => c.tape_play(false),
                    "rewind" => c.tape_rewind(),
                    "eject_disk" => c.eject_disk(),
                    "eject_cart" => c.remove_cartridge(),
                    _ => c.eject(),
                },
                Cmd::FastLoad(f) => c.fast_load = f,
                Cmd::Volume(v) => *sound.volume.lock().unwrap() = v.clamp(0.0, 1.0),
                Cmd::Memory(reply) => {
                    let _ = reply.send(c.memory());
                }
                Cmd::Poke(a, v) => c.poke(a, v),
                Cmd::Lock(a, v) => c.lock(a, v),
                Cmd::Quit => break 'main,
            }
        }
        if paused {
            next = Instant::now();
            continue;
        }

        if c.is_turbo() {
            // Starting up / fast loading: as many frames as fit in one frame's time, silently.
            let until = Instant::now() + FRAME;
            sound.queue.lock().unwrap().clear();
            while c.is_turbo() && Instant::now() < until {
                c.run_frame();
                c.skip_audio();
            }
        } else {
            c.run_frame();
            c.audio(&mut samples);
            let mut q = sound.queue.lock().unwrap();
            // Keep latency low: never more than ~120 ms queued.
            let max = (sound.rate as usize * 2) * 12 / 100;
            if q.len() + samples.len() > max {
                let drop = (q.len() + samples.len() - max).min(q.len());
                q.drain(..drop);
            }
            q.extend(samples.iter().copied());
        }

        c.render(&mut frame);
        let mut gone = false;
        if let Some(ch) = &out {
            if frame != last {
                gone = ch.send(InvokeResponseBody::Raw(frame.clone())).is_err();
                std::mem::swap(&mut frame, &mut last);
            }
            if status_at.elapsed() > Duration::from_millis(250) {
                status_at = Instant::now();
                let tape = c.tape_state().map(|(playing, progress, name)| json!({"playing": playing, "progress": progress, "name": name}));
                let disk = c.disk_state().map(|(name, files)| json!({"name": name, "files": files}));
                let st = json!({"status": {
                    "roms": if c.roms().is_open_roms() { "open" } else { "commodore" },
                    "turbo": c.is_turbo(), "starting": c.is_starting(), "tape": tape, "disk": disk,
                    "cartridge": c.has_cartridge(), "fast_load": c.fast_load, "joy_port": c.joystick_port(),
                }});
                if st != last_status {
                    let _ = ch.send(InvokeResponseBody::Json(st.to_string()));
                    last_status = st;
                }
            }
        }
        if gone {
            out = None;
        }

        if !c.is_turbo() {
            next += FRAME;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > Duration::from_millis(200) {
                next = now;
            }
        } else {
            next = Instant::now();
        }
    }
}

/// Turns the C64 on (if needed) and connects the page: messages are the picture (bytes) or
/// JSON ({size, palette} first, then {status}).
#[tauri::command]
pub fn c64_attach(s: State<'_, C64State>, on_frame: Channel<InvokeResponseBody>) -> CmdResult<()> {
    {
        let mut tx = s.tx.lock().unwrap();
        if tx.is_none() {
            let (t, r) = mpsc::channel();
            let roms = s.roms();
            std::thread::Builder::new().name("c64".into()).spawn(move || run(r, roms)).map_err(|e| CmdError::new("c64", e.to_string()))?;
            *tx = Some(t);
        }
    }
    s.send(Cmd::Attach(on_frame))
}

/// The keys held down: their places in the keyboard matrix (row × 8 + column; 64 is RESTORE).
#[tauri::command]
pub fn c64_keys(s: State<'_, C64State>, keys: Vec<u8>) -> CmdResult<()> {
    s.send(Cmd::Keys(keys))
}

/// The joystick: up 1, down 2, left 4, right 8, fire 16.
#[tauri::command]
pub fn c64_joystick(s: State<'_, C64State>, bits: u8) -> CmdResult<()> {
    s.send(Cmd::Joy(bits))
}

/// Which port the joystick is in (1 or 2).
#[tauri::command]
pub fn c64_joystick_port(s: State<'_, C64State>, port: u8) -> CmdResult<()> {
    s.send(Cmd::JoyPort(port))
}

fn ask<T: Send + 'static>(rx: Receiver<T>, secs: u64) -> impl std::future::Future<Output = CmdResult<T>> {
    async move {
        tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(secs)))
            .await
            .map_err(|e| CmdError::new("c64", e.to_string()))?
            .map_err(|_| CmdError::new("c64", "The C64 didn't answer"))
    }
}

/// Loads a program (PRG/P00/T64: reset, then RUN), a tape (TAP: LOAD and play), a disk (D64:
/// LOAD"*",8,1 and RUN), a cartridge (CRT) or a save state (C64S). `data` is base64.
#[tauri::command]
pub async fn c64_load(s: State<'_, C64State>, name: String, data: String) -> CmdResult<Value> {
    let bytes = unb64(&data)?;
    let (tx, rx) = mpsc::channel();
    s.send(Cmd::Load(name, bytes, tx))?;
    match ask(rx, 10).await?.map_err(|e| CmdError::new("c64", e))? {
        Loaded::Program { name } => Ok(json!({"kind": "program", "names": [name]})),
        Loaded::Tape { names } => Ok(json!({"kind": "tape", "names": names})),
        Loaded::Disk { names } => Ok(json!({"kind": "disk", "names": names})),
        Loaded::Cartridge { name } => Ok(json!({"kind": "cartridge", "names": [name]})),
        Loaded::Snapshot => Ok(json!({"kind": "snapshot"})),
    }
}

/// Puts a disk (D64) in the drive without loading anything (for games that ask for disk 2).
#[tauri::command]
pub async fn c64_insert_disk(s: State<'_, C64State>, name: String, data: String) -> CmdResult<Vec<String>> {
    let bytes = unb64(&data)?;
    let (tx, rx) = mpsc::channel();
    s.send(Cmd::Disk(name, bytes, tx))?;
    ask(rx, 10).await?.map_err(|e| CmdError::new("c64", e))
}

/// The machine's state (base64), to keep as a save state.
#[tauri::command]
pub async fn c64_save_state(s: State<'_, C64State>) -> CmdResult<String> {
    let (tx, rx) = mpsc::channel();
    s.send(Cmd::SaveState(tx))?;
    Ok(b64(&ask(rx, 10).await?.map_err(|e| CmdError::new("c64", e))?))
}

/// reset · pause · resume · tape_play · tape_stop · tape_rewind · eject · eject_disk ·
/// eject_cart · fast_on · fast_off · volume (with `value` 0–1).
#[tauri::command]
pub fn c64_control(s: State<'_, C64State>, action: String, value: Option<f32>) -> CmdResult<()> {
    let c = match action.as_str() {
        "reset" => Cmd::Reset,
        "pause" => Cmd::Pause(true),
        "resume" => Cmd::Pause(false),
        "tape_play" => Cmd::Tape("play"),
        "tape_stop" => Cmd::Tape("stop"),
        "tape_rewind" => Cmd::Tape("rewind"),
        "eject" => Cmd::Tape("eject"),
        "eject_disk" => Cmd::Tape("eject_disk"),
        "eject_cart" => Cmd::Tape("eject_cart"),
        "fast_on" => Cmd::FastLoad(true),
        "fast_off" => Cmd::FastLoad(false),
        "volume" => Cmd::Volume(value.unwrap_or(0.8)),
        _ => return Err(CmdError::new("bad_request", "unknown action")),
    };
    s.send(c)
}

/// The live machine's memory as the CPU sees it (64K, base64): for the lab's cheat finder.
#[tauri::command]
pub async fn c64_memory(s: State<'_, C64State>) -> CmdResult<String> {
    let (tx, rx) = mpsc::channel();
    s.send(Cmd::Memory(tx))?;
    Ok(b64(&ask(rx, 5).await?))
}

#[tauri::command]
pub fn c64_poke(s: State<'_, C64State>, addr: u16, value: u8) -> CmdResult<()> {
    s.send(Cmd::Poke(addr, value))
}

/// Holds a byte at a value every frame (`value` null releases it).
#[tauri::command]
pub fn c64_lock(s: State<'_, C64State>, addr: u16, value: Option<u8>) -> CmdResult<()> {
    s.send(Cmd::Lock(addr, value))
}

#[tauri::command]
pub fn c64_power_off(s: State<'_, C64State>) {
    s.stop();
}

// ------------------------------------------------------------------ ROMs

/// Uses these ROMs (base64: character 4K, BASIC 8K, KERNAL 8K), or the free Open ROMs when
/// they're null. Returns which set is in use ("commodore" or "open").
#[tauri::command]
pub fn c64_set_roms(s: State<'_, C64State>, chars: Option<String>, basic: Option<String>, kernal: Option<String>) -> CmdResult<String> {
    let roms = match (chars, basic, kernal) {
        (Some(c), Some(b), Some(k)) => Roms::new(unb64(&c)?, unb64(&b)?, unb64(&k)?).map_err(|e| CmdError::new("c64_roms", e.to_string()))?,
        _ => Roms::open_roms(),
    };
    let kind = if roms.is_open_roms() { "open" } else { "commodore" };
    let roms = Arc::new(roms);
    *s.roms.lock().unwrap() = roms.clone();
    if s.tx.lock().unwrap().is_some() {
        s.send(Cmd::Roms(roms))?;
    }
    Ok(kind.into())
}

/// What a ROM image is, by its size and contents: "chars", "basic" or "kernal".
fn rom_kind(data: &[u8]) -> Option<&'static str> {
    match data.len() {
        0x1000 => Some("chars"),
        0x2000 => {
            // BASIC has its name after its start vectors ("CBMBASIC"; the Open ROMs'
            // "ORGBASIC"); the KERNAL ends with the CPU's vectors into E000–FFFF.
            let reset = u16::from_le_bytes([data[0x1ffc], data[0x1ffd]]);
            if data[7..12] == *b"BASIC" {
                Some("basic")
            } else if reset >= 0xe000 {
                Some("kernal")
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Reads ROM files the user picked (or found): which is which, as base64. Unknown files are
/// listed in `skipped`.
#[tauri::command]
pub async fn c64_read_roms(paths: Vec<String>) -> CmdResult<Value> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = serde_json::Map::new();
        let mut skipped = Vec::new();
        for p in paths {
            let p = PathBuf::from(p);
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Ok(data) = std::fs::read(&p).map(|d| if d.len() > 0x4000 { Vec::new() } else { d }) else {
                skipped.push(name);
                continue;
            };
            match rom_kind(&data) {
                Some(k) => {
                    out.insert(k.into(), json!(b64(&data)));
                }
                None => skipped.push(name),
            }
        }
        Ok(json!({"roms": out, "skipped": skipped}))
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?
}

/// Looks for C64 ROMs from an installed VICE emulator: the files found (character, BASIC,
/// KERNAL), or none.
#[tauri::command]
pub fn c64_find_roms() -> Vec<String> {
    let mut dirs: Vec<PathBuf> = vec!["/opt/homebrew/share/vice/C64".into(), "/usr/local/share/vice/C64".into(), "/usr/share/vice/C64".into()];
    for apps in ["/Applications".to_string(), format!("{}/Applications", std::env::var("HOME").unwrap_or_default())] {
        if let Ok(rd) = std::fs::read_dir(&apps) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if n.contains("vice") {
                    find_c64_dirs(&e.path(), 0, &mut dirs);
                }
            }
        }
    }
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        let pick = |prefix: &str| files.iter().filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with(prefix))).min().cloned();
        if let (Some(c), Some(b), Some(k)) = (pick("chargen"), pick("basic"), pick("kernal")) {
            return [c, b, k].iter().map(|p| p.to_string_lossy().into_owned()).collect();
        }
    }
    Vec::new()
}

fn find_c64_dirs(p: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(p) else { return };
    for e in rd.flatten() {
        let path = e.path();
        if path.is_dir() {
            if e.file_name() == "C64" {
                out.push(path);
            } else {
                find_c64_dirs(&path, depth + 1, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tells_roms_apart() {
        let r = rad_c64::Roms::open_roms();
        assert_eq!(super::rom_kind(&r.chars), Some("chars"));
        assert_eq!(super::rom_kind(&r.basic), Some("basic"));
        assert_eq!(super::rom_kind(&r.kernal), Some("kernal"));
        assert_eq!(super::rom_kind(&[0; 100]), None);
    }
}

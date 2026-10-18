//! RAD's DOS engine: a small libretro host for DOSBox Pure (built by scripts/build-dos.sh), run
//! as its own process by the app's DOS view (app/src-tauri/src/dos.rs).
//!
//! - args: `--core <dosbox_pure_libretro.dylib> --hdd <folder> --system <folder> --save <folder>
//!   --options <JSON object of DOSBox Pure core options>`. The folder is drive C: (RAD's "hard
//!   disk"), written directly (RAD's patch to DOSBox Pure, `RAD_DOS_DIRECT_C`).
//! - stdin: one JSON command per line (see `Host::command`).
//! - stdout: framed messages `[kind: u8][len: u32 LE][payload]`:
//!     1 = JSON event (`ready`, `geometry`, `message`, `error`, `exit`),
//!     2 = picture: `[w: u32][h: u32][format: u8][0, 0, 0]` then, for format 1, a 256-colour
//!         palette (768 bytes RGB) and one byte per pixel; for format 4, RGBA pixels.
//! The sound is played here (cpal), not sent to the app.

mod retro;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{BufRead, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use retro::*;

/// What the core's callbacks share with the main loop.
struct Shared {
    frame: Vec<u32>,
    width: u32,
    height: u32,
    frame_new: bool,
    keys: HashSet<u32>,
    mouse_acc: (i32, i32),
    mouse_frame: (i16, i16),
    buttons: u8,
    wheel: i8,
    joy: u16,
    options: HashMap<String, CString>,
    options_changed: bool,
    system_dir: CString,
    save_dir: CString,
    keyboard: Option<KeyboardEventFn>,
    disk: Option<DiskControlExt>,
    av: Option<SystemAvInfo>,
    av_changed: bool,
    messages: Vec<String>,
    shutdown: bool,
    audio: Arc<Mutex<VecDeque<f32>>>,
    /// Core sample rate → device rate, and the fractional position between input samples.
    resample: (f64, f64),
    last: (f32, f32),
}

static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();

fn shared() -> std::sync::MutexGuard<'static, Shared> {
    SHARED.get().expect("set up").lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------ output

fn send(kind: u8, payload: &[u8]) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&[kind]);
    let _ = out.write_all(&(payload.len() as u32).to_le_bytes());
    let _ = out.write_all(payload);
    let _ = out.flush();
}

fn event(v: Value) {
    send(1, v.to_string().as_bytes());
}

/// The picture as indexed colours when it has 256 or fewer (almost always, for DOS), else RGBA.
fn encode(frame: &[u32], w: u32, h: u32, out: &mut Vec<u8>) {
    out.clear();
    out.extend(w.to_le_bytes());
    out.extend(h.to_le_bytes());
    let head = out.len();
    out.extend([1, 0, 0, 0]);
    let pal_at = out.len();
    out.resize(pal_at + 768, 0);
    // A small open-addressing table: colour → index.
    let mut table = [u32::MAX; 1024];
    let mut slots = [0u8; 1024];
    let mut count = 0usize;
    let mut indexed = true;
    let px_at = out.len();
    out.reserve(frame.len());
    for &c in frame {
        let c = c & 0x00ff_ffff;
        let mut h = (c.wrapping_mul(2_654_435_761) >> 22) as usize;
        let idx = loop {
            if table[h] == c {
                break slots[h];
            }
            if table[h] == u32::MAX {
                if count == 256 {
                    indexed = false;
                    break 0;
                }
                table[h] = c;
                slots[h] = count as u8;
                let p = pal_at + count * 3;
                out[p] = (c >> 16) as u8;
                out[p + 1] = (c >> 8) as u8;
                out[p + 2] = c as u8;
                count += 1;
                break slots[h];
            }
            h = (h + 1) & 1023;
        };
        if !indexed {
            break;
        }
        out.push(idx);
    }
    if !indexed {
        out.truncate(px_at - 768);
        out[head] = 4;
        out.reserve(frame.len() * 4);
        for &c in frame {
            out.extend([(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]);
        }
    }
}

// ------------------------------------------------------------------ the core's callbacks

unsafe extern "C" fn environment(cmd: u32, data: *mut c_void) -> bool {
    let cmd = cmd & 0xffff; // without RETRO_ENVIRONMENT_EXPERIMENTAL
    let mut s = shared();
    match cmd {
        ENV_GET_CAN_DUPE => {
            *(data as *mut bool) = true;
            true
        }
        ENV_SET_PIXEL_FORMAT => *(data as *const u32) == PIXEL_FORMAT_XRGB8888,
        ENV_GET_SYSTEM_DIRECTORY => {
            *(data as *mut *const c_char) = s.system_dir.as_ptr();
            true
        }
        ENV_GET_SAVE_DIRECTORY => {
            *(data as *mut *const c_char) = s.save_dir.as_ptr();
            true
        }
        ENV_SET_VARIABLES => {
            // Defaults for the options RAD doesn't set: "Description; default|other|…".
            let mut v = data as *const Variable;
            while !v.is_null() && !(*v).key.is_null() {
                let key = CStr::from_ptr((*v).key).to_string_lossy().into_owned();
                let value = CStr::from_ptr((*v).value).to_string_lossy();
                if let Some(def) = value.split_once("; ").map(|(_, o)| o.split('|').next().unwrap_or("").to_string()) {
                    s.options.entry(key).or_insert_with(|| CString::new(def).unwrap_or_default());
                }
                v = v.add(1);
            }
            true
        }
        ENV_GET_VARIABLE => {
            let v = data as *mut Variable;
            if v.is_null() || (*v).key.is_null() {
                return false;
            }
            let key = CStr::from_ptr((*v).key).to_string_lossy();
            match s.options.get(key.as_ref()) {
                Some(val) => {
                    (*v).value = val.as_ptr();
                    true
                }
                None => false,
            }
        }
        ENV_GET_VARIABLE_UPDATE => {
            *(data as *mut bool) = std::mem::take(&mut s.options_changed);
            true
        }
        ENV_SET_KEYBOARD_CALLBACK => {
            s.keyboard = Some((*(data as *const KeyboardCallback)).callback);
            true
        }
        ENV_SET_DISK_CONTROL_EXT_INTERFACE => {
            s.disk = Some(*(data as *const DiskControlExt));
            true
        }
        ENV_SET_DISK_CONTROL_INTERFACE => {
            let d = &*(data as *const DiskControl);
            s.disk = Some(DiskControlExt {
                set_eject_state: d.set_eject_state,
                get_eject_state: d.get_eject_state,
                get_image_index: d.get_image_index,
                set_image_index: d.set_image_index,
                get_num_images: d.get_num_images,
                replace_image_index: d.replace_image_index,
                add_image_index: d.add_image_index,
                set_initial_image: None,
                get_image_path: None,
                get_image_label: None,
            });
            true
        }
        ENV_SET_SYSTEM_AV_INFO => {
            s.av = Some(*(data as *const SystemAvInfo));
            s.av_changed = true;
            true
        }
        ENV_SET_GEOMETRY => {
            if let Some(av) = s.av.as_mut() {
                av.geometry = *(data as *const GameGeometry);
                s.av_changed = true;
            }
            true
        }
        ENV_SET_MESSAGE => {
            let m = &*(data as *const Message);
            if !m.msg.is_null() {
                let t = CStr::from_ptr(m.msg).to_string_lossy().into_owned();
                s.messages.push(t);
            }
            true
        }
        ENV_SET_MESSAGE_EXT => {
            let m = &*(data as *const MessageExt);
            if !m.msg.is_null() {
                let t = CStr::from_ptr(m.msg).to_string_lossy().into_owned();
                s.messages.push(t);
            }
            true
        }
        ENV_GET_AUDIO_VIDEO_ENABLE => {
            *(data as *mut i32) = 3;
            true
        }
        ENV_SHUTDOWN => {
            s.shutdown = true;
            true
        }
        ENV_SET_SUPPORT_NO_GAME | ENV_SET_INPUT_DESCRIPTORS | ENV_SET_CONTROLLER_INFO | ENV_SET_MEMORY_MAPS | ENV_SET_SUPPORT_ACHIEVEMENTS => true,
        // Software rendering only; no VFS, log, MIDI or perf interfaces: the core falls back.
        _ => false,
    }
}

unsafe extern "C" fn video_refresh(data: *const c_void, width: u32, height: u32, pitch: usize) {
    if data.is_null() || width == 0 || height == 0 {
        return; // the same picture as last time
    }
    let mut s = shared();
    let (w, h) = (width as usize, height as usize);
    s.frame.resize(w * h, 0);
    for y in 0..h {
        let row = std::slice::from_raw_parts((data as *const u8).add(y * pitch) as *const u32, w);
        s.frame[y * w..y * w + w].copy_from_slice(row);
    }
    s.width = width;
    s.height = height;
    s.frame_new = true;
}

unsafe extern "C" fn audio_sample(l: i16, r: i16) {
    audio_sample_batch([l, r].as_ptr(), 1);
}

unsafe extern "C" fn audio_sample_batch(data: *const i16, frames: usize) -> usize {
    let samples = std::slice::from_raw_parts(data, frames * 2);
    let mut s = shared();
    let (step, mut pos) = s.resample;
    let mut last = s.last;
    let q = s.audio.clone();
    let mut q = q.lock().unwrap();
    for f in samples.chunks_exact(2) {
        let cur = (f[0] as f32 / 32768.0, f[1] as f32 / 32768.0);
        // Linear resampling from the core's rate to the device's (often the same: step 1).
        while pos < 1.0 {
            q.push_back(last.0 + (cur.0 - last.0) * pos as f32);
            q.push_back(last.1 + (cur.1 - last.1) * pos as f32);
            pos += step;
        }
        pos -= 1.0;
        last = cur;
    }
    s.resample.1 = pos;
    s.last = last;
    frames
}

unsafe extern "C" fn input_poll() {
    let mut s = shared();
    let (dx, dy) = std::mem::take(&mut s.mouse_acc);
    s.mouse_frame = (dx.clamp(-32767, 32767) as i16, dy.clamp(-32767, 32767) as i16);
}

unsafe extern "C" fn input_state(port: u32, device: u32, _index: u32, id: u32) -> i16 {
    let s = shared();
    match (port, device) {
        (0, DEVICE_KEYBOARD) => s.keys.contains(&id) as i16,
        (0, DEVICE_MOUSE) => match id {
            MOUSE_X => s.mouse_frame.0,
            MOUSE_Y => s.mouse_frame.1,
            MOUSE_LEFT => (s.buttons & 1 != 0) as i16,
            MOUSE_RIGHT => (s.buttons & 2 != 0) as i16,
            MOUSE_MIDDLE => (s.buttons & 4 != 0) as i16,
            MOUSE_WHEELUP => (s.wheel < 0) as i16,
            MOUSE_WHEELDOWN => (s.wheel > 0) as i16,
            _ => 0,
        },
        (0, DEVICE_JOYPAD) => {
            if id == JOYPAD_MASK {
                s.joy as i16
            } else {
                ((s.joy >> id) & 1) as i16
            }
        }
        _ => 0,
    }
}

// ------------------------------------------------------------------ sound

struct Sound {
    rate: u32,
    volume: Arc<Mutex<f32>>,
    _stream: Option<cpal::Stream>,
}

fn open_sound(queue: Arc<Mutex<VecDeque<f32>>>) -> Sound {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let volume = Arc::new(Mutex::new(0.8f32));
    let mut s = Sound { rate: 48_000, volume: volume.clone(), _stream: None };
    let Some(dev) = cpal::default_host().default_output_device() else { return s };
    let Ok(cfg) = dev.default_output_config() else { return s };
    let mut config: cpal::StreamConfig = cfg.config();
    config.channels = config.channels.clamp(1, 2);
    let channels = config.channels as usize;
    s.rate = config.sample_rate;
    let stream = dev.build_output_stream(
        config,
        move |out: &mut [f32], _| {
            let vol = *volume.lock().unwrap();
            let mut q = queue.lock().unwrap();
            for frame in out.chunks_mut(channels) {
                let (l, r) = match (q.pop_front(), q.pop_front()) {
                    (Some(l), Some(r)) => (l, r),
                    _ => (0.0, 0.0),
                };
                if channels == 1 {
                    frame[0] = (l + r) * 0.5 * vol;
                } else {
                    frame[0] = l * vol;
                    frame[1] = r * vol;
                }
            }
        },
        |e| eprintln!("rad-dos sound: {e}"),
        None,
    );
    if let Ok(st) = stream {
        if st.play().is_ok() {
            s._stream = Some(st);
        }
    }
    s
}

// ------------------------------------------------------------------ the host

/// Keys typed by the host (`type`): one key press per couple of frames.
enum Typed {
    Down(u32, bool),
    Up(u32, bool),
}

/// A typed character's key and whether it needs shift (US layout).
fn key_for(c: char) -> Option<(u32, bool)> {
    let shifted = ")!@#$%^&*(";
    Some(match c {
        'a'..='z' | '0'..='9' => (c as u32, false),
        'A'..='Z' => (c.to_ascii_lowercase() as u32, true),
        '\n' | '\r' => (13, false),
        ' ' => (32, false),
        '-' | '=' | '[' | ']' | '\\' | ';' | '\'' | '`' | ',' | '.' | '/' => (c as u32, false),
        '_' => (45, true),
        '+' => (61, true),
        '{' => (91, true),
        '}' => (93, true),
        '|' => (92, true),
        ':' => (59, true),
        '"' => (39, true),
        '~' => (96, true),
        '<' => (44, true),
        '>' => (46, true),
        '?' => (47, true),
        _ => match shifted.find(c) {
            Some(d) => (b'0' as u32 + d as u32, true),
            None => return None,
        },
    })
}

struct Host {
    core: Core,
    sound: Sound,
    paused: bool,
    typing: VecDeque<Typed>,
    type_wait: u32,
    quit: bool,
    disk_paths: Vec<CString>,
}

impl Host {
    fn key(&mut self, code: u32, down: bool) {
        let cb = {
            let mut s = shared();
            if down {
                s.keys.insert(code);
            } else {
                s.keys.remove(&code);
            }
            s.keyboard
        };
        if let Some(cb) = cb {
            unsafe { cb(down, code, 0, 0) };
        }
    }

    fn command(&mut self, v: Value) {
        let n = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
        match v.get("cmd").and_then(Value::as_str).unwrap_or("") {
            "key" => self.key(n("code") as u32, v.get("down").and_then(Value::as_bool).unwrap_or(false)),
            "release_all" => {
                let held: Vec<u32> = shared().keys.iter().copied().collect();
                for k in held {
                    self.key(k, false);
                }
                let mut s = shared();
                s.buttons = 0;
                s.joy = 0;
            }
            "mouse" => {
                let mut s = shared();
                s.mouse_acc.0 += n("dx") as i32;
                s.mouse_acc.1 += n("dy") as i32;
                s.buttons = n("buttons") as u8;
                s.wheel = n("wheel").signum() as i8;
            }
            "joy" => shared().joy = n("bits") as u16,
            "type" => {
                for c in v.get("text").and_then(Value::as_str).unwrap_or("").chars() {
                    if let Some((k, shift)) = key_for(c) {
                        self.typing.push_back(Typed::Down(k, shift));
                        self.typing.push_back(Typed::Up(k, shift));
                    }
                }
            }
            "pause" => self.paused = v.get("on").and_then(Value::as_bool).unwrap_or(true),
            "reset" => unsafe { (self.core.reset)() },
            "volume" => *self.sound.volume.lock().unwrap() = v.get("value").and_then(Value::as_f64).unwrap_or(0.8).clamp(0.0, 1.0) as f32,
            "option" => {
                let key = v.get("key").and_then(Value::as_str).unwrap_or("").to_string();
                let value = v.get("value").and_then(Value::as_str).unwrap_or("");
                let mut s = shared();
                s.options.insert(key, CString::new(value).unwrap_or_default());
                s.options_changed = true;
            }
            "insert" => {
                // A floppy (A:) or CD (D:) image, through the core's disk control.
                let path = v.get("path").and_then(Value::as_str).unwrap_or("");
                match self.insert(path) {
                    Ok(()) => event(json!({"event": "inserted", "path": path})),
                    Err(e) => event(json!({"event": "error", "message": e})),
                }
            }
            "eject" => {
                if let Some(d) = shared().disk {
                    unsafe { (d.set_eject_state)(true) };
                }
            }
            "quit" => self.quit = true,
            _ => {}
        }
    }

    fn insert(&mut self, path: &str) -> Result<(), String> {
        let d = shared().disk.ok_or("this core can't change disks")?;
        let c = CString::new(path).map_err(|_| "bad path")?;
        unsafe {
            (d.set_eject_state)(true);
            if !(d.add_image_index)() {
                return Err("couldn't add a disk".into());
            }
            let index = (d.get_num_images)().saturating_sub(1);
            let info = GameInfo { path: c.as_ptr(), data: std::ptr::null(), size: 0, meta: std::ptr::null() };
            if !(d.replace_image_index)(index, &info) {
                return Err(format!("couldn't read {path}"));
            }
            (d.set_image_index)(index);
            (d.set_eject_state)(false);
        }
        self.disk_paths.push(c); // the core keeps the pointer
        Ok(())
    }

    fn step_typing(&mut self) {
        if self.type_wait > 0 {
            self.type_wait -= 1;
            return;
        }
        match self.typing.pop_front() {
            Some(Typed::Down(k, shift)) => {
                if shift {
                    self.key(KEY_LSHIFT, true);
                }
                self.key(k, true);
                self.type_wait = 1;
            }
            Some(Typed::Up(k, shift)) => {
                self.key(k, false);
                if shift {
                    self.key(KEY_LSHIFT, false);
                }
                self.type_wait = 1;
            }
            None => {}
        }
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(core_path), Some(hdd)) = (arg(&args, "--core"), arg(&args, "--hdd")) else {
        eprintln!("usage: rad-dos --core <dosbox_pure_libretro> --hdd <folder> [--system <folder>] [--save <folder>] [--options <json>]");
        std::process::exit(2);
    };
    let system = arg(&args, "--system").unwrap_or_else(|| ".".into());
    let save = arg(&args, "--save").unwrap_or_else(|| system.clone());
    let options: serde_json::Map<String, Value> = arg(&args, "--options").and_then(|o| serde_json::from_str(&o).ok()).unwrap_or_default();
    // RAD's C: is written directly (see dos/patches/direct-c.patch).
    std::env::set_var("RAD_DOS_DIRECT_C", "1");

    let queue = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(48_000)));
    let sound = open_sound(queue.clone());
    let mut opts = HashMap::new();
    for (k, v) in &options {
        if let Some(v) = v.as_str() {
            opts.insert(k.clone(), CString::new(v).unwrap_or_default());
        }
    }
    // DOSBox makes sound at the device's own rate when it can (no resampling).
    if ["48000", "44100", "32000", "22050"].contains(&sound.rate.to_string().as_str()) {
        opts.entry("dosbox_pure_audiorate".into()).or_insert_with(|| CString::new(sound.rate.to_string()).unwrap_or_default());
    }
    let _ = SHARED.set(Mutex::new(Shared {
        frame: Vec::new(),
        width: 0,
        height: 0,
        frame_new: false,
        keys: HashSet::new(),
        mouse_acc: (0, 0),
        mouse_frame: (0, 0),
        buttons: 0,
        wheel: 0,
        joy: 0,
        options: opts,
        options_changed: false,
        system_dir: CString::new(system).unwrap_or_default(),
        save_dir: CString::new(save).unwrap_or_default(),
        keyboard: None,
        disk: None,
        av: None,
        av_changed: false,
        messages: Vec::new(),
        shutdown: false,
        audio: queue.clone(),
        resample: (1.0, 0.0),
        last: (0.0, 0.0),
    }));

    let core = match unsafe { Core::load(&core_path) } {
        Ok(c) => c,
        Err(e) => {
            event(json!({"event": "error", "message": format!("couldn't load the DOS core: {e}")}));
            std::process::exit(1);
        }
    };
    let hdd_c = CString::new(hdd.clone()).unwrap_or_default();
    let mut av = SystemAvInfo::default();
    unsafe {
        (core.set_environment)(environment);
        (core.set_video_refresh)(video_refresh);
        (core.set_audio_sample)(audio_sample);
        (core.set_audio_sample_batch)(audio_sample_batch);
        (core.set_input_poll)(input_poll);
        (core.set_input_state)(input_state);
        (core.init)();
        let info = GameInfo { path: hdd_c.as_ptr(), data: std::ptr::null(), size: 0, meta: std::ptr::null() };
        if !(core.load_game)(&info) {
            event(json!({"event": "error", "message": format!("DOSBox couldn't start with {hdd} as drive C:")}));
            std::process::exit(1);
        }
        (core.set_controller_port_device)(0, DEVICE_JOYPAD);
        (core.get_system_av_info)(&mut av);
    }
    {
        let mut s = shared();
        s.av = Some(av);
        s.resample = (av.timing.sample_rate / sound.rate as f64, 0.0);
    }
    event(json!({"event": "ready", "fps": av.timing.fps, "sample_rate": av.timing.sample_rate, "width": av.geometry.base_width, "height": av.geometry.base_height}));

    // Commands from the app, read on their own thread.
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });

    let mut host = Host { core, sound, paused: false, typing: VecDeque::new(), type_wait: 0, quit: false, disk_paths: Vec::new() };
    run(&mut host, rx, &hdd_c);
}

fn run(host: &mut Host, rx: Receiver<Value>, _hdd: &CString) {
    let mut fps = shared().av.map(|a| a.timing.fps).filter(|f| *f > 1.0).unwrap_or(60.0);
    let mut next = Instant::now();
    let mut last_sent: Vec<u32> = Vec::new();
    let mut last_size = (0, 0);
    let mut buf = Vec::new();
    loop {
        loop {
            match rx.try_recv() {
                Ok(v) => host.command(v),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    host.quit = true;
                    break;
                }
            }
        }
        if host.quit {
            break;
        }
        if host.paused {
            std::thread::sleep(Duration::from_millis(30));
            next = Instant::now();
            continue;
        }
        host.step_typing();
        unsafe { (host.core.run)() };

        // Messages, a changed picture size or speed, a shutdown.
        let (msgs, av, shutdown) = {
            let mut s = shared();
            let av = std::mem::take(&mut s.av_changed).then_some(s.av).flatten();
            (std::mem::take(&mut s.messages), av, s.shutdown)
        };
        for m in msgs {
            event(json!({"event": "message", "text": m}));
        }
        if let Some(av) = av {
            if av.timing.fps > 1.0 {
                fps = av.timing.fps;
            }
            let rate = host.sound.rate as f64;
            shared().resample.0 = av.timing.sample_rate / rate;
            event(json!({"event": "geometry", "fps": av.timing.fps, "width": av.geometry.base_width, "height": av.geometry.base_height}));
        }
        if shutdown {
            break;
        }

        // The picture, when it changed.
        {
            let mut s = shared();
            if s.frame_new {
                s.frame_new = false;
                let size = (s.width, s.height);
                if size != last_size || s.frame != last_sent {
                    encode(&s.frame, s.width, s.height, &mut buf);
                    last_sent.clone_from(&s.frame);
                    last_size = size;
                    drop(s);
                    send(2, &buf);
                }
            }
        }

        // Keep the sound queue short (latency) and keep time with the machine's frame rate.
        {
            let q = shared().audio.clone();
            let mut q = q.lock().unwrap();
            let max = (host.sound.rate as usize * 2) * 15 / 100;
            if q.len() > max {
                let drop = q.len() - max;
                q.drain(..drop);
            }
        }
        next += Duration::from_secs_f64(1.0 / fps);
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_millis(250) {
            next = now;
        }
    }
    unsafe {
        (host.core.unload_game)();
        (host.core.deinit)();
    }
    event(json!({"event": "exit"}));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_indexed_when_they_can_be() {
        let frame: Vec<u32> = (0..64u32).map(|i| if i % 2 == 0 { 0x00ff0000 } else { 0x000000ff }).collect();
        let mut out = Vec::new();
        encode(&frame, 8, 8, &mut out);
        assert_eq!(out[8], 1);
        assert_eq!(&out[12..18], &[255, 0, 0, 0, 0, 255]);
        assert_eq!(out.len(), 12 + 768 + 64);
        assert_eq!(&out[12 + 768..12 + 768 + 4], &[0, 1, 0, 1]);
        let many: Vec<u32> = (0..400u32).collect();
        encode(&many, 20, 20, &mut out);
        assert_eq!(out[8], 4);
        assert_eq!(out.len(), 12 + 400 * 4);
        assert_eq!(&out[12 + 4 * 300..12 + 4 * 300 + 4], &[0, 1, 44, 255]);
    }

    #[test]
    fn types_text() {
        assert_eq!(key_for('d'), Some((100, false)));
        assert_eq!(key_for('D'), Some((100, true)));
        assert_eq!(key_for(':'), Some((59, true)));
        assert_eq!(key_for('*'), Some((56, true)));
        assert_eq!(key_for('\n'), Some((13, false)));
    }
}

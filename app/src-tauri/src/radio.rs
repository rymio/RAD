//! Radio (RTL-SDR): the owner's dongle as a spectrum and waterfall, a scanner that lists what's
//! transmitting, an FM / airband receiver and a local aircraft radar from ADS-B.
//!
//! The dongle is driven through `rtl_tcp` (from the owner's rtl-sdr install: librtlsdr is GPL, so
//! it isn't shipped): it streams 8-bit I/Q over a local socket and takes commands (frequency,
//! sample rate, gain) on the same socket, so retuning is instant. One thread owns the socket and
//! does everything (rad-sdr does the signal processing); what it finds goes to the UI on a
//! channel, as JSON. The sound goes straight to the sound card (cpal), as the ZX view's does.

use crate::client::{CmdError, CmdResult};
use base64::Engine as _;
use rad_sdr::{adsb, burst, dsp, scan, Complex32 as C};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, State};

const RATE: f64 = 2_048_000.0;
const FFT: usize = 2048;
const ROW_BINS: usize = 1024;

#[derive(Default)]
pub struct Radio {
    run: Mutex<Option<Running>>,
}

struct Running {
    tx: mpsc::Sender<Cmd>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Radio {
    pub fn stop(&self) {
        if let Some(mut r) = self.run.lock().unwrap().take() {
            r.stop.store(true, Ordering::SeqCst);
            let _ = r.tx.send(Cmd::Quit);
            if let Some(j) = r.join.take() {
                let _ = j.join();
            }
        }
    }
}

/// What the UI asks of the radio.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    /// Shows the ~2 MHz around `center` (and stops scanning / ADS-B).
    Spectrum { center: f64 },
    /// Listens to `freq` (moving the view if it's outside it).
    Tune { freq: f64, demod: dsp::Demod },
    Listen { on: bool },
    /// Sweeps `from`–`to` Hz over and over, listing what's transmitting.
    Scan {
        from: f64,
        to: f64,
        #[serde(default = "threshold")]
        threshold: f32,
    },
    /// The aircraft radar: 1090 MHz.
    Adsb,
    /// Watches ~2 MHz around `center` for short transmissions (ISM, LoRa, remotes, RC links) and
    /// identifies each.
    Bursts { center: f64 },
    /// Tuner gain in dB; `None` = automatic.
    Gain { db: Option<f32> },
    Ppm { ppm: i32 },
    /// Squelch in dBFS; `None` = open.
    Squelch { db: Option<f32> },
    Volume { v: f32 },
    /// The receiver's place, for distances (and positions from a single message).
    Location { lat: Option<f64>, lon: Option<f64> },
    /// Waterfall rows a second.
    Rows { rate: f32 },
    ClearSignals,
    #[serde(skip)]
    Quit,
}

fn threshold() -> f32 {
    10.0
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

fn root<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("radio")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// rtl_tcp: on the PATH, or where rtl-sdr installs usually put it (a GUI app doesn't get the
/// shell's PATH).
fn find_rtl_tcp() -> Option<PathBuf> {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let exe = if cfg!(windows) { "rtl_tcp.exe" } else { "rtl_tcp" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for d in ["radioconda/bin", "miniforge3/bin", "miniconda3/bin", "anaconda3/bin", ".local/bin"] {
        dirs.push(home.join(d));
    }
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"] {
        dirs.push(d.into());
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

fn tuner_name(t: u32) -> &'static str {
    ["unknown", "E4000", "FC0012", "FC0013", "FC2580", "R820T", "R828D"].get(t as usize).copied().unwrap_or("unknown")
}

/// The gains each tuner offers, in dB.
fn tuner_gains(t: u32) -> Vec<f32> {
    match t {
        5 | 6 => vec![0.0, 0.9, 1.4, 2.7, 3.7, 7.7, 8.7, 12.5, 14.4, 15.7, 16.6, 19.7, 20.7, 22.9, 25.4, 28.0, 29.7, 32.8, 33.8, 36.4, 37.2, 38.6, 40.2, 42.1, 43.4, 43.9, 44.5, 48.0, 49.6],
        1 => vec![-1.0, 1.5, 4.0, 6.5, 9.0, 11.5, 14.0, 16.5, 19.0, 21.5, 24.0, 29.0, 34.0, 42.0, 43.0, 45.0, 47.0, 49.0],
        _ => (0..=25).map(|g| g as f32 * 2.0).collect(),
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn radio_status(r: State<'_, Radio>) -> Value {
    json!({ "rtl_tcp": find_rtl_tcp().map(|p| p.to_string_lossy().to_string()), "running": r.run.lock().unwrap().is_some() })
}

/// Starts the radio: rtl_tcp, then the thread that owns the dongle. Events come on `on_event`.
#[tauri::command]
pub fn radio_start(r: State<'_, Radio>, on_event: Channel<Value>) -> CmdResult<()> {
    r.stop();
    let run = launch(on_event)?;
    *r.run.lock().unwrap() = Some(run);
    Ok(())
}

fn launch(on_event: Channel<Value>) -> CmdResult<Running> {
    let path = find_rtl_tcp().ok_or_else(|| CmdError::new("setup", "rtl_tcp wasn't found. Install rtl-sdr (e.g. brew install librtlsdr, or radioconda) so RAD can use the dongle."))?;
    let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).map_err(io)?;
    let mut child = Command::new(&path)
        .args(["-a", "127.0.0.1", "-p", &port.to_string(), "-s", &(RATE as u32).to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CmdError::new("setup", format!("Couldn't start rtl_tcp: {e}")))?;
    // rtl_tcp says on stderr whether it found and opened the dongle.
    let (ltx, lrx) = mpsc::channel::<String>();
    for out in [child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>)].into_iter().flatten() {
        let ltx = ltx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = ltx.send(line);
            }
        });
    }
    // rtl_tcp's "listening" goes to its (buffered) stdout, so it may never show: try to connect
    // once it has found the dongle, and watch for the errors meanwhile.
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut said = String::new();
    let mut found = false;
    let sock = loop {
        while let Ok(line) = lrx.recv_timeout(Duration::from_millis(100)) {
            said.push_str(&line);
            said.push('\n');
            let l = line.to_lowercase();
            if l.contains("tuned to") || l.contains("listening") {
                found = true;
            }
            if l.contains("no supported devices") || l.contains("no matching devices") {
                kill(&mut child);
                return Err(CmdError::new("device", "No RTL-SDR dongle found. Plug it in (and give it a moment) and try again."));
            }
            if l.contains("failed to open") || l.contains("usb_claim_interface") {
                kill(&mut child);
                return Err(CmdError::new("busy", "The dongle is busy: another program (SDR++, GQRX, rtl_fm, dump1090…) has it open. Close it and try again."));
            }
        }
        if let Ok(Some(st)) = child.try_wait() {
            return Err(CmdError::new("device", format!("rtl_tcp stopped ({st}): {}", said.trim())));
        }
        if found {
            if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
                break s;
            }
        }
        if Instant::now() > deadline {
            kill(&mut child);
            return Err(CmdError::new("device", format!("rtl_tcp didn't start: {}", said.trim())));
        }
    };
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let join = std::thread::Builder::new()
        .name("radio".into())
        .spawn(move || {
            let mut child = child;
            if let Err(e) = Engine::new(sock, on_event.clone(), rx, st).and_then(|mut e| e.run()) {
                let _ = on_event.send(json!({ "type": "error", "message": e }));
            }
            kill(&mut child);
            let _ = on_event.send(json!({ "type": "stopped" }));
        })
        .map_err(io)?;
    Ok(Running { tx, stop, join: Some(join) })
}

fn kill(c: &mut Child) {
    // rtl_tcp ignores SIGTERM while streaming.
    let _ = c.kill();
    let _ = c.wait();
}

#[tauri::command]
pub fn radio_stop(r: State<'_, Radio>) {
    r.stop();
}

#[tauri::command]
pub fn radio_command(r: State<'_, Radio>, cmd: Cmd) -> CmdResult<()> {
    let g = r.run.lock().unwrap();
    let run = g.as_ref().ok_or_else(|| CmdError::new("stopped", "The radio isn't on"))?;
    run.tx.send(cmd).map_err(|_| CmdError::new("stopped", "The radio has stopped"))
}

/// The radio's settings (frequency, gain, bookmarks, the receiver's place…), kept on this computer.
#[tauri::command]
pub fn radio_settings_get(app: AppHandle) -> Value {
    std::fs::read_to_string(root(&app).join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

#[tauri::command]
pub fn radio_settings_set(app: AppHandle, settings: Value) -> CmdResult<()> {
    let d = root(&app);
    std::fs::create_dir_all(&d).map_err(io)?;
    std::fs::write(d.join("settings.json"), serde_json::to_vec_pretty(&settings).map_err(io)?).map_err(io)
}

/// Saves an export (packets, aircraft, signals) in Radio's folder; returns its path.
#[tauri::command]
pub fn radio_export(app: AppHandle, name: String, text: String) -> CmdResult<String> {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    if safe.is_empty() || safe.starts_with('.') {
        return Err(CmdError::new("io", "Bad file name"));
    }
    let d = root(&app).join("exports");
    std::fs::create_dir_all(&d).map_err(io)?;
    let p = d.join(safe);
    std::fs::write(&p, text).map_err(io)?;
    Ok(p.to_string_lossy().into())
}

// ---------------------------------------------------------------- the engine

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Spectrum,
    Scan,
    Adsb,
    Bursts,
}

struct Engine {
    sock: TcpStream,
    w: TcpStream,
    ch: Channel<Value>,
    rx: mpsc::Receiver<Cmd>,
    stop: Arc<AtomicBool>,
    mode: Mode,
    rate: f64,
    center: f64,
    direct: u32,
    gain: Option<f32>,
    gains: Vec<f32>,
    /// Bytes to throw away after retuning (still from the old frequency).
    skip: usize,
    // Listening.
    listen: bool,
    freq: f64,
    demod: dsp::Demod,
    squelch: Option<f32>,
    volume: f32,
    rx_dsp: Option<dsp::Receiver>,
    sound: Option<crate::zx::Sound>,
    // Waterfall.
    spec: dsp::Spectrum,
    row_every: Duration,
    last_row: Instant,
    last_status: Instant,
    // Scanner.
    sweep: Option<scan::Sweep>,
    step: usize,
    threshold: f32,
    book: scan::Book,
    // ADS-B.
    demod_ms: adsb::Demod,
    tracker: adsb::Tracker,
    packets: Vec<adsb::Packet>,
    last_packets: Instant,
    last_aircraft: Instant,
    frames_at: (Instant, u64),
    /// Bytes from the dongle since `bytes_at`, for the sample rate actually kept up with.
    bytes: u64,
    bytes_at: Instant,
    msps: f64,
    emergencies: std::collections::HashSet<String>,
    // Sub-GHz bursts.
    detector: Option<burst::Detector>,
    bursts: Vec<burst::Burst>,
    last_bursts: Instant,
}

impl Engine {
    fn new(sock: TcpStream, ch: Channel<Value>, rx: mpsc::Receiver<Cmd>, stop: Arc<AtomicBool>) -> Result<Self, String> {
        sock.set_read_timeout(Some(Duration::from_millis(500))).map_err(|e| e.to_string())?;
        sock.set_nodelay(true).ok();
        let w = sock.try_clone().map_err(|e| e.to_string())?;
        let now = Instant::now();
        Ok(Engine {
            sock,
            w,
            ch,
            rx,
            stop,
            mode: Mode::Spectrum,
            rate: RATE,
            center: 100e6,
            direct: 99,
            gain: None,
            gains: vec![],
            skip: 0,
            listen: false,
            freq: 100e6,
            demod: dsp::Demod::Wfm,
            squelch: None,
            volume: 0.8,
            rx_dsp: None,
            sound: None,
            spec: dsp::Spectrum::new(FFT),
            row_every: Duration::from_millis(66),
            last_row: now,
            last_status: now,
            sweep: None,
            step: 0,
            threshold: 10.0,
            book: scan::Book::default(),
            demod_ms: adsb::Demod::default(),
            tracker: adsb::Tracker::default(),
            packets: vec![],
            last_packets: now,
            last_aircraft: now,
            frames_at: (now, 0),
            bytes: 0,
            bytes_at: now,
            msps: 0.0,
            emergencies: Default::default(),
            detector: None,
            bursts: vec![],
            last_bursts: now,
        })
    }

    fn send(&self, v: Value) -> Result<(), String> {
        self.ch.send(v).map_err(|_| "The window closed".to_string())
    }

    fn cmd(&mut self, c: u8, v: u32) -> Result<(), String> {
        let mut b = [0u8; 5];
        b[0] = c;
        b[1..].copy_from_slice(&v.to_be_bytes());
        self.w.write_all(&b).map_err(|e| format!("Lost the dongle: {e}"))
    }

    fn set_rate(&mut self, rate: f64) -> Result<(), String> {
        if (self.rate - rate).abs() > 1.0 {
            self.rate = rate;
            self.cmd(0x02, rate as u32)?;
            self.skip = (rate * 2.0 * 0.1) as usize;
        }
        Ok(())
    }

    fn set_center(&mut self, f: f64) -> Result<(), String> {
        let f = f.clamp(0.5e6, 1_766e6);
        // Below 24 MHz the tuner can't go: the RTL2832's own ADC (direct sampling, Q branch)
        // takes over, on dongles wired for it.
        let direct = if f < 24e6 { 2 } else { 0 };
        if direct != self.direct {
            self.direct = direct;
            self.cmd(0x09, direct)?;
        }
        self.center = f;
        self.cmd(0x01, f.round() as u32)?;
        self.skip = (self.rate * 2.0 * 0.05) as usize;
        self.spec.reset();
        Ok(())
    }

    fn apply_gain(&mut self) -> Result<(), String> {
        // ADS-B wants the most gain the tuner has, unless the owner set one.
        let g = if self.mode == Mode::Adsb { self.gain.or(self.gains.last().copied()) } else { self.gain };
        match g {
            None => self.cmd(0x03, 0),
            Some(db) => {
                self.cmd(0x03, 1)?;
                self.cmd(0x04, (db * 10.0).round() as i32 as u32)
            }
        }
    }

    fn make_receiver(&mut self) {
        if !self.listen || self.mode != Mode::Spectrum {
            self.rx_dsp = None;
            self.sound = None;
            return;
        }
        if self.sound.is_none() {
            let s = crate::zx::open_sound();
            *s.volume.lock().unwrap() = self.volume;
            self.sound = Some(s);
        }
        let out = self.sound.as_ref().map(|s| s.rate as f64).unwrap_or(48_000.0);
        let mut r = dsp::Receiver::new(self.demod, self.freq - self.center, self.rate, out);
        r.squelch = self.squelch;
        self.rx_dsp = Some(r);
    }

    fn handle(&mut self, c: Cmd) -> Result<bool, String> {
        match c {
            Cmd::Quit => return Ok(false),
            Cmd::Spectrum { center } => {
                self.mode = Mode::Spectrum;
                self.sweep = None;
                self.set_rate(RATE)?;
                self.apply_gain()?;
                self.set_center(center)?;
                self.make_receiver();
            }
            Cmd::Tune { freq, demod } => {
                let was = self.mode;
                self.mode = Mode::Spectrum;
                self.sweep = None;
                self.freq = freq;
                self.demod = demod;
                self.set_rate(RATE)?;
                if was != Mode::Spectrum {
                    self.apply_gain()?;
                }
                // Stays in view, away from the DC spike in the middle; else the view moves so
                // the station sits a quarter-MHz off centre.
                let off = freq - self.center;
                if was != Mode::Spectrum || off.abs() > self.rate * 0.42 || off.abs() < 60e3 {
                    self.set_center(freq + 250e3)?;
                }
                self.make_receiver();
            }
            Cmd::Listen { on } => {
                self.listen = on;
                self.make_receiver();
            }
            Cmd::Scan { from, to, threshold } => {
                let (from, to) = (from.min(to).max(24e6), to.max(from).min(1_766e6));
                self.mode = Mode::Scan;
                self.threshold = threshold;
                self.set_rate(RATE)?;
                self.apply_gain()?;
                let same = self.sweep.as_ref().map(|s| s.from == from && s.to == to).unwrap_or(false);
                if !same {
                    self.book.clear();
                }
                self.sweep = Some(scan::Sweep::new(from, to, RATE, FFT));
                self.step = 0;
                let c = self.sweep.as_ref().unwrap().center(0);
                self.set_center(c)?;
                self.make_receiver();
            }
            Cmd::Adsb => {
                self.mode = Mode::Adsb;
                self.sweep = None;
                self.set_rate(adsb::RATE as f64)?;
                self.apply_gain()?;
                self.set_center(adsb::FREQ as f64)?;
                self.make_receiver();
            }
            Cmd::Bursts { center } => {
                self.mode = Mode::Bursts;
                self.sweep = None;
                self.set_rate(RATE)?;
                self.apply_gain()?;
                self.set_center(center)?;
                self.detector = Some(burst::Detector::new(RATE, self.center));
                self.make_receiver();
            }
            Cmd::Gain { db } => {
                self.gain = db;
                self.apply_gain()?;
            }
            Cmd::Ppm { ppm } => self.cmd(0x05, ppm as u32)?,
            Cmd::Squelch { db } => {
                self.squelch = db;
                if let Some(r) = self.rx_dsp.as_mut() {
                    r.squelch = db;
                }
            }
            Cmd::Volume { v } => {
                self.volume = v.clamp(0.0, 2.0);
                if let Some(s) = &self.sound {
                    *s.volume.lock().unwrap() = self.volume;
                }
            }
            Cmd::Location { lat, lon } => self.tracker.receiver = lat.zip(lon),
            Cmd::Rows { rate } => self.row_every = Duration::from_secs_f32(1.0 / rate.clamp(1.0, 40.0)),
            Cmd::ClearSignals => {
                self.book.clear();
                self.send(json!({ "type": "signals", "list": [], "sweeps": 0 }))?;
            }
        }
        Ok(true)
    }

    fn run(&mut self) -> Result<(), String> {
        let mut hdr = [0u8; 12];
        self.sock.read_exact(&mut hdr).map_err(|e| format!("rtl_tcp said nothing: {e}"))?;
        if &hdr[..4] != b"RTL0" {
            return Err("That isn't rtl_tcp".into());
        }
        let tuner = u32::from_be_bytes(hdr[4..8].try_into().unwrap());
        self.gains = tuner_gains(tuner);
        self.cmd(0x0a, 0)?; // no offset tuning
        self.cmd(0x08, 0)?; // RTL AGC off
        self.rate = 0.0;
        self.set_rate(RATE)?;
        self.set_center(self.center)?;
        self.apply_gain()?;
        self.send(json!({ "type": "ready", "tuner": tuner_name(tuner), "gains": self.gains }))?;

        let mut buf = vec![0u8; 65536];
        let mut have: Vec<u8> = Vec::with_capacity(1 << 17);
        let mut quiet = 0;
        let mut x: Vec<C> = Vec::new();
        let mut audio: Vec<f32> = Vec::new();
        let mut frames = Vec::new();
        while !self.stop.load(Ordering::SeqCst) {
            while let Ok(c) = self.rx.try_recv() {
                if !self.handle(c)? {
                    return Ok(());
                }
            }
            let n = match self.sock.read(&mut buf) {
                Ok(0) => return Err("The dongle stopped sending (unplugged?)".into()),
                Ok(n) => n,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    quiet += 1;
                    if quiet > 8 {
                        return Err("No samples from the dongle for 4 seconds (unplugged?)".into());
                    }
                    continue;
                }
                Err(e) => return Err(format!("Lost the dongle: {e}")),
            };
            quiet = 0;
            self.bytes += n as u64;
            let mut data = &buf[..n];
            if self.skip > 0 {
                // What's waiting is from before the retune too; dropping an even number of bytes
                // in all keeps I and Q in step.
                if have.len() % 2 == 1 {
                    self.skip += 1;
                }
                have.clear();
                let k = self.skip.min(data.len());
                self.skip -= k;
                data = &data[k..];
            }
            have.extend_from_slice(data);
            if have.len() < 32768 {
                continue;
            }
            let take = have.len() & !1;
            let chunk: Vec<u8> = have.drain(..take).collect();
            match self.mode {
                Mode::Spectrum => {
                    dsp::to_complex(&chunk, &mut x);
                    self.spec.feed(&x, 4);
                    if let Some(r) = self.rx_dsp.as_mut() {
                        audio.clear();
                        r.process(&x, &mut audio);
                        if let Some(s) = &self.sound {
                            let mut q = s.queue.lock().unwrap();
                            let max = s.rate as usize * 2 * 2 / 5;
                            if q.len() > max {
                                let extra = q.len() - max / 2;
                                q.drain(..extra);
                            }
                            for a in &audio {
                                q.push_back(*a);
                                q.push_back(*a);
                            }
                        }
                    }
                    self.rows()?;
                }
                Mode::Scan => {
                    dsp::to_complex(&chunk, &mut x);
                    self.spec.feed(&x, 1);
                    if self.spec.blocks() >= 24 {
                        self.scan_step()?;
                    }
                }
                Mode::Bursts => {
                    dsp::to_complex(&chunk, &mut x);
                    self.spec.feed(&x, 4);
                    if let Some(d) = self.detector.as_mut() {
                        let found = d.feed(&x, now_ms());
                        self.bursts.extend(found);
                    }
                    self.rows()?;
                    if self.last_bursts.elapsed() >= Duration::from_millis(250) && !self.bursts.is_empty() {
                        self.last_bursts = Instant::now();
                        let list: Vec<_> = self.bursts.drain(..).collect();
                        self.send(json!({ "type": "bursts", "list": list }))?;
                    }
                }
                Mode::Adsb => {
                    frames.clear();
                    self.demod_ms.feed(&chunk, &mut frames);
                    let now = now_ms();
                    for f in &frames {
                        if let Some(p) = self.tracker.handle(f, now) {
                            self.packets.push(p);
                        }
                    }
                    // A glance at the band now and then.
                    if self.last_row.elapsed() >= Duration::from_millis(250) {
                        dsp::to_complex(&chunk[..chunk.len().min(FFT * 2 * 8)], &mut x);
                        self.spec.feed(&x, 1);
                    }
                    self.rows()?;
                    self.adsb_out()?;
                }
            }
            self.status()?;
        }
        Ok(())
    }

    fn rows(&mut self) -> Result<(), String> {
        let every = if self.mode == Mode::Adsb { Duration::from_millis(250) } else { self.row_every };
        if self.last_row.elapsed() < every || self.spec.blocks() == 0 {
            return Ok(());
        }
        self.last_row = Instant::now();
        let db = self.spec.take();
        let bins = base64::engine::general_purpose::STANDARD.encode(dsp::to_bytes(&dsp::shrink(&db, ROW_BINS)));
        self.send(json!({ "type": "row", "center": self.center, "rate": self.rate, "bins": bins }))
    }

    fn scan_step(&mut self) -> Result<(), String> {
        let db = self.spec.take();
        let center = self.center;
        let Some(sw) = self.sweep.as_mut() else { return Ok(()) };
        sw.add(center, &db);
        self.step += 1;
        let steps = sw.steps();
        if self.step >= steps {
            let hits = sw.signals(self.threshold);
            let wide = dsp::shrink(&sw.db, 2048);
            let bins = base64::engine::general_purpose::STANDARD.encode(dsp::to_bytes(&wide));
            let (from, to) = (sw.from, sw.to);
            self.book.add(&hits, now_ms());
            self.step = 0;
            self.send(json!({ "type": "sweep", "from": from, "to": to, "bins": bins }))?;
            self.send(json!({ "type": "signals", "list": self.book.signals, "sweeps": self.book.sweeps }))?;
        }
        let c = self.sweep.as_ref().unwrap().center(self.step);
        self.set_center(c)?;
        // The tuner settles and the samples already on their way go by.
        self.skip = (self.rate * 2.0 * 0.08) as usize;
        if self.last_status.elapsed() > Duration::from_millis(200) {
            self.send(json!({ "type": "scan", "step": self.step, "steps": steps, "center": c }))?;
        }
        Ok(())
    }

    fn adsb_out(&mut self) -> Result<(), String> {
        if self.last_packets.elapsed() >= Duration::from_millis(250) && !self.packets.is_empty() {
            self.last_packets = Instant::now();
            let list: Vec<_> = self.packets.drain(..).collect();
            let skip = list.len().saturating_sub(500);
            self.send(json!({ "type": "packets", "list": &list[skip..] }))?;
        }
        if self.last_aircraft.elapsed() >= Duration::from_secs(1) {
            self.last_aircraft = Instant::now();
            self.tracker.expire(now_ms());
            let list = self.tracker.list();
            let (t0, f0) = self.frames_at;
            let rate = (self.tracker.frames - f0) as f64 / t0.elapsed().as_secs_f64();
            self.frames_at = (Instant::now(), self.tracker.frames);
            // Emergencies are told once.
            for a in &list {
                let sq = a.squawk.as_deref().unwrap_or("");
                if a.emergency.is_some() || ["7500", "7600", "7700"].contains(&sq) {
                    let key = format!("{}{}{}", a.icao, sq, a.emergency.unwrap_or(""));
                    if self.emergencies.insert(key) {
                        self.send(json!({ "type": "emergency", "aircraft": a }))?;
                    }
                }
            }
            self.send(json!({ "type": "aircraft", "list": list, "frames": self.tracker.frames, "bad": self.tracker.bad, "rate": (rate * 10.0).round() / 10.0 }))?;
        }
        Ok(())
    }

    fn status(&mut self) -> Result<(), String> {
        if self.last_status.elapsed() < Duration::from_millis(200) {
            return Ok(());
        }
        self.last_status = Instant::now();
        let dt = self.bytes_at.elapsed().as_secs_f64();
        if dt >= 1.0 {
            self.msps = self.bytes as f64 / 2.0 / dt / 1e6;
            self.bytes = 0;
            self.bytes_at = Instant::now();
        }
        let (level, open) = self.rx_dsp.as_ref().map(|r| (Some((r.level * 10.0).round() / 10.0), r.open)).unwrap_or((None, false));
        let mode = match self.mode {
            Mode::Spectrum => "spectrum",
            Mode::Scan => "scan",
            Mode::Adsb => "adsb",
            Mode::Bursts => "bursts",
        };
        self.send(json!({ "type": "status", "mode": mode, "center": self.center, "rate": self.rate, "freq": self.freq, "demod": self.demod, "listening": self.rx_dsp.is_some(), "level": level, "open": open, "gain": self.gain, "msps": (self.msps * 1000.0).round() / 1000.0 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::ipc::InvokeResponseBody;

    /// With the owner's dongle plugged in: `cargo test -p rad-app radio -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn with_the_dongle() {
        let got = Arc::new(Mutex::new(Vec::<Value>::new()));
        let g = got.clone();
        let ch = Channel::new(move |b: InvokeResponseBody| {
            if let InvokeResponseBody::Json(s) = b {
                g.lock().unwrap().push(serde_json::from_str(&s).unwrap());
            }
            Ok(())
        });
        let run = launch(ch).expect("rtl_tcp");
        let of = |t: &str| got.lock().unwrap().iter().filter(|v| v["type"] == t).cloned().collect::<Vec<_>>();
        let wait = |s: f32| std::thread::sleep(Duration::from_secs_f32(s));
        wait(1.0);
        assert_eq!(of("ready").len(), 1, "{:?}", of("error"));
        println!("tuner {}", of("ready")[0]["tuner"]);

        // The waterfall.
        run.tx.send(Cmd::Spectrum { center: 100e6 }).unwrap();
        wait(2.0);
        let rows = of("row");
        assert!(rows.len() > 15, "{} rows", rows.len());
        let bins = base64::engine::general_purpose::STANDARD.decode(rows.last().unwrap()["bins"].as_str().unwrap()).unwrap();
        assert_eq!(bins.len(), ROW_BINS);

        // Listening (silently) to FM.
        run.tx.send(Cmd::Volume { v: 0.0 }).unwrap();
        run.tx.send(Cmd::Tune { freq: 98.0e6, demod: dsp::Demod::Wfm }).unwrap();
        run.tx.send(Cmd::Listen { on: true }).unwrap();
        wait(3.0);
        let st = of("status");
        let last = st.last().unwrap();
        println!("listening: {last}");
        assert_eq!(last["listening"], true);

        // The scanner, over the FM band.
        run.tx.send(Cmd::Scan { from: 87.5e6, to: 108e6, threshold: 10.0 }).unwrap();
        wait(8.0);
        let sig = of("signals");
        assert!(!sig.is_empty(), "no sweep finished");
        let list = sig.last().unwrap()["list"].as_array().unwrap().clone();
        println!("{} sweeps; FM: {}", sig.last().unwrap()["sweeps"], list.iter().map(|s| format!("{:.1}", s["freq"].as_f64().unwrap() / 1e6)).collect::<Vec<_>>().join(" "));
        assert!(list.len() >= 3);

        // Sub-GHz: whatever transmits around 433.92 MHz for a while.
        let fixed: Option<f32> = std::env::var("RAD_GAIN").ok().and_then(|g| g.parse().ok());
        run.tx.send(Cmd::Gain { db: fixed }).unwrap();
        run.tx.send(Cmd::Bursts { center: 433.92e6 }).unwrap();
        wait(10.0);
        let bursts: Vec<Value> = of("bursts").iter().flat_map(|b| b["list"].as_array().unwrap().clone()).collect();
        println!("433.92 MHz: {} bursts", bursts.len());
        if let Ok(out) = std::env::var("RAD_BURSTS_JSON") {
            std::fs::write(out, serde_json::to_string(&bursts).unwrap()).unwrap();
        }
        for b in bursts.iter().take(10) {
            println!("  {:.4} MHz {} {:.0} kHz {}µs snr {:.0}: {} | {}", b["freq"].as_f64().unwrap() / 1e6, b["kind"], b["bw"].as_f64().unwrap() / 1e3, b["dur_us"], b["snr"].as_f64().unwrap(), b["modulation"], b["guess"]);
        }

        // The aircraft.
        run.tx.send(Cmd::Adsb).unwrap();
        wait(30.0);
        let packets: usize = of("packets").iter().map(|p| p["list"].as_array().unwrap().len()).sum();
        let ac = of("aircraft");
        let last = ac.last().unwrap();
        println!("ADS-B: {packets} packets, {} frames, {} bad, aircraft: {}", last["frames"], last["bad"], last["list"].as_array().unwrap().iter().map(|a| format!("{} {} {}", a["icao"], a["callsign"], a["alt"])).collect::<Vec<_>>().join("; "));
        for p in of("packets").iter().flat_map(|p| p["list"].as_array().unwrap().clone()).take(8) {
            println!("  {} DF{} {} {}", p["icao"], p["df"], p["hex"], p["info"]);
        }
        let mut run = run;
        run.stop.store(true, Ordering::SeqCst);
        let _ = run.tx.send(Cmd::Quit);
        run.join.take().unwrap().join().unwrap();
        assert!(of("stopped").len() == 1);
    }
}

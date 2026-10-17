//! The ZX Spectrum view: a Spectrum (crates/rad-zx) running on its own thread at 50 frames a
//! second. Its picture goes to the page over a channel (one byte per pixel, colour 0–15, only
//! when it changed); its sound plays through this Mac's audio output; the page sends key and
//! joystick states.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use rad_zx::{Loaded, Model, Spectrum};
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::client::{CmdError, CmdResult};

enum Cmd {
    Attach(Channel<InvokeResponseBody>),
    Keys(u64),
    Joy(u8),
    Load(String, Vec<u8>, Sender<Result<Loaded, String>>),
    SaveState(Sender<Result<Vec<u8>, String>>),
    Model(Model),
    Reset,
    Pause(bool),
    Tape(&'static str),
    FastLoad(bool),
    Volume(f32),
    TapeAudible(bool),
    Quit,
}

#[derive(Default)]
pub struct Zx {
    tx: Mutex<Option<Sender<Cmd>>>,
}

impl Zx {
    fn send(&self, c: Cmd) -> CmdResult<()> {
        let tx = self.tx.lock().unwrap();
        tx.as_ref().ok_or_else(|| CmdError::new("zx", "The Spectrum isn't on"))?.send(c).map_err(|_| CmdError::new("zx", "The Spectrum stopped"))
    }

    pub fn stop(&self) {
        if let Some(tx) = self.tx.lock().unwrap().take() {
            let _ = tx.send(Cmd::Quit);
        }
    }
}

/// Sound: samples waiting for the audio device, shared with its callback.
struct Sound {
    queue: Arc<Mutex<VecDeque<f32>>>,
    volume: Arc<Mutex<f32>>,
    rate: u32,
    channels: usize,
    _stream: Option<cpal::Stream>,
}

fn open_sound() -> Sound {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let queue = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(48_000)));
    let volume = Arc::new(Mutex::new(0.8f32));
    let mut s = Sound { queue: queue.clone(), volume: volume.clone(), rate: 44_100, channels: 2, _stream: None };
    let Some(dev) = cpal::default_host().default_output_device() else { return s };
    let Ok(cfg) = dev.default_output_config() else { return s };
    let mut config: cpal::StreamConfig = cfg.config();
    config.channels = config.channels.clamp(1, 2);
    let channels = config.channels as usize;
    s.rate = config.sample_rate;
    s.channels = channels;
    let (q, v) = (queue, volume);
    let stream = dev.build_output_stream(
        config,
        move |out: &mut [f32], _| {
            let vol = *v.lock().unwrap();
            let mut q = q.lock().unwrap();
            // The queue holds stereo pairs; mono devices get their average.
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
        |e| eprintln!("zx sound: {e}"),
        None,
    );
    if let Ok(st) = stream {
        if st.play().is_ok() {
            s._stream = Some(st);
        }
    }
    s
}

/// The emulator thread: one frame every 20 ms (faster while fast-loading a tape).
fn run(rx: Receiver<Cmd>, model: Model) {
    let mut zx = Spectrum::new(model);
    let sound = open_sound();
    zx.set_sample_rate(sound.rate);
    let (w, h) = zx.size();
    let mut frame = vec![0u8; w * h];
    let mut last = vec![255u8; w * h];
    let mut out: Option<Channel<InvokeResponseBody>> = None;
    let mut paused = false;
    let mut tape_audible = true;
    let mut samples = Vec::new();
    let mut next = Instant::now();
    let mut status_at = Instant::now();
    let mut last_status = Value::Null;
    const FRAME: Duration = Duration::from_micros(19_968);

    'main: loop {
        // Commands from the page.
        loop {
            let c = if paused {
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
            match c {
                Cmd::Attach(ch) => {
                    let _ = ch.send(InvokeResponseBody::Json(json!({"size": [w, h], "palette": rad_zx::PALETTE}).to_string()));
                    last.fill(255); // send the next picture in full
                    last_status = Value::Null;
                    out = Some(ch);
                }
                Cmd::Keys(k) => zx.set_keys(rad_zx::Keys::from_bits_truncate(k)),
                Cmd::Joy(j) => zx.set_joystick(j),
                Cmd::Load(name, data, reply) => {
                    let r = zx.load(&name, &data).map_err(|e| e.to_string());
                    zx.set_sample_rate(sound.rate);
                    let _ = reply.send(r);
                    paused = false;
                }
                Cmd::SaveState(reply) => {
                    let _ = reply.send(zx.save_state().map_err(|e| e.to_string()));
                }
                Cmd::Model(m) => {
                    zx.set_model(m);
                    zx.set_sample_rate(sound.rate);
                }
                Cmd::Reset => zx.reset(),
                Cmd::Pause(p) => paused = p,
                Cmd::Tape(a) => match a {
                    "play" => zx.tape_play(true),
                    "stop" => zx.tape_play(false),
                    "rewind" => zx.tape_rewind(),
                    _ => zx.eject(),
                },
                Cmd::FastLoad(f) => zx.fast_load = f,
                Cmd::Volume(v) => *sound.volume.lock().unwrap() = v.clamp(0.0, 1.0),
                Cmd::TapeAudible(a) => tape_audible = a,
                Cmd::Quit => break 'main,
            }
        }
        if paused {
            next = Instant::now();
            continue;
        }

        if zx.is_turbo() {
            // Fast loading: as many frames as fit in one frame's time, silently.
            let until = Instant::now() + FRAME;
            sound.queue.lock().unwrap().clear();
            while zx.is_turbo() && Instant::now() < until {
                zx.run_frame();
                zx.skip_audio();
            }
        } else {
            zx.run_frame();
            zx.audio(&mut samples, tape_audible);
            let mut q = sound.queue.lock().unwrap();
            // Keep latency low: never more than ~120 ms queued.
            let max = (sound.rate as usize * 2) * 12 / 100;
            if q.len() + samples.len() > max {
                let drop = (q.len() + samples.len() - max).min(q.len());
                q.drain(..drop);
            }
            q.extend(samples.iter().copied());
        }

        // The picture, when it changed.
        zx.render(&mut frame);
        let mut gone = false;
        if let Some(ch) = &out {
            if frame != last {
                gone = ch.send(InvokeResponseBody::Raw(frame.clone())).is_err();
                std::mem::swap(&mut frame, &mut last);
            }
            if status_at.elapsed() > Duration::from_millis(250) {
                status_at = Instant::now();
                let tape = zx.tape_state().map(|(playing, progress, names)| json!({"playing": playing, "progress": progress, "names": names}));
                let st = json!({"status": {"model": if zx.model() == Model::Spectrum48 { "48" } else { "128" }, "turbo": zx.is_turbo(), "tape": tape, "fast_load": zx.fast_load}});
                if st != last_status {
                    let _ = ch.send(InvokeResponseBody::Json(st.to_string()));
                    last_status = st;
                }
            }
        }
        if gone {
            out = None;
        }

        // Keep time with the Spectrum's 50 Hz (and the sound queue from running dry).
        if !zx.is_turbo() {
            next += FRAME;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > Duration::from_millis(200) {
                next = now; // we fell behind (the Mac was busy): don't race to catch up
            }
        } else {
            next = Instant::now();
        }
    }
}

fn model_of(m: &str) -> Model {
    if m == "48" { Model::Spectrum48 } else { Model::Spectrum128 }
}

/// Turns the Spectrum on (if needed) and connects the page: messages are the picture (bytes)
/// or JSON ({size, palette} first, then {status}).
#[tauri::command]
pub fn zx_attach(z: State<'_, Zx>, model: Option<String>, on_frame: Channel<InvokeResponseBody>) -> CmdResult<()> {
    {
        let mut tx = z.tx.lock().unwrap();
        if tx.is_none() {
            let (t, r) = mpsc::channel();
            let m = model_of(model.as_deref().unwrap_or("48"));
            std::thread::Builder::new().name("zx-spectrum".into()).spawn(move || run(r, m)).map_err(|e| CmdError::new("zx", e.to_string()))?;
            *tx = Some(t);
        }
    }
    z.send(Cmd::Attach(on_frame))
}

/// The keys held down (a ZX keyboard map: see rad_zx::Keys).
#[tauri::command]
pub fn zx_keys(z: State<'_, Zx>, keys: u64) -> CmdResult<()> {
    z.send(Cmd::Keys(keys))
}

/// The Kempston joystick: up 1, down 2, left 4, right 8, fire 16.
#[tauri::command]
pub fn zx_joystick(z: State<'_, Zx>, bits: u8) -> CmdResult<()> {
    z.send(Cmd::Joy(bits))
}

/// Loads a TZX/TAP tape (reset + LOAD "") or a Z80/SNA snapshot. `data` is base64.
#[tauri::command]
pub async fn zx_load(z: State<'_, Zx>, name: String, data: String) -> CmdResult<Value> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).map_err(|_| CmdError::new("bad_request", "That file's data is damaged"))?;
    let (tx, rx) = mpsc::channel();
    z.send(Cmd::Load(name, bytes, tx))?;
    let r = tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(10)))
        .await
        .map_err(|e| CmdError::new("zx", e.to_string()))?
        .map_err(|_| CmdError::new("zx", "The Spectrum didn't answer"))?;
    match r.map_err(|e| CmdError::new("zx", e))? {
        Loaded::Tape { names } => Ok(json!({"kind": "tape", "names": names})),
        Loaded::Snapshot => Ok(json!({"kind": "snapshot"})),
    }
}

/// The machine's state as a Z80 snapshot (base64), to keep as a save state.
#[tauri::command]
pub async fn zx_save_state(z: State<'_, Zx>) -> CmdResult<String> {
    let (tx, rx) = mpsc::channel();
    z.send(Cmd::SaveState(tx))?;
    let r = tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(10)))
        .await
        .map_err(|e| CmdError::new("zx", e.to_string()))?
        .map_err(|_| CmdError::new("zx", "The Spectrum didn't answer"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(r.map_err(|e| CmdError::new("zx", e))?))
}

/// reset · pause · resume · model48 · model128 · tape_play · tape_stop · tape_rewind · eject ·
/// fast_on · fast_off · tape_sound_on · tape_sound_off · volume (with `value` 0–1).
#[tauri::command]
pub fn zx_control(z: State<'_, Zx>, action: String, value: Option<f32>) -> CmdResult<()> {
    let c = match action.as_str() {
        "reset" => Cmd::Reset,
        "pause" => Cmd::Pause(true),
        "resume" => Cmd::Pause(false),
        "model48" => Cmd::Model(Model::Spectrum48),
        "model128" => Cmd::Model(Model::Spectrum128),
        "tape_play" => Cmd::Tape("play"),
        "tape_stop" => Cmd::Tape("stop"),
        "tape_rewind" => Cmd::Tape("rewind"),
        "eject" => Cmd::Tape("eject"),
        "fast_on" => Cmd::FastLoad(true),
        "fast_off" => Cmd::FastLoad(false),
        "tape_sound_on" => Cmd::TapeAudible(true),
        "tape_sound_off" => Cmd::TapeAudible(false),
        "volume" => Cmd::Volume(value.unwrap_or(0.8)),
        _ => return Err(CmdError::new("bad_request", "unknown action")),
    };
    z.send(c)
}

/// Switches the Spectrum off.
#[tauri::command]
pub fn zx_power_off(z: State<'_, Zx>) {
    z.stop();
}

#[cfg(test)]
mod tests {
    #[test]
    fn opens_this_macs_sound_output() {
        let s = super::open_sound();
        println!("sound: {} Hz, {} channels, open: {}", s.rate, s.channels, s._stream.is_some());
        assert!(s.rate >= 8000);
    }
}

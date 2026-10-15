//! RAD media engine: plays local files and vault streams with libVLC (through vlc-rs) for the
//! app's Music and Video views. It runs as its own process, like the browser engines:
//! - args: `--mode audio|video`. In audio mode pictures are never decoded, so a video file
//!   plays as sound only.
//! - stdin: one JSON command per line (see `Engine::command`).
//! - stdout: framed messages `[kind: u8][len: u32 LE][payload]`:
//!     1 = JSON event (`ready`, `status`, `meta`, `tracks`, `probe`, `error`),
//!     2 = video frame: `[w, h, y0, rows]` (u32 LE) then RGBA rows, as the browsers send.

mod out;
mod video;

use std::ffi::{CStr, CString};
use std::io::BufRead;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use vlc::sys;
use vlc::{Instance, Media, MediaPlayer, Meta, Position, State};

use out::Out;
use video::Video;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Audio,
    Video,
}

/// How often the engine reports progress while something plays.
const TICK: Duration = Duration::from_millis(200);

struct Engine {
    mode: Mode,
    vlc: Instance,
    player: MediaPlayer,
    media: Option<Media>,
    video: Option<&'static Video>,
    out: Arc<Mutex<Out>>,
    /// What the user chose; re-applied whenever libVLC starts a new audio output.
    volume: i32,
    muted: bool,
    eq: Option<(f32, Vec<f32>)>,
    state: State,
    last_status: Value,
    last_meta: Value,
    last_tracks: Value,
}

fn state_name(s: State) -> &'static str {
    match s {
        State::NothingSpecial => "idle",
        State::Opening => "opening",
        State::Buffering => "buffering",
        State::Playing => "playing",
        State::Paused => "paused",
        State::Stopped => "stopped",
        State::Ended => "ended",
        State::Error => "error",
    }
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// Reads and frees a libVLC track list.
unsafe fn track_list(list: *mut sys::libvlc_track_description_t) -> Vec<Value> {
    let mut out = Vec::new();
    let mut p = list;
    while !p.is_null() {
        let name = if (*p).psz_name.is_null() { String::new() } else { CStr::from_ptr((*p).psz_name).to_string_lossy().into_owned() };
        out.push(json!({"id": (*p).i_id, "name": name}));
        p = (*p).p_next;
    }
    if !list.is_null() {
        sys::libvlc_track_description_list_release(list);
    }
    out
}

fn meta(m: &Media) -> Value {
    let get = |k: Meta| m.get_meta(k).filter(|s| !s.trim().is_empty());
    json!({
        "title": get(Meta::Title), "artist": get(Meta::Artist), "album": get(Meta::Album),
        "genre": get(Meta::Genre), "date": get(Meta::Date), "track": get(Meta::TrackNumber),
        "now_playing": get(Meta::NowPlaying),
    })
}

fn new_media(vlc: &Instance, src: &str) -> Option<Media> {
    let is_url = ["http://", "https://", "file://"].iter().any(|p| src.starts_with(p));
    if is_url {
        Media::new_location(vlc, src)
    } else {
        Media::new_path(vlc, src)
    }
}

fn add_option(m: &Media, opt: &str) {
    let o = cstr(opt);
    unsafe { sys::libvlc_media_add_option(m.raw(), o.as_ptr()) };
}

impl Engine {
    fn emit(&self, v: Value) {
        self.out.lock().unwrap().event(&v);
    }

    /// The equalizer's bands and libVLC's presets, for the app's EQ panel.
    fn eq_info(&self) -> Value {
        unsafe {
            let bands = sys::libvlc_audio_equalizer_get_band_count();
            let freqs: Vec<f32> = (0..bands).map(|i| sys::libvlc_audio_equalizer_get_band_frequency(i)).collect();
            let presets: Vec<Value> = (0..sys::libvlc_audio_equalizer_get_preset_count())
                .map(|i| {
                    let name = CStr::from_ptr(sys::libvlc_audio_equalizer_get_preset_name(i)).to_string_lossy().into_owned();
                    let eq = sys::libvlc_audio_equalizer_new_from_preset(i);
                    let preamp = sys::libvlc_audio_equalizer_get_preamp(eq);
                    let amps: Vec<f32> = (0..bands).map(|b| sys::libvlc_audio_equalizer_get_amp_at_index(eq, b)).collect();
                    sys::libvlc_audio_equalizer_release(eq);
                    json!({"name": name, "preamp": preamp, "bands": amps})
                })
                .collect();
            json!({"freqs": freqs, "presets": presets})
        }
    }

    fn apply_audio(&self) {
        let mp = self.player.raw();
        unsafe {
            sys::libvlc_audio_set_volume(mp, self.volume);
            sys::libvlc_audio_set_mute(mp, self.muted as i32);
            match &self.eq {
                Some((preamp, bands)) => {
                    let eq = sys::libvlc_audio_equalizer_new();
                    sys::libvlc_audio_equalizer_set_preamp(eq, *preamp);
                    for (i, a) in bands.iter().enumerate() {
                        sys::libvlc_audio_equalizer_set_amp_at_index(eq, *a, i as u32);
                    }
                    // libVLC copies the settings.
                    sys::libvlc_media_player_set_equalizer(mp, eq);
                    sys::libvlc_audio_equalizer_release(eq);
                }
                None => {
                    sys::libvlc_media_player_set_equalizer(mp, std::ptr::null_mut());
                }
            }
        }
    }

    fn open(&mut self, c: &Value) {
        let Some(src) = c["src"].as_str().filter(|s| !s.is_empty()) else { return };
        let Some(m) = new_media(&self.vlc, src) else {
            self.emit(json!({"type": "error", "message": format!("VLC can't open {src}")}));
            return;
        };
        if self.mode == Mode::Audio {
            add_option(&m, ":no-video");
        }
        if let Some(v) = self.video {
            v.set_max(c["max_w"].as_u64().unwrap_or(0) as u32, c["max_h"].as_u64().unwrap_or(0) as u32);
        }
        if let Some(ms) = c["start_ms"].as_i64().filter(|ms| *ms > 0) {
            add_option(&m, &format!(":start-time={:.3}", ms as f64 / 1000.0));
        }
        if c["paused"] == true {
            add_option(&m, ":start-paused");
        }
        self.player.set_media(&m);
        self.media = Some(m);
        self.last_meta = Value::Null;
        self.last_tracks = Value::Null;
        if self.player.play().is_err() {
            self.emit(json!({"type": "error", "message": vlc::errmsg().unwrap_or_else(|| "VLC couldn't start playing".into())}));
        }
    }

    fn play(&self) {
        match self.player.state() {
            State::Paused => self.player.set_pause(false),
            State::Playing | State::Opening | State::Buffering => {}
            _ if self.media.is_some() => {
                let _ = self.player.play();
            }
            _ => {}
        }
    }

    /// One command from the app.
    fn command(&mut self, c: Value) {
        let mp = self.player.raw();
        let int = |k: &str| c[k].as_i64().unwrap_or(0);
        match c["cmd"].as_str().unwrap_or("") {
            // {src, max_w?, max_h?, start_ms?, paused?}: `src` is a file path or a URL.
            "open" => self.open(&c),
            "play" => self.play(),
            "pause" => self.player.set_pause(true),
            "toggle" => {
                if self.player.state() == State::Playing {
                    self.player.set_pause(true);
                } else {
                    self.play();
                }
            }
            "stop" => {
                self.player.stop();
            }
            "close" => {
                self.player.stop();
                self.media = None;
            }
            "seek" => {
                let ms = int("ms").max(0);
                if matches!(self.player.state(), State::Ended | State::Stopped) && self.media.is_some() {
                    let _ = self.player.play();
                }
                self.player.set_time(ms);
            }
            "volume" => {
                self.volume = (int("value") as i32).clamp(0, 200);
                unsafe { sys::libvlc_audio_set_volume(mp, self.volume) };
            }
            "mute" => {
                self.muted = c["value"] == true;
                unsafe { sys::libvlc_audio_set_mute(mp, self.muted as i32) };
            }
            "rate" => {
                let _ = self.player.set_rate(c["value"].as_f64().unwrap_or(1.0).clamp(0.25, 4.0) as f32);
            }
            // {enabled, preamp, bands: [dB…]}
            "eq" => {
                self.eq = (c["enabled"] == true).then(|| {
                    let bands = c["bands"].as_array().map(|a| a.iter().map(|b| b.as_f64().unwrap_or(0.0).clamp(-20.0, 20.0) as f32).collect()).unwrap_or_default();
                    (c["preamp"].as_f64().unwrap_or(0.0).clamp(-20.0, 20.0) as f32, bands)
                });
                self.apply_audio();
            }
            "audio_track" => unsafe {
                sys::libvlc_audio_set_track(mp, int("id") as i32);
                self.last_tracks = Value::Null;
            },
            "spu" => unsafe {
                sys::libvlc_video_set_spu(mp, int("id") as i32);
                self.last_tracks = Value::Null;
            },
            "subtitle_file" => {
                if let Some(p) = c["path"].as_str() {
                    let p = cstr(p);
                    unsafe { sys::libvlc_video_set_subtitle_file(mp, p.as_ptr()) };
                    self.last_tracks = Value::Null;
                }
            }
            // {w, h}: the box video is shown in, in device pixels (applies to the next video).
            "max" => {
                if let Some(v) = self.video {
                    v.set_max(int("w") as u32, int("h") as u32);
                }
            }
            // {id, src}: tags and duration of a local file, without playing it.
            "probe" => {
                let id = c["id"].clone();
                let src = c["src"].as_str().unwrap_or("");
                let mut ev = json!({"type": "probe", "id": id});
                if let Some(m) = new_media(&self.vlc, src) {
                    m.parse();
                    ev["meta"] = meta(&m);
                    ev["duration"] = json!(m.duration().unwrap_or(0).max(0));
                }
                self.emit(ev);
            }
            "eq_info" => {
                let mut v = self.eq_info();
                v["type"] = json!("eq_info");
                self.emit(v);
            }
            // The app wants everything again (a view was reopened).
            "refresh" => {
                self.last_status = Value::Null;
                self.last_meta = Value::Null;
                self.last_tracks = Value::Null;
            }
            "quit" => {
                self.player.stop();
                std::process::exit(0);
            }
            other => self.emit(json!({"type": "error", "message": format!("unknown command {other}")})),
        }
    }

    /// Reports what changed since the last tick.
    fn tick(&mut self) {
        let mp = self.player.raw();
        let st = self.player.state();
        if st != self.state {
            if st == State::Playing {
                // A new audio output starts with libVLC's defaults.
                self.apply_audio();
            }
            if st == State::Error {
                self.emit(json!({"type": "error", "message": vlc::errmsg().unwrap_or_else(|| "VLC couldn't play this file".into())}));
            }
            self.state = st;
        }
        let (mut vw, mut vh) = (0u32, 0u32);
        let has_video = self.mode == Mode::Video && unsafe { sys::libvlc_video_get_size(mp, 0, &mut vw, &mut vh) } == 0 && vw > 0 && vh > 0;
        // How wide the picture is meant to be shown: its visible size with the pixels' shape.
        let aspect = has_video.then(|| vw as f64 * unsafe { video::track_shape(mp) }.map_or(1.0, |t| t.2) / vh as f64);
        let status = json!({
            "type": "status",
            "state": state_name(st),
            "has_media": self.media.is_some(),
            "time": self.player.get_time().unwrap_or(0).max(0),
            "length": unsafe { sys::libvlc_media_player_get_length(mp) }.max(0),
            "seekable": self.player.is_seekable(),
            "rate": self.player.get_rate(),
            "volume": self.volume,
            "muted": self.muted,
            "video": if has_video { json!([vw, vh]) } else { Value::Null },
            "aspect": aspect.map(|a| (a * 10_000.0).round() / 10_000.0),
        });
        if status != self.last_status {
            self.emit(status.clone());
            self.last_status = status;
        }
        if !matches!(st, State::Playing | State::Paused) {
            return;
        }
        if let Some(m) = &self.media {
            let mut md = meta(m);
            md["type"] = json!("meta");
            if md != self.last_meta {
                self.emit(md.clone());
                self.last_meta = md;
            }
        }
        let tracks = unsafe {
            json!({
                "type": "tracks",
                "audio": track_list(sys::libvlc_audio_get_track_description(mp)),
                "audio_current": sys::libvlc_audio_get_track(mp),
                "spu": if self.mode == Mode::Video { json!(track_list(sys::libvlc_video_get_spu_description(mp))) } else { json!([]) },
                "spu_current": if self.mode == Mode::Video { sys::libvlc_video_get_spu(mp) } else { -1 },
            })
        };
        if tracks != self.last_tracks {
            self.emit(tracks.clone());
            self.last_tracks = tracks;
        }
    }
}

fn main() {
    // libVLC and its plugins may print; keep the protocol stream for ourselves.
    let out = Arc::new(Mutex::new(Out::take_stdout()));
    let args: Vec<String> = std::env::args().collect();
    let mode = match args.iter().position(|a| a == "--mode").and_then(|i| args.get(i + 1)).map(String::as_str) {
        Some("video") => Mode::Video,
        _ => Mode::Audio,
    };

    let fatal = |out: &Arc<Mutex<Out>>, msg: &str| {
        out.lock().unwrap().event(&json!({"type": "fatal", "message": msg}));
        std::process::exit(1);
    };
    // libVLC finds its plugins next to its own library only inside VLC.app; point it at ours
    // (bundled next to the engine) or, on macOS, at an installed VLC.
    if std::env::var_os("VLC_PLUGIN_PATH").is_none() {
        let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf()));
        let candidates = exe_dir.map(|d| d.join("plugins")).into_iter().chain(cfg!(target_os = "macos").then(|| "/Applications/VLC.app/Contents/MacOS/plugins".into()));
        if let Some(p) = candidates.into_iter().find(|p: &std::path::PathBuf| p.is_dir()) {
            std::env::set_var("VLC_PLUGIN_PATH", p);
        }
    }
    let Some(vlc) = Instance::new() else {
        fatal(&out, "libVLC didn't start (its plugins weren't found). Reinstall VLC or rebuild the media engine with ./scripts/build-media.sh.");
        unreachable!()
    };
    vlc.set_user_agent("RAD", "RAD-media/0.1");
    if std::env::var_os("RAD_MEDIA_DEBUG").is_some() {
        vlc.set_log(|level, _, msg| eprintln!("vlc[{}]: {msg}", level as i32));
    }
    let Some(player) = MediaPlayer::new(&vlc) else {
        fatal(&out, "libVLC couldn't create a player");
        unreachable!()
    };
    player.set_video_title_display(Position::Disable, 0);
    let video = (mode == Mode::Video).then(|| Video::attach(player.raw(), out.clone()));

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    return;
                }
            }
        }
        // The app went away.
        let _ = tx.send(json!({"cmd": "quit"}));
    });

    let mut e = Engine {
        mode,
        vlc,
        player,
        media: None,
        video,
        out,
        volume: 100,
        muted: false,
        eq: None,
        state: State::NothingSpecial,
        last_status: Value::Null,
        last_meta: Value::Null,
        last_tracks: Value::Null,
    };
    let mut ready = e.eq_info();
    ready["type"] = json!("ready");
    ready["version"] = json!(vlc::version());
    ready["mode"] = json!(if mode == Mode::Video { "video" } else { "audio" });
    e.emit(ready);

    loop {
        match rx.recv_timeout(TICK) {
            Ok(c) => {
                e.command(c);
                while let Ok(c) = rx.try_recv() {
                    e.command(c);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        e.tick();
    }
}

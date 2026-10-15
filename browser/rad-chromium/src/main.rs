//! RAD Chromium engine: CEF (the Chromium Embedded Framework) rendering offscreen, driven by
//! the RAD app. It speaks the same protocol as `rad-browser` (Servo), so the app treats both
//! engines alike:
//! - stdin: one JSON command per line, e.g. `{"cmd":"load","url":"https://youtube.com"}`.
//! - stdout: framed messages `[kind: u8][len: u32 LE][payload]`.
//!   - kind 1: a JSON event (`state`, `cursor`, `status`, `dialog`, `eval`, `download`, …).
//!   - kind 2: a frame update: `w, h, y0, rows` (u32 LE each), then `rows` RGBA rows starting
//!     at row `y0`. Only the band of rows that changed since the previous frame is sent.
//!
//! Unlike the Servo engine, browsing data (cookies, sign-ins, cache) persists in the profile
//! folder given with `--profile`, so YouTube and other sites stay signed in. Downloads go to
//! `--downloads` and are reported as `download` events.

mod out;

#[cfg(target_os = "macos")]
mod mac;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::{args::Args, *};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use serde_json::{json, Value};

use crate::out::Out;

/// CEF's `windowless_frame_rate`; the app is sent at most this many frames a second.
const FRAME_RATE: i32 = 60;
/// Longest gap between message-loop turns, whatever CEF asks for.
const MAX_PUMP: Duration = Duration::from_millis(33);
const HOME: &str = "https://www.google.com";

enum Msg {
    /// CEF wants `do_message_loop_work` within this many milliseconds.
    Pump(i64),
    Cmd(Value),
    Eof,
}

struct Tab {
    id: u32,
    browser: Browser,
    url: String,
    title: String,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    zoom: f64,
    /// Keeps the DevTools observer (used for `eval`) registered.
    _devtools: Option<Registration>,
}

struct Download {
    name: String,
    path: String,
    last_percent: i32,
}

/// Everything the engine knows. CEF calls our handlers on this (the main) thread, so it
/// lives in a thread-local and handlers reach it with [`engine`].
struct Engine {
    out: RefCell<Out>,
    tabs: RefCell<Vec<Tab>>,
    active: Cell<u32>,
    next_tab: Cell<u32>,
    /// Viewport in device pixels, and device pixels per CSS pixel.
    size: Cell<(u32, u32)>,
    scale: Cell<f32>,
    dark: Cell<bool>,
    state_dirty: Cell<bool>,
    last_frame: RefCell<Vec<u8>>,
    /// The page as last painted (RGBA), so an open `<select>` popup can be drawn over it.
    view: RefCell<(u32, u32, Vec<u8>)>,
    /// `<select>` and similar popups: where (CSS pixels) and their pixels (RGBA).
    popup_rect: Cell<Option<(i32, i32, i32, i32)>>,
    popup: RefCell<Option<(u32, u32, Vec<u8>)>>,
    next_message: Cell<i32>,
    /// DevTools message id → the app's `eval` id.
    evals: RefCell<HashMap<i32, Value>>,
    next_control: Cell<u64>,
    dialogs: RefCell<HashMap<u64, (i32, JsdialogCallback)>>,
    downloads: RefCell<HashMap<u32, Download>>,
    downloads_dir: PathBuf,
    /// For double clicks: last press (time, x, y, button, count).
    last_click: Cell<Option<(Instant, i32, i32, i32, i32)>>,
    buttons: Cell<u32>,
}

thread_local! {
    static ENGINE: RefCell<Option<Rc<Engine>>> = const { RefCell::new(None) };
}

fn engine() -> Rc<Engine> {
    ENGINE.with_borrow(|e| e.clone()).expect("engine initialised")
}

fn s(v: &CefString) -> String {
    v.to_string()
}

fn opt_s(v: Option<&CefString>) -> String {
    v.map(s).unwrap_or_default()
}

impl Engine {
    fn event(&self, v: Value) {
        self.out.borrow_mut().event(&v);
    }

    fn tab_of(&self, browser: &Browser) -> Option<u32> {
        let id = browser.identifier();
        self.tabs.borrow().iter().find(|t| t.browser.identifier() == id).map(|t| t.id)
    }

    fn browser(&self, tab: Option<u32>) -> Option<Browser> {
        let id = tab.unwrap_or(self.active.get());
        self.tabs.borrow().iter().find(|t| t.id == id).map(|t| t.browser.clone())
    }

    fn host(&self) -> Option<BrowserHost> {
        self.browser(None).and_then(|b| b.host())
    }

    fn is_active(&self, browser: &Browser) -> bool {
        self.tab_of(browser) == Some(self.active.get())
    }

    fn with_tab(&self, browser: &Browser, f: impl FnOnce(&mut Tab)) {
        let id = browser.identifier();
        let Ok(mut tabs) = self.tabs.try_borrow_mut() else { return };
        if let Some(t) = tabs.iter_mut().find(|t| t.browser.identifier() == id) {
            f(t);
            self.state_dirty.set(true);
        }
    }

    /// Adds a browser CEF created (ours, or a page's popup) as a tab, once.
    fn adopt(&self, browser: Browser) -> u32 {
        if let Some(id) = self.tab_of(&browser) {
            return id;
        }
        let id = self.next_tab.get();
        self.next_tab.set(id + 1);
        let devtools = browser.host().and_then(|h| h.add_dev_tools_message_observer(Some(&mut DevToolsObserverBuilder::build())));
        self.tabs.borrow_mut().push(Tab {
            id,
            browser: browser.clone(),
            url: String::new(),
            title: String::new(),
            loading: true,
            can_back: false,
            can_forward: false,
            zoom: 1.0,
            _devtools: devtools,
        });
        self.apply_theme(&browser);
        self.activate(id);
        id
    }

    fn open_tab(&self, url: &str) {
        let window_info = WindowInfo { windowless_rendering_enabled: 1, ..Default::default() };
        let settings = BrowserSettings { windowless_frame_rate: FRAME_RATE, background_color: 0xFFFF_FFFF, ..Default::default() };
        let mut client = ClientBuilder::build();
        let url = CefString::from(url);
        if let Some(b) = browser_host_create_browser_sync(Some(&window_info), Some(&mut client), Some(&url), Some(&settings), None, None) {
            self.adopt(b);
        } else {
            self.event(json!({"type": "error", "message": "Couldn't open a new tab"}));
        }
    }

    /// The tabs' browsers, so CEF can be called without holding a borrow (CEF may call our
    /// handlers right away, and they borrow the tabs too).
    fn browsers(&self) -> Vec<(u32, Browser)> {
        self.tabs.borrow().iter().map(|t| (t.id, t.browser.clone())).collect()
    }

    fn activate(&self, id: u32) {
        for (tid, b) in self.browsers() {
            if let Some(h) = b.host() {
                if tid == id {
                    h.was_hidden(0);
                    h.set_focus(1);
                    h.invalidate(PaintElementType::from(sys::cef_paint_element_type_t::PET_VIEW));
                } else {
                    h.was_hidden(1);
                }
            }
        }
        self.active.set(id);
        self.popup_rect.set(None);
        *self.popup.borrow_mut() = None;
        self.last_frame.borrow_mut().clear();
        self.state_dirty.set(true);
    }

    fn close_tab(&self, id: u32) {
        let removed = {
            let mut tabs = self.tabs.borrow_mut();
            let Some(i) = tabs.iter().position(|t| t.id == id) else { return };
            let t = tabs.remove(i);
            let next = if tabs.is_empty() { None } else { Some(tabs[i.min(tabs.len() - 1)].id) };
            (t, next)
        };
        let (tab, next) = removed;
        if let Some(h) = tab.browser.host() {
            h.close_browser(1);
        }
        match next {
            Some(n) if self.active.get() == id => self.activate(n),
            Some(_) => self.state_dirty.set(true),
            None => {
                self.active.set(0);
                self.state_dirty.set(true);
            }
        }
    }

    fn send_state(&self) {
        let tabs: Vec<Value> = self
            .tabs
            .borrow()
            .iter()
            .map(|t| {
                json!({
                    "id": t.id, "url": t.url, "title": t.title, "loading": t.loading,
                    "can_back": t.can_back, "can_forward": t.can_forward, "zoom": t.zoom,
                })
            })
            .collect();
        self.event(json!({"type": "state", "tabs": tabs, "active": self.active.get()}));
    }

    /// Sends the composed frame (page + popup), only the rows that changed.
    fn present(&self) {
        let view = self.view.borrow();
        let (w, h) = (view.0, view.1);
        if w == 0 || h == 0 {
            return;
        }
        let mut px = view.2.clone();
        drop(view);
        if let (Some((rx, ry, _, _)), Some((pw, ph, pop))) = (self.popup_rect.get(), self.popup.borrow().as_ref()) {
            let scale = self.scale.get();
            let (ox, oy) = ((rx as f32 * scale).round() as i64, (ry as f32 * scale).round() as i64);
            for y in 0..*ph as i64 {
                let ty = oy + y;
                if ty < 0 || ty >= h as i64 {
                    continue;
                }
                for x in 0..*pw as i64 {
                    let tx = ox + x;
                    if tx < 0 || tx >= w as i64 {
                        continue;
                    }
                    let s = ((y * *pw as i64 + x) * 4) as usize;
                    let d = ((ty * w as i64 + tx) * 4) as usize;
                    px[d..d + 4].copy_from_slice(&pop[s..s + 4]);
                }
            }
        }
        let stride = w as usize * 4;
        let mut last = self.last_frame.borrow_mut();
        let (y0, y1) = if last.len() == px.len() {
            let differs = |y: usize| px[y * stride..(y + 1) * stride] != last[y * stride..(y + 1) * stride];
            let Some(first) = (0..h as usize).find(|&y| differs(y)) else { return };
            let end = (first..h as usize).rev().find(|&y| differs(y)).unwrap_or(first) + 1;
            (first, end)
        } else {
            (0, h as usize)
        };
        self.out.borrow_mut().frame(w, h, y0 as u32, &px[y0 * stride..y1 * stride]);
        *last = px;
    }

    fn devtools(&self, browser: &Browser, method: &str, params: Value) -> i32 {
        let id = self.next_message.get();
        self.next_message.set(id + 1);
        let msg = json!({"id": id, "method": method, "params": params}).to_string();
        if let Some(h) = browser.host() {
            h.send_dev_tools_message(Some(msg.as_bytes()));
        }
        id
    }

    fn apply_theme(&self, browser: &Browser) {
        let scheme = if self.dark.get() { "dark" } else { "light" };
        self.devtools(browser, "Emulation.setEmulatedMedia", json!({"features": [{"name": "prefers-color-scheme", "value": scheme}]}));
    }

    fn resize(&self, w: u32, h: u32, scale: f32) {
        self.size.set((w.max(1), h.max(1)));
        self.scale.set(scale);
        self.last_frame.borrow_mut().clear();
        for (_, b) in self.browsers() {
            if let Some(host) = b.host() {
                host.notify_screen_info_changed();
                host.was_resized();
            }
        }
    }

    fn mouse(&self, c: &Value) -> MouseEvent {
        let scale = self.scale.get().max(0.5) as f64;
        MouseEvent {
            x: (c["x"].as_f64().unwrap_or(0.0) / scale).round() as i32,
            y: (c["y"].as_f64().unwrap_or(0.0) / scale).round() as i32,
            modifiers: self.buttons.get() | modifiers(c),
        }
    }

    fn handle(&self, c: &Value) {
        let str_ = |k: &str| c[k].as_str().unwrap_or_default().to_string();
        let tab_arg = c["tab"].as_u64().map(|t| t as u32);
        match c["cmd"].as_str().unwrap_or_default() {
            "new_tab" => {
                let url = str_("url");
                self.open_tab(if url.is_empty() { "about:blank" } else { &url });
            }
            "close_tab" => self.close_tab(tab_arg.unwrap_or(self.active.get())),
            "activate" => {
                if let Some(t) = tab_arg {
                    self.activate(t);
                }
            }
            "load" => {
                let url = str_("url");
                match self.browser(tab_arg).and_then(|b| b.main_frame()) {
                    Some(f) => f.load_url(Some(&CefString::from(url.as_str()))),
                    None => self.open_tab(&url),
                }
            }
            "back" => {
                if let Some(b) = self.browser(tab_arg).filter(|b| b.can_go_back() == 1) {
                    b.go_back();
                }
            }
            "forward" => {
                if let Some(b) = self.browser(tab_arg).filter(|b| b.can_go_forward() == 1) {
                    b.go_forward();
                }
            }
            "reload" => {
                if let Some(b) = self.browser(tab_arg) {
                    b.reload();
                }
            }
            "zoom" => {
                // The app speaks in page-zoom factors (1.0 = 100%); Chromium in steps of 20%.
                let factor = c["level"].as_f64().unwrap_or(1.0).clamp(0.3, 5.0);
                if let Some(b) = self.browser(tab_arg) {
                    if let Some(h) = b.host() {
                        h.set_zoom_level(factor.ln() / 1.2f64.ln());
                    }
                    self.with_tab(&b, |t| t.zoom = factor);
                }
            }
            "theme" => {
                self.dark.set(str_("theme") == "dark");
                for (_, b) in self.browsers() {
                    self.apply_theme(&b);
                }
            }
            "resize" => self.resize(
                c["w"].as_f64().unwrap_or(1.0) as u32,
                c["h"].as_f64().unwrap_or(1.0) as u32,
                (c["scale"].as_f64().unwrap_or(1.0) as f32).max(0.5),
            ),
            "repaint" => {
                self.last_frame.borrow_mut().clear();
                if let Some(h) = self.host() {
                    h.invalidate(PaintElementType::from(sys::cef_paint_element_type_t::PET_VIEW));
                }
            }
            "mouse_move" => {
                if let Some(h) = self.host() {
                    h.send_mouse_move_event(Some(&self.mouse(c)), 0);
                }
            }
            "mouse_leave" => {
                if let Some(h) = self.host() {
                    h.send_mouse_move_event(Some(&self.mouse(c)), 1);
                }
            }
            "mouse_button" => {
                let Some(h) = self.host() else { return };
                let up = str_("action") == "up";
                let button = c["button"].as_i64().unwrap_or(0) as i32;
                let (kind, flag) = match button {
                    1 => (sys::cef_mouse_button_type_t::MBT_MIDDLE, sys::cef_event_flags_t::EVENTFLAG_MIDDLE_MOUSE_BUTTON.0),
                    2 => (sys::cef_mouse_button_type_t::MBT_RIGHT, sys::cef_event_flags_t::EVENTFLAG_RIGHT_MOUSE_BUTTON.0),
                    _ => (sys::cef_mouse_button_type_t::MBT_LEFT, sys::cef_event_flags_t::EVENTFLAG_LEFT_MOUSE_BUTTON.0),
                };
                let ev = self.mouse(c);
                let count = if up {
                    self.last_click.get().map(|l| l.4).unwrap_or(1)
                } else {
                    let now = Instant::now();
                    let n = match self.last_click.get() {
                        Some((t, x, y, b, n)) if b == button && now.duration_since(t) < Duration::from_millis(500) && (x - ev.x).abs() < 5 && (y - ev.y).abs() < 5 => (n % 3) + 1,
                        _ => 1,
                    };
                    self.last_click.set(Some((now, ev.x, ev.y, button, n)));
                    n
                };
                if up {
                    self.buttons.set(self.buttons.get() & !(flag as u32));
                } else {
                    self.buttons.set(self.buttons.get() | flag as u32);
                    h.set_focus(1);
                }
                h.send_mouse_click_event(Some(&ev), MouseButtonType::from(kind), up as i32, count);
            }
            "wheel" => {
                let Some(h) = self.host() else { return };
                let scale = self.scale.get().max(0.5) as f64;
                // The app sends DOM deltas (positive = down) in device pixels, lines or pages;
                // CEF wants CSS pixels with positive = up.
                let unit = match c["mode"].as_u64() {
                    Some(1) => 40.0,
                    Some(2) => self.size.get().1 as f64 / scale,
                    _ => 1.0 / scale,
                };
                let dx = -(c["dx"].as_f64().unwrap_or(0.0) * unit).round() as i32;
                let dy = -(c["dy"].as_f64().unwrap_or(0.0) * unit).round() as i32;
                h.send_mouse_wheel_event(Some(&self.mouse(c)), dx, dy);
            }
            "key" => self.key(c),
            "text" => {
                // Committed input-method text (accents, CJK, emoji): typed as characters, which
                // works without an input-method composition in progress.
                let Some(h) = self.host() else { return };
                for unit in str_("text").encode_utf16() {
                    let ev = KeyEvent {
                        size: std::mem::size_of::<sys::cef_key_event_t>(),
                        type_: KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_CHAR),
                        modifiers: 0,
                        windows_key_code: unit as i32,
                        native_key_code: 0,
                        is_system_key: 0,
                        character: unit,
                        unmodified_character: unit,
                        focus_on_editable_field: 0,
                    };
                    h.send_key_event(Some(&ev));
                }
            }
            "clipboard" => {
                let Some(b) = self.browser(None) else { return };
                let Some(f) = b.focused_frame().or_else(|| b.main_frame()) else { return };
                match str_("action").as_str() {
                    "cut" => f.cut(),
                    "paste" => f.paste(),
                    _ => f.copy(),
                }
            }
            "eval" => {
                let id = c["id"].clone();
                let Some(b) = self.browser(tab_arg) else {
                    self.event(json!({"type": "eval", "id": id, "ok": false, "error": "no open tab"}));
                    return;
                };
                let msg = self.devtools(&b, "Runtime.evaluate", json!({"expression": str_("script"), "returnByValue": true, "awaitPromise": true}));
                self.evals.borrow_mut().insert(msg, id);
            }
            "dialog_reply" => {
                let control = c["control"].as_u64().unwrap_or(0);
                // Take it out first: answering resumes the page, which may open the next dialog.
                let pending = self.dialogs.borrow_mut().remove(&control);
                if let Some((_, cb)) = pending {
                    let ok = c["ok"].as_bool().unwrap_or(false);
                    cb.cont(ok as i32, Some(&CefString::from(str_("value").as_str())));
                }
            }
            // <select> menus are drawn by Chromium itself (a popup in the frame).
            "select_reply" => {}
            other => log::warn!("unknown command {other:?}"),
        }
    }

    fn key(&self, c: &Value) {
        let Some(h) = self.host() else { return };
        let key = c["key"].as_str().unwrap_or_default();
        let code = c["code"].as_str().unwrap_or_default();
        let up = c["state"].as_str() == Some("up");
        let (ctrl, meta, alt) = (c["ctrl"].as_bool().unwrap_or(false), c["meta"].as_bool().unwrap_or(false), c["alt"].as_bool().unwrap_or(false));
        let mut mods = modifiers(c);
        match c["location"].as_u64() {
            Some(1) => mods |= sys::cef_event_flags_t::EVENTFLAG_IS_LEFT.0 as u32,
            Some(2) => mods |= sys::cef_event_flags_t::EVENTFLAG_IS_RIGHT.0 as u32,
            Some(3) => mods |= sys::cef_event_flags_t::EVENTFLAG_IS_KEY_PAD.0 as u32,
            _ => {}
        }
        if c["repeat"].as_bool().unwrap_or(false) {
            mods |= sys::cef_event_flags_t::EVENTFLAG_IS_REPEAT.0 as u32;
        }
        // Edit shortcuts: offscreen Chromium on macOS doesn't map ⌘A/⌘Z itself.
        let shortcut = if cfg!(target_os = "macos") { meta } else { ctrl };
        if shortcut && !up {
            if let Some(f) = self.browser(None).and_then(|b| b.focused_frame().or_else(|| b.main_frame())) {
                match key.to_ascii_lowercase().as_str() {
                    "a" => return f.select_all(),
                    "z" if c["shift"].as_bool().unwrap_or(false) => return f.redo(),
                    "z" => return f.undo(),
                    _ => {}
                }
            }
        }
        let (vk, native) = key_codes(code, key);
        let chars: Vec<u16> = key.encode_utf16().collect();
        let printable = key.chars().count() == 1;
        let typed = if printable && chars.len() == 1 { chars[0] } else if key == "Enter" { 13 } else { 0 };
        // macOS builds a native key event from these: a key without characters comes out as a
        // second key-down on release, so special keys carry AppKit's function-key characters.
        let character = if typed == 0 && cfg!(target_os = "macos") { mac_function_char(code) } else { typed };
        let mut ev = KeyEvent {
            size: std::mem::size_of::<sys::cef_key_event_t>(),
            type_: KeyEventType::from(if up { sys::cef_key_event_type_t::KEYEVENT_KEYUP } else { sys::cef_key_event_type_t::KEYEVENT_RAWKEYDOWN }),
            modifiers: mods,
            windows_key_code: vk,
            native_key_code: native,
            is_system_key: (alt && !cfg!(target_os = "macos")) as i32,
            character,
            unmodified_character: key.to_lowercase().encode_utf16().next().filter(|_| printable).unwrap_or(character),
            focus_on_editable_field: 0,
        };
        h.send_key_event(Some(&ev));
        // Typed characters (and Enter) also arrive as a CHAR event, unless a shortcut is held.
        if !up && typed != 0 && !shortcut && !(ctrl && !cfg!(target_os = "macos")) {
            ev.type_ = KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_CHAR);
            if !cfg!(target_os = "macos") {
                ev.windows_key_code = character as i32;
            }
            h.send_key_event(Some(&ev));
        }
    }
}

fn modifiers(c: &Value) -> u32 {
    let mut m = 0u32;
    for (k, f) in [
        ("shift", sys::cef_event_flags_t::EVENTFLAG_SHIFT_DOWN),
        ("ctrl", sys::cef_event_flags_t::EVENTFLAG_CONTROL_DOWN),
        ("alt", sys::cef_event_flags_t::EVENTFLAG_ALT_DOWN),
        ("meta", sys::cef_event_flags_t::EVENTFLAG_COMMAND_DOWN),
    ] {
        if c[k].as_bool().unwrap_or(false) {
            m |= f.0 as u32;
        }
    }
    m
}

/// DOM `code` → (Windows virtual-key code, macOS virtual key code). Chromium uses the first on
/// every platform and the second on macOS, where offscreen key events need the native code.
fn key_codes(code: &str, key: &str) -> (i32, i32) {
    const MAC_LETTERS: [i32; 26] = [
        0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F, 0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10,
        0x06,
    ];
    const MAC_DIGITS: [i32; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
    if let Some(l) = code.strip_prefix("Key").and_then(|l| l.bytes().next()).filter(|b| b.is_ascii_uppercase()) {
        return (l as i32, MAC_LETTERS[(l - b'A') as usize]);
    }
    if let Some(d) = code.strip_prefix("Digit").and_then(|d| d.bytes().next()).filter(|b| b.is_ascii_digit()) {
        return (d as i32, MAC_DIGITS[(d - b'0') as usize]);
    }
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<i32>().ok()).filter(|n| (1..=12).contains(n)) {
        const MAC_F: [i32; 12] = [0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F];
        return (0x6F + n, MAC_F[(n - 1) as usize]);
    }
    if let Some(d) = code.strip_prefix("Numpad").and_then(|d| d.bytes().next()).filter(|b| b.is_ascii_digit() && code.len() == 7) {
        const MAC_PAD: [i32; 10] = [0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5B, 0x5C];
        return (0x60 + (d - b'0') as i32, MAC_PAD[(d - b'0') as usize]);
    }
    let pair = match code {
        "Enter" | "NumpadEnter" => (0x0D, if code == "Enter" { 0x24 } else { 0x4C }),
        "Tab" => (0x09, 0x30),
        "Space" => (0x20, 0x31),
        "Backspace" => (0x08, 0x33),
        "Escape" => (0x1B, 0x35),
        "Delete" => (0x2E, 0x75),
        "Insert" => (0x2D, 0x72),
        "Home" => (0x24, 0x73),
        "End" => (0x23, 0x77),
        "PageUp" => (0x21, 0x74),
        "PageDown" => (0x22, 0x79),
        "ArrowLeft" => (0x25, 0x7B),
        "ArrowRight" => (0x27, 0x7C),
        "ArrowDown" => (0x28, 0x7D),
        "ArrowUp" => (0x26, 0x7E),
        "ShiftLeft" => (0x10, 0x38),
        "ShiftRight" => (0x10, 0x3C),
        "ControlLeft" => (0x11, 0x3B),
        "ControlRight" => (0x11, 0x3E),
        "AltLeft" => (0x12, 0x3A),
        "AltRight" => (0x12, 0x3D),
        "MetaLeft" => (0x5B, 0x37),
        "MetaRight" => (0x5C, 0x36),
        "CapsLock" => (0x14, 0x39),
        "Equal" => (0xBB, 0x18),
        "Minus" => (0xBD, 0x1B),
        "BracketLeft" => (0xDB, 0x21),
        "BracketRight" => (0xDD, 0x1E),
        "Backslash" => (0xDC, 0x2A),
        "Semicolon" => (0xBA, 0x29),
        "Quote" => (0xDE, 0x27),
        "Comma" => (0xBC, 0x2B),
        "Period" => (0xBE, 0x2F),
        "Slash" => (0xBF, 0x2C),
        "Backquote" => (0xC0, 0x32),
        "IntlBackslash" => (0xE2, 0x0A),
        "NumpadAdd" => (0x6B, 0x45),
        "NumpadSubtract" => (0x6D, 0x4E),
        "NumpadMultiply" => (0x6A, 0x43),
        "NumpadDivide" => (0x6F, 0x4B),
        "NumpadDecimal" => (0x6E, 0x41),
        _ => {
            // Unknown physical key: fall back to the character it typed.
            let c = key.chars().next().filter(|_| key.chars().count() == 1).map(|c| c.to_ascii_uppercase() as i32).unwrap_or(0);
            (c, 0)
        }
    };
    pair
}

/// AppKit's characters for keys that don't type text (NSDeleteCharacter, NSUpArrowFunctionKey…).
fn mac_function_char(code: &str) -> u16 {
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()).filter(|n| (1..=12).contains(n)) {
        return 0xF703 + n;
    }
    match code {
        "Backspace" => 0x7F,
        "Tab" => 0x09,
        "Escape" => 0x1B,
        "Delete" => 0xF728,
        "Insert" => 0xF727,
        "ArrowUp" => 0xF700,
        "ArrowDown" => 0xF701,
        "ArrowLeft" => 0xF702,
        "ArrowRight" => 0xF703,
        "Home" => 0xF729,
        "End" => 0xF72B,
        "PageUp" => 0xF72C,
        "PageDown" => 0xF72D,
        _ => 0,
    }
}

fn css_cursor(t: CursorType) -> &'static str {
    match t.get_raw() {
        1 => "crosshair",
        2 => "pointer",
        3 => "text",
        4 => "wait",
        5 => "help",
        6 | 13 | 15 => "ew-resize",
        7 | 10 | 14 => "ns-resize",
        8 | 12 | 16 => "nesw-resize",
        9 | 11 | 17 => "nwse-resize",
        18 => "col-resize",
        19 => "row-resize",
        20..=28 | 43 | 44 => "all-scroll",
        29 => "move",
        30 => "vertical-text",
        31 => "cell",
        32 => "context-menu",
        33 => "alias",
        34 => "progress",
        35 | 46 => "no-drop",
        36 | 48 => "copy",
        37 => "none",
        38 => "not-allowed",
        39 => "zoom-in",
        40 => "zoom-out",
        41 => "grab",
        42 => "grabbing",
        _ => "default",
    }
}

/// BGRA → RGBA.
fn to_rgba(buf: &[u8]) -> Vec<u8> {
    let mut px = buf.to_vec();
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    px
}

// ------------------------------------------------------------------ CEF handlers

#[derive(Clone)]
struct RadApp {
    tx: Sender<Msg>,
}

wrap_app! {
    struct AppBuilder {
        app: RadApp,
    }

    impl App {
        fn on_before_command_line_processing(&self, _process_type: Option<&CefStringUtf16>, command_line: Option<&mut CommandLine>) {
            let Some(cl) = command_line else { return };
            for sw in ["no-startup-window", "noerrdialogs", "hide-crash-restore-bubble", "disable-session-crashed-bubble"] {
                cl.append_switch(Some(&sw.into()));
            }
            // The profile's cookies are protected by the OS account (its folder is private);
            // this avoids a Keychain prompt every time the engine is rebuilt or updated.
            #[cfg(target_os = "macos")]
            cl.append_switch(Some(&"use-mock-keychain".into()));
            cl.append_switch_with_value(Some(&"autoplay-policy".into()), Some(&"no-user-gesture-required".into()));
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(BrowserProcessHandlerBuilder::new(self.app.clone()))
        }
    }
}

impl AppBuilder {
    fn build(app: RadApp) -> cef::App {
        Self::new(app)
    }
}

wrap_browser_process_handler! {
    struct BrowserProcessHandlerBuilder {
        app: RadApp,
    }

    impl BrowserProcessHandler {
        // May be called from any thread: it only queues a wake-up for the main loop.
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            let _ = self.app.tx.send(Msg::Pump(delay_ms));
        }
    }
}

#[derive(Clone)]
struct Handlers;

wrap_render_handler! {
    struct RenderHandlerBuilder {
        h: Handlers,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            let Some(rect) = rect else { return };
            let e = engine();
            let (w, h) = e.size.get();
            let scale = e.scale.get().max(0.5);
            rect.x = 0;
            rect.y = 0;
            rect.width = ((w as f32 / scale).round() as i32).max(1);
            rect.height = ((h as f32 / scale).round() as i32).max(1);
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, info: Option<&mut ScreenInfo>) -> ::std::os::raw::c_int {
            let Some(info) = info else { return 0 };
            let e = engine();
            let (w, h) = e.size.get();
            let scale = e.scale.get().max(0.5);
            info.device_scale_factor = scale;
            info.depth = 24;
            info.depth_per_component = 8;
            let r = Rect { x: 0, y: 0, width: (w as f32 / scale) as i32, height: (h as f32 / scale) as i32 };
            info.rect = r.clone();
            info.available_rect = r;
            1
        }

        fn screen_point(
            &self,
            _browser: Option<&mut Browser>,
            view_x: ::std::os::raw::c_int,
            view_y: ::std::os::raw::c_int,
            screen_x: Option<&mut ::std::os::raw::c_int>,
            screen_y: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let (Some(x), Some(y)) = (screen_x, screen_y) {
                *x = view_x;
                *y = view_y;
                return 1;
            }
            0
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: ::std::os::raw::c_int) {
            if show == 0 {
                let e = engine();
                e.popup_rect.set(None);
                *e.popup.borrow_mut() = None;
                e.present();
            }
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if let Some(r) = rect {
                engine().popup_rect.set(Some((r.x, r.y, r.width, r.height)));
            }
        }

        fn on_paint(
            &self,
            browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            let Some(browser) = browser else { return };
            if buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let e = engine();
            if !e.is_active(browser) {
                return;
            }
            let (w, h) = (width as u32, height as u32);
            let px = to_rgba(unsafe { std::slice::from_raw_parts(buffer, w as usize * h as usize * 4) });
            if type_.get_raw() == sys::cef_paint_element_type_t::PET_POPUP as u32 {
                *e.popup.borrow_mut() = Some((w, h, px));
            } else {
                *e.view.borrow_mut() = (w, h, px);
            }
            e.present();
        }
    }
}

wrap_display_handler! {
    struct DisplayHandlerBuilder {
        h: Handlers,
    }

    impl DisplayHandler {
        fn on_address_change(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            let (Some(b), Some(f)) = (browser, frame) else { return };
            if f.is_main() == 1 {
                let url = opt_s(url);
                engine().with_tab(b, |t| t.url = url);
            }
        }

        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            let Some(b) = browser else { return };
            let title = opt_s(title);
            engine().with_tab(b, |t| t.title = title);
        }

        fn on_status_message(&self, browser: Option<&mut Browser>, value: Option<&CefString>) {
            let Some(b) = browser else { return };
            let e = engine();
            if e.is_active(b) {
                let text = value.map(s).filter(|t| !t.is_empty());
                e.event(json!({"type": "status", "text": text}));
            }
        }

        fn on_cursor_change(
            &self,
            browser: Option<&mut Browser>,
            _cursor: *mut u8,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(b) = browser else { return 0 };
            let e = engine();
            if e.is_active(b) {
                e.event(json!({"type": "cursor", "cursor": css_cursor(type_)}));
            }
            1
        }
    }
}

wrap_load_handler! {
    struct LoadHandlerBuilder {
        h: Handlers,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            let Some(b) = browser else { return };
            engine().with_tab(b, |t| {
                t.loading = is_loading == 1;
                t.can_back = can_go_back == 1;
                t.can_forward = can_go_forward == 1;
            });
        }

        fn on_load_error(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            let (Some(b), Some(f)) = (browser, frame) else { return };
            // -3: aborted (a new navigation replaced this one) — not an error.
            if f.is_main() != 1 || error_code.get_raw() == -3 {
                return;
            }
            let e = engine();
            if e.is_active(b) {
                e.event(json!({"type": "error", "message": format!("Couldn't open {}: {}", opt_s(failed_url), opt_s(error_text))}));
            }
        }
    }
}

wrap_life_span_handler! {
    struct LifeSpanHandlerBuilder {
        h: Handlers,
    }

    impl LifeSpanHandler {
        /// window.open / target=_blank / sign-in popups: let Chromium create them (so
        /// `window.opener` keeps working) as offscreen browsers that become tabs.
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            _target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let Some(wi) = window_info {
                wi.windowless_rendering_enabled = 1;
                wi.shared_texture_enabled = 0;
            }
            if let Some(s) = settings {
                s.windowless_frame_rate = FRAME_RATE;
            }
            if let Some(c) = client {
                *c = Some(ClientBuilder::build());
            }
            0
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser {
                engine().adopt(b.clone());
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let Some(b) = browser else { return };
            let e = engine();
            if let Some(id) = e.tab_of(b) {
                e.close_tab(id);
            }
        }
    }
}

wrap_jsdialog_handler! {
    struct JsdialogHandlerBuilder {
        h: Handlers,
    }

    impl JsdialogHandler {
        fn on_jsdialog(
            &self,
            browser: Option<&mut Browser>,
            _origin_url: Option<&CefString>,
            dialog_type: JsdialogType,
            message_text: Option<&CefString>,
            default_prompt_text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            _suppress_message: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let (Some(b), Some(cb)) = (browser, callback) else { return 0 };
            let e = engine();
            let id = e.next_control.get();
            e.next_control.set(id + 1);
            let kind = match dialog_type.get_raw() {
                1 => "confirm",
                2 => "prompt",
                _ => "alert",
            };
            e.dialogs.borrow_mut().insert(id, (b.identifier(), cb.clone()));
            e.event(json!({"type": "dialog", "control": id, "tab": e.tab_of(b), "kind": kind, "message": opt_s(message_text), "default": opt_s(default_prompt_text)}));
            1
        }

        fn on_before_unload_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _message_text: Option<&CefString>,
            _is_reload: ::std::os::raw::c_int,
            callback: Option<&mut JsdialogCallback>,
        ) -> ::std::os::raw::c_int {
            // "Leave site?": leaving is what the user asked for.
            if let Some(cb) = callback {
                cb.cont(1, None);
            }
            1
        }

        fn on_reset_dialog_state(&self, browser: Option<&mut Browser>) {
            let Some(b) = browser else { return };
            let e = engine();
            let id = b.identifier();
            let gone: Vec<u64> = e.dialogs.borrow().iter().filter(|(_, (bid, _))| *bid == id).map(|(k, _)| *k).collect();
            for k in gone {
                e.dialogs.borrow_mut().remove(&k);
                e.event(json!({"type": "control_hidden", "control": k}));
            }
        }
    }
}

wrap_download_handler! {
    struct DownloadHandlerBuilder {
        h: Handlers,
    }

    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _request_method: Option<&CefString>) -> ::std::os::raw::c_int {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(item), Some(cb)) = (download_item, callback) else { return 0 };
            let e = engine();
            let name = sanitize_file_name(&opt_s(suggested_name));
            let path = unique_path(&e.downloads_dir, &name);
            let _ = std::fs::create_dir_all(&e.downloads_dir);
            let path_s = path.display().to_string();
            cb.cont(Some(&CefString::from(path_s.as_str())), 0);
            e.downloads.borrow_mut().insert(item.id(), Download { name: name.clone(), path: path_s.clone(), last_percent: -1 });
            e.event(json!({"type": "download", "id": item.id(), "name": name, "path": path_s, "received": 0, "total": item.total_bytes(), "state": "started"}));
            1
        }

        fn on_download_updated(&self, _browser: Option<&mut Browser>, download_item: Option<&mut DownloadItem>, _callback: Option<&mut DownloadItemCallback>) {
            let Some(item) = download_item else { return };
            let e = engine();
            let mut downloads = e.downloads.borrow_mut();
            let Some(d) = downloads.get_mut(&item.id()) else { return };
            let state = if item.is_complete() == 1 {
                "done"
            } else if item.is_canceled() == 1 {
                "canceled"
            } else if item.is_interrupted() == 1 {
                "failed"
            } else {
                "progress"
            };
            let percent = item.percent_complete();
            if state == "progress" && percent == d.last_percent {
                return;
            }
            d.last_percent = percent;
            let ev = json!({
                "type": "download", "id": item.id(), "name": d.name, "path": d.path,
                "received": item.received_bytes(), "total": item.total_bytes(), "state": state,
            });
            if state != "progress" {
                downloads.remove(&item.id());
            }
            drop(downloads);
            e.event(ev);
        }
    }
}

wrap_context_menu_handler! {
    struct ContextMenuHandlerBuilder {
        h: Handlers,
    }

    impl ContextMenuHandler {
        // An offscreen browser has no native menu to show.
        fn on_before_context_menu(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _params: Option<&mut ContextMenuParams>, model: Option<&mut MenuModel>) {
            if let Some(m) = model {
                m.clear();
            }
        }
    }
}

wrap_dev_tools_message_observer! {
    struct DevToolsObserverBuilder {
        h: Handlers,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            let e = engine();
            let Some(id) = e.evals.borrow_mut().remove(&message_id) else { return };
            let v: Value = result.and_then(|r| serde_json::from_slice(r).ok()).unwrap_or(Value::Null);
            let ev = if success != 1 {
                json!({"type": "eval", "id": id, "ok": false, "error": v["message"].as_str().unwrap_or("the page refused the script")})
            } else if let Some(ex) = v.get("exceptionDetails") {
                let msg = ex["exception"]["description"].as_str().or(ex["text"].as_str()).unwrap_or("script error");
                json!({"type": "eval", "id": id, "ok": false, "error": msg})
            } else {
                json!({"type": "eval", "id": id, "ok": true, "value": v["result"]["value"].clone()})
            };
            e.event(ev);
        }
    }
}

impl DevToolsObserverBuilder {
    fn build() -> DevToolsMessageObserver {
        Self::new(Handlers)
    }
}

wrap_client! {
    struct ClientBuilder {
        render: RenderHandler,
        display: DisplayHandler,
        load: LoadHandler,
        life_span: LifeSpanHandler,
        jsdialog: JsdialogHandler,
        download: DownloadHandler,
        context_menu: ContextMenuHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }
        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            Some(self.jsdialog.clone())
        }
        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.download.clone())
        }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(self.context_menu.clone())
        }
    }
}

impl ClientBuilder {
    fn build() -> Client {
        Self::new(
            RenderHandlerBuilder::new(Handlers),
            DisplayHandlerBuilder::new(Handlers),
            LoadHandlerBuilder::new(Handlers),
            LifeSpanHandlerBuilder::new(Handlers),
            JsdialogHandlerBuilder::new(Handlers),
            DownloadHandlerBuilder::new(Handlers),
            ContextMenuHandlerBuilder::new(Handlers),
        )
    }
}

// ------------------------------------------------------------------ helpers

fn sanitize_file_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '-' } else { c }).collect();
    let n = n.trim().trim_start_matches('.').to_string();
    if n.is_empty() { "download".into() } else { n }
}

fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap()
}

fn arg(name: &str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(a) = args.next() {
        if a == name {
            return args.next();
        }
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
    }
    None
}

fn default_downloads() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_else(|| ".".into());
    home.join("Downloads")
}

fn main() {
    let args = Args::new();

    #[cfg(target_os = "macos")]
    let _loader = {
        let loader = library_loader::LibraryLoader::new(&std::env::current_exe().expect("exe path"), false);
        assert!(loader.load(), "couldn't load the Chromium Embedded Framework");
        loader
    };
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    // On Linux and Windows, Chromium's helper processes are this same executable.
    let is_browser_process = args.as_cmd_line().is_none_or(|c| c.has_switch(Some(&CefString::from("type"))) != 1);
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = crossbeam_channel::unbounded();
    let mut app = AppBuilder::build(RadApp { tx: tx.clone() });
    if !is_browser_process {
        let code = execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
        std::process::exit(code.max(0));
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    // CEF and its libraries may print to stdout; keep the protocol stream for ourselves. Done
    // before CEF starts, so its helper processes inherit stderr as their stdout.
    let out = Out::take_stdout();

    #[cfg(target_os = "macos")]
    mac::setup_application();

    let width: u32 = arg("--width").and_then(|v| v.parse().ok()).unwrap_or(1280);
    let height: u32 = arg("--height").and_then(|v| v.parse().ok()).unwrap_or(800);
    let scale: f32 = arg("--scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let start = arg("--url").unwrap_or_else(|| HOME.into());
    let profile = arg("--profile").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("rad-chromium-profile"));
    let downloads_dir = arg("--downloads").map(PathBuf::from).unwrap_or_else(default_downloads);
    let _ = std::fs::create_dir_all(&profile);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700));
    }

    let ret = execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
    assert_eq!(ret, -1, "not the browser process");

    let profile_s = CefString::from(profile.display().to_string().as_str());
    #[allow(unused_mut)]
    let mut settings = Settings {
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        no_sandbox: (!cfg!(target_os = "macos")) as _,
        root_cache_path: profile_s.clone(),
        cache_path: profile_s,
        persist_session_cookies: 1,
        background_color: 0xFFFF_FFFF,
        ..Default::default()
    };
    // Inside RAD.app the engine is a nested app (Contents/Helpers/rad-chromium.app), and
    // Chromium would otherwise look for its framework and helpers in the outermost app.
    #[cfg(target_os = "macos")]
    if let Some(bundle) = std::env::current_exe().ok().and_then(|e| e.parent()?.parent()?.parent().map(PathBuf::from)) {
        let framework = bundle.join("Contents/Frameworks/Chromium Embedded Framework.framework");
        settings.main_bundle_path = CefString::from(bundle.display().to_string().as_str());
        settings.framework_dir_path = CefString::from(framework.display().to_string().as_str());
    }

    let eng = Rc::new(Engine {
        out: RefCell::new(out),
        tabs: RefCell::new(Vec::new()),
        active: Cell::new(0),
        next_tab: Cell::new(1),
        size: Cell::new((width.max(1), height.max(1))),
        scale: Cell::new(scale.max(0.5)),
        dark: Cell::new(false),
        state_dirty: Cell::new(true),
        last_frame: RefCell::new(Vec::new()),
        view: RefCell::new((0, 0, Vec::new())),
        popup_rect: Cell::new(None),
        popup: RefCell::new(None),
        next_message: Cell::new(1),
        evals: RefCell::new(HashMap::new()),
        next_control: Cell::new(1),
        dialogs: RefCell::new(HashMap::new()),
        downloads: RefCell::new(HashMap::new()),
        downloads_dir,
        last_click: Cell::new(None),
        buttons: Cell::new(0),
    });
    ENGINE.with_borrow_mut(|e| *e = Some(eng.clone()));

    if initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) != 1 {
        eng.event(json!({"type": "fatal", "message": "Chromium couldn't start (is another copy using the same profile?)"}));
        std::process::exit(2);
    }

    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Value>(&line) {
                    Ok(v) => {
                        let _ = tx.send(Msg::Cmd(v));
                    }
                    Err(e) => log::warn!("bad command: {e}"),
                }
            }
            let _ = tx.send(Msg::Eof);
        });
    }

    eng.open_tab(&start);
    eng.event(json!({"type": "ready", "engine": "chromium", "version": env!("CARGO_PKG_VERSION")}));

    let mut next_pump = Instant::now();
    'main: loop {
        let wait = next_pump.saturating_duration_since(Instant::now()).min(MAX_PUMP);
        let mut msgs = Vec::new();
        match rx.recv_timeout(wait) {
            Ok(m) => msgs.push(m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        msgs.extend(rx.try_iter());
        let mut pump_now = Instant::now() >= next_pump;
        for m in msgs {
            match m {
                Msg::Pump(ms) => {
                    let at = Instant::now() + Duration::from_millis(ms.max(0) as u64);
                    if ms <= 0 {
                        pump_now = true;
                    } else if at < next_pump || next_pump <= Instant::now() {
                        next_pump = at;
                    }
                }
                Msg::Cmd(c) => {
                    eng.handle(&c);
                    pump_now = true;
                }
                // The app went away: shut down.
                Msg::Eof => break 'main,
            }
        }
        if pump_now {
            do_message_loop_work();
            next_pump = Instant::now() + MAX_PUMP;
        }
        if eng.state_dirty.replace(false) {
            eng.send_state();
        }
        if eng.out.borrow_mut().flush().is_err() {
            break;
        }
    }

    // Close every browser, let CEF finish, then shut down cleanly (flushes cookies).
    let browsers: Vec<Browser> = eng.tabs.borrow().iter().map(|t| t.browser.clone()).collect();
    eng.tabs.borrow_mut().clear();
    for b in browsers {
        if let Some(h) = b.host() {
            h.close_browser(1);
        }
    }
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        do_message_loop_work();
        std::thread::sleep(Duration::from_millis(10));
    }
    shutdown();
}

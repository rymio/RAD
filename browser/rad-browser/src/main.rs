//! RAD web browser engine: a Servo instance that renders offscreen and is driven by the RAD app.
//!
//! Protocol (the app is the only client):
//! - stdin: one JSON command per line, e.g. `{"cmd":"load","url":"https://servo.org"}`.
//! - stdout: framed messages `[kind: u8][len: u32 LE][payload]`.
//!   - kind 1: a JSON event (`state`, `cursor`, `dialog`, `select`, `eval`, …).
//!   - kind 2: a frame update: `w, h, y0, rows` (u32 LE each), then `rows` RGBA rows starting
//!     at row `y0`. Only the band of rows that changed since the previous frame is sent.
//!
//! Browsing data (cookies, storage, cache) lives in memory only and is gone when the process ends.

mod out;

use std::cell::{Cell, OnceCell, RefCell};
use std::io::BufRead;
use std::rc::{Rc, Weak};
use std::str::FromStr;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use dpi::PhysicalSize;
use euclid::Scale;
use serde_json::{json, Value};
use servo::{
    ClipboardAction, Code, CompositionEvent, CompositionState, CreateNewWebViewRequest, Cursor, DeviceIntRect, DevicePoint, EmbedderControl,
    EmbedderControlId, EventLoopWaker, ImeEvent, InputEvent, JSValue, Key, KeyState, KeyboardEvent, LoadStatus, Location, Modifiers,
    MouseButton, MouseButtonAction, MouseButtonEvent, MouseLeftViewportEvent, MouseMoveEvent, RenderingContext, SelectElementOptionOrOptgroup,
    Servo, ServoBuilder, SimpleDialog, SoftwareRenderingContext, Theme, WebView, WebViewBuilder, WebViewDelegate, WebViewPoint, WheelDelta,
    WheelEvent, WheelMode,
};
use url::Url;

use crate::out::Out;

/// At most ~60 frames per second reach the app.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

enum Msg {
    Wake,
    Cmd(Value),
    Eof,
}

#[derive(Clone)]
struct Waker(Sender<Msg>);

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {
        let _ = self.0.send(Msg::Wake);
    }
}

struct Tab {
    id: u32,
    webview: WebView,
}

/// A pending `<select>` picker or `alert`/`confirm`/`prompt`, waiting for the user's answer.
struct Pending {
    id: u64,
    servo_id: EmbedderControlId,
    control: EmbedderControl,
}

struct Browser {
    me: OnceCell<Weak<Browser>>,
    rc: Rc<SoftwareRenderingContext>,
    out: Rc<RefCell<Out>>,
    tabs: RefCell<Vec<Tab>>,
    active: Cell<u32>,
    next_tab: Cell<u32>,
    next_control: Cell<u64>,
    pending: RefCell<Vec<Pending>>,
    scale: Cell<f32>,
    frame_dirty: Cell<bool>,
    state_dirty: Cell<bool>,
    last_frame: RefCell<Vec<u8>>,
}

impl Browser {
    fn delegate(&self) -> Rc<dyn WebViewDelegate> {
        self.me.get().and_then(Weak::upgrade).expect("browser alive")
    }

    fn event(&self, v: Value) {
        self.out.borrow_mut().event(&v);
    }

    fn tab_id(&self, wv: &WebView) -> Option<u32> {
        self.tabs.borrow().iter().find(|t| t.webview.id() == wv.id()).map(|t| t.id)
    }

    fn tab(&self, id: Option<u32>) -> Option<WebView> {
        let id = id.unwrap_or(self.active.get());
        self.tabs.borrow().iter().find(|t| t.id == id).map(|t| t.webview.clone())
    }

    fn is_active(&self, wv: &WebView) -> bool {
        self.tab_id(wv) == Some(self.active.get())
    }

    fn add_tab(&self, webview: WebView) -> u32 {
        let id = self.next_tab.get();
        self.next_tab.set(id + 1);
        self.tabs.borrow_mut().push(Tab { id, webview });
        self.activate(id);
        id
    }

    fn open_tab(&self, servo: &Servo, url: Url) -> u32 {
        let wv = WebViewBuilder::new(servo, self.rc.clone())
            .url(url)
            .hidpi_scale_factor(Scale::new(self.scale.get()))
            .delegate(self.delegate())
            .build();
        self.add_tab(wv)
    }

    fn activate(&self, id: u32) {
        for t in self.tabs.borrow().iter() {
            if t.id == id {
                t.webview.show();
                t.webview.set_focused(true);
            } else {
                t.webview.hide();
            }
        }
        self.active.set(id);
        self.frame_dirty.set(true);
        self.state_dirty.set(true);
    }

    fn close_tab(&self, id: u32) {
        let next = {
            let mut tabs = self.tabs.borrow_mut();
            let Some(i) = tabs.iter().position(|t| t.id == id) else { return };
            tabs.remove(i);
            if tabs.is_empty() { None } else { Some(tabs[i.min(tabs.len() - 1)].id) }
        };
        self.pending.borrow_mut().clear();
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
                let wv = &t.webview;
                json!({
                    "id": t.id,
                    "url": wv.url().map(|u| u.to_string()).unwrap_or_default(),
                    "title": wv.page_title().unwrap_or_default(),
                    "loading": !matches!(wv.load_status(), LoadStatus::Complete),
                    "can_back": wv.can_go_back(),
                    "can_forward": wv.can_go_forward(),
                    "zoom": wv.page_zoom(),
                })
            })
            .collect();
        self.event(json!({"type": "state", "tabs": tabs, "active": self.active.get()}));
    }

    /// Paints the active tab and sends the rows that changed since the last frame.
    fn paint(&self) {
        let Some(wv) = self.tab(None) else { return };
        wv.paint();
        let size = self.rc.size();
        let rect = DeviceIntRect::from_size(euclid::Size2D::new(size.width as i32, size.height as i32));
        let Some(img) = self.rc.read_to_image(rect) else { return };
        self.rc.present();
        let (w, h) = (img.width(), img.height());
        let px = img.into_raw();
        let mut last = self.last_frame.borrow_mut();
        let stride = w as usize * 4;
        let (y0, y1) = if last.len() == px.len() {
            let row_differs = |y: usize| px[y * stride..(y + 1) * stride] != last[y * stride..(y + 1) * stride];
            let Some(first) = (0..h as usize).find(|&y| row_differs(y)) else { return };
            let end = (first..h as usize).rev().find(|&y| row_differs(y)).unwrap_or(first) + 1;
            (first, end)
        } else {
            (0, h as usize)
        };
        self.out.borrow_mut().frame(w, h, y0 as u32, &px[y0 * stride..y1 * stride]);
        *last = px;
    }

    fn resize(&self, w: u32, h: u32, scale: f32) {
        let size = PhysicalSize::new(w.max(1), h.max(1));
        self.scale.set(scale);
        // Servo resizes the shared surface and re-lays out every tab sharing it. Resizing the
        // surface ourselves first would make Servo see "no change" and keep the old layout.
        if let Some(t) = self.tabs.borrow().first() {
            t.webview.resize(size);
        }
        for t in self.tabs.borrow().iter() {
            t.webview.set_hidpi_scale_factor(Scale::new(scale));
        }
        self.last_frame.borrow_mut().clear();
        self.frame_dirty.set(true);
    }

    fn take_pending(&self, id: u64) -> Option<EmbedderControl> {
        let mut p = self.pending.borrow_mut();
        let i = p.iter().position(|x| x.id == id)?;
        Some(p.remove(i).control)
    }

    fn handle(&self, servo: &Servo, c: &Value) {
        let s = |k: &str| c[k].as_str().unwrap_or_default();
        let f = |k: &str| c[k].as_f64().unwrap_or(0.0);
        let tab_arg = c["tab"].as_u64().map(|t| t as u32);
        let point = || WebViewPoint::Device(DevicePoint::new(f("x") as f32, f("y") as f32));
        match s("cmd") {
            "new_tab" => {
                let url = Url::parse(s("url")).unwrap_or_else(|_| Url::parse("about:blank").unwrap());
                self.open_tab(servo, url);
            }
            "close_tab" => self.close_tab(tab_arg.unwrap_or(self.active.get())),
            "activate" => {
                if let Some(t) = tab_arg {
                    self.activate(t);
                }
            }
            "load" => match (Url::parse(s("url")), self.tab(tab_arg)) {
                (Ok(url), Some(wv)) => wv.load(url),
                (Ok(url), None) => {
                    self.open_tab(servo, url);
                }
                (Err(e), _) => self.event(json!({"type": "error", "message": format!("Invalid address: {e}")})),
            },
            "back" => {
                if let Some(wv) = self.tab(tab_arg).filter(|w| w.can_go_back()) {
                    wv.go_back(1);
                }
            }
            "forward" => {
                if let Some(wv) = self.tab(tab_arg).filter(|w| w.can_go_forward()) {
                    wv.go_forward(1);
                }
            }
            "reload" => {
                if let Some(wv) = self.tab(tab_arg) {
                    wv.reload();
                }
            }
            "zoom" => {
                if let Some(wv) = self.tab(tab_arg) {
                    wv.set_page_zoom(f("level").clamp(0.3, 5.0) as f32);
                    self.state_dirty.set(true);
                }
            }
            "theme" => {
                let theme = if s("theme") == "dark" { Theme::Dark } else { Theme::Light };
                for t in self.tabs.borrow().iter() {
                    t.webview.notify_theme_change(theme);
                }
            }
            "resize" => self.resize(f("w") as u32, f("h") as u32, (f("scale") as f32).max(0.5)),
            "repaint" => {
                self.last_frame.borrow_mut().clear();
                self.frame_dirty.set(true);
            }
            "mouse_move" => {
                if let Some(wv) = self.tab(None) {
                    wv.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(point())));
                }
            }
            "mouse_leave" => {
                if let Some(wv) = self.tab(None) {
                    wv.notify_input_event(InputEvent::MouseLeftViewport(MouseLeftViewportEvent::default()));
                }
            }
            "mouse_button" => {
                if let Some(wv) = self.tab(None) {
                    let action = if s("action") == "up" { MouseButtonAction::Up } else { MouseButtonAction::Down };
                    let button = MouseButton::from(c["button"].as_i64().unwrap_or(0));
                    if action == MouseButtonAction::Down {
                        wv.set_focused(true);
                    }
                    wv.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(action, button, point())));
                }
            }
            "wheel" => {
                if let Some(wv) = self.tab(None) {
                    let mode = match c["mode"].as_u64() {
                        Some(1) => WheelMode::DeltaLine,
                        Some(2) => WheelMode::DeltaPage,
                        _ => WheelMode::DeltaPixel,
                    };
                    // DOM deltas are positive when scrolling down; Servo's wheel deltas the opposite.
                    let delta = WheelDelta { x: -f("dx"), y: -f("dy"), z: 0.0, mode };
                    wv.notify_input_event(InputEvent::Wheel(WheelEvent::new(delta, point())));
                }
            }
            "key" => {
                if let Some(wv) = self.tab(None) {
                    let state = if s("state") == "up" { KeyState::Up } else { KeyState::Down };
                    let key = Key::from_str(s("key")).unwrap_or_else(|_| Key::Character(s("key").to_string()));
                    let code = Code::from_str(s("code")).unwrap_or(Code::Unidentified);
                    let location = match c["location"].as_u64() {
                        Some(1) => Location::Left,
                        Some(2) => Location::Right,
                        Some(3) => Location::Numpad,
                        _ => Location::Standard,
                    };
                    let mut mods = Modifiers::empty();
                    for (k, m) in [("shift", Modifiers::SHIFT), ("ctrl", Modifiers::CONTROL), ("alt", Modifiers::ALT), ("meta", Modifiers::META)] {
                        if c[k].as_bool().unwrap_or(false) {
                            mods |= m;
                        }
                    }
                    let repeat = c["repeat"].as_bool().unwrap_or(false);
                    wv.notify_input_event(InputEvent::Keyboard(KeyboardEvent::new_without_event(state, key, code, location, mods, repeat, false)));
                }
            }
            "text" => {
                // Committed text from an input method (accents, CJK, emoji picker…).
                if let Some(wv) = self.tab(None) {
                    let ev = CompositionEvent { state: CompositionState::End, data: s("text").to_string() };
                    wv.notify_input_event(InputEvent::Ime(ImeEvent::Composition(ev)));
                }
            }
            "clipboard" => {
                if let Some(wv) = self.tab(None) {
                    let action = match s("action") {
                        "cut" => ClipboardAction::Cut,
                        "paste" => ClipboardAction::Paste,
                        _ => ClipboardAction::Copy,
                    };
                    wv.notify_input_event(InputEvent::EditingAction(action));
                }
            }
            "eval" => {
                let id = c["id"].clone();
                let Some(wv) = self.tab(tab_arg) else {
                    self.event(json!({"type": "eval", "id": id, "ok": false, "error": "no open tab"}));
                    return;
                };
                let out = self.out.clone();
                wv.evaluate_javascript(s("script"), move |r| {
                    let v = match r {
                        Ok(v) => json!({"type": "eval", "id": id, "ok": true, "value": js_to_json(v)}),
                        Err(e) => json!({"type": "eval", "id": id, "ok": false, "error": format!("{e:?}")}),
                    };
                    out.borrow_mut().event(&v);
                });
            }
            "dialog_reply" => {
                if let Some(EmbedderControl::SimpleDialog(d)) = self.take_pending(c["control"].as_u64().unwrap_or(0)) {
                    let ok = c["ok"].as_bool().unwrap_or(false);
                    match d {
                        SimpleDialog::Prompt(mut p) if ok => {
                            p.set_current_value(s("value"));
                            p.confirm();
                        }
                        d if ok => d.confirm(),
                        d => d.dismiss(),
                    }
                }
            }
            "select_reply" => {
                if let Some(EmbedderControl::SelectElement(mut sel)) = self.take_pending(c["control"].as_u64().unwrap_or(0)) {
                    if let Some(i) = c["option"].as_u64() {
                        sel.select(vec![i as usize]);
                    }
                    sel.submit();
                }
            }
            other => log::warn!("unknown command {other:?}"),
        }
    }
}

impl WebViewDelegate for Browser {
    fn notify_new_frame_ready(&self, wv: WebView) {
        if self.is_active(&wv) {
            self.frame_dirty.set(true);
        }
    }
    fn notify_url_changed(&self, _: WebView, _: Url) {
        self.state_dirty.set(true);
    }
    fn notify_page_title_changed(&self, _: WebView, _: Option<String>) {
        self.state_dirty.set(true);
    }
    fn notify_load_status_changed(&self, _: WebView, _: LoadStatus) {
        self.state_dirty.set(true);
    }
    fn notify_history_changed(&self, _: WebView, _: Vec<Url>, _: usize) {
        self.state_dirty.set(true);
    }
    fn notify_status_text_changed(&self, wv: WebView, status: Option<String>) {
        if self.is_active(&wv) {
            self.event(json!({"type": "status", "text": status}));
        }
    }
    fn notify_cursor_changed(&self, wv: WebView, cursor: Cursor) {
        if self.is_active(&wv) {
            self.event(json!({"type": "cursor", "cursor": css_cursor(cursor)}));
        }
    }
    fn notify_closed(&self, wv: WebView) {
        if let Some(id) = self.tab_id(&wv) {
            self.close_tab(id);
        }
    }
    fn notify_crashed(&self, wv: WebView, reason: String, _backtrace: Option<String>) {
        self.event(json!({"type": "crashed", "tab": self.tab_id(&wv), "reason": reason}));
    }
    fn request_create_new(&self, _parent: WebView, request: CreateNewWebViewRequest) {
        // window.open / target=_blank: open a new tab.
        let wv = request.builder(self.rc.clone()).hidpi_scale_factor(Scale::new(self.scale.get())).delegate(self.delegate()).build();
        self.add_tab(wv);
    }
    fn show_embedder_control(&self, wv: WebView, control: EmbedderControl) {
        let id = self.next_control.get();
        self.next_control.set(id + 1);
        let servo_id = control.id();
        let tab = self.tab_id(&wv);
        let ev = match &control {
            EmbedderControl::SimpleDialog(d) => {
                let (kind, default) = match d {
                    SimpleDialog::Alert(_) => ("alert", String::new()),
                    SimpleDialog::Confirm(_) => ("confirm", String::new()),
                    SimpleDialog::Prompt(p) => ("prompt", p.current_value().to_string()),
                };
                json!({"type": "dialog", "control": id, "tab": tab, "kind": kind, "message": d.message(), "default": default})
            }
            EmbedderControl::SelectElement(sel) => {
                let mut options = Vec::new();
                for o in sel.options() {
                    match o {
                        SelectElementOptionOrOptgroup::Option(o) => options.push(json!({"id": o.id, "label": o.label, "disabled": o.is_disabled})),
                        SelectElementOptionOrOptgroup::Optgroup { label, options: group } => {
                            options.push(json!({"group": label}));
                            for o in group {
                                options.push(json!({"id": o.id, "label": o.label, "disabled": o.is_disabled}));
                            }
                        }
                    }
                }
                let r = sel.position();
                json!({
                    "type": "select", "control": id, "tab": tab, "options": options, "selected": sel.selected_options(),
                    "x": r.min.x, "y": r.min.y, "w": r.width(), "h": r.height(),
                })
            }
            // File and colour pickers, context menus and IME panels are dismissed by dropping them.
            _ => return,
        };
        self.pending.borrow_mut().push(Pending { id, servo_id, control });
        self.event(ev);
    }
    fn hide_embedder_control(&self, _: WebView, control_id: EmbedderControlId) {
        let mut p = self.pending.borrow_mut();
        if let Some(i) = p.iter().position(|x| x.servo_id == control_id) {
            let gone = p.remove(i);
            self.event(json!({"type": "control_hidden", "control": gone.id}));
        }
    }
}

fn css_cursor(c: Cursor) -> String {
    let name = format!("{c:?}");
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('-');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn js_to_json(v: JSValue) -> Value {
    match v {
        JSValue::Undefined | JSValue::Null => Value::Null,
        JSValue::Boolean(b) => Value::Bool(b),
        JSValue::Number(n) => json!(n),
        JSValue::String(s) => Value::String(s),
        JSValue::Element(s) | JSValue::ShadowRoot(s) | JSValue::Frame(s) | JSValue::Window(s) => Value::String(s),
        JSValue::Array(a) => Value::Array(a.into_iter().map(js_to_json).collect()),
        JSValue::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, js_to_json(v))).collect()),
    }
}

fn arg(name: &str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(a) = args.next() {
        if a == name {
            return args.next();
        }
    }
    None
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    // Servo and its libraries may print to stdout; keep the protocol stream for ourselves.
    let out = Rc::new(RefCell::new(Out::take_stdout()));
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let width: u32 = arg("--width").and_then(|v| v.parse().ok()).unwrap_or(1280);
    let height: u32 = arg("--height").and_then(|v| v.parse().ok()).unwrap_or(800);
    let scale: f32 = arg("--scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let start = arg("--url").and_then(|u| Url::parse(&u).ok()).unwrap_or_else(|| Url::parse("https://servo.org").unwrap());

    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = crossbeam_channel::unbounded();
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

    let rc = match SoftwareRenderingContext::new(PhysicalSize::new(width.max(1), height.max(1))) {
        Ok(rc) => Rc::new(rc),
        Err(e) => {
            out.borrow_mut().event(&json!({"type": "fatal", "message": format!("Couldn't create a rendering surface: {e:?}")}));
            std::process::exit(2);
        }
    };
    let _ = rc.make_current();
    let servo = ServoBuilder::default().event_loop_waker(Box::new(Waker(tx.clone()))).build();

    let browser = Rc::new(Browser {
        me: OnceCell::new(),
        rc,
        out: out.clone(),
        tabs: RefCell::new(Vec::new()),
        active: Cell::new(0),
        next_tab: Cell::new(1),
        next_control: Cell::new(1),
        pending: RefCell::new(Vec::new()),
        scale: Cell::new(scale),
        frame_dirty: Cell::new(false),
        state_dirty: Cell::new(true),
        last_frame: RefCell::new(Vec::new()),
    });
    let _ = browser.me.set(Rc::downgrade(&browser));
    browser.open_tab(&servo, start);
    out.borrow_mut().event(&json!({"type": "ready", "engine": "servo", "version": env!("CARGO_PKG_VERSION")}));

    let mut last_paint = Instant::now() - FRAME_INTERVAL;
    loop {
        let wait = if browser.frame_dirty.get() { FRAME_INTERVAL.saturating_sub(last_paint.elapsed()) } else { Duration::from_millis(250) };
        let mut msgs = Vec::new();
        match rx.recv_timeout(wait) {
            Ok(m) => msgs.push(m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        msgs.extend(rx.try_iter());
        for m in msgs {
            match m {
                Msg::Wake => {}
                Msg::Cmd(c) => browser.handle(&servo, &c),
                Msg::Eof => {
                    // The app went away: shut down quietly.
                    std::process::exit(0);
                }
            }
        }
        servo.spin_event_loop();
        if browser.frame_dirty.get() && last_paint.elapsed() >= FRAME_INTERVAL {
            browser.frame_dirty.set(false);
            last_paint = Instant::now();
            browser.paint();
        }
        if browser.state_dirty.replace(false) {
            browser.send_state();
        }
        if out.borrow_mut().flush().is_err() {
            break;
        }
    }
}

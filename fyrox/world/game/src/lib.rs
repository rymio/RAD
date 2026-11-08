//! Gilgamesh's world: a small 2D land by a river where Gilgamesh lives, and the owner visits him
//! as a traveller. The owner walks (arrows or WASD), talks with him (Enter when close), and
//! builds (B). Gilgamesh wanders, sleeps at night, and builds when he decides to; what he says
//! and decides comes from RAD (he is RAD's Gilgamesh), over stdin/stdout (`proto`).
//!
//! The world is a Fyrox scene (`world.rgs`, made by `world::fresh` the first time), saved
//! whenever something is built, so it can also be shaped in the Fyrox editor.

#[allow(unused_imports)]
use fyrox::graph::prelude::*;
use fyrox::{
    core::{
        algebra::{Vector2, Vector3},
        color::Color,
        pool::Handle,
        reflect::prelude::*,
        visitor::prelude::*,
    },
    event::{ElementState, Event, WindowEvent},
    graph::SceneGraph,
    gui::{
        border::BorderBuilder,
        brush::Brush,
        message::UiMessage,
        text::{Text, TextBuilder, TextMessage},
        text_box::{TextBox, TextBoxBuilder, TextCommitMode},
        widget::{WidgetBuilder, WidgetMessage},
        formatted_text::WrapMode,
        Thickness, UiNode, UserInterface,
    },
    keyboard::{KeyCode, PhysicalKey},
    material::MaterialResource,
    engine::GraphicsContext,
    plugin::{error::GameResult, Plugin, PluginContext, PluginRegistrationContext},
    scene::{camera::Camera, dim2::rectangle::Rectangle, node::Node, Scene},
};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

pub mod art;
pub mod catalog;
pub mod nav;
pub mod proto;
pub mod world;

use nav::{Area, Blocks};
use proto::Cmd;

// Re-export the engine.
pub use fyrox;

const WALK: f32 = 4.0;
const GIL_WALK: f32 = 2.6;
const TALK_RANGE: f32 = 2.2;

/// Where the world is kept: `--world <file>` (RAD passes one in its data folder), else the
/// project's own `data/world.rgs`.
pub fn world_path() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == "--world").and_then(|i| args.get(i + 1)).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("data/world.rgs"))
}

pub fn generate_only() -> bool {
    std::env::args().any(|a| a == "--generate")
}

#[derive(Default, Visit, Reflect, Debug)]
#[reflect(non_cloneable)]
pub struct Game {
    #[visit(skip)]
    #[reflect(hidden)]
    live: Option<Box<Live>>,
}

/// Something being built: it grows out of the ground.
struct Growing {
    node: Handle<Rectangle>,
    t: f32,
    full: Vector3<f32>,
}

#[derive(Clone, Debug, PartialEq)]
enum Doing {
    Idle(f32),
    Walking,
    /// Walking to build this (kind id, name, spot, who asked).
    ToBuild { kind: &'static str, name: String, at: (f32, f32), by: String },
    Building(f32),
    Talking,
    Sleeping,
}

struct Chars {
    gil: Handle<Node>,
    you: Handle<Node>,
    gil_body: Handle<Node>,
    you_body: Handle<Node>,
    face: Handle<Node>,
}

struct Ui {
    bubble: Handle<UiNode>,
    bubble_text: Handle<Text>,
    hint: Handle<Text>,
    label: Handle<Text>,
    log: Handle<Text>,
    log_box: Handle<UiNode>,
    input: Handle<TextBox>,
    menu: Handle<Text>,
    menu_box: Handle<UiNode>,
    tablet: Handle<Text>,
    tablet_box: Handle<UiNode>,
}

pub struct Live {
    rx: Receiver<Cmd>,
    path: PathBuf,
    scene: Handle<Scene>,
    camera: Handle<Node>,
    things: Handle<Node>,
    chars: Option<Chars>,
    ui: Option<Ui>,
    blocks: Blocks,
    keys: HashSet<KeyCode>,
    player_name: String,
    hour: f32,
    // Gilgamesh
    doing: Doing,
    path_ahead: Vec<(f32, f32)>,
    jobs: Vec<Doing>,
    phase: String,
    bubble: String,
    bubble_until: f32,
    facing_gil: f32,
    facing_you: f32,
    frames_gil: Vec<MaterialResource>,
    frames_you: Vec<MaterialResource>,
    growing: Vec<Growing>,
    // The owner
    chat_open: bool,
    near: bool,
    menu_open: bool,
    menu_at: usize,
    log: Vec<String>,
    tasks: Vec<String>,
    // Housekeeping
    time: f32,
    dirty: Option<f32>,
    state_at: f32,
    shot: Option<(String, u32)>,
    rng: art::Rng,
}

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Live")
    }
}

fn xy(v: Vector3<f32>) -> (f32, f32) {
    (v.x, v.y)
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

impl Plugin for Game {
    fn register(&self, _context: PluginRegistrationContext) -> GameResult {
        Ok(())
    }

    fn init(&mut self, _scene_path: Option<&str>, mut ctx: PluginContext) -> GameResult {
        let path = world_path();
        let mut live = Box::new(Live {
            rx: proto::listen(),
            path: path.clone(),
            scene: Handle::NONE,
            camera: Handle::NONE,
            things: Handle::NONE,
            chars: None,
            ui: None,
            blocks: Blocks::default(),
            keys: HashSet::new(),
            player_name: "You".into(),
            hour: 12.0,
            doing: Doing::Idle(2.0),
            path_ahead: Vec::new(),
            jobs: Vec::new(),
            phase: "idle".into(),
            bubble: String::new(),
            bubble_until: 0.0,
            facing_gil: 1.0,
            facing_you: 1.0,
            frames_gil: (0..2).map(|f| world::material(world::texture(&art::gilgamesh(f), false))).collect(),
            frames_you: (0..2).map(|f| world::material(world::texture(&art::traveller(f), false))).collect(),
            growing: Vec::new(),
            chat_open: false,
            near: false,
            menu_open: false,
            menu_at: 0,
            log: Vec::new(),
            tasks: Vec::new(),
            time: 0.0,
            dirty: None,
            state_at: 0.0,
            shot: None,
            rng: art::Rng::new(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(7)),
        });
        if path.exists() && !generate_only() {
            self.live = Some(live);
            ctx.load_scene(path, false, |result, game: &mut Game, ctx| {
                let scene = result?.payload;
                let h = ctx.scenes.add(scene);
                game.ready(h, ctx);
                Ok(())
            });
        } else {
            let h = ctx.scenes.add(world::fresh());
            live.scene = h;
            self.live = Some(live);
            self.ready(h, &mut ctx);
            if let Some(l) = self.live.as_mut() {
                l.save(&mut ctx);
            }
            if generate_only() {
                proto::say(&json!({ "type": "generated", "path": path.to_string_lossy() }));
                ctx.loop_controller.exit();
            }
        }
        Ok(())
    }

    fn update(&mut self, ctx: &mut PluginContext) -> GameResult {
        if let Some(l) = self.live.as_mut() {
            if l.scene.is_some() && l.chars.is_some() {
                l.update(ctx);
            }
        }
        Ok(())
    }

    fn on_os_event(&mut self, event: &Event<()>, ctx: PluginContext) -> GameResult {
        let Some(l) = self.live.as_mut() else { return Ok(()) };
        if let Event::WindowEvent { event: WindowEvent::KeyboardInput { event: key, .. }, .. } = event {
            if let PhysicalKey::Code(code) = key.physical_key {
                if key.state == ElementState::Pressed {
                    if !key.repeat {
                        l.key(code, ctx);
                    }
                    l.keys.insert(code);
                } else {
                    l.keys.remove(&code);
                }
            }
        }
        if let Event::WindowEvent { event: WindowEvent::Focused(false), .. } = event {
            l.keys.clear();
        }
        Ok(())
    }

    fn on_ui_message(&mut self, ctx: &mut PluginContext, message: &UiMessage, _ui: Handle<UserInterface>) -> GameResult {
        let Some(l) = self.live.as_mut() else { return Ok(()) };
        let Some(ui) = l.ui.as_ref() else { return Ok(()) };
        if let Some(TextMessage::Text(t)) = message.data_from::<TextMessage>(ui.input) {
            let t = t.trim().to_string();
            if !t.is_empty() && l.chat_open {
                let input = ui.input;
                ctx.user_interfaces.first_mut().send(input, TextMessage::Text(String::new()));
                l.add_log(format!("{}: {t}", l.player_name));
                proto::say(&json!({ "type": "chat", "text": t }));
            }
        }
        Ok(())
    }

    fn on_deinit(&mut self, mut ctx: PluginContext) -> GameResult {
        if let Some(l) = self.live.as_mut() {
            if l.scene.is_some() && l.dirty.is_some() {
                l.save(&mut ctx);
            }
        }
        Ok(())
    }
}

impl Game {
    fn ready(&mut self, h: Handle<Scene>, ctx: &mut PluginContext) {
        let Some(l) = self.live.as_mut() else { return };
        l.scene = h;
        l.attach(ctx);
        proto::say(&json!({ "type": "ready", "path": l.path.to_string_lossy() }));
    }
}

impl Live {
    fn graph<'a>(&self, ctx: &'a mut PluginContext) -> &'a mut fyrox::scene::graph::Graph {
        &mut ctx.scenes[self.scene].graph
    }

    /// Finds the world's parts in the scene (a fresh one or a saved one) and builds the UI.
    fn attach(&mut self, ctx: &mut PluginContext) {
        let g = self.graph(ctx);
        let find_tag = |g: &fyrox::scene::graph::Graph, t: &str| g.pair_iter().find(|(_, n)| n.tag() == t).map(|(h, _)| h).unwrap_or_default();
        let find_name = |g: &fyrox::scene::graph::Graph, t: &str| g.pair_iter().find(|(_, n)| n.name() == t).map(|(h, _)| h).unwrap_or_default();
        self.camera = g.pair_iter().find(|(_, n)| n.cast::<Camera>().is_some()).map(|(h, _)| h).unwrap_or_default();
        self.things = find_name(g, "Things");
        if self.things.is_none() {
            self.things = fyrox::scene::pivot::PivotBuilder::new(fyrox::scene::base::BaseBuilder::new().with_name("Things")).build(g).transmute();
        }
        let mut gil = find_tag(g, "character:gilgamesh");
        if gil.is_none() {
            gil = world::character(g, "Gilgamesh", "character:gilgamesh", &art::gilgamesh(0), world::GILGAMESH_START);
        }
        let mut you = find_tag(g, "character:you");
        if you.is_none() {
            you = world::character(g, "You", "character:you", &art::traveller(0), world::YOU_START);
        }
        let body_of = |g: &fyrox::scene::graph::Graph, h: Handle<Node>| g[h].children().iter().copied().find(|c| g[*c].name() == "Body").unwrap_or_default();
        let (gil_body, you_body) = (body_of(g, gil), body_of(g, you));
        let face = g[gil].children().iter().copied().find(|c| g[*c].name() == "Face").unwrap_or_default();
        self.chars = Some(Chars { gil, you, gil_body, you_body, face });
        self.rebuild_blocks(ctx);
        // Making a world file needs no window or UI.
        if !generate_only() {
            self.build_ui(ctx);
        }
    }

    /// What can't be walked through, from the scene: things' footprints, the river, bridges.
    fn rebuild_blocks(&mut self, ctx: &mut PluginContext) {
        let g = &ctx.scenes[self.scene].graph;
        let mut b = Blocks { w: world::W, h: world::H, ..Default::default() };
        for (_, n) in g.pair_iter() {
            let tag = n.tag();
            let p = n.global_position();
            let s = n.local_transform().scale();
            let area = Area::new(p.x - s.x / 2.0, p.y - s.y / 2.0, p.x + s.x / 2.0, p.y + s.y / 2.0);
            if tag == "water" {
                b.water.push(area);
            } else if tag == "bridge" {
                b.bridges.push(area);
            } else if tag == "path" {
                b.keep_clear.push(area);
            } else if let Some(rest) = tag.strip_prefix("thing:") {
                let id = rest.split(':').next().unwrap_or("");
                if let Some(k) = catalog::by_id(id) {
                    let bottom = p.y - s.y / 2.0;
                    if k.solid_w > 0.0 {
                        b.solid.push(Area::new(p.x - k.solid_w / 2.0, bottom, p.x + k.solid_w / 2.0, bottom + k.solid_d));
                    }
                    // The way in to anything with a door stays open.
                    if ["house", "workshop", "ziggurat", "gate", "tent", "tower"].contains(&id) {
                        b.keep_clear.push(Area::new(p.x - 1.0, bottom - 1.8, p.x + 1.0, bottom));
                    }
                }
            }
        }
        self.blocks = b;
    }

    fn build_ui(&mut self, ctx: &mut PluginContext) {
        // Fyrox starts with no user interface: the world has one of its own.
        if ctx.user_interfaces.iter().next().is_none() {
            ctx.user_interfaces.add(UserInterface::new(Vector2::new(1280.0, 800.0)));
        }
        let ui = ctx.user_interfaces.first_mut();
        let bctx = &mut ui.build_ctx();
        let panel = |c: Color| Brush::Solid(c).into();
        let text = |bctx: &mut fyrox::gui::BuildContext, size: f32, wrap: bool| {
            TextBuilder::new(WidgetBuilder::new().with_foreground(Brush::Solid(Color::opaque(40, 30, 24)).into()).with_margin(Thickness::uniform(8.0)))
                .with_font_size(size.into())
                .with_wrap(if wrap { WrapMode::Word } else { WrapMode::NoWrap })
                .build(bctx)
        };
        let bubble_text = text(bctx, 17.0, true);
        let bubble = BorderBuilder::new(WidgetBuilder::new().with_visibility(false).with_max_size(Vector2::new(340.0, 400.0)).with_background(panel(Color::from_rgba(255, 250, 236, 240))).with_foreground(Brush::Solid(Color::opaque(90, 70, 50)).into()).with_child(bubble_text))
            .with_corner_radius(10.0f32.into())
            .with_stroke_thickness(Thickness::uniform(2.0).into())
            .build(bctx)
            .transmute();
        let hint = TextBuilder::new(WidgetBuilder::new().with_visibility(false).with_foreground(Brush::Solid(Color::WHITE).into())).with_font_size(16.0f32.into()).with_shadow(true).build(bctx);
        let label = TextBuilder::new(WidgetBuilder::new().with_visibility(false).with_foreground(Brush::Solid(Color::WHITE).into())).with_font_size(14.0f32.into()).with_shadow(true).build(bctx);
        let log = text(bctx, 15.0, true);
        let input = TextBoxBuilder::new(WidgetBuilder::new().with_height(32.0).with_margin(Thickness::uniform(4.0)).with_background(panel(Color::opaque(255, 255, 255))).with_foreground(Brush::Solid(Color::opaque(30, 30, 30)).into()))
            .with_text_commit_mode(TextCommitMode::LostFocusPlusEnter)
            .with_font_size(16.0f32.into())
            .build(bctx);
        let log_box = BorderBuilder::new(
            WidgetBuilder::new()
                .with_visibility(false)
                .with_width(520.0)
                .with_background(panel(Color::from_rgba(255, 250, 236, 230)))
                .with_child(fyrox::gui::stack_panel::StackPanelBuilder::new(WidgetBuilder::new().with_child(log).with_child(input)).build(bctx)),
        )
        .with_corner_radius(10.0f32.into())
        .build(bctx)
        .transmute();
        let menu = text(bctx, 15.0, false);
        let menu_box = BorderBuilder::new(WidgetBuilder::new().with_visibility(false).with_background(panel(Color::from_rgba(255, 250, 236, 235))).with_child(menu)).with_corner_radius(10.0f32.into()).build(bctx).transmute();
        let tablet = text(bctx, 15.0, true);
        let tablet_box = BorderBuilder::new(WidgetBuilder::new().with_visibility(false).with_width(320.0).with_background(panel(Color::from_rgba(214, 180, 128, 240))).with_child(tablet)).with_corner_radius(6.0f32.into()).build(bctx).transmute();
        self.ui = Some(Ui { bubble, bubble_text, hint, label, log, log_box, input, menu, menu_box, tablet, tablet_box });
    }

    fn add_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > 8 {
            self.log.remove(0);
        }
    }

    // ---------------------------------------------------------------- keys

    fn key(&mut self, code: KeyCode, mut ctx: PluginContext) {
        self.key_in(code, &mut ctx);
    }

    fn key_in(&mut self, code: KeyCode, ctx: &mut PluginContext) {
        let Some(ui) = self.ui.as_ref() else { return };
        let (input, ui_root) = (ui.input, ());
        let _ = ui_root;
        if self.chat_open {
            if code == KeyCode::Escape {
                self.close_chat(ctx);
            }
            return;
        }
        if self.menu_open {
            match code {
                KeyCode::Escape | KeyCode::KeyB => self.menu_open = false,
                KeyCode::ArrowUp | KeyCode::KeyW => self.menu_at = (self.menu_at + catalog::KINDS.len() - 1) % catalog::KINDS.len(),
                KeyCode::ArrowDown | KeyCode::KeyS => self.menu_at = (self.menu_at + 1) % catalog::KINDS.len(),
                KeyCode::Enter | KeyCode::Space => {
                    let k = &catalog::KINDS[self.menu_at];
                    self.menu_open = false;
                    self.build_by_you(k, ctx);
                }
                _ => {}
            }
            self.keys.clear();
            return;
        }
        match code {
            KeyCode::Enter | KeyCode::KeyE if self.near => {
                self.chat_open = true;
                self.keys.clear();
                let ui = ctx.user_interfaces.first_mut();
                ui.send(input, WidgetMessage::Focus);
                proto::say(&json!({ "type": "meet" }));
                if self.log.is_empty() {
                    self.add_log(format!("You meet Gilgamesh. Say something, then Enter. (Esc to stop talking)"));
                }
            }
            KeyCode::KeyB => {
                self.menu_open = true;
            }
            _ => {}
        }
    }

    fn close_chat(&mut self, ctx: &mut PluginContext) {
        self.chat_open = false;
        if let Some(ui) = self.ui.as_ref() {
            ctx.user_interfaces.first_mut().send(ui.input, WidgetMessage::Unfocus);
        }
        proto::say(&json!({ "type": "leave" }));
    }

    // ---------------------------------------------------------------- RAD

    fn command(&mut self, cmd: Cmd, ctx: &mut PluginContext) {
        match cmd {
            Cmd::Hello { player, face, hour } => {
                if let Some(p) = player.filter(|p| !p.trim().is_empty()) {
                    self.player_name = p;
                }
                if let Some(h) = hour {
                    self.hour = h;
                }
                if let Some(f) = face {
                    self.set_face(&f, ctx);
                }
            }
            Cmd::Face { path } => self.set_face(&path, ctx),
            Cmd::Clock { hour } => self.hour = hour,
            Cmd::Phase { phase, .. } => {
                if phase == "thinking" && self.bubble_until < self.time {
                    self.show_bubble("…", 30.0);
                }
                if phase == "idle" && self.bubble == "…" {
                    self.bubble_until = self.time;
                }
                self.phase = phase;
            }
            Cmd::Say { text } => {
                let t = text.trim();
                if t.is_empty() {
                    return;
                }
                // Sentences add up while he speaks; a new answer starts a new bubble.
                let next = if self.bubble_until > self.time && self.bubble != "…" && self.phase == "speaking" { format!("{} {t}", self.bubble) } else { t.to_string() };
                let len = next.len();
                let keep: String = if len > 420 { next.chars().skip(next.chars().count() - 400).collect() } else { next };
                self.show_bubble(&keep, 4.0 + t.len() as f32 * 0.06);
                self.add_log(format!("Gilgamesh: {t}"));
            }
            Cmd::Heard { text } => self.add_log(format!("{}: {text}", self.player_name)),
            Cmd::Build { what, near, name, by } => {
                let (k, known) = catalog::find(&what);
                // A tent stands in for many things (a library, a school): it keeps the name asked for.
                let generic = !known || (k.id == "tent" && !what.to_lowercase().contains("tent"));
                let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| if generic { title_case(&what) } else { k.title.to_string() });
                let by = by.unwrap_or_else(|| "gilgamesh".into());
                match self.spot_for(k, near.as_deref(), ctx) {
                    Some(at) => {
                        self.jobs.push(Doing::ToBuild { kind: k.id, name: name.clone(), at, by });
                        proto::say(&json!({ "type": "building", "name": name, "kind": k.id, "x": at.0, "y": at.1 }));
                    }
                    None => proto::say(&json!({ "type": "error", "message": format!("No room for {name} there") })),
                }
            }
            Cmd::Goto { place } => {
                if let Some(p) = self.place_named(&place, ctx) {
                    self.walk_to(p, ctx);
                }
            }
            Cmd::Remove { name } => {
                let g = self.graph(ctx);
                let w = name.to_lowercase();
                let hit = g.pair_iter().filter(|(_, n)| n.tag().starts_with("thing:")).find(|(_, n)| n.name().to_lowercase() == w).map(|(h, _)| h);
                match hit {
                    Some(h) => {
                        g.remove_node(h);
                        self.rebuild_blocks(ctx);
                        self.dirty = Some(self.time);
                        proto::say(&json!({ "type": "removed", "name": name }));
                    }
                    None => proto::say(&json!({ "type": "error", "message": format!("Nothing called {name}") })),
                }
            }
            Cmd::Tasks { items } => self.tasks = items,
            Cmd::Shot { path } => self.shot = Some((path, 30)),
            Cmd::Key { code } => {
                let code = match code.as_str() {
                    "Enter" => KeyCode::Enter,
                    "Escape" => KeyCode::Escape,
                    "KeyB" => KeyCode::KeyB,
                    "ArrowDown" => KeyCode::ArrowDown,
                    "ArrowUp" => KeyCode::ArrowUp,
                    _ => return,
                };
                self.key_in(code, ctx);
            }
            Cmd::Type { text } => {
                if let Some(ui) = self.ui.as_ref() {
                    use fyrox::gui::message::MessageDirection;
                    let msg = UiMessage::with_data(TextMessage::Text(text)).with_destination(ui.input.transmute()).with_direction(MessageDirection::FromWidget);
                    ctx.user_interfaces.first_mut().send_message(msg);
                }
            }
            Cmd::Ui => self.report_ui(ctx),
            Cmd::Save => self.save(ctx),
            Cmd::Quit => {
                self.save(ctx);
                ctx.loop_controller.exit();
            }
        }
    }

    fn show_bubble(&mut self, text: &str, secs: f32) {
        self.bubble = text.to_string();
        self.bubble_until = self.time + secs.min(25.0);
    }

    fn set_face(&mut self, path: &str, ctx: &mut PluginContext) {
        // A PNG or JPEG, whatever its name says.
        let Ok(img) = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| image::load_from_memory(&b).map_err(|e| e.to_string())) else {
            proto::say(&json!({ "type": "error", "message": format!("Couldn't read the face {path}") }));
            return;
        };
        let rgba = img.to_rgba8();
        let face = art::face_from(rgba.as_raw(), rgba.width(), rgba.height());
        let mat = world::material(world::texture(&face, false));
        let Some(ch) = self.chars.as_ref() else { return };
        let (gil, old) = (ch.gil, ch.face);
        let g = self.graph(ctx);
        if old.is_some() {
            g.remove_node(old);
        }
        let f = world::sprite(g, "Face", "", mat, 0.03, 1.02, 0.42, 0.42, -0.001);
        g.link_nodes(f, gil);
        if let Some(ch) = self.chars.as_mut() {
            ch.face = f.transmute();
        }
    }

    // ---------------------------------------------------------------- places and building

    fn pos(&self, h: Handle<Node>, ctx: &mut PluginContext) -> (f32, f32) {
        xy(ctx.scenes[self.scene].graph[h].local_transform().position().clone_owned())
    }

    /// A place by name: a thing ("house", "the well", "Kitchen garden"), "you", "here"/"me".
    fn place_named(&self, name: &str, ctx: &mut PluginContext) -> Option<(f32, f32)> {
        let ch = self.chars.as_ref()?;
        let w = name.trim().to_lowercase();
        let w = w.trim_start_matches("the ").trim_start_matches("my ").trim_start_matches("your ");
        if w.is_empty() || ["here", "me", "gilgamesh", "yourself", "you"].contains(&w) {
            return Some(self.pos(ch.gil, ctx));
        }
        if ["player", "owner", "traveller", "them", "visitor"].contains(&w) || w == self.player_name.to_lowercase() {
            return Some(self.pos(ch.you, ctx));
        }
        if w.contains("river") || w.contains("water") || w.contains("bank") {
            return Some((world::RIVER.0 - 1.5, 18.0));
        }
        if w.contains("bridge") {
            return Some((world::RIVER.0 - 1.5, 15.2));
        }
        let g = &ctx.scenes[self.scene].graph;
        let things: Vec<(String, String, (f32, f32))> = g.pair_iter().filter_map(|(_, n)| n.tag().strip_prefix("thing:").map(|t| (n.name().to_lowercase(), t.split(':').next().unwrap_or("").to_string(), xy(n.global_position()) ))).collect();
        let hit = things.iter().find(|(n, _, _)| n == w).or_else(|| things.iter().find(|(n, _, _)| n.contains(w) || w.contains(n.as_str()))).or_else(|| {
            let (k, known) = catalog::find(w);
            if known { things.iter().find(|(_, id, _)| id == k.id) } else { None }
        })?;
        // Things are positioned at their centre; their foot is lower.
        let node = g.pair_iter().find(|(_, n)| n.name().to_lowercase() == hit.0).map(|(_, n)| n.local_transform().scale().y).unwrap_or(1.0);
        Some((hit.2 .0, hit.2 .1 - node / 2.0))
    }

    fn spot_for(&self, k: &catalog::Kind, near: Option<&str>, ctx: &mut PluginContext) -> Option<(f32, f32)> {
        let ch = self.chars.as_ref()?;
        let anchor = near.and_then(|n| self.place_named(n, ctx)).unwrap_or_else(|| self.pos(ch.gil, ctx));
        let avoid: Vec<Area> = [self.pos(ch.gil, ctx), self.pos(ch.you, ctx)].iter().map(|p| Area::new(p.0 - 0.6, p.1 - 0.2, p.0 + 0.6, p.1 + 0.6)).collect();
        let (w, d) = (k.w.max(k.solid_w), k.solid_d.max(0.6));
        // Not on top of the anchor itself: start a little in front of it.
        self.blocks.spot_near(anchor.0, anchor.1 - 1.5, w, d, &avoid)
    }

    fn build_by_you(&mut self, k: &'static catalog::Kind, ctx: &mut PluginContext) {
        let Some(ch) = self.chars.as_ref() else { return };
        let p = self.pos(ch.you, ctx);
        let front = (p.0 + self.facing_you * 1.5, p.1);
        let avoid = [Area::new(p.0 - 0.6, p.1 - 0.2, p.0 + 0.6, p.1 + 0.6)];
        match self.blocks.spot_near(front.0, front.1, k.w.max(k.solid_w), k.solid_d.max(0.6), &avoid) {
            Some(at) => {
                let name = format!("{}'s {}", self.player_name, k.title.to_lowercase());
                self.add_thing(k, &name, "you", at, ctx);
            }
            None => self.add_log("There's no room here.".into()),
        }
    }

    fn add_thing(&mut self, k: &'static catalog::Kind, name: &str, by: &str, at: (f32, f32), ctx: &mut PluginContext) {
        let things = self.things;
        let g = self.graph(ctx);
        let r = world::place(g, things, k, name, by, at.0, at.1);
        let full = g[r].local_transform().scale().clone_owned();
        g[r].local_transform_mut().set_scale(Vector3::new(full.x * 0.2, full.y * 0.05, 1.0));
        self.growing.push(Growing { node: r, t: 0.0, full });
        self.rebuild_blocks(ctx);
        self.dirty = Some(self.time);
        proto::say(&json!({ "type": "built", "name": name, "kind": k.id, "by": by, "x": at.0, "y": at.1 }));
    }

    fn walk_to(&mut self, to: (f32, f32), ctx: &mut PluginContext) -> bool {
        let Some(ch) = self.chars.as_ref() else { return false };
        let from = self.pos(ch.gil, ctx);
        match self.blocks.path(from, to) {
            Some(p) => {
                self.path_ahead = p;
                if self.doing == Doing::Sleeping || matches!(self.doing, Doing::Idle(_)) {
                    self.doing = Doing::Walking;
                }
                true
            }
            None => false,
        }
    }

    // ---------------------------------------------------------------- every frame

    fn update(&mut self, ctx: &mut PluginContext) {
        let dt = ctx.dt;
        self.time += dt;
        while let Ok(c) = self.rx.try_recv() {
            self.command(c, ctx);
        }
        self.move_you(dt, ctx);
        self.live_gilgamesh(dt, ctx);
        self.grow(dt, ctx);
        self.follow_camera(dt, ctx);
        self.light(ctx);
        self.update_ui(ctx);
        if self.time - self.state_at > 2.0 {
            self.state_at = self.time;
            self.report(ctx);
        }
        if let Some(at) = self.dirty {
            if self.time - at > 2.0 {
                self.save(ctx);
            }
        }
        self.take_shot(ctx);
    }

    fn move_you(&mut self, dt: f32, ctx: &mut PluginContext) {
        let Some(ch) = self.chars.as_ref() else { return };
        let (you, body) = (ch.you, ch.you_body);
        let mut d = Vector2::new(0.0f32, 0.0);
        if !self.chat_open && !self.menu_open {
            let k = |c: &[KeyCode]| c.iter().any(|c| self.keys.contains(c));
            // In Fyrox 2D the camera looks along +z, so +x is to the LEFT on screen.
            if k(&[KeyCode::ArrowLeft, KeyCode::KeyA]) {
                d.x += 1.0;
            }
            if k(&[KeyCode::ArrowRight, KeyCode::KeyD]) {
                d.x -= 1.0;
            }
            if k(&[KeyCode::ArrowUp, KeyCode::KeyW]) {
                d.y += 1.0;
            }
            if k(&[KeyCode::ArrowDown, KeyCode::KeyS]) {
                d.y -= 1.0;
            }
        }
        let mut p = self.pos(you, ctx);
        let moving = d.norm() > 0.0;
        if moving {
            let v = d.normalize() * WALK * dt;
            if self.blocks.free(p.0 + v.x, p.1) {
                p.0 += v.x;
            }
            if self.blocks.free(p.0, p.1 + v.y) {
                p.1 += v.y;
            }
            if d.x != 0.0 {
                self.facing_you = d.x.signum();
            }
        }
        let frame = if moving { ((self.time * 8.0) as usize) % 2 } else { 0 };
        let facing = self.facing_you;
        let mat = self.frames_you[frame].clone();
        let g = self.graph(ctx);
        g[you].local_transform_mut().set_position(Vector3::new(p.0, p.1, world::depth(p.1) - 0.005));
        if let Ok(r) = g.try_get_mut_of_type::<Rectangle>(body) {
            r.material_mut().set_value_and_mark_modified(mat);
        }
        let s = g[body].local_transform().scale().clone_owned();
        g[body].local_transform_mut().set_scale(Vector3::new(s.x.abs() * facing, s.y, 1.0));
    }

    fn live_gilgamesh(&mut self, dt: f32, ctx: &mut PluginContext) {
        let Some(ch) = self.chars.as_ref() else { return };
        let (gil, body, you) = (ch.gil, ch.gil_body, ch.you);
        let me = self.pos(gil, ctx);
        let them = self.pos(you, ctx);
        let close = dist(me, them) < TALK_RANGE;
        if close != self.near {
            self.near = close;
            if !close && self.chat_open {
                self.close_chat(ctx);
            }
        }
        let night = self.hour >= 22.5 || self.hour < 6.0;
        let busy_in_rad = self.phase == "thinking" || self.phase == "speaking" || self.phase == "listening";

        // What he does now.
        if self.chat_open || (busy_in_rad && close) {
            if !matches!(self.doing, Doing::Talking | Doing::Building(_)) {
                self.doing = Doing::Talking;
                self.path_ahead.clear();
            }
            if dist(me, them) > 1.6 && self.path_ahead.is_empty() {
                self.walk_to((them.0 - 1.0 * (them.0 - me.0).signum(), them.1), ctx);
            }
        } else if self.doing == Doing::Talking {
            self.doing = Doing::Idle(2.0);
        }
        match self.doing.clone() {
            Doing::Idle(t) => {
                if let Some(job) = (!self.jobs.is_empty()).then(|| self.jobs.remove(0)) {
                    if let Doing::ToBuild { at, .. } = &job {
                        let stand = (at.0 + 1.2, at.1 - 0.6);
                        if !self.walk_to(stand, ctx) {
                            self.path_ahead = vec![];
                        }
                    }
                    self.doing = job;
                } else if night {
                    let door = self.place_named("Gilgamesh's house", ctx).map(|p| (p.0, p.1 - 0.6)).unwrap_or(world::GILGAMESH_START);
                    if dist(me, door) > 0.8 {
                        self.walk_to(door, ctx);
                    } else {
                        self.doing = Doing::Sleeping;
                    }
                } else if t <= 0.0 {
                    // Wander to something of his, or anywhere nearby.
                    let g = &ctx.scenes[self.scene].graph;
                    let spots: Vec<(f32, f32)> = g.pair_iter().filter(|(_, n)| n.tag().starts_with("thing:")).map(|(_, n)| { let p = n.global_position(); let s = n.local_transform().scale(); (p.x, p.y - s.y / 2.0 - 0.7) }).filter(|p| p.0 < world::RIVER.0 + 14.0).collect();
                    let pick = if !spots.is_empty() && self.rng.f() < 0.7 { spots[self.rng.range(spots.len() as u32) as usize] } else { (me.0 + self.rng.f() * 8.0 - 4.0, me.1 + self.rng.f() * 6.0 - 3.0) };
                    if !self.walk_to(pick, ctx) {
                        self.doing = Doing::Idle(2.0);
                    }
                } else {
                    self.doing = Doing::Idle(t - dt);
                }
            }
            Doing::Sleeping => {
                if !night || !self.jobs.is_empty() {
                    self.doing = Doing::Idle(1.0);
                } else if self.bubble_until < self.time && self.rng.f() < dt * 0.25 {
                    self.show_bubble("Zz…", 2.5);
                }
            }
            Doing::ToBuild { kind, name, at, by } if self.path_ahead.is_empty() => {
                let k = catalog::by_id(kind).expect("kind");
                self.add_thing(k, &name, &by, at, ctx);
                self.doing = Doing::Building(2.5);
                let _ = at;
            }
            Doing::Building(t) => {
                self.doing = if t <= 0.0 { Doing::Idle(3.0) } else { Doing::Building(t - dt) };
            }
            _ => {}
        }

        // Walking along the path.
        let mut p = me;
        let mut moving = false;
        if !self.path_ahead.is_empty() && !matches!(self.doing, Doing::Building(_) | Doing::Sleeping) {
            let target = self.path_ahead[0];
            let (dx, dy) = (target.0 - p.0, target.1 - p.1);
            let l = (dx * dx + dy * dy).sqrt();
            let step = GIL_WALK * dt;
            if l <= step {
                p = target;
                self.path_ahead.remove(0);
            } else {
                p = (p.0 + dx / l * step, p.1 + dy / l * step);
            }
            if dx.abs() > 0.05 {
                self.facing_gil = dx.signum();
            }
            moving = true;
            if self.path_ahead.is_empty() && self.doing == Doing::Walking {
                self.doing = Doing::Idle(3.0 + self.rng.f() * 6.0);
            }
        } else if matches!(self.doing, Doing::Talking) && (them.0 - me.0).abs() > 0.1 {
            self.facing_gil = (them.0 - me.0).signum();
        }
        let frame = if moving { ((self.time * 6.0) as usize) % 2 } else { 0 };
        let mat = self.frames_gil[frame].clone();
        let facing = self.facing_gil;
        let lying = self.doing == Doing::Sleeping;
        let g = self.graph(ctx);
        g[gil].local_transform_mut().set_position(Vector3::new(p.0, p.1, world::depth(p.1) - 0.005));
        if let Ok(r) = g.try_get_mut_of_type::<Rectangle>(body) {
            r.material_mut().set_value_and_mark_modified(mat);
        }
        let s = g[body].local_transform().scale().clone_owned();
        g[body].local_transform_mut().set_scale(Vector3::new(s.x.abs() * facing, s.y, 1.0));
        // Asleep he rests a little lower and dimmer.
        let tint = if lying { Color::opaque(150, 150, 190) } else { Color::WHITE };
        if let Ok(r) = g.try_get_mut_of_type::<Rectangle>(body) {
            r.set_color(tint);
        }
    }

    fn grow(&mut self, dt: f32, ctx: &mut PluginContext) {
        if self.growing.is_empty() {
            return;
        }
        let g = &mut ctx.scenes[self.scene].graph;
        self.growing.retain_mut(|gr| {
            gr.t += dt / 2.5;
            let t = gr.t.min(1.0);
            let ease = 1.0 - (1.0 - t).powi(3);
            if let Ok(n) = g.try_get_node_mut(gr.node.transmute()) {
                let pos = n.local_transform().position().clone_owned();
                let bottom = pos.y - n.local_transform().scale().y / 2.0;
                let (w, h) = (gr.full.x * (0.2 + 0.8 * ease), gr.full.y * (0.05 + 0.95 * ease));
                n.local_transform_mut().set_scale(Vector3::new(w, h, 1.0));
                n.local_transform_mut().set_position(Vector3::new(pos.x, bottom + h / 2.0, pos.z));
            }
            t < 1.0
        });
    }

    fn follow_camera(&mut self, dt: f32, ctx: &mut PluginContext) {
        let Some(ch) = self.chars.as_ref() else { return };
        let you = self.pos(ch.you, ctx);
        let aspect = match &*ctx.graphics_context {
            GraphicsContext::Initialized(g) => {
                let s = g.window.inner_size();
                s.width as f32 / s.height.max(1) as f32
            }
            _ => 16.0 / 10.0,
        };
        let (hw, hh) = (world::VIEW / 2.0 * aspect, world::VIEW / 2.0);
        let g = self.graph(ctx);
        if self.camera.is_none() {
            return;
        }
        let c = g[self.camera].local_transform().position().clone_owned();
        let tx = you.0.clamp(hw.min(world::W / 2.0), (world::W - hw).max(world::W / 2.0));
        let ty = (you.1 + 1.0).clamp(hh.min(world::H / 2.0), (world::H - hh).max(world::H / 2.0));
        let k = (dt * 6.0).min(1.0);
        g[self.camera].local_transform_mut().set_position(Vector3::new(c.x + (tx - c.x) * k, c.y + (ty - c.y) * k, c.z));
    }

    /// Day and night: the light of the hour, and lamps and fires after dark.
    fn light(&mut self, ctx: &mut PluginContext) {
        let h = self.hour;
        // 1 at noon, 0 at night, with dusk and dawn between.
        let day = if (7.0..=18.5).contains(&h) { 1.0 } else if (5.5..7.0).contains(&h) { (h - 5.5) / 1.5 } else if h > 18.5 && h < 21.0 { 1.0 - (h - 18.5) / 2.5 } else { 0.0 };
        let night = Color::opaque(34, 44, 104);
        let dusk = Color::opaque(255, 190, 150);
        let mix = |a: Color, b: Color, t: f32| Color::opaque((a.r as f32 + (b.r as f32 - a.r as f32) * t) as u8, (a.g as f32 + (b.g as f32 - a.g as f32) * t) as u8, (a.b as f32 + (b.b as f32 - a.b as f32) * t) as u8);
        let ambient = if day >= 1.0 { Color::WHITE } else if day > 0.5 { mix(dusk, Color::WHITE, (day - 0.5) * 2.0) } else { mix(night, dusk, day * 2.0) };
        let scene = &mut ctx.scenes[self.scene];
        scene.rendering_options.ambient_lighting_color = ambient;
        let dark = day < 0.6;
        let lights: Vec<Handle<Node>> = scene.graph.pair_iter().filter(|(_, n)| n.tag() == "night-light").map(|(h, _)| h).collect();
        for l in lights {
            if scene.graph[l].visibility() != dark {
                scene.graph[l].set_visibility(dark);
            }
        }
    }

    fn update_ui(&mut self, ctx: &mut PluginContext) {
        let (Some(ui_h), Some(ch)) = (self.ui.as_ref(), self.chars.as_ref()) else { return };
        let (gil, you) = (ch.gil, ch.you);
        let g = &ctx.scenes[self.scene].graph;
        let gp = g[gil].global_position();
        let yp = g[you].global_position();
        let GraphicsContext::Initialized(gc) = &*ctx.graphics_context else { return };
        let screen = gc.renderer.get_frame_bounds();
        let Ok(cam) = g.try_get_of_type::<Camera>(self.camera) else { return };
        let head = to_screen(cam, Vector3::new(gp.x, gp.y + 1.9, gp.z), screen);
        let feet_you = to_screen(cam, Vector3::new(yp.x, yp.y + 1.8, yp.z), screen);
        // Something you're standing by: its name (and the tablet's tasks).
        let mut by: Option<(String, String, String)> = None;
        for (_, n) in g.pair_iter() {
            if let Some(t) = n.tag().strip_prefix("thing:") {
                let p = n.global_position();
                let s = n.local_transform().scale();
                if (p.x - yp.x).abs() < s.x / 2.0 + 0.8 && (yp.y - (p.y - s.y / 2.0)) > -0.8 && (yp.y - (p.y - s.y / 2.0)) < 1.2 {
                    let mut parts = t.split(':');
                    let kind = parts.next().unwrap_or("").to_string();
                    let who = parts.next().unwrap_or("").to_string();
                    by = Some((n.name().to_string(), kind, who));
                    break;
                }
            }
        }
        let ui = ctx.user_interfaces.first_mut();
        let show = |ui: &mut UserInterface, h: Handle<UiNode>, v: bool| ui.send(h, WidgetMessage::Visibility(v));
        let size_of = |ui: &UserInterface, h: Handle<UiNode>| ui.try_get_node(h).map(|n| n.actual_global_size()).unwrap_or_default();

        // Gilgamesh's bubble over his head.
        let bubble_on = self.bubble_until > self.time && !self.bubble.is_empty();
        show(ui, ui_h.bubble, bubble_on);
        if bubble_on {
            ui.send(ui_h.bubble_text, TextMessage::Text(self.bubble.clone()));
            if let Some(p) = head {
                let s = size_of(ui, ui_h.bubble);
                ui.send(ui_h.bubble, WidgetMessage::DesiredPosition(Vector2::new((p.x - s.x / 2.0).clamp(8.0, (screen.x - s.x - 8.0).max(8.0)), (p.y - s.y - 6.0).max(8.0))));
            }
        }

        // Hints at the bottom.
        let hint = if self.chat_open {
            String::new()
        } else if self.menu_open {
            "↑ ↓ choose · Enter build · Esc close".to_string()
        } else if self.near {
            format!("Enter — talk to Gilgamesh{}", if self.doing == Doing::Sleeping { " (he's asleep: you'll wake him)" } else { "" })
        } else {
            "Arrows / WASD walk · B build".to_string()
        };
        show(ui, ui_h.hint.transmute(), !hint.is_empty());
        if !hint.is_empty() {
            ui.send(ui_h.hint, TextMessage::Text(hint));
            let s = size_of(ui, ui_h.hint.transmute());
            ui.send(ui_h.hint, WidgetMessage::DesiredPosition(Vector2::new((screen.x - s.x) / 2.0, screen.y - s.y - 14.0)));
        }

        // The name of what you stand by (and the tablet's tasks).
        let label = by.as_ref().map(|(n, _, who)| match who.as_str() {
            "gilgamesh" => format!("{n} — built by Gilgamesh"),
            "you" => format!("{n} — built by you"),
            _ => n.clone(),
        });
        show(ui, ui_h.label.transmute(), label.is_some() && !self.chat_open);
        if let (Some(l), Some(p)) = (label, feet_you) {
            ui.send(ui_h.label, TextMessage::Text(l));
            let s = size_of(ui, ui_h.label.transmute());
            ui.send(ui_h.label, WidgetMessage::DesiredPosition(Vector2::new(p.x - s.x / 2.0, p.y - s.y - 4.0)));
        }
        let at_tablet = by.as_ref().is_some_and(|(_, k, _)| k == "tablet") && !self.chat_open;
        show(ui, ui_h.tablet_box, at_tablet);
        if at_tablet {
            let text = if self.tasks.is_empty() { "The clay tablet is blank.\nAsk Gilgamesh to write your tasks here.".to_string() } else { format!("Tasks\n{}", self.tasks.iter().map(|t| format!("• {t}")).collect::<Vec<_>>().join("\n")) };
            ui.send(ui_h.tablet, TextMessage::Text(text));
            ui.send(ui_h.tablet_box, WidgetMessage::DesiredPosition(Vector2::new(screen.x - 340.0, 20.0)));
        }

        // The conversation, bottom left.
        let log_on = self.chat_open || (!self.log.is_empty() && self.near);
        show(ui, ui_h.log_box, log_on);
        show(ui, ui_h.input.transmute(), self.chat_open);
        if log_on {
            ui.send(ui_h.log, TextMessage::Text(self.log.join("\n")));
            let s = size_of(ui, ui_h.log_box);
            ui.send(ui_h.log_box, WidgetMessage::DesiredPosition(Vector2::new(16.0, screen.y - s.y - 44.0)));
        }

        // The build menu.
        show(ui, ui_h.menu_box, self.menu_open);
        if self.menu_open {
            let lines: Vec<String> = catalog::KINDS.iter().enumerate().map(|(i, k)| format!("{} {}", if i == self.menu_at { "▶" } else { "  " }, k.title)).collect();
            ui.send(ui_h.menu, TextMessage::Text(format!("Build\n{}", lines.join("\n"))));
            ui.send(ui_h.menu_box, WidgetMessage::DesiredPosition(Vector2::new(20.0, 20.0)));
        }
    }

    /// Tests: what the UI shows (visible widgets, their text and place).
    fn report_ui(&mut self, ctx: &mut PluginContext) {
        let Some(u) = self.ui.as_ref() else { return };
        let ui = ctx.user_interfaces.first_mut();
        let w = |h: Handle<UiNode>| {
            ui.try_get_node(h).map(|n| json!({ "visible": n.is_globally_visible(), "x": n.screen_position().x, "y": n.screen_position().y, "w": n.actual_global_size().x, "h": n.actual_global_size().y })).unwrap_or(Value::Null)
        };
        let t = |h: Handle<Text>| ui.try_get(h).map(|n| n.text()).unwrap_or_default();
        proto::say(&json!({
            "type": "ui",
            "bubble": w(u.bubble), "bubble_text": t(u.bubble_text),
            "hint": w(u.hint.transmute()), "hint_text": t(u.hint),
            "log": w(u.log_box), "log_text": t(u.log),
            "input": w(u.input.transmute()),
            "menu": w(u.menu_box), "menu_text": t(u.menu),
            "chat_open": self.chat_open, "near": self.near,
            "screen": [ui.screen_size().x, ui.screen_size().y],
        }));
    }

    /// What RAD (and so Gilgamesh) knows of the world, every couple of seconds.
    fn report(&mut self, ctx: &mut PluginContext) {
        let Some(ch) = self.chars.as_ref() else { return };
        let (gp, yp) = (self.pos(ch.gil, ctx), self.pos(ch.you, ctx));
        let g = &ctx.scenes[self.scene].graph;
        let things: Vec<_> = g
            .pair_iter()
            .filter_map(|(_, n)| {
                let t = n.tag().strip_prefix("thing:")?;
                let mut parts = t.split(':');
                let kind = parts.next().unwrap_or("");
                let by = parts.next().unwrap_or("");
                let p = n.global_position();
                Some(json!({ "name": n.name(), "kind": kind, "by": by, "x": (p.x * 10.0).round() / 10.0, "y": (p.y * 10.0).round() / 10.0 }))
            })
            .collect();
        let doing = match &self.doing {
            Doing::Idle(_) => "standing about".to_string(),
            Doing::Walking => "walking".into(),
            Doing::ToBuild { name, .. } => format!("on his way to build {name}"),
            Doing::Building(_) => "building".into(),
            Doing::Talking => "talking with you".into(),
            Doing::Sleeping => "asleep in his house".into(),
        };
        proto::say(&json!({
            "type": "state",
            "hour": self.hour,
            "gilgamesh": { "x": gp.0, "y": gp.1, "doing": doing },
            "you": { "x": yp.0, "y": yp.1, "near_gilgamesh": self.near },
            "chatting": self.chat_open,
            "things": things,
            "jobs": self.jobs.len(),
        }));
    }

    pub fn save(&mut self, ctx: &mut PluginContext) {
        self.dirty = None;
        let scene = &mut ctx.scenes[self.scene];
        let mut visitor = Visitor::new();
        let res = scene.save("Scene", &mut visitor).map_err(|e| e.to_string()).and_then(|_| {
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = self.path.with_extension("rgs.tmp");
            visitor.save_ascii_to_file(&tmp).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
        });
        match res {
            Ok(()) => proto::say(&json!({ "type": "saved", "path": self.path.to_string_lossy() })),
            Err(e) => proto::say(&json!({ "type": "error", "message": format!("Couldn't save the world: {e}") })),
        }
    }

    /// For tests: after a few frames, writes what the camera sees to a PNG.
    fn take_shot(&mut self, ctx: &mut PluginContext) {
        let Some((path, frames)) = self.shot.take() else { return };
        if frames > 0 {
            self.shot = Some((path, frames - 1));
            return;
        }
        let scene = self.scene;
        let GraphicsContext::Initialized(gc) = &mut *ctx.graphics_context else { return };
        let size = gc.renderer.get_frame_bounds();
        let Some(data) = gc.renderer.scene_data_map.get(&scene) else {
            proto::say(&json!({ "type": "error", "message": "no frame yet" }));
            return;
        };
        let fb = &data.scene_data.ldr_scene_framebuffer;
        match fb.read_pixels(fyrox::graphics::framebuffer::ReadTarget::Color(0)) {
            Some(px) => {
                let (w, h) = (size.x as u32, size.y as u32);
                let row = (w * 4) as usize;
                if px.len() < row * h as usize {
                    proto::say(&json!({ "type": "error", "message": format!("frame is {} bytes for {w}x{h}", px.len()) }));
                    return;
                }
                let mut flipped = Vec::with_capacity(row * h as usize);
                for y in (0..h as usize).rev() {
                    flipped.extend_from_slice(&px[y * row..(y + 1) * row]);
                }
                let ok = image::save_buffer(&path, &flipped, w, h, image::ColorType::Rgba8).is_ok();
                proto::say(&json!({ "type": "shot", "path": path, "ok": ok, "w": w, "h": h }));
            }
            None => proto::say(&json!({ "type": "error", "message": "couldn't read the frame" })),
        }
    }
}

/// Where a point of the world is on screen. (`Camera::project` leaves out points in the nearer
/// half of the depth range, which is where the characters stand.)
fn to_screen(cam: &Camera, p: Vector3<f32>, screen: Vector2<f32>) -> Option<Vector2<f32>> {
    let v = cam.view_projection_matrix() * fyrox::core::algebra::Vector4::new(p.x, p.y, p.z, 1.0);
    if v.w.abs() < 1e-6 {
        return None;
    }
    let (x, y) = (v.x / v.w, v.y / v.w);
    Some(Vector2::new((x * 0.5 + 0.5) * screen.x, (1.0 - (y * 0.5 + 0.5)) * screen.y))
}

fn title_case(s: &str) -> String {
    let t = s.trim().trim_start_matches("a ").trim_start_matches("an ").trim_start_matches("the ");
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => "Something".into(),
    }
}

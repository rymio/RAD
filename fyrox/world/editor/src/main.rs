//! The Fyrox editor with Gilgamesh's world in it. RAD opens it with `--scene <world file>`.
use fyroxed_base::{fyrox::core::log::Log, fyrox::event_loop::EventLoop, Editor, StartupData};

fn main() {
    Log::set_file_name("gilgamesh_world_editor.log");
    let args: Vec<String> = std::env::args().collect();
    let scene = args.iter().position(|a| a == "--scene").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "data/world.rgs".into());
    let event_loop = EventLoop::new().unwrap();
    let mut editor = Editor::new(Some(StartupData { working_directory: Default::default(), scenes: vec![scene.into()], named_objects: false }));
    editor.add_game_plugin(gilgamesh_world::Game::default());
    editor.run(event_loop)
}

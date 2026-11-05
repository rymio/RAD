//! Runs Gilgamesh's world. RAD starts it with `--world <file>`; `--generate` makes a new world
//! file without opening a window; `--hidden` keeps the window out of sight (for tests).
use fyrox::core::log::Log;
use fyrox::dpi::LogicalSize;
use fyrox::engine::{executor::Executor, GraphicsContextParams};
use fyrox::event_loop::EventLoop;
use fyrox::window::WindowAttributes;
use gilgamesh_world::{generate_only, Game};

fn main() {
    Log::set_file_name("gilgamesh_world.log");
    let hidden = std::env::args().any(|a| a == "--hidden");
    let mut executor = if generate_only() {
        Executor::new(None)
    } else {
        let mut window_attributes = WindowAttributes::default();
        window_attributes.title = "Gilgamesh's World".to_string();
        window_attributes.resizable = true;
        window_attributes.visible = !hidden;
        window_attributes.inner_size = Some(LogicalSize::new(1280.0, 800.0).into());
        Executor::from_params(
            Some(EventLoop::new().unwrap()),
            GraphicsContextParams { window_attributes, vsync: true, msaa_sample_count: None, graphics_server_constructor: Default::default(), named_objects: false },
        )
    };
    executor.add_plugin(Game::default());
    executor.run()
}

//! The parts of the libretro API (libretro.h) RAD's DOS host uses, and the loaded core.

use std::ffi::{c_char, c_void};

pub const ENV_SHUTDOWN: u32 = 7;
pub const ENV_GET_CAN_DUPE: u32 = 3;
pub const ENV_SET_MESSAGE: u32 = 6;
pub const ENV_GET_SYSTEM_DIRECTORY: u32 = 9;
pub const ENV_SET_PIXEL_FORMAT: u32 = 10;
pub const ENV_SET_INPUT_DESCRIPTORS: u32 = 11;
pub const ENV_SET_KEYBOARD_CALLBACK: u32 = 12;
pub const ENV_SET_DISK_CONTROL_INTERFACE: u32 = 13;
pub const ENV_GET_VARIABLE: u32 = 15;
pub const ENV_SET_VARIABLES: u32 = 16;
pub const ENV_GET_VARIABLE_UPDATE: u32 = 17;
pub const ENV_SET_SUPPORT_NO_GAME: u32 = 18;
pub const ENV_GET_SAVE_DIRECTORY: u32 = 31;
pub const ENV_SET_SYSTEM_AV_INFO: u32 = 32;
pub const ENV_SET_CONTROLLER_INFO: u32 = 35;
pub const ENV_SET_MEMORY_MAPS: u32 = 36;
pub const ENV_SET_GEOMETRY: u32 = 37;
pub const ENV_SET_SUPPORT_ACHIEVEMENTS: u32 = 42;
pub const ENV_GET_AUDIO_VIDEO_ENABLE: u32 = 47;
pub const ENV_SET_DISK_CONTROL_EXT_INTERFACE: u32 = 58;
pub const ENV_SET_MESSAGE_EXT: u32 = 60;

pub const PIXEL_FORMAT_XRGB8888: u32 = 1;

pub const DEVICE_JOYPAD: u32 = 1;
pub const DEVICE_MOUSE: u32 = 2;
pub const DEVICE_KEYBOARD: u32 = 3;
pub const JOYPAD_MASK: u32 = 256;
pub const MOUSE_X: u32 = 0;
pub const MOUSE_Y: u32 = 1;
pub const MOUSE_LEFT: u32 = 2;
pub const MOUSE_RIGHT: u32 = 3;
pub const MOUSE_WHEELUP: u32 = 4;
pub const MOUSE_WHEELDOWN: u32 = 5;
pub const MOUSE_MIDDLE: u32 = 6;

pub const KEY_LSHIFT: u32 = 304;

#[repr(C)]
pub struct GameInfo {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GameGeometry {
    pub base_width: u32,
    pub base_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub aspect_ratio: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTiming {
    pub fps: f64,
    pub sample_rate: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemAvInfo {
    pub geometry: GameGeometry,
    pub timing: SystemTiming,
}

#[repr(C)]
pub struct Variable {
    pub key: *const c_char,
    pub value: *const c_char,
}

#[repr(C)]
pub struct Message {
    pub msg: *const c_char,
    pub frames: u32,
}

#[repr(C)]
pub struct MessageExt {
    pub msg: *const c_char,
    pub duration: u32,
    pub priority: u32,
    pub level: i32,
    pub target: i32,
    pub kind: i32,
    pub progress: i8,
}

pub type KeyboardEventFn = unsafe extern "C" fn(down: bool, keycode: u32, character: u32, modifiers: u16);

#[repr(C)]
pub struct KeyboardCallback {
    pub callback: KeyboardEventFn,
}

#[repr(C)]
pub struct DiskControl {
    pub set_eject_state: unsafe extern "C" fn(bool) -> bool,
    pub get_eject_state: unsafe extern "C" fn() -> bool,
    pub get_image_index: unsafe extern "C" fn() -> u32,
    pub set_image_index: unsafe extern "C" fn(u32) -> bool,
    pub get_num_images: unsafe extern "C" fn() -> u32,
    pub replace_image_index: unsafe extern "C" fn(u32, *const GameInfo) -> bool,
    pub add_image_index: unsafe extern "C" fn() -> bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DiskControlExt {
    pub set_eject_state: unsafe extern "C" fn(bool) -> bool,
    pub get_eject_state: unsafe extern "C" fn() -> bool,
    pub get_image_index: unsafe extern "C" fn() -> u32,
    pub set_image_index: unsafe extern "C" fn(u32) -> bool,
    pub get_num_images: unsafe extern "C" fn() -> u32,
    pub replace_image_index: unsafe extern "C" fn(u32, *const GameInfo) -> bool,
    pub add_image_index: unsafe extern "C" fn() -> bool,
    pub set_initial_image: Option<unsafe extern "C" fn(u32, *const c_char) -> bool>,
    pub get_image_path: Option<unsafe extern "C" fn(u32, *mut c_char, usize) -> bool>,
    pub get_image_label: Option<unsafe extern "C" fn(u32, *mut c_char, usize) -> bool>,
}

// The callbacks the host gives the core.
pub type EnvironmentFn = unsafe extern "C" fn(u32, *mut c_void) -> bool;
pub type VideoRefreshFn = unsafe extern "C" fn(*const c_void, u32, u32, usize);
pub type AudioSampleFn = unsafe extern "C" fn(i16, i16);
pub type AudioSampleBatchFn = unsafe extern "C" fn(*const i16, usize) -> usize;
pub type InputPollFn = unsafe extern "C" fn();
pub type InputStateFn = unsafe extern "C" fn(u32, u32, u32, u32) -> i16;

/// A loaded libretro core: its entry points (the library stays loaded while this lives).
pub struct Core {
    _lib: libloading::Library,
    pub init: unsafe extern "C" fn(),
    pub deinit: unsafe extern "C" fn(),
    pub set_environment: unsafe extern "C" fn(EnvironmentFn),
    pub set_video_refresh: unsafe extern "C" fn(VideoRefreshFn),
    pub set_audio_sample: unsafe extern "C" fn(AudioSampleFn),
    pub set_audio_sample_batch: unsafe extern "C" fn(AudioSampleBatchFn),
    pub set_input_poll: unsafe extern "C" fn(InputPollFn),
    pub set_input_state: unsafe extern "C" fn(InputStateFn),
    pub set_controller_port_device: unsafe extern "C" fn(u32, u32),
    pub get_system_av_info: unsafe extern "C" fn(*mut SystemAvInfo),
    pub load_game: unsafe extern "C" fn(*const GameInfo) -> bool,
    pub unload_game: unsafe extern "C" fn(),
    pub run: unsafe extern "C" fn(),
    pub reset: unsafe extern "C" fn(),
}

impl Core {
    /// # Safety
    /// `path` must be a libretro core.
    pub unsafe fn load(path: &str) -> Result<Core, libloading::Error> {
        let lib = libloading::Library::new(path)?;
        macro_rules! sym {
            ($name:literal) => {
                *lib.get($name)?
            };
        }
        Ok(Core {
            init: sym!(b"retro_init"),
            deinit: sym!(b"retro_deinit"),
            set_environment: sym!(b"retro_set_environment"),
            set_video_refresh: sym!(b"retro_set_video_refresh"),
            set_audio_sample: sym!(b"retro_set_audio_sample"),
            set_audio_sample_batch: sym!(b"retro_set_audio_sample_batch"),
            set_input_poll: sym!(b"retro_set_input_poll"),
            set_input_state: sym!(b"retro_set_input_state"),
            set_controller_port_device: sym!(b"retro_set_controller_port_device"),
            get_system_av_info: sym!(b"retro_get_system_av_info"),
            load_game: sym!(b"retro_load_game"),
            unload_game: sym!(b"retro_unload_game"),
            run: sym!(b"retro_run"),
            reset: sym!(b"retro_reset"),
            _lib: lib,
        })
    }
}

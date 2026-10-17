//! RAD's ZX Spectrum: a 48K or 128K Spectrum (SPECTRUSTY's emulation) with a Kempston
//! joystick, the AY sound chip (128K), tapes (TZX and TAP, see [`tzx`]) and snapshots (Z80, SNA).
//!
//! The host drives it one frame (1/50 s) at a time: [`Spectrum::run_frame`], then
//! [`Spectrum::render`] for the picture (one byte per pixel: a Spectrum colour 0–15) and
//! [`Spectrum::audio`] for the sound.

pub mod basic;
pub mod tzx;

use spectrusty::audio::{synth::BandLimited, AudioFrame, BlepStereo, EarMicAmps4, EarOutAmps4, UlaAudioFrame};
use spectrusty::bus::ay::serial128::Ay3_8912Keypad;
use spectrusty::bus::joystick::KempstonJoystick;
use spectrusty::bus::{BusDevice, NullDevice};
use spectrusty::chip::{ula::UlaPAL, ula128::Ula128, ControlUnit, EarIn, HostConfig, MemoryAccess, UlaCommon};
use spectrusty::clock::FTs;
use spectrusty::memory::{Memory48k, ZxMemory};
use spectrusty::peripherals::ay::audio::AyAmps;
use spectrusty::peripherals::joystick::{Directions, JoystickInterface};
use spectrusty::peripherals::{KeyboardInterface, ZXKeyboardMap};
use spectrusty::video::{BorderSize, Palette, PixelBuffer, Video};
use spectrusty::z80emu::Z80NMOS;

pub use spectrusty::peripherals::ZXKeyboardMap as Keys;

static ROM48: &[u8] = include_bytes!("../roms/48.rom");
static ROM128_0: &[u8] = include_bytes!("../roms/128-0.rom");
static ROM128_1: &[u8] = include_bytes!("../roms/128-1.rom");

#[derive(Debug, thiserror::Error)]
pub enum ZxError {
    #[error("{0}")]
    Tape(#[from] tzx::TapeError),
    #[error("couldn't load the snapshot: {0}")]
    Snapshot(String),
    #[error("{0} files aren't supported (use TZX, TAP, Z80 or SNA)")]
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Spectrum48,
    Spectrum128,
}

type Terminator = NullDevice<FTs>;
pub(crate) type Ula48 = UlaPAL<Memory48k, KempstonJoystick<Terminator>>;
pub(crate) type Ula128k = Ula128<Ay3_8912Keypad<KempstonJoystick<Terminator>>>;

pub(crate) struct Zx<U> {
    pub cpu: Z80NMOS,
    pub ula: U,
}

pub(crate) enum Machine {
    S48(Box<Zx<Ula48>>),
    S128(Box<Zx<Ula128k>>),
}

/// Runs `$body` with `$zx` bound to whichever machine is inside.
macro_rules! with_zx {
    ($m:expr, $zx:ident => $body:expr) => {
        match $m {
            Machine::S48($zx) => $body,
            Machine::S128($zx) => $body,
        }
    };
}

mod snapshot;

fn new48() -> Box<Zx<Ula48>> {
    let mut zx = Box::new(Zx { cpu: Z80NMOS::default(), ula: Ula48::default() });
    zx.ula.memory_mut().load_into_rom(ROM48).expect("48K ROM");
    zx
}

fn new128() -> Box<Zx<Ula128k>> {
    let mut zx = Box::new(Zx { cpu: Z80NMOS::default(), ula: Ula128k::default() });
    let mem = zx.ula.memory_mut();
    mem.load_into_rom_bank(0, ROM128_0).expect("128K ROM 0");
    mem.load_into_rom_bank(1, ROM128_1).expect("128K ROM 1");
    zx
}

impl Machine {
    pub(crate) fn new(model: Model) -> Machine {
        match model {
            Model::Spectrum48 => Machine::S48(new48()),
            Model::Spectrum128 => Machine::S128(new128()),
        }
    }

    pub(crate) fn model(&self) -> Model {
        match self {
            Machine::S48(_) => Model::Spectrum48,
            Machine::S128(_) => Model::Spectrum128,
        }
    }

    fn cpu_hz(&self) -> u32 {
        match self {
            Machine::S48(_) => Ula48::CPU_HZ,
            Machine::S128(_) => Ula128k::CPU_HZ,
        }
    }

    fn joystick(&mut self) -> &mut dyn JoystickInterface {
        match self {
            Machine::S48(zx) => &mut **zx.ula.bus_device_mut(),
            Machine::S128(zx) => &mut **zx.ula.bus_device_mut().next_device_mut(),
        }
    }
}

/// One byte per pixel: the Spectrum colour index (0–7 normal, 8–15 bright).
struct PixelBufIndex<'a> {
    iter: std::slice::IterMut<'a, u8>,
}

impl<'a> PixelBuffer<'a> for PixelBufIndex<'a> {
    type Pixel = u8;
    fn from_line(line: &'a mut [u8]) -> Self {
        PixelBufIndex { iter: line.iter_mut() }
    }
    fn put_pixel(&mut self, pixel: u8) {
        if let Some(p) = self.iter.next() {
            *p = pixel;
        }
    }
}

struct IndexPalette;

impl Palette for IndexPalette {
    type Pixel = u8;
    fn get_pixel(index: u8) -> u8 {
        index & 15
    }
    fn get_pixel_gray(index: u8) -> u8 {
        index & 15
    }
    fn get_pixel_grb8(g3r3b2: u8) -> u8 {
        // ULAplus colours (unused here): the nearest of the 8 basic ones.
        let (g, r, b) = (g3r3b2 >> 7 & 1, g3r3b2 >> 4 & 1, g3r3b2 >> 1 & 1);
        (g << 2 | r << 1 | b) | if g3r3b2 & 0b1001_0010 == 0b1001_0010 { 8 } else { 0 }
    }
    fn get_pixel_gray8(value: u8) -> u8 {
        if value > 170 { 15 } else if value > 85 { 7 } else { 0 }
    }
}

/// The 16 Spectrum colours as RGB (for the host's palette).
pub const PALETTE: [[u8; 3]; 16] = [
    [0, 0, 0], [0, 0, 0xd7], [0xd7, 0, 0], [0xd7, 0, 0xd7], [0, 0xd7, 0], [0, 0xd7, 0xd7], [0xd7, 0xd7, 0], [0xd7, 0xd7, 0xd7],
    [0, 0, 0], [0, 0, 0xff], [0xff, 0, 0], [0xff, 0, 0xff], [0, 0xff, 0], [0, 0xff, 0xff], [0xff, 0xff, 0], [0xff, 0xff, 0xff],
];

/// Keys typed by the emulator itself (LOAD "" after inserting a tape): (keys, frames held).
type Typing = std::collections::VecDeque<(ZXKeyboardMap, u32)>;

/// Joystick directions and fire, as a bit set: up 1, down 2, left 4, right 8, fire 16.
pub mod joy {
    pub const UP: u8 = 1;
    pub const DOWN: u8 = 2;
    pub const LEFT: u8 = 4;
    pub const RIGHT: u8 = 8;
    pub const FIRE: u8 = 16;
}

pub struct Spectrum {
    m: Machine,
    keys: ZXKeyboardMap,
    typing: Typing,
    tape: Option<Tape>,
    /// Run tapes as fast as possible while the Spectrum reads them.
    pub fast_load: bool,
    turbo: bool,
    prev_ear_count: u32,
    blep: BlepStereo<BandLimited<f32>>,
    sample_rate: u32,
    border: BorderSize,
}

struct Tape {
    blocks: Vec<tzx::Block>,
    pulses: tzx::Pulses,
    playing: bool,
    names: Vec<String>,
}

/// What a loaded file was.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    /// A tape, inserted and set to load (the machine is reset and types LOAD "").
    Tape { names: Vec<String> },
    Snapshot,
}

impl Spectrum {
    pub fn new(model: Model) -> Spectrum {
        let mut s = Spectrum {
            m: Machine::new(model),
            keys: ZXKeyboardMap::empty(),
            typing: Typing::new(),
            tape: None,
            fast_load: true,
            turbo: false,
            prev_ear_count: 0,
            blep: BlepStereo::build(0.8)(BandLimited::<f32>::new(2)),
            sample_rate: 44_100,
            border: BorderSize::Full,
        };
        s.prepare_audio(44_100);
        s
    }

    pub fn model(&self) -> Model {
        self.m.model()
    }

    /// Switches to another model (a fresh machine; the tape stays in).
    pub fn set_model(&mut self, model: Model) {
        self.m = Machine::new(model);
        self.after_new_machine();
    }

    fn after_new_machine(&mut self) {
        self.prepare_audio(self.sample_rate);
        self.typing.clear();
        if let Some(t) = &mut self.tape {
            t.pulses = tzx::Pulses::new(t.blocks.clone(), self.m.cpu_hz(), self.m.model() == Model::Spectrum48);
            t.playing = false;
        }
    }

    pub fn reset(&mut self) {
        with_zx!(&mut self.m, zx => zx.ula.reset(&mut zx.cpu, true));
        self.typing.clear();
        self.turbo = false;
    }

    /// Sets which Spectrum keys are held down.
    pub fn set_keys(&mut self, keys: ZXKeyboardMap) {
        self.keys = keys;
    }

    /// Sets the Kempston joystick (see [`joy`]).
    pub fn set_joystick(&mut self, bits: u8) {
        let j = self.m.joystick();
        let mut d = Directions::empty();
        if bits & joy::UP != 0 {
            d |= Directions::UP;
        }
        if bits & joy::DOWN != 0 {
            d |= Directions::DOWN;
        }
        if bits & joy::LEFT != 0 {
            d |= Directions::LEFT;
        }
        if bits & joy::RIGHT != 0 {
            d |= Directions::RIGHT;
        }
        j.set_directions(d);
        j.fire(0, bits & joy::FIRE != 0);
    }

    /// Loads a file by its name and contents: a tape (TZX, TAP) or a snapshot (Z80, SNA).
    pub fn load(&mut self, name: &str, data: &[u8]) -> Result<Loaded, ZxError> {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "tzx" | "tap" => {
                let blocks = tzx::parse(data)?;
                let names = tzx::describe(&blocks);
                // 128K-only tapes (or anything with a 128K hint) get a 128K; others the model chosen.
                let pulses = tzx::Pulses::new(blocks.clone(), self.m.cpu_hz(), self.m.model() == Model::Spectrum48);
                self.tape = Some(Tape { blocks, pulses, playing: false, names: names.clone() });
                self.reset();
                self.type_load();
                Ok(Loaded::Tape { names })
            }
            "z80" | "sna" => {
                let m = snapshot::load(&ext, data).map_err(ZxError::Snapshot)?;
                self.m = m;
                self.after_new_machine();
                Ok(Loaded::Snapshot)
            }
            other => Err(ZxError::Unsupported(other.to_uppercase())),
        }
    }

    /// After a reset: wait for the ROM, then LOAD "" + ENTER (48K) or ENTER on "Tape Loader" (128K).
    fn type_load(&mut self) {
        let k = ZXKeyboardMap::empty();
        self.typing.push_back((k, 150));
        match self.m.model() {
            Model::Spectrum48 => {
                for keys in [ZXKeyboardMap::J, ZXKeyboardMap::SS | ZXKeyboardMap::P, ZXKeyboardMap::SS | ZXKeyboardMap::P, ZXKeyboardMap::EN] {
                    self.typing.push_back((keys, 4));
                    self.typing.push_back((k, 4));
                }
            }
            Model::Spectrum128 => {
                self.typing.push_back((ZXKeyboardMap::EN, 4));
                self.typing.push_back((k, 4));
            }
        }
    }

    pub fn eject(&mut self) {
        self.tape = None;
        self.turbo = false;
    }

    /// Starts or stops the tape by hand.
    pub fn tape_play(&mut self, play: bool) {
        if let Some(t) = &mut self.tape {
            t.playing = play;
            if play {
                t.pulses.resume();
            }
        }
        if !play {
            self.turbo = false;
        }
    }

    pub fn tape_rewind(&mut self) {
        if let Some(t) = &mut self.tape {
            t.pulses = tzx::Pulses::new(t.blocks.clone(), self.m.cpu_hz(), self.m.model() == Model::Spectrum48);
            t.playing = false;
        }
        self.turbo = false;
    }

    /// The tape: (inserted, playing, progress 0–1, names on it).
    pub fn tape_state(&self) -> Option<(bool, f32, &[String])> {
        self.tape.as_ref().map(|t| (t.playing, t.pulses.progress(), t.names.as_slice()))
    }

    /// Running faster than real time (fast loading): the host should run several frames per
    /// tick and skip their sound.
    pub fn is_turbo(&self) -> bool {
        self.turbo
    }

    /// Emulates one frame (1/50 s of Spectrum time).
    pub fn run_frame(&mut self) {
        // Keys being typed by the emulator take over from the user's for a moment.
        let keys = match self.typing.front_mut() {
            Some((k, n)) => {
                let k = *k;
                *n -= 1;
                if *n == 0 {
                    self.typing.pop_front();
                }
                k
            }
            None => self.keys,
        };
        let tape_auto = self.fast_load;
        let tape = &mut self.tape;
        let turbo = &mut self.turbo;
        let prev = &mut self.prev_ear_count;
        let typing_done = self.typing.is_empty();
        with_zx!(&mut self.m, zx => {
            if zx.ula.get_key_state() != keys {
                zx.ula.set_key_state(keys);
            }
            // Play the tape when the Spectrum starts reading it, and fast-forward while it does.
            let count = zx.ula.read_ear_in_count();
            if let Some(t) = tape.as_mut() {
                if count != 0 {
                    if t.playing && *turbo && *prev + count < 20 {
                        // It stopped reading: back to normal speed.
                        *turbo = false;
                    } else if !t.playing && typing_done && count > 1000 && !t.pulses.is_stopped() {
                        t.playing = true;
                    } else if !t.playing && t.pulses.is_stopped() && count > 1000 {
                        // Past a "stop the tape" point and the game asks for more: carry on.
                        t.pulses.resume();
                        t.playing = true;
                    }
                    if t.playing && tape_auto && count > 1000 {
                        *turbo = true;
                    }
                }
                *prev = count;
            }
            zx.ula.ensure_next_frame();
            if let Some(t) = tape.as_mut().filter(|t| t.playing) {
                let mut feed = (&mut t.pulses).peekable();
                if feed.peek().is_some() {
                    zx.ula.feed_ear_in(&mut feed, Some(1));
                } else {
                    t.playing = false;
                    *turbo = false;
                }
            }
            zx.ula.execute_next_frame(&mut zx.cpu);
        });
    }

    /// Picture size in pixels (with the border).
    pub fn size(&self) -> (usize, usize) {
        let (w, h) = <Ula128k as Video>::render_size_pixels(self.border);
        (w as usize, h as usize)
    }

    /// Draws the last frame into `out` (width × height bytes, colour indices 0–15).
    pub fn render(&mut self, out: &mut [u8]) {
        let (w, _) = self.size();
        let border = self.border;
        with_zx!(&mut self.m, zx => zx.ula.render_video_frame::<PixelBufIndex, IndexPalette>(out, w, border));
    }

    fn prepare_audio(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        let blep = &mut self.blep;
        let hz = self.m.cpu_hz() as f64;
        with_zx!(&mut self.m, zx => zx.ula.ensure_audio_frame_time(blep, sample_rate, hz));
    }

    /// Sets the host's sample rate.
    pub fn set_sample_rate(&mut self, rate: u32) {
        if rate != self.sample_rate {
            self.prepare_audio(rate);
        }
    }

    /// The last frame's sound: interleaved stereo f32 samples. `tape_audible`: hear the tape too.
    pub fn audio(&mut self, out: &mut Vec<f32>, tape_audible: bool) {
        let blep = &mut self.blep;
        with_zx!(&mut self.m, zx => render_audio(&mut zx.ula, blep, tape_audible));
        let left = self.blep.sum_iter::<f32>(0);
        let n = left.len();
        out.clear();
        out.reserve(n * 2);
        for (l, r) in left.zip(self.blep.sum_iter::<f32>(1)) {
            out.push(l);
            out.push(r);
        }
        self.blep.next_frame();
    }

    /// Ends a frame whose sound is skipped (fast loading).
    pub fn skip_audio(&mut self) {
        let blep = &mut self.blep;
        with_zx!(&mut self.m, zx => {
            render_audio(&mut zx.ula, blep, false);
        });
        self.blep.next_frame();
    }

    /// The machine as a Z80 snapshot (a save state; [`Spectrum::load`] it as "….z80").
    pub fn save_state(&self) -> Result<Vec<u8>, ZxError> {
        snapshot::save(&self.m).map_err(ZxError::Snapshot)
    }

    /// The screen as text: 24 rows of 32 characters, read by matching each 8×8 cell against the
    /// ROM's font (normal or inverse). UDGs show as `\a`…, other graphics as `▒`.
    pub fn screen_text(&self) -> Vec<String> {
        let peek = |a: u16| self.peek(a);
        let font = |code: u8| -> [u8; 8] {
            let base = 0x3d00u16 + (code as u16 - 32) * 8;
            std::array::from_fn(|i| peek(base + i as u16))
        };
        let fonts: Vec<(char, [u8; 8])> = (32u8..128).map(|c| (if c == 0x60 { '£' } else if c == 0x7f { '©' } else { c as char }, font(c))).collect();
        let udg_base = u16::from_le_bytes([peek(23675), peek(23676)]);
        let udgs: Vec<(char, [u8; 8])> = (0..21u16).map(|k| ((b'a' + k as u8) as char, std::array::from_fn(|i| peek(udg_base.wrapping_add(k * 8 + i as u16))))).collect();
        let mut rows = Vec::with_capacity(24);
        for r in 0..24u16 {
            let mut line = String::new();
            for c in 0..32u16 {
                let cell: [u8; 8] = std::array::from_fn(|l| {
                    let y = r * 8 + l as u16;
                    peek(0x4000 | ((y & 0xc0) << 5) | ((y & 7) << 8) | ((y & 0x38) << 2) | c)
                });
                let inv = cell.map(|b| !b);
                let ch = if cell == [0; 8] || inv == [0; 8] {
                    ' '
                } else if let Some((ch, _)) = fonts.iter().find(|(_, f)| *f == cell || *f == inv) {
                    *ch
                } else if let Some((ch, _)) = udgs.iter().find(|(_, f)| *f == cell || *f == inv) {
                    line.push('\\');
                    *ch
                } else {
                    '▒'
                };
                line.push(ch);
            }
            rows.push(line.trim_end().to_string());
        }
        while rows.last().is_some_and(|r| r.is_empty()) {
            rows.pop();
        }
        rows
    }

    /// Reads memory (for tests and pokes).
    pub fn peek(&self, addr: u16) -> u8 {
        with_zx!(&self.m, zx => zx.ula.memory_ref().read(addr))
    }

    /// The border colour (0–7).
    pub fn border_color(&self) -> u8 {
        with_zx!(&self.m, zx => zx.ula.border_color().into())
    }
}

fn render_audio<U>(ula: &mut U, blep: &mut BlepStereo<BandLimited<f32>>, tape: bool)
where
    U: UlaCommon + UlaAudioFrame<BlepStereo<BandLimited<f32>>>,
{
    ula.render_ay_audio_frame::<AyAmps<f32>>(blep, [0, 1, 2]);
    if tape {
        ula.render_earmic_out_audio_frame::<EarMicAmps4<f32>>(blep, 2);
        ula.render_ear_in_audio_frame::<spectrusty::audio::EarInAmps2<f32>>(blep, 2);
    } else {
        ula.render_earmic_out_audio_frame::<EarOutAmps4<f32>>(blep, 2);
    }
    ula.end_audio_frame(blep);
}

// ------------------------------------------------------------------ checking BASIC programs

/// What a BASIC program did in its first seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct BasicRun {
    /// It loaded from tape and started.
    pub started: bool,
    /// The report it stopped with, if it stopped: (code, message, line, statement), e.g.
    /// ("C", "Nonsense in BASIC", 20, 1). "0 OK" means it ended normally.
    pub report: Option<(String, String, u16, u8)>,
    /// The screen as text when the time was up (see [`Spectrum::screen_text`]).
    pub screen: Vec<String>,
}

/// Loads a BASIC tape into a 48K Spectrum and runs it for `seconds` (no keys pressed).
pub fn run_basic(tap: &[u8], seconds: u32) -> Result<BasicRun, ZxError> {
    run_program("program.tap", tap, seconds)
}

/// Loads a program (a tape, or a snapshot from the assembler) and runs it for `seconds` with no
/// keys pressed: what it shows, and the BASIC report if it stopped. A program that crashes
/// usually ends back at the start-up screen (`crashed`).
pub fn run_program(name: &str, data: &[u8], seconds: u32) -> Result<BasicRun, ZxError> {
    let mut zx = Spectrum::new(Model::Spectrum48);
    let loaded = zx.load(name, data)?;
    let mut started = true;
    if matches!(loaded, Loaded::Tape { .. }) {
        // Until the tape has played out (fast-loading, so a few hundred frames).
        started = false;
        for _ in 0..12_000 {
            zx.run_frame();
            zx.skip_audio();
            if zx.tape_state().is_some_and(|(playing, progress, _)| !playing && progress > 0.95) && zx.typing.is_empty() {
                started = true;
                break;
            }
        }
    }
    for _ in 0..seconds * 50 {
        zx.run_frame();
        zx.skip_audio();
    }
    let screen = zx.screen_text();
    // A report sits in the bottom rows: "C Nonsense in BASIC, 20:1".
    let report = screen.iter().rev().take(3).find_map(|row| parse_report(row.trim()));
    Ok(BasicRun { started, report, screen })
}

impl BasicRun {
    /// The program ended up at the Spectrum's start-up screen (it crashed and reset).
    pub fn crashed(&self) -> bool {
        self.screen.iter().any(|l| l.contains("1982 Sinclair Research Ltd"))
    }
}

fn parse_report(row: &str) -> Option<(String, String, u16, u8)> {
    let (head, pos) = row.rsplit_once(", ")?;
    let (line, stmt) = pos.split_once(':')?;
    let (code, msg) = head.split_once(' ')?;
    if code.len() != 1 || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some((code.to_string(), msg.to_string(), line.parse().ok()?, stmt.parse().ok()?))
}

// Keep the types used by snapshot.rs reachable.
pub(crate) use spectrusty::chip::Ula128MemFlags;
pub(crate) fn machine_48() -> Box<Zx<Ula48>> {
    new48()
}
pub(crate) fn machine_128() -> Box<Zx<Ula128k>> {
    new128()
}

#[cfg(test)]
mod tests;

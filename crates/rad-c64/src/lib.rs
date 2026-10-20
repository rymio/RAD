//! RAD's Commodore 64: a PAL C64 (floooh's chips emulation, built from chips/ with shim.c)
//! with joysticks, the SID, tapes (TAP), disks (D64, served to the KERNAL's LOAD directly:
//! there's no 1541 drive emulation, so custom disk fast loaders don't work), cartridges (CRT),
//! programs (PRG, P00, T64) and save states.
//!
//! The host drives it one frame (1/50 s) at a time: [`C64::run_frame`], then
//! [`C64::render`] for the picture (one byte per pixel: a C64 colour 0–15) and
//! [`C64::audio`] for the sound.
//!
//! ROMs: the C64's own KERNAL, BASIC and character ROMs are still under copyright, so RAD
//! starts with the MEGA65 project's free Open ROMs (roms/, LGPL). They run machine-code games
//! and programs, but their BASIC can't run BASIC programs yet (no numeric variables or
//! expressions): for that the user adds their own C64 ROMs ([`Roms::new`]).

pub mod basic;
mod ffi;
pub mod formats;
pub mod lab;

use std::collections::VecDeque;
use std::sync::Arc;

pub use ffi::Regs;
use ffi::Machine;
use formats::{Crt, D64, Prg};

static OPEN_CHARS: &[u8] = include_bytes!("../roms/chargen_openroms.rom");
static OPEN_BASIC: &[u8] = include_bytes!("../roms/basic_generic.rom");
static OPEN_KERNAL: &[u8] = include_bytes!("../roms/kernal_generic.rom");

/// Clock cycles in one PAL frame (312 lines of 63 cycles) and its length in microseconds.
pub const FRAME_TICKS: u32 = 19_656;
pub const FRAME_US: u64 = 19_950;

/// The KERNAL's LOAD entry in its jump table (served from the disk image).
const KERNAL_LOAD: u16 = 0xffd5;

#[derive(Debug, thiserror::Error)]
pub enum C64Error {
    #[error("{0}")]
    Format(String),
    #[error("{0} files aren't supported (use PRG, P00, T64, TAP, D64 or CRT)")]
    Unsupported(String),
    #[error("couldn't load the save: {0}")]
    State(String),
    #[error("{0}")]
    Roms(String),
}

/// A set of ROMs: character generator (4K), BASIC (8K) and KERNAL (8K).
#[derive(Clone)]
pub struct Roms {
    pub chars: Vec<u8>,
    pub basic: Vec<u8>,
    pub kernal: Vec<u8>,
}

impl Roms {
    /// The free Open ROMs that come with RAD.
    pub fn open_roms() -> Roms {
        Roms { chars: OPEN_CHARS.to_vec(), basic: OPEN_BASIC.to_vec(), kernal: OPEN_KERNAL.to_vec() }
    }

    /// The user's own ROM images.
    pub fn new(chars: Vec<u8>, basic: Vec<u8>, kernal: Vec<u8>) -> Result<Roms, C64Error> {
        if chars.len() != 0x1000 {
            return Err(C64Error::Roms(format!("The character ROM should be 4096 bytes (it's {})", chars.len())));
        }
        if basic.len() != 0x2000 {
            return Err(C64Error::Roms(format!("The BASIC ROM should be 8192 bytes (it's {})", basic.len())));
        }
        if kernal.len() != 0x2000 {
            return Err(C64Error::Roms(format!("The KERNAL ROM should be 8192 bytes (it's {})", kernal.len())));
        }
        if kernal[0x1ffc..0x1ffe] == [0, 0] || kernal[0x1ffc..0x1ffe] == [0xff, 0xff] {
            return Err(C64Error::Roms("That doesn't look like a KERNAL ROM (it has no reset vector)".into()));
        }
        Ok(Roms { chars, basic, kernal })
    }

    /// These are the free Open ROMs (BASIC programs won't run).
    pub fn is_open_roms(&self) -> bool {
        self.basic.windows(9).any(|w| w == b"OPEN ROMS")
    }
}

/// The 16 C64 colours as RGB (Pepto's palette, as chips uses).
pub const PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], [0xff, 0xff, 0xff], [0x81, 0x33, 0x38], [0x75, 0xce, 0xc8],
    [0x8e, 0x3c, 0x97], [0x56, 0xac, 0x4d], [0x2e, 0x2c, 0x9b], [0xed, 0xf1, 0x71],
    [0x8e, 0x50, 0x29], [0x55, 0x38, 0x00], [0xc4, 0x6c, 0x71], [0x4a, 0x4a, 0x4a],
    [0x7b, 0x7b, 0x7b], [0xa9, 0xff, 0x9f], [0x70, 0x6d, 0xeb], [0xb2, 0xb2, 0xb2],
];

/// Joystick directions and fire, as a bit set: up 1, down 2, left 4, right 8, fire 16.
pub mod joy {
    pub const UP: u8 = 1;
    pub const DOWN: u8 = 2;
    pub const LEFT: u8 = 4;
    pub const RIGHT: u8 = 8;
    pub const FIRE: u8 = 16;
}

/// The keyboard: each key is its place in the 8×8 matrix (row × 8 + column), RESTORE is 64.
pub mod keys {
    pub const NAMES: [&str; 65] = [
        "DEL", "RETURN", "CRSR_RIGHT", "F7", "F1", "F3", "F5", "CRSR_DOWN",
        "3", "W", "A", "4", "Z", "S", "E", "LSHIFT",
        "5", "R", "D", "6", "C", "F", "T", "X",
        "7", "Y", "G", "8", "B", "H", "U", "V",
        "9", "I", "J", "0", "M", "K", "O", "N",
        "PLUS", "P", "L", "MINUS", "PERIOD", "COLON", "AT", "COMMA",
        "POUND", "ASTERISK", "SEMICOLON", "HOME", "RSHIFT", "EQUALS", "UP_ARROW", "SLASH",
        "1", "LEFT_ARROW", "CTRL", "2", "SPACE", "CBM", "Q", "RUN_STOP",
        "RESTORE",
    ];

    /// A key's number by name.
    pub fn named(name: &str) -> Option<u8> {
        NAMES.iter().position(|n| *n == name).map(|i| i as u8)
    }
}

/// What a loaded file was.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    /// A program, put in memory and started.
    Program { name: String },
    /// A tape, inserted and set to load (LOAD, play).
    Tape { names: Vec<String> },
    /// A disk, inserted; its first program loads and runs.
    Disk { names: Vec<String> },
    Cartridge { name: String },
    Snapshot,
}

/// What the machine does by itself after loading something (like typing LOAD).
#[derive(Debug, Clone)]
enum Step {
    /// Wait (up to n frames) for BASIC's READY. after a reset.
    Ready(u32),
    Quickload(Prg),
    /// Type text through the keyboard buffer.
    Type(Vec<u8>),
    PlayTape,
    /// Wait (up to n frames) for READY. below the line that is this text (the LOAD typed has
    /// finished); if it never comes (the program took over), stop here.
    After(String, u32),
}

struct Tape {
    name: String,
    data: Vec<u8>,
}

struct Disk {
    name: String,
    d64: D64,
}

pub struct C64 {
    roms: Arc<Roms>,
    m: Machine,
    keys: Vec<u8>,
    joy: u8,
    /// Which port the joystick is in (most games use 2).
    joy_port: u8,
    steps: VecDeque<Step>,
    typing: VecDeque<u8>,
    tape: Option<Tape>,
    disk: Option<Disk>,
    /// Run tapes as fast as possible while the C64 reads them.
    pub fast_load: bool,
    sample_rate: u32,
    /// Bytes held at a value every frame (cheats).
    locks: Vec<(u16, u8)>,
    mono: Vec<f32>,
}

impl C64 {
    pub fn new(roms: Arc<Roms>) -> C64 {
        let mut m = Machine::new(&roms, 44_100);
        m.set_traps(&[KERNAL_LOAD]);
        C64 {
            roms,
            m,
            keys: Vec::new(),
            joy: 0,
            joy_port: 2,
            steps: VecDeque::new(),
            typing: VecDeque::new(),
            tape: None,
            disk: None,
            fast_load: true,
            sample_rate: 44_100,
            locks: Vec::new(),
            mono: Vec::new(),
        }
    }

    pub fn roms(&self) -> &Arc<Roms> {
        &self.roms
    }

    /// Switches ROMs (a fresh machine; the tape and disk stay in).
    pub fn set_roms(&mut self, roms: Arc<Roms>) {
        self.roms = roms;
        self.m = Machine::new(&self.roms, self.sample_rate);
        self.m.set_traps(&[KERNAL_LOAD]);
        if let Some(t) = &self.tape {
            self.m.tape_insert(&t.data);
        }
        self.steps.clear();
        self.typing.clear();
    }

    pub fn reset(&mut self) {
        self.m.reset();
        self.steps.clear();
        self.typing.clear();
        self.apply_input();
    }

    /// Takes the cartridge out (and resets).
    pub fn remove_cartridge(&mut self) {
        self.m.cart_remove();
        self.reset();
    }

    pub fn has_cartridge(&self) -> bool {
        self.m.cart_present()
    }

    /// Sets which keys are held down (see [`keys`]).
    pub fn set_keys(&mut self, keys: &[u8]) {
        self.keys = keys.iter().copied().filter(|&k| k <= 64).collect();
        self.apply_input();
    }

    /// Sets the joystick (see [`joy`]).
    pub fn set_joystick(&mut self, bits: u8) {
        self.joy = bits & 31;
        self.apply_input();
    }

    /// Which port the joystick is plugged into (1 or 2).
    pub fn set_joystick_port(&mut self, port: u8) {
        self.joy_port = if port == 1 { 1 } else { 2 };
        self.apply_input();
    }

    pub fn joystick_port(&self) -> u8 {
        self.joy_port
    }

    fn apply_input(&mut self) {
        self.m.set_keys(&self.keys);
        let (p1, p2) = if self.joy_port == 1 { (self.joy, 0) } else { (0, self.joy) };
        self.m.set_joysticks(p1, p2);
    }

    /// Loads a file by its name and contents: a program (PRG, P00, T64), a tape (TAP), a disk
    /// (D64), a cartridge (CRT) or a RAD save state (C64S).
    pub fn load(&mut self, name: &str, data: &[u8]) -> Result<Loaded, C64Error> {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let title = name.rsplit(['/', '\\']).next().unwrap_or(name).to_string();
        match ext.as_str() {
            "prg" | "p00" | "t64" => {
                let prg = match ext.as_str() {
                    "prg" => Prg::parse(&title, data)?,
                    "p00" => Prg::parse_p00(data)?,
                    _ => formats::t64(data)?.remove(0),
                };
                let name = prg.name.clone();
                self.start_program(prg);
                Ok(Loaded::Program { name })
            }
            "tap" => {
                let tap = formats::tap(data)?;
                self.m.cart_remove();
                self.reset();
                if !self.m.tape_insert(&tap) {
                    return Err(C64Error::Format("Couldn't put that tape in".into()));
                }
                self.tape = Some(Tape { name: title.clone(), data: tap });
                self.steps.extend([Step::Ready(400), Step::Type(b"LOAD\"\",1\r".to_vec()), Step::PlayTape, Step::After("LOAD\"\",1".into(), 50 * 60 * 15), Step::Type(b"RUN\r".to_vec())]);
                Ok(Loaded::Tape { names: vec![title] })
            }
            "d64" => {
                let d64 = D64::parse(data)?;
                let names: Vec<String> = d64.directory().into_iter().filter(|e| e.kind == "PRG").map(|e| e.name).collect();
                self.m.cart_remove();
                self.reset();
                self.disk = Some(Disk { name: title, d64 });
                self.steps.extend([Step::Ready(400), Step::Type(b"LOAD\"*\",8,1\r".to_vec()), Step::After("LOAD\"*\",8,1".into(), 50 * 60), Step::Type(b"RUN\r".to_vec())]);
                Ok(Loaded::Disk { names })
            }
            "crt" => {
                let crt = Crt::parse(data)?;
                if !self.m.cart_insert(crt.hw, crt.exrom, crt.game, &crt.rom, &crt.has_h) {
                    return Err(C64Error::Format("That cartridge is too big".into()));
                }
                self.reset();
                Ok(Loaded::Cartridge { name: if crt.name.is_empty() { title } else { crt.name } })
            }
            "c64s" => {
                self.load_state(data)?;
                Ok(Loaded::Snapshot)
            }
            other => Err(C64Error::Unsupported(other.to_uppercase())),
        }
    }

    /// Resets, puts a program in memory as LOAD would and starts it (RUN for BASIC, SYS for
    /// machine code elsewhere).
    pub fn start_program(&mut self, prg: Prg) {
        self.m.cart_remove();
        self.reset();
        let start: Vec<u8> = if prg.addr == 0x0801 { b"RUN\r".to_vec() } else { format!("SYS{}\r", prg.addr).into_bytes() };
        self.steps.extend([Step::Ready(400), Step::Quickload(prg), Step::Type(start)]);
    }

    /// Inserts a disk without loading anything from it.
    pub fn insert_disk(&mut self, name: &str, data: &[u8]) -> Result<Vec<String>, C64Error> {
        let d64 = D64::parse(data)?;
        let names = d64.directory().into_iter().map(|e| e.name).collect();
        self.disk = Some(Disk { name: name.to_string(), d64 });
        Ok(names)
    }

    pub fn eject_disk(&mut self) {
        self.disk = None;
    }

    /// The disk: its file name and the files on it.
    pub fn disk_state(&self) -> Option<(String, Vec<String>)> {
        self.disk.as_ref().map(|d| (d.name.clone(), d.d64.directory().into_iter().map(|e| format!("{} ({})", e.name, e.kind)).collect()))
    }

    pub fn eject(&mut self) {
        self.m.tape_remove();
        self.tape = None;
    }

    /// Starts or stops the tape by hand.
    pub fn tape_play(&mut self, play: bool) {
        if self.tape.is_some() {
            if play {
                self.m.tape_play()
            } else {
                self.m.tape_stop()
            }
        }
    }

    pub fn tape_rewind(&mut self) {
        if self.tape.is_some() {
            self.m.tape_stop();
            self.m.tape_rewind();
        }
    }

    /// The tape: (play pressed, progress 0–1, its name).
    pub fn tape_state(&self) -> Option<(bool, f32, &str)> {
        self.tape.as_ref().map(|t| {
            let (pos, size) = self.m.tape_pos();
            (self.m.tape_pressed(), if size == 0 { 0.0 } else { pos as f32 / size as f32 }, t.name.as_str())
        })
    }

    /// Running faster than real time (starting a program, fast loading a tape): the host
    /// should run several frames per tick and skip their sound.
    pub fn is_turbo(&self) -> bool {
        let starting = matches!(self.steps.front(), Some(Step::Ready(_) | Step::After(..) | Step::PlayTape)) || !self.typing.is_empty();
        starting || (self.fast_load && self.tape.is_some() && self.m.tape_motor())
    }

    /// Something is still being loaded and started by the machine itself.
    pub fn is_starting(&self) -> bool {
        !self.steps.is_empty() || !self.typing.is_empty()
    }

    /// Emulates one frame (1/50 s of C64 time).
    pub fn run_frame(&mut self) {
        self.run_frame_inner(None);
    }

    /// Emulates one frame, telling `on` about every clock tick's CPU pins (the lab's tracing;
    /// see [`ffi::pins`]).
    pub fn run_frame_traced(&mut self, on: &mut dyn FnMut(u64)) {
        self.run_frame_inner(Some(on));
    }

    fn run_frame_inner(&mut self, mut trace: Option<&mut dyn FnMut(u64)>) {
        self.step();
        let mut left = FRAME_TICKS;
        while left > 0 {
            let (n, trap) = match trace.as_mut() {
                Some(f) => self.m.run_traced(left, &mut **f),
                None => self.m.run(left),
            };
            left -= n.min(left);
            if trap == Some(KERNAL_LOAD) {
                self.kernal_load();
            }
            if n == 0 {
                break;
            }
        }
        for &(a, v) in &self.locks {
            self.m.poke(a, v);
        }
    }

    /// One frame's worth of the machine's own typing and loading.
    fn step(&mut self) {
        // Typing: the keyboard buffer holds 10 characters.
        if !self.typing.is_empty() {
            let have = self.m.peek(0xc6) as usize;
            if have < 10 {
                let n = (10 - have).min(self.typing.len());
                for i in 0..n {
                    let c = self.typing.pop_front().unwrap_or(b'\r');
                    self.m.poke(0x0277 + (have + i) as u16, c);
                }
                self.m.poke(0xc6, (have + n) as u8);
            }
            return;
        }
        let rows = if matches!(self.steps.front(), Some(Step::Ready(_) | Step::After(..))) { self.screen_text() } else { Vec::new() };
        let typing_done = self.m.peek(0xc6) == 0;
        let Some(step) = self.steps.front_mut() else { return };
        match step {
            Step::Ready(n) => {
                *n = n.saturating_sub(1);
                if *n == 0 || (typing_done && rows.iter().any(|r| r == "READY.")) {
                    self.steps.pop_front();
                }
            }
            Step::Quickload(prg) => {
                let bytes = prg.bytes();
                self.m.quickload(&bytes);
                self.steps.pop_front();
            }
            Step::Type(t) => {
                self.typing.extend(t.iter().copied());
                self.steps.pop_front();
            }
            Step::PlayTape => {
                self.m.tape_play();
                self.steps.pop_front();
            }
            Step::After(text, n) => {
                *n = n.saturating_sub(1);
                // A tape that has played to its end with the motor off: whatever loaded it
                // (the ROM, or the game's own fast loader) has finished.
                let (pos, size) = self.m.tape_pos();
                if self.tape.is_some() && size > 0 && pos >= size && !self.m.tape_motor() {
                    *n = (*n).min(100);
                }
                let done = rows.iter().rposition(|r| r == text).is_some_and(|at| rows[at + 1..].iter().any(|r| r == "READY."));
                if done && typing_done {
                    self.steps.pop_front();
                } else if *n == 0 {
                    self.steps.clear(); // the program took over (or it never loaded)
                }
            }
        }
    }

    /// The KERNAL's LOAD was called: serve the file from the disk image (device 8–11), as if
    /// a 1541 had sent it. Anything else (tape, no disk) carries on in the ROM.
    fn kernal_load(&mut self) {
        let Some(disk) = &self.disk else { return };
        if self.m.cpu_port() & 2 == 0 {
            return; // the KERNAL isn't mapped in: that's RAM code at $FFD5
        }
        let dev = self.m.peek(0xba);
        if !(8..=11).contains(&dev) {
            return;
        }
        let mut r = self.m.regs();
        let len = self.m.peek(0xb7) as u16;
        let at = u16::from_le_bytes([self.m.peek(0xbb), self.m.peek(0xbc)]);
        let name: Vec<u8> = (0..len).map(|i| self.m.peek(at.wrapping_add(i))).collect();
        let secondary = self.m.peek(0xb9);
        // BASIC's main-loop vector before the load (autostarting programs replace it).
        let imain = u16::from_le_bytes([self.m.ram()[0x302], self.m.ram()[0x303]]);
        let mut autostart = None;
        let file = if name == b"$" {
            let mut v = vec![0x01, 0x08];
            v.extend(disk.d64.listing(0x0801));
            Some(v)
        } else {
            disk.d64.find(if name.is_empty() { b"*" } else { &name }).map(|e| disk.d64.read(&e))
        };
        match file.filter(|f| f.len() >= 2) {
            Some(f) => {
                let own = u16::from_le_bytes([f[0], f[1]]);
                // Secondary address 0: where the caller says (BASIC: its program area).
                let start = if secondary == 0 { u16::from_le_bytes([r.x, r.y]) } else { own };
                let body = &f[2..];
                let verify = r.a != 0;
                if !verify {
                    let ram = self.m.ram_mut();
                    for (i, b) in body.iter().enumerate() {
                        ram[(start as usize + i) & 0xffff] = *b;
                    }
                }
                let end = start.wrapping_add(body.len() as u16);
                // A program loaded over the vectors at $0300 that points BASIC's main loop at
                // itself starts as soon as LOAD returns to BASIC. Some BASICs (the Open ROMs')
                // don't go through that vector, so RAD starts it here, as a C64 would.
                let covers = |a: u32| (start as u32) <= a && a < start as u32 + body.len() as u32;
                if !verify && covers(0x302) && covers(0x303) {
                    let target = u16::from_le_bytes([self.m.ram()[0x302], self.m.ram()[0x303]]);
                    let rom = (0xa000..0xc000).contains(&target) || target >= 0xe000;
                    if target != imain && !rom {
                        autostart = Some(target);
                    }
                }
                let [lo, hi] = end.to_le_bytes();
                r.x = lo;
                r.y = hi;
                r.a = 0;
                r.p &= !1; // carry clear: it worked
                self.m.poke(0xae, lo);
                self.m.poke(0xaf, hi);
                self.m.poke(0x90, 0x40);
            }
            None => {
                r.a = 4; // FILE NOT FOUND
                r.p |= 1;
                self.m.poke(0x90, 0x42);
            }
        }
        if let Some(target) = autostart {
            // RAD's own LOAD/RUN typing is done with: the program has taken over.
            self.steps.clear();
            self.typing.clear();
            self.m.poke(0xc6, 0);
            r.s = 0xfa;
            self.m.set_regs(&r);
            self.m.jump(target);
            return;
        }
        // Return to the caller (an RTS).
        let ram = self.m.ram();
        let lo = ram[0x100 + r.s.wrapping_add(1) as usize];
        let hi = ram[0x100 + r.s.wrapping_add(2) as usize];
        r.s = r.s.wrapping_add(2);
        self.m.set_regs(&r);
        self.m.jump(u16::from_le_bytes([lo, hi]).wrapping_add(1));
    }

    /// Memory as the CPU sees it now (ROMs, I/O registers where readable, RAM).
    pub fn memory(&self) -> Vec<u8> {
        (0..=0xffffu16).map(|a| self.m.peek(a)).collect()
    }

    /// All 64K of RAM, including what's under the ROMs and I/O.
    pub fn ram(&self) -> &[u8] {
        self.m.ram()
    }

    /// Writes a byte (to RAM; under the ROMs, the RAM there).
    pub fn poke(&mut self, addr: u16, value: u8) {
        self.m.poke(addr, value);
    }

    /// Writes a byte straight into RAM, whatever is mapped over it (patches).
    pub fn poke_ram(&mut self, addr: u16, value: u8) {
        self.m.ram_mut()[addr as usize] = value;
    }

    /// Memory as a program sees itself: RAM, with a cartridge's ROM where it's mapped in.
    pub fn program_view(&self) -> Vec<u8> {
        let mut v = self.m.ram().to_vec();
        if self.m.cart_present() {
            for a in 0x8000..0xc000u16 {
                v[a as usize] = self.m.peek(a);
            }
        }
        v
    }

    /// The VIC-II's registers and its 16K bank.
    pub fn vic(&self) -> (Vec<u8>, u16) {
        (self.m.vic_regs().to_vec(), self.m.vic_bank())
    }

    /// The CPU port ($01): which ROMs and I/O are mapped in.
    pub fn cpu_port(&self) -> u8 {
        self.m.cpu_port()
    }

    pub(crate) fn cpu_port_ptr(&self) -> *const u8 {
        self.m.cpu_port_ptr()
    }

    /// Keeps a byte at a value every frame (a cheat: infinite lives…); `None` releases it.
    pub fn lock(&mut self, addr: u16, value: Option<u8>) {
        self.locks.retain(|(a, _)| *a != addr);
        if let Some(v) = value {
            self.locks.push((addr, v));
        }
    }

    pub fn regs(&self) -> Regs {
        self.m.regs()
    }

    /// Where the CPU is (its program counter).
    pub fn pc(&self) -> u16 {
        self.m.regs().pc
    }

    /// Picture size in pixels (with the border).
    pub fn size(&self) -> (usize, usize) {
        let (_, w, h, _) = self.m.framebuffer();
        (w, h)
    }

    /// Draws the last frame into `out` (width × height bytes, colour indices 0–15).
    pub fn render(&mut self, out: &mut [u8]) {
        let (fb, w, h, stride) = self.m.framebuffer();
        for y in 0..h {
            let row = &fb[y * stride..y * stride + w];
            for (o, p) in out[y * w..y * w + w].iter_mut().zip(row) {
                *o = p & 15;
            }
        }
    }

    /// Sets the host's sample rate.
    pub fn set_sample_rate(&mut self, rate: u32) {
        if rate != self.sample_rate {
            self.sample_rate = rate;
            self.m.set_sample_rate(rate);
        }
    }

    /// The last frame's sound: interleaved stereo f32 samples.
    pub fn audio(&mut self, out: &mut Vec<f32>) {
        self.m.take_audio(&mut self.mono);
        out.clear();
        out.reserve(self.mono.len() * 2);
        for &s in &self.mono {
            out.push(s);
            out.push(s);
        }
    }

    /// Ends a frame whose sound is skipped (fast loading).
    pub fn skip_audio(&mut self) {
        self.m.take_audio(&mut self.mono);
    }

    /// The machine as a RAD save state ([`C64::load`] it as "….c64s"). It holds the tape and
    /// disk too, and only loads into this version of RAD's C64.
    pub fn save_state(&mut self) -> Result<Vec<u8>, C64Error> {
        let machine = self.m.save();
        let mut body = Vec::with_capacity(machine.len() + 64);
        let put = |b: &mut Vec<u8>, d: &[u8]| {
            b.extend((d.len() as u32).to_le_bytes());
            b.extend_from_slice(d);
        };
        put(&mut body, &machine);
        put(&mut body, self.tape.as_ref().map(|t| t.data.as_slice()).unwrap_or(&[]));
        put(&mut body, self.tape.as_ref().map(|t| t.name.as_bytes()).unwrap_or(&[]));
        put(&mut body, self.disk.as_ref().map(|d| d.d64.data.as_slice()).unwrap_or(&[]));
        put(&mut body, self.disk.as_ref().map(|d| d.name.as_bytes()).unwrap_or(&[]));
        body.push(self.joy_port);
        let mut out = b"RADC64S\0".to_vec();
        out.extend(1u32.to_le_bytes());
        out.extend(Machine::state_version().to_le_bytes());
        out.extend((machine.len() as u32).to_le_bytes());
        out.extend(miniz_oxide::deflate::compress_to_vec(&body, 6));
        Ok(out)
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), C64Error> {
        let bad = |s: &str| C64Error::State(s.to_string());
        if data.len() < 20 || &data[..8] != b"RADC64S\0" {
            return Err(bad("not a C64 save"));
        }
        let word = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        if word(8) != 1 || word(12) != Machine::state_version() {
            return Err(bad("it was saved by a different version of RAD"));
        }
        let body = miniz_oxide::inflate::decompress_to_vec_with_limit(&data[20..], 64 << 20).map_err(|_| bad("it's damaged"))?;
        let mut at = 0usize;
        let mut take = || -> Result<&[u8], C64Error> {
            let n = body.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or_else(|| bad("it's damaged"))?;
            let d = body.get(at + 4..at + 4 + n).ok_or_else(|| bad("it's damaged"))?;
            at += 4 + n;
            Ok(d)
        };
        let machine = take()?.to_vec();
        let tape = take()?.to_vec();
        let tape_name = String::from_utf8_lossy(take()?).to_string();
        let disk = take()?.to_vec();
        let disk_name = String::from_utf8_lossy(take()?).to_string();
        let port = body.get(at).copied().unwrap_or(2);
        if machine.len() != word(16) as usize {
            return Err(bad("it's damaged"));
        }
        // The tape's pulses first (the machine's state refers to a position in them).
        if tape.is_empty() {
            self.m.tape_remove();
            self.tape = None;
        } else {
            self.m.tape_insert(&tape);
            self.tape = Some(Tape { name: tape_name, data: tape });
        }
        if !self.m.load(&machine) {
            return Err(bad("it's damaged"));
        }
        self.disk = if disk.is_empty() { None } else { D64::parse(&disk).ok().map(|d64| Disk { name: disk_name, d64 }) };
        self.joy_port = port;
        self.steps.clear();
        self.typing.clear();
        self.apply_input();
        Ok(())
    }

    /// The text screen: 25 rows of 40 characters (trailing spaces and empty rows at the end
    /// left off). Graphics characters show as `▒`; in a bitmap mode there's no text.
    pub fn screen_text(&self) -> Vec<String> {
        let vic = self.m.vic_regs();
        if vic[0x11] & 0x20 != 0 {
            return Vec::new();
        }
        let lower = vic[0x18] & 2 != 0;
        let base = self.m.vic_bank() as usize + ((vic[0x18] as usize >> 4) * 0x400);
        let ram = self.m.ram();
        let mut rows: Vec<String> = (0..25)
            .map(|r| (0..40).map(|c| screen_char(ram[(base + r * 40 + c) & 0xffff], lower)).collect::<String>().trim_end().to_string())
            .collect();
        while rows.last().is_some_and(|r| r.is_empty()) {
            rows.pop();
        }
        rows
    }

    /// Reads memory as the CPU sees it.
    pub fn peek(&self, addr: u16) -> u8 {
        self.m.peek(addr)
    }

    /// The border colour (0–15).
    pub fn border_color(&self) -> u8 {
        self.m.vic_regs()[0x20] & 15
    }

    /// The CPU has stopped on a JAM instruction (a crash).
    pub fn jammed(&self) -> bool {
        const JAM: [u8; 12] = [0x02, 0x12, 0x22, 0x32, 0x42, 0x52, 0x62, 0x72, 0x92, 0xb2, 0xd2, 0xf2];
        let pc = self.pc();
        JAM.contains(&self.m.peek(pc)) || JAM.contains(&self.m.peek(pc.wrapping_sub(1)))
    }
}

/// A screen code as text.
pub fn screen_char(code: u8, lower: bool) -> char {
    let c = code & 0x7f;
    match c {
        0 => '@',
        1..=26 => (if lower { b'a' } else { b'A' } + c - 1) as char,
        0x1b => '[',
        0x1c => '£',
        0x1d => ']',
        0x1e => '↑',
        0x1f => '←',
        0x20..=0x3f => c as char,
        0x41..=0x5a if lower => (c - 0x41 + b'A') as char,
        0x40 => '-',
        0x60 => ' ',
        _ => '▒',
    }
}

// ------------------------------------------------------------------ checking programs

/// What a program did in its first seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramRun {
    /// It loaded and was started.
    pub started: bool,
    /// The BASIC error it stopped with, if it did: ("SYNTAX ERROR", line) – the line is
    /// missing for an error in direct mode.
    pub error: Option<(String, Option<u16>)>,
    /// The screen as text when the time was up (see [`C64::screen_text`]).
    pub screen: Vec<String>,
    /// The CPU stopped on a JAM instruction.
    pub crashed: bool,
    /// The border colour at the end (handy when nothing's printed).
    pub border: u8,
}

/// Loads a program (PRG and the like) into a fresh C64 and runs it for `seconds` with no keys
/// pressed: what it shows, and the BASIC error if it stopped with one.
pub fn run_program(roms: Arc<Roms>, name: &str, data: &[u8], seconds: u32) -> Result<ProgramRun, C64Error> {
    let mut c = C64::new(roms);
    c.load(name, data)?;
    let mut started = false;
    for _ in 0..50 * 60 * 3 {
        c.run_frame();
        c.skip_audio();
        if !c.is_starting() {
            started = true;
            break;
        }
    }
    for _ in 0..seconds * 50 {
        c.run_frame();
        c.skip_audio();
    }
    let screen = c.screen_text();
    let error = screen.iter().rev().take(6).find_map(|r| parse_error(r));
    Ok(ProgramRun { started, error, crashed: c.jammed(), border: c.border_color(), screen })
}

/// "?SYNTAX  ERROR IN 20" → ("SYNTAX ERROR", Some(20)); "BREAK IN 30" → ("BREAK", Some(30)).
pub fn parse_error(row: &str) -> Option<(String, Option<u16>)> {
    let row = row.trim();
    let (msg, line) = match row.rsplit_once(" IN ") {
        Some((m, l)) if l.trim().chars().all(|c| c.is_ascii_digit()) && !l.trim().is_empty() => (m, l.trim().parse().ok()),
        _ => (row, None),
    };
    let msg = msg.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(m) = msg.strip_prefix('?').filter(|m| m.ends_with("ERROR")) {
        return Some((m.to_string(), line));
    }
    if msg == "BREAK" && line.is_some() {
        return Some((msg, line));
    }
    None
}

#[cfg(test)]
mod tests;

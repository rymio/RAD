//! The C side (shim.c over chips' c64.h) and a safe owner for one machine.

use std::os::raw::c_void;

#[repr(C)]
struct Rc64 {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Regs {
    pub pc: u16,
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub s: u8,
    pub p: u8,
}

type TickFn = extern "C" fn(*mut c_void, u64);

extern "C" {
    fn rc64_new(chars: *const u8, basic: *const u8, kernal: *const u8, sample_rate: i32) -> *mut Rc64;
    fn rc64_free(r: *mut Rc64);
    fn rc64_reset(r: *mut Rc64);
    fn rc64_set_sample_rate(r: *mut Rc64, hz: i32);
    fn rc64_set_traps(r: *mut Rc64, addrs: *const u16, n: i32);
    fn rc64_trapped(r: *mut Rc64) -> i32;
    fn rc64_run(r: *mut Rc64, ticks: u32, f: Option<TickFn>, user: *mut c_void) -> u32;
    fn rc64_jump(r: *mut Rc64, pc: u16);
    fn rc64_get_regs(r: *mut Rc64, out: *mut Regs);
    fn rc64_set_regs(r: *mut Rc64, inp: *const Regs);
    fn rc64_peek(r: *mut Rc64, addr: u16) -> u8;
    fn rc64_poke(r: *mut Rc64, addr: u16, v: u8);
    fn rc64_ram(r: *mut Rc64) -> *mut u8;
    fn rc64_vic_regs(r: *mut Rc64) -> *const u8;
    fn rc64_vic_bank(r: *mut Rc64) -> u16;
    fn rc64_cpu_port(r: *mut Rc64) -> u8;
    fn rc64_cpu_port_ptr(r: *mut Rc64) -> *const u8;
    fn rc64_framebuffer(r: *mut Rc64, w: *mut i32, h: *mut i32, stride: *mut i32) -> *const u8;
    fn rc64_take_audio(r: *mut Rc64, out: *mut f32, max: i32) -> i32;
    fn rc64_keys(r: *mut Rc64, keys: *const u8, n: i32);
    fn rc64_joystick(r: *mut Rc64, port1: u8, port2: u8);
    fn rc64_quickload(r: *mut Rc64, data: *const u8, len: usize) -> bool;
    fn rc64_tape_insert(r: *mut Rc64, data: *const u8, len: usize) -> bool;
    fn rc64_tape_remove(r: *mut Rc64);
    fn rc64_tape_play(r: *mut Rc64);
    fn rc64_tape_stop(r: *mut Rc64);
    fn rc64_tape_motor(r: *mut Rc64) -> bool;
    fn rc64_tape_pressed(r: *mut Rc64) -> bool;
    fn rc64_tape_pos(r: *mut Rc64, pos: *mut u32, size: *mut u32);
    fn rc64_tape_rewind(r: *mut Rc64);
    fn rc64_cart_insert(r: *mut Rc64, ty: u16, exrom: bool, game: bool, rom: *const u8, len: usize, has_h: *const u8) -> bool;
    fn rc64_cart_remove(r: *mut Rc64);
    fn rc64_cart_present(r: *mut Rc64) -> bool;
    fn rc64_state_size() -> usize;
    fn rc64_state_version() -> u32;
    fn rc64_save(r: *mut Rc64, out: *mut u8);
    fn rc64_load(r: *mut Rc64, inp: *const u8) -> bool;
}

/// The cartridge's banks (16K each: ROML then ROMH) and how many there can be.
pub const CART_MAX: usize = 1 << 20;
pub const CART_BANKS: usize = CART_MAX / 0x4000;

/// The CPU's pins in a tick (for tracing).
pub mod pins {
    pub const RW: u64 = 1 << 24;
    pub const SYNC: u64 = 1 << 25;
    pub fn addr(p: u64) -> u16 {
        p as u16
    }
    pub fn data(p: u64) -> u8 {
        (p >> 16) as u8
    }
}

/// One emulated C64 (owned; freed on drop).
pub struct Machine(*mut Rc64);

// The machine is plain memory owned by this handle; it's used from one thread at a time.
unsafe impl Send for Machine {}

impl Drop for Machine {
    fn drop(&mut self) {
        unsafe { rc64_free(self.0) }
    }
}

extern "C" fn trampoline(user: *mut c_void, pins: u64) {
    let f = unsafe { &mut *(user as *mut &mut dyn FnMut(u64)) };
    f(pins);
}

impl Machine {
    pub fn new(roms: &crate::Roms, sample_rate: u32) -> Machine {
        let p = unsafe { rc64_new(roms.chars.as_ptr(), roms.basic.as_ptr(), roms.kernal.as_ptr(), sample_rate as i32) };
        assert!(!p.is_null(), "out of memory for the C64");
        Machine(p)
    }

    pub fn reset(&mut self) {
        unsafe { rc64_reset(self.0) }
    }

    pub fn set_sample_rate(&mut self, hz: u32) {
        unsafe { rc64_set_sample_rate(self.0, hz as i32) }
    }

    pub fn set_traps(&mut self, addrs: &[u16]) {
        unsafe { rc64_set_traps(self.0, addrs.as_ptr(), addrs.len() as i32) }
    }

    /// Runs up to `ticks` cycles; returns (ticks run, trap address hit).
    pub fn run(&mut self, ticks: u32) -> (u32, Option<u16>) {
        let n = unsafe { rc64_run(self.0, ticks, None, std::ptr::null_mut()) };
        (n, self.trapped())
    }

    /// Like [`Machine::run`], calling `f` with every tick's pins.
    pub fn run_traced(&mut self, ticks: u32, mut f: &mut dyn FnMut(u64)) -> (u32, Option<u16>) {
        let user = &mut f as *mut &mut dyn FnMut(u64) as *mut c_void;
        let n = unsafe { rc64_run(self.0, ticks, Some(trampoline), user) };
        (n, self.trapped())
    }

    fn trapped(&self) -> Option<u16> {
        let t = unsafe { rc64_trapped(self.0) };
        (t >= 0).then_some(t as u16)
    }

    pub fn jump(&mut self, pc: u16) {
        unsafe { rc64_jump(self.0, pc) }
    }

    pub fn regs(&self) -> Regs {
        let mut r = Regs::default();
        unsafe { rc64_get_regs(self.0, &mut r) };
        r
    }

    pub fn set_regs(&mut self, r: &Regs) {
        unsafe { rc64_set_regs(self.0, r) }
    }

    pub fn peek(&self, addr: u16) -> u8 {
        unsafe { rc64_peek(self.0, addr) }
    }

    pub fn poke(&mut self, addr: u16, v: u8) {
        unsafe { rc64_poke(self.0, addr, v) }
    }

    /// All 64K of RAM (including what's hidden under the ROMs and I/O).
    pub fn ram(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(rc64_ram(self.0), 0x10000) }
    }

    pub fn ram_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(rc64_ram(self.0), 0x10000) }
    }

    pub fn vic_regs(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(rc64_vic_regs(self.0), 0x2f) }
    }

    /// The VIC-II's 16K bank (0x0000, 0x4000, 0x8000 or 0xC000).
    pub fn vic_bank(&self) -> u16 {
        unsafe { rc64_vic_bank(self.0) }
    }

    pub fn cpu_port(&self) -> u8 {
        unsafe { rc64_cpu_port(self.0) }
    }

    /// Where the CPU port's value lives (for reading it during a traced run).
    pub fn cpu_port_ptr(&self) -> *const u8 {
        unsafe { rc64_cpu_port_ptr(self.0) }
    }

    /// The visible picture: (pixels, width, height, row stride); one colour index per byte.
    pub fn framebuffer(&self) -> (&[u8], usize, usize, usize) {
        let (mut w, mut h, mut s) = (0, 0, 0);
        let p = unsafe { rc64_framebuffer(self.0, &mut w, &mut h, &mut s) };
        let (w, h, s) = (w as usize, h as usize, s as usize);
        (unsafe { std::slice::from_raw_parts(p, s * h) }, w, h, s)
    }

    /// The sound since the last call (mono samples at the sample rate).
    pub fn take_audio(&mut self, out: &mut Vec<f32>) {
        out.resize(16384, 0.0);
        let n = unsafe { rc64_take_audio(self.0, out.as_mut_ptr(), out.len() as i32) };
        out.truncate(n as usize);
    }

    pub fn set_keys(&mut self, keys: &[u8]) {
        unsafe { rc64_keys(self.0, keys.as_ptr(), keys.len() as i32) }
    }

    pub fn set_joysticks(&mut self, port1: u8, port2: u8) {
        unsafe { rc64_joystick(self.0, port1, port2) }
    }

    pub fn quickload(&mut self, prg: &[u8]) -> bool {
        unsafe { rc64_quickload(self.0, prg.as_ptr(), prg.len()) }
    }

    pub fn tape_insert(&mut self, tap: &[u8]) -> bool {
        unsafe { rc64_tape_insert(self.0, tap.as_ptr(), tap.len()) }
    }
    pub fn tape_remove(&mut self) {
        unsafe { rc64_tape_remove(self.0) }
    }
    pub fn tape_play(&mut self) {
        unsafe { rc64_tape_play(self.0) }
    }
    pub fn tape_stop(&mut self) {
        unsafe { rc64_tape_stop(self.0) }
    }
    pub fn tape_motor(&self) -> bool {
        unsafe { rc64_tape_motor(self.0) }
    }
    pub fn tape_pressed(&self) -> bool {
        unsafe { rc64_tape_pressed(self.0) }
    }
    pub fn tape_rewind(&mut self) {
        unsafe { rc64_tape_rewind(self.0) }
    }
    /// (position, size) in bytes of the tape's pulse data.
    pub fn tape_pos(&self) -> (u32, u32) {
        let (mut p, mut s) = (0, 0);
        unsafe { rc64_tape_pos(self.0, &mut p, &mut s) };
        (p, s)
    }

    pub fn cart_insert(&mut self, ty: u16, exrom: bool, game: bool, rom: &[u8], has_h: &[u8; CART_BANKS]) -> bool {
        unsafe { rc64_cart_insert(self.0, ty, exrom, game, rom.as_ptr(), rom.len(), has_h.as_ptr()) }
    }
    pub fn cart_remove(&mut self) {
        unsafe { rc64_cart_remove(self.0) }
    }
    pub fn cart_present(&self) -> bool {
        unsafe { rc64_cart_present(self.0) }
    }

    pub fn state_version() -> u32 {
        unsafe { rc64_state_version() }
    }

    /// The whole machine as bytes (this build of RAD can load it back).
    pub fn save(&self) -> Vec<u8> {
        let mut v = vec![0u8; unsafe { rc64_state_size() }];
        unsafe { rc64_save(self.0, v.as_mut_ptr()) };
        v
    }

    pub fn load(&mut self, state: &[u8]) -> bool {
        state.len() == unsafe { rc64_state_size() } && unsafe { rc64_load(self.0, state.as_ptr()) }
    }
}

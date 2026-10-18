//! The C64 lab: taking a game apart to see how it works, and changing it.
//!
//! [`analyze`] loads a game (program, tape, disk or cartridge), lets it start (and unpack
//! itself: most C64 programs are packed), takes a snapshot, then runs it for a while with every
//! clock tick traced, recording what really runs: which addresses are code, which bytes change,
//! where jumps and calls go, which chip registers (VIC-II, SID, CIAs) its own code uses. Code
//! reachable from what ran (but not run yet) is found by following branches.
//!
//! [`listing`] turns memory into labelled 6502 assembly (64tass syntax) that [`source`] writes
//! out so it assembles back to the same bytes; [`apply_patches`] puts changes into the loaded
//! game for testing.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use crate::ffi::pins;
use crate::{basic, formats, joy, keys, C64Error, Roms, C64};

// ------------------------------------------------------------------ the 6502

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Imp,
    Acc,
    Imm,
    Zp,
    Zpx,
    Zpy,
    Abs,
    Abx,
    Aby,
    Ind,
    Izx,
    Izy,
    Rel,
}

impl Mode {
    pub fn len(self) -> u16 {
        match self {
            Mode::Imp | Mode::Acc => 1,
            Mode::Abs | Mode::Abx | Mode::Aby | Mode::Ind => 3,
            _ => 2,
        }
    }
}

/// Every opcode: mnemonic, addressing mode, undocumented ("illegal").
pub const OPS: [(&str, Mode, bool); 256] = [
    ("BRK", Mode::Imp, false),
    ("ORA", Mode::Izx, false),
    ("JAM", Mode::Imp, true),
    ("SLO", Mode::Izx, true),
    ("NOP", Mode::Zp, true),
    ("ORA", Mode::Zp, false),
    ("ASL", Mode::Zp, false),
    ("SLO", Mode::Zp, true),
    ("PHP", Mode::Imp, false),
    ("ORA", Mode::Imm, false),
    ("ASL", Mode::Acc, false),
    ("ANC", Mode::Imm, true),
    ("NOP", Mode::Abs, true),
    ("ORA", Mode::Abs, false),
    ("ASL", Mode::Abs, false),
    ("SLO", Mode::Abs, true),
    ("BPL", Mode::Rel, false),
    ("ORA", Mode::Izy, false),
    ("JAM", Mode::Imp, true),
    ("SLO", Mode::Izy, true),
    ("NOP", Mode::Zpx, true),
    ("ORA", Mode::Zpx, false),
    ("ASL", Mode::Zpx, false),
    ("SLO", Mode::Zpx, true),
    ("CLC", Mode::Imp, false),
    ("ORA", Mode::Aby, false),
    ("NOP", Mode::Imp, true),
    ("SLO", Mode::Aby, true),
    ("NOP", Mode::Abx, true),
    ("ORA", Mode::Abx, false),
    ("ASL", Mode::Abx, false),
    ("SLO", Mode::Abx, true),
    ("JSR", Mode::Abs, false),
    ("AND", Mode::Izx, false),
    ("JAM", Mode::Imp, true),
    ("RLA", Mode::Izx, true),
    ("BIT", Mode::Zp, false),
    ("AND", Mode::Zp, false),
    ("ROL", Mode::Zp, false),
    ("RLA", Mode::Zp, true),
    ("PLP", Mode::Imp, false),
    ("AND", Mode::Imm, false),
    ("ROL", Mode::Acc, false),
    ("ANC", Mode::Imm, true),
    ("BIT", Mode::Abs, false),
    ("AND", Mode::Abs, false),
    ("ROL", Mode::Abs, false),
    ("RLA", Mode::Abs, true),
    ("BMI", Mode::Rel, false),
    ("AND", Mode::Izy, false),
    ("JAM", Mode::Imp, true),
    ("RLA", Mode::Izy, true),
    ("NOP", Mode::Zpx, true),
    ("AND", Mode::Zpx, false),
    ("ROL", Mode::Zpx, false),
    ("RLA", Mode::Zpx, true),
    ("SEC", Mode::Imp, false),
    ("AND", Mode::Aby, false),
    ("NOP", Mode::Imp, true),
    ("RLA", Mode::Aby, true),
    ("NOP", Mode::Abx, true),
    ("AND", Mode::Abx, false),
    ("ROL", Mode::Abx, false),
    ("RLA", Mode::Abx, true),
    ("RTI", Mode::Imp, false),
    ("EOR", Mode::Izx, false),
    ("JAM", Mode::Imp, true),
    ("SRE", Mode::Izx, true),
    ("NOP", Mode::Zp, true),
    ("EOR", Mode::Zp, false),
    ("LSR", Mode::Zp, false),
    ("SRE", Mode::Zp, true),
    ("PHA", Mode::Imp, false),
    ("EOR", Mode::Imm, false),
    ("LSR", Mode::Acc, false),
    ("ALR", Mode::Imm, true),
    ("JMP", Mode::Abs, false),
    ("EOR", Mode::Abs, false),
    ("LSR", Mode::Abs, false),
    ("SRE", Mode::Abs, true),
    ("BVC", Mode::Rel, false),
    ("EOR", Mode::Izy, false),
    ("JAM", Mode::Imp, true),
    ("SRE", Mode::Izy, true),
    ("NOP", Mode::Zpx, true),
    ("EOR", Mode::Zpx, false),
    ("LSR", Mode::Zpx, false),
    ("SRE", Mode::Zpx, true),
    ("CLI", Mode::Imp, false),
    ("EOR", Mode::Aby, false),
    ("NOP", Mode::Imp, true),
    ("SRE", Mode::Aby, true),
    ("NOP", Mode::Abx, true),
    ("EOR", Mode::Abx, false),
    ("LSR", Mode::Abx, false),
    ("SRE", Mode::Abx, true),
    ("RTS", Mode::Imp, false),
    ("ADC", Mode::Izx, false),
    ("JAM", Mode::Imp, true),
    ("RRA", Mode::Izx, true),
    ("NOP", Mode::Zp, true),
    ("ADC", Mode::Zp, false),
    ("ROR", Mode::Zp, false),
    ("RRA", Mode::Zp, true),
    ("PLA", Mode::Imp, false),
    ("ADC", Mode::Imm, false),
    ("ROR", Mode::Acc, false),
    ("ARR", Mode::Imm, true),
    ("JMP", Mode::Ind, false),
    ("ADC", Mode::Abs, false),
    ("ROR", Mode::Abs, false),
    ("RRA", Mode::Abs, true),
    ("BVS", Mode::Rel, false),
    ("ADC", Mode::Izy, false),
    ("JAM", Mode::Imp, true),
    ("RRA", Mode::Izy, true),
    ("NOP", Mode::Zpx, true),
    ("ADC", Mode::Zpx, false),
    ("ROR", Mode::Zpx, false),
    ("RRA", Mode::Zpx, true),
    ("SEI", Mode::Imp, false),
    ("ADC", Mode::Aby, false),
    ("NOP", Mode::Imp, true),
    ("RRA", Mode::Aby, true),
    ("NOP", Mode::Abx, true),
    ("ADC", Mode::Abx, false),
    ("ROR", Mode::Abx, false),
    ("RRA", Mode::Abx, true),
    ("NOP", Mode::Imm, true),
    ("STA", Mode::Izx, false),
    ("NOP", Mode::Imm, true),
    ("SAX", Mode::Izx, true),
    ("STY", Mode::Zp, false),
    ("STA", Mode::Zp, false),
    ("STX", Mode::Zp, false),
    ("SAX", Mode::Zp, true),
    ("DEY", Mode::Imp, false),
    ("NOP", Mode::Imm, true),
    ("TXA", Mode::Imp, false),
    ("ANE", Mode::Imm, true),
    ("STY", Mode::Abs, false),
    ("STA", Mode::Abs, false),
    ("STX", Mode::Abs, false),
    ("SAX", Mode::Abs, true),
    ("BCC", Mode::Rel, false),
    ("STA", Mode::Izy, false),
    ("JAM", Mode::Imp, true),
    ("SHA", Mode::Izy, true),
    ("STY", Mode::Zpx, false),
    ("STA", Mode::Zpx, false),
    ("STX", Mode::Zpy, false),
    ("SAX", Mode::Zpy, true),
    ("TYA", Mode::Imp, false),
    ("STA", Mode::Aby, false),
    ("TXS", Mode::Imp, false),
    ("TAS", Mode::Aby, true),
    ("SHY", Mode::Abx, true),
    ("STA", Mode::Abx, false),
    ("SHX", Mode::Aby, true),
    ("SHA", Mode::Aby, true),
    ("LDY", Mode::Imm, false),
    ("LDA", Mode::Izx, false),
    ("LDX", Mode::Imm, false),
    ("LAX", Mode::Izx, true),
    ("LDY", Mode::Zp, false),
    ("LDA", Mode::Zp, false),
    ("LDX", Mode::Zp, false),
    ("LAX", Mode::Zp, true),
    ("TAY", Mode::Imp, false),
    ("LDA", Mode::Imm, false),
    ("TAX", Mode::Imp, false),
    ("LXA", Mode::Imm, true),
    ("LDY", Mode::Abs, false),
    ("LDA", Mode::Abs, false),
    ("LDX", Mode::Abs, false),
    ("LAX", Mode::Abs, true),
    ("BCS", Mode::Rel, false),
    ("LDA", Mode::Izy, false),
    ("JAM", Mode::Imp, true),
    ("LAX", Mode::Izy, true),
    ("LDY", Mode::Zpx, false),
    ("LDA", Mode::Zpx, false),
    ("LDX", Mode::Zpy, false),
    ("LAX", Mode::Zpy, true),
    ("CLV", Mode::Imp, false),
    ("LDA", Mode::Aby, false),
    ("TSX", Mode::Imp, false),
    ("LAS", Mode::Aby, true),
    ("LDY", Mode::Abx, false),
    ("LDA", Mode::Abx, false),
    ("LDX", Mode::Aby, false),
    ("LAX", Mode::Aby, true),
    ("CPY", Mode::Imm, false),
    ("CMP", Mode::Izx, false),
    ("NOP", Mode::Imm, true),
    ("DCP", Mode::Izx, true),
    ("CPY", Mode::Zp, false),
    ("CMP", Mode::Zp, false),
    ("DEC", Mode::Zp, false),
    ("DCP", Mode::Zp, true),
    ("INY", Mode::Imp, false),
    ("CMP", Mode::Imm, false),
    ("DEX", Mode::Imp, false),
    ("SBX", Mode::Imm, true),
    ("CPY", Mode::Abs, false),
    ("CMP", Mode::Abs, false),
    ("DEC", Mode::Abs, false),
    ("DCP", Mode::Abs, true),
    ("BNE", Mode::Rel, false),
    ("CMP", Mode::Izy, false),
    ("JAM", Mode::Imp, true),
    ("DCP", Mode::Izy, true),
    ("NOP", Mode::Zpx, true),
    ("CMP", Mode::Zpx, false),
    ("DEC", Mode::Zpx, false),
    ("DCP", Mode::Zpx, true),
    ("CLD", Mode::Imp, false),
    ("CMP", Mode::Aby, false),
    ("NOP", Mode::Imp, true),
    ("DCP", Mode::Aby, true),
    ("NOP", Mode::Abx, true),
    ("CMP", Mode::Abx, false),
    ("DEC", Mode::Abx, false),
    ("DCP", Mode::Abx, true),
    ("CPX", Mode::Imm, false),
    ("SBC", Mode::Izx, false),
    ("NOP", Mode::Imm, true),
    ("ISB", Mode::Izx, true),
    ("CPX", Mode::Zp, false),
    ("SBC", Mode::Zp, false),
    ("INC", Mode::Zp, false),
    ("ISB", Mode::Zp, true),
    ("INX", Mode::Imp, false),
    ("SBC", Mode::Imm, false),
    ("NOP", Mode::Imp, false),
    ("SBC", Mode::Imm, true),
    ("CPX", Mode::Abs, false),
    ("SBC", Mode::Abs, false),
    ("INC", Mode::Abs, false),
    ("ISB", Mode::Abs, true),
    ("BEQ", Mode::Rel, false),
    ("SBC", Mode::Izy, false),
    ("JAM", Mode::Imp, true),
    ("ISB", Mode::Izy, true),
    ("NOP", Mode::Zpx, true),
    ("SBC", Mode::Zpx, false),
    ("INC", Mode::Zpx, false),
    ("ISB", Mode::Zpx, true),
    ("SED", Mode::Imp, false),
    ("SBC", Mode::Aby, false),
    ("NOP", Mode::Imp, true),
    ("ISB", Mode::Aby, true),
    ("NOP", Mode::Abx, true),
    ("SBC", Mode::Abx, false),
    ("INC", Mode::Abx, false),
    ("ISB", Mode::Abx, true),
];

/// A decoded instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct Instr {
    pub pc: u16,
    pub bytes: Vec<u8>,
    pub name: &'static str,
    pub mode: Mode,
    pub illegal: bool,
    /// The operand (a byte or an address; for branches, the target).
    pub operand: u16,
}

impl Instr {
    fn from_bytes(pc: u16, bytes: &[u8]) -> Instr {
        let (name, mode, illegal) = OPS[bytes[0] as usize];
        let len = mode.len() as usize;
        let b: Vec<u8> = bytes.iter().copied().chain(std::iter::repeat(0)).take(len).collect();
        let operand = match mode {
            Mode::Imp | Mode::Acc => 0,
            Mode::Rel => pc.wrapping_add(2).wrapping_add(b[1] as i8 as u16),
            _ if len == 2 => b[1] as u16,
            _ => u16::from_le_bytes([b[1], b[2]]),
        };
        Instr { pc, bytes: b, name, mode, illegal, operand }
    }

    /// Where it jumps or calls (JMP abs, JSR, branches).
    pub fn target(&self) -> Option<u16> {
        match (self.name, self.mode) {
            ("JMP" | "JSR", Mode::Abs) | (_, Mode::Rel) => Some(self.operand),
            _ => None,
        }
    }

    /// The memory it reads or writes (not for jumps: see [`Instr::target`]).
    pub fn data_addr(&self) -> Option<u16> {
        match self.mode {
            Mode::Zp | Mode::Zpx | Mode::Zpy | Mode::Abx | Mode::Aby | Mode::Izx | Mode::Izy | Mode::Ind => Some(self.operand),
            Mode::Abs if !matches!(self.name, "JMP" | "JSR") => Some(self.operand),
            _ => None,
        }
    }

    /// After this, the next bytes aren't reached by falling through.
    pub fn ends_flow(&self) -> bool {
        matches!(self.name, "JMP" | "RTS" | "RTI" | "BRK" | "JAM")
    }
}

pub fn decode(mem: &[u8], pc: u16) -> Instr {
    let b: Vec<u8> = (0..3).map(|i| mem[pc.wrapping_add(i) as usize]).collect();
    Instr::from_bytes(pc, &b)
}

// ------------------------------------------------------------------ names

/// The KERNAL's jump table and some well-known ROM addresses.
pub const KERNAL: &[(u16, &str, &str)] = &[
    (0xff81, "CINT", "set up the screen editor and VIC-II"),
    (0xff84, "IOINIT", "set up the CIAs"),
    (0xff87, "RAMTAS", "test and clear RAM"),
    (0xff8a, "RESTOR", "restore the default I/O vectors"),
    (0xff8d, "VECTOR", "read or set the I/O vectors"),
    (0xff90, "SETMSG", "KERNAL messages on/off"),
    (0xff93, "SECOND", "serial bus: secondary address after LISTEN"),
    (0xff96, "TKSA", "serial bus: secondary address after TALK"),
    (0xff99, "MEMTOP", "read or set the top of memory"),
    (0xff9c, "MEMBOT", "read or set the bottom of memory"),
    (0xff9f, "SCNKEY", "scan the keyboard"),
    (0xffa2, "SETTMO", "serial bus timeout"),
    (0xffa5, "ACPTR", "serial bus: read a byte"),
    (0xffa8, "CIOUT", "serial bus: send a byte"),
    (0xffab, "UNTLK", "serial bus: UNTALK"),
    (0xffae, "UNLSN", "serial bus: UNLISTEN"),
    (0xffb1, "LISTEN", "serial bus: LISTEN"),
    (0xffb4, "TALK", "serial bus: TALK"),
    (0xffb7, "READST", "read the I/O status"),
    (0xffba, "SETLFS", "set file number, device, secondary address"),
    (0xffbd, "SETNAM", "set the file name (A = length, X/Y = address)"),
    (0xffc0, "OPEN", "open a file"),
    (0xffc3, "CLOSE", "close a file"),
    (0xffc6, "CHKIN", "use a file for input"),
    (0xffc9, "CHKOUT", "use a file for output"),
    (0xffcc, "CLRCHN", "back to keyboard and screen"),
    (0xffcf, "CHRIN", "read a character"),
    (0xffd2, "CHROUT", "print a character (A)"),
    (0xffd5, "LOAD", "load a file (A = 0 load, X/Y = address)"),
    (0xffd8, "SAVE", "save memory to a file"),
    (0xffdb, "SETTIM", "set the jiffy clock"),
    (0xffde, "RDTIM", "read the jiffy clock"),
    (0xffe1, "STOP", "check the RUN/STOP key"),
    (0xffe4, "GETIN", "get a key from the keyboard buffer"),
    (0xffe7, "CLALL", "close all files"),
    (0xffea, "UDTIM", "advance the jiffy clock"),
    (0xffed, "SCREEN", "screen size (X = columns, Y = rows)"),
    (0xfff0, "PLOT", "read or set the cursor (X = row, Y = column)"),
    (0xfff3, "IOBASE", "where the I/O chips are"),
    (0xea31, "IRQ_DEFAULT", "the KERNAL's interrupt routine (keyboard, clock, cursor)"),
    (0xea81, "IRQ_EXIT", "the end of an interrupt: restore registers, RTI"),
    (0xe544, "CLEAR_SCREEN", "clear the screen"),
    (0xe566, "CURSOR_HOME", "cursor to the top left"),
    (0xfce2, "RESET", "the reset routine"),
];

/// Hardware registers and well-known memory: address, name, what it is.
pub const REGS: &[(u16, &str, &str)] = &[
    (0x0000, "CPU_DDR", "CPU port: data direction"),
    (0x0001, "CPU_PORT", "CPU port: which ROMs and I/O are mapped in (bits 0–2), tape motor"),
    (0x00c5, "LSTX", "the key held now (64 = none)"),
    (0x00c6, "NDX", "characters in the keyboard buffer"),
    (0x0277, "KEYD", "the keyboard buffer (10 characters)"),
    (0x0314, "CINV", "the interrupt (IRQ) vector"),
    (0x0316, "CBINV", "the BRK vector"),
    (0x0318, "NMINV", "the NMI vector (RESTORE)"),
    (0x0326, "IBSOUT", "the CHROUT vector"),
    (0x0328, "ISTOP", "the STOP vector"),
    (0x0330, "ILOAD", "the LOAD vector"),
    (0xd000, "SPR0_X", "VIC-II: sprite 0 X"), (0xd001, "SPR0_Y", "VIC-II: sprite 0 Y"),
    (0xd002, "SPR1_X", "VIC-II: sprite 1 X"), (0xd003, "SPR1_Y", "VIC-II: sprite 1 Y"),
    (0xd004, "SPR2_X", "VIC-II: sprite 2 X"), (0xd005, "SPR2_Y", "VIC-II: sprite 2 Y"),
    (0xd006, "SPR3_X", "VIC-II: sprite 3 X"), (0xd007, "SPR3_Y", "VIC-II: sprite 3 Y"),
    (0xd008, "SPR4_X", "VIC-II: sprite 4 X"), (0xd009, "SPR4_Y", "VIC-II: sprite 4 Y"),
    (0xd00a, "SPR5_X", "VIC-II: sprite 5 X"), (0xd00b, "SPR5_Y", "VIC-II: sprite 5 Y"),
    (0xd00c, "SPR6_X", "VIC-II: sprite 6 X"), (0xd00d, "SPR6_Y", "VIC-II: sprite 6 Y"),
    (0xd00e, "SPR7_X", "VIC-II: sprite 7 X"), (0xd00f, "SPR7_Y", "VIC-II: sprite 7 Y"),
    (0xd010, "SPR_X_HI", "VIC-II: bit 8 of each sprite's X"),
    (0xd011, "VIC_CTRL1", "VIC-II: vertical scroll, 25 rows, screen on, bitmap mode, raster bit 8"),
    (0xd012, "VIC_RASTER", "VIC-II: the raster line (read) / raster interrupt line (write)"),
    (0xd013, "LIGHTPEN_X", "VIC-II: light pen X"),
    (0xd014, "LIGHTPEN_Y", "VIC-II: light pen Y"),
    (0xd015, "SPR_ENABLE", "VIC-II: sprites on"),
    (0xd016, "VIC_CTRL2", "VIC-II: horizontal scroll, 40 columns, multicolour mode"),
    (0xd017, "SPR_EXPAND_Y", "VIC-II: sprites double height"),
    (0xd018, "VIC_MEMORY", "VIC-II: where the screen and character set are"),
    (0xd019, "VIC_IRQ", "VIC-II: interrupt flags (write to acknowledge)"),
    (0xd01a, "VIC_IRQ_MASK", "VIC-II: interrupts on (bit 0 = raster)"),
    (0xd01b, "SPR_BEHIND", "VIC-II: sprites behind the background"),
    (0xd01c, "SPR_MULTI", "VIC-II: multicolour sprites"),
    (0xd01d, "SPR_EXPAND_X", "VIC-II: sprites double width"),
    (0xd01e, "SPR_HIT_SPR", "VIC-II: sprite–sprite collisions"),
    (0xd01f, "SPR_HIT_BG", "VIC-II: sprite–background collisions"),
    (0xd020, "BORDER", "VIC-II: border colour"),
    (0xd021, "BACKGROUND", "VIC-II: background colour"),
    (0xd022, "BG_COLOR1", "VIC-II: background colour 1 (multicolour)"),
    (0xd023, "BG_COLOR2", "VIC-II: background colour 2"),
    (0xd024, "BG_COLOR3", "VIC-II: background colour 3"),
    (0xd025, "SPR_MULTI1", "VIC-II: sprite multicolour 1"),
    (0xd026, "SPR_MULTI2", "VIC-II: sprite multicolour 2"),
    (0xd027, "SPR0_COLOR", "VIC-II: sprite 0 colour"), (0xd028, "SPR1_COLOR", "VIC-II: sprite 1 colour"),
    (0xd029, "SPR2_COLOR", "VIC-II: sprite 2 colour"), (0xd02a, "SPR3_COLOR", "VIC-II: sprite 3 colour"),
    (0xd02b, "SPR4_COLOR", "VIC-II: sprite 4 colour"), (0xd02c, "SPR5_COLOR", "VIC-II: sprite 5 colour"),
    (0xd02d, "SPR6_COLOR", "VIC-II: sprite 6 colour"), (0xd02e, "SPR7_COLOR", "VIC-II: sprite 7 colour"),
    (0xd400, "SID_V1_FREQ", "SID: voice 1 frequency"), (0xd402, "SID_V1_PULSE", "SID: voice 1 pulse width"),
    (0xd404, "SID_V1_CTRL", "SID: voice 1 waveform and gate"), (0xd405, "SID_V1_AD", "SID: voice 1 attack/decay"),
    (0xd406, "SID_V1_SR", "SID: voice 1 sustain/release"),
    (0xd407, "SID_V2_FREQ", "SID: voice 2 frequency"), (0xd409, "SID_V2_PULSE", "SID: voice 2 pulse width"),
    (0xd40b, "SID_V2_CTRL", "SID: voice 2 waveform and gate"), (0xd40c, "SID_V2_AD", "SID: voice 2 attack/decay"),
    (0xd40d, "SID_V2_SR", "SID: voice 2 sustain/release"),
    (0xd40e, "SID_V3_FREQ", "SID: voice 3 frequency"), (0xd410, "SID_V3_PULSE", "SID: voice 3 pulse width"),
    (0xd412, "SID_V3_CTRL", "SID: voice 3 waveform and gate"), (0xd413, "SID_V3_AD", "SID: voice 3 attack/decay"),
    (0xd414, "SID_V3_SR", "SID: voice 3 sustain/release"),
    (0xd415, "SID_CUTOFF", "SID: filter cutoff"), (0xd417, "SID_RESONANCE", "SID: filter resonance and voices"),
    (0xd418, "SID_VOLUME", "SID: volume and filter mode"),
    (0xd41b, "SID_OSC3", "SID: voice 3's waveform (random numbers from noise)"), (0xd41c, "SID_ENV3", "SID: voice 3's envelope"),
    (0xd800, "COLOR_RAM", "colour RAM (one colour per character)"),
    (0xdc00, "CIA1_PRA", "CIA 1: keyboard rows (write) / joystick 2 (read)"),
    (0xdc01, "CIA1_PRB", "CIA 1: keyboard columns / joystick 1 (read)"),
    (0xdc02, "CIA1_DDRA", "CIA 1: port A direction"), (0xdc03, "CIA1_DDRB", "CIA 1: port B direction"),
    (0xdc04, "CIA1_TA", "CIA 1: timer A"), (0xdc06, "CIA1_TB", "CIA 1: timer B"),
    (0xdc0d, "CIA1_ICR", "CIA 1: interrupt control (IRQ)"), (0xdc0e, "CIA1_CRA", "CIA 1: timer A control"),
    (0xdc0f, "CIA1_CRB", "CIA 1: timer B control"),
    (0xdd00, "CIA2_PRA", "CIA 2: VIC-II bank (bits 0–1) and the serial bus"),
    (0xdd02, "CIA2_DDRA", "CIA 2: port A direction"),
    (0xdd0d, "CIA2_ICR", "CIA 2: interrupt control (NMI)"),
    (0xfffa, "NMI_VECTOR", "the CPU's NMI vector"),
    (0xfffc, "RESET_VECTOR", "the CPU's reset vector"),
    (0xfffe, "IRQ_VECTOR", "the CPU's IRQ/BRK vector"),
];

/// What a chip register does (for the overview and comments).
pub fn reg_note(addr: u16) -> Option<&'static str> {
    REGS.iter().find(|r| r.0 == addr).map(|r| r.2)
}

/// The chip at an I/O address.
pub fn chip_of(addr: u16) -> Option<&'static str> {
    Some(match addr {
        0xd000..=0xd3ff => "VIC-II",
        0xd400..=0xd7ff => "SID",
        0xd800..=0xdbff => "colour RAM",
        0xdc00..=0xdcff => "CIA 1",
        0xdd00..=0xddff => "CIA 2",
        0xde00..=0xdfff => "cartridge I/O",
        _ => return None,
    })
}

/// What a byte of memory turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Data = 0,
    Code = 1,
    CodeTail = 2,
    Reachable = 3,
    ReachableTail = 4,
}

pub struct Analysis {
    /// The game after loading and starting up, as a RAD save state (patches go into this).
    pub loaded: Vec<u8>,
    /// Memory at that moment as the program sees itself (RAM; a cartridge's ROM).
    pub memory: Vec<u8>,
    pub kinds: Vec<Kind>,
    /// Bytes written while it ran (variables, the screen, the stack).
    pub written: Vec<bool>,
    /// Where the program starts (its SYS address) or first ran after the snapshot.
    pub entry: Option<u16>,
    pub calls: BTreeSet<u16>,
    pub jumps: BTreeSet<u16>,
    pub data_refs: BTreeSet<u16>,
    /// Chip registers its own code used: address → (reads, writes).
    pub io: BTreeMap<u16, (u64, u64)>,
    /// ROM routines it called: address → how often.
    pub rom_calls: BTreeMap<u16, u64>,
    pub basic: Option<String>,
    /// The files on the disk (or the tape's name).
    pub files: Vec<String>,
    pub cartridge: bool,
    /// The ROMs were mapped in at the end ($01 bits), so $A000/$E000 code there is ROM.
    pub cpu_port: u8,
    /// What the VIC-II showed at the end: its registers, bank, sprites in use (data address).
    pub vic: Vec<u8>,
    pub vic_bank: u16,
    pub sprites: Vec<(u8, u16)>,
    /// Pictures (392 × 272, colour indices): when the snapshot was taken, and at the end.
    pub pictures: Vec<(String, Vec<u8>)>,
    pub steps: u64,
    pub frames: u32,
}

/// The user's (and the AI's) names and comments for addresses.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Notes {
    #[serde(default)]
    pub labels: BTreeMap<u16, String>,
    #[serde(default)]
    pub comments: BTreeMap<u16, String>,
}

/// Keys and joystick pressed while tracing, to get past title screens: a few frames of each,
/// now and then.
fn autoplay(frame: u32) -> (Vec<u8>, u8) {
    let slot = frame / 25;
    if frame < 100 || frame % 25 > 4 {
        return (Vec::new(), 0);
    }
    let k = |n: &str| keys::named(n).unwrap();
    match slot % 8 {
        0 => (vec![k("RETURN")], 0),
        1 => (vec![k("SPACE")], joy::FIRE),
        2 => (vec![k("F1")], 0),
        3 => (vec![k("1")], joy::FIRE),
        4 => (vec![], joy::LEFT),
        5 => (vec![], joy::RIGHT | joy::FIRE),
        6 => (vec![k("Y")], joy::UP),
        _ => (vec![k("N")], joy::DOWN),
    }
}

fn rom_mapped(addr: u16, port: u8) -> bool {
    match addr {
        0xa000..=0xbfff => port & 3 == 3,
        0xe000..=0xffff => port & 2 != 0,
        _ => false,
    }
}

fn io_mapped(port: u8) -> bool {
    port & 3 != 0 && port & 4 != 0
}

/// The register an I/O address reaches (the chips repeat their registers through their
/// areas; colour RAM counts as one).
fn io_reg(addr: u16) -> u16 {
    match addr {
        0xd000..=0xd3ff => 0xd000 | (addr & 0x3f),
        0xd400..=0xd7ff => 0xd400 | (addr & 0x1f),
        0xd800..=0xdbff => 0xd800,
        0xdc00..=0xdcff => 0xdc00 | (addr & 0x0f),
        0xdd00..=0xddff => 0xdd00 | (addr & 0x0f),
        _ => addr,
    }
}

/// What RAM holds when nothing has written it (64 bytes of 0, 64 of $FF, … as chips sets it).
fn pristine(addr: usize) -> u8 {
    if (addr >> 6) & 1 == 0 { 0 } else { 0xff }
}

/// What tracing records, tick by tick.
struct Recorder {
    kinds: Vec<Kind>,
    written: Vec<bool>,
    calls: BTreeSet<u16>,
    jumps: BTreeSet<u16>,
    data_refs: BTreeSet<u16>,
    io: BTreeMap<u16, (u64, u64)>,
    rom_calls: BTreeMap<u16, u64>,
    entry: Option<u16>,
    steps: u64,
    port: *const u8,
    prev: u64,
    /// The instruction being fetched: its address, bytes so far, length, from ROM.
    cur: Option<(u16, Vec<u8>, u16, bool)>,
    /// Only note chip registers and ROM calls (while the game starts up and maybe unpacks
    /// itself: code seen then may be gone by the snapshot).
    light: bool,
}

impl Recorder {
    fn tick(&mut self, p: u64) {
        let port = unsafe { *self.port };
        let addr = pins::addr(p);
        let read = p & pins::RW != 0;
        let data = pins::data(p);
        let new_op = p & pins::SYNC != 0 && self.prev & pins::SYNC == 0;
        self.prev = p;
        if new_op {
            self.steps += 1;
            let rom = rom_mapped(addr, port);
            let len = OPS[data as usize].1.len();
            if !rom && !self.light {
                self.entry.get_or_insert(addr);
                self.kinds[addr as usize] = Kind::Code;
                for i in 1..len {
                    let a = addr.wrapping_add(i) as usize;
                    if self.kinds[a] != Kind::Code {
                        self.kinds[a] = Kind::CodeTail;
                    }
                }
            }
            self.cur = Some((addr, vec![data], len, rom));
            if len == 1 {
                self.finish();
            }
            return;
        }
        let Some((pc, bytes, len, rom)) = self.cur.as_mut() else { return };
        if read && bytes.len() < *len as usize && addr == pc.wrapping_add(bytes.len() as u16) {
            bytes.push(data);
            if bytes.len() == *len as usize {
                self.finish();
            }
            return;
        }
        let from_rom = *rom;
        if from_rom {
            return; // the ROM's own workings
        }
        if !read && !self.light {
            self.written[addr as usize] = true;
        }
        if (0xd000..0xe000).contains(&addr) && io_mapped(port) {
            let e = self.io.entry(io_reg(addr)).or_default();
            if read {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
    }

    fn finish(&mut self) {
        let Some((pc, bytes, _, rom)) = self.cur.as_ref() else { return };
        if *rom {
            return;
        }
        let port = unsafe { *self.port };
        let i = Instr::from_bytes(*pc, bytes);
        if let Some(t) = i.target() {
            if rom_mapped(t, port) {
                *self.rom_calls.entry(t).or_default() += 1;
            } else if self.light {
            } else if i.name == "JSR" {
                self.calls.insert(t);
            } else {
                self.jumps.insert(t);
            }
        }
        if let Some(a) = i.data_addr().filter(|_| !self.light) {
            self.data_refs.insert(a);
        }
    }
}

/// Loads a game, lets it start, and runs it for `seconds` (pressing keys now and then),
/// tracing what runs.
pub fn analyze(roms: Arc<Roms>, name: &str, data: &[u8], seconds: u32) -> Result<Analysis, C64Error> {
    let mut c = C64::new(roms);
    let loaded_kind = c.load(name, data)?;
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let sys = match ext.as_str() {
        "prg" => formats::Prg::parse(name, data).ok().and_then(|p| p.sys_address()),
        "t64" => formats::t64(data).ok().and_then(|mut v| v.remove(0).sys_address()),
        _ => None,
    };
    let files = match &loaded_kind {
        crate::Loaded::Disk { names } | crate::Loaded::Tape { names } => names.clone(),
        _ => Vec::new(),
    };
    let mut rec = Recorder {
        kinds: vec![Kind::Data; 0x10000],
        written: vec![false; 0x10000],
        calls: BTreeSet::new(),
        jumps: BTreeSet::new(),
        data_refs: BTreeSet::new(),
        io: BTreeMap::new(),
        rom_calls: BTreeMap::new(),
        entry: None,
        steps: 0,
        port: c.cpu_port_ptr(),
        prev: 0,
        cur: None,
        light: true,
    };
    // Load and start, then two seconds more for setting up and unpacking (noting only the
    // chips and ROM routines used).
    for _ in 0..50 * 60 * 20 {
        if !c.is_starting() {
            break;
        }
        c.run_frame_traced(&mut |p| rec.tick(p));
        c.skip_audio();
    }
    for _ in 0..100 {
        c.run_frame_traced(&mut |p| rec.tick(p));
        c.skip_audio();
    }
    rec.light = false;
    rec.cur = None;
    let loaded = c.save_state()?;
    let memory = c.program_view();
    let mut picture = vec![0u8; 392 * 272];
    c.render(&mut picture);
    let mut pictures = vec![("When RAD took the snapshot".to_string(), picture.clone())];
    let basic_text = basic::program_in_memory(&memory).map(|p| basic::detokenize(&p)).filter(|t| !t.trim().is_empty());

    if let Some(s) = sys {
        rec.calls.insert(s);
    }
    let frames = seconds * 50;
    for f in 0..frames {
        let (k, j) = autoplay(f);
        c.set_keys(&k);
        c.set_joystick(j);
        c.run_frame_traced(&mut |p| rec.tick(p));
        c.skip_audio();
    }
    c.set_keys(&[]);
    c.set_joystick(0);
    c.render(&mut picture);
    pictures.push(("After playing for a while".to_string(), picture));
    let cpu_port = c.cpu_port();
    let (vic, vic_bank) = c.vic();
    let screen = vic_bank as usize + (vic[0x18] as usize >> 4) * 0x400;
    let ram = c.ram();
    let sprites: Vec<(u8, u16)> = (0..8u8).filter(|s| vic[0x15] & (1 << s) != 0).map(|s| (s, vic_bank + ram[screen + 0x3f8 + s as usize] as u16 * 64)).collect();
    let Recorder { mut kinds, written, mut calls, mut jumps, mut data_refs, io, rom_calls, entry, steps, .. } = rec;
    let entry = sys.or(entry);

    // Follow branches from code that ran, to find code that hasn't run yet.
    let off_limits = |a: u16| a < 0x0200 || (0xd000..0xe000).contains(&a) || rom_mapped(a, cpu_port);
    let mut todo: Vec<u16> = calls.iter().chain(jumps.iter()).copied().filter(|a| !off_limits(*a)).collect();
    todo.extend(kinds.iter().enumerate().filter(|(_, k)| **k == Kind::Code).map(|(a, _)| a as u16));
    let mut seen = BTreeSet::new();
    while let Some(mut pc) = todo.pop() {
        if !seen.insert(pc) {
            continue;
        }
        for _ in 0..4096 {
            if off_limits(pc) {
                break;
            }
            let i = decode(&memory, pc);
            let len = i.bytes.len() as u16;
            let k = kinds[pc as usize];
            if k == Kind::Data {
                // Undocumented opcodes in code that hasn't run are usually data; so are bytes the
                // game writes, and anything overlapping known code.
                if i.illegal || written[pc as usize] || (1..len).any(|n| matches!(kinds[pc.wrapping_add(n) as usize], Kind::Code | Kind::Reachable)) {
                    break;
                }
                kinds[pc as usize] = Kind::Reachable;
                for n in 1..len {
                    let a = pc.wrapping_add(n) as usize;
                    if kinds[a] == Kind::Data {
                        kinds[a] = Kind::ReachableTail;
                    }
                }
            } else if k != Kind::Code && k != Kind::Reachable {
                break;
            }
            if let Some(t) = i.target() {
                if !off_limits(t) && !seen.contains(&t) {
                    todo.push(t);
                }
                if !rom_mapped(t, cpu_port) {
                    if i.name == "JSR" {
                        calls.insert(t);
                    } else {
                        jumps.insert(t);
                    }
                }
            }
            if let Some(a) = i.data_addr() {
                if k == Kind::Data || k == Kind::Reachable {
                    data_refs.insert(a);
                }
            }
            if i.ends_flow() {
                break;
            }
            pc = pc.wrapping_add(len);
            if k != Kind::Data && seen.contains(&pc) {
                break;
            }
            if k != Kind::Data {
                seen.insert(pc);
            }
        }
    }
    data_refs.retain(|a| *a >= 0x0002 && matches!(kinds[*a as usize], Kind::Data) && !(0xd000..0xe000).contains(a));
    Ok(Analysis {
        loaded,
        memory,
        kinds,
        written,
        entry,
        calls,
        jumps,
        data_refs,
        io,
        rom_calls,
        basic: basic_text,
        files,
        cartridge: c.has_cartridge(),
        cpu_port,
        vic,
        vic_bank,
        sprites,
        pictures,
        steps,
        frames,
    })
}

/// A name for an address: the user's label, a KERNAL routine or hardware register, or an
/// automatic one ("sub_c000" for functions, "l_c000" for jump targets, "d_c000" for data).
pub fn name_of(an: &Analysis, notes: &Notes, addr: u16) -> Option<String> {
    if let Some(l) = notes.labels.get(&addr) {
        return Some(clean_label(l));
    }
    if rom_mapped(addr, an.cpu_port) || an.rom_calls.contains_key(&addr) {
        if let Some(k) = KERNAL.iter().find(|k| k.0 == addr) {
            return Some(k.1.to_string());
        }
    }
    if let Some(r) = REGS.iter().find(|r| r.0 == addr) {
        if addr >= 0xd000 || addr < 0x0400 {
            return Some(r.1.to_string());
        }
    }
    if an.calls.contains(&addr) {
        return Some(format!("sub_{addr:04x}"));
    }
    if an.jumps.contains(&addr) {
        return Some(format!("l_{addr:04x}"));
    }
    if an.data_refs.contains(&addr) && addr >= 0x0200 {
        return Some(format!("d_{addr:04x}"));
    }
    None
}

/// A name 64tass accepts as a label: letters, digits and "_", not starting with a digit or
/// "_" (64tass's local labels), not a register or a mnemonic.
pub fn clean_label(n: &str) -> String {
    let mut s: String = n.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit() || c == '_') {
        s.insert_str(0, "l");
    }
    let lower = s.to_ascii_lowercase();
    if matches!(lower.as_str(), "a" | "x" | "y" | "s" | "z") || OPS.iter().any(|o| o.0.eq_ignore_ascii_case(&lower)) {
        s.push('_');
    }
    s
}

// ------------------------------------------------------------------ listing

#[derive(Debug, Clone, serde::Serialize)]
pub struct Line {
    pub addr: u16,
    pub bytes: Vec<u8>,
    /// "code" (ran), "reachable" (code that hasn't run yet), "data", "text".
    pub kind: &'static str,
    /// The instruction or data in 64tass syntax (with labels).
    pub text: String,
    pub label: Option<String>,
    pub comment: Option<String>,
    pub target: Option<u16>,
    pub variable: bool,
}

/// Formats an instruction in 64tass syntax; `name` gives labels for addresses.
pub fn format_instr(i: &Instr, name: &dyn Fn(u16) -> Option<String>) -> (String, Option<u16>) {
    let m = i.name.to_lowercase();
    let op = i.operand;
    let named = |a: u16, digits: usize| name(a).unwrap_or_else(|| if digits == 2 { format!("${a:02x}") } else { format!("${a:04x}") });
    // An absolute address below $100 must stay absolute (64tass would pick zero page).
    let wide = |a: u16| if a < 0x100 { format!("@w {}", named(a, 4)) } else { named(a, 4) };
    let (args, target) = match i.mode {
        Mode::Imp => (String::new(), None),
        Mode::Acc => ("a".into(), None),
        Mode::Imm => (format!("#${op:02x}"), None),
        Mode::Zp => (named(op, 2), Some(op)),
        Mode::Zpx => (format!("{},x", named(op, 2)), Some(op)),
        Mode::Zpy => (format!("{},y", named(op, 2)), Some(op)),
        Mode::Abs => (wide(op), Some(op)),
        Mode::Abx => (format!("{},x", wide(op)), Some(op)),
        Mode::Aby => (format!("{},y", wide(op)), Some(op)),
        Mode::Ind => (format!("({})", named(op, 4)), Some(op)),
        Mode::Izx => (format!("({},x)", named(op, 2)), Some(op)),
        Mode::Izy => (format!("({}),y", named(op, 2)), Some(op)),
        Mode::Rel => (named(op, 4), Some(op)),
    };
    let text = if args.is_empty() { m } else { format!("{m:<4} {args}") };
    (text, target)
}

fn petscii_text(b: u8) -> bool {
    (0x20..0x5b).contains(&b) && b != b'"'
}

/// Memory from `from` to `to` (exclusive) as listing lines: instructions where code is, data
/// (with text spotted) elsewhere. `force_db`: instructions to show as bytes.
pub fn listing(an: &Analysis, notes: &Notes, from: u16, to: u32, force_db: &BTreeSet<u16>) -> Vec<Line> {
    let mem = &an.memory;
    let name = |a: u16| name_of(an, notes, a);
    let mut out = Vec::new();
    let mut a = from as u32;
    while a < to {
        let pc = a as u16;
        let k = an.kinds[a as usize];
        let label = name(pc);
        let mut comment = notes.comments.get(&pc).cloned();
        if matches!(k, Kind::Code | Kind::Reachable) && !force_db.contains(&pc) {
            let i = decode(mem, pc);
            let len = i.bytes.len() as u32;
            if a + len <= to && (1..len).all(|n| !matches!(an.kinds[(a + n) as usize], Kind::Code | Kind::Reachable)) {
                let (text, target) = format_instr(&i, &name);
                if comment.is_none() {
                    comment = auto_comment(an, &i, target);
                }
                out.push(Line { addr: pc, bytes: i.bytes.clone(), kind: if k == Kind::Code { "code" } else { "reachable" }, text, label, comment, target, variable: false });
                a += len;
                continue;
            }
        }
        // Data: a run until the next code, label or comment; text when it reads as text.
        let mut end = a + 1;
        while end < to && end - a < 64 && !matches!(an.kinds[end as usize], Kind::Code | Kind::Reachable) && name(end as u16).is_none() && !notes.comments.contains_key(&(end as u16)) {
            end += 1;
        }
        let chunk = &mem[a as usize..end as usize];
        let text_len = chunk.iter().take_while(|b| petscii_text(**b)).count();
        let (n, text, kind) = if text_len >= 5 && chunk[..text_len].iter().filter(|b| b.is_ascii_alphabetic()).count() * 2 >= text_len {
            (text_len, format!(".text \"{}\"", String::from_utf8_lossy(&chunk[..text_len])), "text")
        } else {
            let n = chunk.len().min(8);
            (n, format!(".byte {}", chunk[..n].iter().map(|b| format!("${b:02x}")).collect::<Vec<_>>().join(",")), "data")
        };
        let variable = (a..a + n as u32).any(|x| an.written[x as usize]);
        if comment.is_none() && kind == "data" {
            // Text in screen codes (how games often store it) is worth a comment.
            let sc: String = chunk[..n].iter().map(|b| crate::screen_char(*b, false)).collect();
            if n >= 5 && chunk[..n].iter().all(|b| (1..=0x3f).contains(b)) && sc.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 3 {
                comment = Some(format!("screen codes: \"{sc}\""));
            }
        }
        out.push(Line { addr: pc, bytes: chunk[..n].to_vec(), kind, text, label, comment, target: None, variable });
        a += n as u32;
    }
    out
}

/// Notes the disassembler can add by itself: KERNAL calls, chip registers, undocumented opcodes.
fn auto_comment(an: &Analysis, i: &Instr, target: Option<u16>) -> Option<String> {
    if i.illegal {
        return Some(if i.name == "JAM" { "undocumented: stops the CPU".into() } else { "undocumented opcode".into() });
    }
    let t = target?;
    if matches!(i.name, "JSR" | "JMP") {
        if let Some(k) = KERNAL.iter().find(|k| k.0 == t).filter(|_| rom_mapped(t, an.cpu_port) || an.rom_calls.contains_key(&t)) {
            return Some(format!("KERNAL: {}", k.2));
        }
    }
    if i.mode == Mode::Rel {
        return None;
    }
    reg_note(t).filter(|_| t >= 0xd000 || t < 0x0400).map(String::from).or_else(|| chip_of(t).map(|c| c.to_string()))
}

/// The range worth listing: from the program's first code to the end of that block of memory.
pub fn program_range(an: &Analysis) -> (u16, u32) {
    let content = |a: usize| {
        matches!(an.kinds[a], Kind::Code | Kind::CodeTail | Kind::Reachable | Kind::ReachableTail) || (an.memory[a] != pristine(a) && !an.written[a]) || an.data_refs.contains(&(a as u16))
    };
    let usable = |a: usize| a >= 0x0200 && !(0xd000..0xe000).contains(&a) && !rom_mapped(a as u16, an.cpu_port);
    let from = an
        .entry
        .map(|e| e as usize)
        .filter(|e| usable(*e))
        .into_iter()
        .chain((0x0200..0x10000).find(|a| usable(*a) && matches!(an.kinds[*a], Kind::Code | Kind::Reachable)))
        .min()
        .unwrap_or(0x0801);
    // Back to the start of the block it's in (a BASIC SYS line, data and tables often come
    // first), over gaps of up to 16 empty bytes.
    let mut start = from;
    let mut a = from;
    while a > 0x0200 && usable(a - 1) && start - a <= 16 {
        a -= 1;
        if content(a) {
            start = a;
        }
    }
    let mut last = from;
    let mut a = from;
    while a < 0x10000 && a - last <= 256 {
        if usable(a) && content(a) {
            last = a;
        }
        a += 1;
    }
    (start as u16, last as u32 + 1)
}

/// Assembly source (64tass) for `from`–`to` that assembles back to the same bytes: the
/// names it uses, "* = from", the listing. Undocumented opcodes are written as bytes.
pub fn source(an: &Analysis, notes: &Notes, from: u16, to: u32, force_db: &BTreeSet<u16>, title: &str) -> String {
    let lines = listing(an, notes, from, to, force_db);
    let starts: BTreeSet<u16> = lines.iter().map(|l| l.addr).collect();
    let name = |a: u16| name_of(an, notes, a);
    let mut placed: BTreeSet<String> = BTreeSet::new();
    for l in &lines {
        if let Some(lb) = &l.label {
            placed.insert(lb.clone());
        }
    }
    let mut equs: BTreeMap<String, u16> = BTreeMap::new();
    for l in &lines {
        if let Some(t) = l.target {
            if let Some(n) = name(t) {
                let inside = starts.contains(&t) && (t as u32) >= from as u32 && (t as u32) < to;
                if !inside || !placed.contains(&n) {
                    equs.insert(n, t);
                }
            }
        }
    }
    let mut s = String::new();
    let _ = writeln!(s, "; {title}\n; Disassembled by RAD's C64 lab. Assemble with 64tass.\n");
    for (n, a) in &equs {
        if !placed.contains(n) {
            let _ = writeln!(s, "{n:<16} = ${a:04x}");
        }
    }
    let _ = writeln!(s, "\n        * = ${from:04x}\n");
    for l in &lines {
        if let Some(lb) = &l.label {
            let _ = writeln!(s, "{lb}");
        }
        let illegal = matches!(l.kind, "code" | "reachable") && OPS[l.bytes[0] as usize].2;
        let mut line = if illegal {
            format!("        .byte {}", l.bytes.iter().map(|b| format!("${b:02x}")).collect::<Vec<_>>().join(","))
        } else {
            format!("        {}", l.text)
        };
        let comment = if illegal { Some(format!("{}{}", l.text, l.comment.as_ref().map(|c| format!(" — {c}")).unwrap_or_default())) } else { l.comment.clone() };
        if let Some(c) = comment {
            let pad = 40usize.saturating_sub(line.len()).max(1);
            line.push_str(&" ".repeat(pad));
            line.push_str("; ");
            line.push_str(&c.replace('\n', " "));
        }
        let _ = writeln!(s, "{line}");
    }
    s
}

// ------------------------------------------------------------------ patches & extras

/// The loaded game with patches (address, new bytes) written into RAM: a save state to play.
pub fn apply_patches(roms: Arc<Roms>, loaded: &[u8], patches: &[(u16, Vec<u8>)]) -> Result<Vec<u8>, C64Error> {
    let mut c = C64::new(roms);
    c.load("game.c64s", loaded)?;
    for (addr, bytes) in patches {
        for (i, b) in bytes.iter().enumerate() {
            c.poke_ram(addr.wrapping_add(i as u16), *b);
        }
    }
    c.save_state()
}

/// Text in memory: runs of at least `min` characters, in PETSCII or in screen codes.
pub fn strings(mem: &[u8], from: usize, min: usize) -> Vec<(u16, String)> {
    let mut out = Vec::new();
    for screen in [false, true] {
        let ok = |b: u8| if screen { (1..=0x3f).contains(&b) } else { (0x20..0x5b).contains(&b) || (0xc1..=0xda).contains(&b) };
        let ch = |b: u8| if screen { crate::screen_char(b, false) } else { formats::petscii_char(b) };
        let mut i = from;
        while i < mem.len() {
            let start = i;
            while i < mem.len() && ok(mem[i]) {
                i += 1;
            }
            let s: String = mem[start..i].iter().map(|b| ch(*b)).collect();
            let letters = s.chars().filter(|c| c.is_ascii_alphabetic()).count();
            if s.trim().len() >= min && letters * 2 >= s.len() && letters >= 3 && !(screen && s.chars().all(|c| c == ' ' || c == '@')) {
                out.push((start as u16, s));
            }
            i += 1;
        }
    }
    out.sort_by_key(|(a, _)| *a);
    out.dedup_by(|b, a| b.0 < a.0 + a.1.len() as u16);
    out
}

/// One class per 256-byte page, for a memory map: "zeropage", "screen", "basic", "io", "rom",
/// "cartridge", "code", "reachable", "data", "variables", "empty".
pub fn memory_map(an: &Analysis) -> Vec<&'static str> {
    let screen = an.vic_bank as usize + (an.vic.get(0x18).copied().unwrap_or(0x15) as usize >> 4) * 0x400;
    let prog = basic::program_in_memory(&an.memory).map(|p| (0x0801usize, 0x0801 + p.len()));
    (0..256)
        .map(|page| {
            let a = page * 256;
            let r = a..a + 256;
            let count = |f: &dyn Fn(usize) -> bool| r.clone().filter(|x| f(*x)).count();
            let code = count(&|x| matches!(an.kinds[x], Kind::Code | Kind::CodeTail));
            let reach = count(&|x| matches!(an.kinds[x], Kind::Reachable | Kind::ReachableTail));
            if a < 0x0200 {
                return "zeropage";
            }
            if (screen..screen + 0x400).contains(&a) {
                return "screen";
            }
            if (0xd000..0xe000).contains(&a) {
                return "io";
            }
            if an.cartridge && (0x8000..0xc000).contains(&a) {
                return "cartridge";
            }
            if rom_mapped(a as u16, an.cpu_port) && code == 0 {
                return "rom";
            }
            if let Some((p, e)) = prog {
                if a + 128 >= p && a < e && code == 0 {
                    return "basic";
                }
            }
            let var = count(&|x| an.written[x]);
            let nonzero = count(&|x| an.memory[x] != pristine(x));
            if code > 32 {
                "code"
            } else if reach > 32 {
                "reachable"
            } else if var > 64 {
                "variables"
            } else if nonzero > 16 {
                "data"
            } else {
                "empty"
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_6502() {
        let mem: Vec<u8> = {
            let mut m = vec![0u8; 0x10000];
            m[0xc000..0xc00c].copy_from_slice(&[0xa9, 0x05, 0x8d, 0x20, 0xd0, 0xd0, 0xfe, 0xad, 0x12, 0x00, 0x07, 0x10]);
            m
        };
        let i = decode(&mem, 0xc000);
        assert_eq!((i.name, i.mode, i.bytes.len()), ("LDA", Mode::Imm, 2));
        let i = decode(&mem, 0xc002);
        assert_eq!(i.data_addr(), Some(0xd020));
        let i = decode(&mem, 0xc005);
        assert_eq!(i.target(), Some(0xc005), "bne * loops on itself");
        let (t, _) = format_instr(&decode(&mem, 0xc007), &|_| None);
        assert_eq!(t, "lda  @w $0012", "absolute stays absolute");
        let i = decode(&mem, 0xc00a);
        assert!(i.illegal && i.name == "SLO");
        assert_eq!(OPS.iter().filter(|o| !o.2).count(), 151);
    }

    #[test]
    fn finds_text() {
        let mut m = vec![0u8; 64];
        m[4..13].copy_from_slice(b"GAME OVER");
        m[20..26].copy_from_slice(&[19, 3, 15, 18, 5, 32]); // "SCORE " in screen codes
        let s = strings(&m, 0, 5);
        assert_eq!(s.iter().map(|x| x.1.trim()).collect::<Vec<_>>(), vec!["GAME OVER", "SCORE"]);
    }
}

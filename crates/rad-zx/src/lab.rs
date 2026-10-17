//! The Spectrum lab: taking a game apart to see how it works, and changing it.
//!
//! [`analyze`] loads a game (tape or snapshot, BASIC and/or machine code), then runs it for a
//! while instruction by instruction, recording what really runs: which addresses are code,
//! which bytes change (variables, the screen), where jumps and calls go, which ports it uses.
//! The CPU's own path through the program is a far better guide to "what's code, what's
//! graphics" than reading bytes cold. Code reachable from what ran (but not run yet) is found
//! by following branches.
//!
//! [`listing`] turns memory into labelled Z80 assembly (sjasmplus syntax) that [`source`]
//! writes out so it assembles back to the same bytes; [`apply_patches`] puts changes into the
//! loaded game for testing.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use spectrusty::z80emu::{CpuDebug, CpuDebugAddr, CpuDebugArg, CpuDebugArgs, CpuDebugPort, Z80NMOS};

use crate::{basic, tzx, Keys, Loaded, Model, Spectrum, ZxError};

/// What a byte of memory turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Nothing ran here and nothing points here (data, or unknown).
    Data = 0,
    /// An instruction that ran (its first byte).
    Code = 1,
    /// The other bytes of an instruction that ran.
    CodeTail = 2,
    /// Code found by following branches from code that ran, but not seen running.
    Reachable = 3,
    ReachableTail = 4,
}

pub struct Analysis {
    /// The game right after loading, as a Z80 snapshot (patches are applied to this).
    pub loaded: Vec<u8>,
    /// Memory at that moment (64K: ROM, screen, RAM).
    pub memory: Vec<u8>,
    /// What each address is.
    pub kinds: Vec<Kind>,
    /// Bytes that changed while it ran (variables, the screen, the stack).
    pub written: Vec<bool>,
    /// Where the program's own code was first entered after loading.
    pub entry: Option<u16>,
    /// Targets of CALLs (functions) and of jumps.
    pub calls: BTreeSet<u16>,
    pub jumps: BTreeSet<u16>,
    /// Addresses used as data by instructions like LD A,(nn).
    pub data_refs: BTreeSet<u16>,
    /// Port use: "out ($fe)" → how often.
    pub ports: BTreeMap<String, u64>,
    /// ROM routines the game called: address → how often.
    pub rom_calls: BTreeMap<u16, u64>,
    pub model: Model,
    /// The BASIC program in memory after loading, as a listing.
    pub basic: Option<String>,
    /// The tape's blocks (for tapes).
    pub tape: Vec<tzx::DataBlock>,
    /// Instructions run while tracing (all of them, ROM included).
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

/// Keys pressed while tracing, to get past title screens and "press any key": a few frames of
/// each, now and then (and the joystick), so more of the game runs.
fn autoplay(frame: u32) -> (Keys, u8) {
    let slot = frame / 25;
    if frame < 100 || frame % 25 > 4 {
        return (Keys::empty(), 0);
    }
    match slot % 8 {
        0 => (Keys::EN, 0),
        1 => (Keys::BR, 0),
        2 => (Keys::N0, 16),
        3 => (Keys::N1, 0),
        4 => (Keys::O, 4),
        5 => (Keys::P, 8),
        6 => (Keys::Q | Keys::N7, 1),
        _ => (Keys::A | Keys::M, 2),
    }
}

fn is_branch(m: &str) -> bool {
    matches!(m, "JP" | "JR" | "CALL" | "DJNZ" | "RST")
}

/// The branch target of an instruction (JP nn, JR e, CALL nn, DJNZ e, RST n).
fn target(d: &CpuDebug) -> Option<u16> {
    if !is_branch(d.mnemonic) {
        return None;
    }
    let arg = match d.args {
        CpuDebugArgs::Single(a) => a,
        CpuDebugArgs::Double(_, a) => a,
        _ => return None,
    };
    match arg {
        CpuDebugArg::Imm16(nn) => Some(nn),
        CpuDebugArg::Imm8(n) if d.mnemonic == "RST" => Some(n as u16),
        _ => None,
    }
}

/// Instructions after which the next bytes aren't reached by falling through.
fn ends_flow(d: &CpuDebug) -> bool {
    let conditional = matches!(d.args, CpuDebugArgs::Double(CpuDebugArg::Cond(_), _) | CpuDebugArgs::Single(CpuDebugArg::Cond(_)));
    match d.mnemonic {
        "JP" | "JR" | "RET" => !conditional,
        "RETI" | "RETN" => true,
        // HALT carries on after the next interrupt.
        _ => false,
    }
}

/// Decodes the instruction at `pc`.
pub fn decode(mem: &[u8], pc: u16) -> CpuDebug {
    let mut bytes = [0u8; 4];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = mem[pc.wrapping_add(i as u16) as usize];
    }
    let mut out = None;
    let _ = spectrusty::z80emu::disasm::disasm_memory::<Z80NMOS, _, ()>(pc, &bytes, |d| {
        out = Some(d);
        Err(())
    });
    // A lone prefix (DD/FD before an instruction that doesn't use IX/IY) acts as its own one-byte
    // instruction: the disassembler reports what follows from the next address.
    let out = out.filter(|d| d.pc == pc);
    out.unwrap_or_else(|| {
        let mut code = spectrusty::z80emu::CpuDebugCode::new();
        code.push(bytes[0]);
        CpuDebug { code, mnemonic: "DB", pc, prefix: None, args: CpuDebugArgs::None }
    })
}

/// What tracing records, instruction by instruction.
struct Recorder {
    kinds: Vec<Kind>,
    calls: BTreeSet<u16>,
    jumps: BTreeSet<u16>,
    data_refs: BTreeSet<u16>,
    ports: BTreeMap<String, u64>,
    rom_calls: BTreeMap<u16, u64>,
    entry: Option<u16>,
    steps: u64,
}

impl Recorder {
    fn step(&mut self, d: &CpuDebug) {
        self.steps += 1;
        let pc = d.pc;
        if pc >= 0x4000 && self.entry.is_none() {
            self.entry = Some(pc);
        }
        self.kinds[pc as usize] = Kind::Code;
        for i in 1..d.code.len() as u16 {
            let a = pc.wrapping_add(i) as usize;
            if self.kinds[a] != Kind::Code {
                self.kinds[a] = Kind::CodeTail;
            }
        }
        if pc < 0x4000 {
            return; // the ROM's own workings (BASIC, the tape loader)
        }
        if let Some(t) = target(d) {
            if t < 0x4000 {
                *self.rom_calls.entry(t).or_default() += 1;
            }
            if d.mnemonic == "CALL" || d.mnemonic == "RST" {
                self.calls.insert(t);
            } else if t >= 0x4000 {
                self.jumps.insert(t);
            }
        }
        let args = match d.args {
            CpuDebugArgs::Single(a) => [Some(a), None],
            CpuDebugArgs::Double(a, b) => [Some(a), Some(b)],
            _ => [None, None],
        };
        for a in args.into_iter().flatten() {
            match a {
                CpuDebugArg::Addr(CpuDebugAddr::ImmAddr(nn)) => {
                    self.data_refs.insert(nn);
                }
                CpuDebugArg::Port(p) => {
                    let port = match p {
                        CpuDebugPort::ImmPort(n) => format!("${n:02x}"),
                        CpuDebugPort::RegPort => "(c)".to_string(),
                    };
                    *self.ports.entry(format!("{} {port}", d.mnemonic.to_lowercase())).or_default() += 1;
                }
                _ => {}
            }
        }
    }
}

/// Loads a game and runs it for `seconds` (pressing keys now and then), tracing what runs.
pub fn analyze(name: &str, data: &[u8], seconds: u32) -> Result<Analysis, ZxError> {
    let mut zx = Spectrum::new(Model::Spectrum48);
    let loaded_kind = zx.load(name, data)?;
    let mut rec = Recorder {
        kinds: vec![Kind::Data; 0x10000],
        calls: BTreeSet::new(),
        jumps: BTreeSet::new(),
        data_refs: BTreeSet::new(),
        ports: BTreeMap::new(),
        rom_calls: BTreeMap::new(),
        entry: None,
        steps: 0,
    };
    let mut tape = Vec::new();
    // Tapes: trace while loading, until the game's machine code takes over (or, for BASIC-only
    // games, the tape has played out). "Loaded" is the snapshot at that moment.
    if let Loaded::Tape { .. } = loaded_kind {
        tape = tzx::data_blocks(&tzx::parse(data)?);
        for _ in 0..30_000 {
            zx.run_frame_traced(&mut |d| rec.step(d));
            zx.skip_audio();
            if rec.entry.is_some() {
                break;
            }
            if zx.tape_state().is_some_and(|(playing, progress, _)| !playing && progress > 0.95) && zx.typing.is_empty() {
                break;
            }
        }
    }
    let loaded = zx.save_state()?;
    let memory = zx.memory();
    let basic = basic::program_in_memory(&memory).map(|p| basic::detokenize(&p)).filter(|t| !t.trim().is_empty());
    // A BASIC loader says where the code starts: RANDOMIZE USR 32768.
    for usr in basic.iter().flat_map(|b| b.split("USR ").skip(1)).filter_map(|t| t.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok()) {
        if (0x4000..0x10000).contains(&usr) {
            rec.calls.insert(usr as u16);
        }
    }

    let mut written = vec![false; 0x10000];
    let mut before = memory.clone();
    let frames = seconds * 50;
    for f in 0..frames {
        let (keys, joy) = autoplay(f);
        zx.set_keys(keys);
        zx.set_joystick(joy);
        zx.run_frame_traced(&mut |d| rec.step(d));
        zx.skip_audio();
        let now = zx.memory();
        for (a, (x, y)) in before.iter().zip(&now).enumerate() {
            if x != y {
                written[a] = true;
            }
        }
        before = now;
    }
    let Recorder { mut kinds, mut calls, mut jumps, mut data_refs, ports, rom_calls, entry, steps } = rec;
    zx.set_keys(Keys::empty());
    zx.set_joystick(0);

    // Follow branches from code that ran, to find code that hasn't run yet.
    let mut todo: Vec<u16> = calls.iter().chain(jumps.iter()).copied().filter(|a| *a >= 0x4000).collect();
    for (a, k) in kinds.iter().enumerate() {
        if *k == Kind::Code && a >= 0x4000 {
            todo.push(a as u16);
        }
    }
    let mut seen = BTreeSet::new();
    while let Some(mut pc) = todo.pop() {
        if !seen.insert(pc) {
            continue;
        }
        for _ in 0..4096 {
            if pc < 0x5b00 {
                break; // the ROM and the screen
            }
            let d = decode(&memory, pc);
            let len = d.code.len().max(1) as u16;
            let k = kinds[pc as usize];
            if k == Kind::Data {
                // Don't run over bytes that the game itself writes (variables) or a clash.
                if written[pc as usize] || (1..len).any(|i| matches!(kinds[pc.wrapping_add(i) as usize], Kind::Code | Kind::Reachable)) {
                    break;
                }
                kinds[pc as usize] = Kind::Reachable;
                for i in 1..len {
                    let a = pc.wrapping_add(i) as usize;
                    if kinds[a] == Kind::Data {
                        kinds[a] = Kind::ReachableTail;
                    }
                }
            } else if k != Kind::Code && k != Kind::Reachable {
                break;
            }
            if let Some(t) = target(&d) {
                if t >= 0x5b00 && !seen.contains(&t) {
                    todo.push(t);
                }
                if d.mnemonic == "CALL" {
                    calls.insert(t);
                } else if d.mnemonic != "RST" {
                    jumps.insert(t);
                }
            }
            if d.mnemonic == "DB" || ends_flow(&d) {
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
    data_refs.retain(|a| matches!(kinds[*a as usize], Kind::Data));
    Ok(Analysis { loaded, memory, kinds, written, entry, calls, jumps, data_refs, ports, rom_calls, model: zx.model(), basic, tape, steps, frames })
}

// ------------------------------------------------------------------ names

/// Well-known ROM routines (48K).
pub const ROM: &[(u16, &str, &str)] = &[
    (0x0000, "START", "reset"),
    (0x0008, "ERROR_1", "report an error"),
    (0x0010, "PRINT_A", "print the character in A"),
    (0x0018, "GET_CHAR", "BASIC: the current character"),
    (0x0020, "NEXT_CHAR", "BASIC: the next character"),
    (0x0028, "FP_CALC", "the floating-point calculator"),
    (0x0030, "BC_SPACES", "make room in the workspace"),
    (0x0038, "MASK_INT", "the 50 Hz interrupt: keyboard scan and FRAMES"),
    (0x0066, "NMI", "non-maskable interrupt"),
    (0x028e, "KEY_SCAN", "scan the keyboard"),
    (0x02bf, "KEYBOARD", "the keyboard routine (from the interrupt)"),
    (0x031e, "K_TEST", "decode a key"),
    (0x03b5, "BEEPER", "make a sound: HL = pitch, DE = length"),
    (0x03f8, "BEEP", "BASIC BEEP"),
    (0x04c2, "SA_BYTES", "save bytes to tape"),
    (0x0556, "LD_BYTES", "load bytes from tape: IX = address, DE = length"),
    (0x05e3, "LD_EDGE_2", "tape: wait for two edges"),
    (0x05e7, "LD_EDGE_1", "tape: wait for an edge"),
    (0x0605, "SAVE_ETC", "SAVE / LOAD / VERIFY / MERGE"),
    (0x0802, "LD_BLOCK", "load a block"),
    (0x0d6b, "CLS", "clear the screen"),
    (0x0daf, "CL_ALL", "clear the whole screen"),
    (0x0dfe, "CL_SCROLL", "scroll the screen"),
    (0x0e44, "CL_LINE", "clear lines"),
    (0x0e9b, "CL_ADDR", "screen address of a line"),
    (0x0eac, "COPY", "COPY (printer)"),
    (0x15d4, "WAIT_KEY", "wait for a key"),
    (0x1601, "CHAN_OPEN", "open a channel (A = stream: 2 = the screen)"),
    (0x1a1b, "OUT_NUM_1", "print a line number"),
    (0x1e94, "FIND_INT1", "BASIC: an 8-bit argument"),
    (0x1e99, "FIND_INT2", "BASIC: a 16-bit argument"),
    (0x203c, "PR_STRING", "print a string: DE = text, BC = length"),
    (0x22aa, "PIXEL_ADD", "screen address of a pixel"),
    (0x22e5, "PLOT_SUB", "plot a pixel"),
    (0x2d28, "STACK_A", "put A on the calculator stack"),
    (0x2d2b, "STACK_BC", "put BC on the calculator stack"),
    (0x2da2, "FP_TO_BC", "calculator stack to BC"),
    (0x2de3, "PRINT_FP", "print a number"),
    (0x3d00, "CHARSET", "the character set (font)"),
];

/// System variables (48K): address, name, size.
pub const SYSVARS: &[(u16, &str, u8)] = &[
    (23552, "KSTATE", 8), (23560, "LAST_K", 1), (23561, "REPDEL", 1), (23562, "REPPER", 1), (23563, "DEFADD", 2), (23565, "K_DATA", 1),
    (23566, "TVDATA", 2), (23568, "STRMS", 38), (23606, "CHARS", 2), (23608, "RASP", 1), (23609, "PIP", 1), (23610, "ERR_NR", 1),
    (23611, "FLAGS", 1), (23612, "TV_FLAG", 1), (23613, "ERR_SP", 2), (23615, "LIST_SP", 2), (23617, "MODE", 1), (23618, "NEWPPC", 2),
    (23620, "NSPPC", 1), (23621, "PPC", 2), (23623, "SUBPPC", 1), (23624, "BORDCR", 1), (23625, "E_PPC", 2), (23627, "VARS", 2),
    (23629, "DEST", 2), (23631, "CHANS", 2), (23633, "CURCHL", 2), (23635, "PROG", 2), (23637, "NXTLIN", 2), (23639, "DATADD", 2),
    (23641, "E_LINE", 2), (23643, "K_CUR", 2), (23645, "CH_ADD", 2), (23647, "X_PTR", 2), (23649, "WORKSP", 2), (23651, "STKBOT", 2),
    (23653, "STKEND", 2), (23655, "BREG", 1), (23656, "MEM", 2), (23658, "FLAGS2", 1), (23659, "DF_SZ", 1), (23660, "S_TOP", 2),
    (23662, "OLDPPC", 2), (23664, "OSPPC", 1), (23665, "FLAGX", 1), (23666, "STRLEN", 2), (23668, "T_ADDR", 2), (23670, "SEED", 2),
    (23672, "FRAMES", 3), (23675, "UDG", 2), (23677, "COORDS", 2), (23679, "P_POSN", 1), (23680, "PR_CC", 2), (23682, "ECHO_E", 2),
    (23684, "DF_CC", 2), (23686, "DF_CCL", 2), (23688, "S_POSN", 2), (23690, "SPOSNL", 2), (23692, "SCR_CT", 1), (23693, "ATTR_P", 1),
    (23694, "MASK_P", 1), (23695, "ATTR_T", 1), (23696, "MASK_T", 1), (23697, "P_FLAG", 1), (23698, "MEMBOT", 30), (23730, "RAMTOP", 2),
    (23732, "P_RAMT", 2),
];

/// What a port does, for comments.
pub fn port_note(port: &str) -> Option<&'static str> {
    Some(match port {
        "$fe" => "ULA: border (bits 0-2), speaker (bit 4) / keyboard and tape (IN)",
        "$1f" => "Kempston joystick",
        "$fd" => "AY sound chip / 128K memory paging (with B)",
        "(c)" => "port in BC",
        _ => return None,
    })
}

/// A name for an address: the user's label, a ROM routine, a system variable, or an automatic
/// one ("sub_8000" for functions, "l_8000" for jump targets, "d_8000" for data).
pub fn name_of(an: &Analysis, notes: &Notes, addr: u16) -> Option<String> {
    if let Some(l) = notes.labels.get(&addr) {
        return Some(l.clone());
    }
    if addr < 0x4000 {
        return ROM.iter().find(|r| r.0 == addr).map(|r| r.1.to_string());
    }
    if let Some(v) = SYSVARS.iter().find(|v| v.0 == addr) {
        return Some(v.1.to_string());
    }
    if an.calls.contains(&addr) {
        return Some(format!("sub_{addr:04x}"));
    }
    if an.jumps.contains(&addr) {
        return Some(format!("l_{addr:04x}"));
    }
    if an.data_refs.contains(&addr) {
        return Some(format!("d_{addr:04x}"));
    }
    None
}

// ------------------------------------------------------------------ listing

#[derive(Debug, Clone, serde::Serialize)]
pub struct Line {
    pub addr: u16,
    pub bytes: Vec<u8>,
    /// "code" (ran), "reachable" (code that hasn't run yet), "data", "text".
    pub kind: &'static str,
    /// The instruction or data in sjasmplus syntax (with labels).
    pub text: String,
    pub label: Option<String>,
    pub comment: Option<String>,
    /// Where it jumps, calls or points (to follow in the viewer).
    pub target: Option<u16>,
    /// Bytes here change while the game runs (a variable).
    pub variable: bool,
}

fn hex8(n: u8) -> String {
    format!("${n:02x}")
}

/// Formats an instruction in sjasmplus syntax; `name` gives labels for addresses.
pub fn format_instr(d: &CpuDebug, name: &dyn Fn(u16) -> Option<String>) -> (String, Option<u16>) {
    let m = d.mnemonic.to_lowercase();
    let mut target = None;
    let branch = is_branch(d.mnemonic);
    let mut arg = |a: &CpuDebugArg| -> String {
        match *a {
            CpuDebugArg::Imm8(n) => {
                if d.mnemonic == "RST" {
                    target = Some(n as u16);
                }
                hex8(n)
            }
            CpuDebugArg::Bit(b) => b.to_string(),
            CpuDebugArg::IntMode(im) => (im as u8).to_string(),
            CpuDebugArg::Imm16(nn) => {
                if branch || nn >= 0x4000 {
                    target = Some(nn);
                }
                match name(nn).filter(|_| branch || nn >= 0x4000) {
                    Some(l) => l,
                    None => format!("${nn:04x}"),
                }
            }
            CpuDebugArg::Addr(CpuDebugAddr::ImmAddr(nn)) => {
                target = Some(nn);
                format!("({})", name(nn).unwrap_or_else(|| format!("${nn:04x}")))
            }
            CpuDebugArg::Addr(CpuDebugAddr::IndexAddr(px, Some(o))) => {
                let r = format!("{px}").to_lowercase();
                if o < 0 { format!("({r}-{})", -(o as i16)) } else { format!("({r}+{o})") }
            }
            CpuDebugArg::Port(CpuDebugPort::ImmPort(n)) => format!("({})", hex8(n)),
            CpuDebugArg::Port(CpuDebugPort::RegPort) => "(c)".into(),
            ref other => format!("{other}").to_lowercase(),
        }
    };
    let args = match &d.args {
        CpuDebugArgs::None => String::new(),
        CpuDebugArgs::Single(a) => arg(a),
        CpuDebugArgs::Double(CpuDebugArg::Stk16(spectrusty::z80emu::StkReg16::AF), CpuDebugArg::Stk16(spectrusty::z80emu::StkReg16::AF)) => "af,af'".into(),
        CpuDebugArgs::Double(a, b) => {
            let x = arg(a);
            format!("{x},{}", arg(b))
        }
        CpuDebugArgs::BitOpExt(bit, a, r) => format!("{bit},{},{}", arg(a), format!("{r}").to_lowercase()),
    };
    let text = if args.is_empty() { m } else { format!("{m:<5}{args}") };
    (text, target)
}

fn printable(b: u8) -> bool {
    (0x20..0x7f).contains(&b) && b != b'"'
}

/// Memory from `from` to `to` (exclusive) as listing lines: instructions where code is, data
/// (with text spotted) elsewhere. `force_db`: instructions to show as bytes (those that don't
/// assemble back to the same bytes).
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
            let d = decode(mem, pc);
            let len = (d.code.len() as u32).max(1);
            // An instruction must fit and not overlap other code we know starts inside it.
            // Undocumented forms z80emu marks with "*" have no assembler syntax: bytes instead.
            let plain = d.mnemonic.chars().all(|c| c.is_ascii_alphanumeric());
            // A redundant IX/IY prefix (one that doesn't make the instruction use IX/IY) can't be
            // written either: the assembler wouldn't put it back.
            let (text_try, _) = format_instr(&d, &name);
            let prefix_ok = d.prefix.is_none() || text_try.contains("ix") || text_try.contains("iy");
            let fits = plain && prefix_ok && a + len <= to && (1..len).all(|i| !matches!(an.kinds[(a + i) as usize], Kind::Code | Kind::Reachable)) && d.mnemonic != "DB";
            if fits {
                let (text, target) = format_instr(&d, &name);
                if comment.is_none() {
                    comment = auto_comment(&d, target);
                }
                out.push(Line { addr: pc, bytes: d.code.as_slice().to_vec(), kind: if k == Kind::Code { "code" } else { "reachable" }, text, label, comment, target, variable: false });
                a += len;
                continue;
            }
        }
        // Data: a run until the next code, label or 8 bytes; text when it reads as text.
        let mut end = a + 1;
        while end < to && end - a < 64 && !matches!(an.kinds[end as usize], Kind::Code | Kind::Reachable) && name(end as u16).is_none() && !notes.comments.contains_key(&(end as u16)) {
            end += 1;
        }
        let chunk = &mem[a as usize..end as usize];
        let text_len = chunk.iter().take_while(|b| printable(**b)).count();
        let (n, text, kind) = if text_len >= 5 {
            // Text, maybe with the last letter's bit 7 set (a common end marker).
            let mut n = text_len;
            let mut t = format!("db   \"{}\"", String::from_utf8_lossy(&chunk[..n]));
            if let Some(&last) = chunk.get(n) {
                if last >= 0x80 && printable(last & 0x7f) {
                    t.push_str(&format!(",{}", hex8(last)));
                    n += 1;
                }
            }
            (n, t, "text")
        } else {
            // Up to 8 bytes, stopping where text (5+ printable characters) starts.
            let mut n = 0;
            while n < chunk.len().min(8) {
                if n > 0 && chunk[n..].iter().take_while(|b| printable(**b)).count() >= 5 {
                    break;
                }
                n += 1;
            }
            let bytes = &chunk[..n];
            (n, format!("db   {}", bytes.iter().map(|b| hex8(*b)).collect::<Vec<_>>().join(",")), "data")
        };
        let variable = (a..a + n as u32).any(|x| an.written[x as usize]);
        out.push(Line { addr: pc, bytes: chunk[..n].to_vec(), kind, text, label, comment, target: None, variable });
        a += n as u32;
    }
    out
}

/// Notes the disassembler can add by itself: ROM calls, ports.
fn auto_comment(d: &CpuDebug, target: Option<u16>) -> Option<String> {
    if let (Some(t), true) = (target, matches!(d.mnemonic, "CALL" | "JP" | "RST")) {
        if let Some(r) = ROM.iter().find(|r| r.0 == t) {
            return Some(format!("ROM: {}", r.2));
        }
    }
    if matches!(d.mnemonic, "IN" | "OUT") {
        let port = match d.args {
            CpuDebugArgs::Double(CpuDebugArg::Port(p), _) | CpuDebugArgs::Double(_, CpuDebugArg::Port(p)) => p,
            _ => return None,
        };
        let key = match port {
            CpuDebugPort::ImmPort(n) => format!("${n:02x}"),
            CpuDebugPort::RegPort => "(c)".into(),
        };
        return port_note(&key).map(String::from);
    }
    if let Some(t) = target {
        if let Some(v) = SYSVARS.iter().find(|v| v.0 == t) {
            return Some(format!("system variable {}", v.1));
        }
    }
    None
}

/// The range worth listing: from the program's first code (or its first loaded bytes) to the
/// end of that block of memory (where 256 bytes in a row are empty or only variables).
pub fn program_range(an: &Analysis) -> (u16, u32) {
    let floor = basic::program_in_memory(&an.memory).map(|p| 23755 + p.len()).unwrap_or(0x5ccb).max(0x5b00);
    // Code, loaded bytes, and variables the code uses (even if they start as 0).
    let content = |a: usize| {
        matches!(an.kinds[a], Kind::Code | Kind::CodeTail | Kind::Reachable | Kind::ReachableTail) || (an.memory[a] != 0 && !an.written[a]) || an.data_refs.contains(&(a as u16))
    };
    let from = (0x5b00..0x10000)
        .find(|a| matches!(an.kinds[*a], Kind::Code | Kind::Reachable))
        .or_else(|| (floor..0x10000).find(|a| content(*a)))
        .unwrap_or(0x8000);
    let mut last = from;
    let mut a = from;
    while a < 0x10000 && a - last <= 256 {
        if content(a) {
            last = a;
        }
        a += 1;
    }
    (from as u16, last as u32 + 1)
}

/// Assembly source (sjasmplus) for `from`–`to` that assembles back to the same bytes: an ORG,
/// the listing, and EQUs for every name used that isn't a line of it.
pub fn source(an: &Analysis, notes: &Notes, from: u16, to: u32, force_db: &BTreeSet<u16>, title: &str) -> String {
    let lines = listing(an, notes, from, to, force_db);
    let starts: BTreeSet<u16> = lines.iter().map(|l| l.addr).collect();
    // Names used in operands, and names of addresses that aren't line starts.
    let mut equs: BTreeMap<String, u16> = BTreeMap::new();
    let name = |a: u16| name_of(an, notes, a);
    let mut placed: BTreeSet<String> = BTreeSet::new();
    for l in &lines {
        if let Some(lb) = &l.label {
            placed.insert(lb.clone());
        }
    }
    for l in &lines {
        if let Some(t) = l.target {
            if let Some(n) = name(t) {
                if !(starts.contains(&t) && (t as u32) >= from as u32 && (t as u32) < to) || !placed.contains(&n) {
                    equs.insert(n, t);
                }
            }
        }
    }
    let mut s = String::new();
    let _ = writeln!(s, "; {title}\n; Disassembled by RAD's Spectrum lab. Assemble with sjasmplus.\n");
    for (n, a) in &equs {
        if !placed.contains(n) {
            let _ = writeln!(s, "{n:<16} EQU ${a:04x}");
        }
    }
    let _ = writeln!(s, "\n        ORG ${from:04x}\n");
    for l in &lines {
        if let Some(lb) = &l.label {
            let _ = writeln!(s, "{lb}:");
        }
        let mut line = format!("        {}", l.text);
        if let Some(c) = &l.comment {
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

/// The loaded game with patches (address, new bytes) written in: a Z80 snapshot to play.
pub fn apply_patches(loaded: &[u8], patches: &[(u16, Vec<u8>)]) -> Result<Vec<u8>, ZxError> {
    let mut zx = Spectrum::new(Model::Spectrum48);
    zx.load("game.z80", loaded)?;
    for (addr, bytes) in patches {
        for (i, b) in bytes.iter().enumerate() {
            zx.poke(addr.wrapping_add(i as u16), *b);
        }
    }
    zx.save_state()
}

/// Text in memory: runs of at least `min` printable characters (the last may have bit 7 set).
pub fn strings(mem: &[u8], from: usize, min: usize) -> Vec<(u16, String)> {
    let mut out = Vec::new();
    let mut i = from;
    while i < mem.len() {
        let start = i;
        while i < mem.len() && (0x20..0x7f).contains(&mem[i]) {
            i += 1;
        }
        let mut s: String = mem[start..i].iter().map(|b| *b as char).collect();
        if i < mem.len() && mem[i] >= 0xa0 && mem[i] < 0xff && (i - start) >= min.saturating_sub(1) {
            s.push((mem[i] & 0x7f) as char);
            i += 1;
        }
        if s.trim().len() >= min && s.chars().filter(|c| c.is_ascii_alphabetic()).count() * 2 >= s.len() {
            out.push((start as u16, s));
        }
        i += 1;
    }
    out
}

/// One class per 256-byte page, for a memory map: "rom", "screen", "sysvars", "basic",
/// "code", "reachable", "data", "variables", "empty".
pub fn memory_map(an: &Analysis) -> Vec<&'static str> {
    let prog = basic::program_in_memory(&an.memory).map(|p| (23755usize, 23755 + p.len()));
    (0..256)
        .map(|page| {
            let a = page * 256;
            if a < 0x4000 {
                return "rom";
            }
            if a < 0x5b00 {
                return "screen";
            }
            if a < 0x5c00 + 0xcb && a < 23755 {
                return "sysvars";
            }
            let r = a..a + 256;
            let count = |f: &dyn Fn(usize) -> bool| r.clone().filter(|x| f(*x)).count();
            if let Some((p, e)) = prog {
                if a + 128 >= p && a < e {
                    return "basic";
                }
            }
            let code = count(&|x| matches!(an.kinds[x], Kind::Code | Kind::CodeTail));
            let reach = count(&|x| matches!(an.kinds[x], Kind::Reachable | Kind::ReachableTail));
            let var = count(&|x| an.written[x]);
            let nonzero = count(&|x| an.memory[x] != 0);
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

// ------------------------------------------------------------------ live: what the hardware does each frame

/// One frame of activity, for the board diagram: who talked to whom, how much.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct FrameStats {
    /// Instructions the CPU fetched from each part of memory.
    pub fetch_rom: u32,
    /// The lower 16K of RAM ($4000–$7FFF): shared with the ULA, which reads the screen from it.
    pub fetch_ram_low: u32,
    /// The upper 32K ($8000–$FFFF): the CPU's alone.
    pub fetch_ram_high: u32,
    /// Bytes that changed this frame in the screen, the rest of the lower RAM, and upper RAM.
    pub write_screen: u32,
    pub write_ram_low: u32,
    pub write_ram_high: u32,
    /// Port traffic: reads of port $FE (keyboard / tape), writes to $FE (border / speaker),
    /// the Kempston joystick ($1F), and other ports (sound chip, …).
    pub in_fe: u32,
    pub out_fe: u32,
    pub in_joy: u32,
    pub other_io: u32,
    /// HALTs (the game waiting for the next frame) and instructions in all.
    pub halts: u32,
    pub instructions: u32,
    /// Where the CPU was at the end of the frame, and the border colour.
    pub pc: u16,
    pub border: u8,
}

/// Runs one frame while counting what the CPU did; `prev` holds last frame's memory.
pub fn frame_with_stats(zx: &mut Spectrum, prev: &mut Vec<u8>) -> FrameStats {
    let mut st = FrameStats::default();
    zx.run_frame_traced(&mut |d| {
        st.instructions += 1;
        match d.pc {
            0..=0x3fff => st.fetch_rom += 1,
            0x4000..=0x7fff => st.fetch_ram_low += 1,
            _ => st.fetch_ram_high += 1,
        }
        match d.mnemonic {
            "HALT" => st.halts += 1,
            "IN" | "OUT" | "INI" | "INIR" | "IND" | "INDR" | "OUTI" | "OTIR" | "OUTD" | "OTDR" => {
                let port = match d.args {
                    CpuDebugArgs::Double(CpuDebugArg::Port(p), _) | CpuDebugArgs::Double(_, CpuDebugArg::Port(p)) => Some(p),
                    _ => None,
                };
                let input = d.mnemonic.starts_with("IN");
                match (port, input) {
                    (Some(CpuDebugPort::ImmPort(0xfe)), true) => st.in_fe += 1,
                    (Some(CpuDebugPort::ImmPort(0xfe)), false) => st.out_fe += 1,
                    (Some(CpuDebugPort::ImmPort(0x1f)), true) => st.in_joy += 1,
                    _ => st.other_io += 1,
                }
            }
            _ => {}
        }
    });
    let now = zx.memory();
    if prev.len() == now.len() {
        for a in 0x4000..0x10000 {
            if prev[a] != now[a] {
                match a {
                    0x4000..=0x5aff => st.write_screen += 1,
                    0x5b00..=0x7fff => st.write_ram_low += 1,
                    _ => st.write_ram_high += 1,
                }
            }
        }
    }
    *prev = now;
    st.pc = zx.pc();
    st.border = zx.border_color();
    st
}

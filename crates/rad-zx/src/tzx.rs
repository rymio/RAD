//! Tapes: TZX (v1.20, every block type) and TAP files, played as the pulses a Spectrum's EAR
//! input would hear.
//!
//! A tape is parsed into blocks, then played block by block (following TZX jumps, loops and
//! calls) as a stream of signal levels and durations in 3.5 MHz T-states. [`Pulses`] turns that
//! into what the emulator wants: the time from one edge (level change) to the next.
//!
//! Spec: https://worldofspectrum.net/TZXformat.html

use std::num::NonZeroU32;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TapeError {
    #[error("not a TZX or TAP file")]
    NotATape,
    #[error("the tape is damaged: {0}")]
    Damaged(&'static str),
}

/// T-states (at 3.5 MHz) in a millisecond.
const MS: u32 = 3500;

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// Pulses with given lengths, each toggling the level (pilot tones, sync pulses).
    Pulses(Vec<(u32, u32)>),
    /// Data bits: each bit is two pulses of `zero` or `one` length.
    Data { zero: u32, one: u32, used_bits: u8, data: Vec<u8> },
    /// Levels sampled every `tstates_per_sample` T-states (direct recording).
    Direct { tstates_per_sample: u32, used_bits: u8, data: Vec<u8> },
    /// Pulse lengths in T-states (CSW recordings, generalized data), each toggling the level
    /// unless a symbol says otherwise: (length, polarity rule).
    Symbols(Vec<(u32, Polarity)>),
    /// Silence (low level) for `ms` milliseconds; 0 means "stop the tape".
    Pause(u32),
    /// Set the level.
    Level(bool),
    /// Stop the tape if the machine is a 48K.
    StopIn48k,
    Jump(i16),
    LoopStart(u16),
    LoopEnd,
    Call(Vec<i16>),
    Return,
    /// Informational blocks (text, archive info…): nothing to play.
    Info(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Polarity {
    /// The pulse starts with an edge (the usual case).
    Toggle,
    /// The pulse continues the current level.
    Same,
    Low,
    High,
}

fn u16le(d: &[u8], i: usize) -> Result<u32, TapeError> {
    d.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u32).ok_or(TapeError::Damaged("a block is cut short"))
}
fn u24le(d: &[u8], i: usize) -> Result<usize, TapeError> {
    d.get(i..i + 3).map(|b| b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16).ok_or(TapeError::Damaged("a block is cut short"))
}
fn u32le(d: &[u8], i: usize) -> Result<usize, TapeError> {
    d.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or(TapeError::Damaged("a block is cut short"))
}
fn bytes(d: &[u8], i: usize, n: usize) -> Result<&[u8], TapeError> {
    d.get(i..i.checked_add(n).ok_or(TapeError::Damaged("a block is too long"))?).ok_or(TapeError::Damaged("a block is cut short"))
}

/// The blocks of a standard-speed ROM block (pilot, sync, data, pause).
fn standard(data: &[u8], pause_ms: u32, out: &mut Vec<Block>) {
    let pilot = if data.first().is_some_and(|f| *f < 128) { 8063 } else { 3223 };
    out.push(Block::Pulses(vec![(2168, pilot), (667, 1), (735, 1)]));
    out.push(Block::Data { zero: 855, one: 1710, used_bits: 8, data: data.to_vec() });
    if pause_ms > 0 {
        out.push(Block::Pause(pause_ms));
    }
}

/// Parses a TZX or TAP file (by its contents).
pub fn parse(file: &[u8]) -> Result<Vec<Block>, TapeError> {
    if file.starts_with(b"ZXTape!\x1a") {
        parse_tzx(file)
    } else {
        parse_tap(file)
    }
}

fn parse_tap(d: &[u8]) -> Result<Vec<Block>, TapeError> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < d.len() {
        let n = u16le(d, i).map_err(|_| TapeError::NotATape)? as usize;
        let block = bytes(d, i + 2, n).map_err(|_| TapeError::NotATape)?;
        if n < 2 {
            return Err(TapeError::NotATape);
        }
        standard(block, 1000, &mut out);
        i += 2 + n;
    }
    if out.is_empty() {
        return Err(TapeError::NotATape);
    }
    Ok(out)
}

fn parse_tzx(d: &[u8]) -> Result<Vec<Block>, TapeError> {
    let mut out = Vec::new();
    let mut i = 10;
    while i < d.len() {
        let id = d[i];
        i += 1;
        match id {
            0x10 => {
                let pause = u16le(d, i)?;
                let n = u16le(d, i + 2)? as usize;
                standard(bytes(d, i + 4, n)?, pause, &mut out);
                i += 4 + n;
            }
            0x11 => {
                let (pilot, sync1, sync2, zero, one, count) = (u16le(d, i)?, u16le(d, i + 2)?, u16le(d, i + 4)?, u16le(d, i + 6)?, u16le(d, i + 8)?, u16le(d, i + 10)?);
                let used_bits = *d.get(i + 12).ok_or(TapeError::Damaged("turbo block"))?;
                let pause = u16le(d, i + 13)?;
                let n = u24le(d, i + 15)?;
                let data = bytes(d, i + 18, n)?.to_vec();
                out.push(Block::Pulses(vec![(pilot, count), (sync1, 1), (sync2, 1)]));
                out.push(Block::Data { zero, one, used_bits, data });
                if pause > 0 {
                    out.push(Block::Pause(pause));
                }
                i += 18 + n;
            }
            0x12 => {
                out.push(Block::Pulses(vec![(u16le(d, i)?, u16le(d, i + 2)?)]));
                i += 4;
            }
            0x13 => {
                let n = *d.get(i).ok_or(TapeError::Damaged("pulse sequence"))? as usize;
                let mut p = Vec::with_capacity(n);
                for k in 0..n {
                    p.push((u16le(d, i + 1 + 2 * k)?, 1));
                }
                out.push(Block::Pulses(p));
                i += 1 + 2 * n;
            }
            0x14 => {
                let (zero, one) = (u16le(d, i)?, u16le(d, i + 2)?);
                let used_bits = *d.get(i + 4).ok_or(TapeError::Damaged("pure data"))?;
                let pause = u16le(d, i + 5)?;
                let n = u24le(d, i + 7)?;
                out.push(Block::Data { zero, one, used_bits, data: bytes(d, i + 10, n)?.to_vec() });
                if pause > 0 {
                    out.push(Block::Pause(pause));
                }
                i += 10 + n;
            }
            0x15 => {
                let tps = u16le(d, i)?;
                let pause = u16le(d, i + 2)?;
                let used_bits = *d.get(i + 4).ok_or(TapeError::Damaged("direct recording"))?;
                let n = u24le(d, i + 5)?;
                out.push(Block::Direct { tstates_per_sample: tps.max(1), used_bits, data: bytes(d, i + 8, n)?.to_vec() });
                if pause > 0 {
                    out.push(Block::Pause(pause));
                }
                i += 8 + n;
            }
            0x18 => {
                let len = u32le(d, i)?;
                let b = bytes(d, i + 4, len)?;
                let pause = u16le(b, 0)?;
                let rate = u24le(b, 2)?.max(1) as u64;
                let compression = *b.get(5).ok_or(TapeError::Damaged("CSW"))?;
                let raw = &b[10.min(b.len())..];
                let rle = match compression {
                    1 => raw.to_vec(),
                    2 => miniz_oxide::inflate::decompress_to_vec_zlib(raw).map_err(|_| TapeError::Damaged("CSW data"))?,
                    _ => return Err(TapeError::Damaged("unknown CSW compression")),
                };
                let mut p = Vec::new();
                let mut k = 0;
                while k < rle.len() {
                    let mut samples = rle[k] as u64;
                    k += 1;
                    if samples == 0 {
                        samples = u32le(&rle, k)? as u64;
                        k += 4;
                    }
                    p.push(((samples * 3_500_000 / rate).max(1) as u32, Polarity::Toggle));
                }
                out.push(Block::Symbols(p));
                if pause > 0 {
                    out.push(Block::Pause(pause));
                }
                i += 4 + len;
            }
            0x19 => {
                let len = u32le(d, i)?;
                let b = bytes(d, i + 4, len)?;
                generalized(b, &mut out)?;
                i += 4 + len;
            }
            0x20 => {
                out.push(Block::Pause(u16le(d, i)?));
                i += 2;
            }
            0x21 => {
                let n = *d.get(i).ok_or(TapeError::Damaged("group"))? as usize;
                out.push(Block::Info(String::from_utf8_lossy(bytes(d, i + 1, n)?).into_owned()));
                i += 1 + n;
            }
            0x22 => out.push(Block::Info(String::new())),
            0x23 => {
                out.push(Block::Jump(u16le(d, i)? as u16 as i16));
                i += 2;
            }
            0x24 => {
                out.push(Block::LoopStart(u16le(d, i)? as u16));
                i += 2;
            }
            0x25 => out.push(Block::LoopEnd),
            0x26 => {
                let n = u16le(d, i)? as usize;
                let mut v = Vec::with_capacity(n);
                for k in 0..n {
                    v.push(u16le(d, i + 2 + 2 * k)? as u16 as i16);
                }
                out.push(Block::Call(v));
                i += 2 + 2 * n;
            }
            0x27 => out.push(Block::Return),
            0x28 => {
                // Select block: a menu for the user; we play on (like most emulators).
                let n = u16le(d, i)? as usize;
                out.push(Block::Info(String::new()));
                i += 2 + n;
            }
            0x2A => {
                out.push(Block::StopIn48k);
                i += 4;
            }
            0x2B => {
                out.push(Block::Level(*d.get(i + 4).ok_or(TapeError::Damaged("set level"))? != 0));
                i += 5;
            }
            0x30 => {
                let n = *d.get(i).ok_or(TapeError::Damaged("text"))? as usize;
                out.push(Block::Info(String::from_utf8_lossy(bytes(d, i + 1, n)?).into_owned()));
                i += 1 + n;
            }
            0x31 => {
                let n = *d.get(i + 1).ok_or(TapeError::Damaged("message"))? as usize;
                out.push(Block::Info(String::from_utf8_lossy(bytes(d, i + 2, n)?).into_owned()));
                i += 2 + n;
            }
            0x32 => {
                let n = u16le(d, i)? as usize;
                out.push(Block::Info(String::new()));
                i += 2 + n;
            }
            0x33 => {
                let n = *d.get(i).ok_or(TapeError::Damaged("hardware"))? as usize;
                out.push(Block::Info(String::new()));
                i += 1 + 3 * n;
            }
            0x35 => {
                let n = u32le(d, i + 16)?;
                out.push(Block::Info(String::new()));
                i += 20 + n;
            }
            0x5A => {
                out.push(Block::Info(String::new()));
                i += 9;
            }
            // Old/unknown blocks: since TZX 1.10 they start with their length.
            _ => {
                let n = u32le(d, i)?;
                out.push(Block::Info(String::new()));
                i += 4 + n;
            }
        }
        if i > d.len() {
            return Err(TapeError::Damaged("a block is cut short"));
        }
    }
    if out.is_empty() {
        return Err(TapeError::NotATape);
    }
    Ok(out)
}

/// TZX block 0x19 (generalized data): symbol tables for the pilot/sync and the data.
fn generalized(b: &[u8], out: &mut Vec<Block>) -> Result<(), TapeError> {
    let pause = u16le(b, 0)?;
    let totp = u32le(b, 2)?;
    let npp = *b.get(6).ok_or(TapeError::Damaged("generalized"))? as usize;
    let asp = match *b.get(7).ok_or(TapeError::Damaged("generalized"))? {
        0 => 256,
        n => n as usize,
    };
    let totd = u32le(b, 8)?;
    let npd = *b.get(12).ok_or(TapeError::Damaged("generalized"))? as usize;
    let asd = match *b.get(13).ok_or(TapeError::Damaged("generalized"))? {
        0 => 256,
        n => n as usize,
    };
    let mut i = 14;
    // A symbol: its polarity rule and up to n pulse lengths (a 0 ends it early).
    let read_symbols = |i: &mut usize, count: usize, np: usize| -> Result<Vec<(Polarity, Vec<u32>)>, TapeError> {
        let mut v = Vec::with_capacity(count);
        for _ in 0..count {
            let flags = *b.get(*i).ok_or(TapeError::Damaged("generalized symbols"))?;
            let pol = match flags & 3 {
                0 => Polarity::Toggle,
                1 => Polarity::Same,
                2 => Polarity::Low,
                _ => Polarity::High,
            };
            let mut p = Vec::with_capacity(np);
            for k in 0..np {
                let l = u16le(b, *i + 1 + 2 * k)?;
                if l == 0 {
                    break;
                }
                p.push(l);
            }
            v.push((pol, p));
            *i += 1 + 2 * np;
        }
        Ok(v)
    };
    let mut pulses: Vec<(u32, Polarity)> = Vec::new();
    let emit = |pulses: &mut Vec<(u32, Polarity)>, sym: &(Polarity, Vec<u32>)| {
        for (k, l) in sym.1.iter().enumerate() {
            pulses.push((*l, if k == 0 { sym.0 } else { Polarity::Toggle }));
        }
    };
    if totp > 0 {
        let syms = read_symbols(&mut i, asp, npp)?;
        for _ in 0..totp {
            let s = *b.get(i).ok_or(TapeError::Damaged("generalized pilot"))? as usize;
            let reps = u16le(b, i + 1)?;
            i += 3;
            let sym = syms.get(s).ok_or(TapeError::Damaged("generalized pilot symbol"))?;
            for _ in 0..reps {
                emit(&mut pulses, sym);
            }
        }
    }
    if totd > 0 {
        let syms = read_symbols(&mut i, asd, npd)?;
        let nb = (usize::BITS - (asd - 1).leading_zeros()).max(1) as usize;
        let stream = &b[i.min(b.len())..];
        for k in 0..totd {
            let mut s = 0usize;
            for j in 0..nb {
                let bit = k * nb + j;
                let byte = *stream.get(bit / 8).ok_or(TapeError::Damaged("generalized data"))?;
                s = (s << 1) | ((byte >> (7 - bit % 8)) & 1) as usize;
            }
            let sym = syms.get(s).ok_or(TapeError::Damaged("generalized data symbol"))?;
            emit(&mut pulses, sym);
        }
    }
    out.push(Block::Symbols(pulses));
    if pause > 0 {
        out.push(Block::Pause(pause));
    }
    Ok(())
}

// ------------------------------------------------------------------ playing

/// What a tape produces: a level held for a duration (T-states), or "stop the tape".
#[derive(Debug, Clone, Copy, PartialEq)]
enum Signal {
    Level(bool, u32),
    Stop,
}

/// Plays blocks as levels and durations (with TZX flow control).
struct Player {
    blocks: Vec<Block>,
    is_48k: bool,
    pos: usize,
    level: bool,
    loops: Vec<(usize, u16)>,
    calls: Vec<(usize, Vec<i16>, usize)>,
    /// Signals of the current block not yet handed out.
    queue: std::collections::VecDeque<Signal>,
    /// Steps taken without producing anything (protects against jump loops).
    idle: u32,
}

impl Player {
    fn toggle(&mut self, len: u32) {
        self.level = !self.level;
        self.queue.push_back(Signal::Level(self.level, len));
    }

    fn pause(&mut self, ms: u32) {
        if ms == 0 {
            self.queue.push_back(Signal::Stop);
            return;
        }
        // At least 1 ms of the opposite level ends the last pulse with an edge; then low.
        let total = ms * MS;
        let first = MS.min(total);
        self.level = !self.level;
        self.queue.push_back(Signal::Level(self.level, first));
        if total > first {
            self.level = false;
            self.queue.push_back(Signal::Level(false, total - first));
        }
        self.level = false;
    }

    /// Queues the next block's signals; false at the end of the tape.
    fn step(&mut self) -> bool {
        let Some(block) = self.blocks.get(self.pos).cloned() else { return false };
        let mut next = self.pos + 1;
        match block {
            Block::Pulses(p) => {
                for (len, count) in p {
                    for _ in 0..count {
                        self.toggle(len);
                    }
                }
            }
            Block::Data { zero, one, used_bits, data } => {
                let last = data.len().saturating_sub(1);
                for (k, byte) in data.iter().enumerate() {
                    let bits = if k == last { used_bits.clamp(1, 8) } else { 8 };
                    for b in 0..bits {
                        let len = if byte & (0x80 >> b) != 0 { one } else { zero };
                        self.toggle(len);
                        self.toggle(len);
                    }
                }
            }
            Block::Direct { tstates_per_sample, used_bits, data } => {
                let last = data.len().saturating_sub(1);
                for (k, byte) in data.iter().enumerate() {
                    let bits = if k == last { used_bits.clamp(1, 8) } else { 8 };
                    for b in 0..bits {
                        self.level = byte & (0x80 >> b) != 0;
                        self.queue.push_back(Signal::Level(self.level, tstates_per_sample));
                    }
                }
            }
            Block::Symbols(p) => {
                for (len, pol) in p {
                    self.level = match pol {
                        Polarity::Toggle => !self.level,
                        Polarity::Same => self.level,
                        Polarity::Low => false,
                        Polarity::High => true,
                    };
                    self.queue.push_back(Signal::Level(self.level, len));
                }
            }
            Block::Pause(ms) => self.pause(ms),
            Block::Level(l) => self.level = l,
            Block::StopIn48k => {
                if self.is_48k {
                    self.queue.push_back(Signal::Stop);
                }
            }
            Block::Jump(off) => next = (self.pos as i64 + off as i64).max(0) as usize,
            Block::LoopStart(n) => self.loops.push((next, n)),
            Block::LoopEnd => {
                if let Some((start, n)) = self.loops.pop() {
                    if n > 1 {
                        self.loops.push((start, n - 1));
                        next = start;
                    }
                }
            }
            Block::Call(offsets) => {
                if let Some(first) = offsets.first() {
                    let target = (self.pos as i64 + *first as i64).max(0) as usize;
                    self.calls.push((self.pos, offsets, 1));
                    next = target;
                }
            }
            Block::Return => {
                if let Some((from, offsets, k)) = self.calls.pop() {
                    match offsets.get(k) {
                        Some(off) => {
                            next = (from as i64 + *off as i64).max(0) as usize;
                            self.calls.push((from, offsets, k + 1));
                        }
                        None => next = from + 1,
                    }
                }
            }
            Block::Info(_) => {}
        }
        self.pos = next;
        true
    }

    fn next_signal(&mut self) -> Option<Signal> {
        loop {
            if let Some(s) = self.queue.pop_front() {
                self.idle = 0;
                return Some(s);
            }
            self.idle += 1;
            if self.idle > 100_000 || !self.step() {
                return None;
            }
        }
    }
}

/// A playing tape as pulses: T-states from one edge to the next, scaled to the machine's clock.
pub struct Pulses {
    player: Player,
    scale: (u64, u64),
    /// The tape asked to be stopped (a 0 ms pause, or "stop in 48K mode").
    stopped: bool,
    pending: Option<(bool, u64)>,
    total: u64,
    elapsed: u64,
}

impl Pulses {
    /// `cpu_hz`: the machine's clock (3 500 000 for a 48K, 3 546 900 for a 128K).
    pub fn new(blocks: Vec<Block>, cpu_hz: u32, is_48k: bool) -> Self {
        let total = estimate_tstates(&blocks);
        Pulses {
            player: Player { blocks, is_48k, pos: 0, level: false, loops: Vec::new(), calls: Vec::new(), queue: Default::default(), idle: 0 },
            scale: (cpu_hz as u64, 3_500_000),
            stopped: false,
            pending: None,
            total,
            elapsed: 0,
        }
    }

    /// The tape reached a "stop the tape" point; [`Pulses::resume`] carries on after it.
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn resume(&mut self) {
        self.stopped = false;
    }

    /// How far through the tape (0.0 – 1.0, estimated).
    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            (self.elapsed as f64 / self.total as f64).min(1.0) as f32
        }
    }

    /// The block being played (for showing the tape position).
    pub fn block(&self) -> usize {
        self.player.pos
    }
}

impl Iterator for Pulses {
    type Item = NonZeroU32;

    fn next(&mut self) -> Option<NonZeroU32> {
        if self.stopped {
            return None;
        }
        // Merge consecutive durations of the same level: a pulse ends at an edge.
        loop {
            match self.player.next_signal() {
                Some(Signal::Level(l, d)) => match self.pending {
                    Some((pl, pd)) if pl == l => self.pending = Some((pl, pd + d as u64)),
                    Some((_, pd)) => {
                        self.pending = Some((l, d as u64));
                        return Some(self.emit(pd));
                    }
                    None => self.pending = Some((l, d as u64)),
                },
                Some(Signal::Stop) => {
                    self.stopped = true;
                    return self.pending.take().map(|(_, pd)| self.emit(pd));
                }
                None => return self.pending.take().map(|(_, pd)| self.emit(pd)),
            }
        }
    }
}

impl Pulses {
    fn emit(&mut self, tstates: u64) -> NonZeroU32 {
        self.elapsed += tstates;
        let scaled = tstates * self.scale.0 / self.scale.1;
        NonZeroU32::new(scaled.clamp(1, u32::MAX as u64) as u32).unwrap()
    }
}

/// Roughly how long the tape plays, in T-states (for a progress bar; loops counted once).
fn estimate_tstates(blocks: &[Block]) -> u64 {
    blocks
        .iter()
        .map(|b| match b {
            Block::Pulses(p) => p.iter().map(|(l, c)| *l as u64 * *c as u64).sum(),
            Block::Data { zero, one, data, .. } => data.iter().map(|byte| (byte.count_ones() as u64 * *one as u64 + byte.count_zeros() as u64 * *zero as u64) * 2).sum(),
            Block::Direct { tstates_per_sample, data, .. } => data.len() as u64 * 8 * *tstates_per_sample as u64,
            Block::Symbols(p) => p.iter().map(|(l, _)| *l as u64).sum(),
            Block::Pause(ms) => *ms as u64 * MS as u64,
            _ => 0,
        })
        .sum()
}

/// Names and texts found on the tape (program names in headers, TZX text blocks).
pub fn describe(blocks: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    for b in blocks {
        match b {
            Block::Data { data, .. } if data.len() == 19 && data[0] == 0 => {
                let name = String::from_utf8_lossy(&data[2..12]).trim().to_string();
                let kind = match data[1] {
                    0 => "Program",
                    3 => "Bytes",
                    _ => "Array",
                };
                out.push(format!("{kind}: {name}"));
            }
            Block::Info(t) if !t.trim().is_empty() => out.push(t.trim().to_string()),
            _ => {}
        }
    }
    out
}

/// A block of bytes on a tape, with its ROM header decoded when it is one.
#[derive(Debug, Clone, PartialEq)]
pub struct DataBlock {
    pub index: usize,
    /// "standard" (the ROM's own speed) or "custom" (a turbo or protected loader).
    pub speed: &'static str,
    /// The flag byte: 0 = a header, 255 = data (custom loaders use anything).
    pub flag: u8,
    pub header: Option<Header>,
    /// The bytes without the flag and checksum (when the block looks like a ROM block).
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// 0 program, 1 number array, 2 character array, 3 bytes (code or a screen).
    pub kind: u8,
    pub name: String,
    pub length: u16,
    /// Programs: the autostart line; bytes: the address they load to.
    pub param1: u16,
    pub param2: u16,
}

/// The tape's data blocks, in order.
pub fn data_blocks(blocks: &[Block]) -> Vec<DataBlock> {
    let mut out = Vec::new();
    for (index, b) in blocks.iter().enumerate() {
        let Block::Data { zero, one, data, .. } = b else { continue };
        if data.is_empty() {
            continue;
        }
        let speed = if (*zero, *one) == (855, 1710) { "standard" } else { "custom" };
        let flag = data[0];
        let checksum_ok = data.iter().fold(0u8, |a, v| a ^ v) == 0;
        let body = if checksum_ok && data.len() >= 2 { data[1..data.len() - 1].to_vec() } else { data.clone() };
        let header = (flag == 0 && body.len() == 17).then(|| Header {
            kind: body[0],
            name: String::from_utf8_lossy(&body[1..11]).trim_end().to_string(),
            length: u16::from_le_bytes([body[11], body[12]]),
            param1: u16::from_le_bytes([body[13], body[14]]),
            param2: u16::from_le_bytes([body[15], body[16]]),
        });
        out.push(DataBlock { index, speed, flag, header, data: body });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tzx(blocks: &[u8]) -> Vec<u8> {
        let mut v = b"ZXTape!\x1a\x01\x14".to_vec();
        v.extend_from_slice(blocks);
        v
    }

    fn levels(blocks: Vec<Block>) -> Vec<u32> {
        Pulses::new(blocks, 3_500_000, true).map(|p| p.get()).collect()
    }

    #[test]
    fn tap_and_standard_tzx_give_the_same_pulses() {
        let tap = [3u8, 0, 0xff, 0xaa, 0x55];
        let a = levels(parse(&tap).unwrap());
        let b = levels(parse(&tzx(&[0x10, 0xe8, 0x03, 3, 0, 0xff, 0xaa, 0x55])).unwrap());
        assert_eq!(a, b);
        // Data block (flag 0xff): short pilot, sync, then 24 bits as pulse pairs.
        assert_eq!(&a[..3], &[2168, 2168, 2168]);
        assert_eq!(a[3223], 667);
        assert_eq!(a[3224], 735);
        assert_eq!(&a[3225..3227], &[1710, 1710], "0xff starts with a one");
        assert_eq!(a.len(), 3223 + 2 + 48 + 1, "…then the pause's closing pulse");
    }

    #[test]
    fn turbo_pure_data_and_generalized_blocks() {
        // Turbo block with tiny pilot, one byte 0b1000_0000 using 2 bits.
        let mut t = vec![0x11];
        for v in [100u16, 10, 20, 30, 60, 4] {
            t.extend_from_slice(&v.to_le_bytes());
        }
        t.push(2);
        t.extend_from_slice(&0u16.to_le_bytes());
        t.extend_from_slice(&[1, 0, 0, 0x80]);
        assert_eq!(levels(parse(&tzx(&t)).unwrap()), [100, 100, 100, 100, 10, 20, 60, 60, 30, 30]);

        // Generalized: no pilot; data alphabet of 2 symbols (0: 2×5, 1: 2×9), 3 symbols "101".
        let mut g = vec![0x19];
        let mut body = Vec::new();
        body.extend_from_slice(&0u16.to_le_bytes()); // pause
        body.extend_from_slice(&0u32.to_le_bytes()); // TOTP
        body.extend_from_slice(&[0, 0]); // NPP, ASP
        body.extend_from_slice(&3u32.to_le_bytes()); // TOTD
        body.extend_from_slice(&[2, 2]); // NPD, ASD
        body.extend_from_slice(&[0, 5, 0, 5, 0]);
        body.extend_from_slice(&[0, 9, 0, 9, 0]);
        body.push(0b1010_0000);
        g.extend_from_slice(&(body.len() as u32).to_le_bytes());
        g.extend_from_slice(&body);
        assert_eq!(levels(parse(&tzx(&g)).unwrap()), [9, 9, 5, 5, 9, 9]);
    }

    #[test]
    fn loops_jumps_stops_and_scaling() {
        let mut t = vec![0x24, 3, 0, 0x12, 50, 0, 1, 0, 0x25];
        t.extend_from_slice(&[0x23, 2, 0]); // jump over the next block
        t.extend_from_slice(&[0x12, 99, 0, 1, 0]);
        t.extend_from_slice(&[0x20, 0, 0]); // stop the tape
        t.extend_from_slice(&[0x12, 77, 0, 1, 0]);
        let blocks = parse(&tzx(&t)).unwrap();
        let mut p = Pulses::new(blocks.clone(), 3_500_000, true);
        let first: Vec<u32> = p.by_ref().map(|x| x.get()).collect();
        assert_eq!(first, [50, 50, 50], "the loop runs 3 times, the jump skips 99");
        assert!(p.is_stopped());
        p.resume();
        assert_eq!(p.map(|x| x.get()).collect::<Vec<_>>(), [77]);
        // A 128K runs a little faster than the tape's 3.5 MHz T-states.
        let fast: Vec<u32> = Pulses::new(vec![Block::Pulses(vec![(3_500, 1), (3_500, 1)])], 3_546_900, false).map(|x| x.get()).collect();
        assert_eq!(fast, [3546, 3546]);
    }

    #[test]
    fn direct_recording_and_csw() {
        // Direct: levels 1,1,0,1 at 10 T-states each → 20 high, 10 low, 10 high.
        let mut d = vec![0x15, 10, 0, 0, 0, 4, 1, 0, 0, 0b1101_0000];
        assert_eq!(levels(parse(&tzx(&d)).unwrap()), [20, 10, 10]);
        // CSW RLE at 3.5 MHz: pulses of 7, 300 and 70000 samples (the long ones escaped by a 0).
        let mut body = Vec::new();
        body.extend_from_slice(&0u16.to_le_bytes()); // pause
        body.extend_from_slice(&[0xe0, 0x67, 0x35]); // 3 500 000 Hz
        body.push(1); // RLE
        body.extend_from_slice(&3u32.to_le_bytes()); // pulses
        body.push(7);
        body.push(0);
        body.extend_from_slice(&300u32.to_le_bytes());
        body.push(0);
        body.extend_from_slice(&70000u32.to_le_bytes());
        d = vec![0x18];
        d.extend_from_slice(&(body.len() as u32).to_le_bytes());
        d.extend_from_slice(&body);
        assert_eq!(levels(parse(&tzx(&d)).unwrap()), [7, 300, 70000]);
    }

    #[test]
    fn rejects_junk() {
        assert_eq!(parse(b"hello"), Err(TapeError::NotATape));
        assert!(parse(&tzx(&[0x10, 0, 0, 50, 0, 1])).is_err());
    }
}

//! C64 files: programs (PRG, P00), tape archives (T64), tape recordings (TAP), disk images
//! (D64) and cartridges (CRT).

use crate::C64Error;

/// A program file: its load address and bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Prg {
    pub name: String,
    pub addr: u16,
    pub data: Vec<u8>,
}

impl Prg {
    pub fn parse(name: &str, file: &[u8]) -> Result<Prg, C64Error> {
        if file.len() < 3 {
            return Err(C64Error::Format("That program file is empty".into()));
        }
        Ok(Prg { name: name.to_string(), addr: u16::from_le_bytes([file[0], file[1]]), data: file[2..].to_vec() })
    }

    /// PC64's P00: "C64File\0", a 16-byte PETSCII name, then a PRG.
    pub fn parse_p00(file: &[u8]) -> Result<Prg, C64Error> {
        if file.len() < 29 || &file[..8] != b"C64File\0" {
            return Err(C64Error::Format("Not a P00 file".into()));
        }
        Prg::parse(&petscii_name(&file[8..24]), &file[26..])
    }

    /// The file as written to disk (address first).
    pub fn bytes(&self) -> Vec<u8> {
        let mut v = self.addr.to_le_bytes().to_vec();
        v.extend_from_slice(&self.data);
        v
    }

    pub fn end(&self) -> u32 {
        self.addr as u32 + self.data.len() as u32
    }

    /// A BASIC program that's only `SYS <addr>` (the usual way machine code starts): the
    /// address.
    pub fn sys_address(&self) -> Option<u16> {
        if self.addr != 0x0801 || self.data.len() < 7 {
            return None;
        }
        // link(2) line(2) SYS-token, then digits (maybe spaces / a "(")
        if self.data[4] != 0x9e {
            return None;
        }
        let digits: String = self.data[5..].iter().take_while(|&&b| b != 0).map(|&b| b as char).filter(|c| !c.is_whitespace() && *c != '(').take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u32>().ok().filter(|a| *a <= 0xffff).map(|a| a as u16)
    }
}

/// PETSCII bytes (a name padded with 0xA0) as text.
pub fn petscii_name(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0xa0 && c != 0).map(|&c| petscii_char(c)).collect::<String>().trim_end().to_string()
}

pub fn petscii_char(c: u8) -> char {
    match c {
        0x20..=0x5a => c as char,
        0x5c => '£',
        0x5e => '↑',
        0x5f => '←',
        0x5b | 0x5d => c as char,
        0xc1..=0xda => (c - 0x80) as char,
        _ => '?',
    }
}

// ------------------------------------------------------------------ T64

/// The programs in a T64 tape archive.
pub fn t64(file: &[u8]) -> Result<Vec<Prg>, C64Error> {
    if file.len() < 0x40 || !file.starts_with(b"C64") {
        return Err(C64Error::Format("Not a T64 file".into()));
    }
    let max = u16::from_le_bytes([file[0x22], file[0x23]]).max(1) as usize;
    let mut entries = Vec::new();
    for i in 0..max {
        let e = 0x40 + i * 32;
        if e + 32 > file.len() {
            break;
        }
        let d = &file[e..e + 32];
        if d[0] != 1 {
            continue;
        }
        let start = u16::from_le_bytes([d[2], d[3]]);
        let end = u16::from_le_bytes([d[4], d[5]]);
        let offset = u32::from_le_bytes([d[8], d[9], d[10], d[11]]) as usize;
        entries.push((start, end, offset, petscii_name(&d[16..32])));
    }
    // End addresses are often wrong (written by old tools): a file runs until the next one
    // starts, or to the end of the archive.
    let mut offsets: Vec<usize> = entries.iter().map(|e| e.2).collect();
    offsets.push(file.len());
    offsets.sort_unstable();
    let mut out = Vec::new();
    for (start, end, offset, name) in entries {
        if offset >= file.len() {
            continue;
        }
        let next = offsets.iter().copied().find(|&o| o > offset).unwrap_or(file.len());
        let stated = (end as usize).saturating_sub(start as usize);
        let room = next - offset;
        let len = if stated == 0 || stated > room { room } else { stated };
        out.push(Prg { name, addr: start, data: file[offset..offset + len].to_vec() });
    }
    if out.is_empty() {
        return Err(C64Error::Format("That T64 archive has no programs".into()));
    }
    Ok(out)
}

// ------------------------------------------------------------------ TAP

/// A TAP recording as the datasette plays it (version 1: a 0 byte is followed by a 24-bit
/// length in cycles). Version 0 files (a 0 byte meaning "a long pause") are converted.
pub fn tap(file: &[u8]) -> Result<Vec<u8>, C64Error> {
    if file.len() < 20 || &file[..12] != b"C64-TAPE-RAW" {
        return Err(C64Error::Format("Not a TAP file".into()));
    }
    let size = (u32::from_le_bytes([file[16], file[17], file[18], file[19]]) as usize).min(file.len() - 20);
    let data = &file[20..20 + size];
    if file[12] == 1 {
        let mut v = file[..20 + size].to_vec();
        v[16..20].copy_from_slice(&(size as u32).to_le_bytes());
        return Ok(v);
    }
    let mut body = Vec::with_capacity(data.len());
    for &b in data {
        if b == 0 {
            body.extend_from_slice(&[0, 0x00, 0x08, 0x00]); // 2048 cycles
        } else {
            body.push(b);
        }
    }
    let mut v = b"C64-TAPE-RAW".to_vec();
    v.extend_from_slice(&[1, 0, 0, 0]);
    v.extend_from_slice(&(body.len() as u32).to_le_bytes());
    v.extend(body);
    Ok(v)
}

/// A program recorded the way the C64's own SAVE writes tapes (a header block and the data,
/// each twice), as a TAP file that LOAD reads back.
pub fn tap_from_prg(name: &str, prg: &Prg) -> Vec<u8> {
    const S: u8 = 0x30;
    const M: u8 = 0x42;
    const L: u8 = 0x56;
    fn byte(out: &mut Vec<u8>, b: u8) {
        out.extend([L, M]);
        let mut parity = 1u8;
        for i in 0..8 {
            let bit = (b >> i) & 1;
            parity ^= bit;
            out.extend(if bit == 1 { [M, S] } else { [S, M] });
        }
        out.extend(if parity == 1 { [M, S] } else { [S, M] });
    }
    fn block(out: &mut Vec<u8>, data: &[u8], leader: usize) {
        for copy in 0..2 {
            out.extend(std::iter::repeat_n(S, if copy == 0 { leader } else { 0x4f }));
            let first = if copy == 0 { 0x89u8 } else { 0x09 };
            for k in 0..9 {
                byte(out, first - k);
            }
            let mut check = 0u8;
            for &b in data {
                byte(out, b);
                check ^= b;
            }
            byte(out, check);
            out.extend([L, S]);
        }
        out.extend(std::iter::repeat_n(S, 0x4e));
    }
    let end = prg.end().min(0xffff) as u16;
    let mut header = vec![1u8];
    header.extend(prg.addr.to_le_bytes());
    header.extend(end.to_le_bytes());
    let mut n: Vec<u8> = name.to_ascii_uppercase().bytes().filter(|b| (0x20..0x60).contains(b)).take(16).collect();
    n.resize(16, b' ');
    header.extend(n);
    header.resize(192, b' ');
    let mut pulses = Vec::new();
    block(&mut pulses, &header, 0x6a00);
    block(&mut pulses, &prg.data, 0x1500);
    let mut out = b"C64-TAPE-RAW".to_vec();
    out.extend([1, 0, 0, 0]);
    out.extend((pulses.len() as u32).to_le_bytes());
    out.extend(pulses);
    out
}

// ------------------------------------------------------------------ D64

/// A 1541 disk image (35 or 40 tracks).
#[derive(Debug, Clone)]
pub struct D64 {
    pub data: Vec<u8>,
    tracks: u8,
}

/// A file in a disk's directory.
#[derive(Debug, Clone, PartialEq)]
pub struct DirEntry {
    pub name: String,
    /// PETSCII name bytes (without the padding).
    pub raw: Vec<u8>,
    /// PRG, SEQ, USR, REL or DEL.
    pub kind: &'static str,
    pub blocks: u16,
    track: u8,
    sector: u8,
}

fn sectors(track: u8) -> u8 {
    match track {
        1..=17 => 21,
        18..=24 => 19,
        25..=30 => 18,
        _ => 17,
    }
}

impl D64 {
    pub fn parse(file: &[u8]) -> Result<D64, C64Error> {
        let tracks = match file.len() {
            174_848 | 175_531 => 35,
            196_608 | 197_376 => 40,
            _ => return Err(C64Error::Format("Not a D64 disk image (G64 and other formats aren't supported)".into())),
        };
        Ok(D64 { data: file.to_vec(), tracks })
    }

    fn sector(&self, track: u8, sector: u8) -> Option<&[u8]> {
        if track == 0 || track > self.tracks || sector >= sectors(track) {
            return None;
        }
        let before: usize = (1..track).map(|t| sectors(t) as usize).sum();
        let off = (before + sector as usize) * 256;
        self.data.get(off..off + 256)
    }

    /// The disk's name and ID.
    pub fn title(&self) -> (String, String) {
        match self.sector(18, 0) {
            Some(s) => (petscii_name(&s[0x90..0xa0]), petscii_name(&s[0xa2..0xa7])),
            None => (String::new(), String::new()),
        }
    }

    pub fn blocks_free(&self) -> u16 {
        let Some(bam) = self.sector(18, 0) else { return 0 };
        (1..=35u8).filter(|&t| t != 18).map(|t| bam[4 * t as usize] as u16).sum()
    }

    pub fn directory(&self) -> Vec<DirEntry> {
        let mut out = Vec::new();
        let (mut t, mut s) = (18u8, 1u8);
        let mut seen = 0;
        while t != 0 && seen < 20 {
            seen += 1;
            let Some(sec) = self.sector(t, s) else { break };
            for i in 0..8 {
                let e = &sec[i * 32..i * 32 + 32];
                let ty = e[2];
                if ty & 0x80 == 0 {
                    continue; // deleted / not closed
                }
                let kind = match ty & 7 {
                    0 => "DEL",
                    1 => "SEQ",
                    2 => "PRG",
                    3 => "USR",
                    _ => "REL",
                };
                let raw: Vec<u8> = e[5..21].iter().copied().take_while(|&c| c != 0xa0).collect();
                out.push(DirEntry { name: petscii_name(&e[5..21]), raw, kind, blocks: u16::from_le_bytes([e[30], e[31]]), track: e[3], sector: e[4] });
            }
            (t, s) = (sec[0], sec[1]);
        }
        out
    }

    pub fn read(&self, e: &DirEntry) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut t, mut s) = (e.track, e.sector);
        let mut n = 0;
        while t != 0 && n < 800 {
            n += 1;
            let Some(sec) = self.sector(t, s) else { break };
            if sec[0] == 0 {
                let last = (sec[1] as usize).clamp(1, 255);
                out.extend_from_slice(&sec[2..=last]);
                break;
            }
            out.extend_from_slice(&sec[2..]);
            (t, s) = (sec[0], sec[1]);
        }
        out
    }

    /// The first loadable file whose name matches a LOAD pattern (`*`, `NAME*`, `?`, a
    /// leading `0:` or `:`).
    pub fn find(&self, pattern: &[u8]) -> Option<DirEntry> {
        let p = match pattern.iter().position(|&c| c == b':') {
            Some(i) if i <= 1 => &pattern[i + 1..],
            _ => pattern,
        };
        self.directory().into_iter().filter(|e| e.kind != "DEL").find(|e| matches(p, &e.raw))
    }

    /// A new 35-track disk with these programs on it (each a PRG file's bytes).
    pub fn build(name: &str, id: &str, files: &[(&str, &[u8])]) -> Result<D64, C64Error> {
        let mut d = D64 { data: vec![0u8; 174_848], tracks: 35 };
        let off = |t: u8, s: u8| ((1..t).map(|t| sectors(t) as usize).sum::<usize>() + s as usize) * 256;
        let mut used: Vec<(u8, u8)> = vec![(18, 0), (18, 1)];
        // Files fill tracks from 17 down and 19 up, like a 1541 (but without interleave).
        let order: Vec<u8> = (1..=17u8).rev().chain(19..=35).collect();
        let mut free = order.iter().flat_map(|&t| (0..sectors(t)).map(move |s| (t, s)));
        let mut dir = Vec::new();
        for (fname, bytes) in files {
            let chunks: Vec<&[u8]> = if bytes.is_empty() { vec![&[][..]] } else { bytes.chunks(254).collect() };
            let secs: Vec<(u8, u8)> = (0..chunks.len()).map(|_| free.next().ok_or_else(|| C64Error::Format("the files don't fit on one disk".into()))).collect::<Result<_, _>>()?;
            for (k, chunk) in chunks.iter().enumerate() {
                let o = off(secs[k].0, secs[k].1);
                match secs.get(k + 1) {
                    Some(&(t, s)) => (d.data[o], d.data[o + 1]) = (t, s),
                    None => (d.data[o], d.data[o + 1]) = (0, chunk.len() as u8 + 1),
                }
                d.data[o + 2..o + 2 + chunk.len()].copy_from_slice(chunk);
            }
            used.extend(&secs);
            dir.push((*fname, secs[0], secs.len() as u16));
        }
        if dir.len() > 8 {
            return Err(C64Error::Format("RAD's disk builder holds up to 8 files".into()));
        }
        let pad = |s: &str, n: usize| -> Vec<u8> {
            let mut v: Vec<u8> = s.to_ascii_uppercase().bytes().take(n).collect();
            v.resize(n, 0xa0);
            v
        };
        // Directory (18/1).
        let o = off(18, 1);
        d.data[o] = 0;
        d.data[o + 1] = 0xff;
        for (i, (fname, (t, s), blocks)) in dir.iter().enumerate() {
            let e = o + i * 32;
            d.data[e + 2] = 0x82;
            d.data[e + 3] = *t;
            d.data[e + 4] = *s;
            d.data[e + 5..e + 21].copy_from_slice(&pad(fname, 16));
            d.data[e + 30..e + 32].copy_from_slice(&blocks.to_le_bytes());
        }
        // BAM (18/0).
        let b = off(18, 0);
        d.data[b..b + 4].copy_from_slice(&[18, 1, 0x41, 0]);
        for t in 1..=35u8 {
            let mut bits = 0u32;
            let mut n = 0u8;
            for s in 0..sectors(t) {
                if !used.contains(&(t, s)) {
                    bits |= 1 << s;
                    n += 1;
                }
            }
            let e = b + 4 * t as usize;
            d.data[e] = n;
            d.data[e + 1..e + 4].copy_from_slice(&bits.to_le_bytes()[..3]);
        }
        d.data[b + 0x90..b + 0xa0].copy_from_slice(&pad(name, 16));
        d.data[b + 0xa0..b + 0xa2].fill(0xa0);
        d.data[b + 0xa2..b + 0xa4].copy_from_slice(&pad(id, 2));
        d.data[b + 0xa4] = 0xa0;
        d.data[b + 0xa5..b + 0xa7].copy_from_slice(b"2A");
        d.data[b + 0xa7..b + 0xab].fill(0xa0);
        Ok(d)
    }

    /// The directory as a BASIC program loaded at `addr` (what `LOAD"$",8` gives).
    pub fn listing(&self, addr: u16) -> Vec<u8> {
        let (name, id) = self.title();
        let mut lines: Vec<(u16, Vec<u8>)> = Vec::new();
        let mut head = vec![0x12, b'"'];
        let mut n: Vec<u8> = name.bytes().collect();
        n.resize(16, b' ');
        head.extend(n);
        head.extend_from_slice(b"\" ");
        head.extend(id.bytes());
        lines.push((0, head));
        for e in self.directory() {
            let pad = match e.blocks {
                0..=9 => 3,
                10..=99 => 2,
                _ => 1,
            };
            let mut l = vec![b' '; pad];
            l.push(b'"');
            l.extend(&e.raw);
            l.push(b'"');
            l.extend(std::iter::repeat_n(b' ', 17usize.saturating_sub(e.raw.len() + 1)));
            l.push(b' ');
            l.extend(e.kind.bytes());
            lines.push((e.blocks, l));
        }
        lines.push((self.blocks_free(), b"BLOCKS FREE.".to_vec()));
        let mut out = Vec::new();
        let mut at = addr as usize;
        for (num, text) in lines {
            let next = at + 4 + text.len() + 1;
            out.extend((next as u16).to_le_bytes());
            out.extend(num.to_le_bytes());
            out.extend(text);
            out.push(0);
            at = next;
        }
        out.extend([0, 0]);
        out
    }
}

/// CBM DOS pattern matching: `*` ends the match, `?` matches any one character.
fn matches(pattern: &[u8], name: &[u8]) -> bool {
    let mut i = 0;
    for &c in pattern {
        match c {
            b'*' => return true,
            b'?' => {
                if i >= name.len() {
                    return false;
                }
            }
            _ => {
                if name.get(i) != Some(&c) {
                    return false;
                }
            }
        }
        i += 1;
    }
    i == name.len()
}

// ------------------------------------------------------------------ CRT

/// A cartridge: its hardware type and its ROM, laid out for the cartridge port (bank n's
/// ROML at n × 16K, its ROMH 8K above that).
#[derive(Debug, Clone)]
pub struct Crt {
    pub name: String,
    pub hw: u16,
    pub exrom: bool,
    pub game: bool,
    pub rom: Vec<u8>,
    pub has_h: [u8; crate::ffi::CART_BANKS],
}

/// The cartridge types RAD can run.
pub fn crt_type_name(hw: u16) -> Option<&'static str> {
    Some(match hw {
        0 => "Normal cartridge",
        5 => "Ocean",
        15 => "C64 Game System",
        17 => "Dinamic",
        19 => "Magic Desk",
        _ => return None,
    })
}

impl Crt {
    pub fn parse(file: &[u8]) -> Result<Crt, C64Error> {
        if file.len() < 0x40 || &file[..16] != b"C64 CARTRIDGE   " {
            return Err(C64Error::Format("Not a CRT cartridge file".into()));
        }
        let be16 = |o: usize| u16::from_be_bytes([file[o], file[o + 1]]);
        let be32 = |o: usize| u32::from_be_bytes([file[o], file[o + 1], file[o + 2], file[o + 3]]);
        let header = (be32(0x10) as usize).max(0x40);
        let hw = be16(0x16);
        let exrom = file[0x18] == 0;
        let game = file[0x19] == 0;
        let name = String::from_utf8_lossy(&file[0x20..0x40]).trim_end_matches('\0').trim().to_string();
        if crt_type_name(hw).is_none() {
            return Err(C64Error::Format(format!("This cartridge type ({hw}) isn't supported yet")));
        }
        if game && !exrom {
            return Err(C64Error::Format("Ultimax cartridges aren't supported yet".into()));
        }
        let mut rom = vec![0xffu8; crate::ffi::CART_MAX];
        let mut has_h = [0u8; crate::ffi::CART_BANKS];
        let mut top = 0usize;
        let mut at = header;
        while at + 16 <= file.len() && &file[at..at + 4] == b"CHIP" {
            let len = be32(at + 4) as usize;
            let bank = be16(at + 0x0a) as usize;
            let load = be16(at + 0x0c);
            let size = be16(at + 0x0e) as usize;
            let data = &file[(at + 16).min(file.len())..(at + 16 + size).min(file.len())];
            if bank >= crate::ffi::CART_BANKS {
                return Err(C64Error::Format("That cartridge is too big".into()));
            }
            let base = bank * 0x4000;
            match load {
                0x8000 => {
                    rom[base..base + data.len().min(0x4000)].copy_from_slice(&data[..data.len().min(0x4000)]);
                    if data.len() > 0x2000 {
                        has_h[bank] = 1;
                    }
                }
                0xa000 | 0xe000 => {
                    rom[base + 0x2000..base + 0x2000 + data.len().min(0x2000)].copy_from_slice(&data[..data.len().min(0x2000)]);
                    has_h[bank] = 1;
                }
                _ => {}
            }
            top = top.max(base + 0x4000);
            if len < 16 {
                break;
            }
            at += len;
        }
        if top == 0 {
            return Err(C64Error::Format("That cartridge has no ROM in it".into()));
        }
        rom.truncate(top);
        Ok(Crt { name, hw, exrom, game, rom, has_h })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(matches(b"*", b"GAME"));
        assert!(matches(b"GA*", b"GAME"));
        assert!(matches(b"G?ME", b"GAME"));
        assert!(!matches(b"GAM", b"GAME"));
        assert!(matches(b"GAME", b"GAME"));
    }

    #[test]
    fn sys_stub() {
        let p = Prg { name: "x".into(), addr: 0x0801, data: vec![0x0b, 0x08, 0x0a, 0x00, 0x9e, b'2', b'0', b'6', b'1', 0, 0, 0] };
        assert_eq!(p.sys_address(), Some(2061));
    }

    #[test]
    fn tap_v0_converts() {
        let mut f = b"C64-TAPE-RAW".to_vec();
        f.extend([0, 0, 0, 0, 3, 0, 0, 0, 0x30, 0, 0x40]);
        let t = tap(&f).unwrap();
        assert_eq!(t[12], 1);
        assert_eq!(&t[20..], &[0x30, 0, 0, 8, 0, 0x40]);
        assert_eq!(u32::from_le_bytes([t[16], t[17], t[18], t[19]]), 6);
    }
}

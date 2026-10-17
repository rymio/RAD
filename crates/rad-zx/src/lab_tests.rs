//! The lab against programs whose source we know.

use super::lab::*;
use super::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn sjasm() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../zxtools/bin/sjasmplus");
    p.is_file().then_some(p)
}

/// Assembles `src` and returns the bytes from `from` to `to`.
fn assemble(asm: &Path, src: &str, from: u16, to: u32) -> Result<Vec<u8>, String> {
    let dir = tempfile::tempdir().unwrap();
    let full = format!("    DEVICE ZXSPECTRUM48\n{src}\n    SAVEBIN \"out.bin\", ${from:04x}, {}\n", to - from as u32);
    std::fs::write(dir.path().join("src.asm"), full).unwrap();
    let out = std::process::Command::new(asm).args(["--nologo", "--msg=war", "src.asm"]).current_dir(dir.path()).output().unwrap();
    if !out.status.success() {
        return Err(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    }
    Ok(std::fs::read(dir.path().join("out.bin")).unwrap())
}

const BAT: &str = include_str!("../../../app/ui/src/lib/zx-templates/bat-and-ball.asm");

/// Bat & Ball as a snapshot, and as a raw block at $8000.
fn bat(asm: &Path) -> (Vec<u8>, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.asm"), BAT).unwrap();
    std::fs::write(dir.path().join("w.asm"), "    DEVICE ZXSPECTRUM48\n    INCLUDE \"main.asm\"\n    SAVESNA \"g.sna\", start\n    SAVEBIN \"g.bin\", $8000, $1000\n").unwrap();
    let out = std::process::Command::new(asm).args(["--nologo", "w.asm"]).current_dir(dir.path()).output().unwrap();
    assert!(out.status.success());
    let bin = std::fs::read(dir.path().join("g.bin")).unwrap();
    let end = bin.iter().rposition(|b| *b != 0).unwrap() + 1;
    (std::fs::read(dir.path().join("g.sna")).unwrap(), bin[..end].to_vec())
}

#[test]
fn traces_and_reassembles_a_game() {
    let Some(asm) = sjasm() else { return eprintln!("skipped: ./scripts/build-zx-tools.sh") };
    let (sna, _) = bat(&asm);
    let an = analyze("bat.sna", &sna, 6).unwrap();
    println!("steps {} entry {:?} calls {} jumps {} ports {:?}", an.steps, an.entry, an.calls.len(), an.jumps.len(), an.ports);
    // The game's own routines were seen running, and its ports.
    assert!(an.calls.contains(&0x8000) || an.kinds[0x8000] == Kind::Code);
    assert!(an.ports.contains_key("in $fe") && an.ports.contains_key("out $fe") && an.ports.contains_key("in $1f"), "{:?}", an.ports);
    assert!(an.calls.len() >= 5, "input, move_ball, attr_addr, print, char…");
    let notes = Notes::default();
    let (from, to) = program_range(&an);
    println!("range {from:04x}-{to:04x}");
    assert_eq!(from, 0x8000);
    let src = source(&an, &notes, from, to, &BTreeSet::new(), "Bat & Ball");
    let back = assemble(&asm, &src, from, to).unwrap_or_else(|e| panic!("{e}\n{src}"));
    assert_eq!(back, an.memory[from as usize..to as usize], "the source assembles back to the same bytes:\n{src}");
    // Text and data were recognised.
    assert!(src.contains("\"BAT & BALL  O/P\""), "{src}");
    let lines = listing(&an, &notes, from, to, &BTreeSet::new());
    assert!(lines.iter().any(|l| l.text.starts_with("out  ($fe),a") && l.comment.as_deref().is_some_and(|c| c.contains("border"))));
    for l in lines.iter().rev().take(12).rev() {
        println!("{:04x} {:<10} {:<30} {:?} var={}", l.addr, l.kind, l.text, l.label, l.variable);
    }
    assert!(lines.iter().any(|l| l.kind == "data" && l.variable), "variables (bat_x, score…) are marked");
}

#[test]
fn the_whole_rom_reassembles() {
    // Every instruction form the ROM uses, decoded linearly as code: the formatter must write
    // each so sjasmplus makes the same bytes (or fall back to db for the few it can't).
    let Some(asm) = sjasm() else { return };
    let zx = Spectrum::new(Model::Spectrum48);
    let memory = zx.memory();
    let mut kinds = vec![Kind::Data; 0x10000];
    let mut pc: u32 = 0;
    while pc < 0x3d00 {
        let d = decode(&memory, pc as u16);
        kinds[pc as usize] = Kind::Code;
        pc += d.code.len().max(1) as u32;
    }
    let an = Analysis {
        loaded: Vec::new(), memory, kinds, written: vec![false; 0x10000], entry: None, calls: Default::default(), jumps: Default::default(),
        data_refs: Default::default(), ports: Default::default(), rom_calls: Default::default(), model: Model::Spectrum48, basic: None, tape: Vec::new(), steps: 0, frames: 0,
    };
    let mut force = BTreeSet::new();
    for round in 0..3 {
        let src = source(&an, &Notes::default(), 0, 0x3d00, &force, "ROM");
        let back = assemble(&asm, &src, 0, 0x3d00).unwrap_or_else(|e| panic!("{e}"));
        let lines = listing(&an, &Notes::default(), 0, 0x3d00, &force);
        let bad: Vec<&Line> = lines.iter().filter(|l| l.kind == "code" && back[l.addr as usize..l.addr as usize + l.bytes.len()] != l.bytes[..]).collect();
        println!("round {round}: {} instructions differ", bad.len());
        for l in bad.iter().take(15) {
            println!("  {:04x} {:<24} {:02x?} → {:02x?}", l.addr, l.text, l.bytes, &back[l.addr as usize..l.addr as usize + l.bytes.len()]);
        }
        if bad.is_empty() {
            assert_eq!(back, an.memory[..0x3d00]);
            return;
        }
        force.extend(bad.iter().map(|l| l.addr));
    }
    panic!("still different after 3 rounds");
}

#[test]
fn takes_apart_a_tape_with_a_basic_loader_and_code() {
    let Some(asm) = sjasm() else { return };
    let (_, bin) = bat(&asm);
    // Like a 1980s game: a BASIC loader, then the code.
    let loader = basic::compile("10 BORDER 0: CLEAR 32767\n20 LOAD \"\" CODE\n30 RANDOMIZE USR 32768").unwrap();
    let mut tap = basic::tap(&loader, "BatBall", 10);
    let mut h = vec![0x00, 0x03];
    h.extend_from_slice(b"batcode   ");
    h.extend_from_slice(&(bin.len() as u16).to_le_bytes());
    h.extend_from_slice(&0x8000u16.to_le_bytes());
    h.extend_from_slice(&0x8000u16.to_le_bytes());
    let mut d = vec![0xff];
    d.extend_from_slice(&bin);
    for mut b in [h, d] {
        let x = b.iter().fold(0u8, |a, v| a ^ v);
        b.push(x);
        tap.extend_from_slice(&(b.len() as u16).to_le_bytes());
        tap.extend(b);
    }
    let an = analyze("batball.tap", &tap, 4).unwrap();
    let names: Vec<String> = an.tape.iter().filter_map(|b| b.header.as_ref().map(|h| format!("{} {} {}", h.kind, h.name, h.param1))).collect();
    assert_eq!(names, ["0 BatBall 10", "3 batcode 32768"]);
    let listing_text = an.basic.clone().unwrap_or_default();
    println!("{listing_text}");
    assert!(listing_text.contains("RANDOMIZE USR 32768") && listing_text.contains("LOAD \"\"CODE") || listing_text.contains("LOAD \"\" CODE"), "{listing_text}");
    assert_eq!(an.entry, Some(0x8000), "the code was entered at USR 32768");
    assert!(an.kinds[0x8000] == Kind::Code);
    let map = memory_map(&an);
    assert_eq!(map[0x80], "code");
    assert_eq!(map[0x00], "rom");
    assert_eq!(map[0x40], "screen");
    // Patch: make the bat wider (BAT_W 5 → 8 is in `cp BAT_W`… find the `ld b,5` of the bat drawing).
    let patched = apply_patches(&an.loaded, &[(0x9000, vec![1, 2, 3])]).unwrap();
    let mut zx = Spectrum::new(Model::Spectrum48);
    zx.load("p.z80", &patched).unwrap();
    assert_eq!((zx.peek(0x9000), zx.peek(0x9002)), (1, 3));
    // Strings: the title.
    assert!(strings(&an.memory, 0x8000, 5).iter().any(|(_, s)| s.contains("BAT & BALL")));
}

#[test]
#[ignore]
fn dump_rom_source() {
    let zx = Spectrum::new(Model::Spectrum48);
    let memory = zx.memory();
    let mut kinds = vec![Kind::Data; 0x10000];
    let mut pc: u32 = 0;
    while pc < 0x3d00 {
        let d = decode(&memory, pc as u16);
        kinds[pc as usize] = Kind::Code;
        pc += d.code.len().max(1) as u32;
    }
    let an = Analysis {
        loaded: Vec::new(), memory, kinds, written: vec![false; 0x10000], entry: None, calls: Default::default(), jumps: Default::default(),
        data_refs: Default::default(), ports: Default::default(), rom_calls: Default::default(), model: Model::Spectrum48, basic: None, tape: Vec::new(), steps: 0, frames: 0,
    };
    let src = source(&an, &Notes::default(), 0, 0x3d00, &BTreeSet::new(), "ROM");
    std::fs::write(std::env::var("RAD_OUT").unwrap(), format!("    DEVICE ZXSPECTRUM48\n{src}")).unwrap();
}

#[test]
#[ignore]
fn show_rom_lines() {
    let zx = Spectrum::new(Model::Spectrum48);
    let memory = zx.memory();
    let mut kinds = vec![Kind::Data; 0x10000];
    let mut pc: u32 = 0;
    while pc < 0x3d00 {
        let d = decode(&memory, pc as u16);
        kinds[pc as usize] = Kind::Code;
        pc += d.code.len().max(1) as u32;
    }
    let an = Analysis {
        loaded: Vec::new(), memory, kinds, written: vec![false; 0x10000], entry: None, calls: Default::default(), jumps: Default::default(),
        data_refs: Default::default(), ports: Default::default(), rom_calls: Default::default(), model: Model::Spectrum48, basic: None, tape: Vec::new(), steps: 0, frames: 0,
    };
    for l in listing(&an, &Notes::default(), 0x248, 0x258, &BTreeSet::new()) {
        let d = decode(&an.memory, l.addr);
        println!("{:04x} {:02x?} {:<20} {} {:?} {:?}", l.addr, l.bytes, l.text, l.kind, d.mnemonic, d.prefix);
    }
}

#[test]
#[ignore]
fn where_does_ram_code_run_while_loading() {
    let loader = basic::compile("10 BORDER 0: CLEAR 32767\n20 LOAD \"\" CODE\n30 RANDOMIZE USR 32768").unwrap();
    let tap = basic::tap(&loader, "x", 10);
    let mut zx = Spectrum::new(Model::Spectrum48);
    zx.load("x.tap", &tap).unwrap();
    for f in 0..2000 {
        let mut first = None;
        zx.run_frame_traced(&mut |d| if d.pc >= 0x4000 && first.is_none() { first = Some((d.pc, d.mnemonic)) });
        zx.skip_audio();
        if let Some(p) = first {
            println!("frame {f}: {:04x} {:?}", p.0, p.1);
            break;
        }
    }
}

#[test]
fn counts_what_the_hardware_does() {
    let Some(asm) = sjasm() else { return };
    let (sna, _) = bat(&asm);
    let mut zx = Spectrum::new(Model::Spectrum48);
    zx.load("bat.sna", &sna).unwrap();
    let mut prev = Vec::new();
    let mut total = FrameStats::default();
    for _ in 0..100 {
        let s = frame_with_stats(&mut zx, &mut prev);
        zx.skip_audio();
        total.fetch_rom += s.fetch_rom;
        total.fetch_ram_high += s.fetch_ram_high;
        total.write_screen += s.write_screen;
        total.in_fe += s.in_fe;
        total.out_fe += s.out_fe;
        total.halts += s.halts;
        total.in_joy += s.in_joy;
        total.instructions += s.instructions;
    }
    println!("{total:?}");
    // The game's code is in upper RAM; the ROM runs the interrupt; it reads keys and draws.
    assert!(total.fetch_ram_high > 1000 && total.fetch_rom > 1000);
    // The bat is read every 2nd frame (keyboard and joystick), the screen changes, one HALT a frame.
    assert!(total.write_screen > 0 && total.in_fe >= 35 && total.in_joy == total.in_fe && total.halts >= 90, "{total:?}");
}

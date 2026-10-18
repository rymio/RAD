use super::*;

fn open() -> Arc<Roms> {
    Arc::new(Roms::open_roms())
}

fn frames(c: &mut C64, n: u32) {
    for _ in 0..n {
        c.run_frame();
        c.skip_audio();
    }
}

/// 10 SYS2061, then: border orange, "S" top left, loop.
fn demo_prg() -> Vec<u8> {
    let mut v = vec![0x01, 0x08, 0x0b, 0x08, 0x0a, 0x00, 0x9e, b'2', b'0', b'6', b'1', 0, 0, 0];
    v.extend([0xa9, 0x08, 0x8d, 0x20, 0xd0, 0xa9, 0x13, 0x8d, 0x00, 0x04, 0x4c, 0x17, 0x08]);
    v
}

fn crt(hw: u16, exrom: bool, game: bool, banks: &[Vec<u8>]) -> Vec<u8> {
    let mut f = b"C64 CARTRIDGE   ".to_vec();
    f.extend(0x40u32.to_be_bytes());
    f.extend([1, 0]);
    f.extend(hw.to_be_bytes());
    f.push(if exrom { 0 } else { 1 });
    f.push(if game { 0 } else { 1 });
    f.resize(0x20, 0);
    let mut name = b"TEST".to_vec();
    name.resize(32, 0);
    f.extend(name);
    for (i, b) in banks.iter().enumerate() {
        f.extend(b"CHIP");
        f.extend((0x10 + b.len() as u32).to_be_bytes());
        f.extend([0, 0]);
        f.extend((i as u16).to_be_bytes());
        f.extend(0x8000u16.to_be_bytes());
        f.extend((b.len() as u16).to_be_bytes());
        f.extend(b);
    }
    f
}

/// An 8K ROML bank that autostarts (CBM80) and runs `code` from $8009.
fn cart_bank(code: &[u8], at_9000: u8) -> Vec<u8> {
    let mut b = vec![0xffu8; 0x2000];
    b[0..4].copy_from_slice(&[0x09, 0x80, 0x09, 0x80]);
    b[4..9].copy_from_slice(&[0xc3, 0xc2, 0xcd, 0x38, 0x30]);
    b[9..9 + code.len()].copy_from_slice(code);
    b[0x1000] = at_9000;
    b
}

#[test]
fn boots_to_basic() {
    let mut c = C64::new(open());
    frames(&mut c, 150);
    let s = c.screen_text();
    assert!(s.iter().any(|r| r == "READY."), "{s:?}");
    assert!(c.roms().is_open_roms());
    assert_eq!(c.size(), (392, 272));
}

#[test]
fn starts_a_program() {
    let mut c = C64::new(open());
    assert_eq!(c.load("demo.prg", &demo_prg()).unwrap(), Loaded::Program { name: "demo.prg".into() });
    assert!(c.is_turbo());
    frames(&mut c, 300);
    assert!(!c.is_starting());
    assert_eq!(c.border_color(), 8);
    assert_eq!(c.screen_text()[0].chars().next(), Some('S'));
    assert_eq!(c.pc(), 0x0817);
}

#[test]
fn loads_from_a_disk() {
    let prg = demo_prg();
    let d = formats::D64::build("my disk", "ab", &[("demo", &prg), ("other", &[0x00, 0xc0, 1, 2, 3])]).unwrap();
    assert_eq!(d.directory().len(), 2);
    assert_eq!(d.read(&d.find(b"DEMO").unwrap()), prg);
    assert_eq!(d.find(b"O*").unwrap().name, "OTHER");
    let listing = basic::detokenize(&d.listing(0x0801));
    assert!(listing.starts_with("0 {RVS ON}\"MY DISK"), "{listing}");
    assert!(listing.contains("\"DEMO\""));

    let mut c = C64::new(open());
    assert!(matches!(c.load("game.d64", &d.data).unwrap(), Loaded::Disk { .. }));
    frames(&mut c, 400);
    assert!(!c.is_starting(), "{:?}", c.screen_text());
    assert_eq!(c.border_color(), 8, "{:?}", c.screen_text());
    // A program loading another file through the KERNAL (multi-load games do this):
    // SETNAM "OTHER", SETLFS 1,8,1, LOAD (to the file's own $C000), then keep the end address.
    let code = [
        0xa9, 5, 0xa2, 0x20, 0xa0, 0xc2, 0x20, 0xbd, 0xff, // SETNAM len 5 at $C220
        0xa9, 1, 0xa2, 8, 0xa0, 1, 0x20, 0xba, 0xff, // SETLFS 1,8,1
        0xa9, 0, 0x20, 0xd5, 0xff, // LOAD
        0x8c, 0x00, 0xc1, // STY $C100 (end address high)
        0x8e, 0x01, 0xc1, // STX $C101
        0x4c, 0x1e, 0xc2, // JMP *
    ];
    let mut c2 = C64::new(open());
    c2.insert_disk("game.d64", &d.data).unwrap();
    frames(&mut c2, 150);
    for (i, b) in code.iter().enumerate() {
        c2.poke(0xc200 + i as u16, *b);
    }
    for (i, b) in b"OTHER".iter().enumerate() {
        c2.poke(0xc220 + i as u16, *b);
    }
    c2.typing.extend(b"SYS49664\r");
    frames(&mut c2, 30);
    assert_eq!(&c2.ram()[0xc000..0xc003], &[1, 2, 3]);
    assert_eq!((c2.peek(0xc100), c2.peek(0xc101)), (0xc0, 0x03));
}

#[test]
fn runs_cartridges() {
    // An 8K cartridge: border blue.
    let code = [0x78, 0xa9, 0x06, 0x8d, 0x20, 0xd0, 0x4c, 0x0f, 0x80];
    let mut c = C64::new(open());
    assert_eq!(c.load("t.crt", &crt(0, true, false, &[cart_bank(&code, 0)])).unwrap(), Loaded::Cartridge { name: "TEST".into() });
    frames(&mut c, 10);
    assert_eq!(c.border_color(), 6);
    assert!(c.has_cartridge());

    // Ocean: switch to bank 1 and show its $9000 byte as the border colour.
    let code = [0x78, 0xa9, 0x01, 0x8d, 0x00, 0xde, 0xad, 0x00, 0x90, 0x8d, 0x20, 0xd0, 0x4c, 0x15, 0x80];
    let mut c = C64::new(open());
    c.load("o.crt", &crt(5, true, false, &[cart_bank(&code, 2), cart_bank(&code, 7)])).unwrap();
    frames(&mut c, 10);
    assert_eq!(c.border_color(), 7);

    // Ultimax and unknown types are turned down.
    assert!(C64::new(open()).load("u.crt", &crt(0, false, true, &[cart_bank(&code, 0)])).is_err());
    assert!(C64::new(open()).load("e.crt", &crt(32, true, false, &[cart_bank(&code, 0)])).is_err());
}

#[test]
fn the_keyboard_types() {
    let mut c = C64::new(open());
    frames(&mut c, 150);
    let a = keys::named("A").unwrap();
    c.set_keys(&[a]);
    frames(&mut c, 3);
    c.set_keys(&[]);
    frames(&mut c, 5);
    let s = c.screen_text();
    let ready = s.iter().position(|r| r == "READY.").unwrap();
    assert!(s[ready + 1].starts_with('A'), "{s:?}");
}

#[test]
fn save_states_come_back() {
    let mut c = C64::new(open());
    c.load("demo.prg", &demo_prg()).unwrap();
    frames(&mut c, 300);
    c.lock(0xc000, Some(42));
    frames(&mut c, 1);
    let st = c.save_state().unwrap();
    assert!(st.len() < 200_000, "{} bytes", st.len());
    let mut d = C64::new(open());
    assert_eq!(d.load("x.c64s", &st).unwrap(), Loaded::Snapshot);
    frames(&mut d, 2);
    assert_eq!(d.border_color(), 8);
    assert_eq!(d.pc(), 0x0817);
    assert_eq!(d.peek(0xc000), 42);
    assert!(d.load("x.c64s", &st[..100]).is_err());
}

#[test]
fn checks_programs() {
    let r = run_program(open(), "demo.prg", &demo_prg(), 1).unwrap();
    assert!(r.started && !r.crashed);
    assert_eq!(r.border, 8);
    assert!(r.screen[0].starts_with('S'));
    // A program that stops on a JAM.
    let mut v = demo_prg();
    v[14] = 0x02;
    let r = run_program(open(), "jam.prg", &v, 1).unwrap();
    assert!(r.crashed);
}

#[test]
fn reads_errors() {
    assert_eq!(parse_error("?SYNTAX  ERROR IN 20"), Some(("SYNTAX ERROR".into(), Some(20))));
    assert_eq!(parse_error("?OUT OF DATA  ERROR"), Some(("OUT OF DATA ERROR".into(), None)));
    assert_eq!(parse_error("BREAK IN 30"), Some(("BREAK".into(), Some(30))));
    assert_eq!(parse_error("READY."), None);
}

#[test]
fn sound_comes_out() {
    let mut c = C64::new(open());
    c.set_sample_rate(48_000);
    frames(&mut c, 50);
    // Voice 1: full volume, a square wave, gate on – then wait (BASIC's READY would silence it).
    let mut code = vec![];
    for (a, v) in [(0xd418u16, 15u8), (0xd400, 0), (0xd401, 0x20), (0xd403, 8), (0xd405, 0), (0xd406, 0xf0), (0xd404, 0x41)] {
        let [lo, hi] = a.to_le_bytes();
        code.extend([0xa9, v, 0x8d, lo, hi]);
    }
    let at = 0xc000 + code.len() as u16;
    code.push(0x4c);
    code.extend(at.to_le_bytes());
    for (i, b) in code.iter().enumerate() {
        c.poke(0xc000 + i as u16, *b);
    }
    c.typing.extend(b"SYS49152\r");
    frames(&mut c, 5);
    let mut out = Vec::new();
    c.run_frame();
    c.audio(&mut out);
    // ~958 stereo pairs per frame at 48 kHz.
    assert!((1800..2100).contains(&out.len()), "{}", out.len());
    let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "silent: {peak}");
}

#[test]
fn loads_from_tape() {
    let prg = Prg::parse("demo", &demo_prg()).unwrap();
    let tap = formats::tap_from_prg("demo", &prg);
    let mut c = C64::new(open());
    assert_eq!(c.load("demo.tap", &tap).unwrap(), Loaded::Tape { names: vec!["demo.tap".into()] });
    let mut n = 0;
    while c.is_starting() && n < 50 * 120 {
        frames(&mut c, 50);
        n += 50;
    }
    println!("{n} frames: {:?}", c.screen_text());
    assert!(!c.is_starting());
    assert_eq!(c.border_color(), 8, "{:?}", c.screen_text());
    assert!(c.tape_state().unwrap().1 > 0.9);
}

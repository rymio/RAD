//! The emulator loading real tapes through the Spectrum ROM's own loader.

use super::*;

/// `10 POKE 30000,123: BORDER 2` as a tokenized BASIC line.
fn program() -> Vec<u8> {
    let num = |text: &str, v: u16| -> Vec<u8> {
        let mut n = text.as_bytes().to_vec();
        n.extend_from_slice(&[0x0e, 0, 0, v as u8, (v >> 8) as u8, 0]);
        n
    };
    let mut body = vec![0xf4]; // POKE
    body.extend(num("30000", 30000));
    body.push(b',');
    body.extend(num("123", 123));
    body.push(b':');
    body.push(0xe7); // BORDER
    body.extend(num("2", 2));
    body.push(0x0d);
    let mut line = vec![0, 10];
    line.extend_from_slice(&(body.len() as u16).to_le_bytes());
    line.extend(body);
    line
}

fn with_checksum(mut b: Vec<u8>) -> Vec<u8> {
    let x = b.iter().fold(0u8, |a, v| a ^ v);
    b.push(x);
    b
}

/// The two ROM blocks: a header that autostarts at line 10, then the program.
fn rom_blocks() -> Vec<Vec<u8>> {
    let p = program();
    let mut h = vec![0x00, 0x00];
    h.extend_from_slice(b"rad test  ");
    h.extend_from_slice(&(p.len() as u16).to_le_bytes());
    h.extend_from_slice(&10u16.to_le_bytes());
    h.extend_from_slice(&(p.len() as u16).to_le_bytes());
    let mut d = vec![0xff];
    d.extend(p);
    vec![with_checksum(h), with_checksum(d)]
}

fn tap() -> Vec<u8> {
    let mut v = Vec::new();
    for b in rom_blocks() {
        v.extend_from_slice(&(b.len() as u16).to_le_bytes());
        v.extend(b);
    }
    v
}

fn tzx(blocks: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
    let mut v = b"ZXTape!\x1a\x01\x14".to_vec();
    for b in blocks {
        v.extend(b);
    }
    v
}

fn w16(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&(x as u16).to_le_bytes());
}
fn w24(v: &mut Vec<u8>, x: usize) {
    v.extend_from_slice(&(x as u32).to_le_bytes()[..3]);
}

/// The standard ROM timings of a block, as plain pulse lengths (pilot, sync, data bits).
fn pulses_of(b: &[u8]) -> Vec<u32> {
    let mut p = vec![2168; if b[0] < 128 { 8063 } else { 3223 }];
    p.extend([667, 735]);
    for byte in b {
        for i in 0..8 {
            let l = if byte & (0x80 >> i) != 0 { 1710 } else { 855 };
            p.extend([l, l]);
        }
    }
    p
}

fn variants() -> Vec<(&'static str, Vec<u8>)> {
    let blocks = rom_blocks();
    let standard = tzx(blocks.iter().map(|b| {
        let mut v = vec![0x10];
        w16(&mut v, 1000);
        w16(&mut v, b.len() as u32);
        v.extend(b);
        v
    }));
    let turbo = tzx(blocks.iter().map(|b| {
        let mut v = vec![0x11];
        for x in [2168, 667, 735, 855, 1710, if b[0] < 128 { 8063 } else { 3223 }] {
            w16(&mut v, x);
        }
        v.push(8);
        w16(&mut v, 1000);
        w24(&mut v, b.len());
        v.extend(b);
        v
    }));
    let pieces = tzx(blocks.iter().map(|b| {
        let mut v = vec![0x12];
        w16(&mut v, 2168);
        w16(&mut v, if b[0] < 128 { 8063 } else { 3223 });
        v.extend([0x13, 2]);
        w16(&mut v, 667);
        w16(&mut v, 735);
        v.push(0x14);
        w16(&mut v, 855);
        w16(&mut v, 1710);
        v.push(8);
        w16(&mut v, 1000);
        w24(&mut v, b.len());
        v.extend(b);
        v
    }));
    let generalized = tzx(blocks.iter().map(|b| {
        let mut body = Vec::new();
        w16(&mut body, 1000);
        body.extend_from_slice(&2u32.to_le_bytes()); // TOTP
        body.extend([2, 2]); // NPP, ASP
        body.extend_from_slice(&((b.len() * 8) as u32).to_le_bytes()); // TOTD
        body.extend([2, 2]); // NPD, ASD
        body.push(0);
        w16(&mut body, 2168);
        w16(&mut body, 0);
        body.push(0);
        w16(&mut body, 667);
        w16(&mut body, 735);
        body.push(0);
        w16(&mut body, if b[0] < 128 { 8063 } else { 3223 });
        body.push(1);
        w16(&mut body, 1);
        body.push(0);
        w16(&mut body, 855);
        w16(&mut body, 855);
        body.push(0);
        w16(&mut body, 1710);
        w16(&mut body, 1710);
        body.extend(b);
        let mut v = vec![0x19];
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend(body);
        v
    }));
    let direct = tzx(blocks.iter().map(|b| {
        const TPS: u32 = 50;
        let mut bits = Vec::new();
        let mut level = false;
        for p in pulses_of(b) {
            level = !level;
            for _ in 0..(p / TPS) {
                bits.push(level);
            }
        }
        let mut data = vec![0u8; bits.len().div_ceil(8)];
        for (i, bit) in bits.iter().enumerate() {
            if *bit {
                data[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let mut v = vec![0x15];
        w16(&mut v, TPS);
        w16(&mut v, 1000);
        v.push(if bits.len() % 8 == 0 { 8 } else { (bits.len() % 8) as u8 });
        w24(&mut v, data.len());
        v.extend(data);
        v
    }));
    let csw = tzx(blocks.iter().map(|b| {
        let mut body = Vec::new();
        w16(&mut body, 1000);
        body.extend([0xe0, 0x67, 0x35]); // 3.5 MHz
        body.push(1);
        let p = pulses_of(b);
        body.extend_from_slice(&(p.len() as u32).to_le_bytes());
        for l in p {
            body.push(0);
            body.extend_from_slice(&l.to_le_bytes());
        }
        let mut v = vec![0x18];
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend(body);
        v
    }));
    vec![
        ("tap", tap()),
        ("standard.tzx", standard),
        ("turbo.tzx", turbo),
        ("pieces.tzx", pieces),
        ("generalized.tzx", generalized),
        ("direct.tzx", direct),
        ("csw.tzx", csw),
    ]
}

/// Runs frames until the program has run (or gives up): the frames it took.
fn load_and_run(model: Model, name: &str, data: &[u8]) -> Option<usize> {
    let mut zx = Spectrum::new(model);
    let loaded = zx.load(name, data).unwrap();
    // Names show for blocks that keep their bytes (not generalized, direct or CSW recordings).
    let readable = !["generalized", "direct", "csw"].iter().any(|k| name.starts_with(k));
    assert!(matches!(&loaded, Loaded::Tape { names } if !readable || names.iter().any(|n| n == "Program: rad test")), "{loaded:?}");
    for frame in 0..4000 {
        zx.run_frame();
        zx.skip_audio();
        if zx.peek(30000) == 123 && zx.border_color() == 2 {
            return Some(frame);
        }
    }
    None
}

#[test]
fn every_tape_kind_loads_on_48k_and_128k() {
    for model in [Model::Spectrum48, Model::Spectrum128] {
        for (name, data) in variants() {
            let file = if name == "tap" { "test.tap".to_string() } else { name.to_string() };
            let frames = load_and_run(model, &file, &data);
            println!("{model:?} {file}: {frames:?} frames");
            assert!(frames.is_some(), "{model:?} {file} didn't load");
        }
    }
}

#[test]
fn picture_and_sound() {
    let mut zx = Spectrum::new(Model::Spectrum48);
    let (w, h) = zx.size();
    let mut px = vec![0u8; w * h];
    for _ in 0..100 {
        zx.run_frame();
        zx.skip_audio();
    }
    zx.run_frame();
    zx.render(&mut px);
    // The 48K start-up screen: white paper, black copyright text in the bottom line.
    assert_eq!(px[w * 50 + 60], 7, "white paper");
    assert!(px.iter().any(|p| *p == 0), "some black text");
    let mut s = Vec::new();
    zx.audio(&mut s, false);
    assert!(s.len() >= 2 * 800 && s.len() <= 2 * 1000, "about 882 stereo samples per frame at 44.1 kHz: {}", s.len());
}

#[test]
fn snapshots_round_trip_through_z80() {
    // Load a program from tape, save a Z80 snapshot with SPECTRUSTY's writer, load it back.
    let mut zx = Spectrum::new(Model::Spectrum48);
    let data = tap();
    zx.load("t.tap", &data).unwrap();
    for _ in 0..4000 {
        zx.run_frame();
        zx.skip_audio();
        if zx.peek(30000) == 123 {
            break;
        }
    }
    let snap = zx.save_state().unwrap();
    let mut zx2 = Spectrum::new(Model::Spectrum128);
    assert_eq!(zx2.load("game.z80", &snap).unwrap(), Loaded::Snapshot);
    assert_eq!(zx2.model(), Model::Spectrum48, "the snapshot's model wins");
    assert_eq!(zx2.peek(30000), 123);
    assert_eq!(zx2.border_color(), 2);
    assert!(zx2.load("x.bin", b"").is_err());
}

#[test]
fn picture_size() {
    let zx = Spectrum::new(Model::Spectrum48);
    assert_eq!(zx.size(), (352, 288), "the screen with the full border");
}

#[test]
fn runs_faster_than_real_time() {
    // 500 frames are 10 s of Spectrum time; even a dev build must manage that well under 10 s.
    let mut zx = Spectrum::new(Model::Spectrum128);
    let (w, h) = zx.size();
    let mut px = vec![0u8; w * h];
    let mut s = Vec::new();
    let t = std::time::Instant::now();
    for _ in 0..500 {
        zx.run_frame();
        zx.audio(&mut s, true);
        zx.render(&mut px);
    }
    let took = t.elapsed();
    println!("500 frames (10 s of Spectrum time) in {took:?}");
    assert!(took < std::time::Duration::from_secs(5), "{took:?}");
}

/// Writes frames as BMP images to look at: RAD_ZX_SHOTS=/some/dir cargo test shots -- --ignored
#[test]
#[ignore]
fn shots() {
    let dir = std::path::PathBuf::from(std::env::var("RAD_ZX_SHOTS").expect("RAD_ZX_SHOTS"));
    let bmp = |zx: &mut Spectrum, name: &str| {
        let (w, h) = zx.size();
        let mut px = vec![0u8; w * h];
        zx.render(&mut px);
        let mut f = Vec::new();
        let size = 54 + w * h * 3;
        f.extend_from_slice(b"BM");
        f.extend_from_slice(&(size as u32).to_le_bytes());
        f.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
        f.extend_from_slice(&(w as i32).to_le_bytes());
        f.extend_from_slice(&(-(h as i32)).to_le_bytes());
        f.extend_from_slice(&[1, 0, 24, 0, 0, 0, 0, 0]);
        f.extend_from_slice(&((w * h * 3) as u32).to_le_bytes());
        f.extend_from_slice(&[0; 16]);
        for p in px {
            let [r, g, b] = PALETTE[p as usize];
            f.extend_from_slice(&[b, g, r]);
        }
        std::fs::write(dir.join(name), f).unwrap();
    };
    let mut zx = Spectrum::new(Model::Spectrum48);
    for _ in 0..120 {
        zx.run_frame();
        zx.skip_audio();
    }
    bmp(&mut zx, "48k-boot.bmp");
    let mut zx = Spectrum::new(Model::Spectrum128);
    zx.load("t.tzx", &variants()[2].1).unwrap();
    for _ in 0..700 {
        zx.run_frame();
        zx.skip_audio();
    }
    bmp(&mut zx, "128k-after-load.bmp");
}

#[test]
fn fast_load_kicks_in_while_the_tape_loads() {
    let mut zx = Spectrum::new(Model::Spectrum48);
    zx.load("t.tap", &tap()).unwrap();
    let (mut turbo, mut frames) = (0, 0);
    while zx.peek(30000) != 123 && frames < 4000 {
        zx.run_frame();
        zx.skip_audio();
        frames += 1;
        if zx.is_turbo() {
            turbo += 1;
        }
    }
    println!("{turbo} of {frames} frames fast-loaded");
    assert!(turbo * 2 > frames, "most of the loading runs at top speed");
    assert!(!zx.is_turbo(), "back to normal speed once loaded");
    // The tape plays out its last pause (a second of silence), then stops by itself.
    for _ in 0..80 {
        zx.run_frame();
        zx.skip_audio();
    }
    let (playing, progress, _) = zx.tape_state().unwrap();
    assert!(!playing && progress > 0.95, "{playing} {progress}");
}

#[test]
fn save_states_keep_a_128k_running() {
    let mut zx = Spectrum::new(Model::Spectrum128);
    zx.load("t.tap", &tap()).unwrap();
    while zx.peek(30000) != 123 {
        zx.run_frame();
        zx.skip_audio();
    }
    let snap = zx.save_state().unwrap();
    let mut back = Spectrum::new(Model::Spectrum48);
    back.load("state.z80", &snap).unwrap();
    assert_eq!(back.model(), Model::Spectrum128);
    assert_eq!((back.peek(30000), back.border_color()), (123, 2));
    // It carries on running from where it was.
    let (w, h) = back.size();
    let (mut a, mut b) = (vec![0u8; w * h], vec![0u8; w * h]);
    for _ in 0..5 {
        zx.run_frame();
        zx.skip_audio();
        back.run_frame();
        back.skip_audio();
    }
    zx.render(&mut a);
    back.render(&mut b);
    assert_eq!(a, b, "the restored machine shows the same screen");
}

#[test]
fn basic_programs_run_and_report() {
    let ok = basic::compile("10 CLS\n20 PRINT AT 5,3;\"HELLO FROM RAD\"\n30 FOR i=1 TO 3: PRINT i: NEXT i").unwrap();
    let r = run_basic(&basic::tap(&ok, "hello", 10), 2).unwrap();
    println!("{r:#?}");
    assert!(r.started);
    assert!(r.screen.iter().any(|l| l.contains("HELLO FROM RAD")), "{:?}", r.screen);
    assert_eq!(r.report.as_ref().map(|r| (r.0.as_str(), r.2)), Some(("0", 30)), "it ends OK at line 30");

    // Errors: a variable that doesn't exist, and nonsense.
    let bad = basic::compile("10 PRINT \"START\"\n20 PRINT nothere\n30 PRINT \"END\"").unwrap();
    let r = run_basic(&basic::tap(&bad, "bad", 10), 2).unwrap();
    assert_eq!(r.report.as_ref().map(|r| (r.0.as_str(), r.1.as_str(), r.2, r.3)), Some(("2", "Variable not found", 20, 1)), "{r:?}");
    let nonsense = basic::compile("10 PRINT 1+\n").unwrap();
    let r = run_basic(&basic::tap(&nonsense, "bad", 10), 2).unwrap();
    assert_eq!(r.report.as_ref().map(|r| r.0.as_str()), Some("C"), "{r:?}");

    // A game loop keeps running: no report.
    let looping = basic::compile("10 PRINT AT 0,0;\"SCORE\"\n20 GO TO 20").unwrap();
    let r = run_basic(&basic::tap(&looping, "loop", 10), 2).unwrap();
    assert_eq!(r.report, None);
    assert_eq!(r.screen[0], "SCORE");
}

/// Runs a file and writes its screen as a BMP: RAD_ZX_FILE=game.sna RAD_ZX_OUT=shot.bmp cargo test render_file -- --ignored
#[test]
#[ignore]
fn render_file() {
    let file = std::env::var("RAD_ZX_FILE").expect("RAD_ZX_FILE");
    let out = std::env::var("RAD_ZX_OUT").expect("RAD_ZX_OUT");
    let mut zx = Spectrum::new(Model::Spectrum48);
    zx.load(&file, &std::fs::read(&file).unwrap()).unwrap();
    for _ in 0..std::env::var("RAD_ZX_FRAMES").ok().and_then(|f| f.parse().ok()).unwrap_or(100) {
        zx.run_frame();
        zx.skip_audio();
    }
    let (w, h) = zx.size();
    let mut px = vec![0u8; w * h];
    zx.render(&mut px);
    let mut f = Vec::new();
    f.extend_from_slice(b"BM");
    f.extend_from_slice(&((54 + w * h * 3) as u32).to_le_bytes());
    f.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
    f.extend_from_slice(&(w as i32).to_le_bytes());
    f.extend_from_slice(&(-(h as i32)).to_le_bytes());
    f.extend_from_slice(&[1, 0, 24, 0, 0, 0, 0, 0]);
    f.extend_from_slice(&((w * h * 3) as u32).to_le_bytes());
    f.extend_from_slice(&[0; 16]);
    for p in px {
        let [r, g, b] = PALETTE[p as usize];
        f.extend_from_slice(&[b, g, r]);
    }
    std::fs::write(out, f).unwrap();
}

#[test]
fn spots_a_crash() {
    // A snapshot that jumps to address 0: the Spectrum resets to its start-up screen.
    let mut zx = Spectrum::new(Model::Spectrum48);
    for _ in 0..100 {
        zx.run_frame();
        zx.skip_audio();
    }
    let boot = zx.screen_text();
    let r = BasicRun { started: true, report: None, screen: boot };
    assert!(r.crashed());
}

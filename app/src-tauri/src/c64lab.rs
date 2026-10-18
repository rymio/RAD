//! The C64 lab's commands: analysing a game (crates/rad-c64 lab.rs), its listing and source,
//! memory, assembling patches with 64tass and building the patched game. Like the Spectrum lab
//! (zxlab.rs), an analysis is kept in memory per lab project while RAD runs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use rad_c64::lab::{self, Analysis, Notes};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

use crate::c64::C64State;
use crate::client::{CmdError, CmdResult};

#[derive(Default)]
pub struct C64Lab {
    cache: Mutex<HashMap<String, Arc<Analysis>>>,
}

fn b64(d: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(d)
}

fn unb64(s: &str) -> CmdResult<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).map_err(|_| CmdError::new("bad_request", "damaged data"))
}

fn get(l: &C64Lab, id: &str) -> CmdResult<Arc<Analysis>> {
    l.cache.lock().unwrap().get(id).cloned().ok_or_else(|| CmdError::new("lab", "Open the project first"))
}

fn err(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("lab", e.to_string())
}

/// A short report of what was found (for the overview and the AI).
fn summary(an: &Analysis, roms_open: bool) -> Value {
    let (from, to) = lab::program_range(an);
    let rom_calls: Vec<Value> = an
        .rom_calls
        .iter()
        .map(|(a, n)| {
            let k = lab::KERNAL.iter().find(|k| k.0 == *a);
            json!({"addr": a, "count": n, "name": k.map(|k| k.1), "what": k.map(|k| k.2)})
        })
        .collect();
    let user = |a: usize| a >= 0x0200 && !(0xd000..0xe000).contains(&a);
    let code_bytes = an.kinds.iter().enumerate().filter(|(a, k)| user(*a) && matches!(k, lab::Kind::Code | lab::Kind::CodeTail)).count();
    let reach_bytes = an.kinds.iter().enumerate().filter(|(a, k)| user(*a) && matches!(k, lab::Kind::Reachable | lab::Kind::ReachableTail)).count();
    // Chip registers, by chip: reads and writes from the game's own code.
    let hardware: Vec<Value> = an
        .io
        .iter()
        .map(|(a, (r, w))| {
            let reg = lab::REGS.iter().find(|x| x.0 == *a);
            json!({"addr": a, "chip": lab::chip_of(*a), "name": reg.map(|x| x.1), "what": reg.map(|x| x.2), "reads": r, "writes": w})
        })
        .collect();
    let mut ports = BTreeMap::new();
    for (a, (r, w)) in &an.io {
        if let Some(c) = lab::chip_of(*a) {
            *ports.entry(c.to_string()).or_insert(0u64) += r + w;
        }
    }
    let d018 = an.vic.get(0x18).copied().unwrap_or(0x15);
    let screen = an.vic_bank + (d018 as u16 >> 4) * 0x400;
    let charset = an.vic_bank + ((d018 as u16 >> 1) & 7) * 0x800;
    let char_rom = matches!(an.vic_bank, 0x0000 | 0x8000) && matches!(charset & 0x3fff, 0x1000 | 0x1800);
    let bitmap = (an.vic.get(0x11).copied().unwrap_or(0) & 0x20 != 0).then_some(an.vic_bank + (d018 as u16 & 8) * 0x400);
    json!({
        "model": "C64",
        "entry": an.entry,
        "range": [from, to],
        "functions": an.calls.len(),
        "function_addrs": an.calls.iter().collect::<Vec<_>>(),
        "code_bytes": code_bytes,
        "reachable_bytes": reach_bytes,
        "variables": an.written.iter().enumerate().filter(|(a, w)| user(*a) && **w).count(),
        "ports": ports,
        "hardware": hardware,
        "rom_calls": rom_calls,
        "basic": an.basic,
        "tape": [],
        "files": an.files,
        "map": lab::memory_map(an),
        "strings": lab::strings(&an.memory, 0x0200, 5).into_iter().take(400).map(|(a, s)| json!({"addr": a, "text": s})).collect::<Vec<_>>(),
        "steps": an.steps,
        "seconds": an.frames / 50,
        "udg": charset,
        "screen_addr": screen,
        "charset": {"addr": charset, "rom": char_rom},
        "bitmap": bitmap,
        "sprites": an.sprites.iter().map(|(n, a)| json!({"n": n, "addr": a, "multicolor": an.vic.get(0x1c).is_some_and(|m| m & (1 << n) != 0)})).collect::<Vec<_>>(),
        "pictures": an.pictures.iter().map(|(t, p)| json!({"title": t, "data": b64(p)})).collect::<Vec<_>>(),
        "cartridge": an.cartridge,
        "roms": if roms_open { "open" } else { "commodore" },
    })
}

/// Analyses a game (`data` base64) for lab project `id`: loads it, lets it start, runs it for
/// `seconds` while tracing, and returns the overview.
#[tauri::command]
pub async fn c64lab_open(l: State<'_, C64Lab>, c: State<'_, C64State>, id: String, name: String, data: String, seconds: Option<u32>) -> CmdResult<Value> {
    let bytes = unb64(&data)?;
    let roms = c.roms();
    let open = roms.is_open_roms();
    let an = tauri::async_runtime::spawn_blocking(move || lab::analyze(roms, &name, &bytes, seconds.unwrap_or(15).clamp(2, 120)))
        .await
        .map_err(err)?
        .map_err(err)?;
    let s = summary(&an, open);
    l.cache.lock().unwrap().insert(id, Arc::new(an));
    Ok(s)
}

fn parse_notes(v: Option<Value>) -> Notes {
    v.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

#[tauri::command]
pub fn c64lab_listing(l: State<'_, C64Lab>, id: String, from: u16, to: u32, notes: Option<Value>) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let to = to.clamp(from as u32 + 1, 0x10000).min(from as u32 + 0x4000);
    Ok(serde_json::to_value(lab::listing(&an, &parse_notes(notes), from, to, &BTreeSet::new()))?)
}

/// Where an address is used: lines anywhere in RAM that jump to, call or point at it.
#[tauri::command]
pub fn c64lab_xrefs(l: State<'_, C64Lab>, id: String, addr: u16, notes: Option<Value>) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let notes = parse_notes(notes);
    let mut refs = Vec::new();
    for (from, to) in [(0x0200u16, 0xd000u32), (0xe000, 0x10000)] {
        for x in lab::listing(&an, &notes, from, to, &BTreeSet::new()) {
            if x.target == Some(addr) && matches!(x.kind, "code" | "reachable") {
                refs.push(json!({"addr": x.addr, "text": x.text}));
            }
        }
    }
    Ok(json!(refs))
}

#[tauri::command]
pub fn c64lab_memory(l: State<'_, C64Lab>, id: String, from: u16, len: u32) -> CmdResult<String> {
    let an = get(&l, &id)?;
    let end = (from as usize + len as usize).min(0x10000);
    Ok(b64(&an.memory[from as usize..end]))
}

/// Assembles `src` with 64tass as a plain binary: (bytes from its first address, labels, output).
fn assemble(asm: &Path, root: &Path, src: &str) -> Result<(Vec<u8>, BTreeMap<String, u32>, String), String> {
    let dir = root.join(crate::zxdev::unique_dir("lab"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let r = (|| {
        std::fs::write(dir.join("src.asm"), src).map_err(|e| e.to_string())?;
        let out = crate::helper_command(asm).args(["-q", "-b", "-o", "out.bin", "-l", "labels.txt", "src.asm"]).current_dir(&dir).output().map_err(|e| e.to_string())?;
        let msgs = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if !out.status.success() {
            return Err(msgs);
        }
        let labels = std::fs::read_to_string(dir.join("labels.txt")).unwrap_or_default();
        let labels = labels
            .lines()
            .filter_map(|l| {
                let (n, v) = l.split_once('=')?;
                Some((n.trim().to_string(), u32::from_str_radix(v.trim().trim_start_matches('$'), 16).ok()?))
            })
            .collect();
        Ok((std::fs::read(dir.join("out.bin")).map_err(|e| e.to_string())?, labels, msgs))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    r
}

fn lab_root(app: &AppHandle) -> std::path::PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("c64build")
}

/// Source that assembles back to the same bytes: lines that don't are written as bytes.
fn verified_source(asm: Option<&Path>, root: &Path, an: &Analysis, notes: &Notes, from: u16, to: u32, title: &str) -> Value {
    let mut force = BTreeSet::new();
    for _ in 0..4 {
        let src = lab::source(an, notes, from, to, &force, title);
        let Some(asm) = asm else { return json!({"source": src, "verified": false, "note": "the assembler isn't built, so this wasn't checked"}) };
        match assemble(asm, root, &src) {
            Ok((bytes, _, _)) => {
                let want = &an.memory[from as usize..to as usize];
                if bytes == want {
                    return json!({"source": src, "verified": true});
                }
                let lines = lab::listing(an, notes, from, to, &force);
                let bad: Vec<u16> = lines
                    .iter()
                    .filter(|x| {
                        let o = (x.addr - from) as usize;
                        matches!(x.kind, "code" | "reachable") && bytes.get(o..o + x.bytes.len()) != Some(&x.bytes[..])
                    })
                    .map(|x| x.addr)
                    .collect();
                if bad.is_empty() {
                    return json!({"source": src, "verified": false, "note": "it assembles, but not to exactly the same bytes"});
                }
                force.extend(bad);
            }
            Err(msgs) => return json!({"source": src, "verified": false, "note": msgs}),
        }
    }
    json!({"source": lab::source(an, notes, from, to, &force, title), "verified": false})
}

#[tauri::command]
pub async fn c64lab_source(app: AppHandle, l: State<'_, C64Lab>, id: String, from: u16, to: u32, notes: Option<Value>, title: String) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let notes = parse_notes(notes);
    let asm = crate::c64dev::tass(&app);
    let root = lab_root(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let to = to.clamp(from as u32 + 1, 0x10000);
        Ok(verified_source(asm.as_deref(), &root, &an, &notes, from, to, &title))
    })
    .await
    .map_err(err)?
}

/// Assembles a patch: `code` placed at `addr`, with every name the project knows available.
/// {ok, bytes (hex), end, len, splits (an instruction it cuts in half, if any), messages}.
#[tauri::command]
pub async fn c64lab_assemble_patch(app: AppHandle, l: State<'_, C64Lab>, id: String, addr: u16, code: String, notes: Option<Value>) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let notes = parse_notes(notes);
    let asm = crate::c64dev::tass(&app).ok_or_else(|| CmdError::new("no_assembler", "The 6502 assembler isn't built yet. Run ./scripts/build-c64-tools.sh."))?;
    let root = lab_root(&app);
    tauri::async_runtime::spawn_blocking(move || assemble_patch(&asm, &root, &an, &notes, addr, &code)).await.map_err(err)?
}

fn assemble_patch(asm: &Path, root: &Path, an: &Analysis, notes: &Notes, addr: u16, code: &str) -> CmdResult<Value> {
    // Every name of the program, so the patch can say "jsr draw_sprite".
    let (from, to) = lab::program_range(an);
    let mut names = BTreeMap::new();
    for line in lab::listing(an, notes, from, to, &BTreeSet::new()) {
        if let Some(n) = line.label {
            names.insert(n, line.addr);
        }
        if let Some(t) = line.target {
            if let Some(n) = lab::name_of(an, notes, t) {
                names.entry(n).or_insert(t);
            }
        }
    }
    for (a, n) in &notes.labels {
        names.insert(lab::clean_label(n), *a);
    }
    let defines = |n: &str| code.lines().any(|l| l.split(|c: char| c.is_whitespace() || c == ':' || c == '=').next() == Some(n));
    let mut src = String::new();
    for (n, a) in &names {
        if !defines(n) {
            src.push_str(&format!("{n:<16} = ${a:04x}\n"));
        }
    }
    src.push_str(&format!("\n        * = ${addr:04x}\n{code}\nradpatchend\n"));
    let (bytes, labels, out) = match assemble(asm, root, &src) {
        Ok(r) => r,
        Err(m) => return Ok(json!({"ok": false, "messages": m.replace("src.asm", "patch")})),
    };
    let end = *labels.get("radpatchend").ok_or_else(|| CmdError::new("lab", "the assembler didn't say where the patch ends"))?;
    let len = end.saturating_sub(addr as u32) as usize;
    if len == 0 || bytes.is_empty() {
        return Ok(json!({"ok": false, "messages": "the patch is empty"}));
    }
    if len > 4096 || end > 0x10000 {
        return Ok(json!({"ok": false, "messages": "the patch is too long (or wraps past $FFFF)"}));
    }
    let bytes = bytes[..len.min(bytes.len())].to_vec();
    // Does it end inside an instruction of the original? Then that one is cut in half.
    let mut splits = None;
    if end < 0x10000 && matches!(an.kinds[end as usize], lab::Kind::CodeTail | lab::Kind::ReachableTail) {
        let mut start = end;
        while start > 0 && matches!(an.kinds[start as usize], lab::Kind::CodeTail | lab::Kind::ReachableTail) {
            start -= 1;
        }
        splits = Some(start);
    }
    Ok(json!({"ok": true, "bytes": bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(), "end": end, "len": len, "splits": splits, "messages": out.trim()}))
}

#[derive(Deserialize)]
pub struct Patch {
    addr: u16,
    /// Hex bytes.
    bytes: String,
}

fn hex_bytes(h: &str) -> CmdResult<Vec<u8>> {
    let h: String = h.chars().filter(|c| !c.is_whitespace()).collect();
    if h.len() % 2 != 0 {
        return Err(CmdError::new("bad_request", "patch bytes must be pairs of hex digits"));
    }
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| CmdError::new("bad_request", "patch bytes must be hex"))).collect()
}

/// The game with patches applied: a save state (base64) to play — and, when `check`, what it
/// did in its first seconds on a hidden C64 (screen text, a crash).
#[tauri::command]
pub async fn c64lab_build(l: State<'_, C64Lab>, c: State<'_, C64State>, id: String, patches: Vec<Patch>, check: Option<bool>) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let roms = c.roms();
    let p: Vec<(u16, Vec<u8>)> = patches.iter().map(|p| Ok((p.addr, hex_bytes(&p.bytes)?))).collect::<CmdResult<_>>()?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = lab::apply_patches(roms.clone(), &an.loaded, &p).map_err(err)?;
        let mut out = json!({"file": {"name": "patched.c64s", "data": b64(&state)}});
        if check.unwrap_or(false) {
            let r = rad_c64::run_program(roms, "patched.c64s", &state, 3).map_err(err)?;
            out["check"] = json!({"crashed": r.crashed, "screen": r.screen, "error": r.error.map(|(m, l)| json!({"message": m, "line": l}))});
        }
        Ok(out)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn c64lab_close(l: State<'_, C64Lab>, id: String) {
    l.cache.lock().unwrap().remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAR_CATCH: &str = include_str!("../../ui/src/lib/c64-templates/star-catch.asm");

    fn tass() -> Option<std::path::PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../c64tools/bin").join(if cfg!(windows) { "64tass.exe" } else { "64tass" });
        p.is_file().then_some(p)
    }

    #[test]
    fn take_a_game_apart_patch_it_and_play_it() {
        let Some(asm) = tass() else { return eprintln!("skipped: ./scripts/build-c64-tools.sh") };
        let td = tempfile::tempdir().unwrap();
        // Build Star Catch and take it apart.
        std::fs::write(td.path().join("main.asm"), STAR_CATCH).unwrap();
        assert!(std::process::Command::new(&asm).args(["-q", "--cbm-prg", "-o", "g.prg", "main.asm"]).current_dir(td.path()).status().unwrap().success());
        let prg = std::fs::read(td.path().join("g.prg")).unwrap();
        let roms = Arc::new(rad_c64::Roms::open_roms());
        let an = lab::analyze(roms.clone(), "g.prg", &prg, 4).unwrap();
        assert_eq!(an.entry, Some(2061), "the SYS address");
        let s = summary(&an, true);
        println!("range {} entry {} io {:?}", s["range"], s["entry"], an.io.keys().map(|a| format!("{a:04x}")).collect::<Vec<_>>());
        assert!(an.io.contains_key(&0xd012), "it reads the raster line");
        assert!(an.io.contains_key(&0xd020), "it set the border (while starting up)");
        assert!(an.io.contains_key(&0xdc00), "it reads the joystick");
        assert!(s["strings"].as_array().unwrap().iter().any(|x| x["text"].as_str().unwrap().contains("STAR CATCH")), "{}", s["strings"]);
        let notes = Notes::default();
        let (from, to) = lab::program_range(&an);
        assert_eq!(from, 0x0801);
        let lines = lab::listing(&an, &notes, from, to, &BTreeSet::new());
        let text: Vec<String> = lines.iter().map(|l| format!("{:04x} {} {}", l.addr, l.label.clone().unwrap_or_default(), l.text)).collect();
        assert!(lines.iter().any(|l| l.text == "lda  VIC_RASTER"), "{}", text.join("\n"));
        assert!(lines.iter().any(|l| l.text.starts_with("jsr  sub_")), "functions get names");

        // The source assembles back to exactly the same program.
        let v = verified_source(Some(&asm), td.path(), &an, &notes, from, to, "Star Catch");
        assert_eq!(v["verified"], true, "{}\n{}", v["note"], v["source"]);

        // Patch: the game sets a black border (lda #0 / sta $d020 at the start); make it red.
        let k = lines.iter().position(|l| l.text == "sta  BORDER").unwrap();
        let at = lines[k - 1].addr;
        assert_eq!(lines[k - 1].text, "lda  #$00", "{}", text.join("\n"));
        let p = assemble_patch(&asm, td.path(), &an, &notes, at, "        lda #2\n").unwrap();
        assert_eq!(p["ok"], true, "{p}");
        assert_eq!(p["bytes"], "a902");
        assert_eq!(p["splits"], Value::Null);
        // Names resolve; a patch that's too long cuts the next instruction.
        let p2 = assemble_patch(&asm, td.path(), &an, &notes, at, "        lda #2\n        sta BORDER\n        nop\n").unwrap();
        assert_eq!(p2["ok"], true, "{p2}");
        assert!(p2["splits"].is_u64(), "{p2}");
        let bad = assemble_patch(&asm, td.path(), &an, &notes, at, "        lda #\n").unwrap();
        assert_eq!(bad["ok"], false);
        // Patch a line in the main loop's path: the title is coloured white (lda #1 / sta
        // COLOR_RAM,x in `status`, which runs for every star caught or missed): make it red.
        let k = lines.iter().position(|l| l.text == "sta  COLOR_RAM,x").unwrap();
        assert_eq!(lines[k - 1].text, "lda  #$01", "{}", text.join("\n"));
        let at = lines[k - 1].addr;
        let state = lab::apply_patches(roms.clone(), &an.loaded, &[(at, hex_bytes("a902").unwrap())]).unwrap();
        let mut c = rad_c64::C64::new(roms);
        c.load("p.c64s", &state).unwrap();
        let mut red = false;
        for _ in 0..50 * 20 {
            c.run_frame();
            c.skip_audio();
            if c.peek(0xd800) & 15 == 2 {
                red = true;
                break;
            }
        }
        assert!(red, "the patched game coloured its title red");
    }
}

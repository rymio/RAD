//! The Spectrum lab's commands: analysing a game (crates/rad-zx lab.rs), its listing and
//! source, memory and tape blocks, assembling patches and building the patched game.
//!
//! An analysis is kept in memory per lab project while RAD runs (redoing it takes a moment:
//! the game's file is in the encrypted Games database, with the notes and patches).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use rad_zx::lab::{self, Analysis, Notes};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

use crate::client::{CmdError, CmdResult};

#[derive(Default)]
pub struct ZxLab {
    cache: Mutex<HashMap<String, Arc<Analysis>>>,
}

fn b64(d: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(d)
}

fn unb64(s: &str) -> CmdResult<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).map_err(|_| CmdError::new("bad_request", "damaged data"))
}

fn get(l: &ZxLab, id: &str) -> CmdResult<Arc<Analysis>> {
    l.cache.lock().unwrap().get(id).cloned().ok_or_else(|| CmdError::new("lab", "Open the project first"))
}

fn err(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("lab", e.to_string())
}

/// A short report of what was found (for the overview and the AI).
fn summary(an: &Analysis) -> Value {
    let (from, to) = lab::program_range(an);
    let rom_calls: Vec<Value> = an
        .rom_calls
        .iter()
        .map(|(a, n)| {
            let r = lab::ROM.iter().find(|r| r.0 == *a);
            json!({"addr": a, "count": n, "name": r.map(|r| r.1), "what": r.map(|r| r.2)})
        })
        .collect();
    let code_bytes = an.kinds.iter().enumerate().filter(|(a, k)| *a >= 0x4000 && matches!(k, lab::Kind::Code | lab::Kind::CodeTail)).count();
    let reach_bytes = an.kinds.iter().enumerate().filter(|(a, k)| *a >= 0x4000 && matches!(k, lab::Kind::Reachable | lab::Kind::ReachableTail)).count();
    let tape: Vec<Value> = an
        .tape
        .iter()
        .map(|b| {
            json!({
                "index": b.index, "speed": b.speed, "flag": b.flag, "length": b.data.len(),
                "header": b.header.as_ref().map(|h| json!({"kind": h.kind, "name": h.name, "length": h.length, "param1": h.param1, "param2": h.param2})),
                // A loading screen: 6912 bytes (pixels + colours).
                "screen": b.data.len() == 6912,
            })
        })
        .collect();
    json!({
        "model": if an.model == rad_zx::Model::Spectrum48 { "48" } else { "128" },
        "entry": an.entry,
        "range": [from, to],
        "functions": an.calls.iter().filter(|a| **a >= 0x4000).count(),
        "function_addrs": an.calls.iter().filter(|a| **a >= 0x4000).collect::<Vec<_>>(),
        "code_bytes": code_bytes,
        "reachable_bytes": reach_bytes,
        "variables": an.written.iter().enumerate().filter(|(a, w)| *a >= 0x5b00 && **w).count(),
        "ports": an.ports,
        "rom_calls": rom_calls,
        "basic": an.basic,
        "tape": tape,
        "map": lab::memory_map(an),
        "strings": lab::strings(&an.memory, 0x5b00, 5).into_iter().take(400).map(|(a, s)| json!({"addr": a, "text": s})).collect::<Vec<_>>(),
        "steps": an.steps,
        "seconds": an.frames / 50,
        "udg": u16::from_le_bytes([an.memory[23675], an.memory[23676]]),
    })
}

/// Analyses a game (`data` base64) for lab project `id`: loads it, runs it for `seconds` while
/// tracing, and returns the overview.
#[tauri::command]
pub async fn zxlab_open(l: State<'_, ZxLab>, id: String, name: String, data: String, seconds: Option<u32>) -> CmdResult<Value> {
    let bytes = unb64(&data)?;
    let an = tauri::async_runtime::spawn_blocking(move || lab::analyze(&name, &bytes, seconds.unwrap_or(15).clamp(2, 120)))
        .await
        .map_err(err)?
        .map_err(err)?;
    let s = summary(&an);
    l.cache.lock().unwrap().insert(id, Arc::new(an));
    Ok(s)
}

fn parse_notes(v: Option<Value>) -> Notes {
    v.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// Listing lines for `from`–`to` (labels from `notes`).
#[tauri::command]
pub fn zxlab_listing(l: State<'_, ZxLab>, id: String, from: u16, to: u32, notes: Option<Value>) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let to = to.clamp(from as u32 + 1, 0x10000).min(from as u32 + 0x4000);
    let lines = lab::listing(&an, &parse_notes(notes), from, to, &BTreeSet::new());
    Ok(serde_json::to_value(lines)?)
}

/// Where an address is used: lines (in the program's range) that jump to, call or point at it.
#[tauri::command]
pub fn zxlab_xrefs(l: State<'_, ZxLab>, id: String, addr: u16, notes: Option<Value>) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let (from, to) = lab::program_range(&an);
    let lines = lab::listing(&an, &parse_notes(notes), from.min(0x5b00), to.max(0xffff).min(0x10000), &BTreeSet::new());
    let refs: Vec<Value> = lines.iter().filter(|x| x.target == Some(addr)).map(|x| json!({"addr": x.addr, "text": x.text})).collect();
    Ok(json!(refs))
}

/// Memory bytes from the loaded game (base64): for the graphics viewer and hex dumps.
#[tauri::command]
pub fn zxlab_memory(l: State<'_, ZxLab>, id: String, from: u16, len: u32) -> CmdResult<String> {
    let an = get(&l, &id)?;
    let end = (from as usize + len as usize).min(0x10000);
    Ok(b64(&an.memory[from as usize..end]))
}

/// A tape block's bytes (base64): loading screens, code to export.
#[tauri::command]
pub fn zxlab_tape_block(l: State<'_, ZxLab>, id: String, index: usize) -> CmdResult<String> {
    let an = get(&l, &id)?;
    let b = an.tape.iter().find(|b| b.index == index).ok_or_else(|| CmdError::new("not_found", "no such block"))?;
    Ok(b64(&b.data))
}

use crate::zxdev::sjasmplus;

/// Assembles `src` (a full program from ORG `from`) and returns `from`–`to` as bytes with the
/// assembler's output, or its messages.
fn assemble_out(asm: &Path, root: &Path, src: &str, from: u16, to: u32) -> Result<(Vec<u8>, String), String> {
    let dir = root.join(crate::zxdev::unique_dir("lab"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let full = format!("    DEVICE ZXSPECTRUM48\n{src}\n    SAVEBIN \"__out.bin\", ${from:04x}, {}\n", to - from as u32);
    let r = (|| {
        std::fs::write(dir.join("src.asm"), full).map_err(|e| e.to_string())?;
        let out = std::process::Command::new(asm).args(["--nologo", "--msg=war", "src.asm"]).current_dir(&dir).output().map_err(|e| e.to_string())?;
        let msgs = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if !out.status.success() {
            return Err(msgs);
        }
        Ok((std::fs::read(dir.join("__out.bin")).map_err(|e| e.to_string())?, msgs))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    r
}

fn assemble(asm: &Path, root: &Path, src: &str, from: u16, to: u32) -> Result<Vec<u8>, String> {
    assemble_out(asm, root, src, from, to).map(|(b, _)| b)
}

/// Assembly source for `from`–`to`, checked by assembling it back: instructions that wouldn't
/// come back the same are written as bytes. {source, verified}.
#[tauri::command]
pub async fn zxlab_source(app: AppHandle, l: State<'_, ZxLab>, id: String, from: u16, to: u32, notes: Option<Value>, title: String) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let notes = parse_notes(notes);
    let asm = sjasmplus(&app);
    let root = app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("zxbuild");
    tauri::async_runtime::spawn_blocking(move || {
        let to = to.clamp(from as u32 + 1, 0x10000);
        let mut force = BTreeSet::new();
        for _ in 0..4 {
            let src = lab::source(&an, &notes, from, to, &force, &title);
            let Some(asm) = &asm else { return Ok(json!({"source": src, "verified": false, "note": "the assembler isn't built, so this wasn't checked"})) };
            match assemble(asm, &root, &src, from, to) {
                Ok(bytes) => {
                    let lines = lab::listing(&an, &notes, from, to, &force);
                    let bad: Vec<u16> = lines.iter().filter(|x| matches!(x.kind, "code" | "reachable") && bytes[(x.addr - from) as usize..(x.addr - from) as usize + x.bytes.len()] != x.bytes[..]).map(|x| x.addr).collect();
                    if bad.is_empty() && bytes[..] == an.memory[from as usize..to as usize] {
                        return Ok(json!({"source": src, "verified": true}));
                    }
                    if bad.is_empty() {
                        return Ok(json!({"source": src, "verified": false, "note": "it assembles, but not to exactly the same bytes"}));
                    }
                    force.extend(bad);
                }
                Err(msgs) => return Ok(json!({"source": src, "verified": false, "note": msgs})),
            }
        }
        Ok(json!({"source": lab::source(&an, &notes, from, to, &force, &title), "verified": false}))
    })
    .await
    .map_err(err)?
}

/// Assembles a patch: `code` placed at `addr`, with every name the project knows available.
/// Returns {ok, bytes (hex), end, splits (an instruction it cuts in half, if any), messages}.
#[tauri::command]
pub async fn zxlab_assemble_patch(app: AppHandle, l: State<'_, ZxLab>, id: String, addr: u16, code: String, notes: Option<Value>) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let notes = parse_notes(notes);
    let asm = sjasmplus(&app).ok_or_else(|| CmdError::new("no_assembler", "The Z80 assembler isn't built yet. Run ./scripts/build-zx-tools.sh."))?;
    let root = app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("zxbuild");
    tauri::async_runtime::spawn_blocking(move || assemble_patch(&asm, &root, &an, &notes, addr, &code)).await.map_err(err)?
}

/// Assembles a patch at `addr` with the program's names available (see zxlab_assemble_patch).
fn assemble_patch(asm: &Path, root: &Path, an: &Analysis, notes: &Notes, addr: u16, code: &str) -> CmdResult<Value> {
    {
        // Every label of the program, so the patch can say "call draw_sprite".
        let (from, to) = lab::program_range(&an);
        let mut equs = String::new();
        let mut names = std::collections::BTreeMap::new();
        for line in lab::listing(&an, &notes, from, to, &BTreeSet::new()) {
            if let Some(n) = line.label {
                names.insert(n, line.addr);
            }
        }
        for (a, n) in &notes.labels {
            names.insert(n.clone(), *a);
        }
        for r in lab::ROM {
            names.entry(r.1.to_string()).or_insert(r.0);
        }
        for (n, a) in &names {
            if code.lines().any(|l| l.trim_start().starts_with(&format!("{n}:"))) {
                continue; // the patch defines it itself
            }
            equs.push_str(&format!("{n:<16} EQU ${a:04x}\n"));
        }
        let src = format!("{equs}\n    ORG ${addr:04x}\n{code}\n__rad_patch_end:\n    DISPLAY \"RADEND=\", /H, __rad_patch_end\n");
        let max = (0x10000 - addr as u32).min(4096);
        let (bytes, out) = match assemble_out(asm, root, &src, addr, addr as u32 + max) {
            Ok(r) => r,
            Err(m) => return Ok(json!({"ok": false, "messages": m})),
        };
        let end = out
            .lines()
            .find_map(|l| l.split("RADEND=0x").nth(1))
            .and_then(|h| u32::from_str_radix(h.trim(), 16).ok())
            .ok_or_else(|| CmdError::new("lab", "the assembler didn't say where the patch ends"))?;
        let len = end.saturating_sub(addr as u32) as usize;
        if len == 0 {
            return Ok(json!({"ok": false, "messages": "the patch is empty"}));
        }
        let bytes = bytes[..len.min(bytes.len())].to_vec();
        let messages: String = out.lines().filter(|l| !l.contains("RADEND=")).collect::<Vec<_>>().join("\n");
        // Does it end inside an instruction of the original? Then that one is cut in half.
        let mut splits = None;
        let end32 = addr as u32 + len as u32;
        if end32 < 0x10000 && matches!(an.kinds[end32 as usize], lab::Kind::CodeTail | lab::Kind::ReachableTail) {
            let mut start = end32;
            while start > 0 && matches!(an.kinds[start as usize], lab::Kind::CodeTail | lab::Kind::ReachableTail) {
                start -= 1;
            }
            splits = Some(start);
        }
        Ok(json!({"ok": true, "bytes": bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(), "end": end, "len": len, "splits": splits, "messages": messages}))
    }
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

/// The game with patches applied: a Z80 snapshot (base64) to play — and, when `check`, what it
/// did in its first seconds on a hidden Spectrum (screen text, crash).
#[tauri::command]
pub async fn zxlab_build(l: State<'_, ZxLab>, id: String, patches: Vec<Patch>, check: Option<bool>) -> CmdResult<Value> {
    let an = get(&l, &id)?;
    let p: Vec<(u16, Vec<u8>)> = patches.iter().map(|p| Ok((p.addr, hex_bytes(&p.bytes)?))).collect::<CmdResult<_>>()?;
    tauri::async_runtime::spawn_blocking(move || {
        let snap = lab::apply_patches(&an.loaded, &p).map_err(err)?;
        let mut out = json!({"file": {"name": "patched.z80", "data": b64(&snap)}});
        if check.unwrap_or(false) {
            let r = rad_zx::run_program("patched.z80", &snap, 3).map_err(err)?;
            out["check"] = json!({"crashed": r.crashed(), "screen": r.screen, "report": r.report.map(|(c, m, l, s)| json!({"code": c, "message": m, "line": l, "statement": s}))});
        }
        Ok(out)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn zxlab_close(l: State<'_, ZxLab>, id: String) {
    l.cache.lock().unwrap().remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAT: &str = include_str!("../../ui/src/lib/zx-templates/bat-and-ball.asm");

    #[test]
    fn patch_a_game_and_play_it() {
        let asm = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../zxtools/bin/sjasmplus");
        if !asm.is_file() {
            return eprintln!("skipped: ./scripts/build-zx-tools.sh");
        }
        let td = tempfile::tempdir().unwrap();
        // Build Bat & Ball, take it apart.
        std::fs::write(td.path().join("main.asm"), BAT).unwrap();
        std::fs::write(td.path().join("w.asm"), "    DEVICE ZXSPECTRUM48\n    INCLUDE \"main.asm\"\n    SAVESNA \"g.sna\", start\n").unwrap();
        assert!(std::process::Command::new(&asm).args(["--nologo", "w.asm"]).current_dir(td.path()).status().unwrap().success());
        let sna = std::fs::read(td.path().join("g.sna")).unwrap();
        let an = lab::analyze("g.sna", &sna, 3).unwrap();
        let notes = Notes::default();
        let (from, to) = lab::program_range(&an);
        let lines = lab::listing(&an, &notes, from, to, &BTreeSet::new());
        // `restart: xor a / out ($fe),a` sets the border black: make it green.
        let k = lines.iter().position(|l| l.text == "out  ($fe),a").unwrap();
        assert_eq!(lines[k - 1].text, "xor  a");
        let at = lines[k - 1].addr;
        // The patch can use the program's own names (here: a call to the routine after it).
        let call = lines.iter().find(|l| l.text.starts_with("call ")).unwrap().text.clone();
        let p = assemble_patch(&asm, td.path(), &an, &notes, at, "    ld a,4\n").unwrap();
        assert_eq!(p["ok"], true, "{p}");
        assert_eq!(p["bytes"], "3e04");
        assert_eq!(p["splits"], json!(at + 1), "2 bytes over xor a + the first byte of out: it cuts the OUT in half, and says so");
        let p2 = assemble_patch(&asm, td.path(), &an, &notes, at, &format!("    {call}\n")).unwrap();
        assert_eq!(p2["ok"], true, "names resolve: {p2}");
        let bad = assemble_patch(&asm, td.path(), &an, &notes, at, "    ld a,\n").unwrap();
        assert_eq!(bad["ok"], false);
        // Apply: ld a,4 over xor a + the OUT's opcode would break it, so write ld a,4 / out ($fe),a in full.
        let full = assemble_patch(&asm, td.path(), &an, &notes, at, "    ld a,4\n    out ($fe),a\n").unwrap();
        assert_eq!(full["len"], 4);
        let snap = lab::apply_patches(&an.loaded, &[(at, hex_bytes(full["bytes"].as_str().unwrap()).unwrap())]).unwrap();
        let mut zx = rad_zx::Spectrum::new(rad_zx::Model::Spectrum48);
        zx.load("p.z80", &snap).unwrap();
        // The game restarts when the ball is lost (no one's playing): then the border is green.
        let mut green = false;
        for _ in 0..600 {
            zx.run_frame();
            zx.skip_audio();
            green |= zx.border_color() == 4;
        }
        assert!(green, "the patched game set a green border");
    }
}

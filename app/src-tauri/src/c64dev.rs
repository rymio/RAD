//! Commodore 64 projects in the Games view: building them (BASIC V2 with RAD's own tokenizer;
//! 6502 assembly with 64tass, see scripts/build-c64-tools.sh) and trying them on a hidden C64,
//! so the AI agent sees errors and what's on the screen.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

use crate::c64::C64State;
use crate::client::{CmdError, CmdResult};
use crate::zxdev::{unique_dir, SrcFile};

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// 64tass: in RAD's bundle (Contents/Helpers/c64) or, in development, c64tools/bin.
pub(crate) fn tass(app: &AppHandle) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        c.push(dir.join("../Helpers/c64/64tass"));
    }
    if let Ok(d) = app.path().resource_dir() {
        c.push(d.join("c64/64tass"));
    }
    if cfg!(debug_assertions) {
        c.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../c64tools/bin/64tass"));
    }
    c.into_iter().find(|p| p.is_file())
}

/// What can be built and run here: BASIC always (running it needs the C64's own ROMs),
/// assembler when 64tass is built.
#[tauri::command]
pub fn c64_tools(app: AppHandle, s: State<'_, C64State>) -> Value {
    json!({ "basic": true, "asm": tass(&app).is_some(), "roms": if s.roms().is_open_roms() { "open" } else { "commodore" } })
}

fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let n = n.split('_').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_").to_lowercase();
    if n.is_empty() { "game".into() } else { n.chars().take(30).collect() }
}

fn build_basic(name: &str, files: &[SrcFile]) -> Value {
    let Some(main) = files.iter().find(|f| f.path == "main.bas").or_else(|| files.iter().find(|f| f.path.ends_with(".bas"))) else {
        return json!({"ok": false, "errors": [{"file": "main.bas", "line": null, "message": "the project has no main.bas"}], "output": "No main.bas"});
    };
    match rad_c64::basic::compile(&main.content) {
        Ok(prog) => {
            let prg = rad_c64::basic::prg(&prog);
            json!({"ok": true, "errors": [], "output": format!("Built {} bytes of BASIC.", prog.len()), "file": {"name": format!("{}.prg", safe_name(name)), "data": b64(&prg)}})
        }
        Err(errs) => {
            let errors: Vec<Value> = errs.iter().map(|e| json!({"file": main.path, "line": e.text_line, "basic_line": e.line, "message": e.message})).collect();
            let output = errs.iter().map(|e| format!("{}: {e}", main.path)).collect::<Vec<_>>().join("\n");
            json!({"ok": false, "errors": errors, "output": output})
        }
    }
}

fn build_asm(app: &AppHandle, name: &str, files: &[SrcFile]) -> CmdResult<Value> {
    let asm = tass(app).ok_or_else(|| CmdError::new("no_assembler", "The 6502 assembler isn't built yet. Run ./scripts/build-c64-tools.sh."))?;
    let root = app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("c64build");
    assemble(&asm, &root, name, files)
}

/// Writes the project's files into a fresh folder under `root` (deleted by the caller).
pub(crate) fn write_files(dir: &Path, files: &[SrcFile]) -> CmdResult<()> {
    std::fs::create_dir_all(dir).map_err(|e| CmdError::new("io", e.to_string()))?;
    for f in files {
        let rel = Path::new(&f.path);
        if !rel.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
            return Err(CmdError::new("bad_request", format!("invalid file path {}", f.path)));
        }
        let p = dir.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| CmdError::new("io", e.to_string()))?;
        }
        std::fs::write(&p, &f.content).map_err(|e| CmdError::new("io", e.to_string()))?;
    }
    Ok(())
}

/// Runs 64tass on `main` in `dir`: (assembled program, its errors and warnings, the output).
pub(crate) fn run_tass(asm: &Path, dir: &Path, main: &str, extra: &[&str]) -> CmdResult<(Option<Vec<u8>>, Vec<Value>, String)> {
    let out = Command::new(asm)
        .args(["--cbm-prg", "-q", "-o", "__rad_out.prg"])
        .args(extra)
        .arg(main)
        .current_dir(dir)
        .output()
        .map_err(|e| CmdError::new("build", format!("Couldn't run the assembler: {e}")))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let mut errors = Vec::new();
    for l in text.lines() {
        // "main.asm:12:9: error: not defined symbol 'nowhere'"
        let mut parts = l.splitn(4, ':');
        let (Some(file), Some(line), Some(_col), Some(rest)) = (parts.next(), parts.next(), parts.next(), parts.next()) else { continue };
        let Ok(line) = line.trim().parse::<u32>() else { continue };
        let (kind, msg) = rest.trim().split_once(": ").unwrap_or(("error", rest.trim()));
        if kind == "note" {
            continue;
        }
        errors.push(json!({"file": file, "line": line, "kind": kind, "message": msg}));
    }
    let prg = std::fs::read(dir.join("__rad_out.prg")).ok().filter(|_| out.status.success());
    Ok((prg, errors, text))
}

/// Assembles in a fresh folder under `root` (deleted afterwards).
fn assemble(asm: &Path, root: &Path, name: &str, files: &[SrcFile]) -> CmdResult<Value> {
    if !files.iter().any(|f| f.path == "main.asm") {
        return Ok(json!({"ok": false, "errors": [{"file": "main.asm", "line": null, "message": "the project has no main.asm"}], "output": "No main.asm"}));
    }
    let dir = root.join(unique_dir("b"));
    let result = (|| -> CmdResult<Value> {
        write_files(&dir, files)?;
        let (prg, mut errors, text) = run_tass(asm, &dir, "main.asm", &[])?;
        let ok = prg.is_some();
        errors.retain(|e| !ok || e["kind"] != "error");
        let output = if text.trim().is_empty() { if ok { "Assembled.".to_string() } else { "The assembler failed.".to_string() } } else { text };
        Ok(match prg {
            Some(prg) if prg.len() > 2 => {
                let addr = u16::from_le_bytes([prg[0], prg[1]]);
                let note = if addr == 0x0801 { String::new() } else { format!(" It loads at ${addr:04X}; RAD starts it with SYS {addr}.") };
                json!({"ok": true, "errors": errors, "output": format!("{output}{note}"), "file": {"name": format!("{}.prg", safe_name(name)), "data": b64(&prg)}})
            }
            _ => json!({"ok": false, "errors": errors, "output": output}),
        })
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Builds a C64 project: {ok, errors: [{file, line, message}], output, file?: {name, data}}.
#[tauri::command]
pub async fn c64_build(app: AppHandle, kind: String, name: String, files: Vec<SrcFile>) -> CmdResult<Value> {
    tauri::async_runtime::spawn_blocking(move || match kind.as_str() {
        "c64-basic" => Ok(build_basic(&name, &files)),
        "c64-asm" => build_asm(&app, &name, &files),
        _ => Err(CmdError::new("bad_request", "unknown project kind")),
    })
    .await
    .map_err(|e| CmdError::new("build", e.to_string()))?
}

fn check(roms: Arc<rad_c64::Roms>, name: &str, data: &[u8], seconds: u32) -> CmdResult<Value> {
    let r = rad_c64::run_program(roms, name, data, seconds).map_err(|e| CmdError::new("c64", e.to_string()))?;
    Ok(json!({
        "started": r.started,
        "crashed": r.crashed,
        "error": r.error.as_ref().map(|(m, l)| json!({"message": m, "line": l})),
        "screen": r.screen,
        "border": r.border,
    }))
}

/// Runs a built program on a hidden C64 for `seconds` with no keys pressed: what was on the
/// screen, the BASIC error it stopped with (if any), and whether it crashed. BASIC programs
/// need the C64's own ROMs (`needs_roms` says so when RAD only has the free ones).
#[tauri::command]
pub async fn c64_check(s: State<'_, C64State>, name: String, data: String, seconds: Option<u32>, basic: Option<bool>) -> CmdResult<Value> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).map_err(|_| CmdError::new("bad_request", "damaged data"))?;
    let roms = s.roms();
    let open = roms.is_open_roms();
    if basic.unwrap_or(false) && open {
        return Ok(json!({"needs_roms": true, "roms": "open"}));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut v = check(roms, &name, &bytes, seconds.unwrap_or(3).clamp(1, 30))?;
        v["roms"] = json!(if open { "open" } else { "commodore" });
        Ok(v)
    })
    .await
    .map_err(|e| CmdError::new("c64", e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAR_CATCH_ASM: &str = include_str!("../../ui/src/lib/c64-templates/star-catch.asm");
    const STAR_CATCH_BAS: &str = include_str!("../../ui/src/lib/c64-templates/star-catch.bas");
    const BLANK_ASM: &str = include_str!("../../ui/src/lib/c64-templates/blank.asm");

    fn tass_dev() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../c64tools/bin/64tass");
        p.is_file().then_some(p)
    }

    fn data(file: &Value) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(file["data"].as_str().unwrap()).unwrap()
    }

    fn open() -> Arc<rad_c64::Roms> {
        Arc::new(rad_c64::Roms::open_roms())
    }

    #[test]
    fn the_assembler_template_builds_and_plays() {
        let Some(asm) = tass_dev() else { return eprintln!("skipped: run ./scripts/build-c64-tools.sh") };
        let td = tempfile::tempdir().unwrap();
        let b = assemble(&asm, td.path(), "Star Catch", &[SrcFile { path: "main.asm".into(), content: STAR_CATCH_ASM.into() }]).unwrap();
        assert_eq!(b["ok"], true, "{b}");
        assert_eq!(b["file"]["name"], "star_catch.prg");
        let r = check(open(), "star_catch.prg", &data(&b["file"]), 4).unwrap();
        let screen: Vec<String> = serde_json::from_value(r["screen"].clone()).unwrap();
        println!("{screen:#?}");
        assert_eq!(r["crashed"], false);
        assert!(screen[0].starts_with("STAR CATCH") && screen[0].contains("SCORE 00"), "{screen:?}");
        assert!(screen.get(23).is_some_and(|l| l.contains("<=>")), "the catcher is drawn: {screen:?}");
        assert!(std::fs::read_dir(td.path()).unwrap().next().is_none(), "the build folder is cleaned up");

        let b = assemble(&asm, td.path(), "Hi", &[SrcFile { path: "main.asm".into(), content: BLANK_ASM.replace("TITLE", "HI THERE") }]).unwrap();
        let r = check(open(), "hi.prg", &data(&b["file"]), 1).unwrap();
        assert_eq!(r["screen"][0], "HELLO FROM HI THERE", "{r}");
    }

    #[test]
    fn the_game_plays_with_the_joystick() {
        let Some(asm) = tass_dev() else { return };
        let td = tempfile::tempdir().unwrap();
        let b = assemble(&asm, td.path(), "Star Catch", &[SrcFile { path: "main.asm".into(), content: STAR_CATCH_ASM.into() }]).unwrap();
        let mut c = rad_c64::C64::new(open());
        c.load("game.prg", &data(&b["file"])).unwrap();
        for _ in 0..150 {
            c.run_frame();
        }
        let at = |c: &rad_c64::C64| c.screen_text()[23].find("<=>");
        let before = at(&c).unwrap();
        c.set_joystick(rad_c64::joy::LEFT);
        for _ in 0..30 {
            c.run_frame();
        }
        c.set_joystick(0);
        assert!(at(&c).unwrap() + 5 < before, "pushing left moves the catcher");
    }

    #[test]
    fn assembler_errors_point_at_lines() {
        let Some(asm) = tass_dev() else { return };
        let td = tempfile::tempdir().unwrap();
        let b = assemble(&asm, td.path(), "x", &[SrcFile { path: "main.asm".into(), content: "        * = $c000\nstart   lda #\n        jmp nowhere\n".into() }]).unwrap();
        assert_eq!(b["ok"], false);
        let errs: Vec<(String, u64)> = b["errors"].as_array().unwrap().iter().map(|e| (e["file"].as_str().unwrap().to_string(), e["line"].as_u64().unwrap_or(0))).collect();
        assert!(errs.contains(&("main.asm".into(), 2)) && errs.contains(&("main.asm".into(), 3)), "{b}");
        // Not at $0801: RAD says how it starts.
        let b = assemble(&asm, td.path(), "x", &[SrcFile { path: "main.asm".into(), content: "        * = $c000\n        inc $d020\n        jmp $c000\n".into() }]).unwrap();
        assert!(b["output"].as_str().unwrap().contains("SYS 49152"), "{b}");
        let r = check(open(), "x.prg", &data(&b["file"]), 1).unwrap();
        assert_eq!(r["started"], true, "{r}");
    }

    #[test]
    fn basic_projects_build_or_explain() {
        let ok = build_basic("Star Catch", &[SrcFile { path: "main.bas".into(), content: STAR_CATCH_BAS.into() }]);
        assert_eq!(ok["ok"], true, "{ok}");
        assert_eq!(ok["file"]["name"], "star_catch.prg");
        let text = rad_c64::basic::detokenize(&data(&ok["file"])[2..]);
        assert!(text.starts_with("10 REM *** STAR CATCH ***\n"), "{text}");
        let bad = build_basic("x", &[SrcFile { path: "main.bas".into(), content: "10 PRINT \"{NOPE}\"\nPRINT 2".into() }]);
        assert_eq!(bad["ok"], false);
        assert_eq!(bad["errors"].as_array().unwrap().len(), 2);
        assert_eq!(bad["errors"][0]["basic_line"], 10);
    }

    /// With the C64's own ROMs (set RAD_C64_ROMS to a folder with VICE's kernal, basic and
    /// chargen files), the BASIC template runs.
    #[test]
    fn the_basic_template_plays_with_real_roms() {
        let Ok(dir) = std::env::var("RAD_C64_ROMS") else { return eprintln!("skipped: set RAD_C64_ROMS") };
        let find = |p: &str| std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).find(|f| f.file_name().unwrap().to_string_lossy().to_lowercase().starts_with(p)).map(|f| std::fs::read(f).unwrap()).unwrap();
        let roms = Arc::new(rad_c64::Roms::new(find("chargen"), find("basic"), find("kernal")).unwrap());
        let b = build_basic("Star Catch", &[SrcFile { path: "main.bas".into(), content: STAR_CATCH_BAS.into() }]);
        let r = check(roms, "star_catch.prg", &data(&b["file"]), 6).unwrap();
        println!("{r:#}");
        assert_eq!(r["error"], Value::Null, "{r}");
        assert!(r["screen"][0].as_str().unwrap().starts_with("STAR CATCH"), "{r}");
    }
}

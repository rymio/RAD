//! ZX Spectrum projects in the Games view: building them (Sinclair BASIC with RAD's own
//! tokenizer; Z80 assembly with sjasmplus, see scripts/build-zx-tools.sh) and trying them on a
//! hidden Spectrum, so the AI agent sees errors and what's on the screen.

use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::client::{CmdError, CmdResult};

#[derive(Deserialize)]
pub struct SrcFile {
    path: String,
    content: String,
}

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// sjasmplus: in RAD's bundle (Contents/Helpers/zx) or, in development, zxtools/bin.
fn sjasmplus(app: &AppHandle) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        c.push(dir.join("../Helpers/zx/sjasmplus"));
    }
    if let Ok(d) = app.path().resource_dir() {
        c.push(d.join("zx/sjasmplus"));
    }
    if cfg!(debug_assertions) {
        c.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../zxtools/bin/sjasmplus"));
    }
    c.into_iter().find(|p| p.is_file())
}

#[tauri::command]
pub fn zx_tools(app: AppHandle) -> Value {
    json!({ "basic": true, "asm": sjasmplus(&app).is_some() })
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
    match rad_zx::basic::compile(&main.content) {
        Ok(prog) => {
            let tap = rad_zx::basic::tap(&prog, name, rad_zx::basic::first_line(&prog));
            json!({"ok": true, "errors": [], "output": format!("Built {} bytes of BASIC.", prog.len()), "file": {"name": format!("{}.tap", safe_name(name)), "data": b64(&tap)}})
        }
        Err(errs) => {
            let errors: Vec<Value> = errs.iter().map(|e| json!({"file": main.path, "line": e.text_line, "basic_line": e.line, "message": e.message})).collect();
            let output = errs.iter().map(|e| format!("{}: {e}", main.path)).collect::<Vec<_>>().join("\n");
            json!({"ok": false, "errors": errors, "output": output})
        }
    }
}

fn build_asm(app: &AppHandle, name: &str, files: &[SrcFile]) -> CmdResult<Value> {
    let asm = sjasmplus(app).ok_or_else(|| CmdError::new("no_assembler", "The Z80 assembler isn't built yet. Run ./scripts/build-zx-tools.sh."))?;
    let root = app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("zxbuild");
    assemble(&asm, &root, name, files)
}

/// Assembles in a fresh folder under `root` (deleted afterwards).
fn assemble(asm: &Path, root: &Path, name: &str, files: &[SrcFile]) -> CmdResult<Value> {
    let Some(main) = files.iter().find(|f| f.path == "main.asm") else {
        return Ok(json!({"ok": false, "errors": [{"file": "main.asm", "line": null, "message": "the project has no main.asm"}], "output": "No main.asm"}));
    };
    let dir = root.join(format!("b{}", crate::now_ms()));
    std::fs::create_dir_all(&dir).map_err(|e| CmdError::new("io", e.to_string()))?;
    let result = (|| -> CmdResult<Value> {
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
        // RAD's wrapper: the machine, the program, and a snapshot that starts at `start`.
        let has_device = main.content.lines().any(|l| l.trim_start().to_ascii_uppercase().starts_with("DEVICE"));
        let wrapper = format!(
            "{}    INCLUDE \"main.asm\"\n    SAVESNA \"__rad_out.sna\", start\n",
            if has_device { "" } else { "    DEVICE ZXSPECTRUM48\n" }
        );
        std::fs::write(dir.join("__rad_build.asm"), wrapper).map_err(|e| CmdError::new("io", e.to_string()))?;
        let out = Command::new(asm)
            .args(["--nologo", "--msg=war", "__rad_build.asm"])
            .current_dir(&dir)
            .output()
            .map_err(|e| CmdError::new("build", format!("Couldn't run the assembler: {e}")))?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let mut errors = Vec::new();
        let mut lines_out = Vec::new();
        for l in text.lines() {
            // "main.asm(12): error: Illegal instruction:     ld a,"
            let mut l = l.to_string();
            if l.contains("__rad_build.asm") && l.contains("start") {
                l = "main.asm: error: main.asm needs a `start:` label where the program begins".into();
            }
            if let Some((loc, rest)) = l.split_once("): ") {
                if let Some((file, line)) = loc.rsplit_once('(') {
                    let (kind, msg) = rest.split_once(": ").unwrap_or(("error", rest));
                    errors.push(json!({"file": file, "line": line.parse::<u32>().ok(), "kind": kind, "message": msg}));
                }
            }
            lines_out.push(l);
        }
        let sna = std::fs::read(dir.join("__rad_out.sna")).ok();
        let ok = out.status.success() && sna.is_some();
        errors.retain(|e| !ok || e["kind"] != "error");
        let output = if lines_out.is_empty() { if ok { "Assembled.".to_string() } else { "The assembler failed.".to_string() } } else { lines_out.join("\n") };
        Ok(match (ok, sna) {
            (true, Some(sna)) => json!({"ok": true, "errors": errors, "output": output, "file": {"name": format!("{}.sna", safe_name(name)), "data": b64(&sna)}}),
            _ => json!({"ok": false, "errors": errors, "output": output}),
        })
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Builds a Spectrum project: {ok, errors: [{file, line, message}], output, file?: {name, data}}.
#[tauri::command]
pub async fn zx_build(app: AppHandle, kind: String, name: String, files: Vec<SrcFile>) -> CmdResult<Value> {
    tauri::async_runtime::spawn_blocking(move || match kind.as_str() {
        "zx-basic" => Ok(build_basic(&name, &files)),
        "zx-asm" => build_asm(&app, &name, &files),
        _ => Err(CmdError::new("bad_request", "unknown project kind")),
    })
    .await
    .map_err(|e| CmdError::new("build", e.to_string()))?
}

/// Runs a built program on a hidden Spectrum for `seconds` with no keys pressed: what was on
/// the screen, the BASIC report it stopped with (if any), and whether it crashed.
#[tauri::command]
pub async fn zx_check(name: String, data: String, seconds: Option<u32>) -> CmdResult<Value> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).map_err(|_| CmdError::new("bad_request", "damaged data"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let r = rad_zx::run_program(&name, &bytes, seconds.unwrap_or(3).clamp(1, 30)).map_err(|e| CmdError::new("zx", e.to_string()))?;
        Ok(json!({
            "started": r.started,
            "crashed": r.crashed(),
            "report": r.report.as_ref().map(|(c, m, l, s)| json!({"code": c, "message": m, "line": l, "statement": s})),
            "screen": r.screen,
        }))
    })
    .await
    .map_err(|e| CmdError::new("zx", e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAR_CATCH: &str = include_str!("../../ui/src/lib/zx-templates/star-catch.bas");
    const BAT_AND_BALL: &str = include_str!("../../ui/src/lib/zx-templates/bat-and-ball.asm");

    fn run(file: &Value, seconds: u32) -> rad_zx::BasicRun {
        let data = base64::engine::general_purpose::STANDARD.decode(file["data"].as_str().unwrap()).unwrap();
        rad_zx::run_program(file["name"].as_str().unwrap(), &data, seconds).unwrap()
    }

    #[test]
    fn the_basic_template_builds_and_plays() {
        let b = build_basic("Star Catch", &[SrcFile { path: "main.bas".into(), content: STAR_CATCH.into() }]);
        assert_eq!(b["ok"], true, "{b}");
        let r = run(&b["file"], 4);
        println!("{:#?}", r.screen);
        assert!(r.started && !r.crashed());
        assert_eq!(r.report, None, "a game keeps running: {:?}", r.report);
        // Nobody's playing, so stars land or fall past on their own.
        assert!(r.screen.first().is_some_and(|l| l.starts_with("SCORE ") && l.contains("LIVES ")), "{:?}", r.screen);
        assert!(r.screen.iter().any(|l| l.contains("=^=")), "the catcher is drawn");
    }

    fn sjasm() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../zxtools/bin/sjasmplus");
        p.is_file().then_some(p)
    }

    #[test]
    fn the_assembler_template_builds_and_plays() {
        let Some(asm) = sjasm() else { return eprintln!("skipped: run ./scripts/build-zx-tools.sh") };
        let td = tempfile::tempdir().unwrap();
        let b = assemble(&asm, td.path(), "Bat & Ball", &[SrcFile { path: "main.asm".into(), content: BAT_AND_BALL.into() }]).unwrap();
        assert_eq!(b["ok"], true, "{b}");
        assert_eq!(b["file"]["name"], "bat_ball.sna");
        let r = run(&b["file"], 3);
        println!("{:#?}", r.screen);
        assert!(!r.crashed(), "{:?}", r.screen);
        assert!(r.screen.first().is_some_and(|l| l.contains("BAT & BALL") && l.contains("SCORE 00")), "{:?}", r.screen);
        assert!(std::fs::read_dir(td.path()).unwrap().next().is_none(), "the build folder is cleaned up");
    }

    #[test]
    fn assembler_errors_point_at_lines() {
        let Some(asm) = sjasm() else { return };
        let td = tempfile::tempdir().unwrap();
        let b = assemble(&asm, td.path(), "x", &[SrcFile { path: "main.asm".into(), content: "    ORG $8000\nstart:\n    ld a,\n    jp nowhere\n".into() }]).unwrap();
        assert_eq!(b["ok"], false);
        let errs: Vec<(String, u64)> = b["errors"].as_array().unwrap().iter().map(|e| (e["file"].as_str().unwrap().to_string(), e["line"].as_u64().unwrap_or(0))).collect();
        assert!(errs.contains(&("main.asm".into(), 3)) && errs.contains(&("main.asm".into(), 4)), "{b}");
        let b = assemble(&asm, td.path(), "x", &[SrcFile { path: "main.asm".into(), content: "    ORG $8000\n    ret\n".into() }]).unwrap();
        assert!(b["output"].as_str().unwrap().contains("needs a `start:` label"), "{b}");
    }

    #[test]
    fn basic_projects_build_or_explain() {
        let ok = build_basic("Star Catch", &[SrcFile { path: "main.bas".into(), content: "10 PRINT \"HI\"".into() }]);
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["file"]["name"], "star_catch.tap");
        let bad = build_basic("x", &[SrcFile { path: "main.bas".into(), content: "10 PRINT \"HI\nPRINT 2".into() }]);
        assert_eq!(bad["ok"], false);
        assert_eq!(bad["errors"].as_array().unwrap().len(), 2);
        assert_eq!(bad["errors"][0]["basic_line"], 10);
    }
}

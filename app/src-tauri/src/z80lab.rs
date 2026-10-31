//! Z80 Lab (Games → Z80 Lab): assembling the program for a computer the owner wires up on the
//! canvas. sjasmplus (the Spectrum projects' assembler, scripts/build-zx-tools.sh) makes a 64K
//! image; the listing says which bytes are code and which source line each address comes from,
//! so the UI can burn just those into the ROM chip and follow the program as it runs.

use base64::Engine as _;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::client::{CmdError, CmdResult};
use crate::zxdev::{sjasmplus, unique_dir};

/// "12+   0007 7E 23        ld a,(hl)": line 12 of main.asm put bytes 7E 23 at 0007.
fn listing(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut in_main = false;
    for l in text.lines() {
        if let Some(f) = l.strip_prefix("# file opened: ") {
            in_main = f.trim() == "main.asm";
            continue;
        }
        if l.starts_with("# file closed: main.asm") {
            in_main = false;
            continue;
        }
        if !in_main {
            continue;
        }
        let t = l.trim_start();
        let Some((num, rest)) = t.split_once(|c: char| !c.is_ascii_digit()) else { continue };
        let Ok(line) = num.parse::<u32>() else { continue };
        let rest = rest.trim_start_matches(['+', '~']).trim_start();
        let Some((addr, rest)) = rest.split_once(' ') else { continue };
        let Ok(addr) = u16::from_str_radix(addr, 16) else { continue };
        // The bytes: pairs of hex digits, until the source text starts (two spaces or more).
        let field = rest.get(..rest.len().min(24)).unwrap_or(rest);
        let bytes = field.split("  ").next().unwrap_or("").split(' ').filter(|b| b.len() == 2 && u8::from_str_radix(b, 16).is_ok()).count();
        if bytes > 0 {
            out.push(json!([addr, bytes, line]));
        }
    }
    out
}

/// Assembles `source` (main.asm): {ok, errors: [{line, message}], output, image?: base64 64K,
/// code?: [[addr, bytes, line]…], labels?: {name: addr}}.
#[tauri::command]
pub async fn z80lab_assemble(app: AppHandle, source: String) -> CmdResult<Value> {
    let asm = sjasmplus(&app).ok_or_else(|| CmdError::new("no_assembler", "The Z80 assembler isn't built yet. Run ./scripts/build-zx-tools.sh."))?;
    let root = app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("z80lab");
    tauri::async_runtime::spawn_blocking(move || {
        let dir = root.join(unique_dir("a"));
        std::fs::create_dir_all(&dir).map_err(|e| CmdError::new("io", e.to_string()))?;
        let result = (|| -> CmdResult<Value> {
            std::fs::write(dir.join("main.asm"), &source).map_err(|e| CmdError::new("io", e.to_string()))?;
            std::fs::write(dir.join("__lab.asm"), "    DEVICE NOSLOT64K\n    INCLUDE \"main.asm\"\n    SAVEBIN \"__out.bin\", 0, $10000\n").map_err(|e| CmdError::new("io", e.to_string()))?;
            let out = crate::helper_command(&asm)
                .args(["--nologo", "--msg=war", "--lst=__out.lst", "--lstlab=sort", "__lab.asm"])
                .current_dir(&dir)
                .output()
                .map_err(|e| CmdError::new("build", format!("Couldn't run the assembler: {e}")))?;
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            let mut errors = Vec::new();
            for l in text.lines() {
                if let Some((loc, rest)) = l.split_once("): ") {
                    if let Some((file, line)) = loc.rsplit_once('(') {
                        let (kind, msg) = rest.split_once(": ").unwrap_or(("error", rest));
                        errors.push(json!({"file": file, "line": line.parse::<u32>().ok(), "kind": kind, "message": msg}));
                    }
                }
            }
            let image = std::fs::read(dir.join("__out.bin")).ok();
            let ok = out.status.success() && image.is_some();
            let output = if text.trim().is_empty() { if ok { "Assembled.".into() } else { "The assembler failed.".into() } } else { text.trim().to_string() };
            let Some(image) = image.filter(|_| ok) else {
                return Ok(json!({"ok": false, "errors": errors, "output": output}));
            };
            let lst = std::fs::read_to_string(dir.join("__out.lst")).unwrap_or_default();
            let mut labels = serde_json::Map::new();
            let mut table = false;
            for l in lst.lines() {
                if l.starts_with("------") {
                    table = true;
                    continue;
                }
                // "0x0007   loop" (an X marks labels never used).
                if let (true, Some(v)) = (table, l.strip_prefix("0x")) {
                    let mut parts = v.split_whitespace();
                    if let (Some(addr), Some(mut name)) = (parts.next(), parts.next()) {
                        if name == "X" {
                            name = parts.next().unwrap_or("");
                        }
                        if let Ok(a) = u32::from_str_radix(addr, 16) {
                            labels.insert(name.to_string(), json!(a));
                        }
                    }
                }
            }
            Ok(json!({
                "ok": true,
                "errors": errors,
                "output": output,
                "image": base64::engine::general_purpose::STANDARD.encode(&image),
                "code": listing(&lst),
                "labels": labels,
            }))
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    })
    .await
    .map_err(|e| CmdError::new("build", e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::listing;

    #[test]
    fn reads_the_listing() {
        let lst = "# file opened: __lab.asm\n 1    0000                  DEVICE NOSLOT64K\n# file opened: main.asm\n 1+   0000              X equ 1\n 3+   0000 F3           start:  di\n 4+   0001 31 00 00             ld sp,$0000\n13+   0011 48 69 0D 0A  msg:    db \"Hi\",13,10,0\n13+   0015 00\n# file closed: main.asm\n 3    0016                  SAVEBIN\n";
        let v = listing(lst);
        assert_eq!(serde_json::to_string(&v).unwrap(), "[[0,1,3],[1,3,4],[17,4,13],[21,1,13]]");
    }
}

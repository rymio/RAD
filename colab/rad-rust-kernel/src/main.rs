//! RAD's Rust Colab kernel: evcxr behind JSON lines, one process per notebook.
//!
//! Requests come one per line on stdin, `{"id": 1, "op": "run", "code": "…"}`; every answer is a
//! line on stdout carrying the same id. A run streams `stdout` and `stderr` lines while the cell
//! runs, then ends with `done` (the value of the cell's last expression, by MIME type) or `error`
//! (compile errors with their line in the cell, a panic, or a lost runtime).
//!
//! - `run` — evaluates a cell. State (variables, functions, `:dep` crates) is kept between cells;
//!   evcxr's commands (`:dep`, `:vars`, `:clear`, `:opt`…) work as in its REPL.
//! - `interrupt` — read while a cell runs: kills the runtime process, so the cell stops and the
//!   variables go (functions, types and dependencies stay).
//! - `offline` — `{"on": true}` builds with `cargo --offline`, from the crates already downloaded.
//! - `vars` — the variables and their types.
//!
//! evcxr runs cells in a child process that is this same executable started again, so
//! `runtime_hook` must come first in `main`.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use evcxr::{CommandContext, Error, EvalOutputs};
use serde_json::{json, Value};

type Out = Arc<Mutex<std::io::Stdout>>;

fn send(out: &Out, v: Value) {
    let mut o = out.lock().unwrap();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn main() {
    evcxr::runtime_hook();
    // evcxr builds cells with `strip = "debuginfo"` and `-C prefer-dynamic`; on macOS (Xcode 27's
    // strip) that leaves a dylib dyld refuses ("mis-aligned LINKEDIT string pool"). Cargo's
    // profile settings from the environment win over the manifest's.
    // A panic shows its message, not a backtrace through evcxr's runtime.
    // SAFETY: no other thread is running yet.
    unsafe {
        std::env::set_var("CARGO_PROFILE_DEV_STRIP", "none");
        std::env::set_var("RUST_BACKTRACE", "0");
    }
    let out: Out = Arc::new(Mutex::new(std::io::stdout()));

    let (mut ctx, outputs) = match CommandContext::new() {
        Ok(c) => c,
        Err(e) => {
            send(&out, json!({ "type": "fatal", "message": e.to_string() }));
            std::process::exit(1);
        }
    };

    // What the cell prints, as it prints it, labelled with the cell being run.
    // A panic only shows on stderr: the cell then ends in an error, not `done`.
    let current = Arc::new(Mutex::new(0u64));
    let panicked = Arc::new(AtomicBool::new(false));
    let interrupted = Arc::new(AtomicBool::new(false));
    for (rx, name) in [(outputs.stdout, "stdout"), (outputs.stderr, "stderr")] {
        let (out, current, panicked) = (out.clone(), current.clone(), panicked.clone());
        std::thread::spawn(move || {
            while let Ok(line) = rx.recv() {
                let id = *current.lock().unwrap();
                if name == "stderr" && line.starts_with("thread '") && line.contains("panicked at") {
                    panicked.store(true, Ordering::SeqCst);
                }
                send(&out, json!({ "id": id, "type": name, "text": format!("{line}\n") }));
            }
        });
    }

    // Requests are read on their own thread, so an interrupt arrives while a cell runs.
    let (tx, requests) = mpsc::channel::<Value>();
    let child = ctx.process_handle();
    let stop = interrupted.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
            if req["op"] == "interrupt" {
                stop.store(true, Ordering::SeqCst);
                let _ = child.lock().unwrap().kill();
            } else if tx.send(req).is_err() {
                break;
            }
        }
        std::process::exit(0);
    });

    send(&out, json!({ "type": "ready", "version": env!("CARGO_PKG_VERSION") }));
    for req in requests {
        let id = req["id"].as_u64().unwrap_or(0);
        match req["op"].as_str().unwrap_or("") {
            "run" => {
                *current.lock().unwrap() = id;
                panicked.store(false, Ordering::SeqCst);
                interrupted.store(false, Ordering::SeqCst);
                let code = req["code"].as_str().unwrap_or("");
                let started = Instant::now();
                let result = ctx.execute(code);
                // The cell's last output lines arrive on the other threads; let them go first.
                std::thread::sleep(std::time::Duration::from_millis(30));
                let ms = started.elapsed().as_millis() as u64;
                let reply = match result {
                    Err(Error::SubprocessTerminated(_)) if interrupted.load(Ordering::SeqCst) => json!({ "id": id, "type": "error", "kind": "interrupted", "message": "Stopped. The variables were lost; functions, types and crates are still defined.", "ms": ms }),
                    Ok(_) if panicked.load(Ordering::SeqCst) => json!({ "id": id, "type": "error", "kind": "panic", "message": "The cell panicked", "ms": ms }),
                    r => answer(id, code, r, ms),
                };
                send(&out, reply);
            }
            "offline" => {
                let on = req["on"].as_bool().unwrap_or(false);
                let r = ctx.execute(&format!(":offline {}", if on { 1 } else { 0 }));
                send(&out, json!({ "id": id, "type": "done", "ok": r.is_ok() }));
            }
            "vars" => {
                let vars: Vec<Value> = ctx.variables_and_types().map(|(n, t)| json!({ "name": n, "type": t })).collect();
                send(&out, json!({ "id": id, "type": "vars", "vars": vars }));
            }
            other => send(&out, json!({ "id": id, "type": "error", "kind": "request", "message": format!("unknown op {other}") })),
        }
    }
}

fn answer(id: u64, code: &str, result: Result<EvalOutputs, Error>, ms: u64) -> Value {
    match result {
        Ok(o) => json!({ "id": id, "type": "done", "data": o.content_by_mime_type, "ms": ms }),
        Err(Error::CompilationErrors(errors)) => {
            let list: Vec<Value> = errors
                .iter()
                .filter(|e| e.level() != "warning")
                .map(|e| {
                    let span = e.primary_spanned_message().and_then(|m| m.span.as_ref());
                    // The plain-text report points at the cell's own lines.
                    let mut report = Vec::new();
                    if let Some(r) = e.build_report("cell".into(), code.to_string(), evcxr::Theme::Light) {
                        let _ = r.write(("cell".to_string(), ariadne_source(code)), &mut report);
                    }
                    json!({
                        "message": e.message(),
                        "code": e.code(),
                        "line": span.map(|s| s.start_line),
                        "column": span.map(|s| s.start_column),
                        "help": e.help(),
                        "hint": e.evcxr_extra_hint(),
                        "report": strip_ansi(&String::from_utf8_lossy(&report)),
                        "rendered": e.rendered(),
                    })
                })
                .collect();
            json!({ "id": id, "type": "error", "kind": "compile", "message": "The cell didn't compile", "errors": list, "ms": ms })
        }
        Err(Error::TypeRedefinedVariablesLost(vars)) => json!({ "id": id, "type": "error", "kind": "lost", "message": format!("A type was redefined, so these variables were lost: {}", vars.join(", ")), "ms": ms }),
        Err(Error::SubprocessTerminated(m)) => json!({ "id": id, "type": "error", "kind": "terminated", "message": m, "ms": ms }),
        Err(e) => json!({ "id": id, "type": "error", "kind": "runtime", "message": e.to_string(), "ms": ms }),
    }
}

fn ariadne_source(code: &str) -> ariadne::Source<String> {
    ariadne::Source::from(code.to_string())
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI: ESC [ … final byte in @..~
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

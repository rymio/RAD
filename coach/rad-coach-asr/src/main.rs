//! `rad-coach-asr whisper <model>` or `rad-coach-asr vad <model>`: whisper.cpp for Coach, in its own
//! process so its ggml never meets the llama.cpp linked into RAD. The protocol is in `asr::remote`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;

use asr::remote::{Reply, Request};
use asr::whisper::{SileroVad, Transcriber};

enum Engine {
    Whisper(Transcriber),
    Vad(SileroVad),
}

fn say(out: &mut impl Write, reply: &Reply) {
    let line = serde_json::to_string(reply).unwrap_or_else(|_| r#"{"error":"could not encode the reply"}"#.into());
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (kind, model) = match (args.next(), args.next()) {
        (Some(k), Some(m)) => (k, PathBuf::from(m)),
        _ => {
            eprintln!("usage: rad-coach-asr whisper|vad <model>");
            std::process::exit(2);
        }
    };
    let mut out = std::io::stdout().lock();
    let loaded = match kind.as_str() {
        "whisper" => Transcriber::load(&model).map(Engine::Whisper),
        "vad" => SileroVad::load(&model).map(Engine::Vad),
        other => {
            say(&mut out, &Reply { error: Some(format!("unknown engine {other}")), ..Default::default() });
            std::process::exit(2);
        }
    };
    let mut engine = match loaded {
        Ok(e) => e,
        Err(e) => {
            say(&mut out, &Reply { error: Some(e.to_string()), ..Default::default() });
            std::process::exit(1);
        }
    };
    say(&mut out, &Reply { ready: true, ..Default::default() });

    let mut input = BufReader::new(std::io::stdin().lock());
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => return, // RAD closed the pipe
            Ok(_) => {}
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                say(&mut out, &Reply { error: Some(format!("bad request: {e}")), ..Default::default() });
                return; // the stream is out of step; RAD restarts the helper
            }
        };
        let mut bytes = vec![0u8; req.n * 4];
        if input.read_exact(&mut bytes).is_err() {
            return;
        }
        let samples: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let reply = match &mut engine {
            Engine::Whisper(t) => match t.words(&samples, req.prompt.as_deref()) {
                Ok(words) => Reply { words: Some(words), ..Default::default() },
                Err(e) => Reply { error: Some(e.to_string()), ..Default::default() },
            },
            Engine::Vad(v) => match v.segments(&samples) {
                Ok(segments) => Reply { segments: Some(segments), ..Default::default() },
                Err(e) => Reply { error: Some(e.to_string()), ..Default::default() },
            },
        };
        say(&mut out, &reply);
    }
}

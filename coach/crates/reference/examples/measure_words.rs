//! Developer check: measure the stressed vowel of isolated-word recordings.
//!
//! ```sh
//! cargo run -p reference --example measure_words -- heed.wav had.wav
//! ```
//! Each file should hold one word; its name (before .wav) is the word.

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for arg in std::env::args().skip(1) {
        let path = Path::new(&arg);
        let word = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        let audio = audio_io::decode::decode_file(path)?;
        let a = analysis::analyse(&audio)?;
        let span = (a.segments.first().map(|s| s.start).unwrap_or(0.0), a.segments.last().map(|s| s.end).unwrap_or(0.0));
        let input = vowels::WordInput { index: 0, text: word.clone(), span: Some(span), phones: vec![] };
        let toks = vowels::tokens(&audio, vowels::Intensity { db: &a.intensity_db, offset_s: a.frame_offset_s }, &[input]);
        match toks.first() {
            Some(t) => println!(
                "{word:>8} {:<7} onset F1 {:>4.0} F2 {:>4.0} | mid F1 {:>4.0} F2 {:>4.0} F3 {:>4.0} | offset F1 {:>4.0} F2 {:>4.0}{}",
                t.set.code(),
                t.measure.onset.f1,
                t.measure.onset.f2,
                t.measure.mid.f1,
                t.measure.mid.f2,
                t.measure.mid.f3,
                t.measure.offset.f1,
                t.measure.offset.f2,
                t.f3_r.map(|f| format!(" | F3 near R {f:.0}")).unwrap_or_default()
            ),
            None => println!("{word:>8} not measured"),
        }
    }
    Ok(())
}

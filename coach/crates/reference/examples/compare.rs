//! Developer tool: compare an attempt against a reference from the command line.
//!
//! ```sh
//! cargo run -p reference --example compare -- reference.wav attempt.wav "G'day mate, how's it going?"
//! cargo run -p reference --example compare -- --split narration.wav 3
//! ```

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--split") {
        let audio = audio_io::decode::decode_file(Path::new(&args[1]))?;
        let count: usize = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(1);
        for (i, s) in reference::narration::split(&audio, count)?.iter().enumerate() {
            println!("sentence {}: {:.2}–{:.2} s", i + 1, s.start, s.end);
        }
        return Ok(());
    }
    let [reference, attempt, sentence] = args.as_slice() else {
        eprintln!("usage: compare <reference.wav> <attempt.wav> <sentence>  |  compare --split <narration> <count>");
        std::process::exit(2);
    };
    let r = analysis::analyse(&audio_io::decode::decode_file(Path::new(reference))?)?;
    let a = analysis::analyse(&audio_io::decode::decode_file(Path::new(attempt))?)?;
    for (name, x) in [("reference", &r), ("attempt", &a)] {
        println!(
            "{name:>9}: median {:.0} Hz, range {:.1} st, ending {:+.1} st, {} syllables, {:.1} syl/s, {} pauses",
            x.median_f0_hz,
            x.range_st,
            x.final_movement_st.unwrap_or(0.0),
            x.rate.syllables,
            x.rate.articulation_sps,
            x.rate.pause_count
        );
    }
    let c = analysis::compare(&a, &r, Some(reference::text::count_syllables(sentence)))?;
    println!("tune match {:?}, pace ratio {:.2}, range ratio {:.2}", c.contour_corr, c.articulation_ratio, c.range_ratio);
    let input = coach::issues::build_input(sentence, &c, Default::default());
    let reply = coach::rules::coach(&input);
    println!("issues: {}", serde_json::to_string_pretty(&input.issues)?);
    println!("coach: {}", reply.spoken);
    Ok(())
}

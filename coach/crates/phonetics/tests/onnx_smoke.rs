//! Runs only with `--features onnx` and `PHONETICS_TEST_DIR` pointing at a folder holding
//! `wav2vec2-phonemes.onnx`, its vocab and `libonnxruntime.{dylib,so}` (any model with the
//! wav2vec2 interface works, including a tiny stand-in).
#![cfg(feature = "onnx")]

use std::path::PathBuf;

use phonetics::model::PhonemeModel;
use phonetics::{ReferencePhones, ScoreConfig};

#[test]
fn onnx_model_runs_and_scores() {
    let Ok(dir) = std::env::var("PHONETICS_TEST_DIR") else {
        eprintln!("PHONETICS_TEST_DIR not set; skipping");
        return;
    };
    let dir = PathBuf::from(dir);
    let mut model =
        PhonemeModel::load(&dir.join(phonetics::MODEL_FILE), &dir.join(phonetics::VOCAB_FILE), &dir).expect("load model");
    let audio: Vec<f32> = (0..32_000).map(|i| (i as f32 * 0.07).sin() * 0.3 * ((i / 4000) % 2) as f32).collect();
    let post = model.posteriors(&audio).expect("posteriors");
    assert_eq!(post.frames, 100, "2 s at 20 ms per frame");
    assert_eq!(post.vocab, model.vocab.len());
    for t in 0..post.frames {
        let total: f32 = post.row(t).iter().map(|v| v.exp()).sum();
        assert!((total - 1.0).abs() < 1e-3);
    }
    // Whatever the stand-in model "hears", scoring the clip against itself must be near perfect.
    if let Some(r) = ReferencePhones::build(&post, &model.vocab, &["x".into()], &[Some((0.0, 2.0))]) {
        let s = r.score(&post, &model.vocab, &ScoreConfig::default()).expect("score");
        assert!(s.overall > 0.95, "self-score {}", s.overall);
    }
    assert!(model.posteriors(&audio[..800]).is_err());
}

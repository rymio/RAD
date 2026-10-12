//! Runs the real built-in embedding model from the repo's `models/` folder.
//! Skipped when the model hasn't been fetched (`scripts/fetch-models.sh`).

use std::path::PathBuf;

use rad_local::{Library, DEFAULT_EMBEDDING};

fn library() -> Option<std::sync::Arc<Library>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models");
    let lib = Library::new(std::env::temp_dir().join("rad-local-test-models"), vec![dir]);
    if lib.locate(&DEFAULT_EMBEDDING).is_none() {
        eprintln!("skipping: {} not in models/", DEFAULT_EMBEDDING.file);
        return None;
    }
    Some(lib)
}

fn cos(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[tokio::test]
async fn embeds_similar_texts_close_together() {
    let Some(lib) = library() else { return };
    let texts = [
        "search_query: Where does the cat sleep?",
        "search_document: The cat sleeps on the sofa in the living room.",
        "search_document: The quarterly VAT return is due at the end of the month.",
    ]
    .map(String::from)
    .to_vec();
    let v = lib.embed(DEFAULT_EMBEDDING.id, texts).await.unwrap();
    assert_eq!(v.len(), 3);
    assert!(v.iter().all(|e| e.len() == 768));
    assert!((cos(&v[0], &v[0]) - 1.0).abs() < 1e-3, "vectors are normalised");
    assert!(cos(&v[0], &v[1]) > cos(&v[0], &v[2]) + 0.1, "related text ranks higher");
}

#[tokio::test]
async fn handles_long_inputs_and_many_texts() {
    let Some(lib) = library() else { return };
    let long = "search_document: ".to_string() + &"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(800);
    let mut texts = vec![long];
    texts.extend((0..70).map(|i| format!("search_document: passage number {i} about invoices")));
    let v = lib.embed(DEFAULT_EMBEDDING.id, texts).await.unwrap();
    assert_eq!(v.len(), 71);
    assert!(v.iter().all(|e| e.len() == 768 && e.iter().all(|x| x.is_finite())));
}

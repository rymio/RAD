//! Runs a real built-in chat model from the repo's `models/` folder (skipped when absent).
//! Deliberately exits without `shutdown()` while the model is loaded on the GPU: the process
//! must still exit cleanly.

use std::path::PathBuf;
use std::time::Instant;

use rad_local::{ChatEvent, ChatMessage, ChatOptions, Library, CHAT_MODELS};

fn library() -> Option<(std::sync::Arc<Library>, &'static str)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models");
    let lib = Library::new(std::env::temp_dir().join("rad-local-test-models"), vec![dir]);
    let spec = CHAT_MODELS.iter().find(|s| lib.locate(s).is_some())?;
    Some((lib, spec.id))
}

fn msg(role: &str, content: &str) -> ChatMessage {
    ChatMessage { role: role.into(), content: content.into() }
}

async fn answer(lib: &Library, model: &str, messages: Vec<ChatMessage>) -> (String, usize, usize) {
    let mut rx = lib.chat(model, messages, ChatOptions { max_tokens: 200, temperature: Some(0.2) }).unwrap();
    let (mut text, mut tin, mut tout) = (String::new(), 0, 0);
    while let Some(ev) = rx.recv().await {
        match ev.unwrap() {
            ChatEvent::Text(t) => text.push_str(&t),
            ChatEvent::Done { prompt_tokens, completion_tokens } => (tin, tout) = (prompt_tokens, completion_tokens),
        }
    }
    (text, tin, tout)
}

#[tokio::test]
async fn answers_from_sources_and_fits_long_prompts() {
    let Some((lib, model)) = library() else {
        eprintln!("skipping: no built-in chat model in models/");
        return;
    };
    let system = "You are RAD. Answer only from the numbered sources and cite them like [1].\n\n\
                  [1] Garden notes: Plant the San Marzano tomatoes after the last frost, around 10 May.\n\
                  [2] Car: The insurance renewal is due in March.";
    let start = Instant::now();
    let (text, tin, tout) = answer(&lib, model, vec![msg("system", system), msg("user", "When should I plant the tomatoes?")]).await;
    let secs = start.elapsed().as_secs_f64();
    println!("{model}: {tin} prompt + {tout} answer tokens in {secs:.1}s ({:.0} tok/s overall)\n{text}", tout as f64 / secs);
    assert!(text.contains("May") || text.contains("frost"), "{text}");
    assert!(text.contains("[1]"), "cites its source: {text}");

    // Far more than the context holds: old turns are dropped and the sources shortened.
    let huge = format!("{system}\n\n{}", "[3] Filler passage about unrelated household paperwork. ".repeat(6000));
    let mut messages = vec![msg("system", &huge)];
    for i in 0..10 {
        messages.push(msg("user", &format!("Earlier question {i}")));
        messages.push(msg("assistant", &format!("Earlier answer {i}")));
    }
    messages.push(msg("user", "When should I plant the tomatoes? Answer in one sentence."));
    let (text, tin, _) = answer(&lib, model, messages).await;
    println!("long prompt trimmed to {tin} tokens: {text}");
    assert!(!text.trim().is_empty());
}

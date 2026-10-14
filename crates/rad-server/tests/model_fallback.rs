//! Switching a provider off (or removing it) moves every role that used it to what's still
//! available — embeddings to the built-in llama.cpp model — instead of leaving documents with
//! "Semantic index unavailable".

use rad_server::embedded::Embedded;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_provider_falls_back_to_builtin_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")];
    let emb = Embedded::start(dir.path().join("v.radvault"), "V", None, dir.path().join("models"), bundled).await.unwrap();
    let http = reqwest::Client::new();
    let (base, token) = (emb.url(), emb.device_token.clone());
    let r: Value = http.post(format!("{base}/v1/vault/init")).bearer_auth(&token).json(&json!({ "passphrase": "fallback test pass" })).send().await.unwrap().json().await.unwrap();
    let session = r["session"].as_str().unwrap().to_string();
    let call = |m: reqwest::Method, path: &str, body: Option<Value>| {
        let rb = http.request(m, format!("{base}{path}")).bearer_auth(&token).header("x-rad-session", &session);
        match body {
            Some(b) => rb.json(&b),
            None => rb,
        }
    };
    let get = |p: &'static str| call(reqwest::Method::GET, p, None).send();

    let builtin: Value = get("/v1/providers").await.unwrap().json::<Value>().await.unwrap().as_array().unwrap().iter().find(|p| p["kind"] == "builtin").cloned().unwrap();
    let lm: Value = call(reqwest::Method::POST, "/v1/providers", Some(json!({ "kind": "openai", "name": "LM Studio", "base_url": "http://127.0.0.1:9/v1" }))).send().await.unwrap().json().await.unwrap();
    let on_lm = |model: &str| json!({ "provider_id": lm["id"], "model": model });
    let resp = call(
        reqwest::Method::PUT,
        "/v1/settings",
        Some(json!({ "chat_model": on_lm("chat"), "embedding_model": on_lm("embed"), "vision_model": on_lm("vl"), "rewrite_model": on_lm("chat") })),
    )
    .send()
    .await
    .unwrap();
    assert!(resp.status().is_success());
    let chat: Value = call(reqwest::Method::POST, "/v1/chats", Some(json!({ "model": on_lm("chat") }))).send().await.unwrap().json().await.unwrap();

    // Switch LM Studio off.
    let resp = call(reqwest::Method::PATCH, &format!("/v1/providers/{}", lm["id"]), Some(json!({ "enabled": false }))).send().await.unwrap();
    assert!(resp.status().is_success());

    let s: Value = get("/v1/settings").await.unwrap().json().await.unwrap();
    assert_eq!(s["embedding_model"], json!({ "provider_id": builtin["id"], "model": "nomic-embed-text-v1.5-q8" }), "{s}");
    assert!(s["vision_model"].is_null(), "{s}");
    assert!(s["rewrite_model"].is_null(), "{s}");
    // Chat moves to a downloaded built-in model when there is one, else it's unset.
    assert!(s["chat_model"].is_null() || s["chat_model"]["provider_id"] == builtin["id"], "{s}");
    let c: Value = call(reqwest::Method::GET, &format!("/v1/chats/{}", chat["id"]), None).send().await.unwrap().json().await.unwrap();
    assert!(c["chat"]["model"].is_null(), "per-chat choice cleared: {c}");

    // A note added now gets its semantic index from the built-in model.
    let p: Value = call(reqwest::Method::POST, "/v1/projects", Some(json!({ "name": "P" }))).send().await.unwrap().json().await.unwrap();
    call(reqwest::Method::POST, &format!("/v1/projects/{}/notes", p["id"]), Some(json!({ "title": "SSH", "content": "coursemat.au is reached as deploy@coursemat.au on port 2222." }))).send().await.unwrap();
    for _ in 0..120 {
        let j: Value = get("/v1/jobs").await.unwrap().json().await.unwrap();
        if j["queued"] == 0 && j["running"] == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let docs: Value = call(reqwest::Method::GET, &format!("/v1/projects/{}/documents", p["id"]), None).send().await.unwrap().json().await.unwrap();
    let doc = &docs.as_array().unwrap()[0];
    assert!(doc["notice"].is_null(), "no 'Semantic index unavailable': {doc}");
    let info: Value = get("/v1/vault/info").await.unwrap().json().await.unwrap();
    assert!(info["embedded_chunks"].as_i64().unwrap() >= 1, "{info}");
    emb.stop().await;
}

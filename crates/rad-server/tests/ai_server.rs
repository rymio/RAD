//! The AI API server: switched on from the settings, it shares RAD's models in the OpenAI,
//! Anthropic and Ollama dialects, asks for its API keys, and counts what's used.

use axum::routing::{get, post};
use axum::{Json, Router};
use rad_server::embedded::Embedded;
use serde_json::{json, Value};

/// A fake OpenAI-compatible provider that answers "Hello there" (streamed in two pieces).
async fn fake_provider() -> String {
    let app = Router::new()
        .route("/v1/models", get(|| async { Json(json!({"data": [{"id": "fake-chat"}]})) }))
        .route(
            "/v1/chat/completions",
            post(|Json(_): Json<Value>| async {
                let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" there\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";
                ([("content-type", "text/event-stream")], sse)
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn sse_data(text: &str) -> Vec<Value> {
    text.lines().filter_map(|l| l.strip_prefix("data: ")).filter_map(|d| serde_json::from_str(d).ok()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shares_models_in_three_dialects() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")];
    let emb = Embedded::start(dir.path().join("v.radvault"), "Studio", None, dir.path().join("models"), bundled, None, None).await.unwrap();
    let http = reqwest::Client::new();
    let (base, token) = (emb.url(), emb.device_token.clone());
    let r: Value = http.post(format!("{base}/v1/vault/init")).bearer_auth(&token).json(&json!({ "passphrase": "ai server test" })).send().await.unwrap().json().await.unwrap();
    let session = r["session"].as_str().unwrap().to_string();
    let call = |m: reqwest::Method, path: &str, body: Value| http.request(m, format!("{base}{path}")).bearer_auth(&token).header("x-rad-session", &session).json(&body);

    let url = fake_provider().await;
    call(reqwest::Method::POST, "/v1/providers", json!({ "kind": "openai", "name": "Fake Cloud", "base_url": url })).send().await.unwrap();

    // Off until switched on.
    let s: Value = call(reqwest::Method::GET, "/v1/ai-server", json!(null)).send().await.unwrap().json().await.unwrap();
    assert_eq!(s["running"], false);
    assert_eq!(s["config"]["require_key"], true);

    let port = free_port();
    let s: Value = call(reqwest::Method::PUT, "/v1/ai-server", json!({ "enabled": true, "port": port, "share_providers": true })).send().await.unwrap().json().await.unwrap();
    assert_eq!(s["running"], true, "{s}");
    let names: Vec<&str> = s["models"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"fake-cloud/fake-chat"), "{names:?}");
    assert!(names.contains(&"nomic-embed-text-v1.5-q8"), "{names:?}");

    let ai = format!("http://127.0.0.1:{port}");
    // A key is needed.
    let r = http.get(format!("{ai}/v1/models")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r: Value = http.post(format!("{ai}/api/chat")).bearer_auth("radai_nope").body("{}").send().await.unwrap().json().await.unwrap();
    assert_eq!(r["error"], "unknown or deleted API key");
    let k: Value = call(reqwest::Method::POST, "/v1/ai-server/keys", json!({ "name": "Laptop" })).send().await.unwrap().json().await.unwrap();
    let key = k["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("radai_"));

    // OpenAI
    let m: Value = http.get(format!("{ai}/v1/models")).bearer_auth(&key).send().await.unwrap().json().await.unwrap();
    assert_eq!(m["object"], "list");
    assert!(m["data"].as_array().unwrap().iter().any(|x| x["id"] == "fake-cloud/fake-chat" && x["owned_by"] == "Fake Cloud"), "{m}");
    let msgs = json!([{"role": "system", "content": "Be kind."}, {"role": "user", "content": "Hi"}]);
    let r: Value = http.post(format!("{ai}/v1/chat/completions")).bearer_auth(&key).json(&json!({"model": "fake-cloud/fake-chat", "messages": msgs})).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["choices"][0]["message"]["content"], "Hello there", "{r}");
    assert_eq!(r["usage"]["prompt_tokens"], 11);
    assert_eq!(r["usage"]["completion_tokens"], 2);
    let text = http
        .post(format!("{ai}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&json!({"model": "fake-chat", "messages": msgs, "stream": true, "stream_options": {"include_usage": true}}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.trim_end().ends_with("data: [DONE]"), "{text}");
    let evs = sse_data(&text);
    let said: String = evs.iter().filter_map(|e| e["choices"][0]["delta"]["content"].as_str()).collect();
    assert_eq!(said, "Hello there");
    assert_eq!(evs.iter().find(|e| e["choices"][0]["finish_reason"] == "stop").map(|_| true), Some(true));
    assert_eq!(evs.last().unwrap()["usage"]["total_tokens"], 13);
    let r = http.post(format!("{ai}/v1/chat/completions")).bearer_auth(&key).json(&json!({"model": "nope", "messages": msgs})).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r: Value = r.json().await.unwrap();
    assert_eq!(r["error"]["type"], "not_found_error");

    // Anthropic (with x-api-key)
    let body = json!({"model": "fake-cloud/fake-chat", "max_tokens": 64, "system": "Be kind.", "messages": [{"role": "user", "content": [{"type": "text", "text": "Hi"}]}]});
    let r: Value = http.post(format!("{ai}/v1/messages")).header("x-api-key", &key).header("anthropic-version", "2023-06-01").json(&body).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["type"], "message");
    assert_eq!(r["content"][0]["text"], "Hello there", "{r}");
    assert_eq!(r["stop_reason"], "end_turn");
    let mut sb = body.clone();
    sb["stream"] = json!(true);
    let text = http.post(format!("{ai}/v1/messages")).header("x-api-key", &key).json(&sb).send().await.unwrap().text().await.unwrap();
    let names: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("event: ")).collect();
    assert_eq!(names, ["message_start", "content_block_start", "ping", "content_block_delta", "content_block_delta", "content_block_stop", "message_delta", "message_stop"], "{text}");
    let m: Value = http.get(format!("{ai}/v1/models")).header("x-api-key", &key).header("anthropic-version", "2023-06-01").send().await.unwrap().json().await.unwrap();
    assert!(m["data"].as_array().unwrap().iter().all(|x| x["type"] == "model" && x["id"] != "nomic-embed-text-v1.5-q8"), "{m}");

    // Ollama (streams JSON lines by default)
    let t: Value = http.get(format!("{ai}/api/tags")).bearer_auth(&key).send().await.unwrap().json().await.unwrap();
    let nomic = t["models"].as_array().unwrap().iter().find(|m| m["name"] == "nomic-embed-text-v1.5-q8").unwrap();
    assert_eq!(nomic["details"]["quantization_level"], "Q8_0");
    let text = http.post(format!("{ai}/api/chat")).bearer_auth(&key).body(json!({"model": "fake-cloud/fake-chat:latest", "messages": [{"role": "user", "content": "Hi"}]}).to_string()).send().await.unwrap().text().await.unwrap();
    let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let said: String = lines.iter().filter_map(|l| l["message"]["content"].as_str()).collect();
    assert_eq!(said, "Hello there");
    assert_eq!(lines.last().unwrap()["done"], true);
    assert_eq!(lines.last().unwrap()["eval_count"], 2);
    let r: Value = http.post(format!("{ai}/api/generate")).bearer_auth(&key).json(&json!({"model": "fake-cloud/fake-chat", "prompt": "Hi", "stream": false})).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["response"], "Hello there", "{r}");
    let r: Value = http.post(format!("{ai}/api/show")).bearer_auth(&key).json(&json!({"model": "nomic-embed-text-v1.5-q8"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["capabilities"], json!(["embedding"]));

    // Built-in embeddings, for real.
    let r: Value = http.post(format!("{ai}/v1/embeddings")).bearer_auth(&key).json(&json!({"model": "nomic-embed-text-v1.5-q8", "input": ["one", "two"]})).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["data"].as_array().map(Vec::len), Some(2), "{r}");
    assert_eq!(r["data"][1]["embedding"].as_array().map(Vec::len), Some(768));
    let r: Value = http.post(format!("{ai}/api/embed")).bearer_auth(&key).json(&json!({"model": "nomic-embed-text-v1.5-q8", "input": "one"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(r["embeddings"][0].as_array().map(Vec::len), Some(768), "{r}");
    let r = http.post(format!("{ai}/v1/embeddings")).bearer_auth(&key).json(&json!({"model": "qwen3-4b-instruct-2507-q4", "input": "x"})).send().await.unwrap();
    assert!(r.status() == 400 || r.status() == 404, "{}", r.status());

    // Statistics, for programs and for the settings.
    let stats: Value = http.get(format!("{ai}/rad/stats")).bearer_auth(&key).send().await.unwrap().json().await.unwrap();
    let fake = stats["models"].as_array().unwrap().iter().find(|m| m["name"] == "fake-cloud/fake-chat").unwrap();
    assert_eq!(fake["usage"]["requests"], 6, "{stats}");
    assert_eq!(fake["usage"]["tokens_out"], 12);
    assert_eq!(stats["apis"]["ollama"], 3);
    let s: Value = call(reqwest::Method::GET, "/v1/ai-server", json!(null)).send().await.unwrap().json().await.unwrap();
    assert_eq!(s["keys"][0]["name"], "Laptop");
    assert_eq!(s["keys"][0]["usage"]["requests"], 8, "{s}");
    assert!(s["urls"][0].as_str().unwrap().ends_with(&format!(":{port}")));

    // Hidden models aren't shared; a deleted key stops working; without keys, web pages are turned away.
    call(reqwest::Method::PUT, "/v1/ai-server", json!({ "enabled": true, "port": port, "share_providers": true, "hidden": ["fake-cloud/fake-chat"] })).send().await.unwrap();
    let r = http.post(format!("{ai}/v1/chat/completions")).bearer_auth(&key).json(&json!({"model": "fake-cloud/fake-chat", "messages": msgs})).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let id = s["keys"][0]["id"].as_str().unwrap();
    call(reqwest::Method::DELETE, &format!("/v1/ai-server/keys/{id}"), json!(null)).send().await.unwrap();
    assert_eq!(http.get(format!("{ai}/api/tags")).bearer_auth(&key).send().await.unwrap().status(), 401);
    call(reqwest::Method::PUT, "/v1/ai-server", json!({ "enabled": true, "port": port, "require_key": false })).send().await.unwrap();
    assert_eq!(http.get(format!("{ai}/api/tags")).send().await.unwrap().status(), 200);
    assert_eq!(http.get(format!("{ai}/api/tags")).header("origin", "https://evil.example").send().await.unwrap().status(), 403);
    assert_eq!(http.get(format!("{ai}/api/tags")).header("origin", "http://localhost:3000").send().await.unwrap().status(), 200);

    // Locking the vault stops it; unlocking starts it again.
    call(reqwest::Method::POST, "/v1/vault/lock", json!({})).send().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(http.get(format!("{ai}/")).send().await.is_err());
    let r: Value = http.post(format!("{base}/v1/unlock")).bearer_auth(&token).json(&json!({ "passphrase": "ai server test" })).send().await.unwrap().json().await.unwrap();
    assert!(r["session"].is_string(), "{r}");
    let mut up = false;
    for _ in 0..40 {
        if http.get(format!("{ai}/")).send().await.is_ok() {
            up = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(up, "the AI server didn't start again after unlocking");

    // Off.
    let session = r["session"].as_str().unwrap().to_string();
    http.put(format!("{base}/v1/ai-server")).bearer_auth(&token).header("x-rad-session", &session).json(&json!({ "enabled": false, "port": port })).send().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(http.get(format!("{ai}/")).send().await.is_err());
    emb.stop().await;
}

/// Needs the bundled Qwen3 4B model (2.5 GB, in the repository's `models` folder):
/// cargo test -p rad-server --test ai_server -- --ignored
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn builtin_model_answers() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")];
    let emb = Embedded::start(dir.path().join("v.radvault"), "Studio", None, dir.path().join("models"), bundled, None, None).await.unwrap();
    let http = reqwest::Client::new();
    let (base, token) = (emb.url(), emb.device_token.clone());
    let r: Value = http.post(format!("{base}/v1/vault/init")).bearer_auth(&token).json(&json!({ "passphrase": "ai server test" })).send().await.unwrap().json().await.unwrap();
    let session = r["session"].as_str().unwrap().to_string();
    let port = free_port();
    http.put(format!("{base}/v1/ai-server")).bearer_auth(&token).header("x-rad-session", &session).json(&json!({ "enabled": true, "port": port, "require_key": false })).send().await.unwrap();
    let ai = format!("http://127.0.0.1:{port}");
    let model = "qwen3-4b-instruct-2507-q4";

    let r: Value = http
        .post(format!("{ai}/v1/chat/completions"))
        .json(&json!({"model": model, "max_tokens": 20, "temperature": 0, "messages": [{"role": "user", "content": "What is the capital of France? Answer with one word."}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let answer = r["choices"][0]["message"]["content"].as_str().unwrap_or_default();
    assert!(answer.contains("Paris"), "{r}");
    assert!(r["usage"]["prompt_tokens"].as_u64().unwrap() > 5, "{r}");

    let ps: Value = http.get(format!("{ai}/api/ps")).send().await.unwrap().json().await.unwrap();
    assert_eq!(ps["models"][0]["name"], model, "{ps}");

    let text = http
        .post(format!("{ai}/api/chat"))
        .json(&json!({"model": model, "options": {"num_predict": 20, "temperature": 0}, "messages": [{"role": "user", "content": "Say the word hello."}]}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let said: String = lines.iter().filter_map(|l| l["message"]["content"].as_str()).collect();
    assert!(said.to_lowercase().contains("hello"), "{text}");
    assert!(lines.len() > 2, "not streamed: {text}");

    let stats: Value = http.get(format!("{ai}/rad/stats")).send().await.unwrap().json().await.unwrap();
    let q = stats["models"].as_array().unwrap().iter().find(|m| m["name"] == model).unwrap();
    assert_eq!(q["usage"]["requests"], 2);
    assert!(q["usage"]["tokens_per_second"].as_f64().unwrap() > 0.0, "{stats}");
    println!("{}", serde_json::to_string_pretty(q).unwrap());
    emb.stop().await;
}

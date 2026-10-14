//! Per-message context from the web browser: page text and a screenshot reach the model with the
//! question, the screenshot routes the answer to the vision model, and none of it is stored.

use std::sync::{Arc, Mutex};

use axum::routing::{get, post};
use axum::{Json, Router};
use rad_server::embedded::Embedded;
use serde_json::{json, Value};

/// 1×1 PNG.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

/// A fake OpenAI-compatible provider that records every chat request and answers "ok".
async fn fake_provider() -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let app = Router::new()
        .route("/v1/models", get(|| async { Json(json!({ "data": [{ "id": "text-model" }, { "id": "vision-model" }] })) }))
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let s = s.clone();
                async move {
                    s.lock().unwrap().push(body);
                    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
                    ([("content-type", "text/event-stream")], sse)
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/v1"), seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn page_context_and_screenshot_reach_the_model_but_are_not_stored() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")];
    let emb = Embedded::start(dir.path().join("v.radvault"), "V", None, dir.path().join("models"), bundled, None, None).await.unwrap();
    let http = reqwest::Client::new();
    let token = emb.device_token.clone();
    let base = emb.url();
    let r: Value = http.post(format!("{base}/v1/vault/init")).bearer_auth(&token).json(&json!({ "passphrase": "page context test" })).send().await.unwrap().json().await.unwrap();
    let session = r["session"].as_str().unwrap().to_string();
    let call = |m: reqwest::Method, path: &str, body: Value| http.request(m, format!("{base}{path}")).bearer_auth(&token).header("x-rad-session", &session).json(&body);

    let (url, seen) = fake_provider().await;
    let prov: Value = call(reqwest::Method::POST, "/v1/providers", json!({ "kind": "openai", "name": "Fake", "base_url": url })).send().await.unwrap().json().await.unwrap();
    let settings: Value = call(reqwest::Method::GET, "/v1/settings", json!(null)).send().await.unwrap().json().await.unwrap();
    let resp = call(
        reqwest::Method::PUT,
        "/v1/settings",
        json!({
            "chat_model": { "provider_id": prov["id"], "model": "text-model" },
            "vision_model": { "provider_id": prov["id"], "model": "vision-model" },
            "embedding_model": settings["embedding_model"],
        }),
    )
    .send()
    .await
    .unwrap();
    assert!(resp.status().is_success(), "{}", resp.text().await.unwrap());
    let chat: Value = call(reqwest::Method::POST, "/v1/chats", json!({ "title": "Web · example.com", "mode": "chat_only" })).send().await.unwrap().json().await.unwrap();

    let send = |body: Value| call(reqwest::Method::POST, &format!("/v1/chats/{}/messages", chat["id"]), body).send();
    let done = |text: String| {
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .find(|e| e["type"] == "done")
            .unwrap_or_else(|| panic!("no done event: {text}"))
    };

    // With a screenshot: the vision model answers and sees the text, the question and the image.
    let d = done(send(json!({ "content": "What colour is the button?", "context": "PAGE TEXT:\nBuy now — 19 EUR", "images": [{ "mime": "image/png", "data": PIXEL }] })).await.unwrap().text().await.unwrap());
    assert!(d["message"]["error"].is_null(), "{d}");
    assert_eq!(d["message"]["model"], "vision-model");
    {
        let req = seen.lock().unwrap().last().cloned().unwrap();
        assert_eq!(req["model"], "vision-model");
        let user = req["messages"].as_array().unwrap().last().unwrap();
        let parts = user["content"].as_array().expect("text + image parts");
        let text = parts[0]["text"].as_str().unwrap();
        assert!(text.contains("Buy now — 19 EUR") && text.contains("What colour is the button?"), "{text}");
        assert!(parts[1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/jpeg;base64,"), "{parts:?}");
    }

    // Text only: the chat model answers.
    let d = done(send(json!({ "content": "Summarise the page", "context": "PAGE TEXT:\nOpening hours 9–17" })).await.unwrap().text().await.unwrap());
    assert_eq!(d["message"]["model"], "text-model");
    {
        let req = seen.lock().unwrap().last().cloned().unwrap();
        let content = req["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().to_string();
        assert!(content.contains("Opening hours 9–17") && content.contains("Summarise the page"), "{content}");
        // Earlier turns go back as plain history, without the old page.
        assert!(!serde_json::to_string(&req["messages"]).unwrap().contains("Buy now"), "{req}");
    }

    // Stored messages hold only what the user typed.
    let full: Value = call(reqwest::Method::GET, &format!("/v1/chats/{}", chat["id"]), json!(null)).send().await.unwrap().json().await.unwrap();
    let users: Vec<&str> = full["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "user").map(|m| m["content"].as_str().unwrap()).collect();
    assert_eq!(users, ["What colour is the button?", "Summarise the page"]);

    // Garbage image data is refused up front.
    let resp = send(json!({ "content": "x", "images": [{ "mime": "image/png", "data": "%%%" }] })).await.unwrap();
    assert_eq!(resp.status(), 400);

    emb.stop().await;
}

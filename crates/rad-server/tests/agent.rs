//! "open ssh to coursemat.au": the chat looks up the host's details in the vault, tells the
//! app to open the session, and remembers the details for next time.

use std::sync::{Arc, Mutex};

use axum::routing::post;
use axum::{Json, Router};
use rad_server::embedded::Embedded;
use serde_json::{json, Value};

/// Fake model: planning requests get an SSH action, everything else "ok".
async fn fake_provider() -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let s = s.clone();
            async move {
                let planning = body["messages"][0]["content"].as_str().unwrap_or("").contains("ONE action");
                s.lock().unwrap().push(body);
                let text = if planning { r#"{\"kind\":\"ssh\",\"host\":\"coursemat.au\",\"user\":\"deploy\",\"port\":2222,\"identity_file\":\"~/.ssh/coursemat\",\"sources\":[1]}"# } else { "ok" };
                let sse = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\ndata: [DONE]\n\n");
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
async fn ssh_request_uses_the_vault_and_is_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")];
    let emb = Embedded::start(dir.path().join("v.radvault"), "V", None, dir.path().join("models"), bundled).await.unwrap();
    let http = reqwest::Client::new();
    let (base, token) = (emb.url(), emb.device_token.clone());
    let r: Value = http.post(format!("{base}/v1/vault/init")).bearer_auth(&token).json(&json!({ "passphrase": "agent test pass" })).send().await.unwrap().json().await.unwrap();
    let session = r["session"].as_str().unwrap().to_string();
    let call = |m: reqwest::Method, path: &str, body: Value| http.request(m, format!("{base}{path}")).bearer_auth(&token).header("x-rad-session", &session).json(&body);

    let (url, seen) = fake_provider().await;
    let prov: Value = call(reqwest::Method::POST, "/v1/providers", json!({ "kind": "openai", "name": "Fake", "base_url": url })).send().await.unwrap().json().await.unwrap();
    let settings: Value = call(reqwest::Method::GET, "/v1/settings", json!(null)).send().await.unwrap().json().await.unwrap();
    call(reqwest::Method::PUT, "/v1/settings", json!({ "chat_model": { "provider_id": prov["id"], "model": "m" }, "embedding_model": settings["embedding_model"] })).send().await.unwrap();
    let p: Value = call(reqwest::Method::POST, "/v1/projects", json!({ "name": "Servers" })).send().await.unwrap().json().await.unwrap();
    call(reqwest::Method::POST, &format!("/v1/projects/{}/notes", p["id"]), json!({ "title": "Coursemat server", "content": "coursemat.au — log in as deploy on SSH port 2222 with the key ~/.ssh/coursemat." })).send().await.unwrap();
    for _ in 0..120 {
        let j: Value = call(reqwest::Method::GET, "/v1/jobs", json!(null)).send().await.unwrap().json().await.unwrap();
        if j["queued"] == 0 && j["running"] == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    let chat: Value = call(reqwest::Method::POST, "/v1/chats", json!({ "mode": "chat_only" })).send().await.unwrap().json().await.unwrap();
    let events = |text: String| text.lines().filter_map(|l| l.strip_prefix("data:")).filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok()).collect::<Vec<_>>();
    let text = call(reqwest::Method::POST, &format!("/v1/chats/{}/messages", chat["id"]), json!({ "content": "open ssh to coursemat.au" })).send().await.unwrap().text().await.unwrap();
    let evs = events(text);
    let action = evs.iter().find(|e| e["type"] == "action").unwrap_or_else(|| panic!("no action: {evs:?}"));
    assert_eq!(action["action"], json!({ "kind": "ssh", "host": "coursemat.au", "user": "deploy", "port": 2222, "identity_file": "~/.ssh/coursemat" }));
    let done = evs.iter().find(|e| e["type"] == "done").unwrap();
    let reply = done["message"]["content"].as_str().unwrap();
    assert!(reply.contains("deploy@coursemat.au") && reply.contains("2222"), "{reply}");
    assert_eq!(done["message"]["citations"][0]["title"], "Coursemat server", "{done}");

    // The planner was shown the note.
    let plan_prompt = seen.lock().unwrap()[0]["messages"][1]["content"].as_str().unwrap().to_string();
    assert!(plan_prompt.contains("log in as deploy"), "{plan_prompt}");

    // Remembered, once.
    let mems: Value = call(reqwest::Method::GET, "/v1/memories", json!(null)).send().await.unwrap().json().await.unwrap();
    let ssh: Vec<&Value> = mems.as_array().unwrap().iter().filter(|m| m["text"].as_str().unwrap().starts_with("SSH to coursemat.au:")).collect();
    assert_eq!(ssh.len(), 1, "{mems}");
    assert_eq!(ssh[0]["text"], "SSH to coursemat.au: connect as deploy@coursemat.au, port 2222, key ~/.ssh/coursemat");
    let text = call(reqwest::Method::POST, &format!("/v1/chats/{}/messages", chat["id"]), json!({ "content": "connect to coursemat.au again" })).send().await.unwrap().text().await.unwrap();
    assert!(events(text).iter().any(|e| e["type"] == "action"));
    let last_plan = seen.lock().unwrap().last().unwrap()["messages"][1]["content"].as_str().unwrap().to_string();
    assert!(last_plan.contains("MEMORIES:\n- SSH to coursemat.au"), "{last_plan}");
    let mems: Value = call(reqwest::Method::GET, "/v1/memories", json!(null)).send().await.unwrap().json().await.unwrap();
    assert_eq!(mems.as_array().unwrap().len(), 1, "{mems}");

    // Ordinary questions are answered normally.
    let text = call(reqwest::Method::POST, &format!("/v1/chats/{}/messages", chat["id"]), json!({ "content": "What port does coursemat use?" })).send().await.unwrap().text().await.unwrap();
    let evs = events(text);
    assert!(!evs.iter().any(|e| e["type"] == "action"));
    assert_eq!(evs.iter().find(|e| e["type"] == "done").unwrap()["message"]["content"], "ok");
    emb.stop().await;
}

//! Standalone mode: the exact code path the desktop app embeds.

use rad_server::embedded::Embedded;
use serde_json::{json, Value};

struct C {
    http: reqwest::Client,
    base: String,
    token: String,
    session: Option<String>,
}

impl C {
    async fn req(&self, m: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self.http.request(m, format!("{}{}", self.base, path)).bearer_auth(&self.token);
        if let Some(s) = &self.session {
            r = r.header("x-rad-session", s);
        }
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let code = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (code, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    async fn get(&self, p: &str) -> Value {
        let (c, v) = self.req(reqwest::Method::GET, p, None).await;
        assert!(c < 300, "GET {p} -> {c} {v}");
        v
    }
    async fn post(&self, p: &str, b: Value) -> Value {
        let (c, v) = self.req(reqwest::Method::POST, p, Some(b)).await;
        assert!(c < 300, "POST {p} -> {c} {v}");
        v
    }
}

async fn wait_jobs(c: &C) {
    for _ in 0..120 {
        let j = c.get("/v1/jobs").await;
        if j["queued"].as_i64() == Some(0) && j["running"].as_i64() == Some(0) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("jobs did not finish");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standalone_vault_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("My Vault.radvault");
    let pass = "standalone passphrase 1";

    // ---- first run: create
    let emb = Embedded::start(vault.clone(), "My Vault", None).await.unwrap();
    assert!(emb.addr.ip().is_loopback(), "standalone server must only listen on loopback");
    let mut c = C { http: reqwest::Client::new(), base: emb.url(), token: emb.device_token.clone(), session: None };
    let st = c.get("/v1/status").await;
    assert_eq!(st["initialized"], false);
    assert!(st["device"].is_object(), "embedded device token is accepted");

    let r = c.post("/v1/vault/init", json!({ "passphrase": pass })).await;
    assert!(r["recovery_key"].as_str().unwrap().starts_with("RAD1-"));
    c.session = Some(r["session"].as_str().unwrap().to_string());

    let p = c.post("/v1/projects", json!({ "name": "Garden" })).await;
    let pid = p["id"].as_i64().unwrap();
    c.post(&format!("/v1/projects/{pid}/notes"), json!({ "title": "Tomatoes", "content": "Plant the San Marzano tomatoes after the last frost, around 10 May." })).await;
    wait_jobs(&c).await;
    let hits = c.get("/v1/search?q=when%20to%20plant%20tomatoes").await;
    assert_eq!(hits[0]["title"], "Tomatoes", "{hits}");

    // Optional: full RAG answer through a local LM Studio, if one is running.
    let lm = reqwest::Client::new().get("http://localhost:1234/v1/models").timeout(std::time::Duration::from_secs(2)).send().await;
    if lm.is_ok_and(|r| r.status().is_success()) {
        let prov = c.post("/v1/providers", json!({ "kind": "openai", "name": "LM Studio", "base_url": "http://localhost:1234/v1" })).await;
        let id = prov["id"].as_i64().unwrap();
        let (code, _) = c
            .req(reqwest::Method::PUT, "/v1/settings", Some(json!({ "chat_model": { "provider_id": id, "model": "google/gemma-4-e4b" } })))
            .await;
        assert_eq!(code, 200);
        let chat = c.post("/v1/chats", json!({})).await;
        let resp = c
            .http
            .post(format!("{}/v1/chats/{}/messages", c.base, chat["id"]))
            .bearer_auth(&c.token)
            .header("x-rad-session", c.session.clone().unwrap())
            .json(&json!({ "content": "When should I plant the tomatoes?" }))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let done: Value = resp
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .find(|e| e["type"] == "done")
            .expect("done event");
        let answer = done["message"]["content"].as_str().unwrap().to_lowercase();
        println!("standalone answer: {answer}");
        assert!(answer.contains("may") || answer.contains("frost"), "{answer}");
        assert!(!done["message"]["citations"].as_array().unwrap().is_empty());
    } else {
        println!("LM Studio not running — skipped the model part");
    }

    emb.stop().await;
    assert!(vault.is_file());
    // A cleanly stopped vault is a single file (WAL checkpointed away).
    let wal = std::path::PathBuf::from(format!("{}-wal", vault.display()));
    assert!(!wal.exists() || std::fs::metadata(&wal).unwrap().len() == 0, "WAL left behind");
    let raw = std::fs::read(&vault).unwrap();
    assert!(!raw.windows(11).any(|w| w == b"San Marzano"), "plaintext in vault file");

    // ---- second run: same file, starts locked, unlock with passphrase
    let emb = Embedded::start(vault.clone(), "My Vault", None).await.unwrap();
    let mut c = C { http: reqwest::Client::new(), base: emb.url(), token: emb.device_token.clone(), session: None };
    let st = c.get("/v1/status").await;
    assert_eq!(st["initialized"], true);
    assert_eq!(st["locked"], true);
    let (code, _) = c.req(reqwest::Method::POST, "/v1/unlock", Some(json!({ "passphrase": "wrong one!!" }))).await;
    assert_eq!(code, 401);
    let r = c.post("/v1/unlock", json!({ "passphrase": pass })).await;
    c.session = Some(r["session"].as_str().unwrap().to_string());
    let projects = c.get("/v1/projects").await;
    assert_eq!(projects[0]["name"], "Garden");

    // Lock drops the key; the old session no longer works.
    c.post("/v1/vault/lock", json!({})).await;
    let (code, v) = c.req(reqwest::Method::GET, "/v1/projects", None).await;
    assert_eq!(code, 423, "{v}");
    emb.stop().await;
}

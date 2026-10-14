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

/// Downloads go to a throwaway folder; the repo's `models/` (scripts/fetch-models.sh) is used
/// read-only so the test doesn't fetch the embedding model every run.
fn models(dir: &std::path::Path) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
    (dir.join("models"), vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")])
}

/// Sends a chat message and returns the final `done` event.
async fn ask(c: &C, chat: &Value, content: &str) -> Value {
    let resp = c
        .http
        .post(format!("{}/v1/chats/{}/messages", c.base, chat["id"]))
        .bearer_auth(&c.token)
        .header("x-rad-session", c.session.clone().unwrap())
        .json(&json!({ "content": content }))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    resp.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(|e| e["type"] == "done")
        .unwrap_or_else(|| panic!("no done event: {resp}"))
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
    let emb = {
        let (md, bundled) = models(dir.path());
        Embedded::start(vault.clone(), "My Vault", None, md, bundled, None, None).await.unwrap()
    };
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
    // Every vault gets the built-in engine and uses its embedding model by default.
    let providers = c.get("/v1/providers").await;
    let builtin = providers.as_array().unwrap().iter().find(|p| p["kind"] == "builtin").expect("built-in provider");
    let settings = c.get("/v1/settings").await;
    assert_eq!(settings["embedding_model"]["provider_id"], builtin["id"]);
    assert_eq!(settings["embedding_model"]["model"], "nomic-embed-text-v1.5-q8");
    let (code, _) = c.req(reqwest::Method::DELETE, &format!("/v1/providers/{}", builtin["id"]), None).await;
    assert_eq!(code, 400, "the built-in provider can't be removed");
    let local = c.get("/v1/local-models").await;
    assert_eq!(local["provider_id"], builtin["id"]);
    let nomic = local["models"].as_array().unwrap().iter().find(|m| m["id"] == "nomic-embed-text-v1.5-q8").unwrap();
    assert_eq!(nomic["installed"], true);
    assert!(local["models"].as_array().unwrap().iter().any(|m| m["kind"] == "chat" && m["recommended"] == true));
    let (code, _) = c.req(reqwest::Method::DELETE, "/v1/local-models/nomic-embed-text-v1.5-q8", None).await;
    assert_eq!(code, 400, "the bundled embedding model can't be deleted");

    c.post(&format!("/v1/projects/{pid}/notes"), json!({ "title": "Tomatoes", "content": "Plant the San Marzano tomatoes after the last frost, around 10 May." })).await;
    c.post(&format!("/v1/projects/{pid}/notes"), json!({ "title": "Car", "content": "The car insurance renewal is due in March; compare quotes first." })).await;
    wait_jobs(&c).await;
    let hits = c.get("/v1/search?q=when%20to%20plant%20tomatoes").await;
    assert_eq!(hits[0]["title"], "Tomatoes", "{hits}");
    let info = c.get("/v1/vault/info").await;
    assert!(info["embedded_chunks"].as_i64().unwrap() >= 2, "notes embedded by the built-in model: {info}");
    // No shared keywords with the note — only the semantic index can find it.
    let hits = c.get("/v1/search?q=vegetable%20garden%20sowing%20season").await;
    assert_eq!(hits[0]["title"], "Tomatoes", "{hits}");
    let hits = c.get("/v1/search?q=vehicle%20cover%20policy").await;
    assert_eq!(hits[0]["title"], "Car", "{hits}");

    // Full RAG answer from the built-in engine, if a chat model has been downloaded into models/.
    let local = c.get("/v1/local-models").await;
    if let Some(m) = local["models"].as_array().unwrap().iter().find(|m| m["kind"] == "chat" && m["installed"] == true) {
        let (code, _) = c
            .req(
                reqwest::Method::PUT,
                "/v1/settings",
                Some(json!({ "chat_model": { "provider_id": builtin["id"], "model": m["id"] }, "embedding_model": settings["embedding_model"] })),
            )
            .await;
        assert_eq!(code, 200);
        let chat = c.post("/v1/chats", json!({})).await;
        let done = ask(&c, &chat, "When should I plant the tomatoes?").await;
        let answer = done["message"]["content"].as_str().unwrap();
        println!("built-in {} answer: {answer}", m["id"]);
        assert!(answer.contains("May") || answer.contains("frost"), "{answer}");
        assert!(!done["message"]["citations"].as_array().unwrap().is_empty());

        // A disabled provider still chosen as the chat's model and as the default: the chat
        // falls back to the built-in model instead of failing, and forgets the stale choice.
        let off = c.post("/v1/providers", json!({ "kind": "openai", "name": "LM Studio", "base_url": "http://localhost:1234/v1" })).await;
        let (code, _) = c.req(reqwest::Method::PATCH, &format!("/v1/providers/{}", off["id"]), Some(json!({ "enabled": false }))).await;
        assert_eq!(code, 200);
        let stale = json!({ "provider_id": off["id"], "model": "google/gemma-4-e4b" });
        c.req(reqwest::Method::PUT, "/v1/settings", Some(json!({ "chat_model": stale, "embedding_model": settings["embedding_model"] }))).await;
        let pinned = c.post("/v1/chats", json!({ "model": stale })).await;
        let done = ask(&c, &pinned, "When should I plant the tomatoes?").await;
        assert_eq!(done["message"]["model"], m["id"], "answered by the built-in model: {done}");
        assert!(done["message"]["error"].is_null(), "{done}");
        let chats = c.get("/v1/chats").await;
        let after = chats.as_array().unwrap().iter().find(|ch| ch["id"] == pinned["id"]).unwrap();
        assert!(after["model"].is_null(), "stale per-chat model cleared: {after}");
        c.req(reqwest::Method::PUT, "/v1/settings", Some(json!({ "chat_model": { "provider_id": builtin["id"], "model": m["id"] }, "embedding_model": settings["embedding_model"] }))).await;

        // Chat recall: a decision made in one chat is found — and cited — from another.
        let decided = c.post("/v1/chats", json!({})).await;
        ask(&c, &decided, "I decided to renew the car insurance with Allianz for 820 EUR per year. Acknowledge in one sentence.").await;
        wait_jobs(&c).await;
        let later = c.post("/v1/chats", json!({})).await;
        let done = ask(&c, &later, "Which insurer did I choose for the car insurance renewal, and what does it cost?").await;
        let answer = done["message"]["content"].as_str().unwrap();
        println!("recall answer: {answer}");
        assert!(answer.contains("Allianz") && answer.contains("820"), "{answer}");
        let cites = done["message"]["citations"].as_array().unwrap();
        assert!(cites.iter().any(|ct| ct["chat_id"] == decided["id"]), "cites the earlier chat: {cites:?}");

        // Long chats: the first message has left the history window but survives in the summary.
        let long = c.post("/v1/chats", json!({ "mode": "chat_only" })).await;
        ask(&c, &long, "My gym locker number is 317. Reply with just OK.").await;
        for i in 2..=7 {
            ask(&c, &long, &format!("Filler message {i}. Reply with just OK.")).await;
        }
        wait_jobs(&c).await;
        let done = ask(&c, &long, "What is my gym locker number? Answer with the number.").await;
        let answer = done["message"]["content"].as_str().unwrap();
        println!("summary answer: {answer}");
        assert!(answer.contains("317"), "{answer}");
    } else {
        println!("no built-in chat model downloaded — skipped the built-in chat part");
    }

    // Optional: full RAG answer through a local LM Studio, if one is running.
    let lm = reqwest::Client::new().get("http://localhost:1234/v1/models").timeout(std::time::Duration::from_secs(2)).send().await;
    if lm.is_ok_and(|r| r.status().is_success()) {
        let prov = c.post("/v1/providers", json!({ "kind": "openai", "name": "LM Studio", "base_url": "http://localhost:1234/v1" })).await;
        let id = prov["id"].as_i64().unwrap();
        let (code, _) = c
            .req(reqwest::Method::PUT, "/v1/settings", Some(json!({ "chat_model": { "provider_id": id, "model": "google/gemma-4-e4b" }, "embedding_model": settings["embedding_model"] })))
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
    let emb = {
        let (md, bundled) = models(dir.path());
        Embedded::start(vault.clone(), "My Vault", None, md, bundled, None, None).await.unwrap()
    };
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

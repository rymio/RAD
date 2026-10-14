//! `/v1/assist` with the built-in model following the sandbox agent's protocol: a command in
//! a ```sh block, then — given its output — a plain answer. Runs only when a built-in chat model
//! has been downloaded into models/ (Settings → Models); otherwise it's skipped.

use rad_server::embedded::Embedded;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builtin_model_follows_the_sandbox_agent_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")];
    let emb = Embedded::start(dir.path().join("v.radvault"), "V", None, dir.path().join("models"), bundled, None, None).await.unwrap();
    let http = reqwest::Client::new();
    let (base, token) = (emb.url(), emb.device_token.clone());
    let r: Value = http.post(format!("{base}/v1/vault/init")).bearer_auth(&token).json(&json!({ "passphrase": "assist test pass" })).send().await.unwrap().json().await.unwrap();
    let session = r["session"].as_str().unwrap().to_string();
    let call = |m: reqwest::Method, p: &str| http.request(m, format!("{base}{p}")).bearer_auth(&token).header("x-rad-session", &session);

    let local: Value = call(reqwest::Method::GET, "/v1/local-models").send().await.unwrap().json().await.unwrap();
    let Some(m) = local["models"].as_array().unwrap().iter().find(|m| m["kind"] == "chat" && m["installed"] == true) else {
        println!("no built-in chat model downloaded — skipped");
        return;
    };
    let settings: Value = call(reqwest::Method::GET, "/v1/settings").send().await.unwrap().json().await.unwrap();
    call(reqwest::Method::PUT, "/v1/settings")
        .json(&json!({ "chat_model": { "provider_id": local["provider_id"], "model": m["id"] }, "embedding_model": settings["embedding_model"] }))
        .send()
        .await
        .unwrap();

    let system = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/agent-prompt.txt"));
    let mut messages = vec![json!({ "role": "user", "content": "list the current directory" })];
    let ask = |messages: Vec<Value>| call(reqwest::Method::POST, "/v1/assist").json(&json!({ "system": system, "messages": messages })).send();
    let r: Value = ask(messages.clone()).await.unwrap().json().await.unwrap();
    let reply = r["content"].as_str().unwrap().to_string();
    println!("--- step 1 ({}):\n{reply}", r["model"]);
    assert!(reply.contains("```") && reply.contains("ls"), "{reply}");

    messages.push(json!({ "role": "assistant", "content": reply }));
    messages.push(json!({ "role": "user", "content": "Exit code 0.\n```\ntotal 12\ndrwx------ 3 root root 4096 .\ndrwxr-xr-x 1 root root 4096 ..\ndrwxr-xr-x 2 root root 4096 shared\n```" }));
    let r: Value = ask(messages).await.unwrap().json().await.unwrap();
    let reply = r["content"].as_str().unwrap().to_string();
    println!("--- step 2:\n{reply}");
    assert!(!reply.contains("```"), "should answer, not run more: {reply}");
    assert!(reply.contains("shared"), "{reply}");
    emb.stop().await;
}

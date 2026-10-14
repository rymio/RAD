//! Two computers ("MacBook" and "iMac") using one vault file at the same time: each runs
//! its own RAD server on the same path, as two machines would on a network share.

use std::time::Duration;

use futures::StreamExt;
use rad_server::embedded::Embedded;
use serde_json::{json, Value};

const PASS: &str = "shared vault pass";

struct Computer {
    _emb: Embedded,
    http: reqwest::Client,
    base: String,
    token: String,
    session: Option<String>,
}

impl Computer {
    async fn start(path: &std::path::Path, models: &std::path::Path, id: &str, name: &str) -> Computer {
        let bundled = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")];
        let emb = Embedded::start(path.to_path_buf(), "V", None, models.to_path_buf(), bundled, Some(id.into()), Some(name.into())).await.unwrap();
        Computer { base: emb.url(), token: emb.device_token.clone(), _emb: emb, http: reqwest::Client::new(), session: None }
    }

    fn req(&self, m: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let r = self.http.request(m, format!("{}{path}", self.base)).bearer_auth(&self.token);
        match &self.session {
            Some(s) => r.header("x-rad-session", s),
            None => r,
        }
    }

    async fn unlock(&mut self, path: &str) -> (u16, Value) {
        let r = self.req(reqwest::Method::POST, path).json(&json!({ "passphrase": PASS })).send().await.unwrap();
        let code = r.status().as_u16();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if let Some(s) = v["session"].as_str() {
            self.session = Some(s.to_string());
        }
        (code, v)
    }

    async fn call(&self, m: reqwest::Method, path: &str, body: Value) -> (u16, Value) {
        let r = self.req(m, path).json(&body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_computers_share_one_vault() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("Shared.radvault");
    let models = dir.path().join("models");

    let mut mac = Computer::start(&vault, &models, "machine-macbook", "MacBook").await;
    let (code, _) = mac.unlock("/v1/vault/init").await;
    assert_eq!(code, 200);

    // Sharing is off: a second computer may not open the vault meanwhile.
    let mut imac = Computer::start(&vault, &models, "machine-imac", "iMac").await;
    let (code, v) = imac.unlock("/v1/unlock").await;
    assert_eq!(code, 409, "{v}");
    assert!(v["error"]["message"].as_str().unwrap_or(&v.to_string()).contains("MacBook"), "{v}");

    // Turn sharing on; now the iMac can join.
    let (code, v) = mac.call(reqwest::Method::PUT, "/v1/vault/sharing", json!({ "shared": true })).await;
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["shared"], true);
    tokio::time::sleep(Duration::from_millis(300)).await; // the MacBook's session file is refreshed
    let (code, v) = imac.unlock("/v1/unlock").await;
    assert_eq!(code, 200, "{v}");

    let (_, v) = mac.call(reqwest::Method::GET, "/v1/vault/sharing", Value::Null).await;
    let mut names: Vec<&str> = v["sessions"].as_array().unwrap().iter().map(|s| s["machine"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(names, vec!["MacBook", "iMac"]);

    // Live: what the MacBook writes reaches the iMac's event stream.
    let events = imac.req(reqwest::Method::GET, "/v1/events").send().await.unwrap();
    let mut stream = events.bytes_stream();
    let (code, project) = mac.call(reqwest::Method::POST, "/v1/projects", json!({ "name": "From the MacBook" })).await;
    assert_eq!(code, 200, "{project}");
    let got = tokio::time::timeout(Duration::from_secs(15), async {
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            if buf.contains("\"entity\":\"project\"") {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(got, Ok(true), "the iMac should hear about the MacBook's new project");
    let (_, projects) = imac.call(reqwest::Method::GET, "/v1/projects", Value::Null).await;
    assert!(projects.as_array().unwrap().iter().any(|p| p["name"] == "From the MacBook"));

    // Both write at once: everything lands.
    let (a, b) = tokio::join!(
        async {
            for i in 0..10 {
                assert_eq!(mac.call(reqwest::Method::POST, "/v1/projects", json!({ "name": format!("Mac {i}") })).await.0, 200);
            }
        },
        async {
            for i in 0..10 {
                assert_eq!(imac.call(reqwest::Method::POST, "/v1/projects", json!({ "name": format!("iMac {i}") })).await.0, 200);
            }
        }
    );
    let _ = (a, b);
    let (_, projects) = mac.call(reqwest::Method::GET, "/v1/projects", Value::Null).await;
    assert_eq!(projects.as_array().unwrap().iter().filter(|p| p["name"].as_str().unwrap().contains("Mac")).count(), 21);

    // Sharing can't be turned off, nor the passphrase changed, while the iMac has it open.
    let (code, _) = mac.call(reqwest::Method::PUT, "/v1/vault/sharing", json!({ "shared": false })).await;
    assert_eq!(code, 409);
    let (code, _) = mac.call(reqwest::Method::POST, "/v1/vault/passphrase", json!({ "current": PASS, "new": "another long pass" })).await;
    assert_eq!(code, 409);

    // Once the iMac locks, it's gone from the list and sharing can be turned off.
    let (code, _) = imac.call(reqwest::Method::POST, "/v1/vault/lock", Value::Null).await;
    assert_eq!(code, 200);
    let (code, v) = mac.call(reqwest::Method::PUT, "/v1/vault/sharing", json!({ "shared": false })).await;
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["sessions"].as_array().unwrap().len(), 1);
}

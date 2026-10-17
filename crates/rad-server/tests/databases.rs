//! Databases over HTTP: the RAD app manages them; other programs use API keys.

use rad_server::embedded::Embedded;
use reqwest::Method;
use serde_json::{json, Value};

const PASS: &str = "database vault pass";

struct Client {
    http: reqwest::Client,
    base: String,
    auth: String,
    session: Option<String>,
}

impl Client {
    async fn call(&self, m: Method, path: &str, body: Value) -> (u16, Value) {
        let mut r = self.http.request(m, format!("{}{path}", self.base)).bearer_auth(&self.auth);
        if let Some(s) = &self.session {
            r = r.header("x-rad-session", s);
        }
        if !body.is_null() {
            r = r.json(&body);
        }
        let r = r.send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn databases_crud_and_api_keys() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("Db.radvault");
    let bundled = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")];
    let emb = Embedded::start(vault.clone(), "V", None, dir.path().join("models"), bundled, None, None).await.unwrap();
    let mut app = Client { http: reqwest::Client::new(), base: emb.url(), auth: emb.device_token.clone(), session: None };
    let (code, v) = app.call(Method::POST, "/v1/vault/init", json!({ "passphrase": PASS })).await;
    assert_eq!(code, 200, "{v}");
    app.session = v["session"].as_str().map(String::from);

    // A database with a table, rows, and a schema change.
    let (code, db) = app.call(Method::POST, "/v1/db", json!({ "name": "shop" })).await;
    assert_eq!(code, 200, "{db}");
    let id = db["id"].as_str().unwrap().to_string();
    let (code, v) = app.call(Method::POST, "/v1/db", json!({ "name": "shop" })).await;
    assert_eq!(code, 400, "duplicate names are refused: {v}");
    let cols = json!([
        {"name": "id", "type": "INTEGER", "pk": 1},
        {"name": "title", "type": "TEXT", "not_null": true},
        {"name": "price", "type": "REAL", "default": "0"}
    ]);
    let (code, t) = app.call(Method::POST, &format!("/v1/db/{id}/tables"), json!({ "name": "products", "columns": cols })).await;
    assert_eq!(code, 200, "{t}");
    assert_eq!(t["columns"].as_array().unwrap().len(), 3);
    for (title, price) in [("Pen", 1.5), ("Notebook", 4.0), ("Lamp", 25.0)] {
        let (code, v) = app.call(Method::POST, "/v1/db/shop/tables/products/rows", json!({ "title": title, "price": price })).await;
        assert_eq!(code, 200, "{v}");
    }
    let (code, v) = app.call(Method::POST, "/v1/db/shop/tables/products/rows", json!({ "price": 3 })).await;
    assert_eq!(code, 422, "NOT NULL is enforced: {v}");
    let (code, page) = app.call(Method::GET, "/v1/db/shop/tables/products/rows?order=price&desc=true&limit=2", Value::Null).await;
    assert_eq!(code, 200, "{page}");
    assert_eq!(page["total"], 3);
    assert_eq!(page["rows"][0]["values"][1], "Lamp");
    let lamp = page["rows"][0]["rowid"].as_i64().unwrap();
    let (code, v) = app.call(Method::PATCH, &format!("/v1/db/shop/tables/products/rows/{lamp}"), json!({ "price": 19.99 })).await;
    assert_eq!((code, v["price"].clone()), (200, json!(19.99)), "{v}");
    let (code, _) = app.call(Method::POST, "/v1/db/shop/tables/products/columns", json!({ "name": "stock", "type": "INTEGER", "default": "10" })).await;
    assert_eq!(code, 200);
    let (code, v) = app.call(Method::POST, "/v1/db/shop/sql", json!({ "sql": "UPDATE products SET stock = stock - 1 WHERE title = ?1", "params": ["Pen"] })).await;
    assert_eq!((code, v[0]["changes"].clone()), (200, json!(1)), "{v}");
    let (code, v) = app.call(Method::POST, "/v1/db/shop/sql", json!({ "sql": "SELECT title, stock FROM products ORDER BY id; SELECT count(*) AS n FROM products" })).await;
    assert_eq!(code, 200, "{v}");
    assert_eq!(v[0]["rows"][0], json!(["Pen", 9]));
    assert_eq!(v[1]["rows"][0][0], 3);
    let (code, v) = app.call(Method::POST, "/v1/db/shop/sql", json!({ "sql": "SELEC oops" })).await;
    assert_eq!(code, 422, "{v}");

    // Another program: a read-only key for this database.
    let (code, k) = app.call(Method::POST, "/v1/db-keys", json!({ "name": "report tool", "write": false, "dbs": [id] })).await;
    assert_eq!(code, 200, "{k}");
    let token = k["key"].as_str().unwrap().to_string();
    assert!(token.starts_with("radk_"));
    let program = Client { http: reqwest::Client::new(), base: emb.url(), auth: token.clone(), session: None };
    let (code, v) = program.call(Method::GET, "/v1/db", Value::Null).await;
    assert_eq!((code, v.as_array().map(|a| a.len())), (200, Some(1)), "{v}");
    let (code, v) = program.call(Method::GET, "/v1/db/shop/tables/products/rows", Value::Null).await;
    assert_eq!(code, 200, "{v}");
    let (code, _) = program.call(Method::POST, "/v1/db/shop/sql", json!({ "sql": "SELECT * FROM products" })).await;
    assert_eq!(code, 200);
    let (code, v) = program.call(Method::POST, "/v1/db/shop/sql", json!({ "sql": "DELETE FROM products" })).await;
    assert_eq!(code, 403, "read-only keys can't write: {v}");
    let (code, _) = program.call(Method::POST, "/v1/db/shop/tables/products/rows", json!({ "title": "x" })).await;
    assert_eq!(code, 403);
    let (code, _) = program.call(Method::POST, "/v1/db", json!({ "name": "mine" })).await;
    assert_eq!(code, 403, "keys can't create databases");
    let (code, _) = program.call(Method::GET, "/v1/db-keys", Value::Null).await;
    assert_eq!(code, 401, "keys can't manage keys");
    let (code, _) = program.call(Method::GET, "/v1/chats", Value::Null).await;
    assert_eq!(code, 401, "keys only reach databases");

    // A database the key isn't for.
    let (_, other) = app.call(Method::POST, "/v1/db", json!({ "name": "private" })).await;
    let (code, _) = program.call(Method::GET, &format!("/v1/db/{}/tables", other["id"].as_str().unwrap()), Value::Null).await;
    assert_eq!(code, 403);

    // A read-write key for every database.
    let (_, k2) = app.call(Method::POST, "/v1/db-keys", json!({ "name": "sync job", "write": true })).await;
    let writer = Client { http: reqwest::Client::new(), base: emb.url(), auth: k2["key"].as_str().unwrap().into(), session: None };
    let (code, v) = writer.call(Method::POST, "/v1/db/private/sql", json!({ "sql": "CREATE TABLE notes(body TEXT); INSERT INTO notes VALUES ('hi')" })).await;
    assert_eq!(code, 200, "{v}");

    // Locked vault: nothing opens (the keys are in it); unlocked again, everything is there.
    let (code, _) = app.call(Method::POST, "/v1/vault/lock", Value::Null).await;
    assert_eq!(code, 200);
    let (code, _) = program.call(Method::GET, "/v1/db", Value::Null).await;
    assert_eq!(code, 423);
    app.session = None;
    let (code, v) = app.call(Method::POST, "/v1/unlock", json!({ "passphrase": PASS })).await;
    assert_eq!(code, 200, "{v}");
    app.session = v["session"].as_str().map(String::from);
    let (code, v) = program.call(Method::POST, "/v1/db/shop/sql", json!({ "sql": "SELECT price FROM products WHERE title = 'Lamp'" })).await;
    assert_eq!((code, v[0]["rows"][0][0].clone()), (200, json!(19.99)), "{v}");

    // The files next to the vault are encrypted.
    let files: Vec<_> = std::fs::read_dir(format!("{}-databases", vault.display())).unwrap().flatten().collect();
    assert!(!files.is_empty());
    for f in files {
        let raw = std::fs::read(f.path()).unwrap();
        assert!(!raw.windows(8).any(|w| w == b"Notebook"), "{:?} holds plaintext", f.path());
    }

    // Backups carry the databases (their keys are in the vault backup).
    let (code, b) = app.call(Method::POST, "/v1/vault/backup", Value::Null).await;
    assert_eq!(code, 200, "{b}");
    let backups = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.path()).find(|p| p.is_dir() && p.join(b["file"].as_str().unwrap()).exists());
    let bdir = backups.unwrap_or_else(|| dir.path().join("backups"));
    let copies: Vec<_> = std::fs::read_dir(format!("{}-databases", bdir.join(b["file"].as_str().unwrap()).display())).unwrap().flatten().collect();
    assert_eq!(copies.len(), 2, "{copies:?}");

    // Deleting the database removes the key that was only for it, and its files.
    let (code, _) = app.call(Method::DELETE, "/v1/db/shop", Value::Null).await;
    assert_eq!(code, 204);
    let (code, _) = program.call(Method::GET, "/v1/db", Value::Null).await;
    assert_eq!(code, 401, "a key only for a deleted database is gone");
    let (_, keys) = app.call(Method::GET, "/v1/db-keys", Value::Null).await;
    assert_eq!(keys.as_array().unwrap().len(), 1);
    let (code, _) = app.call(Method::DELETE, &format!("/v1/db-keys/{}", keys[0]["id"].as_str().unwrap()), Value::Null).await;
    assert_eq!(code, 204);
    let (code, _) = writer.call(Method::GET, "/v1/db", Value::Null).await;
    assert_eq!(code, 401);
}

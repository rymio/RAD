//! Databases (Turso, see rad-db): SQL databases next to the vault, encrypted with keys kept in
//! the vault. Managed by the Database view, and open to other programs through API keys:
//!
//!   Authorization: Bearer radk_…   (a key made in Database → API keys; read-only or read-write,
//!                                   for all databases or chosen ones)
//!
//! Everything works only while the vault is unlocked (the databases' keys are in it).
//!
//!   GET    /v1/db                                   databases
//!   POST   /v1/db                                   {name} → create
//!   PATCH  /v1/db/{db}                              {name} → rename
//!   DELETE /v1/db/{db}
//!   GET    /v1/db/{db}/tables                       tables and views, with columns and row counts
//!   POST   /v1/db/{db}/tables                       {name, columns:[{name,type,not_null,default,pk,unique}]}
//!   GET    /v1/db/{db}/tables/{t}                   one table
//!   PATCH  /v1/db/{db}/tables/{t}                   {rename_to}
//!   DELETE /v1/db/{db}/tables/{t}
//!   POST   /v1/db/{db}/tables/{t}/columns           {name,type,not_null,default,unique} → add
//!   PATCH  /v1/db/{db}/tables/{t}/columns/{c}       {rename_to}
//!   DELETE /v1/db/{db}/tables/{t}/columns/{c}
//!   GET    /v1/db/{db}/tables/{t}/rows?limit&offset&order&desc&q
//!   POST   /v1/db/{db}/tables/{t}/rows              {column: value, …} → {rowid}
//!   GET    /v1/db/{db}/tables/{t}/rows/{rowid}
//!   PATCH  /v1/db/{db}/tables/{t}/rows/{rowid}      {column: value, …}
//!   DELETE /v1/db/{db}/tables/{t}/rows/{rowid}
//!   POST   /v1/db/{db}/sql                          {sql, params?} → [{columns, rows, changes, last_insert_rowid}]
//!   GET/POST /v1/db-keys, DELETE /v1/db-keys/{id}   API keys (the RAD app only)
//!
//! `{db}` is a database's id or its name. Blobs are `{"$blob": "<base64>"}`.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;
use rad_vault::rusqlite::{params, OptionalExtension};
use rad_vault::{now_ms, Connection, Vault};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::auth::{bearer, Authed};
use crate::error::{ApiErr, ApiResult};
use crate::state::{db_vault, AppState, Shared};

const REGISTRY: &str = "databases";
const KEYS: &str = "database_api_keys";
pub const KEY_PREFIX: &str = "radk_";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DbEntry {
    id: String,
    name: String,
    /// The database's encryption key (hex); never leaves the server.
    key: String,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiKey {
    id: String,
    name: String,
    /// The first characters, to recognise it.
    prefix: String,
    /// blake3 of the whole key.
    hash: String,
    write: bool,
    /// Databases it may use (ids); empty: all.
    #[serde(default)]
    dbs: Vec<String>,
    created_at: i64,
    #[serde(default)]
    last_used: Option<i64>,
}

fn load<T: for<'de> Deserialize<'de>>(c: &Connection, key: &str) -> ApiResult<Vec<T>> {
    let raw: Option<String> = c.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0)).optional().map_err(ApiErr::internal)?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

fn store<T: Serialize>(c: &Connection, key: &str, v: &[T]) -> ApiResult<()> {
    c.execute("INSERT OR REPLACE INTO settings(key, value) VALUES (?1, ?2)", params![key, serde_json::to_string(v).map_err(ApiErr::internal)?]).map_err(ApiErr::internal)?;
    Ok(())
}

/// Every database's (id, key), for backups.
pub(crate) fn registry(c: &Connection) -> ApiResult<Vec<(String, String)>> {
    Ok(load::<DbEntry>(c, REGISTRY)?.into_iter().map(|e| (e.id, e.key)).collect())
}

fn db_err(e: rad_db::DbError) -> ApiErr {
    match e {
        rad_db::DbError::NotFound(m) => ApiErr::new(StatusCode::NOT_FOUND, "not_found", m),
        rad_db::DbError::Invalid(m) => ApiErr::bad_request(m),
        rad_db::DbError::Sql(m) => ApiErr::new(StatusCode::UNPROCESSABLE_ENTITY, "sql", m),
        rad_db::DbError::Io(e) => ApiErr::internal(e),
    }
}

// ------------------------------------------------------------------ access

/// Who is asking: the RAD app (everything) or a program with an API key.
pub struct DbAccess {
    vault: Arc<Vault>,
    write: bool,
    /// None: every database.
    only: Option<Vec<String>>,
    /// The API key's id (None for the RAD app itself).
    key_id: Option<String>,
}

impl FromRequestParts<Shared> for DbAccess {
    type Rejection = ApiErr;
    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, ApiErr> {
        if let Some(token) = bearer(parts).filter(|t| t.starts_with(KEY_PREFIX)).map(str::to_string) {
            let vault = state.vault()?;
            let hash = blake3::hash(token.as_bytes()).to_hex().to_string();
            let v = vault.clone();
            let key = tokio::task::spawn_blocking(move || -> ApiResult<Option<ApiKey>> {
                let c = v.conn();
                let mut keys: Vec<ApiKey> = load(&c, KEYS)?;
                let Some(k) = keys.iter_mut().find(|k| k.hash == hash) else { return Ok(None) };
                // Note when it was last used (at most once a minute).
                let now = now_ms();
                if k.last_used.is_none_or(|t| now - t > 60_000) {
                    k.last_used = Some(now);
                    let found = k.clone();
                    store(&c, KEYS, &keys)?;
                    return Ok(Some(found));
                }
                Ok(Some(k.clone()))
            })
            .await
            .map_err(ApiErr::internal)??
            .ok_or_else(|| ApiErr::unauthorized("unknown or deleted API key"))?;
            state.touch();
            return Ok(DbAccess { vault, write: key.write, only: (!key.dbs.is_empty()).then_some(key.dbs), key_id: Some(key.id) });
        }
        let a = Authed::from_request_parts(parts, state).await?;
        Ok(DbAccess { vault: a.vault, write: true, only: None, key_id: None })
    }
}

impl DbAccess {
    fn need_write(&self) -> ApiResult<()> {
        if !self.write {
            return Err(ApiErr::new(StatusCode::FORBIDDEN, "read_only", "this API key can only read"));
        }
        Ok(())
    }
    fn may_use(&self, id: &str) -> bool {
        self.only.as_ref().is_none_or(|l| l.iter().any(|x| x == id))
    }
    fn app_only(&self) -> ApiResult<()> {
        if self.key_id.is_some() {
            return Err(ApiErr::new(StatusCode::FORBIDDEN, "forbidden", "API keys can't do this"));
        }
        Ok(())
    }
}

/// A database by id or name, if this caller may use it.
async fn entry(state: &Shared, a: &DbAccess, db: &str) -> ApiResult<DbEntry> {
    let db = db.to_string();
    let list: Vec<DbEntry> = db_vault(a.vault.clone(), state, |_, c| load(c, REGISTRY)).await?;
    let e = list.into_iter().find(|e| e.id == db || e.name == db).ok_or_else(|| ApiErr::new(StatusCode::NOT_FOUND, "not_found", "no such database"))?;
    if !a.may_use(&e.id) {
        return Err(ApiErr::new(StatusCode::FORBIDDEN, "forbidden", "this API key can't use that database"));
    }
    Ok(e)
}

async fn conn(state: &Shared, a: &DbAccess, db: &str) -> ApiResult<(DbEntry, rad_db::turso::Connection)> {
    let e = entry(state, a, db).await?;
    let c = state.dbs.connect(&e.id, &e.key).await.map_err(db_err)?;
    Ok((e, c))
}

fn notify(st: &AppState, c: &Connection, what: &str) {
    st.changed(c, "databases", 0, what);
}

// ------------------------------------------------------------------ databases

#[derive(Serialize)]
pub struct DbInfo {
    id: String,
    name: String,
    created_at: i64,
    size: u64,
}

fn info(st: &AppState, e: &DbEntry) -> DbInfo {
    DbInfo { id: e.id.clone(), name: e.name.clone(), created_at: e.created_at, size: st.dbs.size(&e.id) }
}

fn valid_name(n: &str) -> ApiResult<String> {
    let n = n.trim();
    if n.is_empty() || n.len() > 64 || n.chars().any(|c| c.is_control() || c == '/' || c == '\\') {
        return Err(ApiErr::bad_request("give the database a name (up to 64 characters, no slashes)"));
    }
    Ok(n.to_string())
}

pub async fn list(State(state): State<Shared>, a: DbAccess) -> ApiResult<Json<Vec<DbInfo>>> {
    let only = a.only.clone();
    db_vault(a.vault.clone(), &state, move |st, c| {
        let list: Vec<DbEntry> = load(c, REGISTRY)?;
        Ok(Json(list.iter().filter(|e| only.as_ref().is_none_or(|l| l.contains(&e.id))).map(|e| info(st, e)).collect()))
    })
    .await
}

#[derive(Deserialize)]
pub struct NameInput {
    name: String,
}

pub async fn create(State(state): State<Shared>, a: DbAccess, Json(input): Json<NameInput>) -> ApiResult<Json<DbInfo>> {
    a.app_only()?;
    let name = valid_name(&input.name)?;
    let e = DbEntry { id: format!("db{}", now_ms()), name, key: rad_db::new_key(), created_at: now_ms() };
    {
        let e = e.clone();
        db_vault(a.vault.clone(), &state, move |st, c| {
            let mut list: Vec<DbEntry> = load(c, REGISTRY)?;
            if list.iter().any(|x| x.name == e.name) {
                return Err(ApiErr::bad_request("there's already a database with that name"));
            }
            list.push(e);
            store(c, REGISTRY, &list)?;
            notify(st, c, "create");
            Ok(())
        })
        .await?;
    }
    // Create the file now, so a bad key or folder shows up at once.
    if let Err(err) = state.dbs.connect(&e.id, &e.key).await {
        let id = e.id.clone();
        let _ = db_vault(a.vault.clone(), &state, move |_, c| {
            let mut list: Vec<DbEntry> = load(c, REGISTRY)?;
            list.retain(|x| x.id != id);
            store(c, REGISTRY, &list)
        })
        .await;
        return Err(db_err(err));
    }
    Ok(Json(info(&state, &e)))
}

pub async fn rename(State(state): State<Shared>, a: DbAccess, Path(db): Path<String>, Json(input): Json<NameInput>) -> ApiResult<Json<DbInfo>> {
    a.app_only()?;
    let e = entry(&state, &a, &db).await?;
    let name = valid_name(&input.name)?;
    db_vault(a.vault.clone(), &state, move |st, c| {
        let mut list: Vec<DbEntry> = load(c, REGISTRY)?;
        if list.iter().any(|x| x.name == name && x.id != e.id) {
            return Err(ApiErr::bad_request("there's already a database with that name"));
        }
        let x = list.iter_mut().find(|x| x.id == e.id).ok_or_else(ApiErr::not_found)?;
        x.name = name;
        let out = info(st, x);
        store(c, REGISTRY, &list)?;
        notify(st, c, "rename");
        Ok(Json(out))
    })
    .await
}

pub async fn remove(State(state): State<Shared>, a: DbAccess, Path(db): Path<String>) -> ApiResult<StatusCode> {
    a.app_only()?;
    let e = entry(&state, &a, &db).await?;
    let id = e.id.clone();
    db_vault(a.vault.clone(), &state, move |st, c| {
        let mut list: Vec<DbEntry> = load(c, REGISTRY)?;
        list.retain(|x| x.id != id);
        store(c, REGISTRY, &list)?;
        // API keys limited to it lose it; a key that was only for it is deleted (an empty
        // list would mean "every database").
        let mut keys: Vec<ApiKey> = load(c, KEYS)?;
        keys.retain_mut(|k| {
            if !k.dbs.contains(&id) {
                return true;
            }
            k.dbs.retain(|d| *d != id);
            !k.dbs.is_empty()
        });
        store(c, KEYS, &keys)?;
        notify(st, c, "delete");
        Ok(())
    })
    .await?;
    state.dbs.remove(&e.id).map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ tables

pub async fn tables(State(state): State<Shared>, a: DbAccess, Path(db): Path<String>) -> ApiResult<Json<Vec<rad_db::Table>>> {
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::tables(&c).await.map(Json).map_err(db_err)
}

#[derive(Deserialize)]
pub struct TableInput {
    name: String,
    columns: Vec<rad_db::Column>,
}

pub async fn create_table(State(state): State<Shared>, a: DbAccess, Path(db): Path<String>, Json(input): Json<TableInput>) -> ApiResult<Json<rad_db::Table>> {
    a.need_write()?;
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::create_table(&c, &input.name, &input.columns).await.map(Json).map_err(db_err)
}

pub async fn get_table(State(state): State<Shared>, a: DbAccess, Path((db, t)): Path<(String, String)>) -> ApiResult<Json<rad_db::Table>> {
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::table(&c, &t).await.map(Json).map_err(db_err)
}

#[derive(Deserialize)]
pub struct RenameInput {
    rename_to: String,
}

pub async fn rename_table(State(state): State<Shared>, a: DbAccess, Path((db, t)): Path<(String, String)>, Json(input): Json<RenameInput>) -> ApiResult<Json<rad_db::Table>> {
    a.need_write()?;
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::rename_table(&c, &t, &input.rename_to).await.map_err(db_err)?;
    rad_db::table(&c, input.rename_to.trim()).await.map(Json).map_err(db_err)
}

pub async fn drop_table(State(state): State<Shared>, a: DbAccess, Path((db, t)): Path<(String, String)>) -> ApiResult<StatusCode> {
    a.need_write()?;
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::drop_table(&c, &t).await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_column(State(state): State<Shared>, a: DbAccess, Path((db, t)): Path<(String, String)>, Json(col): Json<rad_db::Column>) -> ApiResult<Json<rad_db::Table>> {
    a.need_write()?;
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::add_column(&c, &t, &col).await.map_err(db_err)?;
    rad_db::table(&c, &t).await.map(Json).map_err(db_err)
}

pub async fn rename_column(State(state): State<Shared>, a: DbAccess, Path((db, t, col)): Path<(String, String, String)>, Json(input): Json<RenameInput>) -> ApiResult<Json<rad_db::Table>> {
    a.need_write()?;
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::rename_column(&c, &t, &col, &input.rename_to).await.map_err(db_err)?;
    rad_db::table(&c, &t).await.map(Json).map_err(db_err)
}

pub async fn drop_column(State(state): State<Shared>, a: DbAccess, Path((db, t, col)): Path<(String, String, String)>) -> ApiResult<Json<rad_db::Table>> {
    a.need_write()?;
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::drop_column(&c, &t, &col).await.map_err(db_err)?;
    rad_db::table(&c, &t).await.map(Json).map_err(db_err)
}

// ------------------------------------------------------------------ rows

pub async fn rows(State(state): State<Shared>, a: DbAccess, Path((db, t)): Path<(String, String)>, Query(q): Query<rad_db::RowQuery>) -> ApiResult<Json<rad_db::RowsPage>> {
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::rows(&c, &t, &q).await.map(Json).map_err(db_err)
}

pub async fn insert(State(state): State<Shared>, a: DbAccess, Path((db, t)): Path<(String, String)>, Json(values): Json<Map<String, Value>>) -> ApiResult<Json<Value>> {
    a.need_write()?;
    let (_, c) = conn(&state, &a, &db).await?;
    let rowid = rad_db::insert(&c, &t, &values).await.map_err(db_err)?;
    Ok(Json(json!({ "rowid": rowid })))
}

pub async fn get_row(State(state): State<Shared>, a: DbAccess, Path((db, t, rowid)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::get_row(&c, &t, rowid).await.map(Json).map_err(db_err)
}

pub async fn update(State(state): State<Shared>, a: DbAccess, Path((db, t, rowid)): Path<(String, String, i64)>, Json(values): Json<Map<String, Value>>) -> ApiResult<Json<Value>> {
    a.need_write()?;
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::update(&c, &t, rowid, &values).await.map_err(db_err)?;
    rad_db::get_row(&c, &t, rowid).await.map(Json).map_err(db_err)
}

pub async fn delete_row(State(state): State<Shared>, a: DbAccess, Path((db, t, rowid)): Path<(String, String, i64)>) -> ApiResult<StatusCode> {
    a.need_write()?;
    let (_, c) = conn(&state, &a, &db).await?;
    let n = rad_db::delete(&c, &t, &[rowid]).await.map_err(db_err)?;
    if n == 0 {
        return Err(ApiErr::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ SQL

#[derive(Deserialize)]
pub struct SqlInput {
    sql: String,
    #[serde(default)]
    params: Vec<Value>,
}

/// First word of each statement that only reads.
fn read_only_sql(sql: &str) -> bool {
    rad_db::split_sql(sql).iter().all(|s| {
        let w = s.trim_start().split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("").to_ascii_uppercase();
        matches!(w.as_str(), "SELECT" | "EXPLAIN" | "VALUES") || (w == "WITH" && !s.to_ascii_uppercase().contains("INSERT") && !s.to_ascii_uppercase().contains("UPDATE") && !s.to_ascii_uppercase().contains("DELETE"))
    })
}

pub async fn sql(State(state): State<Shared>, a: DbAccess, Path(db): Path<String>, Json(input): Json<SqlInput>) -> ApiResult<Json<Vec<rad_db::StatementResult>>> {
    if !a.write && !read_only_sql(&input.sql) {
        return Err(ApiErr::new(StatusCode::FORBIDDEN, "read_only", "this API key can only run SELECT queries"));
    }
    if input.sql.len() > 1 << 20 {
        return Err(ApiErr::bad_request("that SQL is too long"));
    }
    let (_, c) = conn(&state, &a, &db).await?;
    rad_db::run_sql(&c, &input.sql, &input.params).await.map(Json).map_err(db_err)
}

// ------------------------------------------------------------------ API keys (the RAD app only)

#[derive(Serialize)]
pub struct KeyInfo {
    id: String,
    name: String,
    prefix: String,
    write: bool,
    dbs: Vec<String>,
    created_at: i64,
    last_used: Option<i64>,
}

fn key_info(k: &ApiKey) -> KeyInfo {
    KeyInfo { id: k.id.clone(), name: k.name.clone(), prefix: k.prefix.clone(), write: k.write, dbs: k.dbs.clone(), created_at: k.created_at, last_used: k.last_used }
}

pub async fn list_keys(State(state): State<Shared>, a: Authed) -> ApiResult<Json<Vec<KeyInfo>>> {
    db_vault(a.vault, &state, |_, c| {
        let keys: Vec<ApiKey> = load(c, KEYS)?;
        Ok(Json(keys.iter().map(key_info).collect()))
    })
    .await
}

#[derive(Deserialize)]
pub struct KeyInput {
    name: String,
    #[serde(default)]
    write: bool,
    #[serde(default)]
    dbs: Vec<String>,
}

/// Makes an API key. The key itself is shown once, in this answer.
pub async fn create_key(State(state): State<Shared>, a: Authed, Json(input): Json<KeyInput>) -> ApiResult<Json<Value>> {
    let name = input.name.trim().chars().take(60).collect::<String>();
    if name.is_empty() {
        return Err(ApiErr::bad_request("name the key after the program that will use it"));
    }
    let token = format!("{KEY_PREFIX}{}", crate::devices::random_hex(32));
    let k = ApiKey {
        id: format!("k{}", now_ms()),
        name,
        prefix: token.chars().take(KEY_PREFIX.len() + 6).collect(),
        hash: blake3::hash(token.as_bytes()).to_hex().to_string(),
        write: input.write,
        dbs: input.dbs,
        created_at: now_ms(),
        last_used: None,
    };
    db_vault(a.vault, &state, move |st, c| {
        let known: Vec<DbEntry> = load(c, REGISTRY)?;
        if k.dbs.iter().any(|d| !known.iter().any(|e| e.id == *d)) {
            return Err(ApiErr::bad_request("unknown database"));
        }
        let mut keys: Vec<ApiKey> = load(c, KEYS)?;
        let info = key_info(&k);
        keys.push(k);
        store(c, KEYS, &keys)?;
        notify(st, c, "keys");
        Ok(Json(json!({ "key": token, "info": info })))
    })
    .await
}

pub async fn delete_key(State(state): State<Shared>, a: Authed, Path(id): Path<String>) -> ApiResult<StatusCode> {
    db_vault(a.vault, &state, move |st, c| {
        let mut keys: Vec<ApiKey> = load(c, KEYS)?;
        let before = keys.len();
        keys.retain(|k| k.id != id);
        if keys.len() == before {
            return Err(ApiErr::not_found());
        }
        store(c, KEYS, &keys)?;
        notify(st, c, "keys");
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

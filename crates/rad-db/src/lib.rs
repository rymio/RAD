//! RAD databases: SQL databases people and programs create alongside the vault, run by the
//! Turso engine (an in-process, SQLite-compatible database written in Rust).
//!
//! Each database is one file in a folder next to the vault, encrypted with AEGIS-256 under its
//! own random key. The keys live in the (encrypted) vault, so a database opens only while the
//! vault is unlocked. This crate knows nothing about the vault: it takes the keys it's given.
//!
//! Values cross the API as JSON: null, numbers, strings, and blobs as `{"$blob": "<base64>"}`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as Json};
use turso::Value;

pub use turso;

const CIPHER: &str = "aegis256";
/// Rows one query returns at most (the API pages through bigger tables).
pub const MAX_ROWS: usize = 5000;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("{0}")]
    Sql(String),
    #[error("{0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<turso::Error> for DbError {
    fn from(e: turso::Error) -> Self {
        DbError::Sql(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, DbError>;

/// A new random encryption key (256 bits, hex).
pub fn new_key() -> String {
    use rand::RngCore;
    let mut k = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut k);
    hex::encode(k)
}

/// The open databases in one folder.
pub struct Databases {
    dir: PathBuf,
    open: Mutex<HashMap<String, Arc<turso::Database>>>,
}

fn check_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(DbError::Invalid("invalid database id".into()));
    }
    Ok(())
}

impl Databases {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Databases { dir: dir.into(), open: Mutex::new(HashMap::new()) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, id: &str) -> Result<PathBuf> {
        check_id(id)?;
        Ok(self.dir.join(format!("{id}.db")))
    }

    /// Opens (or creates) a database; kept open until closed.
    pub async fn open(&self, id: &str, key: &str) -> Result<Arc<turso::Database>> {
        if let Some(db) = self.open.lock().unwrap().get(id) {
            return Ok(db.clone());
        }
        let path = self.path(id)?;
        std::fs::create_dir_all(&self.dir)?;
        let db = turso::Builder::new_local(&path.to_string_lossy())
            .experimental_encryption(true)
            .with_encryption(turso::core::EncryptionOpts { cipher: CIPHER.into(), hexkey: key.into() })
            .build()
            .await
            .map_err(|e| DbError::Sql(format!("couldn't open the database: {e}")))?;
        let db = Arc::new(db);
        self.open.lock().unwrap().entry(id.to_string()).or_insert(db.clone());
        Ok(db)
    }

    pub async fn connect(&self, id: &str, key: &str) -> Result<turso::Connection> {
        let db = self.open(id, key).await?;
        let c = db.connect()?;
        // Wait briefly for another writer rather than failing at once.
        let _ = c.execute("PRAGMA busy_timeout = 5000", ()).await;
        Ok(c)
    }

    pub fn close(&self, id: &str) {
        self.open.lock().unwrap().remove(id);
    }

    /// Forgets every open database (the vault, and so the keys, went away).
    pub fn close_all(&self) {
        self.open.lock().unwrap().clear();
    }

    /// Deletes a database's files.
    pub fn remove(&self, id: &str) -> Result<()> {
        self.close(id);
        let p = self.path(id)?;
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let f = PathBuf::from(format!("{}{suffix}", p.display()));
            if f.exists() {
                std::fs::remove_file(f)?;
            }
        }
        Ok(())
    }

    /// Bytes on disk (the file and its write-ahead log).
    pub fn size(&self, id: &str) -> u64 {
        let Ok(p) = self.path(id) else { return 0 };
        ["", "-wal"].iter().filter_map(|s| std::fs::metadata(format!("{}{s}", p.display())).ok()).map(|m| m.len()).sum()
    }
}

// ------------------------------------------------------------------ values

pub fn to_json(v: Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Integer(i) => json!(i),
        Value::Real(f) => serde_json::Number::from_f64(f).map(Json::Number).unwrap_or(Json::Null),
        Value::Text(s) => Json::String(s),
        Value::Blob(b) => json!({"$blob": base64::engine::general_purpose::STANDARD.encode(b)}),
    }
}

pub fn from_json(v: &Json) -> Result<Value> {
    Ok(match v {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Integer(*b as i64),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Real(n.as_f64().unwrap_or(0.0)),
        },
        Json::String(s) => Value::Text(s.clone()),
        Json::Object(o) if o.len() == 1 && o.contains_key("$blob") => {
            let s = o["$blob"].as_str().ok_or_else(|| DbError::Invalid("$blob must be base64 text".into()))?;
            Value::Blob(base64::engine::general_purpose::STANDARD.decode(s).map_err(|_| DbError::Invalid("$blob isn't valid base64".into()))?)
        }
        // Arrays and objects are stored as JSON text.
        other => Value::Text(other.to_string()),
    })
}

/// `"name"` with quotes doubled: safe to put any identifier in SQL.
pub fn ident(name: &str) -> Result<String> {
    if name.is_empty() || name.len() > 128 || name.contains('\0') {
        return Err(DbError::Invalid("invalid name".into()));
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

// ------------------------------------------------------------------ running SQL

#[derive(Debug, Serialize)]
pub struct StatementResult {
    pub sql: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Json>>,
    /// Rows changed by INSERT / UPDATE / DELETE.
    pub changes: u64,
    pub last_insert_rowid: i64,
    /// More rows than MAX_ROWS: the rest were left out.
    pub truncated: bool,
}

/// Splits a script into statements (on `;` outside strings, identifiers and comments).
pub fn split_sql(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' | '`' | '[' => {
                let close = if c == '[' { ']' } else { c };
                cur.push(c);
                while let Some(d) = chars.next() {
                    cur.push(d);
                    if d == close {
                        // A doubled quote is an escaped quote.
                        if close != ']' && chars.peek() == Some(&close) {
                            cur.push(chars.next().unwrap());
                            continue;
                        }
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        cur.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for d in chars.by_ref() {
                    if prev == '*' && d == '/' {
                        break;
                    }
                    prev = d;
                }
                cur.push(' ');
            }
            ';' => {
                // Triggers hold `;` inside BEGIN … END.
                let up = cur.to_uppercase();
                let in_trigger = up.trim_start().starts_with("CREATE") && up.contains(" TRIGGER ") && up.matches("BEGIN").count() > up.matches("END").count();
                if in_trigger {
                    cur.push(';');
                } else {
                    if !cur.trim().is_empty() {
                        out.push(cur.trim().to_string());
                    }
                    cur.clear();
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

async fn run_one(c: &turso::Connection, sql: &str, params: Vec<Value>) -> Result<StatementResult> {
    let mut stmt = c.prepare(sql).await?;
    let columns = stmt.column_names();
    let mut rows = Vec::new();
    let mut truncated = false;
    let mut changes = 0;
    if columns.is_empty() {
        changes = stmt.execute(params).await?;
    } else {
        let mut r = stmt.query(params).await?;
        while let Some(row) = r.next().await? {
            if rows.len() >= MAX_ROWS {
                truncated = true;
                break;
            }
            rows.push((0..columns.len()).map(|i| row.get_value(i).map(to_json).unwrap_or(Json::Null)).collect());
        }
    }
    Ok(StatementResult { sql: sql.to_string(), columns, rows, changes, last_insert_rowid: c.last_insert_rowid(), truncated })
}

/// Runs a script statement by statement. `params` (positional) apply to a single statement.
/// Stops at the first error, which names the statement.
pub async fn run_sql(c: &turso::Connection, sql: &str, params: &[Json]) -> Result<Vec<StatementResult>> {
    let stmts = split_sql(sql);
    if stmts.is_empty() {
        return Err(DbError::Invalid("no SQL to run".into()));
    }
    if !params.is_empty() && stmts.len() > 1 {
        return Err(DbError::Invalid("parameters can only be used with a single statement".into()));
    }
    let params: Vec<Value> = params.iter().map(from_json).collect::<Result<_>>()?;
    let mut out = Vec::new();
    for (i, s) in stmts.iter().enumerate() {
        let r = run_one(c, s, if i == 0 { params.clone() } else { Vec::new() })
            .await
            .map_err(|e| DbError::Sql(if stmts.len() > 1 { format!("statement {}: {e}", i + 1) } else { e.to_string() }))?;
        out.push(r);
    }
    Ok(out)
}

async fn query_all(c: &turso::Connection, sql: &str, params: Vec<Value>) -> Result<Vec<Vec<Value>>> {
    let mut stmt = c.prepare(sql).await?;
    let n = stmt.column_count();
    let mut r = stmt.query(params).await?;
    let mut out = Vec::new();
    while let Some(row) = r.next().await? {
        out.push((0..n).map(|i| row.get_value(i).unwrap_or(Value::Null)).collect());
    }
    Ok(out)
}

fn text(v: &Value) -> String {
    match v {
        Value::Text(s) => s.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        _ => String::new(),
    }
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Integer(i) => *i,
        _ => 0,
    }
}

// ------------------------------------------------------------------ schema

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Column {
    pub name: String,
    /// Declared type: INTEGER, REAL, TEXT, BLOB, NUMERIC… (may be empty).
    #[serde(default, rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub not_null: bool,
    /// SQL default expression, e.g. `0`, `'none'`, `CURRENT_TIMESTAMP`.
    #[serde(default)]
    pub default: Option<String>,
    /// Position in the primary key (0: not part of it).
    #[serde(default)]
    pub pk: i64,
    #[serde(default)]
    pub unique: bool,
}

#[derive(Debug, Serialize)]
pub struct Table {
    pub name: String,
    /// "table" or "view".
    pub kind: String,
    pub sql: String,
    pub columns: Vec<Column>,
    pub indexes: Vec<Json>,
    pub rows: Option<i64>,
}

/// Tables and views (not SQLite's own), with their columns and row counts.
pub async fn tables(c: &turso::Connection) -> Result<Vec<Table>> {
    let list = query_all(c, "SELECT name, type, COALESCE(sql, '') FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name", vec![]).await?;
    let mut out = Vec::new();
    for r in list {
        out.push(describe(c, &text(&r[0]), &text(&r[1]), &text(&r[2])).await?);
    }
    Ok(out)
}

pub async fn table(c: &turso::Connection, name: &str) -> Result<Table> {
    let r = query_all(c, "SELECT name, type, COALESCE(sql, '') FROM sqlite_schema WHERE name = ?1 AND type IN ('table','view')", vec![Value::Text(name.into())]).await?;
    let r = r.into_iter().next().ok_or_else(|| DbError::NotFound(format!("table {name}")))?;
    describe(c, &text(&r[0]), &text(&r[1]), &text(&r[2])).await
}

async fn describe(c: &turso::Connection, name: &str, kind: &str, sql: &str) -> Result<Table> {
    let q = ident(name)?;
    let cols = query_all(c, &format!("PRAGMA table_info({q})"), vec![]).await?;
    let mut columns: Vec<Column> = cols
        .iter()
        .map(|r| Column {
            name: text(&r[1]),
            ty: text(&r[2]),
            not_null: int(&r[3]) != 0,
            default: match &r[4] {
                Value::Null => None,
                v => Some(text(v)),
            },
            pk: int(&r[5]),
            unique: false,
        })
        .collect();
    let mut indexes = Vec::new();
    if kind == "table" {
        for ix in query_all(c, &format!("PRAGMA index_list({q})"), vec![]).await.unwrap_or_default() {
            let iname = text(&ix[1]);
            let unique = int(&ix[2]) != 0;
            let icols: Vec<String> = query_all(c, &format!("PRAGMA index_info({})", ident(&iname)?), vec![]).await.unwrap_or_default().iter().map(|r| text(&r[2])).collect();
            if unique && icols.len() == 1 {
                if let Some(col) = columns.iter_mut().find(|cl| cl.name == icols[0]) {
                    col.unique = true;
                }
            }
            indexes.push(json!({"name": iname, "unique": unique, "columns": icols}));
        }
    }
    let rows = query_all(c, &format!("SELECT count(*) FROM {q}"), vec![]).await.ok().and_then(|r| r.first().map(|r| int(&r[0])));
    Ok(Table { name: name.into(), kind: kind.into(), sql: sql.into(), columns, indexes, rows })
}

fn column_sql(col: &Column, single_pk: bool) -> Result<String> {
    let mut s = ident(col.name.trim())?;
    let ty = col.ty.trim();
    if !ty.is_empty() {
        if !ty.chars().all(|c| c.is_ascii_alphanumeric() || " (),".contains(c)) {
            return Err(DbError::Invalid(format!("invalid type {ty}")));
        }
        s.push(' ');
        s.push_str(ty);
    }
    if single_pk && col.pk > 0 {
        s.push_str(" PRIMARY KEY");
    }
    if col.not_null {
        s.push_str(" NOT NULL");
    }
    if col.unique && col.pk == 0 {
        s.push_str(" UNIQUE");
    }
    if let Some(d) = col.default.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        if d.contains(';') {
            return Err(DbError::Invalid("invalid default".into()));
        }
        s.push_str(&format!(" DEFAULT ({d})"));
    }
    Ok(s)
}

/// `CREATE TABLE` from column definitions.
pub fn create_table_sql(name: &str, columns: &[Column]) -> Result<String> {
    if columns.is_empty() {
        return Err(DbError::Invalid("a table needs at least one column".into()));
    }
    let pks: Vec<&Column> = columns.iter().filter(|c| c.pk > 0).collect();
    let single = pks.len() == 1;
    let mut parts: Vec<String> = columns.iter().map(|c| column_sql(c, single)).collect::<Result<_>>()?;
    if pks.len() > 1 {
        let mut pks = pks;
        pks.sort_by_key(|c| c.pk);
        parts.push(format!("PRIMARY KEY ({})", pks.iter().map(|c| ident(&c.name)).collect::<Result<Vec<_>>>()?.join(", ")));
    }
    Ok(format!("CREATE TABLE {} (\n  {}\n)", ident(name.trim())?, parts.join(",\n  ")))
}

pub async fn create_table(c: &turso::Connection, name: &str, columns: &[Column]) -> Result<Table> {
    c.execute(create_table_sql(name, columns)?, ()).await?;
    table(c, name.trim()).await
}

pub async fn drop_table(c: &turso::Connection, name: &str) -> Result<()> {
    let t = table(c, name).await?;
    let what = if t.kind == "view" { "VIEW" } else { "TABLE" };
    c.execute(format!("DROP {what} {}", ident(name)?), ()).await?;
    Ok(())
}

pub async fn rename_table(c: &turso::Connection, name: &str, to: &str) -> Result<()> {
    c.execute(format!("ALTER TABLE {} RENAME TO {}", ident(name)?, ident(to.trim())?), ()).await?;
    Ok(())
}

pub async fn add_column(c: &turso::Connection, table: &str, col: &Column) -> Result<()> {
    let mut col = col.clone();
    col.pk = 0;
    c.execute(format!("ALTER TABLE {} ADD COLUMN {}", ident(table)?, column_sql(&col, false)?), ()).await?;
    Ok(())
}

pub async fn drop_column(c: &turso::Connection, table: &str, column: &str) -> Result<()> {
    c.execute(format!("ALTER TABLE {} DROP COLUMN {}", ident(table)?, ident(column)?), ()).await?;
    Ok(())
}

pub async fn rename_column(c: &turso::Connection, table: &str, column: &str, to: &str) -> Result<()> {
    c.execute(format!("ALTER TABLE {} RENAME COLUMN {} TO {}", ident(table)?, ident(column)?, ident(to.trim())?), ()).await?;
    Ok(())
}

// ------------------------------------------------------------------ rows

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct RowQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// Column to sort by.
    pub order: Option<String>,
    pub desc: bool,
    /// Text to find in any column.
    pub q: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RowsPage {
    pub columns: Vec<String>,
    /// Each row: its rowid (null for views) and its values, in column order.
    pub rows: Vec<Json>,
    pub total: i64,
    pub offset: usize,
    pub limit: usize,
}

pub async fn rows(c: &turso::Connection, table_name: &str, q: &RowQuery) -> Result<RowsPage> {
    let t = table(c, table_name).await?;
    let qt = ident(&t.name)?;
    let columns: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
    let mut sql_where = String::new();
    let mut params = Vec::new();
    if let Some(needle) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let parts: Vec<String> = columns.iter().map(|c| ident(c).map(|i| format!("CAST({i} AS TEXT) LIKE ?1"))).collect::<Result<_>>()?;
        if !parts.is_empty() {
            sql_where = format!(" WHERE {}", parts.join(" OR "));
            params.push(Value::Text(format!("%{needle}%")));
        }
    }
    let total = query_all(c, &format!("SELECT count(*) FROM {qt}{sql_where}"), params.clone()).await?.first().map(|r| int(&r[0])).unwrap_or(0);
    let order = match q.order.as_deref().filter(|o| columns.iter().any(|c| c == o)) {
        Some(o) => format!(" ORDER BY {}{}", ident(o)?, if q.desc { " DESC" } else { "" }),
        None if t.kind == "table" => format!(" ORDER BY rowid{}", if q.desc { " DESC" } else { "" }),
        None => String::new(),
    };
    let limit = q.limit.unwrap_or(100).clamp(1, MAX_ROWS);
    let offset = q.offset.unwrap_or(0);
    let rowid = if t.kind == "table" { "rowid" } else { "NULL" };
    let sel = std::iter::once(rowid.to_string()).chain(columns.iter().map(|c| ident(c).unwrap())).collect::<Vec<_>>().join(", ");
    let data = query_all(c, &format!("SELECT {sel} FROM {qt}{sql_where}{order} LIMIT {limit} OFFSET {offset}"), params).await?;
    let rows = data
        .into_iter()
        .map(|mut r| {
            let id = to_json(r.remove(0));
            json!({"rowid": id, "values": r.into_iter().map(to_json).collect::<Vec<_>>()})
        })
        .collect();
    Ok(RowsPage { columns, rows, total, offset, limit })
}

fn assignments(values: &Map<String, Json>) -> Result<(Vec<String>, Vec<Value>)> {
    if values.is_empty() {
        return Err(DbError::Invalid("no values".into()));
    }
    let mut names = Vec::new();
    let mut vals = Vec::new();
    for (k, v) in values {
        names.push(ident(k)?);
        vals.push(from_json(v)?);
    }
    Ok((names, vals))
}

/// Inserts a row (`{column: value}`); returns its rowid.
pub async fn insert(c: &turso::Connection, table: &str, values: &Map<String, Json>) -> Result<i64> {
    let qt = ident(table)?;
    if values.is_empty() {
        c.execute(format!("INSERT INTO {qt} DEFAULT VALUES"), ()).await?;
    } else {
        let (names, vals) = assignments(values)?;
        let marks: Vec<String> = (1..=vals.len()).map(|i| format!("?{i}")).collect();
        c.execute(format!("INSERT INTO {qt} ({}) VALUES ({})", names.join(", "), marks.join(", ")), vals).await?;
    }
    Ok(c.last_insert_rowid())
}

pub async fn update(c: &turso::Connection, table: &str, rowid: i64, values: &Map<String, Json>) -> Result<u64> {
    let (names, mut vals) = assignments(values)?;
    let set: Vec<String> = names.iter().enumerate().map(|(i, n)| format!("{n} = ?{}", i + 1)).collect();
    vals.push(Value::Integer(rowid));
    let n = c.execute(format!("UPDATE {} SET {} WHERE rowid = ?{}", ident(table)?, set.join(", "), vals.len()), vals).await?;
    if n == 0 {
        return Err(DbError::NotFound(format!("row {rowid}")));
    }
    Ok(n)
}

pub async fn delete(c: &turso::Connection, table: &str, rowids: &[i64]) -> Result<u64> {
    let qt = ident(table)?;
    let mut n = 0;
    for id in rowids {
        n += c.execute(format!("DELETE FROM {qt} WHERE rowid = ?1"), vec![Value::Integer(*id)]).await?;
    }
    Ok(n)
}

pub async fn get_row(c: &turso::Connection, table: &str, rowid: i64) -> Result<Json> {
    let t = self::table(c, table).await?;
    let cols: Vec<String> = t.columns.iter().map(|c| ident(&c.name)).collect::<Result<_>>()?;
    let r = query_all(c, &format!("SELECT {} FROM {} WHERE rowid = ?1", cols.join(", "), ident(table)?), vec![Value::Integer(rowid)]).await?;
    let r = r.into_iter().next().ok_or_else(|| DbError::NotFound(format!("row {rowid}")))?;
    let mut m = Map::new();
    m.insert("rowid".into(), json!(rowid));
    for (col, v) in t.columns.iter().zip(r) {
        m.insert(col.name.clone(), to_json(v));
    }
    Ok(Json::Object(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_scripts() {
        let s = split_sql("CREATE TABLE a(x); -- note; here\nINSERT INTO a VALUES ('x;y', \"q\"\"u;\"); /* ; */ SELECT 1;;");
        assert_eq!(s, ["CREATE TABLE a(x)", "INSERT INTO a VALUES ('x;y', \"q\"\"u;\")", "SELECT 1"]);
        let t = split_sql("CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE a SET x = 1; END; SELECT 2");
        assert_eq!(t.len(), 2, "{t:?}");
    }

    #[test]
    fn quotes_names() {
        assert_eq!(ident("we\"ird").unwrap(), "\"we\"\"ird\"");
        assert!(ident("").is_err());
        let sql = create_table_sql("people", &[
            Column { name: "id".into(), ty: "INTEGER".into(), not_null: false, default: None, pk: 1, unique: false },
            Column { name: "email".into(), ty: "TEXT".into(), not_null: true, default: None, pk: 0, unique: true },
            Column { name: "age".into(), ty: "INTEGER".into(), not_null: false, default: Some("0".into()), pk: 0, unique: false },
        ])
        .unwrap();
        assert_eq!(sql, "CREATE TABLE \"people\" (\n  \"id\" INTEGER PRIMARY KEY,\n  \"email\" TEXT NOT NULL UNIQUE,\n  \"age\" INTEGER DEFAULT (0)\n)");
        assert!(create_table_sql("x", &[Column { name: "a".into(), ty: "TEXT); DROP TABLE y; --".into(), not_null: false, default: None, pk: 0, unique: false }]).is_err());
    }

    #[tokio::test]
    async fn crud_on_an_encrypted_database() {
        let dir = tempfile::tempdir().unwrap();
        let dbs = Databases::new(dir.path());
        let key = new_key();
        let c = dbs.connect("main", &key).await.unwrap();
        create_table(&c, "people", &[
            Column { name: "id".into(), ty: "INTEGER".into(), not_null: false, default: None, pk: 1, unique: false },
            Column { name: "name".into(), ty: "TEXT".into(), not_null: true, default: None, pk: 0, unique: false },
            Column { name: "photo".into(), ty: "BLOB".into(), not_null: false, default: None, pk: 0, unique: false },
        ])
        .await
        .unwrap();
        let mut v = Map::new();
        v.insert("name".into(), json!("Ada secret-name"));
        v.insert("photo".into(), json!({"$blob": "AAEC"}));
        let id = insert(&c, "people", &v).await.unwrap();
        let mut v2 = Map::new();
        v2.insert("name".into(), json!("Grace"));
        insert(&c, "people", &v2).await.unwrap();

        let page = rows(&c, "people", &RowQuery { q: Some("ada".into()), ..Default::default() }).await.unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0]["values"][2], json!({"$blob": "AAEC"}));

        let mut up = Map::new();
        up.insert("name".into(), json!("Ada Lovelace"));
        update(&c, "people", id, &up).await.unwrap();
        assert_eq!(get_row(&c, "people", id).await.unwrap()["name"], "Ada Lovelace");

        add_column(&c, "people", &Column { name: "age".into(), ty: "INTEGER".into(), not_null: false, default: Some("0".into()), pk: 0, unique: false }).await.unwrap();
        let t = table(&c, "people").await.unwrap();
        assert_eq!(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "name", "photo", "age"]);
        assert_eq!(t.rows, Some(2));

        let res = run_sql(&c, "UPDATE people SET age = 36 WHERE name LIKE 'Ada%'; SELECT name, age FROM people ORDER BY id", &[]).await.unwrap();
        assert_eq!(res[0].changes, 1);
        assert_eq!(res[1].rows, vec![vec![json!("Ada Lovelace"), json!(36)], vec![json!("Grace"), json!(0)]]);
        let res = run_sql(&c, "SELECT ?1 + 1 AS n", &[json!(41)]).await.unwrap();
        assert_eq!(res[0].rows[0][0], json!(42));
        assert!(run_sql(&c, "SELECT * FROM nope", &[]).await.is_err());

        assert_eq!(delete(&c, "people", &[id]).await.unwrap(), 1);
        rename_table(&c, "people", "folks").await.unwrap();
        assert_eq!(tables(&c).await.unwrap().iter().map(|t| t.name.clone()).collect::<Vec<_>>(), ["folks"]);
        let _ = run_sql(&c, "PRAGMA wal_checkpoint(TRUNCATE)", &[]).await;
        drop(c);
        dbs.close_all();

        // Encrypted on disk, and only the right key opens it.
        let raw = std::fs::read(dbs.path("main").unwrap()).unwrap();
        assert!(!raw.windows(5).any(|w| w == b"Grace"));
        let wrong = async {
            let c = dbs.connect("main", &new_key()).await?;
            run_sql(&c, "SELECT * FROM folks", &[]).await
        };
        assert!(wrong.await.is_err(), "a wrong key must not read the database");
        dbs.close_all();
        let c = dbs.connect("main", &key).await.unwrap();
        assert_eq!(table(&c, "folks").await.unwrap().rows, Some(1));
        drop(c);
        dbs.remove("main").unwrap();
        assert!(!dbs.path("main").unwrap().exists());
    }
}

#[cfg(test)]
mod games_schema {
    use super::*;

    /// The SQL the Games view runs (app/ui/src/lib/games.ts).
    #[tokio::test]
    async fn games_view_sql_works() {
        let dir = tempfile::tempdir().unwrap();
        let dbs = Databases::new(dir.path());
        let c = dbs.connect("games", &new_key()).await.unwrap();
        let schema = "CREATE TABLE IF NOT EXISTS games (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS files (game_id TEXT NOT NULL, path TEXT NOT NULL, content TEXT NOT NULL, PRIMARY KEY (game_id, path));
CREATE TABLE IF NOT EXISTS chats (game_id TEXT PRIMARY KEY, turns TEXT NOT NULL)";
        run_sql(&c, schema, &[]).await.unwrap();
        run_sql(&c, schema, &[]).await.unwrap(); // twice: IF NOT EXISTS
        run_sql(&c, "INSERT INTO games (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)", &[json!("g1"), json!("Dodge"), json!(1)]).await.unwrap();
        for content in ["fn main() {}", "fn main() { println!(\"v2\"); }"] {
            run_sql(&c, "INSERT OR REPLACE INTO files (game_id, path, content) VALUES (?1, ?2, ?3)", &[json!("g1"), json!("src/main.rs"), json!(content)]).await.unwrap();
        }
        run_sql(&c, "INSERT OR REPLACE INTO chats (game_id, turns) VALUES (?1, ?2)", &[json!("g1"), json!("[]")]).await.unwrap();
        let r = run_sql(&c, "SELECT path, content FROM files WHERE game_id = ?1 ORDER BY path", &[json!("g1")]).await.unwrap();
        assert_eq!(r[0].rows, vec![vec![json!("src/main.rs"), json!("fn main() { println!(\"v2\"); }")]]);
        run_sql(&c, "DELETE FROM files WHERE game_id = ?1 AND path = ?2", &[json!("g1"), json!("src/main.rs")]).await.unwrap();
        let r = run_sql(&c, "SELECT count(*) FROM files", &[]).await.unwrap();
        assert_eq!(r[0].rows[0][0], json!(0));

        // Assets: binary data, folders, moving a folder with what's in it, deleting a folder.
        run_sql(&c, "CREATE TABLE IF NOT EXISTS assets (game_id TEXT NOT NULL, path TEXT NOT NULL, data BLOB NOT NULL, size INTEGER NOT NULL, info TEXT NOT NULL DEFAULT '', PRIMARY KEY (game_id, path))", &[]).await.unwrap();
        let put = "INSERT OR REPLACE INTO assets (game_id, path, data, size, info) VALUES (?1, ?2, ?3, ?4, ?5)";
        for (p, b) in [("sprites/", ""), ("sprites/player.png", "iVBORw0KGgo="), ("sprites/enemies/bat.png", "AAEC"), ("sfx/jump.ogg", "T2dnUw=="), ("spritesheet.png", "AA==")] {
            run_sql(&c, put, &[json!("g1"), json!(p), json!({"$blob": b}), json!(3), json!("")]).await.unwrap();
        }
        let mv = "UPDATE assets SET path = ?3 || substr(path, ?4) WHERE game_id = ?1 AND substr(path, 1, ?5) = ?2";
        let (from, to) = ("sprites/", "art/sprites/");
        run_sql(&c, mv, &[json!("g1"), json!(from), json!(to), json!(from.len() + 1), json!(from.len())]).await.unwrap();
        let paths = |rows: &Vec<Vec<Json>>| rows.iter().map(|r| r[0].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let r = run_sql(&c, "SELECT path, data FROM assets WHERE game_id = ?1 ORDER BY path", &[json!("g1")]).await.unwrap();
        assert_eq!(paths(&r[0].rows), ["art/sprites/", "art/sprites/enemies/bat.png", "art/sprites/player.png", "sfx/jump.ogg", "spritesheet.png"], "spritesheet.png isn't in sprites/");
        assert_eq!(r[0].rows[2][1], json!({"$blob": "iVBORw0KGgo="}));
        let del = "DELETE FROM assets WHERE game_id = ?1 AND substr(path, 1, ?3) = ?2";
        run_sql(&c, del, &[json!("g1"), json!("art/"), json!(4)]).await.unwrap();
        let r = run_sql(&c, "SELECT path FROM assets ORDER BY path", &[]).await.unwrap();
        assert_eq!(paths(&r[0].rows), ["sfx/jump.ogg", "spritesheet.png"]);
    }
}

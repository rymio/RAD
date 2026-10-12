use rusqlite::Connection;

use crate::Result;

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
CREATE TABLE IF NOT EXISTS vault_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE projects(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT '#7c6cf2',
    icon TEXT NOT NULL DEFAULT 'folder',
    system_prompt TEXT NOT NULL DEFAULT '',
    archived INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE blobs(
    id INTEGER PRIMARY KEY,
    hash TEXT UNIQUE,
    size INTEGER NOT NULL DEFAULT 0,
    mime TEXT NOT NULL DEFAULT 'application/octet-stream',
    chunk_count INTEGER NOT NULL DEFAULT 0,
    complete INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE blob_chunks(
    blob_id INTEGER NOT NULL REFERENCES blobs(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY(blob_id, seq)
) WITHOUT ROWID;

CREATE TABLE thumbs(
    blob_id INTEGER PRIMARY KEY REFERENCES blobs(id) ON DELETE CASCADE,
    mime TEXT NOT NULL,
    data BLOB NOT NULL
);

CREATE TABLE documents(
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    blob_id INTEGER NOT NULL REFERENCES blobs(id),
    title TEXT NOT NULL,
    original_name TEXT NOT NULL,
    mime TEXT NOT NULL,
    size INTEGER NOT NULL,
    source_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT,
    notice TEXT,
    page_count INTEGER,
    chunk_count INTEGER NOT NULL DEFAULT 0,
    tags TEXT NOT NULL DEFAULT '[]',
    pinned INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX documents_project ON documents(project_id, updated_at DESC);
CREATE INDEX documents_blob ON documents(blob_id);

CREATE TABLE chunks(
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    project_id INTEGER NOT NULL,
    ord INTEGER NOT NULL,
    text TEXT NOT NULL,
    context TEXT NOT NULL DEFAULT '',
    anchor TEXT NOT NULL DEFAULT '{}',
    kind TEXT NOT NULL DEFAULT 'text'
);
CREATE INDEX chunks_document ON chunks(document_id, ord);
CREATE INDEX chunks_project ON chunks(project_id);

CREATE VIRTUAL TABLE chunks_fts USING fts5(
    text, context,
    content='chunks', content_rowid='id',
    tokenize='porter unicode61 remove_diacritics 2'
);
CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, text, context) VALUES (new.id, new.text, new.context);
END;
CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text, context) VALUES ('delete', old.id, old.text, old.context);
END;
CREATE TRIGGER chunks_au AFTER UPDATE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text, context) VALUES ('delete', old.id, old.text, old.context);
    INSERT INTO chunks_fts(rowid, text, context) VALUES (new.id, new.text, new.context);
END;

CREATE TABLE embedding_spaces(
    id INTEGER PRIMARY KEY,
    model_key TEXT NOT NULL UNIQUE,
    dims INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE embedded(
    space_id INTEGER NOT NULL,
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    PRIMARY KEY(space_id, document_id)
) WITHOUT ROWID;

CREATE TABLE chats(
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT '{"projects":[]}',
    model TEXT,
    mode TEXT NOT NULL DEFAULT 'rag',
    pinned INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE messages(
    id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    attachments TEXT NOT NULL DEFAULT '[]',
    citations TEXT NOT NULL DEFAULT '[]',
    model TEXT,
    error TEXT,
    tokens_in INTEGER,
    tokens_out INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX messages_chat ON messages(chat_id, id);

CREATE TABLE memories(
    id INTEGER PRIMARY KEY,
    text TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);

CREATE TABLE providers(
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    api_key TEXT,
    headers TEXT NOT NULL DEFAULT '{}',
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);

CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE jobs(
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    document_id INTEGER,
    payload TEXT NOT NULL DEFAULT '{}',
    state TEXT NOT NULL DEFAULT 'queued',
    attempts INTEGER NOT NULL DEFAULT 0,
    run_after INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX jobs_state ON jobs(state, run_after);

CREATE TABLE change_log(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    entity TEXT NOT NULL,
    entity_id INTEGER NOT NULL,
    op TEXT NOT NULL,
    at INTEGER NOT NULL
);
"#,
];

pub fn migrate(c: &Connection) -> Result<()> {
    let current: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = i as i64 + 1;
        if v > current {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {v};"))?;
            tx.commit()?;
            tracing::info!("vault schema migrated to v{v}");
        }
    }
    Ok(())
}

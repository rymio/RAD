//! SQLite storage for Aussie Accent Coach.
//!
//! Tables follow the scope: `texts`, `references_`, `voice_profiles`, `attempts`,
//! `feature_stats`. Migrations are numbered and tracked with `PRAGMA user_version`.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("not found")]
    NotFound,
}

pub type Result<T> = std::result::Result<T, StoreError>;

const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r#"
    CREATE TABLE texts (
        id          INTEGER PRIMARY KEY,
        title       TEXT NOT NULL,
        body        TEXT NOT NULL,
        sentences   TEXT NOT NULL,          -- JSON array of strings
        created_at  INTEGER NOT NULL,
        practised_at INTEGER
    );
    CREATE TABLE references_ (
        id          INTEGER PRIMARY KEY,
        text_id     INTEGER NOT NULL REFERENCES texts(id) ON DELETE CASCADE,
        source      TEXT NOT NULL,          -- narration | system_voice | cloned
        source_id   TEXT NOT NULL,          -- file path or voice name
        spans       TEXT,                   -- JSON sentence spans for narration
        created_at  INTEGER NOT NULL
    );
    CREATE TABLE voice_profiles (
        id          INTEGER PRIMARY KEY,
        name        TEXT NOT NULL,
        kind        TEXT NOT NULL,          -- learner | cloned
        data        TEXT NOT NULL,          -- JSON calibration or cloning data
        consent     TEXT,                   -- consent record, required for kind = cloned
        created_at  INTEGER NOT NULL,
        CHECK (kind != 'cloned' OR consent IS NOT NULL)
    );
    CREATE TABLE attempts (
        id            INTEGER PRIMARY KEY,
        text_id       INTEGER REFERENCES texts(id) ON DELETE SET NULL,
        sentence_idx  INTEGER NOT NULL,
        sentence      TEXT NOT NULL,
        audio_path    TEXT,
        analysis      TEXT NOT NULL,        -- JSON CoachInput
        coach         TEXT NOT NULL,        -- JSON CoachReply
        session       INTEGER NOT NULL,
        created_at    INTEGER NOT NULL
    );
    CREATE INDEX attempts_session ON attempts(session);
    CREATE TABLE feature_stats (
        session       INTEGER NOT NULL,
        feature       TEXT NOT NULL,
        attempts      INTEGER NOT NULL,
        flagged       INTEGER NOT NULL,     -- attempts where the feature was an issue
        severity_sum  REAL NOT NULL,
        PRIMARY KEY (session, feature)
    );
    "#,
    // 2: app settings (chosen voice, ...)
    r#"
    CREATE TABLE settings (
        key         TEXT PRIMARY KEY,
        value       TEXT NOT NULL
    );
    "#,
    // 3: documents (an imported PDF); each page with text is a row in `texts`
    r#"
    CREATE TABLE documents (
        id          INTEGER PRIMARY KEY,
        title       TEXT NOT NULL,
        source      TEXT NOT NULL,          -- original file path
        last_text   INTEGER,                -- page last opened (texts.id)
        created_at  INTEGER NOT NULL
    );
    ALTER TABLE texts ADD COLUMN doc_id INTEGER REFERENCES documents(id) ON DELETE CASCADE;
    ALTER TABLE texts ADD COLUMN page_no INTEGER;
    CREATE INDEX texts_doc ON texts(doc_id, page_no);
    "#,
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRow {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub sentences: Vec<String>,
    pub created_at: i64,
    pub practised_at: Option<i64>,
    /// Set for a page of an imported document.
    pub doc_id: Option<i64>,
    /// The document's page number (1-based), for a page of a document.
    pub page_no: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentRow {
    pub id: i64,
    pub title: String,
    pub source: String,
    pub last_text: Option<i64>,
    /// `(page_no, text id)` of every page with text, in page order.
    pub pages: Vec<(i64, i64)>,
    /// Most recent practice on any page.
    pub practised_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewAttempt<'a> {
    pub text_id: Option<i64>,
    pub sentence_idx: usize,
    pub sentence: &'a str,
    pub audio_path: Option<&'a str>,
    pub analysis_json: &'a str,
    pub coach_json: &'a str,
    pub session: i64,
    /// `(feature, severity)` of every issue in this attempt.
    pub issues: &'a [(String, f32)],
}

/// Features tracked in session memory even when an attempt has no issue for them.
pub const TRACKED_FEATURES: &[&str] =
    &[
        "PACE", "PAUSES", "INTONATION", "FINAL_RISE", "PITCH_RANGE", "WORD_SOUND", "MISSED_WORDS", "FACE", "PRICE", "GOAT",
        "MOUTH", "GOOSE", "NON_RHOTIC",
    ];

pub struct Store {
    conn: Connection,
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            conn.execute_batch(&format!("BEGIN; {sql}; PRAGMA user_version = {}; COMMIT;", i + 1))?;
        }
        Ok(Self { conn })
    }

    pub fn add_text(&self, title: &str, body: &str, sentences: &[String]) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO texts (title, body, sentences, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![title, body, serde_json::to_string(sentences)?, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn text(&self, id: i64) -> Result<TextRow> {
        self.conn
            .query_row(&format!("SELECT {TEXT_COLUMNS} FROM texts WHERE id = ?1"), [id], row_to_text)
            .optional()?
            .ok_or(StoreError::NotFound)?
    }

    /// Texts (not document pages), most recently practised (then created) first.
    pub fn texts(&self) -> Result<Vec<TextRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TEXT_COLUMNS} FROM texts WHERE doc_id IS NULL
             ORDER BY COALESCE(practised_at, created_at) DESC, id DESC"
        ))?;
        let rows = stmt.query_map([], row_to_text)?;
        rows.map(|r| r?).collect()
    }

    /// Add a document and its pages, `(page_no, body, sentences)`, in one transaction.
    pub fn add_document(&mut self, title: &str, source: &str, pages: &[(i64, String, Vec<String>)]) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute("INSERT INTO documents (title, source, created_at) VALUES (?1, ?2, ?3)", params![title, source, now()])?;
        let doc = tx.last_insert_rowid();
        for (page_no, body, sentences) in pages {
            tx.execute(
                "INSERT INTO texts (title, body, sentences, created_at, doc_id, page_no) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![format!("{title}, page {page_no}"), body, serde_json::to_string(sentences)?, now(), doc, page_no],
            )?;
        }
        tx.commit()?;
        Ok(doc)
    }

    pub fn document(&self, id: i64) -> Result<DocumentRow> {
        let (title, source, last_text): (String, String, Option<i64>) = self
            .conn
            .query_row("SELECT title, source, last_text FROM documents WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?
            .ok_or(StoreError::NotFound)?;
        let mut stmt = self.conn.prepare("SELECT page_no, id, practised_at FROM texts WHERE doc_id = ?1 ORDER BY page_no")?;
        let rows: Vec<(i64, i64, Option<i64>)> =
            stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(DocumentRow {
            id,
            title,
            source,
            last_text,
            pages: rows.iter().map(|(p, t, _)| (*p, *t)).collect(),
            practised_at: rows.iter().filter_map(|r| r.2).max(),
        })
    }

    /// Documents, most recently practised (then created) first.
    pub fn documents(&self) -> Result<Vec<DocumentRow>> {
        let ids: Vec<i64> = self
            .conn
            .prepare(
                "SELECT d.id FROM documents d LEFT JOIN texts t ON t.doc_id = d.id
                 GROUP BY d.id ORDER BY COALESCE(MAX(t.practised_at), d.created_at) DESC, d.id DESC",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        ids.into_iter().map(|id| self.document(id)).collect()
    }

    /// Remember the page last opened.
    pub fn set_document_page(&self, doc_id: i64, text_id: i64) -> Result<()> {
        self.conn.execute("UPDATE documents SET last_text = ?1 WHERE id = ?2", params![text_id, doc_id])?;
        Ok(())
    }

    pub fn delete_document(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM documents WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn delete_text(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM texts WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_reference(&self, text_id: i64, source: &str, source_id: &str, spans_json: Option<&str>) -> Result<()> {
        self.conn.execute("DELETE FROM references_ WHERE text_id = ?1", [text_id])?;
        self.conn.execute(
            "INSERT INTO references_ (text_id, source, source_id, spans, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![text_id, source, source_id, spans_json, now()],
        )?;
        Ok(())
    }

    /// `(source, source_id)` of a text's reference.
    pub fn reference(&self, text_id: i64) -> Result<Option<(String, String)>> {
        Ok(self
            .conn
            .query_row("SELECT source, source_id FROM references_ WHERE text_id = ?1", [text_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    /// The learner's calibration profile (JSON), if saved.
    pub fn learner_profile(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT data FROM voice_profiles WHERE kind = 'learner' ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
            .optional()?)
    }

    pub fn set_learner_profile(&self, data_json: &str) -> Result<()> {
        self.conn.execute("DELETE FROM voice_profiles WHERE kind = 'learner'", [])?;
        self.conn.execute(
            "INSERT INTO voice_profiles (name, kind, data, created_at) VALUES ('me', 'learner', ?1, ?2)",
            params![data_json, now()],
        )?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Add a cloned (neural) voice. `consent` records the speaker's permission and is required.
    pub fn add_cloned_voice(&self, name: &str, data_json: &str, consent: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO voice_profiles (name, kind, data, consent, created_at) VALUES (?1, 'cloned', ?2, ?3, ?4)",
            params![name, data_json, consent, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// `(id, name, data)` of every cloned voice, oldest first.
    pub fn cloned_voices(&self) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self.conn.prepare("SELECT id, name, data FROM voice_profiles WHERE kind = 'cloned' ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn delete_cloned_voice(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM voice_profiles WHERE id = ?1 AND kind = 'cloned'", [id])?;
        Ok(())
    }

    /// Next session number (one session per app launch).
    pub fn new_session(&self) -> Result<i64> {
        let max: Option<i64> = self.conn.query_row("SELECT MAX(session) FROM attempts", [], |r| r.get(0))?;
        let max_stats: Option<i64> = self.conn.query_row("SELECT MAX(session) FROM feature_stats", [], |r| r.get(0))?;
        Ok(max.max(max_stats).unwrap_or(0) + 1)
    }

    pub fn add_attempt(&mut self, a: &NewAttempt<'_>) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO attempts (text_id, sentence_idx, sentence, audio_path, analysis, coach, session, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![a.text_id, a.sentence_idx as i64, a.sentence, a.audio_path, a.analysis_json, a.coach_json, a.session, now()],
        )?;
        let id = tx.last_insert_rowid();
        for feature in TRACKED_FEATURES {
            let sev = a.issues.iter().filter(|(f, _)| f == feature).map(|(_, s)| *s).fold(0.0f32, f32::max);
            let flagged = i64::from(sev > 0.0);
            tx.execute(
                "INSERT INTO feature_stats (session, feature, attempts, flagged, severity_sum) VALUES (?1, ?2, 1, ?3, ?4)
                 ON CONFLICT(session, feature) DO UPDATE SET
                    attempts = attempts + 1, flagged = flagged + excluded.flagged, severity_sum = severity_sum + excluded.severity_sum",
                params![a.session, feature, flagged, sev as f64],
            )?;
        }
        if let Some(t) = a.text_id {
            tx.execute("UPDATE texts SET practised_at = ?1 WHERE id = ?2", params![now(), t])?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// Mean severity per feature for each of the last `sessions` sessions, oldest first.
    pub fn feature_trend(&self, feature: &str, sessions: usize) -> Result<Vec<(i64, f32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT session, severity_sum / attempts FROM feature_stats WHERE feature = ?1 ORDER BY session DESC LIMIT ?2",
        )?;
        let mut rows: Vec<(i64, f32)> = stmt
            .query_map(params![feature, sessions as i64], |r| Ok((r.get(0)?, r.get::<_, f64>(1)? as f32)))?
            .collect::<std::result::Result<_, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// One line per feature for the coach, e.g. `"PACE" → "improving, 4 sessions"`.
    pub fn coach_history(&self) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for feature in TRACKED_FEATURES {
            let trend = self.feature_trend(feature, 6)?;
            if let Some(line) = describe_trend(&trend) {
                out.insert((*feature).to_string(), line);
            }
        }
        Ok(out)
    }
}

/// Plain-words trend from mean severities (lower is better).
pub fn describe_trend(trend: &[(i64, f32)]) -> Option<String> {
    let n = trend.len();
    if n == 0 {
        return None;
    }
    let latest = trend[n - 1].1;
    let state = if n == 1 {
        if latest < 0.15 { "good" } else { "needs work" }.to_string()
    } else {
        let earlier = trend[..n - 1].iter().map(|t| t.1).sum::<f32>() / (n - 1) as f32;
        let delta = latest - earlier;
        match (latest < 0.15, delta) {
            (true, _) => "good".to_string(),
            (false, d) if d < -0.1 => "improving".to_string(),
            (false, d) if d > 0.1 => "slipping".to_string(),
            _ => "stable, needs work".to_string(),
        }
    };
    Some(format!("{state}, {n} session{}", if n == 1 { "" } else { "s" }))
}

const TEXT_COLUMNS: &str = "id, title, body, sentences, created_at, practised_at, doc_id, page_no";

fn row_to_text(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<TextRow>> {
    let sentences: String = r.get(3)?;
    Ok(serde_json::from_str(&sentences).map_err(StoreError::from).map(|sentences| TextRow {
        id: r.get(0).unwrap_or_default(),
        title: r.get(1).unwrap_or_default(),
        body: r.get(2).unwrap_or_default(),
        sentences,
        created_at: r.get(4).unwrap_or_default(),
        practised_at: r.get(5).unwrap_or_default(),
        doc_id: r.get(6).unwrap_or_default(),
        page_no: r.get(7).unwrap_or_default(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt<'a>(session: i64, text: Option<i64>, issues: &'a [(String, f32)]) -> NewAttempt<'a> {
        NewAttempt {
            text_id: text,
            sentence_idx: 0,
            sentence: "G'day mate.",
            audio_path: None,
            analysis_json: "{}",
            coach_json: "{}",
            session,
            issues,
        }
    }

    #[test]
    fn texts_round_trip_and_order_by_practice() {
        let mut s = Store::open_in_memory().unwrap();
        let a = s.add_text("One", "Hi. There.", &["Hi.".into(), "There.".into()]).unwrap();
        let b = s.add_text("Two", "Yo.", &["Yo.".into()]).unwrap();
        assert_eq!(s.text(a).unwrap().sentences, vec!["Hi.", "There."]);
        assert_eq!(s.texts().unwrap()[0].id, b);
        s.add_attempt(&attempt(1, Some(a), &[])).unwrap();
        assert!(s.text(a).unwrap().practised_at.is_some());
        s.delete_text(b).unwrap();
        assert!(matches!(s.text(b), Err(StoreError::NotFound)));
    }

    #[test]
    fn references_replace() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_text("T", "x", &["x".into()]).unwrap();
        s.set_reference(t, "narration", "/a.wav", Some("[]")).unwrap();
        s.set_reference(t, "system_voice", "Karen", None).unwrap();
        assert_eq!(s.reference(t).unwrap(), Some(("system_voice".into(), "Karen".into())));
    }

    #[test]
    fn learner_profile_replaces() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.learner_profile().unwrap(), None);
        s.set_learner_profile("{\"median_f0_hz\":120}").unwrap();
        s.set_learner_profile("{\"median_f0_hz\":125}").unwrap();
        assert_eq!(s.learner_profile().unwrap().as_deref(), Some("{\"median_f0_hz\":125}"));
    }

    #[test]
    fn documents_keep_pages_out_of_the_text_list() {
        let mut s = Store::open_in_memory().unwrap();
        let loose = s.add_text("Loose", "x", &["x".into()]).unwrap();
        let pages = vec![(1, "One.".to_string(), vec!["One.".to_string()]), (3, "Three.".to_string(), vec!["Three.".to_string()])];
        let doc = s.add_document("Book", "/b.pdf", &pages).unwrap();
        assert_eq!(s.texts().unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), vec![loose]);
        let d = s.document(doc).unwrap();
        assert_eq!(d.pages.iter().map(|p| p.0).collect::<Vec<_>>(), vec![1, 3]);
        let page3 = s.text(d.pages[1].1).unwrap();
        assert_eq!((page3.doc_id, page3.page_no, page3.title.as_str()), (Some(doc), Some(3), "Book, page 3"));
        s.set_document_page(doc, page3.id).unwrap();
        assert_eq!(s.documents().unwrap()[0].last_text, Some(page3.id));
        s.delete_document(doc).unwrap();
        assert!(s.text(page3.id).is_err());
        assert!(s.documents().unwrap().is_empty());
    }

    #[test]
    fn settings_replace() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.setting("voice").unwrap(), None);
        s.set_setting("voice", "Karen").unwrap();
        s.set_setting("voice", "Lee (Premium)").unwrap();
        assert_eq!(s.setting("voice").unwrap().as_deref(), Some("Lee (Premium)"));
    }

    #[test]
    fn cloned_voices_round_trip() {
        let s = Store::open_in_memory().unwrap();
        s.set_learner_profile("{}").unwrap();
        let id = s.add_cloned_voice("Jack", "{\"clip\":\"/a.wav\"}", "self, 2026-10-21").unwrap();
        assert_eq!(s.cloned_voices().unwrap(), vec![(id, "Jack".into(), "{\"clip\":\"/a.wav\"}".into())]);
        s.delete_cloned_voice(id).unwrap();
        assert!(s.cloned_voices().unwrap().is_empty());
        assert!(s.learner_profile().unwrap().is_some());
    }

    #[test]
    fn cloned_voice_requires_consent() {
        let s = Store::open_in_memory().unwrap();
        let err = s.conn.execute(
            "INSERT INTO voice_profiles (name, kind, data, created_at) VALUES ('x', 'cloned', '{}', 0)",
            [],
        );
        assert!(err.is_err());
    }

    #[test]
    fn feature_stats_feed_coach_history() {
        let mut s = Store::open_in_memory().unwrap();
        let bad = [("PACE".to_string(), 0.8)];
        let ok = [("PACE".to_string(), 0.3)];
        for _ in 0..3 {
            s.add_attempt(&attempt(1, None, &bad)).unwrap();
        }
        for _ in 0..3 {
            s.add_attempt(&attempt(2, None, &ok)).unwrap();
        }
        assert_eq!(s.new_session().unwrap(), 3);
        let h = s.coach_history().unwrap();
        assert_eq!(h["PACE"], "improving, 2 sessions");
        assert_eq!(h["INTONATION"], "good, 2 sessions");
    }

    #[test]
    fn migrations_are_idempotent() {
        let dir = std::env::temp_dir().join(format!("aac-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        let t = Store::open(&path).unwrap().add_text("T", "x", &["x".into()]).unwrap();
        assert_eq!(Store::open(&path).unwrap().text(t).unwrap().title, "T");
        std::fs::remove_dir_all(dir).ok();
    }
}

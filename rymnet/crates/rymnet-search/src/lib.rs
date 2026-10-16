use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rymnet_core::{NodeIdentity, canonical_json, sign_json_value, verify_json_value};
use rymnet_site::{SiteManifest, list_latest_manifests, load_latest_manifest};
use rymnet_storage::ChunkStore;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::fs;
use std::path::{Path, PathBuf};
use tantivy::{
    Index, TantivyDocument,
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::{Field, STORED, STRING, Schema, TEXT, Value},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchDocument {
    pub domain: String,
    pub path: String,
    pub title: String,
    pub description: String,
    pub headings: Vec<String>,
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub domain: String,
    pub path: String,
    pub title: String,
    pub description: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndexStatus {
    pub documents: usize,
}

#[derive(Clone)]
struct SearchFields {
    domain: Field,
    path: Field,
    title: Field,
    description: Field,
    headings: Field,
    keywords: Field,
    body: Field,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub domain: String,
    pub path: String,
    pub title: String,
    pub description: String,
    pub headings: Vec<String>,
    pub keywords: Vec<String>,
    pub content_hash: String,
    pub manifest_hash: String,
    pub published_at: DateTime<Utc>,
    pub signature: String,
}

impl SearchRecord {
    pub fn verify(&self, owner_public_key: &str) -> Result<bool> {
        verify_json_value(owner_public_key, &self.signature, &self.unsigned_value())
    }

    pub fn hash(&self) -> Result<String> {
        Ok(format!(
            "blake3:{}",
            blake3::hash(&canonical_json(self)?).to_hex()
        ))
    }

    pub fn unsigned_value(&self) -> JsonValue {
        json!({
            "type": "rymnet.search.record",
            "version": 1,
            "domain": self.domain,
            "path": self.path,
            "title": self.title,
            "description": self.description,
            "headings": self.headings,
            "keywords": self.keywords,
            "content_hash": self.content_hash,
            "manifest_hash": self.manifest_hash,
            "published_at": self.published_at,
        })
    }
}

pub fn extract_html_document(domain: &str, path: &str, html: &str) -> SearchDocument {
    let document = Html::parse_document(html);
    let title = select_text(&document, "title")
        .into_iter()
        .next()
        .unwrap_or_else(|| domain.to_string());
    let description = select_attr(&document, r#"meta[name="description"]"#, "content")
        .into_iter()
        .next()
        .unwrap_or_default();
    let headings = select_text(&document, "h1, h2, h3");
    let body = select_text(&document, "body").join(" ");

    SearchDocument {
        domain: domain.to_string(),
        path: path.to_string(),
        title,
        description,
        headings,
        body,
    }
}

pub fn rebuild_index(
    index_root: impl AsRef<Path>,
    sites_root: impl AsRef<Path>,
    chunks_root: impl AsRef<Path>,
) -> Result<usize> {
    let index_root = index_root.as_ref();
    if index_root.exists() {
        fs::remove_dir_all(index_root)
            .with_context(|| format!("clear {}", index_root.display()))?;
    }
    fs::create_dir_all(index_root).with_context(|| format!("create {}", index_root.display()))?;
    let manifests = list_latest_manifests(sites_root)?;
    let mut count = index_manifests(index_root, chunks_root, &manifests)?;
    count += index_records(
        index_root,
        &list_search_records(default_records_root(
            index_root.parent().unwrap_or(index_root),
        ))?,
    )?;
    Ok(count)
}

pub fn index_site(
    index_root: impl AsRef<Path>,
    chunks_root: impl AsRef<Path>,
    manifest: &SiteManifest,
) -> Result<usize> {
    fs::create_dir_all(index_root.as_ref())
        .with_context(|| format!("create {}", index_root.as_ref().display()))?;
    index_manifests(index_root, chunks_root, std::slice::from_ref(manifest))
}

pub fn index_site_by_domain(
    index_root: impl AsRef<Path>,
    sites_root: impl AsRef<Path>,
    chunks_root: impl AsRef<Path>,
    domain: &str,
) -> Result<usize> {
    let manifest = load_latest_manifest(domain, sites_root)?
        .with_context(|| format!("no latest manifest for {domain}"))?;
    index_site(index_root, chunks_root, &manifest)
}

pub fn clear_index(index_root: impl AsRef<Path>) -> Result<()> {
    let index_root = index_root.as_ref();
    if index_root.exists() {
        fs::remove_dir_all(index_root)
            .with_context(|| format!("clear {}", index_root.display()))?;
    }
    fs::create_dir_all(index_root).with_context(|| format!("create {}", index_root.display()))
}

pub fn index_status(index_root: impl AsRef<Path>) -> Result<IndexStatus> {
    let index_root = index_root.as_ref();
    if !index_root.join("meta.json").exists() {
        return Ok(IndexStatus::default());
    }
    let (index, _) = open_index(index_root)?;
    let reader = index.reader()?;
    Ok(IndexStatus {
        documents: reader.searcher().num_docs() as usize,
    })
}

pub fn search(
    index_root: impl AsRef<Path>,
    query: &str,
    domain_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let index_root = index_root.as_ref();
    if !index_root.join("meta.json").exists() {
        return Ok(Vec::new());
    }
    let (index, fields) = open_index(index_root)?;
    let reader = index.reader()?;
    let searcher = reader.searcher();
    let parser = QueryParser::for_index(
        &index,
        vec![
            fields.domain,
            fields.title,
            fields.description,
            fields.headings,
            fields.keywords,
            fields.body,
        ],
    );
    let query = parser.parse_query(query).context("parse search query")?;
    let top_docs = searcher.search(&query, &TopDocs::with_limit(limit * 4).and_offset(0))?;
    let mut results = Vec::new();
    for (_score, address) in top_docs {
        let doc: TantivyDocument = searcher.doc(address)?;
        let domain = first_text(&doc, fields.domain).unwrap_or_default();
        if let Some(filter) = domain_filter {
            if domain != filter {
                continue;
            }
        }
        let path = first_text(&doc, fields.path).unwrap_or_else(|| "/".to_string());
        let title = first_text(&doc, fields.title).unwrap_or_else(|| domain.clone());
        let description = first_text(&doc, fields.description).unwrap_or_default();
        let path_suffix = if path == "/index.html" {
            "/"
        } else {
            path.as_str()
        };
        results.push(SearchResult {
            url: format!("http://127.0.0.1:7447/{domain}{path_suffix}"),
            domain,
            path,
            title,
            description,
        });
        if results.len() >= limit {
            break;
        }
    }
    Ok(results)
}

pub fn create_search_records_for_manifest(
    records_root: impl AsRef<Path>,
    chunks_root: impl AsRef<Path>,
    manifest: &SiteManifest,
    identity: &NodeIdentity,
) -> Result<Vec<SearchRecord>> {
    let store = ChunkStore::new(chunks_root.as_ref());
    let manifest_hash = manifest.hash()?;
    let mut records = Vec::new();

    for file in &manifest.files {
        if !(file.mime == "text/html" || file.mime == "application/xhtml+xml") {
            continue;
        }
        let bytes = store.get(&file.hash)?;
        if !store.verify(&file.hash, &bytes) {
            anyhow::bail!("chunk hash verification failed for {}", file.path);
        }
        let extracted = extract_html_document(
            &manifest.domain,
            &file.path,
            &String::from_utf8_lossy(&bytes),
        );
        let keywords = keywords_for(&extracted);
        let unsigned = search_record_unsigned(
            &manifest.domain,
            &file.path,
            &extracted.title,
            &extracted.description,
            &extracted.headings,
            &keywords,
            &file.hash,
            &manifest_hash,
            manifest.created_at,
        );
        let signature = sign_json_value(&identity.signing_key()?, &unsigned)?;
        let record = SearchRecord {
            record_type: "rymnet.search.record".to_string(),
            version: 1,
            domain: manifest.domain.clone(),
            path: file.path.clone(),
            title: extracted.title,
            description: extracted.description,
            headings: extracted.headings.clone(),
            keywords,
            content_hash: file.hash.clone(),
            manifest_hash: manifest_hash.clone(),
            published_at: manifest.created_at,
            signature,
        };
        save_search_record(&records_root, &record)?;
        records.push(record);
    }

    Ok(records)
}

pub fn save_verified_search_record(
    records_root: impl AsRef<Path>,
    record: &SearchRecord,
    owner_public_key: &str,
) -> Result<PathBuf> {
    if !record.verify(owner_public_key)? {
        anyhow::bail!("search record signature verification failed");
    }
    save_search_record(records_root, record)
}

pub fn save_search_record(
    records_root: impl AsRef<Path>,
    record: &SearchRecord,
) -> Result<PathBuf> {
    let records_root = records_root.as_ref();
    let domain_dir = records_root.join(&record.domain);
    fs::create_dir_all(&domain_dir).with_context(|| format!("create {}", domain_dir.display()))?;
    let path = domain_dir.join(format!(
        "{}.json",
        record.hash()?.trim_start_matches("blake3:")
    ));
    fs::write(&path, serde_json::to_vec_pretty(record)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn list_search_records(records_root: impl AsRef<Path>) -> Result<Vec<SearchRecord>> {
    let records_root = records_root.as_ref();
    if !records_root.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for domain_dir in
        fs::read_dir(records_root).with_context(|| format!("read {}", records_root.display()))?
    {
        let domain_dir = domain_dir?;
        if !domain_dir.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(domain_dir.path())? {
            let entry = entry?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(entry.path())?;
            records.push(serde_json::from_slice(&bytes)?);
        }
    }
    records.sort_by(|a: &SearchRecord, b: &SearchRecord| {
        (&a.domain, &a.path, &a.content_hash).cmp(&(&b.domain, &b.path, &b.content_hash))
    });
    Ok(records)
}

pub fn index_records(index_root: impl AsRef<Path>, records: &[SearchRecord]) -> Result<usize> {
    if records.is_empty() {
        return Ok(0);
    }
    let (index, fields) = open_or_create_index(index_root)?;
    let mut writer = index.writer(50_000_000)?;
    for record in records {
        writer.add_document(doc!(
            fields.domain => record.domain.clone(),
            fields.path => record.path.clone(),
            fields.title => record.title.clone(),
            fields.description => record.description.clone(),
            fields.headings => record.headings.join(" "),
            fields.keywords => record.keywords.join(" "),
            fields.body => "",
        ))?;
    }
    writer.commit()?;
    Ok(records.len())
}

fn index_manifests(
    index_root: impl AsRef<Path>,
    chunks_root: impl AsRef<Path>,
    manifests: &[SiteManifest],
) -> Result<usize> {
    let (index, fields) = open_or_create_index(index_root)?;
    let store = ChunkStore::new(chunks_root.as_ref());
    let mut writer = index.writer(50_000_000)?;
    let mut count = 0;

    for manifest in manifests {
        for file in &manifest.files {
            if !(file.mime == "text/html" || file.mime == "application/xhtml+xml") {
                continue;
            }
            let bytes = store
                .get(&file.hash)
                .with_context(|| format!("read chunk for {}{}", manifest.domain, file.path))?;
            if !store.verify(&file.hash, &bytes) {
                anyhow::bail!(
                    "chunk hash verification failed for {}{}",
                    manifest.domain,
                    file.path
                );
            }
            let html = String::from_utf8_lossy(&bytes);
            let extracted = extract_html_document(&manifest.domain, &file.path, &html);
            writer.add_document(doc!(
                fields.domain => extracted.domain,
                fields.path => extracted.path,
                fields.title => extracted.title,
                fields.description => extracted.description,
                fields.headings => extracted.headings.join(" "),
                fields.keywords => "",
                fields.body => extracted.body,
            ))?;
            count += 1;
        }
    }

    writer.commit()?;
    Ok(count)
}

fn open_or_create_index(index_root: impl AsRef<Path>) -> Result<(Index, SearchFields)> {
    let index_root = index_root.as_ref();
    fs::create_dir_all(index_root).with_context(|| format!("create {}", index_root.display()))?;
    let schema = schema();
    let fields = fields(&schema)?;
    let index = if index_root.join("meta.json").exists() {
        Index::open_in_dir(index_root)?
    } else {
        Index::create_in_dir(index_root, schema)?
    };
    Ok((index, fields))
}

fn open_index(index_root: impl AsRef<Path>) -> Result<(Index, SearchFields)> {
    let index = Index::open_in_dir(index_root)?;
    let schema = index.schema();
    let fields = fields(&schema)?;
    Ok((index, fields))
}

fn schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_text_field("domain", STRING | STORED);
    builder.add_text_field("path", STRING | STORED);
    builder.add_text_field("title", TEXT | STORED);
    builder.add_text_field("description", TEXT | STORED);
    builder.add_text_field("headings", TEXT | STORED);
    builder.add_text_field("keywords", TEXT | STORED);
    builder.add_text_field("body", TEXT);
    builder.build()
}

fn fields(schema: &Schema) -> Result<SearchFields> {
    Ok(SearchFields {
        domain: schema.get_field("domain")?,
        path: schema.get_field("path")?,
        title: schema.get_field("title")?,
        description: schema.get_field("description")?,
        headings: schema.get_field("headings")?,
        keywords: schema.get_field("keywords")?,
        body: schema.get_field("body")?,
    })
}

fn first_text(doc: &TantivyDocument, field: Field) -> Option<String> {
    doc.get_first(field)
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn select_text(document: &Html, selector: &str) -> Vec<String> {
    let selector = Selector::parse(selector).expect("static selector");
    document
        .select(&selector)
        .map(|node| node.text().collect::<Vec<_>>().join(" "))
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|text| !text.is_empty())
        .collect()
}

fn select_attr(document: &Html, selector: &str, attr: &str) -> Vec<String> {
    let selector = Selector::parse(selector).expect("static selector");
    document
        .select(&selector)
        .filter_map(|node| node.value().attr(attr))
        .map(ToString::to_string)
        .collect()
}

pub fn default_index_root(search_root: impl AsRef<Path>) -> PathBuf {
    search_root.as_ref().join("tantivy-index")
}

pub fn default_records_root(search_root: impl AsRef<Path>) -> PathBuf {
    search_root.as_ref().join("records")
}

fn search_record_unsigned(
    domain: &str,
    path: &str,
    title: &str,
    description: &str,
    headings: &[String],
    keywords: &[String],
    content_hash: &str,
    manifest_hash: &str,
    published_at: DateTime<Utc>,
) -> JsonValue {
    json!({
        "type": "rymnet.search.record",
        "version": 1,
        "domain": domain,
        "path": path,
        "title": title,
        "description": description,
        "headings": headings,
        "keywords": keywords,
        "content_hash": content_hash,
        "manifest_hash": manifest_hash,
        "published_at": published_at,
    })
}

fn keywords_for(document: &SearchDocument) -> Vec<String> {
    let text = format!(
        "{} {} {} {} {}",
        document.domain,
        document.path,
        document.title,
        document.description,
        document.headings.join(" ")
    );
    let mut words = text
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .map(str::to_ascii_lowercase)
        .filter(|word| word.len() >= 3)
        .collect::<Vec<_>>();
    words.sort();
    words.dedup();
    words.truncate(32);
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use rymnet_core::NodeIdentity;
    use rymnet_site::{create_site, publish_site};

    #[test]
    fn extracts_search_text() {
        let doc = extract_html_document(
            "rob.rym",
            "/index.html",
            r#"<html><head><title>Hello</title><meta name="description" content="A page"></head><body><h1>Welcome</h1><p>Encrypted messaging notes.</p></body></html>"#,
        );
        assert_eq!(doc.title, "Hello");
        assert_eq!(doc.description, "A page");
        assert_eq!(doc.headings, vec!["Welcome"]);
        assert!(doc.body.contains("Encrypted messaging notes."));
    }

    #[test]
    fn indexes_and_searches_site() {
        let temp =
            std::env::temp_dir().join(format!("rymnet-search-test-{}", blake3::hash(b"search")));
        let _ = fs::remove_dir_all(&temp);
        let sites = temp.join("sites");
        let chunks = temp.join("chunks");
        let index = temp.join("search").join("tantivy-index");
        let identity = NodeIdentity::generate();

        create_site("rob.rym", &sites).expect("create");
        let published = publish_site("rob.rym", &sites, &chunks, &identity).expect("publish");
        index_site(&index, &chunks, &published.manifest).expect("index");
        let results = search(&index, "welcome", None, 10).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].domain, "rob.rym");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn creates_signed_search_records() {
        let temp = std::env::temp_dir().join(format!(
            "rymnet-search-record-test-{}",
            blake3::hash(b"record")
        ));
        let _ = fs::remove_dir_all(&temp);
        let sites = temp.join("sites");
        let chunks = temp.join("chunks");
        let records = temp.join("search").join("records");
        let identity = NodeIdentity::generate();

        create_site("rob.rym", &sites).expect("create");
        let published = publish_site("rob.rym", &sites, &chunks, &identity).expect("publish");
        let records =
            create_search_records_for_manifest(&records, &chunks, &published.manifest, &identity)
                .expect("records");

        assert_eq!(records.len(), 1);
        assert!(records[0].verify(&identity.public_key).expect("verify"));
        assert_eq!(records[0].domain, "rob.rym");
        assert_eq!(records[0].manifest_hash, published.manifest_hash);

        let _ = fs::remove_dir_all(&temp);
    }
}

//! Turns uploaded bytes into searchable text sections, entirely in memory.

pub mod book;
mod chunk;
mod formats;
mod xml;

pub use chunk::{chunk_document, ChunkOut};
pub use formats::thumbnail;
pub use formats::vision_image;

use rad_api::Anchor;

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("could not read {kind}: {msg}")]
    Parse { kind: &'static str, msg: String },
}

pub type Result<T> = std::result::Result<T, IngestError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Docx,
    Pptx,
    Spreadsheet,
    Odt,
    Odp,
    Csv,
    Html,
    Epub,
    Email,
    Markdown,
    Text,
    Image,
    Audio,
    Video,
    Zip,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    None,
    Image { width: u32, height: u32 },
    Audio,
    Video,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub text: String,
    pub anchor: Anchor,
    /// text | table | meta | caption | transcript
    pub kind: &'static str,
    /// Text is Markdown (headings drive chunk anchors).
    pub markdown: bool,
}

impl Section {
    pub fn text(text: impl Into<String>, anchor: Anchor) -> Self {
        Section { text: text.into(), anchor, kind: "text", markdown: false }
    }
}

#[derive(Debug, Clone)]
pub struct Extracted {
    pub kind: Kind,
    pub title: Option<String>,
    pub sections: Vec<Section>,
    pub page_count: Option<i64>,
    pub media: Media,
    /// Shown to the user, e.g. "scanned PDF without text layer".
    pub notice: Option<String>,
}

const TEXT_EXT: &[&str] = &[
    "txt", "text", "log", "json", "jsonl", "ndjson", "yaml", "yml", "toml", "ini", "cfg", "conf", "env", "xml",
    "svg", "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "go", "java", "kt", "kts", "swift", "c", "h",
    "cc", "cpp", "hpp", "cs", "rb", "php", "pl", "lua", "sh", "bash", "zsh", "fish", "ps1", "bat", "sql",
    "r", "scala", "dart", "vue", "svelte", "css", "scss", "less", "tex", "bib", "rst", "adoc", "org", "srt",
    "vtt", "ics", "vcf", "tsv", "gradle", "dockerfile", "makefile", "proto", "graphql", "tf", "nix", "zig",
    "ex", "exs", "erl", "hs", "ml", "clj", "lisp", "el", "vim", "rtf",
];

pub fn extension(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.rsplit_once('.') {
        Some((_, ext)) if ext.len() <= 10 => ext.to_string(),
        _ => match lower.as_str() {
            "dockerfile" | "makefile" | "readme" | "license" => "txt".into(),
            _ => String::new(),
        },
    }
}

/// Best-effort MIME type from the bytes and the file name.
pub fn detect_mime(bytes: &[u8], filename: &str) -> String {
    let ext = extension(filename);
    // Office formats are zips; trust the extension for those.
    let by_ext = mime_guess::from_ext(&ext).first().map(|m| m.essence_str().to_string());
    if matches!(ext.as_str(), "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" | "xlsm" | "md" | "markdown" | "csv" | "eml") {
        if let Some(m) = by_ext.clone() {
            return m;
        }
        if ext == "md" || ext == "markdown" {
            return "text/markdown".into();
        }
    }
    if let Some(t) = infer::get(bytes) {
        return t.mime_type().to_string();
    }
    if let Some(m) = by_ext {
        return m;
    }
    if looks_like_text(bytes) {
        return "text/plain".into();
    }
    "application/octet-stream".into()
}

fn looks_like_text(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192)];
    !sample.contains(&0) && (std::str::from_utf8(sample).is_ok() || sample.iter().filter(|b| **b < 0x09).count() == 0)
}

pub fn classify(mime: &str, filename: &str) -> Kind {
    let ext = extension(filename);
    match ext.as_str() {
        "pdf" => return Kind::Pdf,
        "docx" | "docm" | "dotx" => return Kind::Docx,
        "pptx" | "pptm" => return Kind::Pptx,
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => return Kind::Spreadsheet,
        "odt" => return Kind::Odt,
        "odp" => return Kind::Odp,
        "csv" | "tsv" => return Kind::Csv,
        "html" | "htm" | "xhtml" | "mhtml" => return Kind::Html,
        // Kindle and FictionBook files are read by book.rs, like EPUB.
        "epub" | "mobi" | "azw" | "azw3" | "prc" | "kf8" | "fb2" => return Kind::Epub,
        "eml" => return Kind::Email,
        "md" | "markdown" | "mdx" => return Kind::Markdown,
        "zip" => return Kind::Zip,
        _ => {}
    }
    if mime.starts_with("image/") && !mime.contains("svg") {
        return Kind::Image;
    }
    if mime.starts_with("audio/") || matches!(ext.as_str(), "mp3" | "m4a" | "wav" | "flac" | "ogg" | "opus" | "aac" | "wma" | "aiff") {
        return Kind::Audio;
    }
    if mime.starts_with("video/") || matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "wmv" | "mpeg" | "mpg") {
        return Kind::Video;
    }
    match mime {
        "application/pdf" => Kind::Pdf,
        "text/markdown" => Kind::Markdown,
        "text/html" | "application/xhtml+xml" => Kind::Html,
        "text/csv" => Kind::Csv,
        "message/rfc822" => Kind::Email,
        "application/zip" => Kind::Zip,
        "application/epub+zip" => Kind::Epub,
        m if m.starts_with("text/") || m.contains("json") || m.contains("xml") || m.contains("yaml") || m.contains("javascript") => Kind::Text,
        _ if TEXT_EXT.contains(&ext.as_str()) => Kind::Text,
        _ => Kind::Binary,
    }
}

/// Extracts text sections. Never panics: parser panics are caught and reported as errors.
pub fn extract(bytes: &[u8], filename: &str, mime: &str) -> Result<Extracted> {
    let kind = classify(mime, filename);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| formats::extract_kind(kind, bytes, filename, 0)));
    match out {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "parser crashed".into());
            Err(IngestError::Parse { kind: kind_name(kind), msg })
        }
    }
}

pub fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Pdf => "PDF",
        Kind::Docx => "Word document",
        Kind::Pptx => "PowerPoint",
        Kind::Spreadsheet => "spreadsheet",
        Kind::Odt => "OpenDocument text",
        Kind::Odp => "OpenDocument presentation",
        Kind::Csv => "CSV",
        Kind::Html => "HTML",
        Kind::Epub => "EPUB",
        Kind::Email => "email",
        Kind::Markdown => "Markdown",
        Kind::Text => "text",
        Kind::Image => "image",
        Kind::Audio => "audio",
        Kind::Video => "video",
        Kind::Zip => "zip archive",
        Kind::Binary => "file",
    }
}

pub fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    if bytes.len() >= 2 && (bytes[..2] == [0xFF, 0xFE] || bytes[..2] == [0xFE, 0xFF]) {
        let enc = if bytes[0] == 0xFF { encoding_rs::UTF_16LE } else { encoding_rs::UTF_16BE };
        return enc.decode(&bytes[2..]).0.into_owned();
    }
    let mut det = chardetng::EncodingDetector::new();
    det.feed(bytes, true);
    let enc = det.guess(None, true);
    enc.decode(bytes).0.into_owned()
}

#[cfg(test)]
mod tests;

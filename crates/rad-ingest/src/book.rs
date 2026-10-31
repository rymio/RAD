//! E-books opened for reading: EPUB, MOBI / AZW / AZW3 / PRC (Kindle, DRM-free only), FB2, plain
//! text and HTML. Each becomes its chapters as (unsanitised) HTML, the pictures they show, and a
//! table of contents. The reader sanitises the HTML; links and pictures are by path:
//!
//! - EPUB: zip paths ("OEBPS/ch1.xhtml", "OEBPS/img/a.jpg"); links are relative to the chapter.
//! - MOBI: pictures are `mobi:N` (the old `recindex="N"` and KF8's `kindle:embed:XXXX` both become
//!   `src="mobi:N"`), and old-style `filepos` links become `#fpN` anchors.
//! - FB2: pictures are `fb2:ID`.
//!
//! The text of a book is also what the vault indexes (see formats.rs), so MOBI and FB2 are
//! searchable like EPUB.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use base64::Engine;

use crate::xml::{self, Ev};
use crate::{decode_text, IngestError, Result};

/// Pictures larger than this are left out (a picture is sent to the reader as a data URL).
const MAX_IMAGE: usize = 12 * 1024 * 1024;
const MAX_IMAGES_TOTAL: usize = 160 * 1024 * 1024;
const MAX_ENTRY: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct Book {
    pub title: Option<String>,
    pub author: Option<String>,
    pub language: Option<String>,
    pub chapters: Vec<Chapter>,
    pub toc: Vec<TocEntry>,
    /// Pictures by the path the chapters use.
    pub resources: HashMap<String, Resource>,
    /// The cover's key in `resources`.
    pub cover: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Chapter {
    /// Where the chapter is in the book (EPUB: its zip path); links point here.
    pub path: String,
    pub html: String,
}

#[derive(Debug, Clone)]
pub struct TocEntry {
    pub title: String,
    /// A chapter path, maybe with "#fragment".
    pub href: String,
    pub depth: u32,
}

#[derive(Debug, Clone)]
pub struct Resource {
    pub mime: String,
    pub data: Vec<u8>,
}

fn err(kind: &'static str, msg: impl std::fmt::Display) -> IngestError {
    IngestError::Parse { kind, msg: msg.to_string() }
}

/// The e-book formats RAD reads (PDF is shown by the reader itself).
pub fn is_book(ext: &str) -> bool {
    matches!(ext, "epub" | "mobi" | "azw" | "azw3" | "prc" | "kf8" | "fb2" | "txt" | "html" | "htm" | "xhtml")
}

/// Opens a book; the format comes from the file name's extension (and the bytes, for Kindle files).
pub fn open(bytes: &[u8], filename: &str) -> Result<Book> {
    let ext = crate::extension(filename);
    let mut b = match ext.as_str() {
        "epub" => epub(bytes)?,
        "mobi" | "azw" | "azw3" | "prc" | "kf8" => mobi(bytes)?,
        "fb2" => fb2(bytes)?,
        "html" | "htm" | "xhtml" => {
            let src = decode_text(bytes);
            Book { title: tag_text(&src, "title"), chapters: vec![Chapter { path: "index.html".into(), html: src }], ..Default::default() }
        }
        _ if bytes.starts_with(b"PK\x03\x04") => epub(bytes)?,
        _ if bytes.len() > 68 && &bytes[60..68] == b"BOOKMOBI" => mobi(bytes)?,
        _ => text(&decode_text(bytes)),
    };
    b.title = b.title.map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty());
    b.author = b.author.map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty());
    if b.chapters.is_empty() {
        return Err(err("e-book", "the book has no text"));
    }
    Ok(b)
}

/// The text inside the first `<name>…</name>`.
fn tag_text(src: &str, name: &str) -> Option<String> {
    let lower = src.to_lowercase();
    let s = lower.find(&format!("<{name}"))?;
    let s = s + lower[s..].find('>')? + 1;
    let e = s + lower[s..].find(&format!("</{name}"))?;
    let t = xml::unescape(src[s..e].trim());
    (!t.is_empty()).then_some(t)
}

pub(crate) fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn image_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if data.starts_with(b"GIF8") {
        Some("image/gif")
    } else if data.starts_with(b"BM") {
        Some("image/bmp")
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if data.starts_with(b"<svg") || data.starts_with(b"<?xml") {
        Some("image/svg+xml")
    } else {
        None
    }
}

// ------------------------------------------------------------------ EPUB

/// `href` (percent-encoded, maybe "../x") relative to the folder `dir` ("OEBPS/text/").
pub fn resolve(dir: &str, href: &str) -> String {
    let href = percent_decode(href);
    let mut parts: Vec<&str> = if href.starts_with('/') { Vec::new() } else { dir.split('/').filter(|p| !p.is_empty()).collect() };
    for p in href.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn dir_of(path: &str) -> String {
    path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default()
}

fn zip_bytes(z: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut f = z.by_name(name).ok()?;
    if f.size() > MAX_ENTRY {
        return None;
    }
    let mut s = Vec::new();
    f.by_ref().take(MAX_ENTRY).read_to_end(&mut s).ok()?;
    Some(s)
}

fn epub(bytes: &[u8]) -> Result<Book> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| err("EPUB", e))?;
    if z.by_name("META-INF/encryption.xml").is_ok_and(|mut f| {
        let mut s = String::new();
        let _ = f.read_to_string(&mut s);
        // Font obfuscation is fine; anything else is DRM.
        s.contains("EncryptedData") && !s.contains("http://www.idpf.org/2008/embedding") && !s.contains("http://ns.adobe.com/pdf/enc#RC")
    }) {
        return Err(err("EPUB", "this book is DRM-protected; RAD can only open DRM-free books"));
    }
    let container = zip_bytes(&mut z, "META-INF/container.xml").map(|b| decode_text(&b)).unwrap_or_default();
    let opf_path = xml::events(&container)
        .into_iter()
        .find_map(|ev| match ev {
            Ev::Empty("rootfile", a) | Ev::Open("rootfile", a) => xml::attr(a, "full-path"),
            _ => None,
        })
        .unwrap_or_else(|| "OEBPS/content.opf".into());
    let opf = zip_bytes(&mut z, &opf_path).map(|b| decode_text(&b)).ok_or_else(|| err("EPUB", "the book's package file is missing"))?;
    let dir = dir_of(&opf_path);

    struct Item {
        href: String,
        mime: String,
        props: String,
    }
    let mut manifest: HashMap<String, Item> = HashMap::new();
    let mut spine = Vec::new();
    let mut toc_id = None;
    let mut cover_id = None;
    let mut b = Book::default();
    let mut field: Option<&str> = None;
    for ev in xml::events(&opf) {
        match ev {
            Ev::Empty("item", a) | Ev::Open("item", a) => {
                if let (Some(id), Some(href)) = (xml::attr(a, "id"), xml::attr(a, "href")) {
                    manifest.insert(
                        id,
                        Item { href: resolve(&dir, &href), mime: xml::attr(a, "media-type").unwrap_or_default(), props: xml::attr(a, "properties").unwrap_or_default() },
                    );
                }
            }
            Ev::Empty("itemref", a) | Ev::Open("itemref", a) => {
                if let Some(id) = xml::attr(a, "idref") {
                    spine.push(id);
                }
            }
            Ev::Open("spine", a) => toc_id = xml::attr(a, "toc"),
            Ev::Empty("meta", a) | Ev::Open("meta", a) if xml::attr(a, "name").as_deref() == Some("cover") => cover_id = xml::attr(a, "content"),
            Ev::Open(n, _) if n.ends_with("title") && n.starts_with("dc:") => field = Some("title"),
            Ev::Open(n, _) if n.ends_with("creator") && n.starts_with("dc:") => field = Some("author"),
            Ev::Open(n, _) if n.ends_with("language") && n.starts_with("dc:") => field = Some("language"),
            Ev::Text(t) => {
                let slot = match field {
                    Some("title") => &mut b.title,
                    Some("author") => &mut b.author,
                    Some("language") => &mut b.language,
                    _ => continue,
                };
                if slot.is_none() && !t.trim().is_empty() {
                    *slot = Some(t.trim().to_string());
                }
            }
            Ev::Close(_) => field = None,
            _ => {}
        }
    }

    for id in &spine {
        let Some(item) = manifest.get(id) else { continue };
        if !(item.mime.contains("html") || item.href.ends_with("html") || item.href.ends_with(".htm")) {
            continue;
        }
        let Some(src) = zip_bytes(&mut z, &item.href) else { continue };
        b.chapters.push(Chapter { path: item.href.clone(), html: decode_text(&src) });
    }

    // Pictures: everything the manifest lists as an image.
    let mut total = 0;
    for item in manifest.values() {
        if !item.mime.starts_with("image/") || total > MAX_IMAGES_TOTAL {
            continue;
        }
        if let Some(data) = zip_bytes(&mut z, &item.href).filter(|d| d.len() <= MAX_IMAGE) {
            total += data.len();
            b.resources.insert(item.href.clone(), Resource { mime: item.mime.clone(), data });
        }
    }
    b.cover = manifest
        .values()
        .find(|i| i.props.split_whitespace().any(|p| p == "cover-image"))
        .or_else(|| cover_id.as_ref().and_then(|id| manifest.get(id)))
        .map(|i| i.href.clone())
        .filter(|h| b.resources.contains_key(h));

    // Table of contents: the EPUB 3 navigation document, else the NCX.
    if let Some(nav) = manifest.values().find(|i| i.props.split_whitespace().any(|p| p == "nav")) {
        if let Some(src) = zip_bytes(&mut z, &nav.href) {
            b.toc = nav_toc(&decode_text(&src), &dir_of(&nav.href));
        }
    }
    if b.toc.is_empty() {
        if let Some(ncx) = toc_id.as_ref().and_then(|id| manifest.get(id)).or_else(|| manifest.values().find(|i| i.mime == "application/x-dtbncx+xml")) {
            if let Some(src) = zip_bytes(&mut z, &ncx.href) {
                b.toc = ncx_toc(&decode_text(&src), &dir_of(&ncx.href));
            }
        }
    }
    Ok(b)
}

fn nav_toc(src: &str, dir: &str) -> Vec<TocEntry> {
    let mut out = Vec::new();
    let mut in_toc = false;
    let mut nav_depth = 0;
    let mut ol = 0u32;
    let mut cur: Option<(String, String)> = None;
    for ev in xml::events(src) {
        match ev {
            Ev::Open("nav", a) => {
                nav_depth += 1;
                if !in_toc && xml::attr(a, "epub:type").is_some_and(|t| t.split_whitespace().any(|t| t == "toc")) {
                    in_toc = true;
                    nav_depth = 1;
                }
            }
            Ev::Close("nav") => {
                nav_depth -= 1;
                if in_toc && nav_depth == 0 {
                    break;
                }
            }
            Ev::Open("ol", _) if in_toc => ol += 1,
            Ev::Close("ol") if in_toc => ol = ol.saturating_sub(1),
            Ev::Open("a", a) if in_toc => cur = xml::attr(a, "href").map(|h| (resolve_link(dir, &h), String::new())),
            Ev::Text(t) => {
                if let Some((_, title)) = cur.as_mut() {
                    title.push_str(&t);
                }
            }
            Ev::Close("a") => {
                if let Some((href, title)) = cur.take() {
                    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !title.is_empty() {
                        out.push(TocEntry { title, href, depth: ol.saturating_sub(1) });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn ncx_toc(src: &str, dir: &str) -> Vec<TocEntry> {
    let mut out: Vec<TocEntry> = Vec::new();
    let mut depth = 0u32;
    let mut in_label = false;
    let mut title = String::new();
    for ev in xml::events(src) {
        match ev {
            Ev::Open("navPoint", _) => {
                depth += 1;
                title.clear();
            }
            Ev::Close("navPoint") => depth = depth.saturating_sub(1),
            Ev::Open("text", _) => in_label = true,
            Ev::Close("text") => in_label = false,
            Ev::Text(t) if in_label => title.push_str(&t),
            Ev::Empty("content", a) | Ev::Open("content", a) if depth > 0 => {
                if let Some(h) = xml::attr(a, "src") {
                    let t = title.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !t.is_empty() {
                        out.push(TocEntry { title: t, href: resolve_link(dir, &h), depth: depth - 1 });
                    }
                    title.clear();
                }
            }
            _ => {}
        }
    }
    out
}

/// A link with its fragment kept: "../text/ch2.xhtml#s1" → "OEBPS/text/ch2.xhtml#s1".
fn resolve_link(dir: &str, href: &str) -> String {
    match href.split_once('#') {
        Some((p, f)) if p.is_empty() => format!("#{f}"),
        Some((p, f)) => format!("{}#{f}", resolve(dir, p)),
        None => resolve(dir, href),
    }
}

// ------------------------------------------------------------------ MOBI / AZW / AZW3

fn be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

struct Palm<'a> {
    bytes: &'a [u8],
    offsets: Vec<usize>,
}

impl<'a> Palm<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < 78 || &bytes[60..68] != b"BOOKMOBI" && &bytes[60..68] != b"TEXtREAd" {
            if bytes.len() >= 68 && &bytes[60..64] == b"TPZ0" || bytes.starts_with(b"TPZ") {
                return Err(err("MOBI", "this is a Topaz Kindle book, which RAD can't read"));
            }
            return Err(err("MOBI", "not a Mobipocket / Kindle file"));
        }
        let n = be16(bytes, 76).unwrap_or(0) as usize;
        let mut offsets = Vec::with_capacity(n);
        for i in 0..n {
            let o = be32(bytes, 78 + i * 8).ok_or_else(|| err("MOBI", "truncated record list"))? as usize;
            offsets.push(o.min(bytes.len()));
        }
        Ok(Palm { bytes, offsets })
    }
    fn len(&self) -> usize {
        self.offsets.len()
    }
    fn record(&self, i: usize) -> &'a [u8] {
        let Some(&s) = self.offsets.get(i) else { return &[] };
        let e = self.offsets.get(i + 1).copied().unwrap_or(self.bytes.len()).max(s);
        &self.bytes[s..e]
    }
}

/// One MOBI header (a combined MOBI 6 + KF8 file has two).
struct Header {
    /// Index of its record 0.
    base: usize,
    compression: u16,
    text_records: usize,
    encryption: u16,
    utf8: bool,
    version: u32,
    first_image: usize,
    huff: (usize, usize),
    extra_flags: u16,
    exth: HashMap<u32, Vec<Vec<u8>>>,
    full_name: Option<String>,
}

fn header(p: &Palm, base: usize) -> Result<Header> {
    let r0 = p.record(base);
    let compression = be16(r0, 0).ok_or_else(|| err("MOBI", "empty header"))?;
    let text_records = be16(r0, 8).unwrap_or(0) as usize;
    let encryption = be16(r0, 12).unwrap_or(0);
    let mut h = Header {
        base,
        compression,
        text_records,
        encryption,
        utf8: false,
        version: 0,
        first_image: usize::MAX,
        huff: (0, 0),
        extra_flags: 0,
        exth: HashMap::new(),
        full_name: None,
    };
    if r0.get(16..20) != Some(b"MOBI") {
        // Plain PalmDOC (TEXtREAd): no MOBI header.
        return Ok(h);
    }
    let hlen = be32(r0, 20).unwrap_or(0) as usize;
    h.utf8 = be32(r0, 28) == Some(65001);
    h.version = be32(r0, 36).unwrap_or(0);
    h.first_image = be32(r0, 108).map(|v| v as usize).filter(|&v| v != 0xFFFF_FFFF).map(|v| v + base).unwrap_or(usize::MAX);
    h.huff = (be32(r0, 112).unwrap_or(0) as usize + base, be32(r0, 116).unwrap_or(0) as usize);
    if hlen >= 0xE4 {
        h.extra_flags = be16(r0, 0xF2).unwrap_or(0);
    }
    let (no, nl) = (be32(r0, 84).unwrap_or(0) as usize, be32(r0, 88).unwrap_or(0) as usize);
    if let Some(n) = r0.get(no..no + nl) {
        h.full_name = Some(decode(n, h.utf8));
    }
    if be32(r0, 128).unwrap_or(0) & 0x40 != 0 {
        let at = 16 + hlen;
        if r0.get(at..at + 4) == Some(b"EXTH") {
            let count = be32(r0, at + 8).unwrap_or(0) as usize;
            let mut o = at + 12;
            for _ in 0..count {
                let (Some(t), Some(l)) = (be32(r0, o), be32(r0, o + 4)) else { break };
                let l = l as usize;
                if l < 8 {
                    break;
                }
                if let Some(d) = r0.get(o + 8..o + l) {
                    h.exth.entry(t).or_default().push(d.to_vec());
                }
                o += l;
            }
        }
    }
    Ok(h)
}

fn decode(b: &[u8], utf8: bool) -> String {
    if utf8 {
        String::from_utf8_lossy(b).into_owned()
    } else {
        encoding_rs::WINDOWS_1252.decode(b).0.into_owned()
    }
}

/// How many bytes of trailing entries end a text record.
fn trailing(data: &[u8], flags: u16) -> usize {
    let mut num = 0usize;
    let size = data.len();
    let mut f = flags >> 1;
    while f != 0 {
        if f & 1 != 0 {
            // A size written backwards, 7 bits a byte; the high bit marks its first byte.
            let mut psize = size.saturating_sub(num);
            let (mut bitpos, mut result) = (0u32, 0usize);
            while psize > 0 {
                let v = data[psize - 1];
                result |= ((v & 0x7F) as usize) << bitpos;
                bitpos += 7;
                psize -= 1;
                if v & 0x80 != 0 || bitpos >= 28 {
                    break;
                }
            }
            num += result;
        }
        f >>= 1;
    }
    if flags & 1 != 0 && size > num {
        num += (data[size - num - 1] & 0x3) as usize + 1;
    }
    num.min(size)
}

fn palmdoc(data: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        i += 1;
        match c {
            1..=8 => {
                let end = (i + c as usize).min(data.len());
                out.extend_from_slice(&data[i..end]);
                i = end;
            }
            0x80..=0xBF => {
                let Some(&n) = data.get(i) else { break };
                i += 1;
                let x = (((c as usize) << 8) | n as usize) & 0x3FFF;
                let (dist, len) = (x >> 3, (x & 7) + 3);
                if dist == 0 || dist > out.len() {
                    continue;
                }
                let from = out.len() - dist;
                for k in 0..len {
                    out.push(out[from + k]);
                }
            }
            0xC0..=0xFF => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
            _ => out.push(c),
        }
    }
}

/// Kindle's Huffman + dictionary compression (HUFF/CDIC records).
struct Huff {
    dict1: Vec<(u32, bool, u64)>,
    mincode: Vec<u64>,
    maxcode: Vec<u64>,
    dictionary: Vec<Option<(Vec<u8>, bool)>>,
}

impl Huff {
    fn new(p: &Palm, first: usize, count: usize) -> Result<Huff> {
        let huff = p.record(first);
        if huff.get(0..8) != Some(b"HUFF\x00\x00\x00\x18") {
            return Err(err("MOBI", "bad HUFF record"));
        }
        let (off1, off2) = (be32(huff, 8).unwrap_or(0) as usize, be32(huff, 12).unwrap_or(0) as usize);
        let mut dict1 = Vec::with_capacity(256);
        for i in 0..256 {
            let v = be32(huff, off1 + i * 4).ok_or_else(|| err("MOBI", "bad HUFF table"))?;
            let codelen = v & 0x1F;
            let term = v & 0x80 != 0;
            let maxcode = (((v >> 8) as u64 + 1) << (32 - codelen.min(32))) - 1;
            dict1.push((codelen, term, maxcode));
        }
        let (mut mincode, mut maxcode) = (vec![0u64], vec![0u64]);
        for codelen in 1..=32usize {
            let lo = be32(huff, off2 + (codelen - 1) * 8).ok_or_else(|| err("MOBI", "bad HUFF table"))? as u64;
            let hi = be32(huff, off2 + (codelen - 1) * 8 + 4).ok_or_else(|| err("MOBI", "bad HUFF table"))? as u64;
            mincode.push(lo << (32 - codelen));
            maxcode.push(((hi + 1) << (32 - codelen)) - 1);
        }
        maxcode[0] = (1u64 << 32) - 1;
        let mut dictionary = Vec::new();
        for r in first + 1..first + count.max(1) {
            let cdic = p.record(r);
            if cdic.get(0..8) != Some(b"CDIC\x00\x00\x00\x10") {
                continue;
            }
            let phrases = be32(cdic, 8).unwrap_or(0) as usize;
            let bits = be32(cdic, 12).unwrap_or(0).min(31);
            let n = (1usize << bits).min(phrases.saturating_sub(dictionary.len()));
            for i in 0..n {
                let off = be16(cdic, 16 + i * 2).unwrap_or(0) as usize;
                let blen = be16(cdic, 16 + off).unwrap_or(0) as usize;
                let s = cdic.get(18 + off..18 + off + (blen & 0x7FFF)).unwrap_or(&[]).to_vec();
                dictionary.push(Some((s, blen & 0x8000 != 0)));
            }
        }
        Ok(Huff { dict1, mincode, maxcode, dictionary })
    }

    fn unpack(&mut self, data: &[u8], out: &mut Vec<u8>, depth: u32) {
        if depth > 32 {
            return;
        }
        let mut d = data.to_vec();
        d.extend_from_slice(&[0; 8]);
        let q = |pos: usize| u64::from_be_bytes(d[pos..pos + 8].try_into().unwrap());
        let mut bitsleft = data.len() as i64 * 8;
        let mut pos = 0usize;
        let mut x = q(pos);
        let mut n: i32 = 32;
        loop {
            if n <= 0 {
                pos += 4;
                if pos + 8 > d.len() {
                    break;
                }
                x = q(pos);
                n += 32;
            }
            let code = (x >> n) & 0xFFFF_FFFF;
            let (mut codelen, term, mut maxcode) = self.dict1[(code >> 24) as usize];
            if !term {
                while (codelen as usize) < 32 && code < self.mincode[codelen as usize] {
                    codelen += 1;
                }
                maxcode = self.maxcode[codelen as usize];
            }
            if codelen == 0 {
                break;
            }
            n -= codelen as i32;
            bitsleft -= codelen as i64;
            if bitsleft < 0 {
                break;
            }
            let r = (maxcode.wrapping_sub(code) >> (32 - codelen)) as usize;
            let Some(Some((slice, done))) = self.dictionary.get(r).cloned() else { break };
            if done {
                out.extend_from_slice(&slice);
            } else {
                // Phrases are themselves compressed; each is unpacked once.
                self.dictionary[r] = None;
                let mut u = Vec::new();
                self.unpack(&slice, &mut u, depth + 1);
                out.extend_from_slice(&u);
                self.dictionary[r] = Some((u, true));
            }
        }
    }
}

fn mobi_text(p: &Palm, h: &Header) -> Result<Vec<u8>> {
    match h.encryption {
        0 => {}
        _ => return Err(err("MOBI", "this book is DRM-protected (locked to a Kindle account); RAD can only open DRM-free books")),
    }
    let mut huff = if h.compression == 17480 { Some(Huff::new(p, h.huff.0, h.huff.1)?) } else { None };
    let mut out = Vec::new();
    for i in 1..=h.text_records {
        let rec = p.record(h.base + i);
        let rec = &rec[..rec.len() - trailing(rec, h.extra_flags)];
        match h.compression {
            1 => out.extend_from_slice(rec),
            2 => palmdoc(rec, &mut out),
            17480 => huff.as_mut().unwrap().unpack(rec, &mut out, 0),
            c => return Err(err("MOBI", format!("unknown compression {c}"))),
        }
    }
    Ok(out)
}

fn exth_str(h: &Header, t: u32) -> Option<String> {
    h.exth.get(&t).and_then(|v| v.first()).map(|d| decode(d, h.utf8)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
fn exth_u32(h: &Header, t: u32) -> Option<u32> {
    h.exth.get(&t).and_then(|v| v.first()).and_then(|d| be32(d, 0))
}

fn mobi(bytes: &[u8]) -> Result<Book> {
    let p = Palm::new(bytes)?;
    let first = header(&p, 0)?;
    // A combined file: the MOBI 6 part comes first and is simpler HTML, so it's read; a KF8-only
    // file (AZW3) is read as it is.
    let h = first;
    let raw = mobi_text(&p, &h)?;
    let kf8 = h.version >= 8;

    let mut b = Book {
        title: exth_str(&h, 503).or_else(|| h.full_name.clone()).or_else(|| Some(decode(bytes[..32].split(|&c| c == 0).next().unwrap_or(&[]), false))),
        author: h.exth.get(&100).map(|v| v.iter().map(|d| decode(d, h.utf8).trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty()),
        language: exth_str(&h, 524),
        ..Default::default()
    };

    // Pictures: the records from the first image on (MOBI numbers them from 1).
    if h.first_image != usize::MAX {
        let mut total = 0;
        for r in h.first_image..p.len() {
            let data = p.record(r);
            if data.starts_with(b"FLIS") || data.starts_with(b"FCIS") || data.starts_with(b"BOUNDARY") || data.starts_with(b"\xE9\x8E\r\n") {
                if data.starts_with(b"BOUNDARY") {
                    break;
                }
                continue;
            }
            let Some(mime) = image_mime(data) else { continue };
            if data.len() > MAX_IMAGE || total > MAX_IMAGES_TOTAL {
                continue;
            }
            total += data.len();
            b.resources.insert(format!("mobi:{}", r - h.first_image + 1), Resource { mime: mime.into(), data: data.to_vec() });
        }
        b.cover = exth_u32(&h, 201).map(|o| format!("mobi:{}", o + 1)).filter(|k| b.resources.contains_key(k));
    }

    let html = if kf8 { kf8_html(&raw, h.utf8) } else { mobi6_html(&raw, h.utf8) };
    b.chapters = split_chapters(&html, kf8);
    Ok(b)
}

/// MOBI 6: `filepos` links point at byte offsets in the text; an anchor is put at each one.
fn mobi6_html(raw: &[u8], utf8: bool) -> String {
    let mut targets: Vec<usize> = Vec::new();
    let pat = b"filepos=";
    let mut i = 0;
    while let Some(p) = find(&raw[i..], pat) {
        let mut j = i + p + pat.len();
        while j < raw.len() && (raw[j] == b'"' || raw[j] == b'\'') {
            j += 1;
        }
        let s = j;
        while j < raw.len() && raw[j].is_ascii_digit() {
            j += 1;
        }
        if let Some(n) = std::str::from_utf8(&raw[s..j]).ok().and_then(|t| t.parse::<usize>().ok()) {
            targets.push(n);
        }
        i = j.max(i + p + 1);
    }
    targets.sort_unstable();
    targets.dedup();
    let mut out = Vec::with_capacity(raw.len() + targets.len() * 24);
    let mut last = 0;
    for t in targets.into_iter().filter(|&t| t < raw.len()) {
        // Never inside a tag: move back to its start.
        let mut at = t;
        if let Some(lt) = raw[last..at].iter().rposition(|&c| c == b'<') {
            if !raw[last + lt..at].contains(&b'>') {
                at = last + lt;
            }
        }
        out.extend_from_slice(&raw[last..at]);
        out.extend_from_slice(format!("<a id=\"fp{t}\"></a>").as_bytes());
        last = at;
    }
    out.extend_from_slice(&raw[last..]);
    let s = decode(&out, utf8);
    let s = replace_attr(&s, "filepos", |v| format!("href=\"#fp{}\"", v.trim_start_matches('0').parse::<usize>().unwrap_or(0)));
    replace_attr(&s, "recindex", |v| format!("src=\"mobi:{}\"", v.trim_start_matches('0').parse::<usize>().unwrap_or(0)))
}

/// KF8: pictures are `kindle:embed:XXXX` (base 32); its own position links aren't followed.
fn kf8_html(raw: &[u8], utf8: bool) -> String {
    let s = decode(raw, utf8);
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(p) = rest.find("kindle:embed:") {
        out.push_str(&rest[..p]);
        let after = &rest[p + 13..];
        let end = after.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(after.len());
        let n = usize::from_str_radix(&after[..end], 32).unwrap_or(0);
        out.push_str(&format!("mobi:{n}"));
        // Drop "?mime=image/jpg".
        let tail = &after[end..];
        rest = if tail.starts_with('?') { &tail[tail.find(['"', '\'', ')']).unwrap_or(tail.len())..] } else { tail };
    }
    out.push_str(rest);
    out.replace("kindle:pos:", "#kindle-pos:")
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w.eq_ignore_ascii_case(n))
}

/// `name=VALUE` / `name="VALUE"` attributes → whatever `f` makes of the value.
fn replace_attr(s: &str, name: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let pat = format!("{name}=");
    while let Some(p) = rest.find(&pat) {
        out.push_str(&rest[..p]);
        let after = &rest[p + pat.len()..];
        let (val, tail) = match after.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let e = after[1..].find(q).map(|e| e + 1).unwrap_or(after.len() - 1);
                (&after[1..e], &after[(e + 1).min(after.len())..])
            }
            _ => {
                let e = after.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(after.len());
                (&after[..e], &after[e..])
            }
        };
        out.push_str(&f(val));
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// MOBI 6 chapters end at `<mbp:pagebreak>`; KF8 parts each start with `<html`.
fn split_chapters(html: &str, kf8: bool) -> Vec<Chapter> {
    let lower = html.to_ascii_lowercase();
    let marker = if kf8 { "<html" } else { "<mbp:pagebreak" };
    let mut cuts: Vec<usize> = lower.match_indices(marker).map(|(i, _)| i).collect();
    cuts.insert(0, 0);
    cuts.push(html.len());
    cuts.dedup();
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let part = &html[w[0]..w[1]];
        // Very small pieces (a stray break) join the next chapter.
        let has_text = part.split('<').map(|t| t.split_once('>').map(|x| x.1).unwrap_or(t)).any(|t| !t.trim().is_empty());
        let has_img = part.contains("mobi:");
        if !has_text && !has_img {
            continue;
        }
        out.push(Chapter { path: format!("part{}", out.len() + 1), html: part.to_string() });
    }
    out
}

// ------------------------------------------------------------------ FB2

fn fb2(bytes: &[u8]) -> Result<Book> {
    let src = decode_text(bytes);
    let mut b = Book::default();
    let mut path: Vec<String> = Vec::new();
    let (mut first, mut last) = (String::new(), String::new());
    let mut html = String::new();
    let mut body_n = 0;
    let mut section_depth = 0usize;
    let mut binary: Option<(String, String, String)> = None;
    let mut cover_ref = None;
    let mut chapter_titles: Vec<(String, u32)> = Vec::new();
    let mut in_title = false;
    let mut title_text = String::new();
    let flush = |b: &mut Book, html: &mut String| {
        if !html.trim().is_empty() {
            let n = b.chapters.len() + 1;
            b.chapters.push(Chapter { path: format!("section{n}"), html: std::mem::take(html) });
        }
        html.clear();
    };
    for ev in xml::events(&src) {
        match ev {
            Ev::Open(n, a) => {
                let n = n.rsplit(':').next().unwrap_or(n).to_string();
                let in_body = path.iter().any(|p| p == "body");
                match n.as_str() {
                    "body" => {
                        body_n += 1;
                    }
                    "binary" => binary = Some((xml::attr(a, "id").unwrap_or_default(), xml::attr(a, "content-type").unwrap_or_default(), String::new())),
                    "section" if in_body && body_n == 1 => {
                        section_depth += 1;
                        if section_depth == 1 {
                            flush(&mut b, &mut html);
                        }
                    }
                    _ if in_body => {
                        if n == "title" {
                            in_title = true;
                            title_text.clear();
                        }
                        html.push_str(&fb2_open(&n, a, section_depth));
                    }
                    _ => {}
                }
                path.push(n);
            }
            Ev::Close(n) => {
                let n = n.rsplit(':').next().unwrap_or(n);
                path.pop();
                let in_body = path.iter().any(|p| p == "body");
                match n {
                    "binary" => {
                        if let Some((id, mime, data)) = binary.take() {
                            let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
                            if let Ok(d) = base64::engine::general_purpose::STANDARD.decode(clean) {
                                if d.len() <= MAX_IMAGE {
                                    let mime = if mime.is_empty() { image_mime(&d).unwrap_or("image/jpeg").to_string() } else { mime };
                                    b.resources.insert(format!("fb2:{id}"), Resource { mime, data: d });
                                }
                            }
                        }
                    }
                    "section" if in_body && body_n == 1 => {
                        section_depth = section_depth.saturating_sub(1);
                        if section_depth == 0 {
                            flush(&mut b, &mut html);
                        }
                    }
                    "body" => flush(&mut b, &mut html),
                    _ if in_body => {
                        if n == "title" && in_title {
                            in_title = false;
                            let t = title_text.split_whitespace().collect::<Vec<_>>().join(" ");
                            if !t.is_empty() && section_depth > 0 {
                                chapter_titles.push((t, section_depth as u32 - 1));
                            }
                        }
                        html.push_str(fb2_close(n, section_depth));
                    }
                    _ => {}
                }
            }
            Ev::Empty(n, a) => {
                let n = n.rsplit(':').next().unwrap_or(n);
                let href = xml::attr(a, "l:href").or_else(|| xml::attr(a, "xlink:href")).unwrap_or_default();
                if n == "image" && path.iter().any(|p| p == "coverpage") {
                    cover_ref = Some(format!("fb2:{}", href.trim_start_matches('#')));
                } else if path.iter().any(|p| p == "body") {
                    match n {
                        "image" => html.push_str(&format!("<img src=\"fb2:{}\" alt=\"\">", escape(href.trim_start_matches('#')))),
                        "empty-line" => html.push_str("<br>"),
                        _ => {}
                    }
                }
            }
            Ev::Text(t) => {
                if let Some((_, _, data)) = binary.as_mut() {
                    data.push_str(&t);
                    continue;
                }
                let leaf = path.last().map(String::as_str).unwrap_or("");
                let ti = path.iter().any(|p| p == "title-info");
                match leaf {
                    "book-title" if ti => b.title = Some(t.trim().to_string()),
                    "first-name" if ti && path.iter().any(|p| p == "author") && first.is_empty() => first = t.trim().to_string(),
                    "last-name" if ti && path.iter().any(|p| p == "author") && last.is_empty() => last = t.trim().to_string(),
                    "lang" if ti => b.language = Some(t.trim().to_string()),
                    _ if path.iter().any(|p| p == "body") => {
                        if in_title {
                            title_text.push_str(&t);
                            title_text.push(' ');
                        }
                        html.push_str(&escape(&t));
                    }
                    _ => {}
                }
            }
        }
    }
    flush(&mut b, &mut html);
    let author = format!("{first} {last}").trim().to_string();
    b.author = (!author.is_empty()).then_some(author);
    b.cover = cover_ref.filter(|k| b.resources.contains_key(k));
    // Section titles become the table of contents, in chapter order.
    let mut ch = 0;
    for (t, depth) in chapter_titles {
        if depth == 0 {
            ch += 1;
        }
        b.toc.push(TocEntry { title: t, href: format!("section{}", ch.max(1)), depth });
    }
    Ok(b)
}

fn fb2_open(n: &str, a: &str, depth: usize) -> String {
    match n {
        "title" => format!("<h{}>", (depth + 1).clamp(2, 5)),
        "subtitle" => "<h5>".into(),
        "p" => "<p>".into(),
        "emphasis" => "<em>".into(),
        "strong" => "<strong>".into(),
        "strikethrough" => "<s>".into(),
        "sup" | "sub" | "code" => format!("<{n}>"),
        "epigraph" | "cite" => "<blockquote>".into(),
        "poem" | "stanza" => "<div class=\"poem\">".into(),
        "v" => "<p class=\"verse\">".into(),
        "text-author" => "<p class=\"author\">".into(),
        "table" | "tr" | "td" | "th" => format!("<{n}>"),
        "a" => {
            let h = xml::attr(a, "l:href").or_else(|| xml::attr(a, "xlink:href")).unwrap_or_default();
            format!("<a href=\"{}\">", escape(&h))
        }
        "section" => {
            let id = xml::attr(a, "id").map(|i| format!(" id=\"{}\"", escape(&i))).unwrap_or_default();
            format!("<section{id}>")
        }
        _ => String::new(),
    }
}

fn fb2_close(n: &str, depth: usize) -> &'static str {
    match n {
        "title" => match (depth + 1).clamp(2, 5) {
            2 => "</h2>",
            3 => "</h3>",
            4 => "</h4>",
            _ => "</h5>",
        },
        "subtitle" => "</h5>",
        "p" | "v" | "text-author" => "</p>",
        "emphasis" => "</em>",
        "strong" => "</strong>",
        "strikethrough" => "</s>",
        "sup" => "</sup>",
        "sub" => "</sub>",
        "code" => "</code>",
        "epigraph" | "cite" => "</blockquote>",
        "poem" | "stanza" => "</div>",
        "table" => "</table>",
        "tr" => "</tr>",
        "td" => "</td>",
        "th" => "</th>",
        "a" => "</a>",
        "section" => "</section>",
        _ => "",
    }
}

// ------------------------------------------------------------------ plain text

/// Paragraphs at blank lines; a short line starting "Chapter …" (or "CHAPTER", "Part …") starts
/// a new chapter.
fn text(src: &str) -> Book {
    let mut b = Book::default();
    let mut html = String::new();
    let mut para = String::new();
    let heading = |l: &str| {
        let w = l.split_whitespace().next().unwrap_or("").to_lowercase();
        l.len() < 80 && matches!(w.as_str(), "chapter" | "part" | "book" | "prologue" | "epilogue")
    };
    let end_para = |html: &mut String, para: &mut String| {
        if !para.trim().is_empty() {
            html.push_str(&format!("<p>{}</p>\n", escape(para.trim())));
        }
        para.clear();
    };
    for line in src.lines() {
        let l = line.trim();
        if l.is_empty() {
            end_para(&mut html, &mut para);
        } else if heading(l) {
            end_para(&mut html, &mut para);
            if html.len() > 200 {
                let n = b.chapters.len() + 1;
                b.chapters.push(Chapter { path: format!("part{n}"), html: std::mem::take(&mut html) });
            }
            b.toc.push(TocEntry { title: l.to_string(), href: format!("part{}", b.chapters.len() + 1), depth: 0 });
            html.push_str(&format!("<h2>{}</h2>\n", escape(l)));
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(l);
        }
    }
    end_para(&mut html, &mut para);
    if !html.is_empty() {
        let n = b.chapters.len() + 1;
        b.chapters.push(Chapter { path: format!("part{n}"), html });
    }
    b
}

/// The plain text of each chapter (for the vault's index).
pub fn chapter_texts(b: &Book) -> Vec<String> {
    b.chapters.iter().map(|c| crate::formats::html_to_text(c.html.as_bytes())).filter(|t| t.trim().len() >= 20).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palmdoc_round_trip_pieces() {
        let mut out = Vec::new();
        // "abc" literal, then a copy of 3 bytes from 3 back, then " x" as a space pair.
        palmdoc(&[b'a', b'b', b'c', 0x80, 0x18, b'x' ^ 0x80], &mut out);
        assert_eq!(&out[..6], b"abcabc");
        assert_eq!(&out[6..], b" x");
    }

    #[test]
    fn trailing_multibyte_and_entries() {
        // One trailing entry of size 2 (0x82 = size 2, first byte), then nothing else.
        assert_eq!(trailing(&[1, 2, 3, 9, 0x82], 0b10), 2);
        // Multibyte overlap only: the last byte says 1 extra byte (+1).
        assert_eq!(trailing(&[1, 2, 3, 0x01], 0b01), 2);
    }

    #[test]
    fn resolves_relative_paths() {
        assert_eq!(resolve("OEBPS/text/", "../img/a%20b.jpg"), "OEBPS/img/a b.jpg");
        assert_eq!(resolve_link("OEBPS/", "ch2.xhtml#s1"), "OEBPS/ch2.xhtml#s1");
    }

    #[test]
    fn plain_text_chapters() {
        let b = text(&format!("Chapter 1\n\n{}\n\nChapter 2\n\nSecond.", "Words here. ".repeat(30)));
        assert_eq!(b.chapters.len(), 2);
        assert_eq!(b.toc.len(), 2);
        assert_eq!(b.toc[1].href, "part2");
    }

    #[test]
    fn mobi6_filepos_anchors() {
        let raw = b"<p><a filepos=0000000030>x</a></p><mbp:pagebreak/><p>there</p>";
        let h = mobi6_html(raw, true);
        assert!(h.contains("href=\"#fp30\""), "{h}");
        assert!(h.contains("<a id=\"fp30\"></a>"), "{h}");
    }

    #[test]
    fn fb2_basic() {
        let src = r#"<?xml version="1.0"?><FictionBook xmlns:l="http://www.w3.org/1999/xlink"><description><title-info><author><first-name>Ann</first-name><last-name>Lee</last-name></author><book-title>Tide</book-title></title-info></description><body><section><title><p>One</p></title><p>Hello <emphasis>there</emphasis>.</p></section><section><title><p>Two</p></title><p>Bye.</p></section></body></FictionBook>"#;
        let b = fb2(src.as_bytes()).unwrap();
        assert_eq!(b.title.as_deref(), Some("Tide"));
        assert_eq!(b.author.as_deref(), Some("Ann Lee"));
        assert_eq!(b.chapters.len(), 2);
        assert!(b.chapters[0].html.contains("<em>there</em>"));
        assert_eq!(b.toc.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["One", "Two"]);
    }

    /// Real books: `RAD_BOOKS=folder cargo test -p rad-ingest real_books -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_books() {
        let dir = std::env::var("RAD_BOOKS").expect("RAD_BOOKS");
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let b = open(&std::fs::read(&p).unwrap(), &name).unwrap_or_else(|e| panic!("{name}: {e}"));
            let words: usize = chapter_texts(&b).iter().map(|t| t.split_whitespace().count()).sum();
            println!("{name}: {:?} by {:?}, {} chapters, {} words, {} toc, {} pictures, cover {:?}", b.title, b.author, b.chapters.len(), words, b.toc.len(), b.resources.len(), b.cover);
            let first = chapter_texts(&b).into_iter().find(|t| t.len() > 400).unwrap_or_default();
            println!("   {}", first.chars().take(300).collect::<String>().replace('\n', " "));
            assert!(words > 20_000, "{name}: only {words} words");
        }
    }
}

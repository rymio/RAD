use std::io::{Cursor, Read};

use rad_api::Anchor;

use crate::xml::{self, Ev};
use crate::{classify, decode_text, detect_mime, Extracted, IngestError, Kind, Media, Result, Section};

const MAX_DEPTH: u32 = 3;
const MAX_ZIP_ENTRIES: usize = 2_000;
const MAX_ZIP_TOTAL: u64 = 1024 * 1024 * 1024;
const MAX_ENTRY: u64 = 256 * 1024 * 1024;

fn perr(kind: &'static str, e: impl std::fmt::Display) -> IngestError {
    IngestError::Parse { kind, msg: e.to_string() }
}

fn base(kind: Kind) -> Extracted {
    Extracted { kind, title: None, sections: Vec::new(), page_count: None, media: Media::None, notice: None }
}

pub(crate) fn extract_kind(kind: Kind, bytes: &[u8], filename: &str, depth: u32) -> Result<Extracted> {
    match kind {
        Kind::Pdf => pdf(bytes),
        Kind::Docx => docx(bytes),
        Kind::Pptx => pptx(bytes),
        Kind::Spreadsheet => spreadsheet(bytes),
        Kind::Odt => odf(bytes, false),
        Kind::Odp => odf(bytes, true),
        Kind::Csv => csv_file(bytes, filename),
        Kind::Html => html(bytes),
        Kind::Epub => epub(bytes),
        Kind::Email => email(bytes, depth),
        Kind::Markdown => {
            let text = decode_text(bytes);
            let mut e = base(kind);
            e.title = text.lines().find_map(|l| l.strip_prefix("# ")).map(|t| t.trim().to_string());
            e.sections.push(Section { text, anchor: Anchor::default(), kind: "text", markdown: true });
            Ok(e)
        }
        Kind::Text => {
            let mut text = decode_text(bytes);
            if crate::extension(filename) == "rtf" {
                text = strip_rtf(&text);
            }
            let mut e = base(kind);
            e.sections.push(Section::text(text, Anchor::default()));
            Ok(e)
        }
        Kind::Image => {
            let mut e = base(kind);
            match image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok().and_then(|r| r.into_dimensions().ok()) {
                Some((w, h)) => e.media = Media::Image { width: w, height: h },
                None => {
                    e.media = Media::Image { width: 0, height: 0 };
                    e.notice = Some("This image format can't be previewed or captioned (convert HEIC/RAW to JPEG or PNG).".into());
                }
            }
            Ok(e)
        }
        Kind::Audio => Ok(Extracted { media: Media::Audio, ..base(kind) }),
        Kind::Video => Ok(Extracted { media: Media::Video, ..base(kind) }),
        Kind::Zip => zip_archive(bytes, depth),
        Kind::Binary => Ok(Extracted {
            notice: Some("Stored in the vault; this file type has no extractable text (searchable by name).".into()),
            ..base(kind)
        }),
    }
}

// ------------------------------------------------------------------ PDF

fn pdf(bytes: &[u8]) -> Result<Extracted> {
    let pages = pdf_extract::extract_text_from_mem_by_pages(bytes).map_err(|e| perr("PDF", e))?;
    let mut e = base(Kind::Pdf);
    e.page_count = Some(pages.len() as i64);
    let mut total = 0usize;
    for (i, p) in pages.iter().enumerate() {
        let t = clean_pdf_text(p);
        total += t.trim().len();
        if !t.trim().is_empty() {
            e.sections.push(Section::text(t, Anchor { page: Some(i as u32 + 1), ..Default::default() }));
        }
    }
    if total < 40 * pages.len().max(1) / 4 {
        e.notice = Some("This PDF has little or no text layer (scanned?). Only the text found was indexed.".into());
    }
    e.title = e.sections.first().and_then(|s| s.text.lines().map(str::trim).find(|l| l.len() > 3 && l.len() < 120)).map(String::from);
    Ok(e)
}

fn clean_pdf_text(s: &str) -> String {
    // Join hyphenated line breaks and collapse runs of blank lines.
    let s = s.replace("-\n", "");
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let l = line.trim_end();
        if l.trim().is_empty() {
            blank += 1;
            if blank <= 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

// ------------------------------------------------------------------ zip helpers

fn open_zip<'a>(bytes: &'a [u8], kind: &'static str) -> Result<zip::ZipArchive<Cursor<&'a [u8]>>> {
    zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| perr(kind, e))
}

fn zip_text(z: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<String> {
    let mut f = z.by_name(name).ok()?;
    if f.size() > MAX_ENTRY {
        return None;
    }
    let mut s = Vec::new();
    f.by_ref().take(MAX_ENTRY).read_to_end(&mut s).ok()?;
    Some(String::from_utf8_lossy(&s).into_owned())
}

fn zip_names(z: &zip::ZipArchive<Cursor<&[u8]>>) -> Vec<String> {
    z.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect()
}

fn numbered(names: &[String], prefix: &str) -> Vec<(u32, String)> {
    let mut v: Vec<(u32, String)> = names
        .iter()
        .filter_map(|n| {
            let rest = n.strip_prefix(prefix)?.strip_suffix(".xml")?;
            rest.parse::<u32>().ok().map(|i| (i, n.clone()))
        })
        .collect();
    v.sort();
    v
}

// ------------------------------------------------------------------ DOCX

fn docx(bytes: &[u8]) -> Result<Extracted> {
    let mut z = open_zip(bytes, "Word document")?;
    let doc = zip_text(&mut z, "word/document.xml").ok_or_else(|| perr("Word document", "missing word/document.xml"))?;
    let mut md = String::new();
    let mut para = String::new();
    let mut heading: Option<usize> = None;
    let mut table_depth = 0usize;
    let mut cell = String::new();
    let mut row: Vec<String> = Vec::new();
    let mut rows_in_table = 0usize;
    let mut in_text = false;

    for ev in xml::events(&doc) {
        match ev {
            Ev::Open("w:p", _) => {
                para.clear();
                heading = None;
            }
            Ev::Empty("w:pStyle", a) | Ev::Open("w:pStyle", a) => {
                if let Some(v) = xml::attr(a, "w:val") {
                    let lv = v.to_lowercase();
                    if lv == "title" {
                        heading = Some(1);
                    } else if let Some(n) = lv.strip_prefix("heading") {
                        heading = n.trim().parse::<usize>().ok().map(|n| n.clamp(1, 6));
                    }
                }
            }
            Ev::Open("w:t", _) => in_text = true,
            Ev::Close("w:t") => in_text = false,
            Ev::Text(t) if in_text => para.push_str(&t),
            Ev::Empty("w:tab", _) => para.push('\t'),
            Ev::Empty("w:br", _) | Ev::Empty("w:cr", _) => para.push('\n'),
            Ev::Close("w:p") => {
                let p = para.trim();
                if table_depth > 0 {
                    if !p.is_empty() {
                        if !cell.is_empty() {
                            cell.push(' ');
                        }
                        cell.push_str(p);
                    }
                } else if !p.is_empty() {
                    if let Some(h) = heading {
                        md.push_str(&"#".repeat(h));
                        md.push(' ');
                    }
                    md.push_str(p);
                    md.push_str("\n\n");
                }
            }
            Ev::Open("w:tbl", _) => {
                table_depth += 1;
                rows_in_table = 0;
            }
            Ev::Close("w:tbl") => {
                table_depth = table_depth.saturating_sub(1);
                md.push('\n');
            }
            Ev::Close("w:tc") => row.push(std::mem::take(&mut cell).replace('|', "\\|")),
            Ev::Close("w:tr") => {
                if table_depth == 1 {
                    md.push_str(&format!("| {} |\n", row.join(" | ")));
                    if rows_in_table == 0 {
                        md.push_str(&format!("|{}\n", " --- |".repeat(row.len().max(1))));
                    }
                    rows_in_table += 1;
                }
                row.clear();
            }
            _ => {}
        }
    }
    let mut e = base(Kind::Docx);
    e.title = zip_text(&mut z, "docProps/core.xml").and_then(|core| core_title(&core));
    e.sections.push(Section { text: md, anchor: Anchor::default(), kind: "text", markdown: true });
    Ok(e)
}

fn core_title(core: &str) -> Option<String> {
    let evs = xml::events(core);
    let mut in_title = false;
    for ev in evs {
        match ev {
            Ev::Open("dc:title", _) => in_title = true,
            Ev::Text(t) if in_title && !t.trim().is_empty() => return Some(t.trim().to_string()),
            Ev::Close("dc:title") => in_title = false,
            _ => {}
        }
    }
    None
}

// ------------------------------------------------------------------ PPTX

fn drawing_text(xml_src: &str) -> String {
    let mut out = String::new();
    let mut in_t = false;
    for ev in xml::events(xml_src) {
        match ev {
            Ev::Open("a:t", _) => in_t = true,
            Ev::Close("a:t") => in_t = false,
            Ev::Text(t) if in_t => out.push_str(&t),
            Ev::Close("a:p") => out.push('\n'),
            Ev::Empty("a:br", _) => out.push('\n'),
            _ => {}
        }
    }
    out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

fn pptx(bytes: &[u8]) -> Result<Extracted> {
    let mut z = open_zip(bytes, "PowerPoint")?;
    let names = zip_names(&z);
    let slides = numbered(&names, "ppt/slides/slide");
    let mut e = base(Kind::Pptx);
    e.page_count = Some(slides.len() as i64);
    for (n, name) in slides {
        let mut text = zip_text(&mut z, &name).map(|s| drawing_text(&s)).unwrap_or_default();
        if let Some(notes) = zip_text(&mut z, &format!("ppt/notesSlides/notesSlide{n}.xml")).map(|s| drawing_text(&s)) {
            let notes: String = notes.lines().filter(|l| l.trim().parse::<u32>().is_err()).collect::<Vec<_>>().join("\n");
            if !notes.trim().is_empty() {
                text.push_str("\n\nSpeaker notes:\n");
                text.push_str(&notes);
            }
        }
        if !text.trim().is_empty() {
            if n == 1 && e.title.is_none() {
                e.title = text.lines().next().map(|l| l.chars().take(120).collect());
            }
            e.sections.push(Section::text(text, Anchor { slide: Some(n), ..Default::default() }));
        }
    }
    if let Some(t) = zip_text(&mut z, "docProps/core.xml").and_then(|c| core_title(&c)) {
        e.title = Some(t);
    }
    Ok(e)
}

// ------------------------------------------------------------------ ODT / ODP

fn odf(bytes: &[u8], presentation: bool) -> Result<Extracted> {
    let kind_name = if presentation { "OpenDocument presentation" } else { "OpenDocument text" };
    let mut z = open_zip(bytes, kind_name)?;
    let content = zip_text(&mut z, "content.xml").ok_or_else(|| perr(kind_name, "missing content.xml"))?;
    let mut e = base(if presentation { Kind::Odp } else { Kind::Odt });
    let mut cur = String::new();
    let mut para = String::new();
    let mut in_para = 0usize;
    let mut heading_level: Option<usize> = None;
    let mut slide = 0u32;
    for ev in xml::events(&content) {
        match ev {
            Ev::Open("draw:page", _) if presentation => {
                if !cur.trim().is_empty() {
                    e.sections.push(Section::text(std::mem::take(&mut cur), Anchor { slide: Some(slide.max(1)), ..Default::default() }));
                }
                cur.clear();
                slide += 1;
            }
            Ev::Open("text:p", _) => {
                in_para += 1;
                heading_level = None;
            }
            Ev::Open("text:h", a) => {
                in_para += 1;
                heading_level = Some(xml::attr(a, "text:outline-level").and_then(|l| l.parse().ok()).unwrap_or(1usize).clamp(1, 6));
            }
            Ev::Text(t) if in_para > 0 => para.push_str(&t),
            Ev::Empty("text:s", a) => para.push_str(&" ".repeat(xml::attr(a, "text:c").and_then(|c| c.parse().ok()).unwrap_or(1usize).min(20))),
            Ev::Empty("text:tab", _) => para.push('\t'),
            Ev::Empty("text:line-break", _) => para.push('\n'),
            Ev::Close("text:p") | Ev::Close("text:h") => {
                in_para = in_para.saturating_sub(1);
                if in_para == 0 {
                    let p = para.trim();
                    if !p.is_empty() {
                        if let Some(h) = heading_level.filter(|_| !presentation) {
                            cur.push_str(&"#".repeat(h));
                            cur.push(' ');
                        }
                        cur.push_str(p);
                        cur.push_str(if presentation { "\n" } else { "\n\n" });
                    }
                    para.clear();
                }
            }
            _ => {}
        }
    }
    if !cur.trim().is_empty() {
        let anchor = if presentation { Anchor { slide: Some(slide.max(1)), ..Default::default() } } else { Anchor::default() };
        e.sections.push(Section { text: cur, anchor, kind: "text", markdown: !presentation });
    }
    if presentation {
        e.page_count = Some(slide as i64);
    }
    Ok(e)
}

// ------------------------------------------------------------------ spreadsheets & CSV

const ROWS_PER_BLOCK: usize = 40;

fn table_sections(e: &mut Extracted, sheet: Option<String>, rows: Vec<Vec<String>>) {
    let rows: Vec<Vec<String>> = rows.into_iter().filter(|r| r.iter().any(|c| !c.trim().is_empty())).collect();
    if rows.is_empty() {
        return;
    }
    let header = &rows[0];
    let fmt_row = |r: &Vec<String>| format!("| {} |", r.iter().map(|c| c.replace('|', "\\|").replace('\n', " ")).collect::<Vec<_>>().join(" | "));
    let head = format!("{}\n|{}", fmt_row(header), " --- |".repeat(header.len().max(1)));
    let body = &rows[1..];
    if body.is_empty() {
        e.sections.push(Section { text: head, anchor: Anchor { sheet: sheet.clone(), ..Default::default() }, kind: "table", markdown: false });
        return;
    }
    for (i, block) in body.chunks(ROWS_PER_BLOCK).enumerate() {
        let mut t = head.clone();
        for r in block {
            t.push('\n');
            t.push_str(&fmt_row(r));
        }
        let first = i * ROWS_PER_BLOCK + 2;
        let label = format!("rows {}–{}", first, first + block.len() - 1);
        e.sections.push(Section {
            text: t,
            anchor: Anchor { sheet: sheet.clone(), heading: Some(label), ..Default::default() },
            kind: "table",
            markdown: false,
        });
    }
}

fn spreadsheet(bytes: &[u8]) -> Result<Extracted> {
    use calamine::Reader;
    let mut wb = calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec())).map_err(|e| perr("spreadsheet", e))?;
    let mut e = base(Kind::Spreadsheet);
    let names = wb.sheet_names();
    e.page_count = Some(names.len() as i64);
    for name in names {
        let Ok(range) = wb.worksheet_range(&name) else { continue };
        let rows: Vec<Vec<String>> = range.rows().map(|r| r.iter().map(|c| c.to_string()).collect()).collect();
        table_sections(&mut e, Some(name), rows);
    }
    Ok(e)
}

fn csv_file(bytes: &[u8], filename: &str) -> Result<Extracted> {
    let text = decode_text(bytes);
    let delim = if crate::extension(filename) == "tsv" || text.lines().next().is_some_and(|l| l.matches('\t').count() > l.matches(',').count()) {
        b'\t'
    } else if text.lines().next().is_some_and(|l| l.matches(';').count() > l.matches(',').count()) {
        b';'
    } else {
        b','
    };
    let mut rdr = csv::ReaderBuilder::new().has_headers(false).flexible(true).delimiter(delim).from_reader(text.as_bytes());
    let rows: Vec<Vec<String>> = rdr.records().filter_map(|r| r.ok()).map(|r| r.iter().map(String::from).collect()).collect();
    let mut e = base(Kind::Csv);
    table_sections(&mut e, None, rows);
    Ok(e)
}

// ------------------------------------------------------------------ HTML / EPUB / email

fn html_title(src: &str) -> Option<String> {
    let lower = src.to_lowercase();
    let s = lower.find("<title")?;
    let s = s + lower[s..].find('>')? + 1;
    let e = s + lower[s..].find("</title")?;
    let t = xml::unescape(src[s..e].trim());
    (!t.is_empty()).then_some(t)
}

pub(crate) fn html_to_text(bytes: &[u8]) -> String {
    html2text::from_read(bytes, 120).unwrap_or_else(|_| decode_text(bytes))
}

fn html(bytes: &[u8]) -> Result<Extracted> {
    let src = decode_text(bytes);
    let mut e = base(Kind::Html);
    e.title = html_title(&src);
    e.sections.push(Section { text: html_to_text(src.as_bytes()), anchor: Anchor::default(), kind: "text", markdown: true });
    Ok(e)
}

fn epub(bytes: &[u8]) -> Result<Extracted> {
    let mut z = open_zip(bytes, "EPUB")?;
    let container = zip_text(&mut z, "META-INF/container.xml").unwrap_or_default();
    let opf_path = xml::events(&container)
        .into_iter()
        .find_map(|ev| match ev {
            Ev::Empty("rootfile", a) | Ev::Open("rootfile", a) => xml::attr(a, "full-path"),
            _ => None,
        })
        .unwrap_or_else(|| "OEBPS/content.opf".into());
    let opf = zip_text(&mut z, &opf_path).unwrap_or_default();
    let dir = opf_path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
    let mut manifest = std::collections::HashMap::new();
    let mut spine = Vec::new();
    let mut title = None;
    let mut in_title = false;
    for ev in xml::events(&opf) {
        match ev {
            Ev::Empty("item", a) | Ev::Open("item", a) => {
                if let (Some(id), Some(href)) = (xml::attr(a, "id"), xml::attr(a, "href")) {
                    manifest.insert(id, href);
                }
            }
            Ev::Empty("itemref", a) | Ev::Open("itemref", a) => {
                if let Some(id) = xml::attr(a, "idref") {
                    spine.push(id);
                }
            }
            Ev::Open("dc:title", _) => in_title = true,
            Ev::Text(t) if in_title => title = Some(t.trim().to_string()),
            Ev::Close("dc:title") => in_title = false,
            _ => {}
        }
    }
    let mut e = base(Kind::Epub);
    e.title = title;
    let mut chapter = 0u32;
    for id in spine {
        let Some(href) = manifest.get(&id) else { continue };
        let path = format!("{dir}{}", href.split('#').next().unwrap_or(href));
        let Some(src) = zip_text(&mut z, &path) else { continue };
        let text = html_to_text(src.as_bytes());
        if text.trim().len() < 20 {
            continue;
        }
        chapter += 1;
        let heading = html_title(&src).filter(|t| Some(t) != e.title.as_ref()).unwrap_or_else(|| format!("Chapter {chapter}"));
        e.sections.push(Section {
            text,
            anchor: Anchor { heading: Some(heading), ..Default::default() },
            kind: "text",
            markdown: true,
        });
    }
    e.page_count = Some(chapter as i64);
    Ok(e)
}

fn email(bytes: &[u8], depth: u32) -> Result<Extracted> {
    use mail_parser::MimeHeaders;
    let msg = mail_parser::MessageParser::default().parse(bytes).ok_or_else(|| perr("email", "unparseable message"))?;
    let mut e = base(Kind::Email);
    e.title = msg.subject().map(String::from);
    let addr = |a: Option<&mail_parser::Address>| {
        a.map(|a| {
            a.iter()
                .map(|x| match (x.name(), x.address()) {
                    (Some(n), Some(ad)) => format!("{n} <{ad}>"),
                    (None, Some(ad)) => ad.to_string(),
                    (Some(n), None) => n.to_string(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
    };
    let mut head = String::new();
    head.push_str(&format!("From: {}\n", addr(msg.from())));
    head.push_str(&format!("To: {}\n", addr(msg.to())));
    if let Some(d) = msg.date() {
        head.push_str(&format!("Date: {}\n", d.to_rfc3339()));
    }
    head.push_str(&format!("Subject: {}\n\n", msg.subject().unwrap_or("")));
    let body = msg
        .body_text(0)
        .map(|b| b.into_owned())
        .or_else(|| msg.body_html(0).map(|h| html_to_text(h.as_bytes())))
        .unwrap_or_default();
    e.sections.push(Section::text(format!("{head}{body}"), Anchor::default()));
    if depth < MAX_DEPTH {
        for att in msg.attachments() {
            let name = att.attachment_name().unwrap_or("attachment").to_string();
            let data = att.contents();
            let mime = detect_mime(data, &name);
            nested(&mut e, data, &name, &mime, depth + 1);
        }
    }
    Ok(e)
}

/// Extracts a nested file (zip entry, email attachment) into the parent with a file anchor.
fn nested(parent: &mut Extracted, data: &[u8], name: &str, mime: &str, depth: u32) {
    let kind = classify(mime, name);
    if matches!(kind, Kind::Image | Kind::Audio | Kind::Video | Kind::Binary) {
        parent.sections.push(Section {
            text: format!("Attached file: {name} ({mime}, {} bytes)", data.len()),
            anchor: Anchor { file: Some(name.to_string()), ..Default::default() },
            kind: "meta",
            markdown: false,
        });
        return;
    }
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| extract_kind(kind, data, name, depth)));
    if let Ok(Ok(child)) = r {
        for mut s in child.sections {
            let inner = s.anchor.file.take();
            s.anchor.file = Some(match inner {
                Some(i) => format!("{name}/{i}"),
                None => name.to_string(),
            });
            parent.sections.push(s);
        }
    }
}

fn zip_archive(bytes: &[u8], depth: u32) -> Result<Extracted> {
    let mut z = open_zip(bytes, "zip archive")?;
    let mut e = base(Kind::Zip);
    if depth >= MAX_DEPTH {
        return Ok(e);
    }
    let mut total = 0u64;
    let mut listing = String::from("Archive contents:\n");
    let n = z.len().min(MAX_ZIP_ENTRIES);
    for i in 0..n {
        let Ok(mut f) = z.by_index(i) else { continue };
        if f.is_dir() {
            continue;
        }
        let name = f.name().map(|n| n.into_owned()).unwrap_or_default();
        if name.starts_with("__MACOSX/") || name.ends_with(".DS_Store") {
            continue;
        }
        listing.push_str(&format!("- {name} ({} bytes)\n", f.size()));
        if f.size() > MAX_ENTRY || total + f.size() > MAX_ZIP_TOTAL {
            continue;
        }
        let mut data = Vec::with_capacity(f.size() as usize);
        if f.by_ref().take(MAX_ENTRY).read_to_end(&mut data).is_err() {
            continue;
        }
        total += data.len() as u64;
        drop(f);
        let mime = detect_mime(&data, &name);
        nested(&mut e, &data, &name, &mime, depth + 1);
    }
    e.sections.insert(0, Section { text: listing, anchor: Anchor::default(), kind: "meta", markdown: false });
    Ok(e)
}

fn strip_rtf(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    let mut depth = 0i32;
    let mut skip_depth: Option<i32> = None;
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' => {
                if skip_depth == Some(depth) {
                    skip_depth = None;
                }
                depth -= 1;
            }
            '\\' => {
                let mut word = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_alphabetic() {
                        word.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_digit() || n == '-' {
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                match word.as_str() {
                    "par" | "line" if skip_depth.is_none() => out.push('\n'),
                    "tab" if skip_depth.is_none() => out.push('\t'),
                    "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "header" | "footer" => skip_depth = Some(depth),
                    "" => {
                        if let Some(n) = chars.next() {
                            if skip_depth.is_none() && matches!(n, '\\' | '{' | '}') {
                                out.push(n);
                            }
                        }
                    }
                    _ => {}
                }
            }
            '\r' | '\n' => {}
            c if skip_depth.is_none() => out.push(c),
            _ => {}
        }
    }
    out
}

// ------------------------------------------------------------------ images

/// Flattens transparency onto white (JPEG has no alpha; naive conversion turns it black).
fn flatten(img: &image::DynamicImage) -> image::RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let mut out = image::RgbImage::new(rgba.width(), rgba.height());
    for (o, p) in out.pixels_mut().zip(rgba.pixels()) {
        let a = p[3] as u32;
        let mix = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        *o = image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])]);
    }
    out
}

/// A small JPEG preview for galleries (max 480 px).
pub fn thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let t = flatten(&img.thumbnail(480, 480));
    let mut out = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode_image(&t).ok()?;
    Some(out.into_inner())
}

/// Re-encodes an image for vision models: max 1568 px on the long side, JPEG.
pub fn vision_image(bytes: &[u8]) -> Option<(Vec<u8>, String)> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width().max(img.height()) > 1568 { img.resize(1568, 1568, image::imageops::FilterType::Lanczos3) } else { img };
    let mut out = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88).encode_image(&flatten(&img)).ok()?;
    Some((out.into_inner(), "image/jpeg".into()))
}

use super::*;
use std::io::Write;

fn make_zip(files: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

#[test]
fn markdown_chunks_carry_headings() {
    let mut md = String::from("# Guide\n\nIntro.\n\n## Install\n\n");
    for _ in 0..80 {
        md.push_str("Run the installer and follow the prompts carefully. ");
    }
    md.push_str("\n\n## Usage\n\nOpen the app.\n");
    let ex = extract(md.as_bytes(), "guide.md", "text/markdown").unwrap();
    assert_eq!(ex.title.as_deref(), Some("Guide"));
    let chunks = chunk_document(&ex, "guide.md", 1200, 100);
    assert!(chunks.len() >= 3);
    assert!(chunks.iter().any(|c| c.anchor.heading.as_deref() == Some("Guide › Install")));
    assert!(chunks.last().unwrap().anchor.heading.as_deref() == Some("Guide › Usage"));
}

#[test]
fn docx_headings_and_tables() {
    let doc = r#"<?xml version="1.0"?><w:document><w:body>
      <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Roof &amp; Gutter</w:t></w:r></w:p>
      <w:p><w:r><w:t xml:space="preserve">Paid </w:t></w:r><w:r><w:t>12,400.</w:t></w:r></w:p>
      <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Cost</w:t></w:r></w:p></w:tc></w:tr>
      <w:tr><w:tc><w:p><w:r><w:t>Shingles</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>9000</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
    </w:body></w:document>"#;
    let bytes = make_zip(&[("word/document.xml", doc)]);
    let ex = extract(&bytes, "house.docx", &detect_mime(&bytes, "house.docx")).unwrap();
    let t = &ex.sections[0].text;
    assert!(t.contains("# Roof & Gutter"), "{t}");
    assert!(t.contains("Paid 12,400."));
    assert!(t.contains("| Shingles | 9000 |"), "{t}");
}

#[test]
fn pptx_slides() {
    let s1 = r#"<p:sld><a:p><a:r><a:t>Quarterly plan</a:t></a:r></a:p></p:sld>"#;
    let s2 = r#"<p:sld><a:p><a:r><a:t>Budget is 50k</a:t></a:r></a:p></p:sld>"#;
    let bytes = make_zip(&[("ppt/slides/slide1.xml", s1), ("ppt/slides/slide2.xml", s2)]);
    let ex = extract(&bytes, "deck.pptx", "application/zip").unwrap();
    assert_eq!(ex.page_count, Some(2));
    assert_eq!(ex.sections[1].anchor.slide, Some(2));
}

#[test]
fn csv_tables_repeat_header() {
    let mut csv = String::from("date,amount,vendor\n");
    for i in 0..100 {
        csv.push_str(&format!("2025-01-{:02},{},Shop {i}\n", i % 28 + 1, i * 3));
    }
    let ex = extract(csv.as_bytes(), "spend.csv", "text/csv").unwrap();
    assert!(ex.sections.len() >= 3);
    assert!(ex.sections.iter().all(|s| s.text.starts_with("| date | amount | vendor |")));
}

#[test]
fn zip_recurses_and_media_classified() {
    let bytes = make_zip(&[("notes/a.txt", "alpha bravo charlie"), ("b.md", "# B\n\ndelta")]);
    let ex = extract(&bytes, "bundle.zip", "application/zip").unwrap();
    assert!(ex.sections.iter().any(|s| s.anchor.file.as_deref() == Some("notes/a.txt") && s.text.contains("bravo")));
    assert_eq!(classify("audio/mpeg", "song.mp3"), Kind::Audio);
    assert_eq!(classify("video/mp4", "clip.mp4"), Kind::Video);
    assert_eq!(classify("application/octet-stream", "clip.MOV"), Kind::Video);
    assert_eq!(classify("image/png", "shot.png"), Kind::Image);
    assert_eq!(classify("text/plain", "main.rs"), Kind::Text);
}

#[test]
fn email_with_attachment() {
    let eml = "From: Alice <alice@example.com>\r\nTo: rob@example.com\r\nSubject: Invoice\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=XX\r\n\r\n--XX\r\nContent-Type: text/plain\r\n\r\nPlease find the invoice attached.\r\n--XX\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=\"inv.txt\"\r\n\r\nTotal due 450 EUR\r\n--XX--\r\n";
    let ex = extract(eml.as_bytes(), "mail.eml", "message/rfc822").unwrap();
    assert_eq!(ex.title.as_deref(), Some("Invoice"));
    assert!(ex.sections[0].text.contains("Alice <alice@example.com>"));
    assert!(ex.sections.iter().any(|s| s.text.contains("450 EUR") && s.anchor.file.as_deref() == Some("inv.txt")));
}

#[test]
fn garbage_pdf_is_an_error_not_a_panic() {
    assert!(extract(b"%PDF-1.4 not really", "x.pdf", "application/pdf").is_err());
}

#[test]
fn latin1_text_decodes() {
    let bytes = b"Caf\xe9 cr\xe8me";
    assert_eq!(decode_text(bytes), "Café crème");
}

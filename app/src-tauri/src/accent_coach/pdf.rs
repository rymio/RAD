//! PDF import: the text of each page, read with macOS PDFKit.

use std::path::Path;

/// Raw text of every page, in order (empty for pages without a text layer).
#[cfg(target_os = "macos")]
pub fn page_texts(path: &Path) -> Result<Vec<String>, String> {
    use objc2::AnyThread;
    use objc2_foundation::{NSString, NSURL};
    use objc2_pdf_kit::PDFDocument;

    let path_str = path.to_str().ok_or("The PDF path is not valid UTF-8.")?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
    let doc = unsafe { PDFDocument::initWithURL(PDFDocument::alloc(), &url) }
        .ok_or("Could not open the PDF. Is it a valid, unlocked PDF file?")?;
    if unsafe { doc.isLocked() } {
        return Err("This PDF is password-protected. Unlock it first, then import it.".into());
    }
    let count = unsafe { doc.pageCount() };
    Ok((0..count)
        .map(|i| {
            unsafe { doc.pageAtIndex(i) }
                .and_then(|page| unsafe { page.string() })
                .map(|s| s.to_string())
                .unwrap_or_default()
        })
        .collect())
}

#[cfg(not(target_os = "macos"))]
pub fn page_texts(_path: &Path) -> Result<Vec<String>, String> {
    Err("PDF import needs macOS.".into())
}

/// Practice pages from a PDF: `(page_no, cleaned text, sentences)` for every page with text.
pub fn pages(path: &Path) -> Result<Vec<(i64, String, Vec<String>)>, String> {
    let pages: Vec<_> = page_texts(path)?
        .iter()
        .enumerate()
        .filter_map(|(i, raw)| {
            let body = reference::text::clean_pdf_page(raw);
            let sentences = reference::text::split_sentences(&body);
            (!sentences.is_empty()).then(|| (i as i64 + 1, body, sentences))
        })
        .collect();
    if pages.is_empty() {
        return Err("No text found in this PDF. It may be scanned images; this needs a PDF with selectable text.".into());
    }
    Ok(pages)
}

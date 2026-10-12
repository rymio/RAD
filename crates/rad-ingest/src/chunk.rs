use rad_api::Anchor;
use text_splitter::{ChunkConfig, MarkdownSplitter, TextSplitter};

use crate::{Extracted, Section};

#[derive(Debug, Clone)]
pub struct ChunkOut {
    pub text: String,
    /// Prepended for embeddings/keyword search: "Title › Heading".
    pub context: String,
    pub anchor: Anchor,
    pub kind: String,
}

/// Splits extracted sections into overlapping chunks of roughly `max_chars`.
pub fn chunk_document(ex: &Extracted, title: &str, max_chars: usize, overlap: usize) -> Vec<ChunkOut> {
    let max_chars = max_chars.max(300);
    let min_chars = (max_chars * 2 / 3).max(100);
    let overlap = overlap.min(max_chars / 3);
    let cfg = || ChunkConfig::new(min_chars..max_chars).with_overlap(overlap).expect("overlap < size").with_trim(true);
    let text_splitter = TextSplitter::new(cfg());
    let md_splitter = MarkdownSplitter::new(cfg());

    let mut out = Vec::new();
    for s in merge_small(&ex.sections, max_chars / 3) {
        if s.text.trim().is_empty() {
            continue;
        }
        let headings = if s.markdown { heading_index(&s.text) } else { Vec::new() };
        let pieces: Vec<(usize, &str)> = if s.text.len() <= max_chars {
            vec![(0, s.text.trim())]
        } else if s.markdown {
            md_splitter.chunk_indices(&s.text).collect()
        } else {
            text_splitter.chunk_indices(&s.text).collect()
        };
        for (offset, piece) in pieces {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            let mut anchor = s.anchor.clone();
            if anchor.heading.is_none() {
                anchor.heading = heading_at(&headings, offset);
            }
            let label = anchor.label();
            let context = if label.is_empty() { title.to_string() } else { format!("{title} › {label}") };
            out.push(ChunkOut { text: piece.to_string(), context, anchor, kind: s.kind.to_string() });
        }
    }
    out
}

/// Joins consecutive tiny sections (short PDF pages, slides) so chunks aren't fragments.
fn merge_small(sections: &[Section], small: usize) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for s in sections {
        if let Some(last) = out.last_mut() {
            let same_file = last.anchor.file == s.anchor.file;
            if same_file && last.kind == s.kind && last.text.len() < small && last.kind == "text" && last.markdown == s.markdown {
                last.text.push_str("\n\n");
                last.text.push_str(&s.text);
                continue;
            }
        }
        out.push(s.clone());
    }
    out
}

/// (byte offset, heading path) for every Markdown heading line.
fn heading_index(md: &str) -> Vec<(usize, String)> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut out = Vec::new();
    let mut offset = 0;
    let mut in_code = false;
    for line in md.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_code = !in_code;
        }
        if !in_code && t.starts_with('#') {
            let level = t.chars().take_while(|c| *c == '#').count();
            let text = t[level..].trim();
            if (1..=6).contains(&level) && !text.is_empty() && t[level..].starts_with(' ') {
                stack.retain(|(l, _)| *l < level);
                stack.push((level, text.chars().take(80).collect()));
                out.push((offset, stack.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" › ")));
            }
        }
        offset += line.len();
    }
    out
}

fn heading_at(index: &[(usize, String)], offset: usize) -> Option<String> {
    index.iter().take_while(|(o, _)| *o <= offset + 1).last().map(|(_, h)| h.clone())
}

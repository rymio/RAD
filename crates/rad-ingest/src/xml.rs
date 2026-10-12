//! A tiny, forgiving XML tokenizer — enough for OOXML/ODF/OPF text extraction.

#[derive(Debug)]
pub enum Ev<'a> {
    Open(&'a str, &'a str),
    Close(&'a str),
    Empty(&'a str, &'a str),
    Text(String),
}

pub fn events(s: &str) -> Vec<Ev<'_>> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            if s[i..].starts_with("<!--") {
                i = s[i..].find("-->").map(|p| i + p + 3).unwrap_or(b.len());
                continue;
            }
            if s[i..].starts_with("<![CDATA[") {
                let end = s[i..].find("]]>").map(|p| i + p).unwrap_or(b.len());
                out.push(Ev::Text(s[i + 9..end].to_string()));
                i = (end + 3).min(b.len());
                continue;
            }
            let end = match s[i..].find('>') {
                Some(p) => i + p,
                None => break,
            };
            let inner = &s[i + 1..end];
            i = end + 1;
            if inner.starts_with('?') || inner.starts_with('!') {
                continue;
            }
            if let Some(name) = inner.strip_prefix('/') {
                out.push(Ev::Close(name.trim()));
                continue;
            }
            let selfclose = inner.ends_with('/');
            let inner = inner.trim_end_matches('/');
            let (name, attrs) = match inner.find(|c: char| c.is_whitespace()) {
                Some(p) => (&inner[..p], &inner[p..]),
                None => (inner, ""),
            };
            out.push(if selfclose { Ev::Empty(name, attrs) } else { Ev::Open(name, attrs) });
        } else {
            let end = s[i..].find('<').map(|p| i + p).unwrap_or(b.len());
            let t = &s[i..end];
            if !t.is_empty() {
                out.push(Ev::Text(unescape(t)));
            }
            i = end;
        }
    }
    out
}

pub fn attr<'a>(attrs: &'a str, name: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(p) = rest.find(name) {
        let after = &rest[p + name.len()..];
        let before_ok = p == 0 || rest.as_bytes()[p - 1].is_ascii_whitespace();
        let after = after.trim_start();
        if before_ok {
            if let Some(after) = after.strip_prefix('=') {
                let after = after.trim_start();
                let q = after.chars().next()?;
                if q == '"' || q == '\'' {
                    let val = &after[1..];
                    let end = val.find(q)?;
                    return Some(unescape(&val[..end]));
                }
            }
        }
        rest = &rest[p + name.len()..];
    }
    None
}

pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

//! Commodore BASIC V2: program text → a C64 program (tokens) → a PRG file that loads at
//! $0801 and runs with RUN.
//!
//! Text is written as you'd type it on a C64, one numbered line per line:
//! `10 PRINT "{CLR}HELLO":GOTO 10`. Keywords may be lowercase. Like the C64 itself, keywords
//! are found anywhere outside strings, REM and DATA, even inside variable names (`SCORE` holds
//! `OR`) – so variable names should be short (only the first two letters count anyway).
//!
//! In strings, `{…}` names a control character the way listings in magazines did: `{CLR}`,
//! `{HOME}`, `{DOWN}`, `{UP}`, `{LEFT}`, `{RIGHT}`, `{RVS ON}`, `{RVS OFF}`, the colours
//! (`{BLACK}`, `{WHITE}`, `{RED}`, `{CYAN}`, `{PURPLE}`, `{GREEN}`, `{BLUE}`, `{YELLOW}`,
//! `{ORANGE}`, `{BROWN}`, `{LIGHT RED}`, `{DARK GRAY}`, `{GRAY}`, `{LIGHT GREEN}`,
//! `{LIGHT BLUE}`, `{LIGHT GRAY}`), `{F1}`…`{F8}`, `{DEL}`, `{INST}`, or `{n}` for any code.
//! `£`, `↑`, `←` and `π` work as themselves.

use std::fmt;

/// BASIC V2's keywords, from token 0x80 (END) to 0xCB (GO).
pub const KEYWORDS: [&str; 76] = [
    "END", "FOR", "NEXT", "DATA", "INPUT#", "INPUT", "DIM", "READ", "LET", "GOTO", "RUN", "IF", "RESTORE", "GOSUB", "RETURN", "REM", "STOP",
    "ON", "WAIT", "LOAD", "SAVE", "VERIFY", "DEF", "POKE", "PRINT#", "PRINT", "CONT", "LIST", "CLR", "CMD", "SYS", "OPEN", "CLOSE", "GET", "NEW",
    "TAB(", "TO", "FN", "SPC(", "THEN", "NOT", "STEP", "+", "-", "*", "/", "^", "AND", "OR", ">", "=", "<", "SGN", "INT", "ABS", "USR", "FRE",
    "POS", "SQR", "RND", "LOG", "EXP", "COS", "SIN", "TAN", "ATN", "PEEK", "LEN", "STR$", "VAL", "ASC", "CHR$", "LEFT$", "RIGHT$", "MID$", "GO",
];

const T_DATA: u8 = 0x83;
const T_REM: u8 = 0x8f;
const T_PI: u8 = 0xff;

/// Where BASIC programs live.
pub const START: u16 = 0x0801;

#[derive(Debug, Clone, PartialEq)]
pub struct BasicError {
    /// The line of the text file (1-based).
    pub text_line: usize,
    /// The BASIC line number, when there is one.
    pub line: Option<u16>,
    pub message: String,
}

impl fmt::Display for BasicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(n) => write!(f, "line {n} (text line {}): {}", self.text_line, self.message),
            None => write!(f, "text line {}: {}", self.text_line, self.message),
        }
    }
}

/// Control characters by their `{…}` names.
const CONTROLS: [(&str, u8); 40] = [
    ("CLR", 0x93), ("CLEAR", 0x93), ("HOME", 0x13), ("DOWN", 0x11), ("UP", 0x91), ("LEFT", 0x9d), ("RIGHT", 0x1d),
    ("RVS ON", 0x12), ("RVSON", 0x12), ("RVS OFF", 0x92), ("RVSOFF", 0x92), ("DEL", 0x14), ("INST", 0x94),
    ("BLACK", 0x90), ("BLK", 0x90), ("WHITE", 0x05), ("WHT", 0x05), ("RED", 0x1c), ("CYAN", 0x9f), ("CYN", 0x9f),
    ("PURPLE", 0x9c), ("PUR", 0x9c), ("GREEN", 0x1e), ("GRN", 0x1e), ("BLUE", 0x1f), ("BLU", 0x1f), ("YELLOW", 0x9e),
    ("YEL", 0x9e), ("ORANGE", 0x81), ("BROWN", 0x95), ("LIGHT RED", 0x96), ("DARK GRAY", 0x97), ("GRAY", 0x98),
    ("GREY", 0x98), ("LIGHT GREEN", 0x99), ("LIGHT BLUE", 0x9a), ("LIGHT GRAY", 0x9b), ("LOWER", 0x0e), ("UPPER", 0x8e), ("PI", T_PI),
];

const FKEYS: [u8; 8] = [0x85, 0x89, 0x86, 0x8a, 0x87, 0x8b, 0x88, 0x8c];

fn control(name: &str) -> Option<u8> {
    let n = name.trim().to_ascii_uppercase();
    if let Some(&(_, c)) = CONTROLS.iter().find(|(k, _)| *k == n) {
        return Some(c);
    }
    if let Some(f) = n.strip_prefix('F').and_then(|d| d.parse::<usize>().ok()).filter(|d| (1..=8).contains(d)) {
        return Some(FKEYS[f - 1]);
    }
    n.parse::<u16>().ok().filter(|v| *v <= 255).map(|v| v as u8)
}

/// A character of the text as PETSCII (letters as the C64's capitals).
fn petscii(c: char) -> Result<u8, String> {
    match c {
        'a'..='z' => Ok(c.to_ascii_uppercase() as u8),
        ' '..='Z' | '[' | ']' => Ok(c as u8),
        '£' => Ok(0x5c),
        '↑' => Ok(0x5e),
        '←' => Ok(0x5f),
        'π' => Ok(T_PI),
        '\t' => Ok(b' '),
        other => Err(format!("the C64 has no “{other}” character (use {{n}} for a character code)")),
    }
}

/// How long a line is when typed on the C64 (a `{…}` control character is one keypress).
fn typed_len(text: &str) -> usize {
    let mut n = 0;
    let mut in_brace = false;
    for c in text.chars() {
        match (c, in_brace) {
            ('{', false) => in_brace = true,
            ('}', true) => {
                in_brace = false;
                n += 1;
            }
            (_, true) => {}
            _ => n += 1,
        }
    }
    n
}

/// One line's text (after its number) → tokens.
fn tokenize(text: &str) -> Result<Vec<u8>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut quoted = false;
    let mut data = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            quoted = !quoted;
            out.push(b'"');
            i += 1;
            continue;
        }
        if quoted || data {
            if c == '{' {
                let close = chars[i..].iter().position(|&x| x == '}').ok_or("a “{” without its “}”")?;
                let name: String = chars[i + 1..i + close].iter().collect();
                out.push(control(&name).ok_or_else(|| format!("“{{{name}}}” isn't a control character RAD knows"))?);
                i += close + 1;
                continue;
            }
            if data && c == ':' {
                data = false;
            }
            out.push(petscii(c)?);
            i += 1;
            continue;
        }
        // Keywords, in the ROM's order: the first that matches here wins.
        let rest: String = chars[i..].iter().collect::<String>().to_ascii_uppercase();
        if let Some((k, kw)) = KEYWORDS.iter().enumerate().find(|(_, kw)| rest.starts_with(*kw)) {
            let t = 0x80 + k as u8;
            out.push(t);
            i += kw.chars().count();
            if t == T_REM {
                for &c in &chars[i..] {
                    out.push(petscii(c)?);
                }
                break;
            }
            if t == T_DATA {
                data = true;
            }
            continue;
        }
        if c == '?' {
            out.push(0x99); // ? is PRINT
        } else if c == 'π' {
            out.push(T_PI);
        } else {
            out.push(petscii(c)?);
        }
        i += 1;
    }
    Ok(out)
}

/// A program: its lines in order, tokenized, as it sits in memory from $0801 (each line's
/// link already pointing to the next).
pub fn compile(source: &str) -> Result<Vec<u8>, Vec<BasicError>> {
    let mut lines: Vec<(u16, usize, Vec<u8>)> = Vec::new();
    let mut errors = Vec::new();
    for (k, raw) in source.lines().enumerate() {
        let text_line = k + 1;
        let t = raw.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
            continue;
        }
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Ok(n) = digits.parse::<u32>() else {
            errors.push(BasicError { text_line, line: None, message: "every line needs a line number (0–63999) at the start".into() });
            continue;
        };
        if n > 63999 {
            errors.push(BasicError { text_line, line: None, message: format!("line number {n} is outside 0–63999") });
            continue;
        }
        let n = n as u16;
        if let Some(prev) = lines.iter().find(|(m, _, _)| *m == n) {
            errors.push(BasicError { text_line, line: Some(n), message: format!("line {n} is already used (text line {})", prev.1) });
            continue;
        }
        let body_text = t[digits.len()..].trim_start();
        if typed_len(body_text) > 80 - digits.len().min(5) - 1 {
            errors.push(BasicError {
                text_line,
                line: Some(n),
                message: "the line is longer than the C64's 80-character screen line (split it, or use abbreviations)".into(),
            });
            continue;
        }
        match tokenize(body_text) {
            Ok(body) if body.is_empty() => errors.push(BasicError { text_line, line: Some(n), message: "the line is empty".into() }),
            Ok(body) => lines.push((n, text_line, body)),
            Err(message) => errors.push(BasicError { text_line, line: Some(n), message }),
        }
    }
    if lines.is_empty() && errors.is_empty() {
        errors.push(BasicError { text_line: 1, line: None, message: "there's no program yet".into() });
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    lines.sort_by_key(|(n, _, _)| *n);
    let mut prog = Vec::new();
    let mut at = START as usize;
    for (n, _, body) in lines {
        let next = at + 4 + body.len() + 1;
        prog.extend((next as u16).to_le_bytes());
        prog.extend(n.to_le_bytes());
        prog.extend(body);
        prog.push(0);
        at = next;
    }
    prog.extend([0, 0]);
    if prog.len() > 38_000 {
        return Err(vec![BasicError { text_line: 1, line: None, message: format!("the program is {} bytes — too big for the C64's 38911 BASIC bytes", prog.len()) }]);
    }
    Ok(prog)
}

/// The program as a PRG file (load address $0801 first).
pub fn prg(program: &[u8]) -> Vec<u8> {
    let mut v = START.to_le_bytes().to_vec();
    v.extend_from_slice(program);
    v
}

/// A C64 program (as in memory from $0801, or a PRG's body) → text, like LIST, in the form
/// [`compile`] reads back (control characters as `{…}`).
pub fn detokenize(prog: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i + 4 <= prog.len() {
        let link = u16::from_le_bytes([prog[i], prog[i + 1]]);
        if link == 0 {
            break;
        }
        let n = u16::from_le_bytes([prog[i + 2], prog[i + 3]]);
        out.push_str(&n.to_string());
        out.push(' ');
        i += 4;
        let mut quoted = false;
        let mut raw = false; // after REM
        while i < prog.len() && prog[i] != 0 {
            let b = prog[i];
            i += 1;
            if b == b'"' {
                quoted = !quoted;
                out.push('"');
            } else if (0x80..=0xcb).contains(&b) && !quoted && !raw {
                out.push_str(KEYWORDS[(b - 0x80) as usize]);
                if b == T_REM {
                    raw = true;
                }
            } else {
                out.push_str(&char_text(b));
            }
        }
        i += 1;
        out.push('\n');
    }
    out
}

fn char_text(b: u8) -> String {
    match b {
        0x20..=0x5b | 0x5d => (b as char).to_string(),
        0x5c => "£".into(),
        0x5e => "↑".into(),
        0x5f => "←".into(),
        T_PI => "π".into(),
        _ => {
            if let Some((name, _)) = CONTROLS.iter().find(|(_, c)| *c == b) {
                format!("{{{name}}}")
            } else if let Some(f) = FKEYS.iter().position(|&c| c == b) {
                format!("{{F{}}}", f + 1)
            } else {
                format!("{{{b}}}")
            }
        }
    }
}

/// The BASIC program in a C64's memory (from $0801 to where its variables start), if any.
pub fn program_in_memory(mem: &[u8]) -> Option<Vec<u8>> {
    if mem.len() < 0x10000 {
        return None;
    }
    let start = u16::from_le_bytes([mem[0x2b], mem[0x2c]]) as usize;
    let end = u16::from_le_bytes([mem[0x2d], mem[0x2e]]) as usize;
    if start != START as usize || end <= start + 2 || end > 0xa000 {
        return None;
    }
    let p = mem[start..end].to_vec();
    (u16::from_le_bytes([p[0], p[1]]) != 0).then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_like_the_c64() {
        let p = compile("10 print \"{clr}hi\";a:goto 10\n20 rem score: goto\n30 data 1,goto,\"x\":print\n").unwrap();
        assert_eq!(&p[2..4], &[10, 0]);
        assert_eq!(p[4], 0x99);
        assert_eq!(&p[5..9], &[b' ', b'"', 0x93, b'H']);
        let text = detokenize(&p);
        assert_eq!(text, "10 PRINT \"{CLR}HI\";A:GOTO 10\n20 REM SCORE: GOTO\n30 DATA 1,GOTO,\"X\":PRINT\n");
        // The DATA line keeps GOTO as letters.
        assert!(p.windows(4).any(|w| w == b"GOTO"));
        // The links chain from $0801.
        let link = u16::from_le_bytes([p[0], p[1]]) as usize;
        assert_eq!(u16::from_le_bytes([p[link - 0x0801 + 2], p[link - 0x0801 + 3]]), 20);
    }

    #[test]
    fn keywords_hide_in_names() {
        // As on a real C64: "SCORE" holds OR.
        let p = compile("10 SCORE=1").unwrap();
        assert!(p.contains(&0xb0));
        assert_eq!(compile("10 X=1:?X").unwrap()[4..].iter().filter(|&&b| b == 0x99).count(), 1);
    }

    #[test]
    fn reports_mistakes() {
        let e = compile("PRINT 1\n10 PRINT \"{NOPE}\"\n10 END").unwrap_err();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].text_line, 1);
        assert!(e[1].message.contains("NOPE"));
    }
}

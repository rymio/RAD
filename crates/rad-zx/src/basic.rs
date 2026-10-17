//! Sinclair BASIC: program text → a Spectrum program (tokens, hidden numbers) → a TAP tape
//! that loads and runs it.
//!
//! Text is written as you'd type it on a Spectrum, one numbered line per line:
//! `10 PRINT AT 10,5; INK 2; "Hello"`. Keywords may be lowercase and `GOTO`/`GOSUB` may be
//! written without spaces. In strings, `\a`…`\u` are the user-defined graphics (UDGs A–U),
//! `\{n}` is the character with code n and `\\` a backslash; `£` and `©` work as themselves.

use std::fmt;

/// The Spectrum 48K's keywords, from token 0xA5 (RND) to 0xFF (COPY).
const KEYWORDS: [&str; 91] = [
    "RND", "INKEY$", "PI", "FN", "POINT", "SCREEN$", "ATTR", "AT", "TAB", "VAL$", "CODE", "VAL", "LEN", "SIN", "COS", "TAN", "ASN", "ACS", "ATN", "LN",
    "EXP", "INT", "SQR", "SGN", "ABS", "PEEK", "IN", "USR", "STR$", "CHR$", "NOT", "BIN", "OR", "AND", "<=", ">=", "<>", "LINE", "THEN", "TO", "STEP",
    "DEF FN", "CAT", "FORMAT", "MOVE", "ERASE", "OPEN #", "CLOSE #", "MERGE", "VERIFY", "BEEP", "CIRCLE", "INK", "PAPER", "FLASH", "BRIGHT",
    "INVERSE", "OVER", "OUT", "LPRINT", "LLIST", "STOP", "READ", "DATA", "RESTORE", "NEW", "BORDER", "CONTINUE", "DIM", "REM", "FOR", "GO TO",
    "GO SUB", "INPUT", "LOAD", "LIST", "LET", "PAUSE", "NEXT", "POKE", "PRINT", "PLOT", "RUN", "SAVE", "RANDOMIZE", "IF", "CLS", "DRAW", "CLEAR",
    "RETURN", "COPY",
];

const TOKEN_BASE: u8 = 0xA5;
const T_REM: u8 = 0xEA;
const T_BIN: u8 = 0xC4;

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

/// A number in the Spectrum's 5-byte form (small integers have their own short form).
pub fn number_bytes(v: f64) -> [u8; 5] {
    if v.fract() == 0.0 && v.abs() <= 65535.0 {
        let n = v.abs() as u32;
        let (n, sign) = if v < 0.0 { ((65536 - n) & 0xffff, 0xff) } else { (n, 0) };
        return [0, sign, n as u8, (n >> 8) as u8, 0];
    }
    if v == 0.0 {
        return [0; 5];
    }
    // v = m × 2^e with 0.5 ≤ m < 1; the mantissa's top bit (always 1) holds the sign.
    let neg = v < 0.0;
    let mut m = v.abs();
    let mut e: i32 = 0;
    while m >= 1.0 {
        m /= 2.0;
        e += 1;
    }
    while m < 0.5 {
        m *= 2.0;
        e -= 1;
    }
    let mut mant = (m * 4_294_967_296.0).round();
    if mant >= 4_294_967_296.0 {
        mant /= 2.0;
        e += 1;
    }
    let mant = mant as u64 as u32;
    let mant = (mant & 0x7fff_ffff) | if neg { 0x8000_0000 } else { 0 };
    let exp = (e + 128).clamp(1, 255) as u8;
    let b = mant.to_be_bytes();
    [exp, b[0], b[1], b[2], b[3]]
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// The keyword at the start of `s` (case-insensitive, `GOTO`/`GOSUB`/`DEFFN` allowed): its
/// token and how many characters it used.
fn keyword_at(s: &[char], prev: Option<char>) -> Option<(u8, usize)> {
    if prev.is_some_and(is_word_char) && s.first().is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut best: Option<(u8, usize)> = None;
    let mut try_word = |word: &str, token: u8| {
        let w: Vec<char> = word.chars().collect();
        if s.len() < w.len() {
            return;
        }
        if !s[..w.len()].iter().zip(&w).all(|(a, b)| a.eq_ignore_ascii_case(b)) {
            return;
        }
        // A keyword ending in a letter must not run into more letters ("TOTAL" isn't "TO").
        if w.last().is_some_and(|c| c.is_ascii_alphabetic()) && s.get(w.len()).is_some_and(|c| is_word_char(*c)) {
            return;
        }
        if best.is_none_or(|(_, l)| w.len() > l) {
            best = Some((token, w.len()));
        }
    };
    for (i, k) in KEYWORDS.iter().enumerate() {
        try_word(k, TOKEN_BASE + i as u8);
    }
    for (alias, token) in [("GOTO", 0xEC), ("GOSUB", 0xED), ("DEFFN", 0xCE), ("OPEN#", 0xD3), ("CLOSE#", 0xD4)] {
        try_word(alias, token);
    }
    best
}

/// One line of text (without its line number) → tokenized bytes.
fn tokenize(text: &str) -> Result<Vec<u8>, String> {
    let s: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut after_bin = false;
    while i < s.len() {
        let c = s[i];
        // Strings: copied as they are, with escapes.
        if c == '"' {
            out.push(b'"');
            i += 1;
            loop {
                let Some(&ch) = s.get(i) else { return Err("a string isn't closed (missing \")".into()) };
                if ch == '"' {
                    out.push(b'"');
                    i += 1;
                    if s.get(i) == Some(&'"') {
                        out.push(b'"');
                        i += 1;
                        continue;
                    }
                    break;
                }
                i += 1;
                if ch == '\\' {
                    let Some(&e) = s.get(i) else { return Err("a \\ at the end of a string".into()) };
                    i += 1;
                    match e {
                        'a'..='u' => out.push(0x90 + (e as u8 - b'a')),
                        'A'..='U' => out.push(0x90 + (e as u8 - b'A')),
                        '\\' => out.push(b'\\'),
                        '{' => {
                            let end = s[i..].iter().position(|c| *c == '}').ok_or("\\{ needs a closing }")?;
                            let n: String = s[i..i + end].iter().collect();
                            let n: u8 = n.trim().parse().map_err(|_| format!("\\{{{n}}} isn't a character code (0–255)"))?;
                            out.push(n);
                            i += end + 1;
                        }
                        other => return Err(format!("unknown escape \\{other} (use \\a–\\u for UDGs, \\{{n}} for a code, \\\\ for \\)")),
                    }
                } else {
                    out.push(char_code(ch)?);
                }
            }
            continue;
        }
        if c.is_whitespace() {
            // Spaces around keywords are the Spectrum's own; keep others (they're harmless).
            let next_is_kw = keyword_at(&s[i + 1..], Some(' ')).is_some();
            let prev_is_kw = out.last().is_some_and(|b| *b >= TOKEN_BASE);
            if !next_is_kw && !prev_is_kw {
                out.push(b' ');
            }
            i += 1;
            continue;
        }
        // Numbers (not digits inside a name like a1): text plus the hidden 5-byte value.
        let prev = if i > 0 { Some(s[i - 1]) } else { None };
        if (c.is_ascii_digit() || (c == '.' && s.get(i + 1).is_some_and(|d| d.is_ascii_digit()))) && !prev.is_some_and(|p| p.is_ascii_alphanumeric() || p == '$') {
            let start = i;
            if after_bin {
                while s.get(i).is_some_and(|d| *d == '0' || *d == '1') {
                    i += 1;
                }
                let digits: String = s[start..i].iter().collect();
                let v = u32::from_str_radix(&digits, 2).map_err(|_| "a BIN number is too long")?;
                out.extend(digits.bytes());
                out.push(0x0e);
                out.extend(number_bytes(v as f64));
                after_bin = false;
                continue;
            }
            while s.get(i).is_some_and(|d| d.is_ascii_digit() || *d == '.') {
                i += 1;
            }
            if s.get(i).is_some_and(|e| *e == 'e' || *e == 'E') {
                let mut j = i + 1;
                if s.get(j).is_some_and(|c| *c == '+' || *c == '-') {
                    j += 1;
                }
                if s.get(j).is_some_and(|d| d.is_ascii_digit()) {
                    i = j;
                    while s.get(i).is_some_and(|d| d.is_ascii_digit()) {
                        i += 1;
                    }
                }
            }
            let lit: String = s[start..i].iter().collect();
            let v: f64 = lit.parse().map_err(|_| format!("{lit} isn't a number"))?;
            if !v.is_finite() || v.abs() > 1.7e38 {
                return Err(format!("{lit} is too big for a Spectrum"));
            }
            out.extend(lit.bytes());
            out.push(0x0e);
            out.extend(number_bytes(v));
            continue;
        }
        if let Some((token, len)) = keyword_at(&s[i..], prev) {
            out.push(token);
            i += len;
            after_bin = token == T_BIN;
            if token == T_REM {
                // The rest of the line is the comment, as written.
                let rest: String = s[i..].iter().collect::<String>();
                for ch in rest.trim_start().chars() {
                    out.push(char_code(ch)?);
                }
                break;
            }
            continue;
        }
        out.push(char_code(c)?);
        i += 1;
    }
    Ok(out)
}

/// A character's Spectrum code.
fn char_code(c: char) -> Result<u8, String> {
    match c {
        '£' => Ok(0x60),
        '©' => Ok(0x7f),
        c if (' '..='~').contains(&c) && c != '`' => Ok(c as u8),
        '`' => Ok(0x60),
        other => Err(format!("the Spectrum has no “{other}” character")),
    }
}

/// A program: its lines in order, tokenized, ready to go in memory at PROG.
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
            errors.push(BasicError { text_line, line: None, message: "every line needs a line number (1–9999) at the start".into() });
            continue;
        };
        if n == 0 || n > 9999 {
            errors.push(BasicError { text_line, line: None, message: format!("line number {n} is outside 1–9999") });
            continue;
        }
        let n = n as u16;
        if let Some(prev) = lines.iter().find(|(m, _, _)| *m == n) {
            errors.push(BasicError { text_line, line: Some(n), message: format!("line {n} is already used (text line {})", prev.1) });
            continue;
        }
        match tokenize(&t[digits.len()..]) {
            Ok(mut body) => {
                while body.first() == Some(&b' ') {
                    body.remove(0);
                }
                if body.is_empty() {
                    errors.push(BasicError { text_line, line: Some(n), message: "the line is empty".into() });
                    continue;
                }
                body.push(0x0d);
                lines.push((n, text_line, body));
            }
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
    for (n, _, body) in lines {
        prog.extend_from_slice(&n.to_be_bytes());
        prog.extend_from_slice(&(body.len() as u16).to_le_bytes());
        prog.extend(body);
    }
    if prog.len() > 40_000 {
        return Err(vec![BasicError { text_line: 1, line: None, message: format!("the program is {} bytes — too big for a 48K Spectrum", prog.len()) }]);
    }
    Ok(prog)
}

fn checksummed(mut b: Vec<u8>) -> Vec<u8> {
    let x = b.iter().fold(0u8, |a, v| a ^ v);
    b.push(x);
    b
}

/// A TAP tape with the program, which runs from `autostart` when loaded (`LOAD ""`).
pub fn tap(program: &[u8], name: &str, autostart: u16) -> Vec<u8> {
    let mut title: Vec<u8> = name.chars().filter_map(|c| char_code(c).ok()).take(10).collect();
    title.resize(10, b' ');
    let mut header = vec![0x00, 0x00];
    header.extend(title);
    header.extend_from_slice(&(program.len() as u16).to_le_bytes());
    header.extend_from_slice(&autostart.to_le_bytes());
    header.extend_from_slice(&(program.len() as u16).to_le_bytes());
    let mut data = vec![0xff];
    data.extend_from_slice(program);
    let mut out = Vec::new();
    for b in [checksummed(header), checksummed(data)] {
        out.extend_from_slice(&(b.len() as u16).to_le_bytes());
        out.extend(b);
    }
    out
}

/// The program's first line number (where it starts when loaded).
pub fn first_line(program: &[u8]) -> u16 {
    if program.len() >= 2 { u16::from_be_bytes([program[0], program[1]]) } else { 10 }
}

/// The Spectrum's error reports, by code (ERR_NR + 1).
pub fn report(code: u8) -> &'static str {
    const R: [&str; 28] = [
        "0 OK", "1 NEXT without FOR", "2 Variable not found", "3 Subscript wrong", "4 Out of memory", "5 Out of screen", "6 Number too big",
        "7 RETURN without GOSUB", "8 End of file", "9 STOP statement", "A Invalid argument", "B Integer out of range", "C Nonsense in BASIC",
        "D BREAK - CONT repeats", "E Out of DATA", "F Invalid file name", "G No room for line", "H STOP in INPUT", "I FOR without NEXT",
        "J Invalid I/O device", "K Invalid colour", "L BREAK into program", "M RAMTOP no good", "N Statement lost", "O Invalid stream",
        "P FN without DEF", "Q Parameter error", "R Tape loading error",
    ];
    R.get(code as usize).copied().unwrap_or("Unknown error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(number_bytes(123.0), [0, 0, 123, 0, 0]);
        assert_eq!(number_bytes(30000.0), [0, 0, 0x30, 0x75, 0]);
        assert_eq!(number_bytes(0.5), [0x80, 0, 0, 0, 0]);
        assert_eq!(number_bytes(3.5), [0x82, 0x60, 0, 0, 0]);
        assert_eq!(number_bytes(-3.5), [0x82, 0xe0, 0, 0, 0]);
        assert_eq!(number_bytes(100000.0), [0x91, 0x43, 0x50, 0, 0]);
    }

    #[test]
    fn tokens() {
        let p = compile("10 POKE 30000,123: BORDER 2").unwrap();
        // Same bytes as a Spectrum stores when you type the line.
        let mut body = vec![0xf4];
        body.extend(b"30000");
        body.extend([0x0e, 0, 0, 0x30, 0x75, 0]);
        body.push(b',');
        body.extend(b"123");
        body.extend([0x0e, 0, 0, 123, 0, 0]);
        body.push(b':');
        body.push(0xe7);
        body.extend(b"2");
        body.extend([0x0e, 0, 0, 2, 0, 0]);
        body.push(0x0d);
        let mut want = vec![0, 10];
        want.extend((body.len() as u16).to_le_bytes());
        want.extend(body);
        assert_eq!(p, want);
    }

    #[test]
    fn words_strings_and_escapes() {
        let t = |s: &str| tokenize(s).unwrap();
        // goto/gosub without spaces; lowercase; a variable called total isn't TO + TAL.
        assert_eq!(t("goto 20")[0], 0xec);
        assert_eq!(t("GOSUB 20")[0], 0xed);
        assert_eq!(&t("LET total=1")[..6], &[0xf1, b't', b'o', b't', b'a', b'l']);
        assert_eq!(t("IF a<=b THEN STOP"), vec![0xfa, b'a', 0xc7, b'b', 0xcb, 0xe2]);
        // Nothing is tokenized in strings; escapes give UDGs and codes.
        assert_eq!(t("PRINT \"PRINT \\a\\{65}\"\"\""), vec![0xf5, b'"', b'P', b'R', b'I', b'N', b'T', b' ', 0x90, 65, b'"', b'"', b'"']);
        // REM keeps the text; digits in a name aren't numbers.
        assert_eq!(t("REM go to 10"), [vec![0xea], b"go to 10".to_vec()].concat());
        assert_eq!(t("LET a1=2"), [vec![0xf1], b"a1=2".to_vec(), vec![0x0e, 0, 0, 2, 0, 0]].concat());
        assert_eq!(t("PRINT INKEY$")[1], 0xa6);
        assert_eq!(t("PRINT CHR$ 65")[1], 0xc2);
        let bin = t("LET u=BIN 101");
        assert_eq!(&bin[bin.len() - 6..], &[0x0e, 0, 0, 5, 0, 0]);
    }

    #[test]
    fn errors_name_their_lines() {
        let e = compile("10 PRINT \"hi\n20 PRINT 1\n   \nPRINT 2\n20 CLS\n99999 CLS").unwrap_err();
        let lines: Vec<(usize, Option<u16>)> = e.iter().map(|e| (e.text_line, e.line)).collect();
        assert_eq!(lines, [(1, Some(10)), (4, None), (5, Some(20)), (6, None)], "{e:?}");
        assert!(e[0].message.contains("isn't closed"));
    }

    #[test]
    fn taps_load() {
        let p = compile("20 PRINT \"B\"\n10 PRINT \"A\"").unwrap();
        assert_eq!(first_line(&p), 10, "lines are put in order");
        let t = tap(&p, "My game!", 10);
        let blocks = crate::tzx::parse(&t).unwrap();
        assert!(crate::tzx::describe(&blocks).contains(&"Program: My game!".to_string()));
    }
}

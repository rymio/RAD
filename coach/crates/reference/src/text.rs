//! Sentence splitting for practice texts, and cleaning text extracted from PDF pages.

/// Common abbreviations that end in a full stop but do not end a sentence.
const ABBREVIATIONS: &[&str] = &["mr", "mrs", "ms", "dr", "st", "mt", "vs", "etc", "e.g", "i.e", "approx", "no"];

/// Split text into sentences, keeping their punctuation. Line breaks also end a
/// sentence, so a poem or list practises line by line.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if matches!(c, '.' | '!' | '?' | '…') {
                // Absorb runs like "?!" and closing quotes or brackets.
                let mut end = i + 1;
                while end < chars.len() && matches!(chars[end], '.' | '!' | '?' | '"' | '\'' | '”' | '’' | ')') {
                    end += 1;
                }
                let at_boundary = end >= chars.len() || chars[end].is_whitespace();
                let word_before: String = chars[start..i]
                    .iter()
                    .collect::<String>()
                    .split_whitespace()
                    .last()
                    .unwrap_or("")
                    .to_lowercase();
                let is_abbrev = c == '.' && ABBREVIATIONS.contains(&word_before.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '.'));
                if at_boundary && !is_abbrev {
                    push(&mut out, &chars[start..end]);
                    start = end;
                }
                i = end;
            } else {
                i += 1;
            }
        }
        push(&mut out, &chars[start..]);
    }
    out
}

/// Estimated syllable count of English text: vowel groups per word, with a silent final
/// "e" dropped (but not "-le"), at least one per word. Good to about ±10% on read text,
/// which is all the pace display needs.
pub fn count_syllables(text: &str) -> usize {
    text.split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter_map(|w| {
            let raw = w.to_lowercase();
            let w: String = raw.chars().filter(|c| c.is_alphabetic()).collect();
            if w.is_empty() {
                return None;
            }
            let chars: Vec<char> = w.chars().collect();
            let is_vowel = |c: char| matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y');
            let mut groups = 0;
            let mut prev = false;
            for &c in &chars {
                let v = is_vowel(c);
                if v && !prev {
                    groups += 1;
                }
                prev = v;
            }
            let n = chars.len();
            if n > 2 && chars[n - 1] == 'e' && !is_vowel(chars[n - 2]) && !(chars[n - 2] == 'l' && !is_vowel(chars[n - 3])) {
                groups -= 1;
            }
            // Vowel + "ing" is two syllables (go-ing, be-ing); g'day and it'll gain one too.
            if n > 3 && w.ends_with("ing") && is_vowel(chars[n - 4]) {
                groups += 1;
            }
            if raw.starts_with("g'") || (raw.ends_with("'ll") && n > 2 && !is_vowel(chars[n - 3])) {
                groups += 1;
            }
            Some(groups.max(1))
        })
        .sum()
}

fn push(out: &mut Vec<String>, chars: &[char]) {
    let s: String = chars.iter().collect();
    let s = s.trim();
    if s.chars().any(char::is_alphanumeric) {
        out.push(s.to_string());
    }
}

/// Turn the raw text of one PDF page into readable prose.
///
/// PDFs break lines at the page edge, hyphenate words across lines and carry page numbers.
/// Lines are joined back into paragraphs (mending "beauti-\nful"), page numbers are dropped,
/// and short unpunctuated lines (headings) are kept on their own line so they do not run
/// into the next sentence.
pub fn clean_pdf_page(raw: &str) -> String {
    let normalised: String = raw
        .replace(['\r', '\u{000C}'], "\n")
        .replace('\u{00A0}', " ")
        .replace('\u{00AD}', "")
        .replace('ﬁ', "fi")
        .replace('ﬂ', "fl")
        .replace('ﬀ', "ff")
        .replace('ﬃ', "ffi")
        .replace('ﬄ', "ffl");
    let lines: Vec<String> = normalised
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !is_page_number(l))
        .collect();
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    let mut out: Vec<String> = Vec::new();
    let mut para = String::new();
    let flush = |para: &mut String, out: &mut Vec<String>| {
        if !para.is_empty() {
            out.push(std::mem::take(para));
        }
    };
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            flush(&mut para, &mut out);
            continue;
        }
        let next = lines.get(i + 1).map(String::as_str).unwrap_or("");
        let ends_sentence = line.ends_with(['.', '!', '?', ':', ';', '"', '”', '’', ')']);
        let starts_upper = line.chars().next().is_some_and(char::is_uppercase);
        let next_upper = next.chars().next().is_some_and(char::is_uppercase);
        let heading = para.is_empty()
            && starts_upper
            && !ends_sentence
            && !line.ends_with([',', '-'])
            && (next.is_empty() || next_upper)
            && line.chars().count() * 10 < longest * 6;
        if heading {
            out.push(line.clone());
            continue;
        }
        if para.ends_with('-') && line.chars().next().is_some_and(char::is_lowercase) {
            para.pop(); // "beauti-" + "ful"
        } else if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(line);
    }
    flush(&mut para, &mut out);
    out.join("\n")
}

/// "12", "- 12 -", "Page 12", "12 of 40", "xii".
fn is_page_number(line: &str) -> bool {
    let l = line.trim().trim_matches(|c: char| c == '-' || c == '–' || c.is_whitespace()).to_lowercase();
    let l = l.strip_prefix("page ").unwrap_or(&l);
    let number = |p: &str| !p.is_empty() && (p.chars().all(|c| c.is_ascii_digit()) || is_roman(p));
    l.split(" of ").all(number)
}

/// A well-formed lower-case Roman numeral up to 399 ("xii" yes, "civic" no).
fn is_roman(s: &str) -> bool {
    const UNITS: [&str; 10] = ["", "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix"];
    const TENS: [&str; 10] = ["", "x", "xx", "xxx", "xl", "l", "lx", "lxx", "lxxx", "xc"];
    const HUNDREDS: [&str; 4] = ["", "c", "cc", "ccc"];
    (1..400).any(|n| format!("{}{}{}", HUNDREDS[n / 100], TENS[n / 10 % 10], UNITS[n % 10]) == s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_pdf_line_breaks_hyphens_and_page_numbers() {
        let raw = "Chapter One\nThe beach at Cottesloe was beauti-\nful that morning, and the water was\ncalm. We swam until lunch.\n\n12\nThen we walked home.";
        assert_eq!(
            clean_pdf_page(raw),
            "Chapter One\nThe beach at Cottesloe was beautiful that morning, and the water was calm. We swam until lunch.\nThen we walked home."
        );
        assert_eq!(
            split_sentences(&clean_pdf_page(raw)),
            vec![
                "Chapter One",
                "The beach at Cottesloe was beautiful that morning, and the water was calm.",
                "We swam until lunch.",
                "Then we walked home."
            ]
        );
    }

    #[test]
    fn recognises_page_numbers() {
        for p in ["12", "- 12 -", "Page 3", "3 of 40", "xii"] {
            assert!(is_page_number(p), "{p}");
        }
        for p in ["12 Main Street", "Mix it up", "Civic", ""] {
            assert!(!is_page_number(p), "{p}");
        }
    }

    #[test]
    fn splits_simple_text() {
        let s = split_sentences("G'day mate! How's it going? Not bad, thanks.");
        assert_eq!(s, vec!["G'day mate!", "How's it going?", "Not bad, thanks."]);
    }

    #[test]
    fn keeps_abbreviations_and_quotes() {
        let s = split_sentences("Dr. Smith said \"No worries.\" Then he left.");
        assert_eq!(s, vec!["Dr. Smith said \"No worries.\"", "Then he left."]);
    }

    #[test]
    fn line_breaks_split_and_blank_lines_vanish() {
        let s = split_sentences("First line\n\nSecond line without stop\n");
        assert_eq!(s, vec!["First line", "Second line without stop"]);
    }

    #[test]
    fn syllable_estimates() {
        assert_eq!(count_syllables("G'day mate, how's it going?"), 7);
        assert_eq!(count_syllables("Reckon it'll be a scorcher today."), 10);
        assert_eq!(count_syllables("little table"), 4);
        assert_eq!(count_syllables("the"), 1);
    }

    #[test]
    fn decimals_do_not_split() {
        assert_eq!(split_sentences("It costs 3.50 today."), vec!["It costs 3.50 today."]);
    }
}

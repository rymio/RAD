//! Pronunciation lexicon: CMUdict (first pronunciation of each word) plus Australian
//! additions. Used only to know *which* vowel a word contains, by Wells lexical set;
//! CMUdict's American pronunciations are never used as a target.

use std::collections::HashMap;
use std::io::Read;
use std::sync::OnceLock;

static CMUDICT_GZ: &[u8] = include_bytes!("../data/cmudict.dict.gz");
static EXTRA: &str = include_str!("../data/aussie-extra.dict");

pub struct Lexicon {
    /// word → ARPAbet phones separated by spaces.
    map: HashMap<String, String>,
}

fn parse_into(text: &str, map: &mut HashMap<String, String>, overwrite: bool) {
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with(";;") {
            continue;
        }
        let Some((word, phones)) = line.split_once(char::is_whitespace) else { continue };
        if word.contains('(') {
            continue;
        }
        if overwrite || !map.contains_key(word) {
            map.insert(word.to_string(), phones.trim().to_string());
        }
    }
}

impl Lexicon {
    pub fn from_text(cmudict: &str, extra: &str) -> Self {
        let mut map = HashMap::with_capacity(130_000);
        parse_into(cmudict, &mut map, false);
        parse_into(extra, &mut map, true);
        Self { map }
    }

    /// The bundled lexicon, decompressed on first use (~100 ms).
    pub fn global() -> &'static Lexicon {
        static LEX: OnceLock<Lexicon> = OnceLock::new();
        LEX.get_or_init(|| {
            let mut text = String::new();
            flate2::read::GzDecoder::new(CMUDICT_GZ).read_to_string(&mut text).expect("bundled cmudict is valid gzip");
            Lexicon::from_text(&text, EXTRA)
        })
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// ARPAbet phones for a word as written in a sentence ("Mate," → M EY1 T).
    pub fn lookup(&self, word: &str) -> Option<Vec<String>> {
        let key = normalise(word);
        if key.is_empty() {
            return None;
        }
        let split = |s: &String| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        if let Some(p) = self.map.get(&key) {
            return Some(split(p));
        }
        // Possessive or contracted "is": "Kate's", "mate's".
        if let Some(base) = key.strip_suffix("'s") {
            if let Some(p) = self.map.get(base) {
                let mut phones = split(p);
                let last = phones.last().map(String::as_str).unwrap_or("");
                phones.push(if matches!(last, "P" | "T" | "K" | "F" | "TH") { "S".into() } else { "Z".into() });
                return Some(phones);
            }
        }
        None
    }
}

/// Lower case, curly apostrophes made straight, punctuation trimmed from the ends.
pub fn normalise(word: &str) -> String {
    let w: String = word.chars().map(|c| if c == '\u{2019}' || c == '\u{2018}' { '\'' } else { c }).collect();
    w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_lexicon_loads() {
        let lex = Lexicon::global();
        assert!(lex.len() > 120_000, "{}", lex.len());
        assert_eq!(lex.lookup("Mate,").unwrap(), vec!["M", "EY1", "T"]);
        assert_eq!(lex.lookup("\u{201C}G\u{2019}day").unwrap()[3], "EY1");
        assert_eq!(lex.lookup("arvo").unwrap()[0], "AA1");
        assert!(lex.lookup("zzqx").is_none());
    }

    #[test]
    fn possessives_fall_back_to_the_base_word() {
        let lex = Lexicon::from_text("BOB B AA1 B\nbob B AA1 B\nkate K EY1 T\n", "");
        assert_eq!(lex.lookup("Kate's").unwrap(), vec!["K", "EY1", "T", "S"]);
        assert_eq!(lex.lookup("bob's").unwrap(), vec!["B", "AA1", "B", "Z"]);
    }
}

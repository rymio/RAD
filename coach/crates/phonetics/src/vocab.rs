//! The phoneme model's vocabulary (`vocab.json` from the Hugging Face tokenizer).

use std::collections::HashMap;
use std::path::Path;

use crate::PhoneticsError;

#[derive(Debug, Clone)]
pub struct Vocab {
    id_to_token: Vec<String>,
    pub blank: usize,
    /// Token ids that are not phones: sentence markers, unknown, word delimiters.
    pub ignore: Vec<usize>,
}

const SPECIAL: &[&str] = &["<s>", "</s>", "<unk>", "|", " "];

impl Vocab {
    /// Parse a `{"token": id, …}` map. The CTC blank is `<pad>`.
    pub fn from_json(json: &str) -> Result<Self, PhoneticsError> {
        let map: HashMap<String, usize> =
            serde_json::from_str(json).map_err(|e| PhoneticsError::Vocab(format!("vocab.json: {e}")))?;
        let size = map.values().copied().max().map(|m| m + 1).unwrap_or(0);
        let mut id_to_token = vec![String::new(); size];
        for (tok, id) in &map {
            id_to_token[*id] = tok.clone();
        }
        let blank = *map.get("<pad>").ok_or_else(|| PhoneticsError::Vocab("vocab.json has no <pad> (CTC blank)".into()))?;
        let ignore = SPECIAL.iter().filter_map(|t| map.get(*t).copied()).collect();
        Ok(Self { id_to_token, blank, ignore })
    }

    pub fn load(path: &Path) -> Result<Self, PhoneticsError> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }

    pub fn len(&self) -> usize {
        self.id_to_token.len()
    }

    pub fn is_empty(&self) -> bool {
        self.id_to_token.is_empty()
    }

    pub fn token(&self, id: usize) -> &str {
        self.id_to_token.get(id).map(String::as_str).unwrap_or("?")
    }

    /// Phone labels as IPA text, e.g. `ɡ ə d æɪ`.
    pub fn render(&self, ids: &[usize]) -> String {
        ids.iter().map(|&i| self.token(i)).collect::<Vec<_>>().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blank_and_specials() {
        let v = Vocab::from_json(r#"{"<pad>":0,"<s>":1,"</s>":2,"<unk>":3,"d":4,"eɪ":5}"#).unwrap();
        assert_eq!(v.blank, 0);
        assert_eq!(v.ignore, vec![1, 2, 3]);
        assert_eq!(v.render(&[4, 5]), "d eɪ");
        assert_eq!(v.len(), 6);
    }

    #[test]
    fn missing_blank_is_an_error() {
        assert!(Vocab::from_json(r#"{"a":0}"#).is_err());
    }
}

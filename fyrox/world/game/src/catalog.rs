//! What can be built in the world: each kind's picture, its size (tiles) and the part of it you
//! can't walk through. Gilgamesh asks for things in words ("a date palm", "a little library"),
//! so names are matched loosely; anything unknown becomes a tent with its name.

use crate::art::{self, Img};

#[derive(Clone, Copy, Debug)]
pub struct Kind {
    pub id: &'static str,
    pub title: &'static str,
    /// Drawn size in tiles.
    pub w: f32,
    pub h: f32,
    /// Solid footprint (width, depth) at the bottom, in tiles; zero means you can walk over it.
    pub solid_w: f32,
    pub solid_d: f32,
    pub draw: fn() -> Img,
    /// Lies flat on the ground (drawn under everything that stands).
    pub flat: bool,
    pub words: &'static [&'static str],
}

const fn k(id: &'static str, title: &'static str, w: f32, h: f32, solid_w: f32, solid_d: f32, draw: fn() -> Img, flat: bool, words: &'static [&'static str]) -> Kind {
    Kind { id, title, w, h, solid_w, solid_d, draw, flat, words }
}

pub const KINDS: &[Kind] = &[
    k("palm", "Date palm", 2.0, 3.0, 0.5, 0.4, art::palm, false, &["palm", "date", "dates", "tree"]),
    k("cedar", "Cedar", 2.0, 3.0, 0.6, 0.5, art::cedar, false, &["cedar", "pine", "fir", "forest", "conifer"]),
    k("house", "House", 4.0, 3.5, 3.5, 1.6, art::house, false, &["house", "home", "hut", "dwelling", "cottage"]),
    k("ziggurat", "Ziggurat", 6.0, 4.5, 5.6, 2.6, art::ziggurat, false, &["ziggurat", "temple", "pyramid", "shrine"]),
    k("well", "Well", 1.5, 1.6, 1.3, 0.8, art::well, false, &["well", "water"]),
    k("garden", "Garden", 2.0, 1.5, 0.0, 0.0, art::garden, true, &["garden", "vegetable", "herb", "plants", "plot"]),
    k("flowers", "Flowers", 1.0, 1.0, 0.0, 0.0, art::flowers, true, &["flower", "flowers", "blossom", "roses"]),
    k("fire", "Campfire", 1.0, 1.25, 0.8, 0.4, art::fire, false, &["fire", "campfire", "hearth", "bonfire"]),
    k("tower", "Watchtower", 2.0, 4.0, 1.3, 0.9, art::tower, false, &["tower", "watchtower", "lookout", "observatory"]),
    k("statue", "Guardian statue", 1.5, 2.5, 1.2, 0.6, art::statue, false, &["statue", "lamassu", "guardian", "monument", "sculpture"]),
    k("tablet", "Clay tablet", 1.25, 1.6, 0.8, 0.3, art::tablet, false, &["tablet", "tablets", "board", "notice", "tasks", "sign", "notes"]),
    k("bench", "Bench", 1.5, 0.75, 1.4, 0.3, art::bench, false, &["bench", "seat", "chair"]),
    k("lamp", "Lamp", 0.6, 1.75, 0.3, 0.2, art::lamp, false, &["lamp", "lantern", "light", "torch"]),
    k("workshop", "Workshop", 3.0, 2.5, 2.7, 1.2, art::workshop, false, &["workshop", "forge", "smithy", "studio", "lab", "laboratory", "workbench"]),
    k("boat", "Reed boat", 2.5, 1.25, 2.2, 0.5, art::boat, false, &["boat", "ship", "raft", "canoe"]),
    k("fountain", "Fountain", 2.0, 1.75, 1.8, 0.8, art::fountain, false, &["fountain", "pool", "pond", "bath"]),
    k("wall", "Wall", 2.0, 1.25, 2.0, 0.5, art::wall, false, &["wall", "fence", "rampart"]),
    k("gate", "Gate", 2.5, 2.75, 2.5, 0.6, art::gate, false, &["gate", "arch", "gateway", "door", "ishtar"]),
    k("field", "Barley field", 2.0, 1.5, 0.0, 0.0, art::field, true, &["field", "barley", "wheat", "farm", "crops", "grain"]),
    k("tent", "Tent", 2.5, 2.0, 2.2, 0.8, art::tent, false, &["tent", "pavilion", "library", "school", "market", "stall", "shop", "hall", "museum"]),
    k("reeds", "Reeds", 0.75, 1.0, 0.0, 0.0, art::reeds, false, &["reed", "reeds", "rushes"]),
    k("rock", "Rock", 1.0, 0.75, 0.8, 0.4, art::rock, false, &["rock", "stone", "boulder"]),
];

pub fn by_id(id: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.id == id)
}

/// The kind for a request in words; the fallback is a tent (with the request as its name).
pub fn find(words: &str) -> (&'static Kind, bool) {
    let w = words.to_lowercase();
    let tokens: Vec<&str> = w.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    if let Some(k) = KINDS.iter().find(|k| k.id == w || k.title.to_lowercase() == w) {
        return (k, true);
    }
    // The last matching word wins ("a stone bench" is a bench, not a rock).
    let mut best: Option<(usize, &Kind)> = None;
    for (pos, t) in tokens.iter().enumerate() {
        let singular = t.strip_suffix('s').unwrap_or(t);
        if let Some(k) = KINDS.iter().find(|k| k.words.contains(t) || k.words.contains(&singular)) {
            if best.is_none_or(|(p, _)| pos >= p) {
                best = Some((pos, k));
            }
        }
    }
    match best {
        Some((_, k)) => (k, true),
        None => (by_id("tent").unwrap(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_kinds() {
        assert_eq!(find("palm").0.id, "palm");
        assert_eq!(find("a date palm by the river").0.id, "palm");
        assert_eq!(find("a stone bench").0.id, "bench");
        assert_eq!(find("Watchtower").0.id, "tower");
        assert_eq!(find("small vegetable garden").0.id, "garden");
        assert_eq!(find("library").0.id, "tent");
        let (k, known) = find("observatory of dreams");
        assert!(known && k.id == "tower");
        let (k, known) = find("zxqv");
        assert!(!known && k.id == "tent");
        for k in KINDS {
            let img = (k.draw)();
            assert!(img.w > 0, "{}", k.id);
        }
    }
}

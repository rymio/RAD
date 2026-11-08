//! Prints vowel-chart JSON for two synthetic speakers (an Australian reference and an
//! American-like learner) reading "G'day mate, how's it going?". Used for UI screenshots
//! and to eyeball the engine; formant targets are approximate textbook values.

mod synth;

use vowels::measure::{f3_low, measure};
use vowels::{clouds, compare, shared_norms, Set, Token, VowelSpace};

type Tri = (f32, f32, f32);

fn aus(set: Set) -> [Tri; 2] {
    let m = |a: Tri| [a, a];
    match set {
        Set::Fleece => m((290.0, 2250.0, 2900.0)),
        Set::Kit => m((370.0, 2150.0, 2800.0)),
        Set::Dress => m((470.0, 2000.0, 2700.0)),
        Set::Trap => m((720.0, 1650.0, 2600.0)),
        Set::Start => m((800.0, 1350.0, 2500.0)),
        Set::Lot => m((620.0, 1000.0, 2500.0)),
        Set::Strut => m((760.0, 1400.0, 2550.0)),
        Set::Thought => m((440.0, 850.0, 2500.0)),
        Set::Foot => m((400.0, 1150.0, 2350.0)),
        Set::Nurse => m((480.0, 1600.0, 2450.0)),
        Set::Goose => m((330.0, 1700.0, 2300.0)),
        Set::Face => [(650.0, 1750.0, 2550.0), (420.0, 2150.0, 2750.0)],
        Set::Price => [(780.0, 1150.0, 2500.0), (470.0, 1900.0, 2600.0)],
        Set::Goat => [(600.0, 1300.0, 2500.0), (430.0, 1550.0, 2400.0)],
        Set::Mouth => [(720.0, 1700.0, 2550.0), (560.0, 1150.0, 2450.0)],
        _ => m((500.0, 1500.0, 2500.0)),
    }
}

fn american(set: Set) -> [Tri; 2] {
    let m = |a: Tri| [a, a];
    match set {
        Set::Fleece => m((280.0, 2300.0, 3000.0)),
        Set::Kit => m((400.0, 2000.0, 2650.0)),
        Set::Dress => m((580.0, 1800.0, 2600.0)),
        Set::Trap => m((700.0, 1750.0, 2550.0)),
        Set::Start => [(780.0, 1250.0, 2400.0), (560.0, 1300.0, 1650.0)],
        Set::Lot => m((760.0, 1150.0, 2550.0)),
        Set::Strut => m((640.0, 1250.0, 2550.0)),
        Set::Thought => m((600.0, 950.0, 2450.0)),
        Set::Foot => m((450.0, 1100.0, 2350.0)),
        Set::Nurse => m((480.0, 1350.0, 1700.0)),
        Set::Goose => m((310.0, 1050.0, 2250.0)),
        Set::Face => [(480.0, 1950.0, 2600.0), (360.0, 2250.0, 2800.0)],
        Set::Price => [(760.0, 1350.0, 2500.0), (450.0, 2000.0, 2600.0)],
        Set::Goat => [(480.0, 1000.0, 2450.0), (380.0, 850.0, 2350.0)],
        Set::Mouth => [(720.0, 1400.0, 2500.0), (460.0, 1050.0, 2400.0)],
        _ => m((500.0, 1500.0, 2500.0)),
    }
}

const SETS: [Set; 15] = [
    Set::Fleece, Set::Kit, Set::Dress, Set::Trap, Set::Start, Set::Lot, Set::Strut, Set::Thought, Set::Foot,
    Set::Nurse, Set::Goose, Set::Face, Set::Price, Set::Goat, Set::Mouth,
];

fn token(table: fn(Set) -> [Tri; 2], set: Set, scale: f32, f0: f32, jitter: f32, index: usize, word: &str) -> Token {
    let [a, b] = table(set);
    let s = |t: Tri| (t.0 * scale * (1.0 + jitter), t.1 * scale * (1.0 - jitter), t.2 * scale);
    let audio = synth::vowel(f0, [s(a), s(b)], 0.28, 0.5);
    let r = set == Set::Start || set == Set::Nurse;
    Token {
        word_index: index,
        word: word.into(),
        set,
        measure: measure(&audio, (0.0, 0.28)).expect("measurable"),
        r_context: r,
        f3_r: r.then(|| f3_low(&audio, (0.0, 0.28), 0.6).unwrap()),
    }
}

fn space(table: fn(Set) -> [Tri; 2], scale: f32, f0: f32) -> VowelSpace {
    let mut toks = Vec::new();
    for (i, &set) in SETS.iter().enumerate() {
        for (k, j) in [-0.04f32, 0.0, 0.05].iter().enumerate() {
            toks.push(token(table, set, scale, f0, *j, 100 + i * 3 + k, set.keyword()));
        }
    }
    VowelSpace::new(toks)
}

fn main() {
    let ref_space = space(aus, 1.0, 115.0);
    let you_space = space(american, 1.12, 180.0);
    let norms = shared_norms(&ref_space, &you_space).expect("shared vowels");
    let words = [(0, "G'day", Set::Face), (1, "mate", Set::Face), (2, "how's", Set::Mouth), (4, "going", Set::Goat)];
    let ref_toks: Vec<Token> = words.iter().map(|&(i, w, s)| token(aus, s, 1.0, 115.0, 0.02, i, w)).collect();
    let you_toks: Vec<Token> = words.iter().map(|&(i, w, s)| token(american, s, 1.12, 180.0, 0.0, i, w)).collect();
    let cmp = compare(&ref_toks, &you_toks, &norms.reference, &norms.learner);
    let chart_words: Vec<serde_json::Value> = cmp
        .vowels
        .iter()
        .map(|c| {
            serde_json::json!({
                "word_index": c.word_index, "word": c.word, "set": c.set, "keyword": c.keyword, "target": c.target,
                "reference": c.reference, "you": c.you, "grade": c.grade, "hint": c.hint,
            })
        })
        .collect();
    let out = serde_json::json!({
        "status": { "state": "ready" },
        "reference": clouds(&ref_space, &norms.reference),
        "yours": clouds(&you_space, &norms.learner),
        "words": chart_words,
        "rhotic": [],
        "analysed": 5,
        "sentences": 5,
        "vowels": cmp.vowels,
    });
    println!("{}", serde_json::to_string(&out).unwrap());
}

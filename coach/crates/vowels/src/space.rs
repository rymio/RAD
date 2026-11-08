//! A speaker's vowel space and Lobanov normalisation over vowel categories both speakers share.
//!
//! Each speaker is normalised by the mean and spread of their *category means* (not raw
//! tokens), computed over the same set of stable monophthongs for both, so the learner and
//! the reference are scaled on equal terms whatever the text happens to contain.

use std::collections::{BTreeMap, BTreeSet};

use analysis::stats;
use serde::{Deserialize, Serialize};

use crate::extract::Token;
use crate::measure::Point;
use crate::sets::Set;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VowelSpace {
    pub tokens: Vec<Token>,
}

impl VowelSpace {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens }
    }

    pub fn add(&mut self, tokens: impl IntoIterator<Item = Token>) {
        self.tokens.extend(tokens);
    }

    pub fn sets(&self) -> BTreeSet<Set> {
        self.tokens.iter().map(|t| t.set).collect()
    }

    pub fn count(&self, set: Set) -> usize {
        self.tokens.iter().filter(|t| t.set == set).count()
    }

    /// Mean mid-vowel formants of a category.
    pub fn mean_mid(&self, set: Set) -> Option<Point> {
        let pts: Vec<Point> = self.tokens.iter().filter(|t| t.set == set).map(|t| t.measure.mid).collect();
        if pts.is_empty() {
            return None;
        }
        let n = pts.len() as f32;
        Some(Point {
            f1: pts.iter().map(|p| p.f1).sum::<f32>() / n,
            f2: pts.iter().map(|p| p.f2).sum::<f32>() / n,
            f3: pts.iter().map(|p| p.f3).sum::<f32>() / n,
        })
    }

    /// The speaker's ordinary F3 level: median over vowels with no droppable R nearby.
    pub fn f3_baseline(&self) -> Option<f32> {
        let f3: Vec<f32> = self.tokens.iter().filter(|t| !t.r_context).map(|t| t.measure.mid.f3).collect();
        if f3.len() < 3 {
            return None;
        }
        stats::median(&f3)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Norm {
    pub f1_mean: f32,
    pub f1_sd: f32,
    pub f2_mean: f32,
    pub f2_sd: f32,
    pub f3_baseline: Option<f32>,
}

impl Norm {
    /// Lobanov z-scores `[z1, z2]`: z1 up = more open, z2 up = further forward.
    pub fn z(&self, p: Point) -> [f32; 2] {
        [(p.f1 - self.f1_mean) / self.f1_sd, (p.f2 - self.f2_mean) / self.f2_sd]
    }

    fn over(space: &VowelSpace, sets: &[Set]) -> Option<Norm> {
        let means: Vec<Point> = sets.iter().filter_map(|s| space.mean_mid(*s)).collect();
        if means.len() < 2 {
            return None;
        }
        let pop_sd = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            (m, (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt().max(30.0))
        };
        let (f1_mean, f1_sd) = pop_sd(&means.iter().map(|p| p.f1).collect::<Vec<_>>());
        let (f2_mean, f2_sd) = pop_sd(&means.iter().map(|p| p.f2).collect::<Vec<_>>());
        Some(Norm { f1_mean, f1_sd, f2_mean, f2_sd, f3_baseline: space.f3_baseline() })
    }
}

/// The three corners of the vowel space a fair normalisation needs.
pub const CORNERS: [(&str, &[Set]); 3] = [
    ("a close front vowel (see, sit)", &[Set::Fleece, Set::Kit]),
    ("an open vowel (cat, car, cup)", &[Set::Trap, Set::Start, Set::Strut]),
    ("a back vowel (hot, saw, put)", &[Set::Lot, Set::Thought, Set::Foot]),
];

pub const MIN_SHARED: usize = 4;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Shortfall {
    /// Normalising categories both speakers have.
    pub shared: Vec<Set>,
    /// Corners of the vowel space not covered by the shared categories.
    pub missing_corners: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SharedNorms {
    pub reference: Norm,
    pub learner: Norm,
    pub sets: Vec<Set>,
}

/// Normalise both speakers over the stable monophthongs they share.
pub fn shared_norms(reference: &VowelSpace, learner: &VowelSpace) -> Result<SharedNorms, Shortfall> {
    let rs = reference.sets();
    let ls = learner.sets();
    let shared: Vec<Set> = rs.intersection(&ls).copied().filter(|s| s.normalises()).collect();
    let missing_corners: Vec<&'static str> =
        CORNERS.iter().filter(|(_, sets)| !sets.iter().any(|s| shared.contains(s))).map(|(name, _)| *name).collect();
    if shared.len() < MIN_SHARED || !missing_corners.is_empty() {
        return Err(Shortfall { shared, missing_corners });
    }
    match (Norm::over(reference, &shared), Norm::over(learner, &shared)) {
        (Some(r), Some(l)) => Ok(SharedNorms { reference: r, learner: l, sets: shared }),
        _ => Err(Shortfall { shared, missing_corners }),
    }
}

/// Category cloud for the vowel chart, in normalised units.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cloud {
    pub set: Set,
    pub keyword: &'static str,
    pub centre: [f32; 2],
    /// Standard deviation along each axis (at least 0.2).
    pub spread: [f32; 2],
    pub onset: Option<[f32; 2]>,
    pub offset: Option<[f32; 2]>,
    pub count: usize,
}

pub fn clouds(space: &VowelSpace, norm: &Norm) -> Vec<Cloud> {
    let mut by_set: BTreeMap<Set, Vec<&Token>> = BTreeMap::new();
    for t in &space.tokens {
        by_set.entry(t.set).or_default().push(t);
    }
    by_set
        .into_iter()
        .filter(|(s, _)| *s != Set::Schwa)
        .map(|(set, toks)| {
            let mean = |f: &dyn Fn(&Token) -> [f32; 2]| -> ([f32; 2], [f32; 2]) {
                let pts: Vec<[f32; 2]> = toks.iter().map(|t| f(t)).collect();
                let n = pts.len() as f32;
                let m = [pts.iter().map(|p| p[0]).sum::<f32>() / n, pts.iter().map(|p| p[1]).sum::<f32>() / n];
                let sd = |k: usize| (pts.iter().map(|p| (p[k] - m[k]).powi(2)).sum::<f32>() / n).sqrt().max(0.2);
                (m, [sd(0), sd(1)])
            };
            let (centre, spread) = mean(&|t| norm.z(t.measure.mid));
            let (onset, offset) = if set.is_diphthong() {
                (Some(mean(&|t| norm.z(t.measure.onset)).0), Some(mean(&|t| norm.z(t.measure.offset)).0))
            } else {
                (None, None)
            };
            Cloud { set, keyword: set.keyword(), centre, spread, onset, offset, count: toks.len() }
        })
        .collect()
}

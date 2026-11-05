//! Finding a way across the world (A* on the tile grid), around buildings and the river.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// A rectangle in world units (x0, y0) – (x1, y1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Area {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Area {
        Area { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
    pub fn overlaps(&self, o: &Area) -> bool {
        self.x0 < o.x1 && self.x1 > o.x0 && self.y0 < o.y1 && self.y1 > o.y0
    }
    pub fn grow(&self, by: f32) -> Area {
        Area::new(self.x0 - by, self.y0 - by, self.x1 + by, self.y1 + by)
    }
}

/// What blocks walking: solid things and water (except where a bridge crosses it).
#[derive(Default, Clone, Debug)]
pub struct Blocks {
    pub w: f32,
    pub h: f32,
    pub solid: Vec<Area>,
    pub water: Vec<Area>,
    pub bridges: Vec<Area>,
    /// Walkable, but not to be built on: doorways and paths.
    pub keep_clear: Vec<Area>,
}

impl Blocks {
    pub fn blocked(&self, x: f32, y: f32) -> bool {
        if x < 0.3 || y < 0.3 || x > self.w - 0.3 || y > self.h - 0.3 {
            return true;
        }
        if self.solid.iter().any(|a| a.contains(x, y)) {
            return true;
        }
        self.water.iter().any(|a| a.contains(x, y)) && !self.bridges.iter().any(|b| b.contains(x, y))
    }

    /// A character's feet (a small box) at (x, y) are free.
    pub fn free(&self, x: f32, y: f32) -> bool {
        [(-0.25, 0.0), (0.25, 0.0), (-0.25, 0.25), (0.25, 0.25), (0.0, 0.12)].iter().all(|(dx, dy)| !self.blocked(x + dx, y + dy))
    }

    /// An area is free to build on (no solid thing, no water, inside the world).
    pub fn can_build(&self, a: &Area) -> bool {
        a.x0 >= 0.5 && a.y0 >= 0.5 && a.x1 <= self.w - 0.5 && a.y1 <= self.h - 0.5 && !self.solid.iter().any(|s| s.overlaps(&a.grow(0.15))) && !self.water.iter().any(|w| w.overlaps(&a.grow(0.3))) && !self.keep_clear.iter().any(|k| k.overlaps(a))
    }

    /// The nearest place for `size` (w, d) near (x, y), searching outwards.
    pub fn spot_near(&self, x: f32, y: f32, w: f32, d: f32, avoid: &[Area]) -> Option<(f32, f32)> {
        for ring in 0..40 {
            let r = ring as f32 * 0.5;
            let steps = (ring * 8).max(1);
            for s in 0..steps {
                let a = s as f32 / steps as f32 * std::f32::consts::TAU;
                let (cx, cy) = ((x + a.cos() * r).round(), (y + a.sin() * r).round());
                let area = Area::new(cx - w / 2.0, cy, cx + w / 2.0, cy + d.max(0.5));
                if self.can_build(&area) && !avoid.iter().any(|v| v.overlaps(&area)) {
                    return Some((cx, cy));
                }
            }
        }
        None
    }

    /// A walkable path of points from `from` to `to` (each a cell centre), or None.
    pub fn path(&self, from: (f32, f32), to: (f32, f32)) -> Option<Vec<(f32, f32)>> {
        let (gw, gh) = (self.w as i32, self.h as i32);
        let cell = |p: (f32, f32)| ((p.0.floor() as i32).clamp(0, gw - 1), (p.1.floor() as i32).clamp(0, gh - 1));
        let free = |c: (i32, i32)| self.free(c.0 as f32 + 0.5, c.1 as f32 + 0.3);
        let start = cell(from);
        let mut goal = cell(to);
        if !free(goal) {
            // The nearest free cell to where it wants to be.
            goal = (0..6).flat_map(|r| (-r..=r).flat_map(move |dx| (-r..=r).map(move |dy| (goal.0 + dx, goal.1 + dy)))).find(|c| c.0 >= 0 && c.1 >= 0 && c.0 < gw && c.1 < gh && free(*c))?;
        }
        let idx = |c: (i32, i32)| (c.1 * gw + c.0) as usize;
        let mut g = vec![f32::INFINITY; (gw * gh) as usize];
        let mut came = vec![usize::MAX; (gw * gh) as usize];
        let h = |c: (i32, i32)| (((c.0 - goal.0).pow(2) + (c.1 - goal.1).pow(2)) as f32).sqrt();
        let mut open = BinaryHeap::new();
        g[idx(start)] = 0.0;
        open.push(Node { f: h(start), c: start });
        let mut seen = 0;
        while let Some(Node { c, .. }) = open.pop() {
            seen += 1;
            if c == goal || seen > 6000 {
                break;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let n = (c.0 + dx, c.1 + dy);
                if n.0 < 0 || n.1 < 0 || n.0 >= gw || n.1 >= gh || !free(n) {
                    continue;
                }
                // No cutting corners past something solid.
                if dx != 0 && dy != 0 && (!free((c.0 + dx, c.1)) || !free((c.0, c.1 + dy))) {
                    continue;
                }
                let cost = g[idx(c)] + if dx != 0 && dy != 0 { 1.414 } else { 1.0 };
                if cost < g[idx(n)] {
                    g[idx(n)] = cost;
                    came[idx(n)] = idx(c);
                    open.push(Node { f: cost + h(n), c: n });
                }
            }
        }
        if !g[idx(goal)].is_finite() {
            return None;
        }
        let mut out = Vec::new();
        let mut i = idx(goal);
        while i != idx(start) && i != usize::MAX {
            out.push(((i as i32 % gw) as f32 + 0.5, (i as i32 / gw) as f32 + 0.3));
            i = came[i];
        }
        out.reverse();
        Some(out)
    }
}

#[derive(PartialEq)]
struct Node {
    f: f32,
    c: (i32, i32),
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> Blocks {
        Blocks { w: 20.0, h: 10.0, solid: vec![Area::new(5.0, 0.0, 6.0, 8.0)], water: vec![Area::new(12.0, 0.0, 14.0, 10.0)], bridges: vec![Area::new(11.5, 4.0, 14.5, 6.0)], keep_clear: vec![] }
    }

    #[test]
    fn walks_around_and_over_the_bridge() {
        let b = world();
        assert!(b.blocked(5.5, 3.0));
        assert!(b.blocked(13.0, 2.0));
        assert!(!b.blocked(13.0, 5.0));
        let p = b.path((2.0, 2.0), (17.0, 2.0)).expect("a way");
        // Around the wall (over y = 8) and over the bridge (4..6).
        assert!(p.iter().any(|(_, y)| *y > 8.0));
        assert!(p.iter().filter(|(x, _)| *x > 12.0 && *x < 14.0).all(|(_, y)| *y > 4.0 && *y < 6.0));
        assert!(p.iter().all(|(x, y)| b.free(*x, *y)));
    }

    #[test]
    fn finds_building_spots() {
        let b = world();
        let (x, y) = b.spot_near(5.5, 4.0, 2.0, 1.0, &[]).unwrap();
        assert!(b.can_build(&Area::new(x - 1.0, y, x + 1.0, y + 1.0)));
        assert!(b.spot_near(13.0, 5.0, 2.0, 1.0, &[]).is_some());
    }
}

//! The world's pixel art, drawn in code (16 pixels to a tile): ground, the river, plants,
//! buildings and the two characters. Everything is original; nothing is loaded from disk.

pub type Rgba = [u8; 4];

pub const fn c(r: u8, g: u8, b: u8) -> Rgba {
    [r, g, b, 255]
}

pub const CLEAR: Rgba = [0, 0, 0, 0];
const OUTLINE: Rgba = c(40, 28, 24);

/// An image, top row first.
#[derive(Clone)]
pub struct Img {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

/// A small deterministic random source (the same art every time).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// 0..1
    pub fn f(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
    pub fn range(&mut self, n: u32) -> u32 {
        (self.next() % n.max(1) as u64) as u32
    }
}

fn shade(col: Rgba, k: f32) -> Rgba {
    let f = |v: u8| ((v as f32 * k).round().clamp(0.0, 255.0)) as u8;
    [f(col[0]), f(col[1]), f(col[2]), col[3]]
}

impl Img {
    pub fn new(w: u32, h: u32) -> Img {
        Img { w, h, px: vec![0; (w * h * 4) as usize] }
    }

    pub fn set(&mut self, x: i32, y: i32, col: Rgba) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || col[3] == 0 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        if col[3] == 255 {
            self.px[i..i + 4].copy_from_slice(&col);
        } else {
            let a = col[3] as f32 / 255.0;
            for k in 0..3 {
                self.px[i + k] = (self.px[i + k] as f32 * (1.0 - a) + col[k] as f32 * a) as u8;
            }
            self.px[i + 3] = self.px[i + 3].max(col[3]);
        }
    }

    pub fn get(&self, x: i32, y: i32) -> Rgba {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return CLEAR;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, col: Rgba) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, col);
            }
        }
    }

    /// A filled rectangle with a little per-pixel variation.
    pub fn rect_noise(&mut self, x: i32, y: i32, w: i32, h: i32, col: Rgba, amount: f32, rng: &mut Rng) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, shade(col, 1.0 + (rng.f() - 0.5) * amount));
            }
        }
    }

    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, col: Rgba) {
        for y in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
            for x in (cx - rx).floor() as i32..=(cx + rx).ceil() as i32 {
                let dx = (x as f32 + 0.5 - cx) / rx;
                let dy = (y as f32 + 0.5 - cy) / ry;
                if dx * dx + dy * dy <= 1.0 {
                    self.set(x, y, col);
                }
            }
        }
    }

    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, col: Rgba) {
        let n = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for i in 0..=n {
            let t = i as f32 / n as f32;
            self.set((x0 as f32 + (x1 - x0) as f32 * t).round() as i32, (y0 as f32 + (y1 - y0) as f32 * t).round() as i32, col);
        }
    }

    /// A dark line around everything drawn, so sprites read well on any ground.
    pub fn outline(&mut self) {
        let src = self.clone();
        for y in 0..self.h as i32 {
            for x in 0..self.w as i32 {
                if src.get(x, y)[3] != 0 {
                    continue;
                }
                let near = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| src.get(x + dx, y + dy)[3] > 128);
                if near {
                    self.set(x, y, OUTLINE);
                }
            }
        }
    }

    /// Rows bottom first.
    pub fn flipped(&self) -> Vec<u8> {
        let row = (self.w * 4) as usize;
        let mut out = Vec::with_capacity(self.px.len());
        for y in (0..self.h as usize).rev() {
            out.extend_from_slice(&self.px[y * row..(y + 1) * row]);
        }
        out
    }

    /// Mirror left to right.
    pub fn mirrored(&self) -> Img {
        let mut out = Img::new(self.w, self.h);
        for y in 0..self.h as i32 {
            for x in 0..self.w as i32 {
                let p = self.get(self.w as i32 - 1 - x, y);
                let i = ((y as u32 * self.w + x as u32) * 4) as usize;
                out.px[i..i + 4].copy_from_slice(&p);
            }
        }
        out
    }
}

// ------------------------------------------------------------------ palette

const GRASS: Rgba = c(108, 158, 74);
const GRASS_D: Rgba = c(84, 130, 60);
const SAND: Rgba = c(222, 196, 140);
const WATER: Rgba = c(64, 132, 186);
const BRICK: Rgba = c(184, 132, 88);
const MUD: Rgba = c(176, 128, 84);
const MUD_D: Rgba = c(132, 92, 60);
const WOOD: Rgba = c(122, 82, 50);
const LEAF: Rgba = c(70, 140, 70);
const LEAF_D: Rgba = c(46, 104, 52);
const STONE: Rgba = c(170, 164, 150);
const STONE_D: Rgba = c(128, 122, 112);
const GOLD: Rgba = c(222, 176, 64);
const LAPIS: Rgba = c(46, 72, 150);
const SKIN: Rgba = c(198, 146, 106);
const HAIR: Rgba = c(34, 26, 22);

// ------------------------------------------------------------------ ground (16×16, tiled)

pub fn grass() -> Img {
    let mut i = Img::new(16, 16);
    let mut r = Rng::new(1);
    i.rect_noise(0, 0, 16, 16, GRASS, 0.12, &mut r);
    for _ in 0..10 {
        let (x, y) = (r.range(16) as i32, r.range(15) as i32);
        i.set(x, y, GRASS_D);
        i.set(x, y + 1, GRASS_D);
    }
    for _ in 0..2 {
        let (x, y) = (r.range(16) as i32, r.range(16) as i32);
        i.set(x, y, c(150, 190, 96));
    }
    i
}

pub fn sand() -> Img {
    let mut i = Img::new(16, 16);
    let mut r = Rng::new(2);
    i.rect_noise(0, 0, 16, 16, SAND, 0.08, &mut r);
    for _ in 0..6 {
        i.set(r.range(16) as i32, r.range(16) as i32, shade(SAND, 0.85));
    }
    i
}

pub fn water() -> Img {
    let mut i = Img::new(16, 16);
    let mut r = Rng::new(3);
    i.rect_noise(0, 0, 16, 16, WATER, 0.06, &mut r);
    for k in 0..3 {
        let y = 3 + k * 5;
        let x = r.range(10) as i32;
        i.line(x, y, x + 4, y, c(120, 180, 220));
    }
    i
}

pub fn path() -> Img {
    let mut i = Img::new(16, 16);
    let mut r = Rng::new(4);
    i.rect(0, 0, 16, 16, shade(BRICK, 0.7));
    for row in 0..4 {
        let off = if row % 2 == 0 { 0 } else { 4 };
        for col in -1..2 {
            let x = col * 8 + off;
            i.rect_noise(x + 1, row * 4 + 1, 7, 3, BRICK, 0.15, &mut r);
        }
    }
    i
}

pub fn field_tile() -> Img {
    let mut i = Img::new(16, 16);
    let mut r = Rng::new(5);
    i.rect_noise(0, 0, 16, 16, c(120, 88, 56), 0.1, &mut r);
    for y in (1..16).step_by(4) {
        i.line(0, y, 15, y, c(96, 68, 44));
    }
    i
}

// ------------------------------------------------------------------ things

pub fn palm() -> Img {
    let mut i = Img::new(32, 48);
    // Trunk, leaning a little.
    for y in 16..46 {
        let x = 15 + ((46 - y) as f32 * 0.08) as i32;
        let col = if y % 3 == 0 { shade(WOOD, 0.8) } else { WOOD };
        i.rect(x, y, 3, 1, col);
    }
    // Fronds.
    let cx = 17.0;
    let cy = 15.0;
    for (dx, dy) in [(-14.0, 4.0), (14.0, 4.0), (-11.0, -6.0), (11.0, -6.0), (0.0, -10.0), (-6.0, 8.0), (6.0, 8.0)] {
        for t in 0..14 {
            let k = t as f32 / 13.0;
            let x = cx + dx * k;
            let y = cy + dy * k + (k * k) * 6.0;
            i.ellipse(x, y, 2.4 - k, 1.6, if t % 2 == 0 { LEAF } else { LEAF_D });
        }
    }
    // Dates.
    i.ellipse(15.0, 17.0, 2.0, 1.5, c(150, 80, 40));
    i.ellipse(19.0, 17.0, 2.0, 1.5, c(150, 80, 40));
    i.outline();
    i
}

pub fn cedar() -> Img {
    let mut i = Img::new(32, 48);
    i.rect(14, 38, 4, 9, WOOD);
    let tiers = [(32.0, 13.0), (24.0, 11.0), (16.0, 9.0), (9.0, 6.0)];
    for (k, (y, r)) in tiers.iter().enumerate() {
        i.ellipse(16.0, *y, *r, 5.5, if k % 2 == 0 { c(40, 100, 64) } else { c(50, 116, 72) });
        i.ellipse(14.0, *y - 1.5, *r * 0.5, 2.0, c(70, 140, 90));
    }
    i.outline();
    i
}

pub fn house() -> Img {
    let mut i = Img::new(64, 56);
    let mut r = Rng::new(10);
    // Walls of mud brick, with a flat roof and a parapet.
    i.rect_noise(4, 18, 56, 36, MUD, 0.1, &mut r);
    for row in (20..54).step_by(4) {
        i.line(4, row, 59, row, MUD_D);
    }
    i.rect_noise(2, 12, 60, 7, shade(MUD, 1.1), 0.08, &mut r);
    for x in (2..62).step_by(6) {
        i.rect(x, 8, 4, 4, shade(MUD, 1.1));
    }
    // Door with a gold lintel, two windows.
    i.rect(26, 34, 12, 20, c(70, 46, 30));
    i.rect(25, 32, 14, 3, GOLD);
    i.rect(10, 28, 8, 7, c(50, 40, 40));
    i.rect(46, 28, 8, 7, c(50, 40, 40));
    // A lapis band.
    i.rect(4, 22, 56, 2, LAPIS);
    i.outline();
    i
}

pub fn ziggurat() -> Img {
    let mut i = Img::new(96, 72);
    let mut r = Rng::new(11);
    let steps = [(4, 52, 88, 18), (14, 36, 68, 17), (24, 22, 48, 15), (34, 10, 28, 13)];
    for (x, y, w, h) in steps {
        i.rect_noise(x, y, w, h, MUD, 0.12, &mut r);
        i.rect(x, y, w, 2, shade(MUD, 1.15));
        for row in (y + 4..y + h).step_by(4) {
            i.line(x, row, x + w - 1, row, MUD_D);
        }
    }
    // The stair up the middle and a shrine on top.
    i.rect(44, 10, 8, 60, shade(MUD, 0.85));
    for y in (12..70).step_by(3) {
        i.line(44, y, 51, y, MUD_D);
    }
    i.rect(40, 2, 16, 9, LAPIS);
    i.rect(40, 2, 16, 2, GOLD);
    i.outline();
    i
}

pub fn well() -> Img {
    let mut i = Img::new(24, 26);
    let mut r = Rng::new(12);
    i.ellipse(12.0, 18.0, 10.0, 6.0, STONE_D);
    i.ellipse(12.0, 16.0, 10.0, 6.0, STONE);
    i.ellipse(12.0, 16.0, 6.5, 3.5, c(30, 60, 90));
    for _ in 0..12 {
        let a = r.f() * 6.28;
        i.set((12.0 + a.cos() * 9.0) as i32, (16.0 + a.sin() * 5.0) as i32, STONE_D);
    }
    i.rect(2, 2, 2, 14, WOOD);
    i.rect(20, 2, 2, 14, WOOD);
    i.rect(2, 1, 20, 3, shade(WOOD, 1.2));
    i.rect(11, 4, 2, 6, c(90, 90, 90));
    i.outline();
    i
}

pub fn garden() -> Img {
    let mut i = Img::new(32, 24);
    let mut r = Rng::new(13);
    i.rect_noise(0, 6, 32, 18, c(110, 78, 50), 0.12, &mut r);
    for row in 0..3 {
        for col in 0..5 {
            let x = 3.0 + col as f32 * 6.5;
            let y = 9.0 + row as f32 * 5.5;
            i.ellipse(x, y, 2.6, 2.2, if (row + col) % 2 == 0 { LEAF } else { c(96, 160, 70) });
            if (row * 5 + col) % 3 == 0 {
                i.set(x as i32, y as i32 - 1, c(220, 80, 60));
            }
        }
    }
    i.rect(0, 4, 32, 2, WOOD);
    i.outline();
    i
}

pub fn flowers() -> Img {
    let mut i = Img::new(16, 16);
    let mut r = Rng::new(14);
    let cols = [c(230, 90, 110), c(250, 210, 80), c(170, 110, 220), c(250, 250, 240)];
    for k in 0..7 {
        let x = 2 + r.range(12) as i32;
        let y = 5 + r.range(9) as i32;
        i.line(x, y, x, y + 3, LEAF_D);
        let col = cols[k % cols.len()];
        i.set(x, y, col);
        i.set(x - 1, y, col);
        i.set(x + 1, y, col);
        i.set(x, y - 1, col);
    }
    i
}

pub fn fire() -> Img {
    let mut i = Img::new(16, 20);
    i.ellipse(8.0, 17.0, 7.0, 2.5, STONE_D);
    i.line(3, 16, 12, 13, WOOD);
    i.line(3, 13, 12, 16, WOOD);
    i.ellipse(8.0, 10.0, 4.0, 6.0, c(240, 120, 40));
    i.ellipse(8.0, 11.0, 2.5, 4.0, c(255, 200, 70));
    i.ellipse(8.0, 12.5, 1.2, 2.0, c(255, 245, 200));
    i.outline();
    i
}

pub fn tower() -> Img {
    let mut i = Img::new(32, 64);
    let mut r = Rng::new(15);
    i.rect_noise(6, 12, 20, 50, STONE, 0.12, &mut r);
    for row in (14..62).step_by(5) {
        i.line(6, row, 25, row, STONE_D);
    }
    for x in (4..28).step_by(6) {
        i.rect(x, 6, 4, 6, STONE);
    }
    i.rect(4, 10, 24, 3, STONE);
    i.rect(13, 46, 6, 16, c(70, 46, 30));
    i.rect(14, 24, 4, 7, c(40, 40, 50));
    i.rect(15, 0, 1, 7, WOOD);
    i.rect(16, 0, 7, 4, c(200, 50, 50));
    i.outline();
    i
}

pub fn statue() -> Img {
    let mut i = Img::new(24, 40);
    i.rect(3, 30, 18, 9, STONE_D);
    i.rect(5, 28, 14, 3, STONE);
    // A lamassu-like guardian, simplified: body, wings, crowned head.
    i.rect(6, 16, 12, 12, STONE);
    i.rect(4, 12, 4, 12, shade(STONE, 1.1));
    i.ellipse(14.0, 9.0, 4.0, 5.0, STONE);
    i.rect(10, 3, 8, 3, GOLD);
    i.rect(12, 12, 5, 4, STONE_D);
    i.outline();
    i
}

pub fn tablet() -> Img {
    let mut i = Img::new(20, 26);
    i.rect(9, 18, 2, 7, WOOD);
    i.rect(4, 23, 12, 2, WOOD);
    i.rect(2, 1, 16, 18, c(196, 160, 110));
    i.rect(2, 1, 16, 2, shade(c(196, 160, 110), 1.1));
    let mut r = Rng::new(16);
    for row in (5..17).step_by(3) {
        let mut x = 4;
        while x < 15 {
            let w = 1 + r.range(3) as i32;
            i.rect(x, row, w, 1, c(120, 90, 60));
            x += w + 1;
        }
    }
    i.outline();
    i
}

pub fn bench() -> Img {
    let mut i = Img::new(24, 12);
    i.rect(1, 3, 22, 3, WOOD);
    i.rect(1, 3, 22, 1, shade(WOOD, 1.2));
    i.rect(3, 6, 2, 6, shade(WOOD, 0.8));
    i.rect(19, 6, 2, 6, shade(WOOD, 0.8));
    i.outline();
    i
}

pub fn lamp() -> Img {
    let mut i = Img::new(10, 28);
    i.rect(4, 8, 2, 19, c(70, 70, 76));
    i.rect(2, 25, 6, 2, c(70, 70, 76));
    i.rect(2, 2, 6, 7, c(60, 60, 66));
    i.rect(3, 3, 4, 5, c(255, 220, 120));
    i.outline();
    i
}

pub fn workshop() -> Img {
    let mut i = Img::new(48, 40);
    let mut r = Rng::new(17);
    i.rect_noise(3, 14, 42, 25, WOOD, 0.12, &mut r);
    for x in (3..45).step_by(5) {
        i.line(x, 14, x, 38, shade(WOOD, 0.75));
    }
    // Sloped roof of reeds.
    for y in 4..15 {
        let inset = (14 - y) * 2;
        i.rect_noise(inset.max(0), y, 48 - 2 * inset.max(0), 1, c(196, 170, 90), 0.15, &mut r);
    }
    i.rect(18, 24, 12, 15, c(60, 40, 26));
    // Anvil outside.
    i.rect(36, 32, 8, 3, c(60, 60, 66));
    i.rect(38, 35, 4, 4, c(60, 60, 66));
    i.outline();
    i
}

pub fn boat() -> Img {
    let mut i = Img::new(40, 20);
    for y in 10..17 {
        let inset = (y - 10) as i32;
        i.rect(2 + inset, y, 36 - 2 * inset, 1, if y == 10 { shade(WOOD, 1.2) } else { WOOD });
    }
    i.rect(19, 0, 2, 11, shade(WOOD, 0.8));
    for y in 1..9 {
        i.rect(21, y, (y as i32) * 2 / 2 + 2, 1, c(236, 228, 210));
    }
    i.outline();
    i
}

pub fn fountain() -> Img {
    let mut i = Img::new(32, 28);
    i.ellipse(16.0, 21.0, 15.0, 6.0, STONE_D);
    i.ellipse(16.0, 19.0, 15.0, 6.0, STONE);
    i.ellipse(16.0, 19.0, 11.0, 4.0, WATER);
    i.rect(14, 7, 4, 12, STONE);
    i.ellipse(16.0, 7.0, 6.0, 2.5, STONE);
    for (x, y) in [(10, 4), (22, 4), (16, 1)] {
        i.line(16, 6, x, y, c(160, 210, 240));
    }
    i.outline();
    i
}

pub fn wall() -> Img {
    let mut i = Img::new(32, 20);
    let mut r = Rng::new(18);
    i.rect_noise(0, 4, 32, 16, MUD, 0.12, &mut r);
    for row in (6..20).step_by(4) {
        i.line(0, row, 31, row, MUD_D);
    }
    for x in (0..32).step_by(6) {
        i.rect(x, 0, 4, 4, MUD);
    }
    i.outline();
    i
}

pub fn gate() -> Img {
    let mut i = Img::new(40, 44);
    let mut r = Rng::new(19);
    i.rect_noise(0, 8, 12, 36, MUD, 0.1, &mut r);
    i.rect_noise(28, 8, 12, 36, MUD, 0.1, &mut r);
    i.rect_noise(0, 4, 40, 10, LAPIS, 0.1, &mut r);
    for x in (2..40).step_by(8) {
        i.rect(x, 7, 4, 4, GOLD);
    }
    i.rect(12, 14, 16, 30, c(60, 44, 30));
    i.outline();
    i
}

pub fn field() -> Img {
    let mut i = Img::new(32, 24);
    let mut r = Rng::new(20);
    i.rect_noise(0, 8, 32, 16, c(120, 88, 56), 0.1, &mut r);
    for x in (1..32).step_by(3) {
        let h = 6 + r.range(4) as i32;
        i.line(x, 22, x, 22 - h, c(214, 180, 80));
        i.set(x, 22 - h - 1, c(236, 206, 110));
    }
    i.outline();
    i
}

pub fn tent() -> Img {
    let mut i = Img::new(40, 32);
    for y in 4..30 {
        let half = (y - 4) * 19 / 26;
        i.rect(20 - half, y, half * 2, 1, if y % 4 == 0 { c(214, 196, 160) } else { c(232, 216, 180) });
    }
    i.rect(17, 18, 6, 12, c(90, 60, 40));
    i.rect(19, 0, 2, 6, WOOD);
    i.rect(21, 0, 6, 3, GOLD);
    i.outline();
    i
}

pub fn reeds() -> Img {
    let mut i = Img::new(12, 16);
    for (k, x) in [2, 5, 7, 10].iter().enumerate() {
        let h = 9 + (k as i32 % 3) * 2;
        i.line(*x, 15, *x, 15 - h, LEAF_D);
        i.rect(*x, 15 - h, 1, 3, c(120, 80, 40));
    }
    i
}

pub fn rock() -> Img {
    let mut i = Img::new(16, 12);
    i.ellipse(8.0, 7.0, 7.0, 4.5, STONE_D);
    i.ellipse(7.0, 6.0, 5.0, 3.0, STONE);
    i.outline();
    i
}

pub fn bridge() -> Img {
    let mut i = Img::new(16, 16);
    let mut r = Rng::new(21);
    for x in (0..16).step_by(4) {
        i.rect_noise(x, 0, 3, 16, WOOD, 0.15, &mut r);
        i.rect(x + 3, 0, 1, 16, shade(WOOD, 0.6));
    }
    i
}

/// A small soft shadow under the characters.
pub fn shadow() -> Img {
    let mut i = Img::new(16, 6);
    i.ellipse(8.0, 3.0, 7.0, 2.6, [0, 0, 0, 70]);
    i
}

// ------------------------------------------------------------------ characters (16×24, two frames)

/// Gilgamesh: a king of Uruk, in a blue robe with a gold band, a black curled beard and a crown.
pub fn gilgamesh(frame: u32) -> Img {
    let mut i = Img::new(16, 24);
    body(&mut i, frame, LAPIS, GOLD);
    // Head, hair, beard, crown.
    i.rect(5, 3, 6, 6, SKIN);
    i.rect(4, 2, 8, 2, HAIR);
    i.rect(4, 3, 1, 5, HAIR);
    i.rect(11, 3, 1, 5, HAIR);
    i.rect(5, 7, 6, 4, HAIR);
    for x in [5, 7, 9] {
        i.set(x, 10, c(60, 44, 34));
    }
    i.rect(4, 0, 8, 2, GOLD);
    i.set(5, 5, c(30, 20, 20));
    i.set(9, 5, c(30, 20, 20));
    i.outline();
    i
}

/// The owner: a traveller in a sandy tunic with a red sash.
pub fn traveller(frame: u32) -> Img {
    let mut i = Img::new(16, 24);
    body(&mut i, frame, c(214, 190, 140), c(190, 60, 50));
    i.rect(5, 3, 6, 6, c(226, 182, 140));
    i.rect(4, 1, 8, 3, c(110, 70, 40));
    i.rect(4, 3, 1, 3, c(110, 70, 40));
    i.set(6, 5, c(30, 20, 20));
    i.set(9, 5, c(30, 20, 20));
    i.rect(6, 7, 4, 1, c(170, 110, 90));
    i.outline();
    i
}

fn body(i: &mut Img, frame: u32, robe: Rgba, band: Rgba) {
    // Robe.
    i.rect(4, 9, 8, 11, robe);
    i.rect(3, 12, 10, 8, robe);
    i.rect(4, 14, 8, 1, band);
    // Arms.
    i.rect(2, 10, 2, 6, shade(robe, 0.85));
    i.rect(12, 10, 2, 6, shade(robe, 0.85));
    i.set(2, 16, SKIN);
    i.set(13, 16, SKIN);
    // Legs: frame 1 is mid-step.
    let (l, r) = if frame % 2 == 0 { (5, 9) } else { (4, 10) };
    i.rect(l, 20, 2, 3, c(90, 60, 40));
    i.rect(r, 20, 2, 3, c(90, 60, 40));
    i.rect(l - 1, 23, 3, 1, c(60, 40, 30));
    i.rect(r, 23, 3, 1, c(60, 40, 30));
}

/// A face for the character's head: the owner's photo of Gilgamesh, made round and small.
pub fn face_from(rgba: &[u8], w: u32, h: u32) -> Img {
    let size = 12u32;
    let mut out = Img::new(size, size);
    let side = w.min(h) as f32;
    let (ox, oy) = ((w as f32 - side) / 2.0, (h as f32 - side) / 2.0);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - size as f32 / 2.0;
            let dy = y as f32 + 0.5 - size as f32 / 2.0;
            if dx * dx + dy * dy > (size as f32 / 2.0).powi(2) {
                continue;
            }
            // Average the block of the photo behind this pixel.
            let x0 = ox + x as f32 * side / size as f32;
            let y0 = oy + y as f32 * side / size as f32;
            let step = (side / size as f32).max(1.0);
            let (mut acc, mut n) = ([0u32; 3], 0u32);
            let mut sy = y0;
            while sy < y0 + step {
                let mut sx = x0;
                while sx < x0 + step {
                    let i = ((sy as u32).min(h - 1) * w + (sx as u32).min(w - 1)) as usize * 4;
                    for k in 0..3 {
                        acc[k] += rgba[i + k] as u32;
                    }
                    n += 1;
                    sx += 1.0;
                }
                sy += 1.0;
            }
            let n = n.max(1);
            out.set(x as i32, y as i32, [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8, 255]);
        }
    }
    out.outline();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprites_draw() {
        for (name, img) in [("palm", palm()), ("house", house()), ("gilgamesh", gilgamesh(0)), ("ziggurat", ziggurat()), ("tent", tent())] {
            assert_eq!(img.px.len(), (img.w * img.h * 4) as usize, "{name}");
            assert!(img.px.chunks(4).any(|p| p[3] > 0), "{name} is empty");
            assert!(img.px.chunks(4).any(|p| p[3] == 0), "{name} has no transparent pixels");
        }
        assert_eq!(grass().flipped().len(), 16 * 16 * 4);
        let photo = vec![200u8; 64 * 48 * 4];
        let f = face_from(&photo, 64, 48);
        assert_eq!((f.w, f.h), (12, 12));
    }
}

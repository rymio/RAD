//! ADS-B and Mode S: the replies and squitters aircraft transponders send on 1090 MHz.
//!
//! [`Demod`] finds the frames in the dongle's samples (2 MS/s, so a 1 µs bit is two samples) the
//! way dump1090 does: a preamble of four pulses, then pulse-position bits. [`Tracker`] checks each
//! frame's parity, keeps what it says about each aircraft (callsign, altitude, speed, position
//! from CPR pairs, squawk) and turns it into a [`Packet`] for the packet list, with every field
//! and the bits it came from.

use serde::Serialize;
use std::collections::HashMap;

/// The sample rate the decoder expects.
pub const RATE: u32 = 2_000_000;
/// Where Mode S lives.
pub const FREQ: u32 = 1_090_000_000;

const POLY: u32 = 0xFFF409;
/// Samples from the start of a preamble to the end of the longest frame.
const FRAME_SAMPLES: usize = 16 + 112 * 2;

// ---------------------------------------------------------------- bits and parity

/// `len` bits of the message from bit `first` (1-based, as in the standards).
pub fn bits(m: &[u8], first: usize, len: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..len {
        let p = first - 1 + i;
        v = (v << 1) | ((m[p / 8] >> (7 - p % 8)) & 1) as u32;
    }
    v
}

/// The parity check: the CRC of everything but the last 24 bits, XORed with those bits. 0 for a
/// good DF17; the transponder's address for replies that carry it in the parity.
pub fn residual(m: &[u8]) -> u32 {
    let n = m.len();
    let mut rem: u32 = 0;
    for &b in &m[..n - 3] {
        rem ^= (b as u32) << 16;
        for _ in 0..8 {
            rem = if rem & 0x80_0000 != 0 { (rem << 1) ^ POLY } else { rem << 1 };
            rem &= 0xFF_FFFF;
        }
    }
    rem ^ ((m[n - 3] as u32) << 16 | (m[n - 2] as u32) << 8 | m[n - 1] as u32)
}

/// Long frames for these downlink formats, short ones for the rest.
fn frame_bits(df: u32) -> usize {
    if df >= 16 {
        112
    } else {
        56
    }
}

// ---------------------------------------------------------------- finding frames

/// A frame as it came off the air.
#[derive(Clone, Debug)]
pub struct Frame {
    pub bytes: Vec<u8>,
    /// Signal strength, dB relative to full scale.
    pub signal: f32,
}

/// Finds Mode S frames in 8-bit I/Q samples at 2 MS/s.
pub struct Demod {
    lut: Vec<f32>,
    mag: Vec<f32>,
}

impl Default for Demod {
    fn default() -> Self {
        let mut lut = vec![0f32; 65536];
        for i in 0..256 {
            for q in 0..256 {
                let (a, b) = (i as f32 - 127.4, q as f32 - 127.4);
                lut[i * 256 + q] = (a * a + b * b).sqrt();
            }
        }
        Demod { lut, mag: Vec::with_capacity(300_000) }
    }
}

impl Demod {
    /// Takes the next samples (I, Q, I, Q…); frames that start near the end are found with the
    /// next call.
    pub fn feed(&mut self, iq: &[u8], out: &mut Vec<Frame>) {
        for p in iq.chunks_exact(2) {
            self.mag.push(self.lut[(p[0] as usize) << 8 | p[1] as usize]);
        }
        let m = &self.mag;
        let mut j = 0;
        let end = m.len().saturating_sub(FRAME_SAMPLES);
        while j < end {
            let p = &m[j..];
            // Pulses at 0, 1, 3.5 and 4.5 µs; quiet between them.
            if !(p[0] > p[1] && p[1] < p[2] && p[2] > p[3] && p[3] < p[0] && p[4] < p[0] && p[5] < p[0] && p[6] < p[0] && p[7] > p[8] && p[8] < p[9] && p[9] > p[6]) {
                j += 1;
                continue;
            }
            let high = (p[0] + p[2] + p[7] + p[9]) / 6.0;
            if p[4] >= high || p[5] >= high || p[11] >= high || p[12] >= high || p[13] >= high || p[14] >= high {
                j += 1;
                continue;
            }
            let mut bytes = [0u8; 14];
            let mut prev = 0u8;
            for i in 0..112 {
                let (a, b) = (p[16 + 2 * i], p[17 + 2 * i]);
                let bit = if a > b {
                    1
                } else if a < b {
                    0
                } else {
                    prev
                };
                prev = bit;
                bytes[i / 8] |= bit << (7 - i % 8);
            }
            let df = (bytes[0] >> 3) as u32;
            if !matches!(df, 0 | 4 | 5 | 11 | 16 | 17 | 18 | 19 | 20 | 21) {
                j += 1;
                continue;
            }
            let n = frame_bits(df) / 8;
            let level = (p[0] + p[2] + p[7] + p[9]) / 4.0;
            out.push(Frame { bytes: bytes[..n].to_vec(), signal: 20.0 * (level / 128.0).max(1e-6).log10() });
            // The next frame can't start inside this one's preamble; the parity check decides
            // later whether this was a frame at all, so only the preamble is skipped.
            j += 8;
        }
        self.mag.drain(..j.min(self.mag.len()));
    }
}

// ---------------------------------------------------------------- what the fields mean

const CHARS: &[u8; 64] = b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

fn callsign(m: &[u8], first: usize) -> String {
    (0..8).map(|i| CHARS[bits(m, first + 6 * i, 6) as usize] as char).collect::<String>().trim().replace('#', "")
}

/// The 13-bit identity / altitude field rearranged as ABCD octal digits (0xABCD layout).
fn id13_to_gillham(f: u32) -> u32 {
    let map = [(0x1000, 0x0010), (0x0800, 0x1000), (0x0400, 0x0020), (0x0200, 0x2000), (0x0100, 0x0040), (0x0080, 0x4000), (0x0020, 0x0100), (0x0010, 0x0001), (0x0008, 0x0200), (0x0004, 0x0002), (0x0002, 0x0400), (0x0001, 0x0004)];
    map.iter().filter(|(b, _)| f & b != 0).fold(0, |a, (_, g)| a | g)
}

/// A squawk from a 13-bit identity field: four octal digits.
pub fn squawk(f: u32) -> String {
    format!("{:04x}", id13_to_gillham(f))
}

/// Gillham (100 ft) altitude, in feet.
fn gillham_alt(mode_a: u32) -> Option<i32> {
    if mode_a & 0xFFFF_8889 != 0 || mode_a & 0xF0 == 0 {
        return None;
    }
    let mut hundreds = 0u32;
    if mode_a & 0x10 != 0 {
        hundreds ^= 7;
    }
    if mode_a & 0x20 != 0 {
        hundreds ^= 3;
    }
    if mode_a & 0x40 != 0 {
        hundreds ^= 1;
    }
    if hundreds & 5 == 5 {
        hundreds ^= 2;
    }
    if hundreds > 5 {
        return None;
    }
    let mut fives = 0u32;
    for (b, x) in [(0x2, 0xFF), (0x4, 0x7F), (0x1000, 0x3F), (0x2000, 0x1F), (0x4000, 0x0F), (0x100, 0x7), (0x200, 0x3), (0x400, 0x1)] {
        if mode_a & b != 0 {
            fives ^= x;
        }
    }
    if fives & 1 != 0 {
        hundreds = 6 - hundreds;
    }
    Some((fives as i32 * 5 + hundreds as i32 - 13) * 100)
}

/// The 13-bit altitude code of replies (DF0/4/16/20), in feet.
pub fn alt13(a: u32) -> Option<i32> {
    if a == 0 || a & 0x40 != 0 {
        return None; // unknown, or metres
    }
    if a & 0x10 != 0 {
        let n = ((a & 0x1F80) >> 2) | ((a & 0x20) >> 1) | (a & 0x0F);
        Some(n as i32 * 25 - 1000)
    } else {
        gillham_alt(id13_to_gillham(a))
    }
}

/// The 12-bit altitude code of airborne positions, in feet.
pub fn alt12(a: u32) -> Option<i32> {
    if a == 0 {
        return None;
    }
    if a & 0x10 != 0 {
        let n = ((a & 0xFE0) >> 1) | (a & 0x0F);
        Some(n as i32 * 25 - 1000)
    } else {
        gillham_alt(id13_to_gillham(((a & 0xFC0) << 1) | (a & 0x3F)))
    }
}

const EMERGENCY: [&str; 8] = ["none", "general emergency", "medical emergency", "minimum fuel", "no communications", "unlawful interference", "downed aircraft", "reserved"];

const CATEGORY: [[&str; 8]; 4] = [
    ["", "", "", "", "", "", "", ""],
    ["", "surface emergency vehicle", "surface service vehicle", "ground obstruction", "ground obstruction", "ground obstruction", "ground obstruction", "ground obstruction"],
    ["", "glider / sailplane", "lighter than air", "parachutist / skydiver", "ultralight / hang glider", "reserved", "UAV (drone)", "space vehicle"],
    ["", "light (< 15 500 lb)", "small (15 500–75 000 lb)", "large (75 000–300 000 lb)", "high-vortex large (B757)", "heavy (> 300 000 lb)", "high performance (> 5 g, > 400 kt)", "rotorcraft"],
];

fn category(tc: u32, ca: u32) -> &'static str {
    // TC 4 = set A (aircraft), 3 = B, 2 = C (surface).
    match tc {
        4 => CATEGORY[3][ca as usize],
        3 => CATEGORY[2][ca as usize],
        2 => CATEGORY[1][ca as usize],
        _ => "",
    }
}

const DF_NAMES: [(u32, &str); 10] = [
    (0, "Short air-air surveillance (ACAS)"),
    (4, "Surveillance, altitude reply"),
    (5, "Surveillance, identity reply"),
    (11, "All-call reply"),
    (16, "Long air-air surveillance (ACAS)"),
    (17, "Extended squitter (ADS-B)"),
    (18, "Extended squitter, non-transponder (TIS-B / ADS-R)"),
    (19, "Military extended squitter"),
    (20, "Comm-B, altitude reply"),
    (21, "Comm-B, identity reply"),
];

fn df_name(df: u32) -> &'static str {
    DF_NAMES.iter().find(|(d, _)| *d == df).map(|(_, n)| *n).unwrap_or("Unknown")
}

fn tc_name(tc: u32) -> &'static str {
    match tc {
        0 => "No position",
        1..=4 => "Aircraft identification",
        5..=8 => "Surface position",
        9..=18 => "Airborne position (barometric altitude)",
        19 => "Airborne velocity",
        20..=22 => "Airborne position (GNSS height)",
        23 => "Test message",
        24 => "Surface system status",
        28 => "Aircraft status",
        29 => "Target state and status",
        31 => "Aircraft operational status",
        _ => "Reserved",
    }
}

// ---------------------------------------------------------------- CPR positions

fn nl(lat: f64) -> i32 {
    let lat = lat.abs();
    if lat < 1e-9 {
        return 59;
    }
    if (lat - 87.0).abs() < 1e-9 {
        return 2;
    }
    if lat > 87.0 {
        return 1;
    }
    let nz = 15.0f64;
    let a = 1.0 - (std::f64::consts::PI / (2.0 * nz)).cos();
    let b = (std::f64::consts::PI / 180.0 * lat).cos().powi(2);
    (2.0 * std::f64::consts::PI / (1.0 - a / b).acos()).floor() as i32
}

fn modf(a: f64, b: f64) -> f64 {
    a - b * (a / b).floor()
}

/// A position from an even and an odd airborne message (no reference needed); `odd_newer` says
/// which is the latest.
pub fn cpr_global(even: (u32, u32), odd: (u32, u32), odd_newer: bool) -> Option<(f64, f64)> {
    let (le, ne) = (even.0 as f64 / 131072.0, even.1 as f64 / 131072.0);
    let (lo, no) = (odd.0 as f64 / 131072.0, odd.1 as f64 / 131072.0);
    let j = (59.0 * le - 60.0 * lo + 0.5).floor();
    let mut lat_e = 6.0 * (modf(j, 60.0) + le);
    let mut lat_o = 360.0 / 59.0 * (modf(j, 59.0) + lo);
    if lat_e >= 270.0 {
        lat_e -= 360.0;
    }
    if lat_o >= 270.0 {
        lat_o -= 360.0;
    }
    if nl(lat_e) != nl(lat_o) {
        return None;
    }
    let (lat, l) = if odd_newer { (lat_o, nl(lat_o)) } else { (lat_e, nl(lat_e)) };
    let ni = if odd_newer { (l - 1).max(1) } else { l.max(1) } as f64;
    let m = (ne * (l - 1) as f64 - no * l as f64 + 0.5).floor();
    let mut lon = 360.0 / ni * (modf(m, ni) + if odd_newer { no } else { ne });
    if lon >= 180.0 {
        lon -= 360.0;
    }
    (lat.abs() <= 90.0).then_some((lat, lon))
}

/// A position from one message and a reference within ~180 NM (45 NM on the surface).
pub fn cpr_local(cpr: (u32, u32), odd: bool, surface: bool, reference: (f64, f64)) -> (f64, f64) {
    let i = odd as i32 as f64;
    let span = if surface { 90.0 } else { 360.0 };
    let (lat_c, lon_c) = (cpr.0 as f64 / 131072.0, cpr.1 as f64 / 131072.0);
    let dlat = span / (60.0 - i);
    let j = (reference.0 / dlat).floor() + (modf(reference.0, dlat) / dlat - lat_c + 0.5).floor();
    let lat = dlat * (j + lat_c);
    let dlon = span / ((nl(lat) as f64 - i).max(1.0));
    let m = (reference.1 / dlon).floor() + (modf(reference.1, dlon) / dlon - lon_c + 0.5).floor();
    let mut lon = dlon * (m + lon_c);
    if lon >= 180.0 {
        lon -= 360.0;
    }
    (lat, lon)
}

/// Great-circle distance in km and bearing in degrees from `a` to `b`.
pub fn range_bearing(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let d = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    let km = 2.0 * 6371.0 * d.sqrt().asin();
    let y = (lo2 - lo1).sin() * la2.cos();
    let x = la1.cos() * la2.sin() - la1.sin() * la2.cos() * (lo2 - lo1).cos();
    (km, (y.atan2(x).to_degrees() + 360.0) % 360.0)
}

// ---------------------------------------------------------------- packets

/// One field of a packet, with the bits it was read from (1-based, as in the standards).
#[derive(Clone, Debug, Serialize)]
pub struct Field {
    pub group: &'static str,
    pub name: &'static str,
    pub value: String,
    pub start: u16,
    pub len: u16,
}

/// A frame decoded for the packet list.
#[derive(Clone, Debug, Serialize)]
pub struct Packet {
    pub n: u64,
    /// Milliseconds since 1970.
    pub t: u64,
    pub df: u32,
    pub icao: String,
    pub hex: String,
    pub signal: f32,
    /// "ok", "corrected" (one bit fixed) or "address" (the address is in the parity).
    pub crc: &'static str,
    /// The kind of message, short ("ident", "position", "velocity", "altitude", "squawk", "all-call", "status", "comm-b", "acas", "other").
    pub kind: &'static str,
    pub info: String,
    pub fields: Vec<Field>,
}

// ---------------------------------------------------------------- aircraft

#[derive(Clone, Debug, Default, Serialize)]
pub struct Aircraft {
    pub icao: String,
    pub country: &'static str,
    pub callsign: Option<String>,
    pub category: Option<&'static str>,
    pub squawk: Option<String>,
    pub emergency: Option<&'static str>,
    pub alt: Option<i32>,
    pub alt_gnss: Option<i32>,
    pub on_ground: bool,
    /// Ground speed in knots, track in degrees, vertical rate in ft/min.
    pub gs: Option<f64>,
    pub track: Option<f64>,
    pub vr: Option<i32>,
    pub ias: Option<u32>,
    pub tas: Option<u32>,
    pub heading: Option<f64>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// From the receiver, when its place is known.
    pub km: Option<f64>,
    pub bearing: Option<f64>,
    pub msgs: u64,
    pub signal: f32,
    pub first_seen: u64,
    pub last_seen: u64,
    pub pos_seen: Option<u64>,
    #[serde(skip)]
    even: Option<((u32, u32), u64)>,
    #[serde(skip)]
    odd: Option<((u32, u32), u64)>,
}

/// Keeps track of the aircraft heard and decodes their frames.
pub struct Tracker {
    pub aircraft: HashMap<u32, Aircraft>,
    /// Where the receiver is, for distances and for positions from a single message.
    pub receiver: Option<(f64, f64)>,
    syndromes: HashMap<u32, usize>,
    count: u64,
    pub frames: u64,
    pub bad: u64,
}

impl Default for Tracker {
    fn default() -> Self {
        // Single-bit errors in a long frame: the residual each bit gives on its own.
        let mut syndromes = HashMap::new();
        for i in 5..112 {
            let mut m = [0u8; 14];
            m[i / 8] = 1 << (7 - i % 8);
            syndromes.insert(residual(&m), i);
        }
        Tracker { aircraft: HashMap::new(), receiver: None, syndromes, count: 0, frames: 0, bad: 0 }
    }
}

impl Tracker {
    /// Forgets aircraft not heard for a minute; returns how many went.
    pub fn expire(&mut self, now: u64) -> usize {
        let before = self.aircraft.len();
        self.aircraft.retain(|_, a| now.saturating_sub(a.last_seen) < 60_000);
        before - self.aircraft.len()
    }

    /// The aircraft, nearest first (or most recently heard when the receiver's place isn't known).
    pub fn list(&self) -> Vec<Aircraft> {
        let mut v: Vec<Aircraft> = self.aircraft.values().filter(|a| a.msgs >= 2).cloned().collect();
        v.sort_by(|a, b| match (a.km, b.km) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => b.last_seen.cmp(&a.last_seen),
        });
        v
    }

    fn known(&self, icao: u32) -> bool {
        self.aircraft.contains_key(&icao)
    }

    /// Checks a frame's parity and decodes it. `None` if it isn't a frame after all.
    pub fn handle(&mut self, f: &Frame, now: u64) -> Option<Packet> {
        let mut m = f.bytes.clone();
        let df = bits(&m, 1, 5);
        let r = residual(&m);
        let (icao, crc) = match df {
            17 | 18 => {
                if r == 0 {
                    (bits(&m, 9, 24), "ok")
                } else if let Some(&bit) = self.syndromes.get(&r).filter(|_| m.len() == 14) {
                    m[bit / 8] ^= 1 << (7 - bit % 8);
                    (bits(&m, 9, 24), "corrected")
                } else {
                    self.bad += 1;
                    return None;
                }
            }
            11 => {
                if r & 0xFF_FF80 != 0 {
                    self.bad += 1;
                    return None;
                }
                (bits(&m, 9, 24), "ok")
            }
            0 | 4 | 5 | 16 | 20 | 21 => {
                if !self.known(r) {
                    self.bad += 1;
                    return None;
                }
                (r, "address")
            }
            _ => {
                self.bad += 1;
                return None;
            }
        };
        self.frames += 1;
        self.count += 1;
        let ac = self.aircraft.entry(icao).or_insert_with(|| Aircraft { icao: format!("{icao:06X}"), country: country(icao), first_seen: now, ..Default::default() });
        ac.msgs += 1;
        ac.last_seen = now;
        ac.signal = if ac.msgs == 1 { f.signal } else { ac.signal * 0.8 + f.signal * 0.2 };

        let mut fields = vec![
            Field { group: "Mode S", name: "Downlink format", value: format!("{df}: {}", df_name(df)), start: 1, len: 5 },
        ];
        let mut kind = "other";
        let mut info = String::new();
        let receiver = self.receiver;
        match df {
            0 | 16 => {
                kind = "acas";
                fields.push(Field { group: "Mode S", name: "Vertical status", value: if bits(&m, 6, 1) == 1 { "on the ground".into() } else { "airborne".into() }, start: 6, len: 1 });
                let alt = alt13(bits(&m, 20, 13));
                fields.push(Field { group: "Mode S", name: "Altitude", value: alt.map(|a| format!("{a} ft")).unwrap_or("unknown".into()), start: 20, len: 13 });
                if let Some(a) = alt {
                    ac.alt = Some(a);
                    info = format!("ACAS, {a} ft");
                } else {
                    info = "ACAS".into();
                }
            }
            4 | 20 => {
                kind = if df == 20 { "comm-b" } else { "altitude" };
                let fs = bits(&m, 6, 3);
                fields.push(Field { group: "Mode S", name: "Flight status", value: flight_status(fs).into(), start: 6, len: 3 });
                if fs == 1 || fs == 3 {
                    ac.on_ground = true;
                } else if fs == 0 || fs == 2 {
                    ac.on_ground = false;
                }
                let alt = alt13(bits(&m, 20, 13));
                fields.push(Field { group: "Mode S", name: "Altitude", value: alt.map(|a| format!("{a} ft")).unwrap_or("unknown".into()), start: 20, len: 13 });
                if let Some(a) = alt {
                    ac.alt = Some(a);
                }
                info = format!("Altitude {}", alt.map(|a| format!("{a} ft")).unwrap_or("?".into()));
                if df == 20 {
                    info += &comm_b(&m, ac, &mut fields);
                }
            }
            5 | 21 => {
                kind = if df == 21 { "comm-b" } else { "squawk" };
                let fs = bits(&m, 6, 3);
                fields.push(Field { group: "Mode S", name: "Flight status", value: flight_status(fs).into(), start: 6, len: 3 });
                let sq = squawk(bits(&m, 20, 13));
                fields.push(Field { group: "Mode S", name: "Squawk (identity)", value: format!("{sq}{}", special_squawk(&sq)), start: 20, len: 13 });
                info = format!("Squawk {sq}{}", special_squawk(&sq));
                ac.squawk = Some(sq);
                if df == 21 {
                    info += &comm_b(&m, ac, &mut fields);
                }
            }
            11 => {
                kind = "all-call";
                fields.push(Field { group: "Mode S", name: "Capability", value: capability(bits(&m, 6, 3)).into(), start: 6, len: 3 });
                info = "All-call reply".into();
            }
            17 | 18 => {
                if df == 18 {
                    fields.push(Field { group: "Mode S", name: "Control field", value: bits(&m, 6, 3).to_string(), start: 6, len: 3 });
                } else {
                    fields.push(Field { group: "Mode S", name: "Capability", value: capability(bits(&m, 6, 3)).into(), start: 6, len: 3 });
                }
                let me = |k: usize, len: usize| bits(&m, 32 + k, len);
                let tc = me(1, 5);
                fields.push(Field { group: "ADS-B", name: "Type code", value: format!("{tc}: {}", tc_name(tc)), start: 33, len: 5 });
                match tc {
                    1..=4 => {
                        kind = "ident";
                        let cs = callsign(&m, 41);
                        let cat = category(tc, me(6, 3));
                        fields.push(Field { group: "ADS-B", name: "Emitter category", value: if cat.is_empty() { "not given".into() } else { cat.into() }, start: 38, len: 3 });
                        fields.push(Field { group: "ADS-B", name: "Callsign", value: cs.clone(), start: 41, len: 48 });
                        info = format!("Callsign {cs}");
                        if !cs.is_empty() {
                            ac.callsign = Some(cs);
                        }
                        if !cat.is_empty() {
                            ac.category = Some(cat);
                        }
                    }
                    5..=8 | 9..=18 | 20..=22 => {
                        kind = "position";
                        let surface = tc <= 8;
                        let odd = me(22, 1) == 1;
                        let cpr = (me(23, 17), me(40, 17));
                        if surface {
                            ac.on_ground = true;
                            let mov = me(6, 7);
                            fields.push(Field { group: "ADS-B", name: "Movement", value: surface_speed(mov).map(|s| format!("{s:.1} kt")).unwrap_or("unknown".into()), start: 38, len: 7 });
                            if me(13, 1) == 1 {
                                let trk = me(14, 7) as f64 * 360.0 / 128.0;
                                ac.track = Some(trk);
                                fields.push(Field { group: "ADS-B", name: "Ground track", value: format!("{trk:.1}°"), start: 46, len: 7 });
                            }
                            if let Some(s) = surface_speed(mov) {
                                ac.gs = Some(s);
                            }
                        } else {
                            ac.on_ground = false;
                            fields.push(Field { group: "ADS-B", name: "Surveillance status", value: ["no condition", "permanent alert", "temporary alert", "SPI"][me(6, 2) as usize].into(), start: 38, len: 2 });
                            let a = me(9, 12);
                            if tc <= 18 {
                                let alt = alt12(a);
                                fields.push(Field { group: "ADS-B", name: "Barometric altitude", value: alt.map(|a| format!("{a} ft")).unwrap_or("unknown".into()), start: 41, len: 12 });
                                if alt.is_some() {
                                    ac.alt = alt;
                                }
                            } else {
                                let h = (a as f64 * 3.28084).round() as i32;
                                fields.push(Field { group: "ADS-B", name: "GNSS height", value: format!("{a} m ({h} ft)"), start: 41, len: 12 });
                                ac.alt_gnss = Some(h);
                            }
                            fields.push(Field { group: "ADS-B", name: "UTC sync", value: (me(21, 1) == 1).to_string(), start: 53, len: 1 });
                        }
                        fields.push(Field { group: "ADS-B", name: "CPR format", value: if odd { "odd".into() } else { "even".into() }, start: 54, len: 1 });
                        fields.push(Field { group: "ADS-B", name: "CPR latitude", value: cpr.0.to_string(), start: 55, len: 17 });
                        fields.push(Field { group: "ADS-B", name: "CPR longitude", value: cpr.1.to_string(), start: 72, len: 17 });
                        if odd {
                            ac.odd = Some((cpr, now));
                        } else {
                            ac.even = Some((cpr, now));
                        }
                        let pos = locate(ac, cpr, odd, surface, now, receiver);
                        if let Some((lat, lon)) = pos {
                            ac.lat = Some(lat);
                            ac.lon = Some(lon);
                            ac.pos_seen = Some(now);
                            fields.push(Field { group: "Decoded", name: "Position", value: format!("{lat:.5}, {lon:.5}"), start: 55, len: 34 });
                        }
                        info = format!(
                            "{} {}{}",
                            if surface { "Surface position" } else { "Position" },
                            if odd { "(odd)" } else { "(even)" },
                            match (pos, ac.alt) {
                                (Some((la, lo)), Some(a)) if !surface => format!(" {la:.4}, {lo:.4} at {a} ft"),
                                (Some((la, lo)), _) => format!(" {la:.4}, {lo:.4}"),
                                (None, Some(a)) if !surface => format!(", {a} ft (waiting for the other half)"),
                                _ => " (waiting for the other half)".into(),
                            }
                        );
                    }
                    19 => {
                        kind = "velocity";
                        let st = me(6, 3);
                        fields.push(Field { group: "ADS-B", name: "Subtype", value: ["reserved", "ground speed", "ground speed (supersonic)", "airspeed", "airspeed (supersonic)", "reserved", "reserved", "reserved"][st as usize].into(), start: 38, len: 3 });
                        let mult = if st == 2 || st == 4 { 4.0 } else { 1.0 };
                        if st == 1 || st == 2 {
                            let (vew, vns) = (me(15, 10), me(26, 10));
                            if vew > 0 && vns > 0 {
                                let vx = (vew as f64 - 1.0) * mult * if me(14, 1) == 1 { -1.0 } else { 1.0 };
                                let vy = (vns as f64 - 1.0) * mult * if me(25, 1) == 1 { -1.0 } else { 1.0 };
                                let gs = vx.hypot(vy);
                                let trk = (vx.atan2(vy).to_degrees() + 360.0) % 360.0;
                                ac.gs = Some(gs);
                                ac.track = Some(trk);
                                fields.push(Field { group: "ADS-B", name: "East-west velocity", value: format!("{vx:.0} kt"), start: 46, len: 11 });
                                fields.push(Field { group: "ADS-B", name: "North-south velocity", value: format!("{vy:.0} kt"), start: 57, len: 11 });
                                fields.push(Field { group: "Decoded", name: "Ground speed", value: format!("{gs:.1} kt"), start: 46, len: 22 });
                                fields.push(Field { group: "Decoded", name: "Track", value: format!("{trk:.1}°"), start: 46, len: 22 });
                                info = format!("{gs:.0} kt, track {trk:.0}°");
                            }
                        } else if st == 3 || st == 4 {
                            if me(14, 1) == 1 {
                                let hdg = me(15, 10) as f64 * 360.0 / 1024.0;
                                ac.heading = Some(hdg);
                                fields.push(Field { group: "ADS-B", name: "Magnetic heading", value: format!("{hdg:.1}°"), start: 47, len: 10 });
                            }
                            let asp = me(26, 10);
                            if asp > 0 {
                                let v = ((asp - 1) as f64 * mult) as u32;
                                let tas = me(25, 1) == 1;
                                if tas {
                                    ac.tas = Some(v);
                                } else {
                                    ac.ias = Some(v);
                                }
                                fields.push(Field { group: "ADS-B", name: if tas { "True airspeed" } else { "Indicated airspeed" }, value: format!("{v} kt"), start: 58, len: 10 });
                                info = format!("{} {v} kt", if tas { "TAS" } else { "IAS" });
                            }
                        }
                        let vr = me(38, 9);
                        if vr > 0 {
                            let v = (vr as i32 - 1) * 64 * if me(37, 1) == 1 { -1 } else { 1 };
                            ac.vr = Some(v);
                            fields.push(Field { group: "ADS-B", name: "Vertical rate", value: format!("{v:+} ft/min ({})", if me(36, 1) == 1 { "barometric" } else { "GNSS" }), start: 68, len: 11 });
                            info += &format!(", {v:+} ft/min");
                        }
                    }
                    28 => {
                        kind = "status";
                        if me(6, 3) == 1 {
                            let e = me(9, 3);
                            let sq = squawk(me(12, 13));
                            fields.push(Field { group: "ADS-B", name: "Emergency state", value: EMERGENCY[e as usize].into(), start: 41, len: 3 });
                            fields.push(Field { group: "ADS-B", name: "Squawk", value: format!("{sq}{}", special_squawk(&sq)), start: 44, len: 13 });
                            ac.emergency = (e != 0).then_some(EMERGENCY[e as usize]);
                            info = format!("Squawk {sq}{}", if e != 0 { format!(", {}", EMERGENCY[e as usize].to_uppercase()) } else { String::new() });
                            ac.squawk = Some(sq);
                        } else {
                            info = "Aircraft status (ACAS RA)".into();
                        }
                    }
                    29 => {
                        kind = "status";
                        info = "Target state and status".into();
                        if me(6, 2) == 1 {
                            let sel = me(10, 11);
                            if sel > 0 {
                                let a = (sel as i32 - 1) * 32;
                                fields.push(Field { group: "ADS-B", name: "Selected altitude", value: format!("{a} ft ({})", if me(9, 1) == 1 { "FMS" } else { "MCP/FCU" }), start: 42, len: 11 });
                                info = format!("Selected altitude {a} ft");
                            }
                        }
                    }
                    31 => {
                        kind = "status";
                        info = format!("Operational status (ADS-B version {})", me(41, 3));
                        fields.push(Field { group: "ADS-B", name: "ADS-B version", value: me(41, 3).to_string(), start: 73, len: 3 });
                    }
                    _ => info = tc_name(tc).into(),
                }
            }
            _ => {}
        }
        fields.insert(1, Field { group: "Mode S", name: "ICAO address", value: format!("{icao:06X} ({})", country(icao)), start: if crc == "address" { (m.len() * 8 - 23) as u16 } else { 9 }, len: 24 });
        fields.push(Field { group: "Mode S", name: "Parity", value: match crc { "ok" => "good".into(), "corrected" => "one bit corrected".into(), _ => "address/parity (known aircraft)".into() }, start: (m.len() * 8 - 23) as u16, len: 24 });
        if let (Some(r), Some(la), Some(lo)) = (receiver, ac.lat, ac.lon) {
            let (km, brg) = range_bearing(r, (la, lo));
            ac.km = Some(km);
            ac.bearing = Some(brg);
        }
        Some(Packet { n: self.count, t: now, df, icao: format!("{icao:06X}"), hex: m.iter().map(|b| format!("{b:02X}")).collect(), signal: (f.signal * 10.0).round() / 10.0, crc, kind, info, fields })
    }
}

fn locate(ac: &Aircraft, cpr: (u32, u32), odd: bool, surface: bool, now: u64, receiver: Option<(f64, f64)>) -> Option<(f64, f64)> {
    // A pair within 10 s gives a position anywhere (airborne).
    if !surface {
        if let (Some((e, te)), Some((o, to))) = (ac.even, ac.odd) {
            if te.abs_diff(to) < 10_000 {
                if let Some(p) = cpr_global(e, o, odd) {
                    if receiver.map(|r| range_bearing(r, p).0 < 700.0).unwrap_or(true) {
                        return Some(p);
                    }
                }
            }
        }
    }
    // Otherwise from where it was a moment ago, or from the receiver.
    if let (Some(la), Some(lo), Some(t)) = (ac.lat, ac.lon, ac.pos_seen) {
        if now.saturating_sub(t) < 60_000 {
            let p = cpr_local(cpr, odd, surface, (la, lo));
            if range_bearing((la, lo), p).0 < 40.0 {
                return Some(p);
            }
        }
    }
    if let Some(r) = receiver {
        let p = cpr_local(cpr, odd, surface, r);
        if range_bearing(r, p).0 < if surface { 40.0 } else { 300.0 } {
            return Some(p);
        }
    }
    None
}

fn surface_speed(m: u32) -> Option<f64> {
    Some(match m {
        0 | 125.. => return None,
        1 => 0.0,
        2..=8 => 0.125 + (m - 2) as f64 * 0.125,
        9..=12 => 1.0 + (m - 9) as f64 * 0.25,
        13..=38 => 2.0 + (m - 13) as f64 * 0.5,
        39..=93 => 15.0 + (m - 39) as f64,
        94..=108 => 70.0 + (m - 94) as f64 * 2.0,
        109..=123 => 100.0 + (m - 109) as f64 * 5.0,
        124 => 175.0,
    })
}

fn flight_status(fs: u32) -> &'static str {
    ["airborne", "on the ground", "airborne, alert", "on the ground, alert", "alert, SPI", "SPI", "reserved", "reserved"][fs as usize]
}

fn capability(ca: u32) -> &'static str {
    match ca {
        0 => "level 1 transponder",
        4 => "level 2+, on the ground",
        5 => "level 2+, airborne",
        6 => "level 2+, ground or airborne",
        7 => "downlink request / ground or airborne",
        _ => "reserved",
    }
}

/// What a squawk means, when it means something special.
pub fn special_squawk(sq: &str) -> &'static str {
    match sq {
        "7500" => " (HIJACK)",
        "7600" => " (RADIO FAILURE)",
        "7700" => " (EMERGENCY)",
        "7000" => " (VFR, Europe)",
        "1200" => " (VFR, US)",
        "2000" => " (no code assigned)",
        _ => "",
    }
}

/// Comm-B replies: the register that's easy to recognise (BDS 2,0: the callsign).
fn comm_b(m: &[u8], ac: &mut Aircraft, fields: &mut Vec<Field>) -> String {
    fields.push(Field { group: "Comm-B", name: "MB field", value: m[4..11].iter().map(|b| format!("{b:02X}")).collect(), start: 33, len: 56 });
    if bits(m, 33, 8) == 0x20 {
        let cs = callsign(m, 41);
        if cs.len() >= 2 && cs.chars().all(|c| c.is_ascii_alphanumeric()) {
            fields.push(Field { group: "Comm-B", name: "BDS 2,0 callsign", value: cs.clone(), start: 41, len: 48 });
            ac.callsign = Some(cs.clone());
            return format!(", callsign {cs}");
        }
    }
    String::new()
}

/// The country an ICAO address block is allocated to (Annex 10, vol. III).
pub fn country(icao: u32) -> &'static str {
    const BLOCKS: &[(u32, u32, &str)] = &[
        (0x008000, 0x00FFFF, "South Africa"),
        (0x010000, 0x017FFF, "Egypt"),
        (0x020000, 0x027FFF, "Morocco"),
        (0x040000, 0x040FFF, "Ethiopia"),
        (0x04C000, 0x04CFFF, "Kenya"),
        (0x064000, 0x064FFF, "Nigeria"),
        (0x06A000, 0x06AFFF, "Qatar"),
        (0x0D0000, 0x0D7FFF, "Mexico"),
        (0x140000, 0x15FFFF, "Russia"),
        (0x300000, 0x33FFFF, "Italy"),
        (0x340000, 0x37FFFF, "Spain"),
        (0x380000, 0x3BFFFF, "France"),
        (0x3C0000, 0x3FFFFF, "Germany"),
        (0x400000, 0x43FFFF, "United Kingdom"),
        (0x440000, 0x447FFF, "Austria"),
        (0x448000, 0x44FFFF, "Belgium"),
        (0x458000, 0x45FFFF, "Denmark"),
        (0x460000, 0x467FFF, "Finland"),
        (0x468000, 0x46FFFF, "Greece"),
        (0x470000, 0x477FFF, "Hungary"),
        (0x478000, 0x47FFFF, "Norway"),
        (0x480000, 0x487FFF, "Netherlands"),
        (0x488000, 0x48FFFF, "Poland"),
        (0x490000, 0x497FFF, "Portugal"),
        (0x498000, 0x49FFFF, "Czechia"),
        (0x4A0000, 0x4A7FFF, "Romania"),
        (0x4A8000, 0x4AFFFF, "Sweden"),
        (0x4B0000, 0x4B7FFF, "Switzerland"),
        (0x4B8000, 0x4BFFFF, "Turkey"),
        (0x4CA000, 0x4CAFFF, "Ireland"),
        (0x4CC000, 0x4CCFFF, "Iceland"),
        (0x4D0000, 0x4D03FF, "Luxembourg"),
        (0x4D2000, 0x4D23FF, "Malta"),
        (0x508000, 0x50FFFF, "Ukraine"),
        (0x700000, 0x700FFF, "Afghanistan"),
        (0x702000, 0x702FFF, "Bangladesh"),
        (0x706000, 0x706FFF, "Kuwait"),
        (0x70C000, 0x70C3FF, "Oman"),
        (0x710000, 0x717FFF, "Saudi Arabia"),
        (0x718000, 0x71FFFF, "South Korea"),
        (0x730000, 0x737FFF, "Iran"),
        (0x738000, 0x73FFFF, "Israel"),
        (0x750000, 0x757FFF, "Malaysia"),
        (0x758000, 0x75FFFF, "Philippines"),
        (0x760000, 0x767FFF, "Pakistan"),
        (0x768000, 0x76FFFF, "Singapore"),
        (0x780000, 0x7BFFFF, "China (incl. Hong Kong, Macau)"),
        (0x7C0000, 0x7FFFFF, "Australia"),
        (0x800000, 0x83FFFF, "India"),
        (0x840000, 0x87FFFF, "Japan"),
        (0x880000, 0x887FFF, "Thailand"),
        (0x888000, 0x88FFFF, "Vietnam"),
        (0x894000, 0x894FFF, "Bahrain"),
        (0x895000, 0x8953FF, "Brunei"),
        (0x896000, 0x896FFF, "United Arab Emirates"),
        (0x899000, 0x8993FF, "Taiwan"),
        (0x8A0000, 0x8AFFFF, "Indonesia"),
        (0xA00000, 0xAFFFFF, "United States"),
        (0xC00000, 0xC3FFFF, "Canada"),
        (0xC80000, 0xC87FFF, "New Zealand"),
        (0xE00000, 0xE3FFFF, "Argentina"),
        (0xE40000, 0xE7FFFF, "Brazil"),
        (0xE80000, 0xE80FFF, "Chile"),
    ];
    BLOCKS.iter().find(|(a, b, _)| (*a..=*b).contains(&icao)).map(|(_, _, c)| *c).unwrap_or("unknown country")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn frame(s: &str) -> Frame {
        Frame { bytes: hex(s), signal: -10.0 }
    }

    #[test]
    fn parity() {
        assert_eq!(residual(&hex("8D4840D6202CC371C32CE0576098")), 0);
        assert_ne!(residual(&hex("8D4840D6202CC371C32CE0576099")), 0);
    }

    #[test]
    fn identification() {
        let mut t = Tracker::default();
        let p = t.handle(&frame("8D4840D6202CC371C32CE0576098"), 1).unwrap();
        assert_eq!(p.icao, "4840D6");
        assert_eq!(p.kind, "ident");
        assert_eq!(t.aircraft[&0x4840D6].callsign.as_deref(), Some("KLM1023"));
        assert_eq!(t.aircraft[&0x4840D6].country, "Netherlands");
    }

    #[test]
    fn one_bad_bit_is_corrected() {
        let mut m = hex("8D4840D6202CC371C32CE0576098");
        m[6] ^= 0x10;
        let mut t = Tracker::default();
        let p = t.handle(&Frame { bytes: m, signal: 0.0 }, 1).unwrap();
        assert_eq!(p.crc, "corrected");
        assert_eq!(t.aircraft[&0x4840D6].callsign.as_deref(), Some("KLM1023"));
    }

    #[test]
    fn airborne_position() {
        let mut t = Tracker::default();
        t.handle(&frame("8D40621D58C386435CC412692AD6"), 1000).unwrap(); // odd
        let p = t.handle(&frame("8D40621D58C382D690C8AC2863A7"), 2000).unwrap(); // even, newest
        let a = &t.aircraft[&0x40621D];
        assert_eq!(a.alt, Some(38000));
        assert!((a.lat.unwrap() - 52.2572).abs() < 1e-3, "{:?}", a.lat);
        assert!((a.lon.unwrap() - 3.91937).abs() < 1e-3, "{:?}", a.lon);
        assert_eq!(p.kind, "position");
    }

    #[test]
    fn position_from_one_message_and_the_receiver() {
        let mut t = Tracker::default();
        t.receiver = Some((52.258, 3.918));
        t.handle(&frame("8D40621D58C382D690C8AC2863A7"), 1).unwrap();
        let a = &t.aircraft[&0x40621D];
        assert!((a.lat.unwrap() - 52.2572).abs() < 1e-3 && (a.lon.unwrap() - 3.91937).abs() < 1e-3);
        assert!(a.km.unwrap() < 1.0);
    }

    #[test]
    fn velocity() {
        let mut t = Tracker::default();
        t.handle(&frame("8D485020994409940838175B284F"), 1).unwrap();
        let a = &t.aircraft[&0x485020];
        assert!((a.gs.unwrap() - 159.2).abs() < 0.1, "{:?}", a.gs);
        assert!((a.track.unwrap() - 182.88).abs() < 0.1, "{:?}", a.track);
        assert_eq!(a.vr, Some(-832));
    }

    #[test]
    fn squawks_and_altitudes() {
        // 7700: A=7 B=7 C=0 D=0 → bits A4 A2 A1 B4 B2 B1.
        let id = 0x0800 | 0x0200 | 0x0080 | 0x0020 | 0x0008 | 0x0002;
        assert_eq!(squawk(id), "7700");
        // Q-bit altitude: n = 1560 → 38 000 ft.
        let n: u32 = (38000 + 1000) / 25;
        let a12 = ((n >> 4) << 5) | 0x10 | (n & 0xF);
        assert_eq!(alt12(a12), Some(38000));
        let a13 = ((n >> 5) << 7) | ((n >> 4 & 1) << 5) | 0x10 | (n & 0xF);
        assert_eq!(alt13(a13), Some(38000));
    }

    #[test]
    fn replies_need_a_known_aircraft() {
        let mut t = Tracker::default();
        // A DF4 whose parity is some address nobody has heard: dropped.
        let mut m = vec![0x20, 0x00, 0x06, 0x30, 0, 0, 0];
        let r = residual(&m);
        let aa = 0x4840D6u32;
        let n = m.len();
        m[n - 3] ^= ((r ^ aa) >> 16) as u8;
        m[n - 2] ^= ((r ^ aa) >> 8) as u8;
        m[n - 1] ^= (r ^ aa) as u8;
        assert_eq!(residual(&m), aa);
        assert!(t.handle(&Frame { bytes: m.clone(), signal: 0.0 }, 1).is_none());
        t.handle(&frame("8D4840D6202CC371C32CE0576098"), 1).unwrap();
        let p = t.handle(&Frame { bytes: m, signal: 0.0 }, 2).unwrap();
        assert_eq!(p.icao, "4840D6");
        assert_eq!(p.crc, "address");
    }

    /// Pulse-position samples of a frame as a 2 MS/s dongle would give them.
    fn on_air(bytes: &[u8], amp: f32) -> Vec<u8> {
        let mut mag = vec![0f32; 16];
        for i in [0, 2, 7, 9] {
            mag[i] = amp;
        }
        for i in 0..bytes.len() * 8 {
            let b = bytes[i / 8] >> (7 - i % 8) & 1;
            if b == 1 {
                mag.extend([amp, 0.0]);
            } else {
                mag.extend([0.0, amp]);
            }
        }
        let mut iq = vec![127u8; 400];
        for m in mag {
            iq.push((127.4 + m).round() as u8);
            iq.push(127);
        }
        iq.extend(vec![127u8; 800]);
        iq
    }

    #[test]
    fn finds_frames_in_samples() {
        let mut d = Demod::default();
        let mut found = vec![];
        let air = on_air(&hex("8D4840D6202CC371C32CE0576098"), 60.0);
        // In two pieces, split mid-frame.
        d.feed(&air[..500], &mut found);
        d.feed(&air[500..], &mut found);
        d.feed(&vec![127u8; 1000], &mut found);
        let mut t = Tracker::default();
        let ok: Vec<_> = found.iter().filter_map(|f| t.handle(f, 1)).collect();
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].icao, "4840D6");
    }
}

#[cfg(test)]
mod recording {
    use super::*;

    /// A real recording: `RAD_MODES_FILE=…/modes1.bin cargo test -p rad-sdr recording -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn decodes_a_recording() {
        let path = std::env::var("RAD_MODES_FILE").expect("RAD_MODES_FILE");
        let data = std::fs::read(path).unwrap();
        let mut d = Demod::default();
        let mut t = Tracker::default();
        // Near Malta, where the recording was made (for distances).
        t.receiver = Some((35.85, 14.48));
        let mut frames = vec![];
        let mut packets = vec![];
        for c in data.chunks(32768) {
            frames.clear();
            d.feed(c, &mut frames);
            packets.extend(frames.iter().filter_map(|f| t.handle(f, 1)));
        }
        let mut by: std::collections::BTreeMap<u32, usize> = Default::default();
        for p in &packets {
            *by.entry(p.df).or_default() += 1;
        }
        println!("{} packets ({} candidates rejected), by DF {by:?}", packets.len(), t.bad);
        for a in t.list() {
            println!("  {} {:?} {:?} ft {:?} kt {:?} {:?},{:?} msgs {}", a.icao, a.callsign, a.alt, a.gs, a.track, a.lat, a.lon, a.msgs);
        }
        assert!(packets.len() > 50);
        if let Ok(out) = std::env::var("RAD_MODES_JSON") {
            std::fs::write(out, serde_json::to_string(&serde_json::json!({ "packets": packets, "aircraft": t.list() })).unwrap()).unwrap();
        }
    }
}

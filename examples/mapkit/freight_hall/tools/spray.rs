// spray <dir>: original graffiti for Freight Hall, drawn from strokes. No fonts, no photographs, no names of anybody.
type P = (f32, f32);
struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % 100000) as f32 / 100000.0
    }
}
struct C { w: i32, h: i32, px: Vec<[u8; 4]> }
impl C {
    fn new(w: i32, h: i32) -> C { C { w, h, px: vec![[0; 4]; (w * h) as usize] } }
    fn set(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = [c[0], c[1], c[2], 255];
        }
    }
    fn clear(&mut self, x: i32, y: i32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = [0, 0, 0, 0];
        }
    }
    fn halved(&self) -> C {
        let (w, h) = (self.w / 2, self.h / 2);
        let mut o = C::new(w, h);
        for y in 0..h { for x in 0..w {
            let (mut sum, mut seen) = ([0u32; 3], 0u32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = self.px[((y * 2 + dy) * self.w + x * 2 + dx) as usize];
                if p[3] > 0 { for k in 0..3 { sum[k] += p[k] as u32; } seen += 1; }
            }
            if seen >= 2 { o.set(x, y, [0, 1, 2].map(|k| (sum[k] / seen) as u8)); }
        } }
        o
    }
    fn save(&self, p: &str) {
        let mut o = Vec::with_capacity(self.px.len() * 4);
        for q in &self.px { o.extend_from_slice(q); }
        image::RgbaImage::from_raw(self.w as u32, self.h as u32, o).unwrap().save(p).unwrap();
    }
}
fn glyph(c: char) -> (f32, Vec<Vec<P>>) {
    let v = |a: &[&[P]]| a.iter().map(|s| s.to_vec()).collect::<Vec<_>>();
    match c {
        'A' => (0.7, v(&[&[(0.0,0.0),(0.35,1.0),(0.7,0.0)], &[(0.14,0.38),(0.56,0.38)]])),
        'B' => (0.68, v(&[&[(0.0,0.0),(0.0,1.0),(0.5,1.0),(0.62,0.85),(0.62,0.65),(0.45,0.52),(0.0,0.52)], &[(0.45,0.52),(0.68,0.38),(0.68,0.14),(0.5,0.0),(0.0,0.0)]])),
        'C' => (0.65, v(&[&[(0.65,0.85),(0.45,1.0),(0.2,1.0),(0.0,0.75),(0.0,0.25),(0.2,0.0),(0.45,0.0),(0.65,0.15)]])),
        'D' => (0.65, v(&[&[(0.0,0.0),(0.0,1.0),(0.35,1.0),(0.65,0.75),(0.65,0.25),(0.35,0.0),(0.0,0.0)]])),
        'E' => (0.62, v(&[&[(0.62,1.0),(0.0,1.0),(0.0,0.0),(0.62,0.0)], &[(0.0,0.52),(0.5,0.52)]])),
        'F' => (0.62, v(&[&[(0.62,1.0),(0.0,1.0),(0.0,0.0)], &[(0.0,0.52),(0.5,0.52)]])),
        'G' => (0.65, v(&[&[(0.65,0.85),(0.45,1.0),(0.2,1.0),(0.0,0.75),(0.0,0.25),(0.2,0.0),(0.45,0.0),(0.65,0.2),(0.65,0.45),(0.38,0.45)]])),
        'H' => (0.62, v(&[&[(0.0,0.0),(0.0,1.0)], &[(0.62,0.0),(0.62,1.0)], &[(0.0,0.52),(0.62,0.52)]])),
        'I' => (0.12, v(&[&[(0.06,0.0),(0.06,1.0)]])),
        'K' => (0.62, v(&[&[(0.0,0.0),(0.0,1.0)], &[(0.6,1.0),(0.0,0.45),(0.62,0.0)]])),
        'L' => (0.55, v(&[&[(0.0,1.0),(0.0,0.0),(0.55,0.0)]])),
        'M' => (0.8, v(&[&[(0.0,0.0),(0.0,1.0),(0.4,0.35),(0.8,1.0),(0.8,0.0)]])),
        'N' => (0.65, v(&[&[(0.0,0.0),(0.0,1.0),(0.65,0.0),(0.65,1.0)]])),
        'O' => (0.65, v(&[&[(0.2,0.0),(0.0,0.25),(0.0,0.75),(0.2,1.0),(0.45,1.0),(0.65,0.75),(0.65,0.25),(0.45,0.0),(0.2,0.0)]])),
        'P' => (0.62, v(&[&[(0.0,0.0),(0.0,1.0),(0.45,1.0),(0.62,0.85),(0.62,0.6),(0.45,0.45),(0.0,0.45)]])),
        'R' => (0.65, v(&[&[(0.0,0.0),(0.0,1.0),(0.45,1.0),(0.62,0.85),(0.62,0.6),(0.45,0.45),(0.0,0.45)], &[(0.3,0.45),(0.65,0.0)]])),
        'S' => (0.65, v(&[&[(0.62,0.85),(0.45,1.0),(0.18,1.0),(0.0,0.82),(0.05,0.62),(0.6,0.38),(0.65,0.18),(0.47,0.0),(0.18,0.0),(0.0,0.15)]])),
        'T' => (0.66, v(&[&[(0.0,1.0),(0.66,1.0)], &[(0.33,1.0),(0.33,0.0)]])),
        'U' => (0.65, v(&[&[(0.0,1.0),(0.0,0.25),(0.2,0.0),(0.45,0.0),(0.65,0.25),(0.65,1.0)]])),
        'V' => (0.66, v(&[&[(0.0,1.0),(0.33,0.0),(0.66,1.0)]])),
        'W' => (0.8, v(&[&[(0.0,1.0),(0.2,0.0),(0.4,0.65),(0.6,0.0),(0.8,1.0)]])),
        'X' => (0.62, v(&[&[(0.0,0.0),(0.62,1.0)], &[(0.0,1.0),(0.62,0.0)]])),
        'Y' => (0.66, v(&[&[(0.0,1.0),(0.33,0.48),(0.66,1.0)], &[(0.33,0.48),(0.33,0.0)]])),
        'Z' => (0.62, v(&[&[(0.0,1.0),(0.62,1.0),(0.0,0.0),(0.62,0.0)]])),
        'a' => (0.55, v(&[&[(0.5,0.42),(0.32,0.6),(0.14,0.6),(0.0,0.42),(0.0,0.14),(0.14,0.0),(0.32,0.0),(0.5,0.18)], &[(0.5,0.62),(0.5,0.0)]])),
        't' => (0.45, v(&[&[(0.2,0.95),(0.2,0.12),(0.32,0.0),(0.45,0.02)], &[(0.0,0.62),(0.42,0.62)]])),
        'e' => (0.5, v(&[&[(0.0,0.3),(0.5,0.3),(0.5,0.45),(0.35,0.6),(0.15,0.6),(0.0,0.42),(0.0,0.15),(0.18,0.0),(0.46,0.02)]])),
        '3' => (0.65, v(&[&[(0.0,0.85),(0.2,1.0),(0.45,1.0),(0.62,0.82),(0.45,0.55),(0.28,0.55)], &[(0.45,0.55),(0.65,0.3),(0.47,0.0),(0.2,0.0),(0.0,0.15)]])),
        _ => (0.35, vec![]),
    }
}
fn chaikin(p: &[P], n: u32) -> Vec<P> {
    let mut a = p.to_vec();
    for _ in 0..n {
        if a.len() < 3 { break; }
        let closed = a[0] == *a.last().unwrap();
        let mut o = vec![a[0]];
        for w in a.windows(2) {
            let (s, t) = (w[0], w[1]);
            o.push((s.0 * 0.75 + t.0 * 0.25, s.1 * 0.75 + t.1 * 0.25));
            o.push((s.0 * 0.25 + t.0 * 0.75, s.1 * 0.25 + t.1 * 0.75));
        }
        o.push(*a.last().unwrap());
        if closed { let m = o[1]; o[0] = m; let k = o.len() - 1; o[k] = m; }
        a = o;
    }
    a
}
/// The letters of a word in picture coordinates, each a list of strokes, one letter's strokes together.
fn word(t: &str, x: f32, base: f32, size: f32, gap: f32, slant: f32, bounce: f32, wobble: f32, smooth: u32, r: &mut Rng) -> Vec<Vec<Vec<P>>> {
    let mut at = x;
    let mut out = Vec::new();
    for c in t.chars() {
        let (w, st) = glyph(c);
        let (dy, tilt) = ((r.f() - 0.5) * bounce * size, (r.f() - 0.5) * 0.12);
        let mut letter = Vec::new();
        for s in st {
            let pts: Vec<P> = chaikin(&s, smooth).iter().map(|&(px, py)| {
                let wx = (r.f() - 0.5) * wobble;
                let wy = (r.f() - 0.5) * wobble;
                let yy = py * size;
                (at + px * size + yy * (slant + tilt) + wx * size, base - yy + dy + wy * size)
            }).collect();
            letter.push(pts);
        }
        if !letter.is_empty() { out.push(letter); }
        at += (w + gap) * size;
    }
    out
}
fn dist(s: &[P], x: f32, y: f32) -> f32 {
    let mut best = f32::MAX;
    if s.len() == 1 { return ((x - s[0].0).powi(2) + (y - s[0].1).powi(2)).sqrt(); }
    for w in s.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = dx * dx + dy * dy;
        let t = if l > 0.0 { (((x - a.0) * dx + (y - a.1) * dy) / l).clamp(0.0, 1.0) } else { 0.0 };
        let d = ((x - a.0 - t * dx).powi(2) + (y - a.1 - t * dy).powi(2)).sqrt();
        if d < best { best = d; }
    }
    best
}
/// Paint within `r` of the strokes; `col(x, y)` gives the colour; `rough` is how uneven the edge is.
fn paint(c: &mut C, st: &[Vec<P>], r: f32, rough: f32, rng: &mut Rng, col: &dyn Fn(f32, f32) -> [u8; 3]) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in st { for p in s { x0 = x0.min(p.0); x1 = x1.max(p.0); y0 = y0.min(p.1); y1 = y1.max(p.1); } }
    let m = r + rough + 2.0;
    for y in (y0 - m) as i32..=(y1 + m) as i32 {
        for x in (x0 - m) as i32..=(x1 + m) as i32 {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = st.iter().map(|s| dist(s, fx, fy)).fold(f32::MAX, f32::min);
            if d < r + (rng.f() - 0.5) * rough { c.set(x, y, col(fx, fy)); }
        }
    }
}
fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|k| (a[k] as f32 * (1.0 - t) + b[k] as f32 * t) as u8)
}
fn overspray(c: &mut C, cx: f32, cy: f32, rx: f32, ry: f32, n: usize, col: [u8; 3], rng: &mut Rng) {
    for _ in 0..n {
        let a = rng.f() * 6.283;
        let d = rng.f().sqrt();
        let (x, y) = (cx + a.cos() * d * rx, cy + a.sin() * d * ry);
        if rng.f() < 0.8 { continue; }
        let s = if rng.f() < 0.3 { 4 } else { 3 };
        for dy in 0..s { for dx in 0..s { c.set(x as i32 + dx, y as i32 + dy, col); } }
    }
}
fn drip(c: &mut C, x: f32, y: f32, len: f32, w: f32, col: [u8; 3]) {
    for i in 0..len as i32 {
        let t = i as f32 / len;
        let half = w * (1.0 - t * 0.5);
        for dx in (-half) as i32..=half as i32 { c.set(x as i32 + dx, y as i32 + i, col); }
    }
    for dy in 0..(w * 2.0) as i32 { for dx in (-w) as i32..=w as i32 {
        if ((dx * dx + dy * dy) as f32) < w * w { c.set(x as i32 + dx, (y + len) as i32 + dy - 2, col); }
    } }
}
/// A throw-up: fat letters, each outlined over the one before, a hard shadow, a bright shine and drips.
struct Bubble { fat: f32, line: f32, shadow: [u8; 3], top: [u8; 3], low: [u8; 3], ink: [u8; 3], shine: [u8; 3], off: f32 }
fn bubble(c: &mut C, letters: &[Vec<Vec<P>>], b: &Bubble, rng: &mut Rng) {
    let (mut y0, mut y1) = (f32::MAX, f32::MIN);
    for l in letters { for s in l { for p in s { y0 = y0.min(p.1); y1 = y1.max(p.1); } } }
    let moved: Vec<Vec<Vec<P>>> = letters.iter().map(|l| l.iter().map(|s| s.iter().map(|p| (p.0 + b.off, p.1 + b.off)).collect()).collect()).collect();
    let sh = b.shadow;
    for l in &moved { paint(c, l, b.fat + b.line, 2.0, rng, &|_, _| sh); }
    let (t, lo, ink) = (b.top, b.low, b.ink);
    let span = (y1 - y0 + 2.0 * b.fat).max(1.0);
    let mut bottoms = Vec::new();
    for l in letters.iter() {
        paint(c, l, b.fat + b.line, 2.0, rng, &|_, _| ink);
        paint(c, l, b.fat, 1.5, rng, &|_, y| mix(t, lo, (y - y0 + b.fat) / span));
        let sh = b.shine;
        for s in l {
            let n = s.len();
            if n >= 2 {
                let (a, e) = (s[0], s[(n / 7).max(1)]);
                let seg = vec![(a.0 - b.fat * 0.38, a.1 - b.fat * 0.38), (e.0 - b.fat * 0.38, e.1 - b.fat * 0.38)];
                let len = ((seg[0].0 - seg[1].0).powi(2) + (seg[0].1 - seg[1].1).powi(2)).sqrt();
                if len > b.fat * 0.3 { paint(c, &[seg], b.fat * 0.16, 1.0, rng, &|_, _| sh); }
            }
            for p in s { if p.1 > y1 - 3.0 { bottoms.push(*p); } }
        }
    }
    for (i, p) in bottoms.iter().enumerate() {
        if i % 5 == 0 { let l = 24.0 + rng.f() * 70.0; drip(c, p.0 + rng.f() * 10.0, p.1 + b.fat - 2.0, l, 3.5, mix(t, lo, 1.0)); }
    }
}
fn star(cx: f32, cy: f32, r: f32) -> Vec<P> {
    (0..=10).map(|i| { let a = i as f32 * 0.6283 - 1.5708; let q = if i % 2 == 0 { r } else { r * 0.42 }; (cx + a.cos() * q, cy + a.sin() * q) }).collect()
}
fn crown(cx: f32, cy: f32, s: f32) -> Vec<Vec<P>> {
    vec![vec![(cx - s, cy), (cx - s, cy - s * 0.9), (cx - s * 0.45, cy - s * 0.35), (cx, cy - s * 1.1), (cx + s * 0.45, cy - s * 0.35), (cx + s, cy - s * 0.9), (cx + s, cy), (cx - s, cy)]]
}
fn fill_poly(c: &mut C, poly: &[P], col: [u8; 3]) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in poly { x0 = x0.min(p.0); x1 = x1.max(p.0); y0 = y0.min(p.1); y1 = y1.max(p.1); }
    for y in y0 as i32..=y1 as i32 { for x in x0 as i32..=x1 as i32 {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut inside = false;
        for w in poly.windows(2) {
            if (w[0].1 > fy) != (w[1].1 > fy) && fx < w[0].0 + (fy - w[0].1) / (w[1].1 - w[0].1) * (w[1].0 - w[0].0) { inside = !inside; }
        }
        if inside { c.set(x, y, col); }
    } }
}
fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let h = |i: i32, j: i32| {
        let mut v = (i as u32).wrapping_mul(0x8da6b343) ^ (j as u32).wrapping_mul(0xd8163841) ^ seed.wrapping_mul(0xcb1ab31f);
        v ^= v >> 13; v = v.wrapping_mul(0x5bd1e995); v ^= v >> 15;
        (v % 10000) as f32 / 10000.0
    };
    let (i, j) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - i as f32, y - j as f32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let top = h(i, j) * (1.0 - sx) + h(i + 1, j) * sx;
    let low = h(i, j + 1) * (1.0 - sx) + h(i + 1, j + 1) * sx;
    top * (1.0 - sy) + low * sy
}
/// Years on a wall: muted and uneven paint, flaked away in patches and down the wall, so the wall shows through it.
fn weather(c: &mut C, seed: u32) { weather_by(c, seed, 0.0) }
fn weather_by(c: &mut C, seed: u32, spared: f32) {
    let (w, h) = (c.w, c.h);
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) as usize;
            let p = c.px[at];
            if p[3] == 0 { continue; }
            let (fx, fy) = (x as f32, y as f32);
            let worn = noise(fx / 170.0, fy / 130.0, seed + 1);
            let patch = noise(fx / 34.0, fy / 34.0, seed + 2) * 0.68 + noise(fx / 15.0, fy / 15.0, seed + 3) * 0.32;
            let limit = 0.74 + spared - (worn - 0.5).max(0.0) * 0.5 - (fy / h as f32) * 0.05;
            let flake = noise(fx / 7.0, fy / 120.0, seed + 4) > 0.86 && noise(fx / 60.0, fy / 60.0, seed + 5) > 0.5;
            if patch > limit || flake { c.px[at] = [0, 0, 0, 0]; continue; }
            let l = (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0;
            let streak = if noise(fx / 7.0, fy / 200.0, seed + 6) > 0.7 { 0.8 } else { 1.0 };
            let m = (0.74 + 0.36 * noise(fx / 55.0, fy / 55.0, seed + 7)) * streak;
            for k in 0..3 {
                let v = (l + (p[k] as f32 - l) * 0.6) * m * 0.86 + 16.0;
                c.px[at][k] = v.clamp(0.0, 255.0) as u8;
            }
        }
    }
}
fn main() {
    let d = std::env::args().nth(1).unwrap();
    let mut r = Rng(0x5eed_1337_abcd_0001);
    let black = [18, 18, 22];
    // RUSH: a chrome throw-up with a magenta shadow
    let mut c = C::new(1024, 512);
    overspray(&mut c, 512.0, 250.0, 500.0, 230.0, 2600, [226, 40, 140], &mut r);
    let l = word("RUSH", 70.0, 372.0, 250.0, 0.30, 0.12, 0.10, 0.012, 2, &mut r);
    bubble(&mut c, &l, &Bubble { fat: 45.0, line: 10.0, shadow: [226, 40, 140], top: [250, 250, 250], low: [120, 140, 170], ink: black, shine: [255, 255, 255], off: 16.0 }, &mut r);
    let st = star(965.0, 72.0, 44.0); fill_poly(&mut c, &st, [255, 214, 40]);
    let st = star(60.0, 420.0, 30.0); fill_poly(&mut c, &st, [255, 214, 40]);
    weather(&mut c, 10); c.save(&format!("{d}/gf_rush.png"));
    // 3V3: teal into violet with a yellow line
    let mut c = C::new(1024, 512);
    overspray(&mut c, 512.0, 250.0, 500.0, 240.0, 3000, [40, 200, 200], &mut r);
    let l = word("3V3", 110.0, 395.0, 280.0, 0.5, 0.1, 0.06, 0.01, 2, &mut r);
    bubble(&mut c, &l, &Bubble { fat: 52.0, line: 11.0, shadow: [250, 208, 40], top: [40, 225, 200], low: [120, 40, 200], ink: black, shine: [240, 255, 255], off: 18.0 }, &mut r);
    for (x, y, s) in [(70.0, 90.0, 26.0), (950.0, 420.0, 34.0), (980.0, 70.0, 20.0)] { let st = star(x, y, s); fill_poly(&mut c, &st, [255, 255, 255]); }
    weather(&mut c, 20); c.save(&format!("{d}/gf_three.png"));
    // CREW: a two-tone tag block
    let mut c = C::new(1024, 512);
    let l = word("FREIGHT", 30.0, 215.0, 144.0, 0.42, 0.14, 0.08, 0.012, 2, &mut r);
    bubble(&mut c, &l, &Bubble { fat: 21.0, line: 7.0, shadow: [20, 90, 180], top: [255, 150, 30], low: [230, 50, 40], ink: black, shine: [255, 235, 200], off: 8.0 }, &mut r);
    let l = word("CREW", 260.0, 450.0, 150.0, 0.42, 0.14, 0.08, 0.012, 2, &mut r);
    bubble(&mut c, &l, &Bubble { fat: 21.0, line: 7.0, shadow: [230, 50, 40], top: [90, 220, 255], low: [20, 90, 180], ink: black, shine: [235, 255, 255], off: 8.0 }, &mut r);
    let cr = crown(150.0, 440.0, 56.0); fill_poly(&mut c, &cr[0], [255, 214, 40]); paint(&mut c, &cr, 4.0, 1.0, &mut r, &|_, _| black);
    weather(&mut c, 30); c.halved().save(&format!("{d}/gf_crew.png"));
    // a stencilled skull in a crosshair
    let mut c = C::new(512, 512);
    let (mx, my) = (256.0, 245.0);
    overspray(&mut c, mx, my, 240.0, 235.0, 2200, black, &mut r);
    for y in 0..512 { for x in 0..512 {
        let (dx, dy) = (x as f32 + 0.5 - mx, y as f32 + 0.5 - my);
        let head = (dx / 118.0).powi(2) + ((dy + 30.0) / 112.0).powi(2) < 1.0;
        let jaw = dx.abs() < 66.0 && dy > 40.0 && dy < 122.0;
        let ring = { let q = (dx * dx + dy * dy).sqrt(); q > 192.0 && q < 214.0 };
        let cross = (dx.abs() < 6.0 && dy.abs() > 170.0 && dy.abs() < 244.0) || (dy.abs() < 6.0 && dx.abs() > 170.0 && dx.abs() < 244.0);
        if (head || jaw || ring || cross) && r.f() > 0.012 { c.set(x, y, black); }
    } }
    for y in 0..512 { for x in 0..512 {
        let (dx, dy) = (x as f32 + 0.5 - mx, y as f32 + 0.5 - my);
        let eye = ((dx.abs() - 46.0) / 32.0).powi(2) + ((dy + 22.0) / 36.0).powi(2) < 1.0;
        let nose = dy > 22.0 && dy < 50.0 && dx.abs() < (dy - 18.0) * 0.48;
        let teeth = dy > 78.0 && dy < 122.0 && ((dx + 66.0) % 26.4) < 6.0 && dx.abs() < 66.0;
        if eye || nose || teeth { c.clear(x, y); }
    } }
    weather(&mut c, 40); c.save(&format!("{d}/gf_skull.png"));
    // the hand tag
    let mut c = C::new(1024, 256);
    let l = word("DECOMBAT", 50.0, 180.0, 121.0, 0.2, 0.3, 0.14, 0.02, 2, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    paint(&mut c, &all, 8.5, 2.0, &mut r, &|_, _| [245, 245, 245]);
    let sw = vec![vec![(40.0, 222.0), (420.0, 232.0), (860.0, 214.0), (980.0, 192.0)]];
    paint(&mut c, &sw, 5.0, 1.5, &mut r, &|_, _| [245, 245, 245]);
    let cr = crown(968.0, 96.0, 34.0); paint(&mut c, &cr, 5.0, 1.0, &mut r, &|_, _| [250, 70, 90]);
    for x in [120.0, 340.0, 610.0, 830.0] { drip(&mut c, x, 200.0, 40.0, 2.5, [245, 245, 245]); }
    weather(&mut c, 50); c.save(&format!("{d}/gf_tag.png"));
    // NO CAMPING, stencilled, struck through in red
    let mut c = C::new(1024, 256);
    let l = word("NO CAMPING", 30.0, 185.0, 106.0, 0.34, 0.0, 0.0, 0.0, 0, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    overspray(&mut c, 512.0, 128.0, 505.0, 124.0, 1400, black, &mut r);
    paint(&mut c, &all, 11.0, 1.0, &mut r, &|_, _| black);
    for x in (60..1000).step_by(92) { for y in 40..215 { for dx in 0..3 { if (y / 7) % 3 == 0 { c.clear(x + dx, y); } } } }
    let sl = vec![vec![(30.0, 214.0), (500.0, 120.0), (990.0, 42.0)]];
    paint(&mut c, &sl, 8.0, 3.0, &mut r, &|_, _| [220, 40, 40]);
    weather(&mut c, 60); c.save(&format!("{d}/gf_camp.png"));
    // UP with an arrow, for the stairs
    let mut c = C::new(512, 256);
    overspray(&mut c, 256.0, 128.0, 250.0, 124.0, 800, [250, 208, 40], &mut r);
    let l = word("UP", 40.0, 205.0, 160.0, 0.32, 0.0, 0.0, 0.0, 0, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    paint(&mut c, &all, 17.0, 1.0, &mut r, &|_, _| [250, 208, 40]);
    let ar: Vec<P> = vec![(330.0, 205.0), (330.0, 110.0), (290.0, 110.0), (385.0, 30.0), (480.0, 110.0), (440.0, 110.0), (440.0, 205.0), (330.0, 205.0)];
    fill_poly(&mut c, &ar, [250, 208, 40]);
    weather(&mut c, 70); c.save(&format!("{d}/gf_up.png"));
    // a patch of small marks
    let mut c = C::new(512, 512);
    let pink = [240, 70, 150];
    let st = star(110.0, 110.0, 76.0); fill_poly(&mut c, &st, [255, 214, 40]);
    paint(&mut c, &[st.clone()], 5.0, 1.0, &mut r, &|_, _| black);
    let cr = crown(370.0, 150.0, 74.0); fill_poly(&mut c, &cr[0], pink); paint(&mut c, &cr, 6.0, 1.0, &mut r, &|_, _| black);
    let l = word("XXX", 36.0, 300.0, 96.0, 0.3, 0.1, 0.08, 0.02, 1, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    paint(&mut c, &all, 10.0, 2.0, &mut r, &|_, _| [60, 230, 200]);
    let bolt: Vec<P> = vec![(350.0, 190.0), (430.0, 190.0), (400.0, 240.0), (450.0, 240.0), (340.0, 325.0), (372.0, 260.0), (326.0, 260.0), (350.0, 190.0)];
    fill_poly(&mut c, &bolt, [255, 90, 40]);
    let l = word("PZ Late", 34.0, 462.0, 96.0, 0.16, 0.1, 0.03, 0.006, 2, &mut r);
    bubble(&mut c, &l, &Bubble { fat: 12.0, line: 5.0, shadow: [120, 40, 200], top: [255, 255, 255], low: [255, 180, 60], ink: black, shine: [255, 255, 255], off: 5.0 }, &mut r);
    weather_by(&mut c, 80, 0.14); c.save(&format!("{d}/gf_marks.png"));
    // FLANK, for the corridor that is one
    let mut c = C::new(1024, 512);
    overspray(&mut c, 512.0, 250.0, 500.0, 230.0, 2600, [90, 220, 90], &mut r);
    let l = word("FLANK", 56.0, 355.0, 206.0, 0.3, 0.12, 0.08, 0.012, 2, &mut r);
    bubble(&mut c, &l, &Bubble { fat: 36.0, line: 9.0, shadow: [60, 30, 110], top: [190, 240, 70], low: [20, 150, 130], ink: black, shine: [245, 255, 220], off: 13.0 }, &mut r);
    weather(&mut c, 90); c.save(&format!("{d}/gf_flank.png"));
    // TAKE COVER, on a wall that is cover
    let mut c = C::new(1024, 512);
    let l = word("TAKE", 60.0, 215.0, 150.0, 0.38, 0.1, 0.06, 0.01, 2, &mut r);
    bubble(&mut c, &l, &Bubble { fat: 21.0, line: 7.0, shadow: [15, 30, 60], top: [90, 170, 230], low: [25, 60, 140], ink: black, shine: [225, 240, 255], off: 8.0 }, &mut r);
    let l = word("COVER", 170.0, 450.0, 150.0, 0.38, 0.1, 0.06, 0.01, 2, &mut r);
    bubble(&mut c, &l, &Bubble { fat: 21.0, line: 7.0, shadow: [15, 30, 60], top: [245, 245, 240], low: [130, 140, 160], ink: black, shine: [255, 255, 255], off: 8.0 }, &mut r);
    weather(&mut c, 100); c.halved().save(&format!("{d}/gf_cover.png"));
    // HIGH GROUND, for the other stairs
    let mut c = C::new(512, 256);
    let white = [28, 74, 150];
    overspray(&mut c, 256.0, 128.0, 250.0, 124.0, 800, white, &mut r);
    let l = word("HIGH", 30.0, 112.0, 82.0, 0.3, 0.0, 0.0, 0.0, 0, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    paint(&mut c, &all, 9.0, 1.0, &mut r, &|_, _| white);
    let l = word("GROUND", 30.0, 232.0, 76.0, 0.28, 0.0, 0.0, 0.0, 0, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    paint(&mut c, &all, 8.5, 1.0, &mut r, &|_, _| white);
    let ar: Vec<P> = vec![(350.0, 120.0), (350.0, 70.0), (316.0, 70.0), (390.0, 14.0), (464.0, 70.0), (430.0, 70.0), (430.0, 120.0), (350.0, 120.0)];
    fill_poly(&mut c, &ar, white);
    weather(&mut c, 110); c.save(&format!("{d}/gf_high.png"));
    // WRONG WAY, stencilled red at the end of the corridor
    let mut c = C::new(512, 256);
    let red = [214, 58, 48];
    overspray(&mut c, 256.0, 128.0, 250.0, 124.0, 700, red, &mut r);
    let l = word("WRONG", 24.0, 112.0, 88.0, 0.3, 0.0, 0.0, 0.0, 0, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    paint(&mut c, &all, 9.0, 1.0, &mut r, &|_, _| red);
    let l = word("WAY", 90.0, 232.0, 88.0, 0.3, 0.0, 0.0, 0.0, 0, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    paint(&mut c, &all, 9.0, 1.0, &mut r, &|_, _| red);
    weather(&mut c, 120); c.save(&format!("{d}/gf_wrong.png"));
    // DEAD END, scrawled at the other end
    let mut c = C::new(512, 256);
    let l = word("DEAD", 30.0, 112.0, 92.0, 0.16, 0.26, 0.12, 0.018, 2, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    paint(&mut c, &all, 6.0, 1.5, &mut r, &|_, _| black);
    let l = word("END", 120.0, 226.0, 92.0, 0.16, 0.26, 0.12, 0.018, 2, &mut r);
    let mut all = Vec::new();
    for w in &l { for s in w { all.push(s.clone()); } }
    paint(&mut c, &all, 6.0, 1.5, &mut r, &|_, _| black);
    for x in [70.0, 200.0, 330.0] { drip(&mut c, x, 118.0, 26.0, 2.0, black); }
    weather(&mut c, 130); c.save(&format!("{d}/gf_dead.png"));
}

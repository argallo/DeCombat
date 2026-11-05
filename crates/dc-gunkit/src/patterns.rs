//! Starter patterns, generated here so that a new camo shows something right away and no art has to ship. They all tile.
// DC-DEFAULT: VAR-6 (proposed) our own patterns; nothing from the game goes into them

use crate::design::Art;

/// The presets, in the order the window lists them.
pub const PRESETS: &[(&str, &str, [[u8; 3]; 4])] = &[
    (
        "woodland",
        "Woodland",
        [[58, 70, 42], [92, 96, 58], [40, 34, 26], [122, 112, 78]],
    ),
    (
        "desert",
        "Desert",
        [
            [176, 150, 110],
            [142, 112, 76],
            [196, 178, 140],
            [104, 84, 62],
        ],
    ),
    (
        "urban",
        "Urban",
        [[70, 74, 80], [120, 124, 130], [36, 38, 42], [168, 170, 174]],
    ),
    (
        "digital",
        "Digital",
        [[64, 80, 60], [104, 116, 84], [38, 44, 36], [146, 146, 112]],
    ),
    (
        "tiger",
        "Tiger",
        [[196, 118, 30], [24, 20, 18], [224, 170, 70], [60, 30, 14]],
    ),
    (
        "stars",
        "Stars",
        [[10, 8, 30], [48, 20, 96], [20, 60, 130], [250, 248, 255]],
    ),
];

/// The size of a generated pattern.
pub const SIZE: u32 = 512;

/// A small deterministic generator (xorshift), so a seed always gives the same picture.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Rng {
        Rng(seed.wrapping_mul(2_654_435_761) | 1)
    }

    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }
}

/// Smooth noise that tiles: a lattice of `cells` x `cells` values, wrapped at its edges.
struct Noise {
    cells: usize,
    values: Vec<f32>,
}

impl Noise {
    fn new(cells: usize, rng: &mut Rng) -> Noise {
        Noise {
            cells,
            values: (0..cells * cells).map(|_| rng.next()).collect(),
        }
    }

    /// `u`, `v` in 0..1.
    fn at(&self, u: f32, v: f32) -> f32 {
        let (x, y) = (u * self.cells as f32, v * self.cells as f32);
        let (x0, y0) = (x.floor(), y.floor());
        let ease = |t: f32| t * t * (3.0 - 2.0 * t);
        let (tx, ty) = (ease(x - x0), ease(y - y0));
        let cell = |i: isize, j: isize| {
            let n = self.cells as isize;
            self.values[(j.rem_euclid(n) * n + i.rem_euclid(n)) as usize]
        };
        let (i, j) = (x0 as isize, y0 as isize);
        let top = cell(i, j) + (cell(i + 1, j) - cell(i, j)) * tx;
        let low = cell(i, j + 1) + (cell(i + 1, j + 1) - cell(i, j + 1)) * tx;
        top + (low - top) * ty
    }
}

/// Layers of noise, from large to small, in about 0..1.
fn layered(rng: &mut Rng, cells: usize) -> impl Fn(f32, f32) -> f32 {
    let a = Noise::new(cells, rng);
    let b = Noise::new(cells * 2, rng);
    let c = Noise::new(cells * 4, rng);
    move |u, v| (a.at(u, v) * 4.0 + b.at(u, v) * 2.0 + c.at(u, v)) / 7.0
}

fn put(out: &mut [u8], x: u32, y: u32, c: [u8; 3]) {
    let i = ((y * SIZE + x) * 4) as usize;
    out[i..i + 3].copy_from_slice(&c);
    out[i + 3] = 255;
}

/// Colour bands of a value in 0..1 that fill the picture about equally.
fn band(v: f32, colours: &[[u8; 3]; 4]) -> [u8; 3] {
    let k = match v {
        v if v < 0.42 => 0,
        v if v < 0.5 => 1,
        v if v < 0.58 => 2,
        _ => 3,
    };
    colours[k]
}

/// A preset's picture, with its own colours or the ones given.
pub fn generate(preset: &str, colours: Option<[[u8; 3]; 4]>, seed: u32) -> Result<Art, String> {
    let Some(&(key, _, default)) = PRESETS.iter().find(|p| p.0.eq_ignore_ascii_case(preset)) else {
        let names: Vec<&str> = PRESETS.iter().map(|p| p.0).collect();
        return Err(format!(
            "no preset named {preset:?}; there are {}",
            names.join(", ")
        ));
    };
    let c = colours.unwrap_or(default);
    let mut rng = Rng::new(seed.wrapping_add(1));
    let mut out = vec![255u8; (SIZE * SIZE * 4) as usize];
    let s = SIZE as f32;
    match key {
        "digital" => {
            let f = layered(&mut rng, 5);
            let block = 16;
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let (bx, by) = (x / block * block, y / block * block);
                    put(&mut out, x, y, band(f(bx as f32 / s, by as f32 / s), &c));
                }
            }
        }
        "tiger" => {
            let f = layered(&mut rng, 4);
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let (u, v) = (x as f32 / s, y as f32 / s);
                    let warp = f(u, v) * 4.0;
                    let stripe = ((u * 8.0 + warp) * std::f32::consts::TAU).sin();
                    let k = match stripe {
                        v if v > 0.45 => 1,
                        v if v > 0.25 => 3,
                        v if v > -0.5 => 0,
                        _ => 2,
                    };
                    put(&mut out, x, y, c[k]);
                }
            }
        }
        "stars" => {
            let f = layered(&mut rng, 3);
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let (u, v) = (x as f32 / s, y as f32 / s);
                    let n = ((f(u, v) - 0.3) * 2.5).clamp(0.0, 1.0);
                    let mut px = [0u8; 3];
                    for (k, p) in px.iter_mut().enumerate() {
                        let a = c[0][k] as f32 + (c[1][k] as f32 - c[0][k] as f32) * n;
                        let t = ((n - 0.5) * 2.0).clamp(0.0, 1.0);
                        *p = (a + (c[2][k] as f32 - a) * t) as u8;
                    }
                    put(&mut out, x, y, px);
                }
            }
            for _ in 0..500 {
                let (x, y) = (
                    (rng.next() * s) as u32 % SIZE,
                    (rng.next() * s) as u32 % SIZE,
                );
                let bright = 0.4 + rng.next() * 0.6;
                let px = c[3].map(|v| (v as f32 * bright) as u8);
                put(&mut out, x, y, px);
                if rng.next() > 0.8 {
                    put(&mut out, (x + 1) % SIZE, y, px);
                    put(&mut out, x, (y + 1) % SIZE, px);
                }
            }
        }
        _ => {
            let f = layered(&mut rng, 5);
            for y in 0..SIZE {
                for x in 0..SIZE {
                    put(&mut out, x, y, band(f(x as f32 / s, y as f32 / s), &c));
                }
            }
        }
    }
    Art::from_rgba(SIZE, SIZE, out)
}

/// `rrggbb` as a colour.
pub fn parse_colour(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 || !t.is_ascii() {
        return None;
    }
    let part = |r: std::ops::Range<usize>| u8::from_str_radix(&t[r], 16).ok();
    Some([part(0..2)?, part(2..4)?, part(4..6)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_makes_a_picture_and_a_seed_repeats() {
        for (key, _, _) in PRESETS {
            let a = generate(key, None, 3).unwrap();
            assert_eq!(a, generate(key, None, 3).unwrap(), "{key}");
            assert_ne!(a.rgba, generate(key, None, 4).unwrap().rgba, "{key}");
            assert_eq!(a.rgba.len(), (SIZE * SIZE * 4) as usize);
        }
        assert!(generate("nope", None, 1).unwrap_err().contains("woodland"));
    }

    #[test]
    fn a_pattern_tiles_with_no_hard_seam() {
        let n = Noise::new(5, &mut Rng::new(9));
        for k in 0..20 {
            let v = k as f32 / 20.0;
            assert!((n.at(0.0, v) - n.at(1.0, v)).abs() < 1e-4);
            assert!((n.at(v, 0.0) - n.at(v, 1.0)).abs() < 1e-4);
        }
    }

    #[test]
    fn given_colours_are_the_colours_used() {
        let c = [[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]];
        let a = generate("woodland", Some(c), 1).unwrap();
        for px in a.rgba.as_chunks::<4>().0 {
            assert!(c.iter().any(|k| k[..] == px[..3]), "{px:?}");
        }
    }

    #[test]
    fn a_colour_reads_with_or_without_its_hash() {
        assert_eq!(parse_colour("#ff0080"), Some([255, 0, 128]));
        assert_eq!(parse_colour("00ff00"), Some([0, 255, 0]));
        assert_eq!(parse_colour("nope"), None);
        assert_eq!(parse_colour("\u{e9}\u{e9}\u{e9}"), None);
    }
}

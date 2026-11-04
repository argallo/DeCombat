//! Synthetic fixture builders, written from the layouts in FMT_SPR_LTC_FX.md. No ROM bytes.
#![allow(dead_code)]

pub fn le32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

pub fn spr(frames: &[&str], fps: u32, a: u32, b: u32, key: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for w in [frames.len() as u32, fps, a, b, key] {
        le32(&mut v, w);
    }
    for f in frames {
        v.extend_from_slice(&(f.len() as u16).to_le_bytes());
        v.extend_from_slice(f.as_bytes());
    }
    v
}

pub fn ins(op: u8, a: u8, b: u8, dst: u8) -> [u8; 4] {
    [op, a, b, dst]
}

pub fn tfs(
    names: &[&str],
    coord: u32,
    out: u32,
    upd: u32,
    consts: &[f32],
    code: &[[u8; 4]],
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&1u16.to_le_bytes());
    let mut nb = vec![names.len() as u8];
    for n in names {
        nb.extend_from_slice(n.as_bytes());
        nb.push(0);
    }
    le32(&mut v, nb.len() as u32);
    v.extend_from_slice(&nb);
    for w in [coord, out, upd, consts.len() as u32] {
        le32(&mut v, w);
    }
    for c in consts {
        le32(&mut v, c.to_bits());
    }
    le32(&mut v, (code.len() * 4) as u32);
    for i in code {
        v.extend_from_slice(i);
    }
    v
}

pub fn tfg_two_stage() -> Vec<u8> {
    let mut v = Vec::new();
    le32(&mut v, 1);
    le32(&mut v, 2);
    // stage 0: own script, channel 1, 8 params (only 6 kept)
    le32(&mut v, 2);
    le32(&mut v, 1);
    let path = "dir\\scroll.tfs";
    v.extend_from_slice(&(path.len() as u16).to_le_bytes());
    v.extend_from_slice(path.as_bytes());
    le32(&mut v, 8);
    for i in 0..8 {
        le32(&mut v, (i as f32 + 0.5).to_bits());
    }
    // stage 1: share stage 0
    le32(&mut v, 1);
    le32(&mut v, 2);
    le32(&mut v, 0);
    v
}

/// One text line: label then value(s).
fn l(out: &mut String, label: &str, val: &str) {
    out.push_str(label);
    out.push(' ');
    out.push_str(val);
    out.push('\n');
}

/// A text-encoding .fxf body (no marker needed: tests call `parse_as`). Three keys in one group; key B
/// links to key C which comes later (ordering pass swaps them).
/// Type names are given by the caller as already-built strings.
pub fn fxf_text(type_names: [&str; 3]) -> String {
    let mut s = String::new();
    l(&mut s, "n", "1");
    l(&mut s, "name", "grp one");
    l(&mut s, "keys", "3");
    l(&mut s, "len", "2000");
    for (i, tn) in type_names.iter().enumerate() {
        l(&mut s, "t", &format!("{tn};"));
        l(&mut s, "id", &format!("{}", 10 + i));
        // key index 1 (id 11) linked to id 12; key 2 linked to itself (must be unlinked)
        let (linked, link_id) = match i {
            1 => (1, 12),
            2 => (1, 12),
            _ => (0, 0),
        };
        l(&mut s, "lk", &format!("{linked}"));
        l(&mut s, "lid", &format!("{link_id}"));
        l(&mut s, "att", "node");
        l(&mut s, "s", if i == 0 { "-1.5" } else { "0.25" });
        l(&mut s, "e", if i == 0 { "9.0" } else { "1.0" });
        l(&mut s, "rep", "1");
        l(&mut s, "x", "0");
        l(&mut s, "x", "0.0");
        l(&mut s, "x", "0.0");
        l(&mut s, "pc", if i == 0 { "8" } else { "0" });
        if i == 0 {
            let props: [(&str, u32, &str); 8] = [
                ("p0", 0, "word"),
                ("p1", 1, "-7"),
                ("p2", 2, "1.5"),
                ("p3", 3, "4,a,b"),
                ("p4", 4, "1 2 3"),
                ("p5", 5, "1 2 3 4"),
                ("p6", 6, "0.5 1 0.5 0 1"),
                ("p7", 7, "spr|a.spr"),
            ];
            for (n, t, v) in props {
                l(&mut s, "pn", n);
                l(&mut s, "pt", &t.to_string());
                l(&mut s, "pv", v);
            }
        }
    }
    s
}

fn fixed(v: &mut Vec<u8>, s: &str, n: usize) {
    let mut b = s.as_bytes().to_vec();
    b.resize(n, 0);
    v.extend_from_slice(&b);
}

/// Binary equivalent of a one-group file with one key and every property type.
pub fn fxf_binary(type_name: &str) -> Vec<u8> {
    let mut v = Vec::new();
    le32(&mut v, 1); // groups
    le32(&mut v, 1); // key count
    fixed(&mut v, "bin grp", 128);
    le32(&mut v, 1000); // length ms
    fixed(&mut v, &format!("{type_name};"), 128);
    le32(&mut v, 5); // id
    v.push(1); // linked
    v.extend_from_slice(&[0, 0, 0]);
    le32(&mut v, 6); // link id
    fixed(&mut v, "attach", 32);
    for f in [0.1f32, 0.9] {
        le32(&mut v, f.to_bits());
    }
    le32(&mut v, 1); // repeat
    le32(&mut v, 0);
    le32(&mut v, 0);
    le32(&mut v, 0);
    le32(&mut v, 8); // props
    let mut prop = |name: &str, ty: u32, val: &[u8]| {
        v.push(name.len() as u8);
        v.extend_from_slice(name.as_bytes());
        le32(&mut v, ty);
        v.extend_from_slice(val);
    };
    let mut w = Vec::new();
    fixed(&mut w, "str", 128);
    prop("a", 0, &w);
    prop("b", 1, &(-3i32).to_le_bytes());
    prop("c", 2, &2.5f32.to_le_bytes());
    let mut w = Vec::new();
    fixed(&mut w, "2,x,y", 128);
    prop("d", 3, &w);
    let f = |xs: &[f32]| xs.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    prop("e", 4, &f(&[1.0, 2.0, 3.0]));
    prop("f", 5, &f(&[1.0, 2.0, 3.0, 4.0]));
    let mut w = 0.5f32.to_le_bytes().to_vec();
    w.extend_from_slice(&0xAABBCCDDu32.to_le_bytes());
    prop("g", 6, &w);
    let mut w = Vec::new();
    fixed(&mut w, "dtx|x.dtx", 128);
    prop("h", 7, &w);
    v
}

pub fn fcf_text() -> String {
    let mut s = String::new();
    l(&mut s, "c", "2"); // root block
    for (name, img, kids) in [("folder", 0, true), ("effect", 2, false)] {
        l(&mut s, "n", name);
        l(&mut s, "i", &img.to_string());
        l(&mut s, "si", &img.to_string());
        l(&mut s, "h", if kids { "1" } else { "0" });
        l(&mut s, "st", "0");
        if kids {
            l(&mut s, "c", "1");
            l(&mut s, "n", "leaf");
            l(&mut s, "i", "2");
            l(&mut s, "si", "2");
            l(&mut s, "h", "0");
            l(&mut s, "st", "0");
        }
    }
    l(&mut s, "w", "1");
    l(&mut s, "wn", "leaf");
    for v in ["10", "20", "300", "400"] {
        l(&mut s, "r", v);
    }
    s
}

/// xorshift64 for deterministic fuzzing.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Mutate a seed buffer: byte flips, truncation, splices, duplicated spans, interesting values.
pub fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..1 + rng.below(4) {
        match rng.below(6) {
            0 if !b.is_empty() => {
                let i = rng.below(b.len());
                b[i] ^= 1 << rng.below(8);
            }
            1 if !b.is_empty() => {
                let i = rng.below(b.len());
                b[i] = [0u8, 0xFF, 0x7F, 0x80, b'9', b'-', b';', b'\n'][rng.below(8)];
            }
            2 => {
                let n = rng.below(b.len() + 1);
                b.truncate(n);
            }
            3 if !b.is_empty() => {
                let i = rng.below(b.len());
                let n = rng.below(16).min(b.len() - i);
                let span: Vec<u8> = b[i..i + n].to_vec();
                let at = rng.below(b.len() + 1);
                for (k, x) in span.into_iter().enumerate() {
                    b.insert(at + k, x);
                }
            }
            4 if b.len() >= 4 => {
                let i = rng.below(b.len() - 3);
                let w = [0u32, 1, 0xFFFF_FFFF, 0x8000_0000, 0x1000, 0x7FFF_FFFF][rng.below(6)];
                b[i..i + 4].copy_from_slice(&w.to_le_bytes());
            }
            _ => b.push(rng.next() as u8),
        }
    }
    b
}

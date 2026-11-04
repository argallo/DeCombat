//! Minimal TrueType reader: enough to turn the ASCII characters of a `glyf`-outline font into contours for the
//! debug overlay and the menu text. Not a general font engine: no hinting instructions, no kerning, no CFF outlines.
//!
//! PUBLIC: OpenType / TrueType specification: collection header (`ttcf`), table directory, `head`
//! (unitsPerEm at +18, indexToLocFormat at +50), `maxp` (numGlyphs at +4), `OS/2` (usWinAscent +74, usWinDescent
//! +76), `hhea` (ascender +4, descender +6,
//! lineGap +8, numberOfHMetrics +34), `hmtx`, `cmap` (encoding records; subtable formats 4 and 12), `loca`
//! (short = offset / 2 as u16, long = u32), `glyf` (simple glyphs: end points, instructions, flags with the
//! repeat bit, delta-coded coordinates; composite glyphs: component flags, word / byte arguments, F2Dot14
//! scale forms), `gasp` (version, numRanges, then rangeMaxPPEM / rangeGaspBehavior pairs; bit 1 GASP_DOGRAY),
//! `VDMX` (version, numRecs, numRatios, ratio records, group offsets; group: recs, startsz, endsz, then
//! yPelHeight / yMax / yMin records), `hdmx` (version, numRecords, sizeDeviceRecord; records: pixelSize,
//! maxWidth, one width byte per glyph).

/// Why a font could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontError {
    NotSfnt,
    NoSuchFace,
    MissingTable(&'static str),
    /// PostScript (CFF) outlines; this reader handles `glyf` only.
    CffOutlines,
    NoUnicodeMap,
    Malformed(&'static str),
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FontError::NotSfnt => f.write_str("not a TrueType / OpenType file"),
            FontError::NoSuchFace => f.write_str("face index out of range"),
            FontError::MissingTable(t) => write!(f, "missing table {t}"),
            FontError::CffOutlines => f.write_str("CFF outlines are not supported"),
            FontError::NoUnicodeMap => f.write_str("no usable character map"),
            FontError::Malformed(w) => write!(f, "malformed {w}"),
        }
    }
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o.checked_add(2)?)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn i16_at(d: &[u8], o: usize) -> Option<i16> {
    u16_at(d, o).map(|v| v as i16)
}

/// `gasp` ranges at `o`.
fn read_gasp(d: &[u8], o: usize) -> Vec<(u16, u16)> {
    let n = u16_at(d, o + 2).unwrap_or(0) as usize;
    (0..n)
        .map_while(|i| Some((u16_at(d, o + 4 + 4 * i)?, u16_at(d, o + 6 + 4 * i)?)))
        .collect()
}

/// `VDMX` records of the first group whose ratio is 1:1 or "any" (bCharSet, xRatio, yStartRatio, yEndRatio).
fn read_vdmx(d: &[u8], o: usize) -> Vec<(u16, i16, i16)> {
    let Some(ratios) = u16_at(d, o + 4) else {
        return Vec::new();
    };
    let ratios = ratios as usize;
    let pick = (0..ratios).find(|&i| {
        let r = o + 6 + 4 * i;
        let (x, y0, y1) = (d.get(r + 1), d.get(r + 2), d.get(r + 3));
        matches!((x, y0, y1), (Some(0), Some(0), Some(0)))
            || matches!((x, y0, y1), (Some(&x), Some(&y0), Some(&y1)) if y0 <= x && x <= y1)
    });
    let Some(i) = pick else {
        return Vec::new();
    };
    let Some(g) = u16_at(d, o + 6 + 4 * ratios + 2 * i) else {
        return Vec::new();
    };
    let g = o + g as usize;
    let n = u16_at(d, g).unwrap_or(0) as usize;
    (0..n)
        .map_while(|k| {
            let r = g + 4 + 6 * k;
            Some((u16_at(d, r)?, i16_at(d, r + 2)?, i16_at(d, r + 4)?))
        })
        .collect()
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o.checked_add(4)?)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn f2dot14(d: &[u8], o: usize) -> Option<f32> {
    i16_at(d, o).map(|v| v as f32 / 16384.0)
}

/// A glyph outline in font units (y up): closed contours of `(x, y, on_curve)` points.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outline {
    pub contours: Vec<Vec<(f32, f32, bool)>>,
}

impl Outline {
    pub fn is_empty(&self) -> bool {
        self.contours.iter().all(|c| c.is_empty())
    }

    /// `(min_x, min_y, max_x, max_y)` of the points, `None` when empty.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut it = self.contours.iter().flatten();
        let first = it.next()?;
        Some(it.fold((first.0, first.1, first.0, first.1), |b, p| {
            (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))
        }))
    }
}

#[derive(Debug, Clone, Copy)]
enum CharMap {
    Format4(usize),
    Format12(usize),
}

/// A parsed `glyf` font face.
#[derive(Debug, Clone)]
pub struct Font {
    data: Vec<u8>,
    glyf: (usize, usize),
    loca: usize,
    loca_long: bool,
    hmtx: usize,
    num_hmetrics: u16,
    num_glyphs: u16,
    cmap: CharMap,
    symbol: bool,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    /// `OS/2` usWinAscent / usWinDescent (0 / 0 without an `OS/2` table): the GDI cell of the face.
    pub win_ascent: u16,
    pub win_descent: u16,
    /// `gasp` ranges (rangeMaxPPEM, rangeGaspBehavior), ascending; empty without the table.
    gasp: Vec<(u16, u16)>,
    /// `VDMX` records of the 1:1 (or any-ratio) group: (yPelHeight, yMax, yMin).
    vdmx: Vec<(u16, i16, i16)>,
    /// `hdmx` table (offset, records, record size); records 0 without the table.
    hdmx: (usize, usize, usize),
}

impl Font {
    /// Parses face `face` of a font file (`face` must be 0 for a single-face file).
    /// The whole file the font was parsed from (all faces of a collection).
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    pub fn parse(data: Vec<u8>, face: usize) -> Result<Font, FontError> {
        let tag = data.get(0..4).ok_or(FontError::NotSfnt)?;
        let dir = if tag == b"ttcf" {
            let n = u32_at(&data, 8).ok_or(FontError::NotSfnt)? as usize;
            if face >= n {
                return Err(FontError::NoSuchFace);
            }
            u32_at(&data, 12 + 4 * face).ok_or(FontError::NotSfnt)? as usize
        } else if tag == [0, 1, 0, 0] || tag == b"true" {
            if face != 0 {
                return Err(FontError::NoSuchFace);
            }
            0
        } else if tag == b"OTTO" {
            return Err(FontError::CffOutlines);
        } else {
            return Err(FontError::NotSfnt);
        };
        let num_tables = u16_at(&data, dir + 4).ok_or(FontError::NotSfnt)? as usize;
        let find = |want: &[u8; 4]| -> Option<(usize, usize)> {
            (0..num_tables).find_map(|i| {
                let r = dir + 12 + 16 * i;
                if data.get(r..r + 4)? == want {
                    let off = u32_at(&data, r + 8)? as usize;
                    let len = u32_at(&data, r + 12)? as usize;
                    data.get(off..off.checked_add(len)?)?;
                    Some((off, len))
                } else {
                    None
                }
            })
        };
        if find(b"CFF ").is_some() && find(b"glyf").is_none() {
            return Err(FontError::CffOutlines);
        }
        let head = find(b"head").ok_or(FontError::MissingTable("head"))?.0;
        let maxp = find(b"maxp").ok_or(FontError::MissingTable("maxp"))?.0;
        let hhea = find(b"hhea").ok_or(FontError::MissingTable("hhea"))?.0;
        let hmtx = find(b"hmtx").ok_or(FontError::MissingTable("hmtx"))?.0;
        let loca = find(b"loca").ok_or(FontError::MissingTable("loca"))?.0;
        let glyf = find(b"glyf").ok_or(FontError::MissingTable("glyf"))?;
        let cmap = find(b"cmap").ok_or(FontError::MissingTable("cmap"))?.0;
        let bad = |w| FontError::Malformed(w);
        let units_per_em = u16_at(&data, head + 18).ok_or(bad("head"))?;
        if units_per_em == 0 {
            return Err(bad("head"));
        }
        let loca_long = i16_at(&data, head + 50).ok_or(bad("head"))? != 0;
        let num_glyphs = u16_at(&data, maxp + 4).ok_or(bad("maxp"))?;
        let ascender = i16_at(&data, hhea + 4).ok_or(bad("hhea"))?;
        let descender = i16_at(&data, hhea + 6).ok_or(bad("hhea"))?;
        let line_gap = i16_at(&data, hhea + 8).ok_or(bad("hhea"))?;
        let num_hmetrics = u16_at(&data, hhea + 34).ok_or(bad("hhea"))?;
        // PUBLIC: OpenType `OS/2` usWinAscent at +74, usWinDescent at +76
        let (win_ascent, win_descent) = find(b"OS/2")
            .and_then(|(o, _)| Some((u16_at(&data, o + 74)?, u16_at(&data, o + 76)?)))
            .unwrap_or((0, 0));
        let (map, symbol) = choose_cmap(&data, cmap).ok_or(FontError::NoUnicodeMap)?;
        let gasp = find(b"gasp").map_or_else(Vec::new, |(o, _)| read_gasp(&data, o));
        let vdmx = find(b"VDMX").map_or_else(Vec::new, |(o, _)| read_vdmx(&data, o));
        let hdmx = find(b"hdmx")
            .and_then(|(o, _)| {
                let n = i16_at(&data, o + 2)?.max(0) as usize;
                Some((o, n, u32_at(&data, o + 4)? as usize))
            })
            .unwrap_or((0, 0, 0));
        Ok(Font {
            data,
            glyf,
            loca,
            loca_long,
            hmtx,
            num_hmetrics,
            num_glyphs,
            cmap: map,
            symbol,
            units_per_em,
            ascender,
            descender,
            line_gap,
            win_ascent,
            win_descent,
            gasp,
            vdmx,
            hdmx,
        })
    }

    /// The device advance of glyph `g` at `ppem` from the face's `hdmx` table (whole pixels), if listed.
    pub fn hdmx_advance(&self, ppem: u16, g: u16) -> Option<u16> {
        let (o, n, size) = self.hdmx;
        if g >= self.num_glyphs {
            return None;
        }
        (0..n).find_map(|i| {
            let r = o + 8 + i * size;
            (*self.data.get(r)? as u16 == ppem)
                .then(|| self.data.get(r + 2 + g as usize).map(|&w| w as u16))
                .flatten()
        })
    }

    /// Whether the face asks for grey-scale (anti-aliased) rendering at `ppem` (`gasp` GASP_DOGRAY); a face
    /// without a `gasp` table: yes.
    pub fn gasp_grayscale(&self, ppem: u16) -> bool {
        if self.gasp.is_empty() {
            return true;
        }
        self.gasp
            .iter()
            .find(|(max, _)| ppem <= *max)
            .is_some_and(|(_, b)| b & 2 != 0)
    }

    /// The face's device ascent / descent (pixels, both positive) at `ppem` from its `VDMX` table, if listed.
    pub fn vdmx(&self, ppem: u16) -> Option<(i16, i16)> {
        self.vdmx
            .iter()
            .find(|r| r.0 == ppem)
            .map(|&(_, max, min)| (max, -min))
    }

    pub fn num_glyphs(&self) -> u16 {
        self.num_glyphs
    }

    /// Glyph index of `ch` (0 = missing glyph). Symbol-encoded faces are also tried at `0xF000 + ch`.
    pub fn glyph_index(&self, ch: char) -> u16 {
        let c = ch as u32;
        let g = self.lookup(c);
        if g == 0 && self.symbol && c < 0x100 {
            return self.lookup(0xF000 + c);
        }
        g
    }

    fn lookup(&self, c: u32) -> u16 {
        let d = &self.data;
        let g = match self.cmap {
            CharMap::Format4(sub) => cmap4(d, sub, c),
            CharMap::Format12(sub) => cmap12(d, sub, c),
        };
        let g = g.unwrap_or(0);
        if g < self.num_glyphs {
            g
        } else {
            0
        }
    }

    /// Advance width in font units.
    pub fn advance(&self, g: u16) -> u16 {
        let n = self.num_hmetrics.max(1);
        let i = g.min(n - 1) as usize;
        u16_at(&self.data, self.hmtx + 4 * i).unwrap_or(0)
    }

    fn glyph_range(&self, g: u16) -> Option<(usize, usize)> {
        if g >= self.num_glyphs {
            return None;
        }
        let (a, b) = if self.loca_long {
            (
                u32_at(&self.data, self.loca + 4 * g as usize)? as usize,
                u32_at(&self.data, self.loca + 4 * (g as usize + 1))? as usize,
            )
        } else {
            (
                u16_at(&self.data, self.loca + 2 * g as usize)? as usize * 2,
                u16_at(&self.data, self.loca + 2 * (g as usize + 1))? as usize * 2,
            )
        };
        if b < a || b > self.glyf.1 {
            return None;
        }
        Some((self.glyf.0 + a, b - a))
    }

    /// The outline of glyph `g` (empty for blank glyphs such as the space, `None` when malformed).
    pub fn outline(&self, g: u16) -> Option<Outline> {
        let mut out = Outline::default();
        self.outline_into(g, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut out)?;
        Some(out)
    }

    /// Appends glyph `g` transformed by `m = [a, b, c, d, dx, dy]` (x' = a x + c y + dx, y' = b x + d y + dy).
    fn outline_into(&self, g: u16, m: [f32; 6], depth: u32, out: &mut Outline) -> Option<()> {
        if depth > 8 {
            return None;
        }
        let (start, len) = self.glyph_range(g)?;
        if len == 0 {
            return Some(());
        }
        let d = &self.data;
        let n = i16_at(d, start)?;
        let tf = |x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        if n >= 0 {
            let n = n as usize;
            let mut ends = Vec::with_capacity(n);
            for i in 0..n {
                ends.push(u16_at(d, start + 10 + 2 * i)? as usize);
            }
            let npts = ends.last().map_or(0, |e| e + 1);
            if npts > 0xffff {
                return None;
            }
            let ninstr = u16_at(d, start + 10 + 2 * n)? as usize;
            let mut p = start + 12 + 2 * n + ninstr;
            let mut flags = Vec::with_capacity(npts);
            while flags.len() < npts {
                let f = *d.get(p)?;
                p += 1;
                flags.push(f);
                if f & 0x08 != 0 {
                    let rep = *d.get(p)?;
                    p += 1;
                    for _ in 0..rep {
                        flags.push(f);
                    }
                }
            }
            flags.truncate(npts);
            let mut read_axis = |short: u8, same: u8| -> Option<Vec<f32>> {
                let mut v = 0i32;
                let mut out = Vec::with_capacity(npts);
                for &f in &flags {
                    if f & short != 0 {
                        let b = *d.get(p)? as i32;
                        p += 1;
                        v += if f & same != 0 { b } else { -b };
                    } else if f & same == 0 {
                        v += i16_at(d, p)? as i32;
                        p += 2;
                    }
                    out.push(v as f32);
                }
                Some(out)
            };
            let xs = read_axis(0x02, 0x10)?;
            let ys = read_axis(0x04, 0x20)?;
            let mut first = 0;
            for &e in &ends {
                if e < first || e >= npts {
                    return None;
                }
                let c: Vec<(f32, f32, bool)> = (first..=e)
                    .map(|i| {
                        let (x, y) = tf(xs[i], ys[i]);
                        (x, y, flags[i] & 0x01 != 0)
                    })
                    .collect();
                out.contours.push(c);
                first = e + 1;
            }
            Some(())
        } else {
            let mut p = start + 10;
            loop {
                let flags = u16_at(d, p)?;
                let child = u16_at(d, p + 2)?;
                p += 4;
                let (a1, a2) = if flags & 0x0001 != 0 {
                    let v = (i16_at(d, p)? as f32, i16_at(d, p + 2)? as f32);
                    p += 4;
                    v
                } else {
                    let v = (*d.get(p)? as i8 as f32, *d.get(p + 1)? as i8 as f32);
                    p += 2;
                    v
                };
                // point-matching placement (args are point numbers) is not supported: offset 0
                let (dx, dy) = if flags & 0x0002 != 0 {
                    (a1, a2)
                } else {
                    (0.0, 0.0)
                };
                let (mut a, mut b, mut c, mut dd) = (1.0, 0.0, 0.0, 1.0);
                if flags & 0x0008 != 0 {
                    a = f2dot14(d, p)?;
                    dd = a;
                    p += 2;
                } else if flags & 0x0040 != 0 {
                    a = f2dot14(d, p)?;
                    dd = f2dot14(d, p + 2)?;
                    p += 4;
                } else if flags & 0x0080 != 0 {
                    a = f2dot14(d, p)?;
                    b = f2dot14(d, p + 2)?;
                    c = f2dot14(d, p + 4)?;
                    dd = f2dot14(d, p + 6)?;
                    p += 8;
                }
                // child transform, then the parent's
                let (ox, oy) = tf(dx, dy);
                let cm = [
                    m[0] * a + m[2] * b,
                    m[1] * a + m[3] * b,
                    m[0] * c + m[2] * dd,
                    m[1] * c + m[3] * dd,
                    ox,
                    oy,
                ];
                self.outline_into(child, cm, depth + 1, out)?;
                if flags & 0x0020 == 0 {
                    break;
                }
            }
            Some(())
        }
    }
}

/// Picks a Unicode subtable: (3,10) or (0,*) format 12 first, then (3,1) or (0,*) format 4, then the
/// symbol encoding (3,0) format 4.
fn choose_cmap(d: &[u8], cmap: usize) -> Option<(CharMap, bool)> {
    let n = u16_at(d, cmap + 2)? as usize;
    let mut best: Option<(u8, CharMap, bool)> = None;
    for i in 0..n {
        let r = cmap + 4 + 8 * i;
        let (Some(pid), Some(eid), Some(off)) = (u16_at(d, r), u16_at(d, r + 2), u32_at(d, r + 4))
        else {
            break;
        };
        let sub = cmap + off as usize;
        let Some(fmt) = u16_at(d, sub) else {
            continue;
        };
        let rank = match (pid, eid, fmt) {
            (3, 10, 12) | (0, _, 12) => 3,
            (3, 1, 4) | (0, _, 4) => 2,
            (3, 0, 4) => 1,
            _ => 0,
        };
        if rank == 0 || best.as_ref().is_some_and(|b| b.0 >= rank) {
            continue;
        }
        let map = if fmt == 12 {
            CharMap::Format12(sub)
        } else {
            CharMap::Format4(sub)
        };
        best = Some((rank, map, rank == 1));
    }
    best.map(|b| (b.1, b.2))
}

fn cmap4(d: &[u8], sub: usize, c: u32) -> Option<u16> {
    if c > 0xffff {
        return Some(0);
    }
    let c = c as u16;
    let segs = u16_at(d, sub + 6)? as usize / 2;
    let ends = sub + 14;
    let starts = ends + 2 * segs + 2;
    let deltas = starts + 2 * segs;
    let ranges = deltas + 2 * segs;
    for i in 0..segs {
        let end = u16_at(d, ends + 2 * i)?;
        if c > end {
            continue;
        }
        let start = u16_at(d, starts + 2 * i)?;
        if c < start {
            return Some(0);
        }
        let delta = u16_at(d, deltas + 2 * i)?;
        let ro = u16_at(d, ranges + 2 * i)? as usize;
        if ro == 0 {
            return Some(c.wrapping_add(delta));
        }
        let at = ranges + 2 * i + ro + 2 * (c - start) as usize;
        let g = u16_at(d, at)?;
        return Some(if g == 0 { 0 } else { g.wrapping_add(delta) });
    }
    Some(0)
}

fn cmap12(d: &[u8], sub: usize, c: u32) -> Option<u16> {
    let n = u32_at(d, sub + 12)? as usize;
    for i in 0..n.min(1 << 20) {
        let g = sub + 16 + 12 * i;
        let (s, e, first) = (u32_at(d, g)?, u32_at(d, g + 4)?, u32_at(d, g + 8)?);
        if c >= s && c <= e {
            return Some((first + (c - s)) as u16);
        }
    }
    Some(0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use crate::testkit::synth_font;

    #[test]
    fn parses_the_made_up_font() {
        let f = Font::parse(synth_font(), 0).expect("font");
        assert_eq!(f.units_per_em, 1000);
        assert_eq!((f.ascender, f.descender), (800, -200));
        assert_eq!(f.glyph_index('A'), 1);
        assert_eq!(f.glyph_index('B'), 2);
        assert_eq!(f.glyph_index('C'), 0);
        assert_eq!(f.advance(1), 800);
        let o = f.outline(1).unwrap();
        assert_eq!(o.contours.len(), 2);
        assert_eq!(o.contours[0].len(), 5);
        assert!(!o.contours[0][2].2, "third point is off-curve");
        assert_eq!(o.bounds(), Some((100.0, 0.0, 700.0, 700.0)));
        let c = f.outline(2).unwrap();
        assert_eq!(c.contours.len(), 2);
        assert_eq!(c.bounds(), Some((100.0, 10.0, 400.0, 360.0)));
        assert!(f.outline(0).unwrap().is_empty());
    }

    #[test]
    fn rejects_hostile_input() {
        assert_eq!(
            Font::parse(b"OTTO....".to_vec(), 0).unwrap_err(),
            FontError::CffOutlines
        );
        assert_eq!(
            Font::parse(b"abcd".to_vec(), 0).unwrap_err(),
            FontError::NotSfnt
        );
        let good = synth_font();
        for cut in [0usize, 4, 12, 40, 100, good.len() / 2, good.len() - 1] {
            if let Ok(f) = Font::parse(good[..cut].to_vec(), 0) {
                for ch in ['A', 'B', ' '] {
                    let _ = f.outline(f.glyph_index(ch));
                }
            }
        }
        // flipping every byte of the glyph data must not panic
        let f = Font::parse(good.clone(), 0).unwrap();
        let (gs, gl) = f.glyf;
        for i in gs..gs + gl {
            let mut bad = good.clone();
            bad[i] ^= 0xff;
            if let Ok(f) = Font::parse(bad, 0) {
                let _ = f.outline(1);
                let _ = f.outline(2);
            }
        }
    }
}

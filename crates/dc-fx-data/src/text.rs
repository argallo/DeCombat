//! Line model shared by the text encodings of `.fxf` and `.fcf`. SPEC: FMT_SPR_LTC_FX.md §5.3.
//!
//! A reader takes up to 0x800 bytes from the current position, cuts at the first newline, and advances by
//! the cut length plus one. The first whitespace-delimited token of a line is its label (never compared);
//! values follow. A value that fails to convert leaves the destination at zero, as the pre-zeroed C
//! destinations do. // EVIDENCE: cshell+0x29b350 (and its five siblings)

use crate::error::FxError;

pub(crate) const LINE_BUF: usize = 0x800;

pub(crate) struct Lines<'a> {
    data: &'a [u8],
    pos: usize,
}

/// One line split into tokens (label first).
pub(crate) struct Line<'a> {
    toks: Vec<&'a [u8]>,
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

impl<'a> Lines<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Lines { data, pos: 0 }
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn next_line(&mut self, what: &'static str) -> Result<Line<'a>, FxError> {
        if self.at_end() {
            return Err(FxError::Truncated {
                what,
                offset: self.pos,
            });
        }
        let avail = &self.data[self.pos..];
        let buf = &avail[..avail.len().min(LINE_BUF)];
        let cut = buf.iter().position(|&c| c == b'\n').unwrap_or(buf.len());
        let mut line = &buf[..cut];
        // C string semantics: the scanner stops at the first NUL.
        if let Some(z) = line.iter().position(|&c| c == 0) {
            line = &line[..z];
        }
        self.pos = (self.pos + cut + 1).min(self.data.len());
        let toks = line
            .split(|&c| is_space(c))
            .filter(|t| !t.is_empty())
            .collect();
        Ok(Line { toks })
    }

    /// True when only whitespace remains.
    pub fn only_blank_left(&self) -> bool {
        self.data[self.pos.min(self.data.len())..]
            .iter()
            .all(|&c| is_space(c) || c == 0)
    }
}

fn parse_sign(t: &[u8]) -> (bool, &[u8]) {
    match t.first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    }
}

fn digits_value(t: &[u8], base: u32) -> Option<(u64, usize)> {
    let mut v: u64 = 0;
    let mut n = 0;
    for &c in t {
        let Some(d) = (c as char).to_digit(base) else {
            break;
        };
        v = v.wrapping_mul(base as u64).wrapping_add(d as u64);
        n += 1;
    }
    if n == 0 {
        None
    } else {
        Some((v, n))
    }
}

/// C `%lu` on a 32-bit `long`: optional sign, decimal digits, wraps.
pub(crate) fn scan_unsigned(t: &[u8]) -> Option<u32> {
    let (neg, rest) = parse_sign(t);
    let (v, _) = digits_value(rest, 10)?;
    let v = v as u32;
    Some(if neg { v.wrapping_neg() } else { v })
}

/// C `%i`: sign, then `0x`/`0X` hex, leading `0` octal, else decimal.
pub(crate) fn scan_int(t: &[u8]) -> Option<i32> {
    let (neg, rest) = parse_sign(t);
    let v = if rest.len() > 2
        && rest[0] == b'0'
        && (rest[1] | 0x20) == b'x'
        && digits_value(&rest[2..], 16).is_some()
    {
        digits_value(&rest[2..], 16)?.0
    } else if rest.first() == Some(&b'0') {
        digits_value(rest, 8)?.0
    } else {
        digits_value(rest, 10)?.0
    } as u32 as i32;
    Some(if neg { v.wrapping_neg() } else { v })
}

/// C `%f`: the longest prefix that is a decimal float.
pub(crate) fn scan_float(t: &[u8]) -> Option<f32> {
    let s = std::str::from_utf8(t).ok().or_else(|| {
        // keep the ASCII prefix of a token with stray high bytes
        let n = t.iter().position(|c| !c.is_ascii()).unwrap_or(0);
        std::str::from_utf8(&t[..n]).ok()
    })?;
    let mut end = s.len();
    while end > 0 {
        if let Some(sub) = s.get(..end) {
            if let Ok(v) = sub.parse::<f32>() {
                return Some(v);
            }
        }
        end -= 1;
    }
    None
}

impl<'a> Line<'a> {
    /// `%s %s`: first token after the label, Latin-1 to String; empty when missing.
    pub fn word(&self) -> String {
        self.toks
            .get(1)
            .map(|t| t.iter().map(|&c| c as char).collect())
            .unwrap_or_default()
    }
    pub fn unsigned(&self) -> u32 {
        self.toks.get(1).and_then(|t| scan_unsigned(t)).unwrap_or(0)
    }
    pub fn int(&self) -> i32 {
        self.toks.get(1).and_then(|t| scan_int(t)).unwrap_or(0)
    }
    pub fn float(&self) -> f32 {
        self.floats::<1>()[0]
    }
    /// `%s %f %f ...`: stops at the first failed conversion, leaving the rest zero.
    pub fn floats<const N: usize>(&self) -> [f32; N] {
        let mut out = [0.0f32; N];
        for (i, slot) in out.iter_mut().enumerate() {
            match self.toks.get(1 + i).and_then(|t| scan_float(t)) {
                Some(v) => *slot = v,
                None => break,
            }
        }
        out
    }
}

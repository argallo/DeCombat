//! Deterministic scalar math.
//!
//! The simulation must give bit-identical results on the server and on every client build (i686, x86_64,
//! aarch64, wasm32). IEEE-754 guarantees that `+ - * /` and square root are correctly rounded, so those are
//! the same everywhere as long as no fused multiply-add is formed; Rust never contracts `a * b + c` on its
//! own, and this crate never calls `mul_add`. Transcendental functions (`exp`, `sin`, `cos`) are not
//! correctly rounded by the platform libraries and differ between them, so this module implements them from
//! public textbook algorithms (Cody-Waite range reduction and Taylor polynomials, evaluated in binary64 and
//! rounded once to binary32). Every routine below uses only IEEE basic operations and integer bit
//! manipulation, so the result depends on nothing but the input bits.
//!
//! i686 note: the `i686-pc-windows-msvc` target enables SSE2, so `f32` / `f64` arithmetic is SSE scalar, not
//! x87 extended precision (MOVEMENT_2009.md §15 item 1 explains why the 2009 client itself is not bit-exact
//! with this crate: it keeps x87 intermediates in places).
// SPEC: MOVEMENT_2009.md §15 items 1-2 (one shared implementation of exp, no FMA)

/// `|x|` by clearing the sign bit (no libm needed).
#[inline]
pub fn abs(x: f32) -> f32 {
    f32::from_bits(x.to_bits() & 0x7fff_ffff)
}

#[inline]
fn abs64(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & 0x7fff_ffff_ffff_ffff)
}

/// Square root, correctly rounded (IEEE `squareRoot`). With the `std` feature the hardware instruction is
/// used; without it the integer routine [`sqrt_soft`]. Both are correctly rounded, so they return the same
/// bits for every input (checked by a unit test over a large sample of bit patterns).
#[inline]
pub fn sqrt(x: f32) -> f32 {
    #[cfg(feature = "std")]
    {
        x.sqrt()
    }
    #[cfg(not(feature = "std"))]
    {
        sqrt_soft(x)
    }
}

/// Integer square root (floor) of a 64-bit value, digit by digit.
fn isqrt_u64(n: u64) -> u64 {
    let mut rem = n;
    let mut root = 0u64;
    let mut bit = 1u64 << 62;
    while bit > n {
        bit >>= 2;
    }
    while bit != 0 {
        if rem >= root + bit {
            rem -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

/// Correctly rounded binary32 square root from integer arithmetic only.
pub fn sqrt_soft(x: f32) -> f32 {
    let bits = x.to_bits();
    if x.is_nan() {
        return x;
    }
    if bits & 0x7fff_ffff == 0 {
        return x; // +0 / -0
    }
    if bits >> 31 != 0 {
        return f32::NAN;
    }
    if x.is_infinite() {
        return x;
    }
    // value = m * 2^e with m in [2^23, 2^24)
    let mut exp = ((bits >> 23) & 0xff) as i32;
    let mut m = (bits & 0x007f_ffff) as u64;
    if exp == 0 {
        // subnormal: normalise
        exp = 1;
        while m & 0x0080_0000 == 0 {
            m <<= 1;
            exp -= 1;
        }
    } else {
        m |= 0x0080_0000;
    }
    let e = exp - 127 - 23;
    // sqrt(m * 2^s) lands in [2^23, 2^24) for s in {23, 24}; pick s with e - s even.
    let s = if e & 1 != 0 { 23 } else { 24 };
    let big = m << s;
    let mut r = isqrt_u64(big);
    let rem = big - r * r;
    // exact root = r + f, f in [0, 1); round up iff f > 1/2 iff rem > r (a tie cannot occur)
    if rem > r {
        r += 1;
    }
    let mut out_e = (e - s) / 2 + 23 + 127;
    if r == 1 << 24 {
        r >>= 1;
        out_e += 1;
    }
    f32::from_bits(((out_e as u32) << 23) | (r as u32 & 0x007f_ffff))
}

/// Round half away from zero to an integer, for range reduction (|v| is small at every call site).
fn round_i64(v: f64) -> i64 {
    let t = v as i64; // truncates toward zero (saturating)
    let frac = v - t as f64;
    if frac >= 0.5 {
        t + 1
    } else if frac <= -0.5 {
        t - 1
    } else {
        t
    }
}

/// 2^k as binary64, for k in the normal range.
fn pow2(k: i64) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

// ln 2 split for Cody-Waite reduction: the high part has 32 significant bits so k * LN2_HI is exact for
// |k| < 2^20.
const LN2_HI: f64 = 0.6931471803691238;
const LN2_LO: f64 = 1.9082149292705877e-10;
const LOG2_E: f64 = core::f64::consts::LOG2_E;

/// `e^x`, deterministic on every platform. Faithful to within about one unit in the last place of binary32
/// (evaluated in binary64, one final rounding); it is not guaranteed to be the correctly rounded value, but
/// it is the same value everywhere, which is what the shared simulation needs.
pub fn exp(x: f32) -> f32 {
    if x.is_nan() {
        return x;
    }
    if x > 100.0 {
        return f32::INFINITY;
    }
    if x < -110.0 {
        return 0.0;
    }
    let xd = x as f64;
    let k = round_i64(xd * LOG2_E);
    let kd = k as f64;
    let r = (xd - kd * LN2_HI) - kd * LN2_LO;
    // Taylor series of e^r for |r| <= 0.35: the degree-13 remainder is below 1e-17.
    let mut p = 1.0 / 6_227_020_800.0; // 1/13!
    p = p * r + 1.0 / 479_001_600.0;
    p = p * r + 1.0 / 39_916_800.0;
    p = p * r + 1.0 / 3_628_800.0;
    p = p * r + 1.0 / 362_880.0;
    p = p * r + 1.0 / 40_320.0;
    p = p * r + 1.0 / 5_040.0;
    p = p * r + 1.0 / 720.0;
    p = p * r + 1.0 / 120.0;
    p = p * r + 1.0 / 24.0;
    p = p * r + 1.0 / 6.0;
    p = p * r + 0.5;
    p = p * r + 1.0;
    p = p * r + 1.0;
    (p * pow2(k)) as f32
}

// pi/2 in three parts (each high part has trailing zero bits), public Cody-Waite constants.
const PIO2_1: f64 = 1.5707963267341256;
const PIO2_2: f64 = 6.077100506303966e-11;
const PIO2_3: f64 = 2.0222662487959506e-21;
const TWO_OVER_PI: f64 = core::f64::consts::FRAC_2_PI;

fn sin_poly(r: f64) -> f64 {
    // r - r^3/3! + ... - r^19/19!, |r| <= pi/4
    let r2 = r * r;
    let mut p = -1.0 / 121_645_100_408_832_000.0; // -1/19!
    p = p * r2 + 1.0 / 355_687_428_096_000.0; // 1/17!
    p = p * r2 - 1.0 / 1_307_674_368_000.0; // 1/15!
    p = p * r2 + 1.0 / 6_227_020_800.0;
    p = p * r2 - 1.0 / 39_916_800.0;
    p = p * r2 + 1.0 / 362_880.0;
    p = p * r2 - 1.0 / 5_040.0;
    p = p * r2 + 1.0 / 120.0;
    p = p * r2 - 1.0 / 6.0;
    p = p * r2 + 1.0;
    p * r
}

fn cos_poly(r: f64) -> f64 {
    // 1 - r^2/2! + r^4/4! - ... - r^18/18!, |r| <= pi/4
    let r2 = r * r;
    let mut p = -1.0 / 6_402_373_705_728_000.0; // -1/18!
    p = p * r2 + 1.0 / 20_922_789_888_000.0; // 1/16!
    p = p * r2 - 1.0 / 87_178_291_200.0; // 1/14!
    p = p * r2 + 1.0 / 479_001_600.0;
    p = p * r2 - 1.0 / 3_628_800.0;
    p = p * r2 + 1.0 / 40_320.0;
    p = p * r2 - 1.0 / 720.0;
    p = p * r2 + 1.0 / 24.0;
    p = p * r2 - 0.5;
    p * r2 + 1.0
}

/// `(sin x, cos x)`, deterministic on every platform (for view angles in radians; accurate for
/// |x| < 1e5, returns NaN for non-finite input).
pub fn sin_cos(x: f32) -> (f32, f32) {
    if !x.is_finite() {
        return (f32::NAN, f32::NAN);
    }
    let xd = x as f64;
    if abs64(xd) < 1e-9 {
        return (x, 1.0);
    }
    let q = round_i64(xd * TWO_OVER_PI);
    let qd = q as f64;
    let r = ((xd - qd * PIO2_1) - qd * PIO2_2) - qd * PIO2_3;
    let (s, c) = (sin_poly(r), cos_poly(r));
    let (s, c) = match q.rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    };
    (s as f32, c as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small deterministic generator for bit patterns (xorshift).
    fn bits(n: usize) -> impl Iterator<Item = u32> {
        let mut s: u32 = 0x9e37_79b9;
        (0..n).map(move |_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s
        })
    }

    #[test]
    fn soft_sqrt_matches_ieee() {
        for b in bits(400_000) {
            let x = f32::from_bits(b & 0x7fff_ffff);
            let want = x.sqrt();
            let got = sqrt_soft(x);
            assert!(
                want.to_bits() == got.to_bits() || (want.is_nan() && got.is_nan()),
                "{x:e}: {want:e} vs {got:e}"
            );
        }
        for x in [
            0.0f32,
            -0.0,
            1.0,
            2.0,
            4.0,
            0.25,
            f32::MIN_POSITIVE,
            1e-45,
            f32::MAX,
            f32::INFINITY,
        ] {
            assert_eq!(sqrt_soft(x).to_bits(), x.sqrt().to_bits(), "{x:e}");
        }
        assert!(sqrt_soft(-1.0).is_nan());
    }

    #[test]
    fn exp_is_close_to_reference() {
        for i in -2000..=500 {
            let x = i as f32 * 0.05;
            let want = (x as f64).exp();
            let got = exp(x) as f64;
            let rel = if want == 0.0 {
                got
            } else {
                ((got - want) / want).abs()
            };
            assert!(rel < 2e-7 || want < 1e-37, "exp({x}) = {got} want {want}");
        }
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(exp(-200.0), 0.0);
        assert_eq!(exp(200.0), f32::INFINITY);
    }

    #[test]
    fn sin_cos_close_to_reference() {
        for i in -4000..=4000 {
            let x = i as f32 * 0.0123;
            let (s, c) = sin_cos(x);
            assert!(((s as f64) - (x as f64).sin()).abs() < 2e-7, "sin {x}");
            assert!(((c as f64) - (x as f64).cos()).abs() < 2e-7, "cos {x}");
        }
    }

    #[test]
    fn abs_clears_sign() {
        assert_eq!(abs(-3.5), 3.5);
        assert_eq!(abs(-0.0).to_bits(), 0);
    }
}

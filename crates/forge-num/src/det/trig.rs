//! `sin` and `cos`: argument reduction to [-pi/4, pi/4] in double-double, then Taylor
//! kernels whose leading term is exact.

use super::consts::{
    INV_PIO2, NAN, PIO2_1, PIO2_2, PIO2_3, PIO2_4, PIO2_DD, PIO4, TWO_OVER_PI, pow2, round_int,
};
use super::horner;
use crate::dd::{Dd, fast_two_sum, two_prod, two_sum};

/// Below this, Cody-Waite reduction with the four pieces of pi/2 is exact enough
/// (`n < 2^20`, so every `n * PIO2_k` for k = 1..3 is exact).
const MEDIUM_LIMIT: f64 = 1_600_000.0;

// Taylor coefficients: sin x = x + x^3 (S1 + x^2 (S2 + ...)), cos x = 1 - x^2/2 + x^4 (C1 + ...).
// Truncation after x^19 (sin) and x^20 (cos) is below 2^-70 relative on |x| <= pi/4.
const SIN_C: [f64; 9] = [
    -1.0 / 6.0,
    1.0 / 120.0,
    -1.0 / 5_040.0,
    1.0 / 362_880.0,
    -1.0 / 39_916_800.0,
    1.0 / 6_227_020_800.0,
    -1.0 / 1_307_674_368_000.0,
    1.0 / 355_687_428_096_000.0,
    -1.0 / 121_645_100_408_832_000.0,
];

const COS_C: [f64; 9] = [
    1.0 / 24.0,
    -1.0 / 720.0,
    1.0 / 40_320.0,
    -1.0 / 3_628_800.0,
    1.0 / 479_001_600.0,
    -1.0 / 87_178_291_200.0,
    1.0 / 20_922_789_888_000.0,
    -1.0 / 6_402_373_705_728_000.0,
    1.0 / 2_432_902_008_176_640_000.0,
];

/// sin(hi + lo) for |hi| <= ~pi/4, |lo| <= ulp(hi).
#[inline(always)]
fn ksin(r: Dd) -> f64 {
    let (x, y) = (r.hi, r.lo);
    let z = x * x;
    let v = z * x;
    let s = horner(z, &SIN_C);
    // sin(x + y) ~= sin x + y cos x; the leading x is exact, everything added to it is small.
    x + (y * (1.0 - 0.5 * z) + v * s)
}

/// cos(hi + lo) for |hi| <= ~pi/4.
#[inline(always)]
fn kcos(r: Dd) -> f64 {
    let (x, y) = (r.hi, r.lo);
    let (zh, zl) = two_prod(x, x); // x^2 exactly
    let hz = 0.5 * zh;
    let w = 1.0 - hz;
    let err = (1.0 - w) - hz; // the rounding error of 1 - hz, recovered exactly
    let c = horner(zh, &COS_C);
    // cos(x + y) ~= cos x - y sin x ~= cos x - x y.
    w + (err + ((zh * zh * c - 0.5 * zl) - x * y))
}

/// 64 bits of 2/pi starting at zero-based bit index `g` (bit 0 has weight 2^-1). Negative
/// indices read the (zero) integer part.
#[inline(always)]
fn two_over_pi_word(g: i32) -> u64 {
    if g <= -64 {
        0
    } else if g < 0 {
        TWO_OVER_PI[0] >> (-g)
    } else {
        let q = (g / 64) as usize;
        let r = (g % 64) as u32;
        if r == 0 {
            TWO_OVER_PI[q]
        } else {
            (TWO_OVER_PI[q] << r) | (TWO_OVER_PI[q + 1] >> (64 - r))
        }
    }
}

/// Payne-Hanek reduction of a positive, finite `ax >= MEDIUM_LIMIT`: returns the quadrant
/// and `ax - n pi/2` as a double-double.
///
/// `ax = m 2^e` with `m` a 53-bit integer. Bits of 2/pi whose product with `m 2^e` is a
/// multiple of 4 cannot affect the quadrant or the remainder, so only a 192-bit window
/// starting at bit weight `2^-(e-1)` is multiplied — in exact integer arithmetic.
fn payne_hanek(ax: f64) -> (u32, Dd) {
    let bits = ax.to_bits();
    let m = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let e = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let g0 = e - 2;
    let w0 = two_over_pi_word(g0);
    let w1 = two_over_pi_word(g0 + 64);
    let w2 = two_over_pi_word(g0 + 128);

    // P = m * (w0:w1:w2), a 245-bit product; x (2/pi) mod 4 = P * 2^-190 mod 4.
    let m = m as u128;
    let a = m * w2 as u128; // weight 2^0
    let b = m * w1 as u128; // weight 2^64
    let c = m * w0 as u128; // weight 2^128
    let limb0 = a as u64;
    let mid = (a >> 64) + (b as u64 as u128);
    let limb1 = mid as u64;
    let top = (b >> 64) + c + (mid >> 64); // bits 128..255 of P

    let mut q = ((top >> 62) & 3) as u32; // bits 190, 191
    // The top 128 bits of the 190-bit fraction.
    let f = ((top & ((1u128 << 62) - 1)) << 66) | ((limb1 as u128) << 2) | ((limb0 >> 62) as u128);
    // Read as two's complement: a fraction >= 1/2 becomes (fraction - 1), one more quadrant.
    let fs = f as i128;
    if fs < 0 {
        q = q.wrapping_add(1);
    }
    let hi = fs as f64;
    let lo = fs.wrapping_sub(hi as i128) as f64;
    let frac = Dd::new(hi, lo).scale(pow2(-64) * pow2(-64));
    (q & 3, frac.mul(PIO2_DD))
}

/// Reduce a finite `x` with `|x| > pi/4`: quadrant `n mod 4` and `r = x - n pi/2` as a
/// double-double with `|r| <= ~pi/4`.
pub(crate) fn rem_pio2(x: f64) -> (u32, Dd) {
    let ax = x.abs();
    if ax < MEDIUM_LIMIT {
        let n = round_int(x * INV_PIO2);
        let t = x - n * PIO2_1; // exact: n*PIO2_1 is exact and within a factor 2 of x
        let (s, e) = two_sum(t, -(n * PIO2_2));
        let (s, e2) = two_sum(s, -(n * PIO2_3));
        let e = e + e2 - n * PIO2_4;
        let (hi, lo) = two_sum(s, e);
        let (hi, lo) = fast_two_sum(hi, lo);
        ((n as i64 & 3) as u32, Dd::new(hi, lo))
    } else {
        let (q, r) = payne_hanek(ax);
        if x < 0.0 {
            (q.wrapping_neg() & 3, r.neg())
        } else {
            (q, r)
        }
    }
}

/// Bit-reproducible sine.
pub fn sin(x: f64) -> f64 {
    let ax = x.abs();
    if ax <= PIO4 {
        if ax < pow2(-26) {
            return x; // sin x = x (1 - x^2/6), and x^2/6 < 2^-54: rounds to x
        }
        return ksin(Dd::new(x, 0.0));
    }
    if !x.is_finite() {
        return NAN;
    }
    let (q, r) = rem_pio2(x);
    match q {
        0 => ksin(r),
        1 => kcos(r),
        2 => -ksin(r),
        _ => -kcos(r),
    }
}

/// Bit-reproducible cosine.
pub fn cos(x: f64) -> f64 {
    let ax = x.abs();
    if ax <= PIO4 {
        if ax < pow2(-27) {
            return 1.0; // cos x = 1 - x^2/2, and x^2/2 < 2^-55: rounds to 1
        }
        return kcos(Dd::new(x, 0.0));
    }
    if !x.is_finite() {
        return NAN;
    }
    let (q, r) = rem_pio2(x);
    match q {
        0 => kcos(r),
        1 => -ksin(r),
        2 => -kcos(r),
        _ => ksin(r),
    }
}

//! `exp`, `ln` and `pow`.
//!
//! `exp` reduces `x = k ln2 + r`, `|r| <= ln2/2`, with a two-piece ln 2 whose high piece
//! makes `k * LN2_HI` exact, then sums the Taylor series of `e^r - 1` with the leading
//! `1 + r` kept error-free. `ln` uses the classic `log(1+f) = f - f^2/2 + s (f^2/2 + R(s^2))`
//! arrangement (`s = f / (2 + f)`), whose leading `f` is exact. `pow` evaluates
//! `exp(y ln x)` with `ln x` in double-double (table-driven, ~2^-78 relative, no division),
//! so the error of the product stays far below an ulp of the result even when `|y ln x|`
//! is near 709.

use super::consts::{INV_LN2, LN2_HI, LN2_LO, NAN, THIRD_DD, pow2, round_int};
use super::horner;
use super::ln_table::{LN_TABLE, LN_TABLE_OFF, LN_TABLE_SIZE};
use crate::dd::{Dd, fast_two_sum, two_prod, two_sum};

// 1/n! for the e^r - 1 series; truncation after r^14 is ~1e-19 on |r| <= ln2/2.
const EXPM1_C: [f64; 13] = [
    1.0 / 2.0,
    1.0 / 6.0,
    1.0 / 24.0,
    1.0 / 120.0,
    1.0 / 720.0,
    1.0 / 5_040.0,
    1.0 / 40_320.0,
    1.0 / 362_880.0,
    1.0 / 3_628_800.0,
    1.0 / 39_916_800.0,
    1.0 / 479_001_600.0,
    1.0 / 6_227_020_800.0,
    1.0 / 87_178_291_200.0,
];

// 2/(2n+1) for the atanh series R(z) = sum_{n>=1} 2 z^n / (2n+1); z = s^2 <= 0.0295, and
// truncation after z^12 is below 2^-60 relative.
// `ln` uses all 12 (`ln_dd`, for `pow`, is table-driven and does not use this series).
const LN_C: [f64; 12] = [
    2.0 / 3.0,
    2.0 / 5.0,
    2.0 / 7.0,
    2.0 / 9.0,
    2.0 / 11.0,
    2.0 / 13.0,
    2.0 / 15.0,
    2.0 / 17.0,
    2.0 / 19.0,
    2.0 / 21.0,
    2.0 / 23.0,
    2.0 / 25.0,
];

/// ln(DBL_MAX) is 709.78...; above this `exp` is +inf.
const EXP_OVERFLOW: f64 = 709.79;
/// ln(2^-1075) is -745.13...; below this `exp` is +0.
const EXP_UNDERFLOW: f64 = -745.2;

const MANT_MASK: u64 = (1u64 << 52) - 1;
/// Bits of sqrt(2) with the exponent of 1: mantissas above this are halved so m is in
/// [sqrt(1/2), sqrt(2)).
const SQRT2_BITS: u64 = 0x3FF6_A09E_667F_3BCD;

/// `y * 2^k` for `y` in [0.7, 1.42] and `k` in [-1076, 1025]: one rounding at most, and
/// only when the result is subnormal.
#[inline(always)]
fn scale2(y: f64, k: i32) -> f64 {
    if k > 1023 {
        y * pow2(1023) * pow2(k - 1023)
    } else if k >= -1022 {
        y * pow2(k)
    } else {
        // y * 2^(k+54) is exact and normal; the final * 2^-54 rounds once into subnormals.
        y * pow2(k + 54) * pow2(-54)
    }
}

/// e^(hi + lo), where `lo` is a small correction (|lo| <= ulp(hi)).
pub(crate) fn exp_core(hi: f64, lo: f64) -> f64 {
    if hi.is_nan() {
        return NAN;
    }
    if hi > EXP_OVERFLOW {
        return f64::INFINITY;
    }
    if hi < EXP_UNDERFLOW {
        return 0.0;
    }
    let kf = round_int(hi * INV_LN2);
    let k = kf as i32;
    let t = hi - kf * LN2_HI; // exact
    let (a, b) = two_sum(t, -(kf * LN2_LO));
    let (r, rl) = two_sum(a, b + lo);
    let q = r * r * horner(r, &EXPM1_C);
    let (s, e) = fast_two_sum(1.0, r); // |r| < 1
    let y = s + (e + (q + rl * (1.0 + r)));
    scale2(y, k)
}

/// Bit-reproducible e^x.
pub fn exp(x: f64) -> f64 {
    if x.abs() < pow2(-54) {
        return 1.0 + x;
    }
    exp_core(x, 0.0)
}

/// Split a positive, finite, nonzero `x` into `x = 2^k (1 + f)` with `1 + f` in
/// [sqrt(1/2), sqrt(2)). `f` is exact.
#[inline(always)]
fn decompose(x: f64) -> (i32, f64) {
    let mut bits = x.to_bits();
    let mut k = 0i32;
    if bits < 0x0010_0000_0000_0000 {
        // subnormal: scale into the normal range exactly
        bits = (x * pow2(54)).to_bits();
        k = -54;
    }
    k += ((bits >> 52) as i32) - 1023;
    let mut mbits = (bits & MANT_MASK) | 0x3FF0_0000_0000_0000;
    if mbits > SQRT2_BITS {
        mbits -= 1u64 << 52; // halve m
        k += 1;
    }
    (k, f64::from_bits(mbits) - 1.0)
}

/// Bit-reproducible natural logarithm.
pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x == f64::INFINITY {
        return x;
    }
    let (k, f) = decompose(x);
    let s = f / (2.0 + f);
    let z = s * s;
    let r = z * horner(z, &LN_C);
    let hfsq = 0.5 * f * f;
    let kf = k as f64;
    kf * LN2_HI - ((hfsq - (s * (hfsq + r) + kf * LN2_LO)) - f)
}

/// `log1p(r) = r - r^2/2 + r^3/3 - ...` tail from `r^4`, over `|r| <= 2^-8`: `r^4 (-1/4 +
/// r/5 - ...)` through `r^11`; the next term is below 2^-88 |r|.
const LOG1P_TAIL: [f64; 8] = [
    -1.0 / 4.0,
    1.0 / 5.0,
    -1.0 / 6.0,
    1.0 / 7.0,
    -1.0 / 8.0,
    1.0 / 9.0,
    -1.0 / 10.0,
    1.0 / 11.0,
];

/// ln x as a double-double, good to ~2^-78 relative, for positive finite `x`.
///
/// Table-driven, with no division: `x = 2^k z`, `z` in `[OFF, 2 OFF)`, and bucket `i` of
/// [`LN_TABLE`] gives `1/c` (21 bits) and `ln c` (double-double). `r = z/c - 1` is exact as
/// a double-double (the split `z = z_hi + z_lo` makes both products exact, and
/// `z_hi/c - 1` is exact by Sterbenz), `|r| <= 2^-8`, and
/// `ln x = k ln 2 + ln c + log1p(r)` with `r - r^2/2 + r^3/3` carried in double-double and
/// the rest in double. In the bucket holding 1.0, `1/c = 1` and `ln c = 0`, so the result
/// keeps its relative accuracy as `x -> 1`.
#[inline(always)]
fn ln_dd(x: f64) -> Dd {
    let mut ix = x.to_bits();
    let mut k = 0i32;
    if ix < 0x0010_0000_0000_0000 {
        // subnormal: scale into the normal range exactly
        ix = (x * pow2(54)).to_bits();
        k = -54;
    }
    let tmp = ix.wrapping_sub(LN_TABLE_OFF);
    let i = ((tmp >> 45) as usize) & (LN_TABLE_SIZE - 1);
    k += ((tmp as i64) >> 52) as i32;
    let iz = ix.wrapping_sub(tmp & (0xFFFu64 << 52));
    let z = f64::from_bits(iz);
    let zh = f64::from_bits(iz & !((1u64 << 27) - 1)); // 26 significant bits
    let zl = z - zh; // exact, at most 27 bits
    let (inv_c, lch, lcl) = LN_TABLE[i];
    let (rh, rl) = two_sum(zh * inv_c - 1.0, zl * inv_c); // r = z/c - 1, exactly

    // r^2 = sh + sl (+ rl^2, below 2^-120 |r|)
    let (sh, sl0) = two_prod(rh, rh);
    let sl = sl0 + 2.0 * rh * rl;
    // r^3 = ch + cl
    let (ch, cl0) = two_prod(sh, rh);
    let cl = cl0 + (sl0 * rh + 3.0 * sh * rl);
    // r^3 / 3 = eh + el
    let (eh, el0) = two_prod(ch, THIRD_DD.hi);
    let el = el0 + (ch * THIRD_DD.lo + cl * THIRD_DD.hi);
    // r^4 (-1/4 + r/5 - ...)
    let tail = sh * sh * horner(rh, &LOG1P_TAIL);

    // log1p(r) = rh - sh/2 + eh + (small terms); |rh| >= |sh/2| >= |eh|.
    let (a, ae) = fast_two_sum(rh, -0.5 * sh);
    let (b, be) = fast_two_sum(a, eh);
    let p_lo = (rl + ae + be) - 0.5 * sl + (el + tail);

    // + k ln 2 + ln c. |k ln 2| > |ln c + log1p r| whenever k != 0: no cancellation.
    let kf = k as f64;
    let (h1, e1) = two_sum(kf * LN2_HI, lch); // k * LN2_HI is exact
    let (h2, e2) = two_sum(h1, b);
    let lo = kf * LN2_LO + lcl + e1 + e2 + p_lo;
    let (hi, lo) = fast_two_sum(h2, lo);
    Dd::new(hi, lo)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IntClass {
    NotInt,
    Even,
    Odd,
}

/// Integer classification of a finite `y`, from its bits alone.
fn classify_int(y: f64) -> IntClass {
    let bits = y.to_bits();
    let e = ((bits >> 52) & 0x7ff) as i32 - 1023;
    if y == 0.0 {
        return IntClass::Even;
    }
    if e < 0 {
        return IntClass::NotInt;
    }
    if e >= 53 {
        return IntClass::Even;
    }
    if e == 0 {
        // |y| in [1, 2): an integer only if it is exactly 1
        return if bits & MANT_MASK == 0 {
            IntClass::Odd
        } else {
            IntClass::NotInt
        };
    }
    let frac_bits = 52 - e as u32;
    if bits & ((1u64 << frac_bits) - 1) != 0 {
        return IntClass::NotInt;
    }
    if (bits >> frac_bits) & 1 == 1 {
        IntClass::Odd
    } else {
        IntClass::Even
    }
}

#[inline(always)]
fn with_sign(r: f64, negative: bool) -> f64 {
    if negative { -r } else { r }
}

/// Bit-reproducible x^y, with the IEEE 754 / C99 Annex F special cases.
pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0; // even for a NaN in the other argument (C99 F.9.4.4)
    }
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    if y == 1.0 {
        return x;
    }
    let ax = x.abs();
    if y.is_infinite() {
        if ax == 1.0 {
            return 1.0; // (-1)^(+-inf)
        }
        return if (ax < 1.0) == (y < 0.0) {
            f64::INFINITY
        } else {
            0.0
        };
    }
    let class = classify_int(y);
    let negative = x.is_sign_negative() && class == IntClass::Odd;
    if x.is_infinite() {
        return with_sign(if y < 0.0 { 0.0 } else { f64::INFINITY }, negative);
    }
    if ax == 0.0 {
        return with_sign(if y < 0.0 { f64::INFINITY } else { 0.0 }, negative);
    }
    if x < 0.0 && class == IntClass::NotInt {
        return NAN;
    }
    if ax == 1.0 {
        return with_sign(1.0, negative); // x = -1, integer y
    }
    if y == 2.0 {
        return x * x;
    }
    let l = ln_dd(ax);
    let ph = y * l.hi;
    if ph.is_nan() || ph.abs() > 1000.0 {
        // far outside [-745, 709]; also keeps the Dekker split below in range
        return with_sign(if ph > 0.0 { f64::INFINITY } else { 0.0 }, negative);
    }
    let (p, e) = two_prod(y, l.hi);
    let (p, e) = fast_two_sum(p, e + y * l.lo);
    with_sign(exp_core(p, e), negative)
}

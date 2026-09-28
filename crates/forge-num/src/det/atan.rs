//! `atan2`: reduce to `t = min/max` in [0, 1] (with the division's rounding error
//! recovered), pick the nearest `c = j/8`, and use
//! `atan t = atan c + atan((t - c) / (1 + t c))` with `atan(j/8)` held in double-double.
//! Quadrant fix-ups (`pi/2 - a`, `pi - a`) are done in double-double too, so the result is
//! rounded exactly once.

use super::consts::{ATAN_J8, NAN, PI_DD, PIO2_DD, PIO4_DD, THREE_PIO4_DD, pow2, round_int};
use super::horner;
use crate::dd::{Dd, fast_two_sum, two_prod, two_sum};

// atan u = u + u^3 (A1 + u^2 (A2 + ...)); |u| <= 1/16, truncation after u^17 < 2^-80.
const ATAN_C: [f64; 8] = [
    -1.0 / 3.0,
    1.0 / 5.0,
    -1.0 / 7.0,
    1.0 / 9.0,
    -1.0 / 11.0,
    1.0 / 13.0,
    -1.0 / 15.0,
    1.0 / 17.0,
];

#[inline(always)]
fn atan_poly(z: f64) -> f64 {
    horner(z, &ATAN_C)
}

/// atan(num / den) as a double-double, for finite `0 < num <= den`.
///
/// Three divisions on the table path (the quotient `t`, its rounding error `tl`, and one
/// reciprocal), where the first version had five: the quotient was computed twice, and
/// `u = (t - c)/(1 + t c)` used a double-double division (two quotients). `u` is now one
/// reciprocal of the denominator's high part plus one exact-remainder correction, which
/// yields the same double-double quotient to ~2^-100 and therefore the same bits (the golden
/// hashes are unchanged; ADR 0004).
#[inline(always)]
fn atan_ratio(num: f64, den: f64) -> Dd {
    let t = num / den;
    if t < pow2(-30) {
        // atan t = t (1 - t^2/3 + ...), and t^2/3 < 2^-61: the correctly rounded quotient
        // is already within a hair of the correctly rounded result.
        return Dd::new(t, 0.0);
    }
    // num >= den 2^-30 here, so scaling both by the same power of two keeps both exact
    // and keeps the Dekker products below away from overflow and underflow. The quotient
    // of the scaled pair is `t` itself (same real quotient, normal result), so it is not
    // recomputed.
    let (n, d) = if den > pow2(500) {
        (num * pow2(-600), den * pow2(-600))
    } else if den < pow2(-500) {
        (num * pow2(600), den * pow2(600))
    } else {
        (num, den)
    };
    let (p, e) = two_prod(t, d);
    let tl = ((n - p) - e) / d; // t + tl = n/d to ~2^-106

    let j = round_int(t * 8.0);
    if j == 0.0 {
        let z = t * t;
        let (hi, lo) = fast_two_sum(t, tl + t * z * atan_poly(z));
        return Dd::new(hi, lo);
    }
    let c = j * 0.125;
    // numerator t - c is exact (Sterbenz: c/2 <= t <= 2c), denominator 1 + t c in dd.
    let (nh, nl) = two_sum(t - c, tl);
    let (ph, pl) = two_prod(t, c);
    let (dh, dl) = fast_two_sum(1.0, ph); // t c <= 1
    let den_u = Dd::new(dh, dl + pl);
    // u = (nh + nl) / den_u: one reciprocal, one exact remainder (`uh * den_u.hi` is
    // error-free and within an ulp or two of `nh`, so `nh - p` is exact), one correction.
    let inv = 1.0 / den_u.hi;
    let uh = nh * inv;
    let (p, e) = two_prod(uh, den_u.hi);
    let rem = (((nh - p) - e) + nl) - uh * den_u.lo;
    let (uh, ul) = fast_two_sum(uh, rem * inv);
    let z = uh * uh;
    let tail = uh * z * atan_poly(z);
    // atan c >= atan(1/8) > 0.124 > 1/16 >= |u| > |tail|: the ordered sums hold.
    ATAN_J8[j as usize]
        .add_ordered(Dd::new(uh, ul))
        .add_ordered(Dd::new(tail, 0.0))
}

/// Bit-reproducible two-argument arctangent, in (-pi, pi], with the IEEE 754 / C99 Annex F
/// special cases for zeros and infinities.
pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    let (ay, ax) = (y.abs(), x.abs());
    let x_neg = x.is_sign_negative();
    let zero = Dd::new(0.0, 0.0);
    let a = if ay == 0.0 {
        if x_neg { PI_DD } else { zero }
    } else if ax == 0.0 {
        PIO2_DD
    } else if ay.is_infinite() {
        if ax.is_infinite() {
            if x_neg { THREE_PIO4_DD } else { PIO4_DD }
        } else {
            PIO2_DD
        }
    } else if ax.is_infinite() {
        if x_neg { PI_DD } else { zero }
    } else {
        let swap = ay > ax;
        let mut a = if swap {
            atan_ratio(ax, ay)
        } else {
            atan_ratio(ay, ax)
        };
        if swap {
            a = PIO2_DD.add_ordered(a.neg()); // a <= pi/4: no cancellation
        }
        if x_neg {
            a = PI_DD.add_ordered(a.neg()); // a <= pi/2: no cancellation
        }
        a
    };
    let r = a.to_f64();
    if y.is_sign_negative() { -r } else { r }
}

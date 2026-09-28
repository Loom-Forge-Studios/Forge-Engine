//! High-precision reference implementations for the accuracy tests — TESTS ONLY.
//!
//! Independent of the code under test: their own double-double type, constants derived by
//! the bignum from first principles, exact argument reduction by a full-width bignum
//! product with 1536 bits of 2/pi (not the production table and window), different
//! algorithms (exp by scaling-and-squaring, ln by a double-double atanh series, atan2 by
//! Newton on sin/cos).
//! Relative accuracy is ~2^-90 or better everywhere the tests sample, so an error
//! measured in ulps is exact to far more digits than the table reports.

#![allow(dead_code)]

use super::bignum::{self, Fixed, pow2};
use std::sync::OnceLock;

/// A double-double.
#[derive(Clone, Copy, Debug)]
pub struct R {
    pub hi: f64,
    pub lo: f64,
}

fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let v = s - a;
    (s, (a - (s - v)) + (b - v))
}

fn split(a: f64) -> (f64, f64) {
    let c = 134_217_729.0 * a;
    let h = c - (c - a);
    (h, a - h)
}

fn two_prod(a: f64, b: f64) -> (f64, f64) {
    let p = a * b;
    let (ah, al) = split(a);
    let (bh, bl) = split(b);
    (p, ((ah * bh - p) + ah * bl + al * bh) + al * bl)
}

impl R {
    pub fn f(x: f64) -> R {
        R { hi: x, lo: 0.0 }
    }
    fn norm(hi: f64, lo: f64) -> R {
        let (h, l) = two_sum(hi, lo);
        R { hi: h, lo: l }
    }
    pub fn add(self, o: R) -> R {
        let (s, e) = two_sum(self.hi, o.hi);
        let (t, f) = two_sum(self.lo, o.lo);
        let (s, e) = two_sum(s, e + t);
        R::norm(s, e + f)
    }
    pub fn neg(self) -> R {
        R {
            hi: -self.hi,
            lo: -self.lo,
        }
    }
    pub fn sub(self, o: R) -> R {
        self.add(o.neg())
    }
    pub fn mul(self, o: R) -> R {
        let (p, e) = two_prod(self.hi, o.hi);
        R::norm(p, e + (self.hi * o.lo + self.lo * o.hi))
    }
    pub fn mulf(self, b: f64) -> R {
        let (p, e) = two_prod(self.hi, b);
        R::norm(p, e + self.lo * b)
    }
    pub fn div(self, o: R) -> R {
        let q1 = self.hi / o.hi;
        let r = self.sub(o.mulf(q1));
        let q2 = r.hi / o.hi;
        let r = r.sub(o.mulf(q2));
        let q3 = r.hi / o.hi;
        R::norm(q1, q2).add(R::f(q3))
    }
    pub fn divf(self, b: f64) -> R {
        self.div(R::f(b))
    }
    /// Exact multiplication by 2^e (no overflow/underflow in the tested ranges).
    pub fn ldexp(self, e: i32) -> R {
        R {
            hi: ldexp(self.hi, e),
            lo: ldexp(self.lo, e),
        }
    }
}

pub fn ldexp(x: f64, e: i32) -> f64 {
    let mut x = x;
    let mut e = e;
    while e > 1000 {
        x *= pow2(1000);
        e -= 1000;
    }
    while e < -1000 {
        x *= pow2(-1000);
        e += 1000;
    }
    x * pow2(e)
}

struct Consts {
    pi: R,
    pio2: R,
    ln2: R,
    two_over_pi: Fixed,
}

fn consts() -> &'static Consts {
    static C: OnceLock<Consts> = OnceLock::new();
    C.get_or_init(|| {
        let pi = bignum::pi();
        let r = |f: &Fixed| {
            let (hi, lo) = f.to_dd();
            R { hi, lo }
        };
        Consts {
            pi: r(&pi),
            pio2: r(&pi.div_small(2)),
            ln2: r(&bignum::ln2()),
            two_over_pi: bignum::two_over_pi(),
        }
    })
}

pub fn pi() -> R {
    consts().pi
}

/// e^x for a double-double x with |x| < 709: reduce by ln 2, scale by 2^-10, Taylor,
/// square ten times.
pub fn exp(x: R) -> R {
    let c = consts();
    let k = (x.hi / c.ln2.hi).round();
    let r = x.sub(c.ln2.mulf(k)).ldexp(-10);
    let mut sum = R::f(1.0);
    let mut term = R::f(1.0);
    for n in 1..=16 {
        term = term.mul(r).divf(n as f64);
        sum = sum.add(term);
    }
    for _ in 0..10 {
        sum = sum.mul(sum);
    }
    sum.ldexp(k as i32)
}

/// ln x for positive finite x: x = m 2^e with m in [sqrt(1/2), sqrt(2)), then
/// ln m = 2 atanh(s), s = (m - 1)/(m + 1), summed in double-double to 2^-104 *relative*
/// (m - 1 is exact, so there is no cancellation near x = 1 — a Newton iteration on exp
/// would only be accurate to 2^-94 *absolute* there).
pub fn ln(x: f64) -> R {
    let c = consts();
    let (x, e0) = if x.to_bits() < 0x0010_0000_0000_0000 {
        (x * pow2(54), -54)
    } else {
        (x, 0)
    };
    let bits = x.to_bits();
    let mut e = ((bits >> 52) as i32) - 1023 + e0;
    let mut m = f64::from_bits((bits & ((1 << 52) - 1)) | 0x3FF0_0000_0000_0000);
    if m > std::f64::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let s = R::f(m - 1.0).div(R::f(m).add(R::f(1.0)));
    let s2 = s.mul(s);
    let mut sum = R::f(0.0);
    let mut pw = s;
    for k in 0..45 {
        sum = sum.add(pw.divf((2 * k + 1) as f64));
        pw = pw.mul(s2);
    }
    sum.mulf(2.0).add(c.ln2.mulf(e as f64))
}

/// Taylor sin and cos of a double-double |r| <= ~1.
fn sin_cos_small(r: R) -> (R, R) {
    let r2 = r.mul(r);
    let mut s = R::f(0.0);
    let mut term = r;
    let mut n = 1.0;
    for _ in 0..30 {
        s = s.add(term);
        term = term.mul(r2).divf((n + 1.0) * (n + 2.0)).neg();
        n += 2.0;
    }
    let mut c = R::f(0.0);
    let mut term = R::f(1.0);
    let mut n = 0.0;
    for _ in 0..30 {
        c = c.add(term);
        term = term.mul(r2).divf((n + 1.0) * (n + 2.0)).neg();
        n += 2.0;
    }
    (s, c)
}

/// Exact reduction of a finite x: quadrant and x - n pi/2 as a double-double, via the full
/// product of x's 53-bit mantissa with 1536 bits of 2/pi.
fn reduce(x: f64) -> (u32, R) {
    let c = consts();
    let ax = x.abs();
    if ax <= 0.78 {
        return (0, R::f(x));
    }
    let bits = ax.to_bits();
    let (m, e) = if bits < 0x0010_0000_0000_0000 {
        (bits & ((1 << 52) - 1), -1074i64)
    } else {
        (
            (bits & ((1 << 52) - 1)) | (1 << 52),
            ((bits >> 52) as i64) - 1075,
        )
    };
    let v = c.two_over_pi.mul_small(m); // exact: m * (2/pi)
    // W = v * 2^e. Bit of W with weight 2^j is bit of v with weight 2^(j - e).
    let bit = |w: i64| -> u128 {
        let pos = w + 64 * bignum::FRAC as i64;
        if pos < 0 || pos >= 64 * (bignum::FRAC as i64 + 1) {
            0
        } else {
            ((v.limbs[(pos / 64) as usize] >> (pos % 64)) & 1) as u128
        }
    };
    let mut q = (bit(1 - e) << 1 | bit(-e)) as u32;
    let mut f = 0u128;
    for k in 1..=128 {
        f |= bit(-k - e) << (128 - k);
    }
    let fs = f as i128;
    if fs < 0 {
        q += 1;
    }
    let hi = fs as f64;
    let lo = fs.wrapping_sub(hi as i128) as f64;
    let r = R::norm(hi, lo).ldexp(-128).mul(c.pio2);
    if x < 0.0 {
        ((4 - (q & 3)) & 3, r.neg())
    } else {
        (q & 3, r)
    }
}

pub fn sin_cos(x: f64) -> (R, R) {
    let (q, r) = reduce(x);
    let (s, c) = sin_cos_small(r);
    match q {
        0 => (s, c),
        1 => (c, s.neg()),
        2 => (s.neg(), c.neg()),
        _ => (c.neg(), s),
    }
}

/// atan2 by Newton on f(t) = x sin t - y cos t from a double start `t0`.
pub fn atan2(y: f64, x: f64, t0: f64) -> R {
    // Scale both to magnitude ~1 (exact: same power of two).
    let m = y.abs().max(x.abs());
    let e = ((m.to_bits() >> 52) as i32) - 1023;
    let (y, x) = (ldexp(y, -e), ldexp(x, -e));
    let mut t = R::f(t0);
    for _ in 0..3 {
        let (s, c) = sin_cos_theta(t);
        let f = R::f(x).mul(s).sub(R::f(y).mul(c));
        let fp = R::f(x).mul(c).add(R::f(y).mul(s));
        t = t.sub(f.div(fp));
    }
    t
}

/// sin/cos of a double-double angle |t| <= ~4, reduced by double-double pi/2 (plenty for
/// angles this small).
fn sin_cos_theta(t: R) -> (R, R) {
    let c = consts();
    let n = (t.hi / c.pio2.hi).round();
    let r = t.sub(c.pio2.mulf(n));
    let (s, co) = sin_cos_small(r);
    match (n as i64).rem_euclid(4) {
        0 => (s, co),
        1 => (co, s.neg()),
        2 => (s.neg(), co.neg()),
        _ => (co.neg(), s),
    }
}

/// x^y = exp(y ln x) for positive x, |y ln x| < 700.
pub fn pow(x: f64, y: f64) -> R {
    exp(ln(x).mulf(y))
}

/// Signed error of `got` against `want`, in ulps of `want` (subnormal ulp below 2^-1022).
pub fn ulp_error(got: f64, want: R) -> f64 {
    let e = ((want.hi.abs().to_bits() >> 52) as i32).max(1) - 1023 - 52;
    let ulp = ldexp(1.0, e);
    ((got - want.hi) - want.lo) / ulp
}

/// e^x split as `m * 2^k` with m in ~[0.7, 1.42], so a subnormal result can be measured
/// without rounding the reference into the subnormal range first.
pub fn exp_parts(x: f64) -> (R, i32) {
    let c = consts();
    let k = (x / c.ln2.hi).round();
    let m = exp(R::f(x).sub(c.ln2.mulf(k)));
    (m, k as i32)
}

/// Error of `got` against `m * 2^k`, in ulps of the true value (whose ulp is 2^-1074 when
/// it is subnormal).
pub fn ulp_error_parts(got: f64, m: R, k: i32) -> f64 {
    let em = ((m.hi.abs().to_bits() >> 52) as i32) - 1023;
    let ulp_exp = (em + k).max(-1022) - 52;
    let g = ldexp(got, -k); // exact: scaling a subnormal up, or a normal by a power of two
    ((g - m.hi) - m.lo) / ldexp(1.0, ulp_exp - k)
}

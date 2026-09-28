//! Error-free transformations and double-double ("dd") arithmetic.
//!
//! Every function here uses only IEEE-754 `+ - * /` in round-to-nearest, whose results the
//! standard fixes to the bit, so every function is bit-reproducible on every conforming
//! platform. No fused multiply-add is used (Ch.3.3: contraction is explicit only, and a
//! software `fma` on a CPU without one would be both correct and very slow); products are
//! split with Dekker's algorithm instead.

/// `(s, e)` with `s = fl(a + b)` and `a + b = s + e` exactly (Knuth's TwoSum).
#[inline(always)]
pub(crate) fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let bb = s - a;
    let e = (a - (s - bb)) + (b - bb);
    (s, e)
}

/// `(s, e)` with `s = fl(a + b)`, `a + b = s + e`, valid when `|a| >= |b|` or `a == 0`.
#[inline(always)]
pub(crate) fn fast_two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let e = b - (s - a);
    (s, e)
}

/// Dekker's split: `a = hi + lo` with each half holding at most 26 significant bits.
/// Valid for `|a| < 2^996` (the caller keeps arguments in range).
#[inline(always)]
fn split(a: f64) -> (f64, f64) {
    const SPLITTER: f64 = 134_217_729.0; // 2^27 + 1
    let t = SPLITTER * a;
    let hi = t - (t - a);
    (hi, a - hi)
}

/// `(p, e)` with `p = fl(a * b)` and `a * b = p + e` exactly (Dekker's TwoProduct), barring
/// overflow and underflow of the partial products.
#[inline(always)]
pub(crate) fn two_prod(a: f64, b: f64) -> (f64, f64) {
    let p = a * b;
    let (ah, al) = split(a);
    let (bh, bl) = split(b);
    let e = ((ah * bh - p) + ah * bl + al * bh) + al * bl;
    (p, e)
}

/// A double-double: the unevaluated sum `hi + lo`, `|lo| <= ulp(hi) / 2`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dd {
    pub hi: f64,
    pub lo: f64,
}

impl Dd {
    #[inline(always)]
    pub(crate) const fn new(hi: f64, lo: f64) -> Self {
        Self { hi, lo }
    }

    /// `self + o` when `|self.hi| >= |o.hi|` (or `self.hi == 0`) and the sum does not cancel
    /// below `|self| / 2`: one error-free `fast_two_sum` of the high parts and one of the
    /// renormalisation, ~2^-104 relative — the accuracy of the general (two `two_sum`)
    /// double-double add at a third of its dependency chain. Callers state why the
    /// precondition holds.
    #[inline(always)]
    pub(crate) fn add_ordered(self, o: Dd) -> Dd {
        let (s, e) = fast_two_sum(self.hi, o.hi);
        let (hi, lo) = fast_two_sum(s, e + (self.lo + o.lo));
        Dd { hi, lo }
    }

    #[inline(always)]
    pub(crate) fn neg(self) -> Dd {
        Dd {
            hi: -self.hi,
            lo: -self.lo,
        }
    }

    #[inline(always)]
    pub(crate) fn mul(self, o: Dd) -> Dd {
        let (p, e) = two_prod(self.hi, o.hi);
        let e = e + (self.hi * o.lo + self.lo * o.hi);
        let (hi, lo) = fast_two_sum(p, e);
        Dd { hi, lo }
    }

    /// `self * 2^k` for a power-of-two `scale` — exact.
    #[inline(always)]
    pub(crate) fn scale(self, scale: f64) -> Dd {
        Dd {
            hi: self.hi * scale,
            lo: self.lo * scale,
        }
    }

    /// Round to the nearest f64.
    #[inline(always)]
    pub(crate) fn to_f64(self) -> f64 {
        self.hi + self.lo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_sum_is_error_free() {
        let (s, e) = two_sum(1.0, 1e-20);
        assert_eq!((s, e), (1.0, 1e-20));
        let (s, e) = two_sum(1e-20, 1.0);
        assert_eq!((s, e), (1.0, 1e-20));
    }

    #[test]
    fn two_prod_is_error_free() {
        // (1 + 2^-30)^2 = 1 + 2^-29 + 2^-60: the 2^-60 is the error term.
        let a = 1.0 + f64::from_bits(0x3E10_0000_0000_0000); // 2^-30
        let (p, e) = two_prod(a, a);
        assert_eq!(p, 1.0 + f64::from_bits(0x3E20_0000_0000_0000));
        assert_eq!(e, f64::from_bits(0x3C30_0000_0000_0000)); // 2^-60
    }

    #[test]
    fn add_ordered_keeps_the_low_part() {
        // (1 + 2^-60) + (2^-3 + 2^-70) = 1.125 + (2^-60 + 2^-70): nothing lost.
        let a = Dd::new(1.0, f64::from_bits(0x3C30_0000_0000_0000));
        let b = Dd::new(0.125, f64::from_bits(0x3B90_0000_0000_0000));
        let s = a.add_ordered(b);
        assert_eq!(s.hi, 1.125);
        assert_eq!(
            s.lo,
            f64::from_bits(0x3C30_0000_0000_0000) + f64::from_bits(0x3B90_0000_0000_0000)
        );
        // pi/2 - atan(1/8)-like: no cancellation, low parts combine.
        let d = a.add_ordered(b.neg());
        assert_eq!(d.hi, 0.875);
    }
}

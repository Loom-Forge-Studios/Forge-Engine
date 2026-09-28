//! Arbitrary-precision unsigned fixed-point arithmetic, for TESTS ONLY.
//!
//! It exists to derive every mathematical constant `forge-num` embeds (the bits of 2/pi, the
//! Cody-Waite pieces of pi/2, ln 2, atan(j/8)) from first principles — Machin's formula, the
//! series for ln 2 and atan, Newton's reciprocal — so a mistyped hex digit in a constant is
//! caught by a test instead of silently shifting every generated world. It is also the
//! exact argument-reduction engine of the high-precision reference used by the accuracy
//! tests. Nothing here is fast; nothing here needs to be.

#![allow(dead_code)]

/// Fraction limbs: 24 x 64 = 1536 bits after the binary point.
pub const FRAC: usize = 24;
const LEN: usize = FRAC + 1;

/// `limbs` little-endian; value = integer(limbs) / 2^(64 * FRAC). Always non-negative.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixed {
    pub limbs: [u64; LEN],
}

pub fn pow2(e: i32) -> f64 {
    assert!(
        (-1022..=1023).contains(&e),
        "pow2({e}) out of the normal range"
    );
    f64::from_bits(((e + 1023) as u64) << 52)
}

impl Fixed {
    pub fn zero() -> Self {
        Self { limbs: [0; LEN] }
    }

    pub fn from_u64(v: u64) -> Self {
        let mut f = Self::zero();
        f.limbs[FRAC] = v;
        f
    }

    /// Exact conversion of a finite, non-negative f64 whose bits all fall inside the
    /// representable range (true for every value used here).
    pub fn from_f64(x: f64) -> Self {
        assert!(x >= 0.0 && x.is_finite());
        let mut f = Self::zero();
        if x == 0.0 {
            return f;
        }
        let bits = x.to_bits();
        let be = ((bits >> 52) & 0x7ff) as i64;
        let (m, e) = if be == 0 {
            (bits & ((1 << 52) - 1), -1074)
        } else {
            ((bits & ((1 << 52) - 1)) | (1 << 52), be - 1075)
        };
        // value = m * 2^e ; bit position of m's bit 0 in the limb array = e + 64*FRAC.
        let pos = e + 64 * FRAC as i64;
        assert!(
            pos >= 0,
            "from_f64: {x:e} is below the fixed-point resolution"
        );
        let (q, r) = ((pos / 64) as usize, (pos % 64) as u32);
        let wide = (m as u128) << r;
        f.limbs[q] |= wide as u64;
        if q + 1 < LEN {
            f.limbs[q + 1] |= (wide >> 64) as u64;
        }
        f
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    pub fn add(&self, o: &Self) -> Self {
        let mut out = Self::zero();
        let mut carry = 0u128;
        for i in 0..LEN {
            let s = self.limbs[i] as u128 + o.limbs[i] as u128 + carry;
            out.limbs[i] = s as u64;
            carry = s >> 64;
        }
        assert_eq!(carry, 0, "Fixed overflow");
        out
    }

    /// `self - o`; panics if negative.
    pub fn sub(&self, o: &Self) -> Self {
        let mut out = Self::zero();
        let mut borrow = 0i128;
        for i in 0..LEN {
            let mut d = self.limbs[i] as i128 - o.limbs[i] as i128 - borrow;
            if d < 0 {
                d += 1 << 64;
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.limbs[i] = d as u64;
        }
        assert_eq!(borrow, 0, "Fixed underflow");
        out
    }

    pub fn ge(&self, o: &Self) -> bool {
        for i in (0..LEN).rev() {
            if self.limbs[i] != o.limbs[i] {
                return self.limbs[i] > o.limbs[i];
            }
        }
        true
    }

    pub fn mul_small(&self, k: u64) -> Self {
        let mut out = Self::zero();
        let mut carry = 0u128;
        for i in 0..LEN {
            let p = self.limbs[i] as u128 * k as u128 + carry;
            out.limbs[i] = p as u64;
            carry = p >> 64;
        }
        assert_eq!(carry, 0, "Fixed overflow");
        out
    }

    pub fn div_small(&self, k: u64) -> Self {
        let mut out = Self::zero();
        let mut rem = 0u128;
        for i in (0..LEN).rev() {
            let cur = (rem << 64) | self.limbs[i] as u128;
            out.limbs[i] = (cur / k as u128) as u64;
            rem = cur % k as u128;
        }
        out
    }

    /// Truncating product.
    pub fn mul(&self, o: &Self) -> Self {
        let mut wide = vec![0u64; 2 * LEN + 1];
        for i in 0..LEN {
            let mut carry = 0u128;
            for j in 0..LEN {
                let cur = wide[i + j] as u128 + self.limbs[i] as u128 * o.limbs[j] as u128 + carry;
                wide[i + j] = cur as u64;
                carry = cur >> 64;
            }
            let mut k = i + LEN;
            while carry != 0 {
                let cur = wide[k] as u128 + carry;
                wide[k] = cur as u64;
                carry = cur >> 64;
                k += 1;
            }
        }
        let mut out = Self::zero();
        out.limbs.copy_from_slice(&wide[FRAC..FRAC + LEN]);
        assert!(wide[FRAC + LEN..].iter().all(|&l| l == 0), "Fixed overflow");
        out
    }

    /// Index of the highest set bit (0 = lowest bit of limb 0), or None for zero.
    fn top_bit(&self) -> Option<i64> {
        (0..LEN)
            .rev()
            .find(|&i| self.limbs[i] != 0)
            .map(|i| i as i64 * 64 + 63 - self.limbs[i].leading_zeros() as i64)
    }

    /// 128 bits starting at bit `top` downwards, as a u128 (bit 127 = bit `top`).
    fn window128(&self, top: i64) -> u128 {
        let mut w = 0u128;
        for k in 0..128 {
            let b = top - k;
            if b < 0 {
                break;
            }
            let bit = (self.limbs[(b / 64) as usize] >> (b % 64)) & 1;
            w |= (bit as u128) << (127 - k);
        }
        w
    }

    /// Round-to-nearest f64 of the value (ties are irrelevant for these transcendental
    /// constants; the 128-bit window decides).
    pub fn to_f64(&self) -> f64 {
        let Some(top) = self.top_bit() else {
            return 0.0;
        };
        let w = self.window128(top);
        // value ~= w * 2^(top - 127 - 64*FRAC)
        let e = top - 127 - 64 * FRAC as i64;
        let hi = w as f64; // correctly rounded
        scale(hi, e)
    }

    /// The value truncated to its top `nbits` significant bits, as an exact f64.
    pub fn truncate_bits(&self, nbits: u32) -> f64 {
        let Some(top) = self.top_bit() else {
            return 0.0;
        };
        let w = self.window128(top) >> (128 - nbits);
        let e = top - (nbits as i64 - 1) - 64 * FRAC as i64;
        scale(w as f64, e)
    }

    /// (hi, lo) with hi = round(v), lo = round(v - hi): a double-double good to ~2^-106.
    pub fn to_dd(&self) -> (f64, f64) {
        let hi = self.to_f64();
        let h = Self::from_f64(hi);
        let lo = if self.ge(&h) {
            self.sub(&h).to_f64()
        } else {
            -h.sub(self).to_f64()
        };
        (hi, lo)
    }

    /// 64 bits of the fraction: word 0 holds the bits of weight 2^-1 .. 2^-64.
    pub fn frac_word(&self, k: usize) -> u64 {
        self.limbs[FRAC - 1 - k]
    }
}

fn scale(x: f64, e: i64) -> f64 {
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
    x * pow2(e as i32)
}

/// atan(1/n) by its Taylor series.
fn atan_inv(n: u64) -> Fixed {
    let mut pos = Fixed::zero();
    let mut neg = Fixed::zero();
    let mut power = Fixed::from_u64(1).div_small(n);
    let mut k = 0u64;
    while !power.is_zero() {
        let term = power.div_small(2 * k + 1);
        if k.is_multiple_of(2) {
            pos = pos.add(&term);
        } else {
            neg = neg.add(&term);
        }
        power = power.div_small(n * n);
        k += 1;
    }
    pos.sub(&neg)
}

/// pi by Machin's formula.
pub fn pi() -> Fixed {
    atan_inv(5).mul_small(16).sub(&atan_inv(239).mul_small(4))
}

/// ln 2 = sum_{k>=1} 1 / (k 2^k).
pub fn ln2() -> Fixed {
    let mut sum = Fixed::zero();
    let mut power = Fixed::from_u64(1).div_small(2);
    let mut k = 1u64;
    while !power.is_zero() {
        sum = sum.add(&power.div_small(k));
        power = power.div_small(2);
        k += 1;
    }
    sum
}

/// atan(num/den) for 0 < num/den < 1, by its Taylor series.
pub fn atan_ratio(num: u64, den: u64) -> Fixed {
    let mut pos = Fixed::zero();
    let mut neg = Fixed::zero();
    let mut power = Fixed::from_u64(num).div_small(den);
    let mut k = 0u64;
    while !power.is_zero() {
        let term = power.div_small(2 * k + 1);
        if k.is_multiple_of(2) {
            pos = pos.add(&term);
        } else {
            neg = neg.add(&term);
        }
        power = power.mul_small(num * num).div_small(den * den);
        k += 1;
    }
    pos.sub(&neg)
}

/// 1/a by Newton's iteration y <- y (2 - a y), from an f64 seed.
pub fn reciprocal(a: &Fixed, seed: f64) -> Fixed {
    let two = Fixed::from_u64(2);
    let mut y = Fixed::from_f64(seed);
    for _ in 0..6 {
        let ay = a.mul(&y);
        y = y.mul(&two.sub(&ay));
    }
    y
}

/// 2/pi.
pub fn two_over_pi() -> Fixed {
    reciprocal(&pi().div_small(2), 2.0 / std::f64::consts::PI)
}

/// |ln(num/den)| and whether ln(num/den) is negative, for `num, den > 0` with
/// `num + den < 2^32`, by `ln q = 2 atanh((q - 1)/(q + 1))`:
/// `2 sum_k s^(2k+1) / (2k+1)` with `s = |num - den| / (num + den)`.
pub fn ln_ratio(num: u64, den: u64) -> (Fixed, bool) {
    assert!(num > 0 && den > 0 && num + den < (1 << 32));
    let (a, b) = (num.abs_diff(den), num + den);
    let mut sum = Fixed::zero();
    let mut power = Fixed::from_u64(a).div_small(b);
    let mut k = 0u64;
    while !power.is_zero() {
        sum = sum.add(&power.div_small(2 * k + 1));
        power = power.mul_small(a * a).div_small(b).div_small(b);
        k += 1;
    }
    (sum.mul_small(2), num < den)
}

//! Bit-reproducible transcendentals (Ch.3.1 failure mode 3, Ch.3.2).
//!
//! Platform `libm`s (glibc, musl, Apple, MSVC's CRT) do not agree to the bit, and differ
//! between x86-64 and aarch64 on the same OS. Everything reachable from generation calls
//! these instead. They are built only from IEEE-754 `+ - * /` and `sqrt` in
//! round-to-nearest (whose results the standard fixes), integer operations and bit
//! manipulation. No FMA, no platform `floor`/`round`, no lookup into the C runtime, and
//! no iteration whose count depends on a computed value — so they return the same bits on
//! every conforming platform, at every optimisation level (Rust never contracts or
//! reassociates floating point).
//!
//! Accuracy (measured by `tests/accuracy.rs` against a 106-bit reference; see
//! `docs/adr/0003-forge-num-deterministic-math.md`): every function is within 1 ulp,
//! most well under.
//!
//! Every NaN returned is the canonical quiet NaN `0x7FF8_0000_0000_0000`, whatever NaN
//! (or operation) produced it: hardware default NaNs differ in sign between x86-64 and
//! aarch64, and a NaN is hashed like any other value.

mod atan;
pub(crate) mod consts;
mod explog;
mod ln_table;
mod trig;

pub use atan::atan2;
pub use explog::{exp, ln, pow};
pub use trig::{cos, sin};

/// IEEE-754 square root — the one operation the standard specifies to the bit, so it is
/// forwarded to the hardware instruction. A negative or NaN input returns the canonical NaN
/// (the hardware's own NaN differs between x86-64 and aarch64).
#[inline]
pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        consts::NAN
    } else {
        x.sqrt()
    }
}

/// Horner evaluation `c[0] + x (c[1] + x (c[2] + ...))`, innermost first. The loop count
/// is the (constant) coefficient count, never data-dependent, and the operation order is
/// exactly that of the written-out nested form.
#[inline(always)]
pub(crate) fn horner(x: f64, c: &[f64]) -> f64 {
    match c.split_last() {
        None => 0.0,
        Some((&last, rest)) => rest.iter().rev().fold(last, |acc, &k| k + x * acc),
    }
}

#[cfg(test)]
#[path = "../../tests/support/bignum.rs"]
mod bignum;

#[cfg(test)]
mod tests {
    use super::consts::*;
    use super::*;
    use crate::dd::Dd;

    fn dd_bits(d: Dd) -> (u64, u64) {
        (d.hi.to_bits(), d.lo.to_bits())
    }

    /// Every embedded constant, re-derived from first principles (Machin, series, Newton).
    #[test]
    fn constants_match_first_principles() {
        use super::bignum::*;
        let tp = two_over_pi();
        for (k, &w) in TWO_OVER_PI.iter().enumerate() {
            assert_eq!(w, tp.frac_word(k), "TWO_OVER_PI[{k}]");
        }
        let pi = pi();
        let pio2 = pi.div_small(2);
        let p1 = pio2.truncate_bits(33);
        let r = pio2.sub(&Fixed::from_f64(p1));
        let p2 = r.truncate_bits(33);
        let r = r.sub(&Fixed::from_f64(p2));
        let p3 = r.truncate_bits(33);
        let r = r.sub(&Fixed::from_f64(p3));
        assert_eq!(
            [PIO2_1, PIO2_2, PIO2_3, PIO2_4].map(f64::to_bits),
            [p1, p2, p3, r.to_f64()].map(f64::to_bits)
        );
        let dd = |f: Fixed| {
            let (h, l) = f.to_dd();
            (h.to_bits(), l.to_bits())
        };
        assert_eq!(dd_bits(PIO2_DD), dd(pio2.clone()));
        assert_eq!(dd_bits(PIO4_DD), dd(pi.div_small(4)));
        assert_eq!(dd_bits(THREE_PIO4_DD), dd(pi.mul_small(3).div_small(4)));
        assert_eq!(dd_bits(PI_DD), dd(pi.clone()));
        assert_eq!(PIO4.to_bits(), pi.div_small(4).to_f64().to_bits());
        assert_eq!(INV_PIO2.to_bits(), tp.to_f64().to_bits());

        let l2 = ln2();
        let lh = l2.truncate_bits(42);
        assert_eq!(LN2_HI.to_bits(), lh.to_bits());
        assert_eq!(
            LN2_LO.to_bits(),
            l2.sub(&Fixed::from_f64(lh)).to_f64().to_bits()
        );
        let inv_ln2 = reciprocal(&l2, std::f64::consts::LOG2_E);
        assert_eq!(INV_LN2.to_bits(), inv_ln2.to_f64().to_bits());

        for (j, &a) in ATAN_J8.iter().enumerate().skip(1).take(7) {
            assert_eq!(dd_bits(a), dd(atan_ratio(j as u64, 8)), "ATAN_J8[{j}]");
        }
        assert_eq!(dd_bits(ATAN_J8[8]), dd(pi.div_small(4)));
        assert_eq!(dd_bits(THIRD_DD), dd(Fixed::from_u64(1).div_small(3)));
    }

    #[test]
    fn positive_control_a_one_bit_error_in_a_constant_is_caught() {
        use super::bignum::*;
        let mut table = TWO_OVER_PI;
        table[7] ^= 1 << 17;
        let tp = two_over_pi();
        assert!(
            table.iter().enumerate().any(|(k, &w)| w != tp.frac_word(k)),
            "the constant re-derivation would not notice a flipped bit"
        );
    }

    #[test]
    fn special_values() {
        let nan = f64::NAN;
        let inf = f64::INFINITY;
        let canon = NAN.to_bits();
        for v in [
            sin(nan),
            sin(inf),
            cos(-inf),
            exp(nan),
            ln(-1.0),
            ln(nan),
            pow(-2.0, 0.5),
            pow(nan, 2.0),
            atan2(nan, 1.0),
            sqrt(-1.0),
            sqrt(-nan),
        ] {
            assert_eq!(v.to_bits(), canon, "non-canonical NaN");
        }
        assert_eq!(sin(-0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(cos(0.0), 1.0);
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(exp(inf), inf);
        assert_eq!(exp(-inf), 0.0);
        assert_eq!(exp(710.0), inf);
        assert_eq!(exp(-746.0), 0.0);
        assert!(exp(-740.0) > 0.0 && exp(-740.0) < f64::MIN_POSITIVE);
        assert_eq!(ln(1.0), 0.0);
        assert_eq!(ln(0.0), -inf);
        assert_eq!(ln(-0.0), -inf);
        assert_eq!(ln(inf), inf);
        assert!(ln(f64::from_bits(1)) < -744.0);
        assert_eq!(pow(nan, 0.0), 1.0);
        assert_eq!(pow(1.0, nan), 1.0);
        assert_eq!(pow(-1.0, inf), 1.0);
        assert_eq!(pow(0.5, inf), 0.0);
        assert_eq!(pow(0.5, -inf), inf);
        assert_eq!(pow(2.0, inf), inf);
        assert_eq!(pow(-0.0, 3.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(pow(-0.0, -3.0), -inf);
        assert_eq!(pow(-0.0, -2.0), inf);
        assert_eq!(pow(-inf, 3.0), -inf);
        assert_eq!(pow(-inf, -3.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(pow(-2.0, 3.0), -8.0);
        assert_eq!(pow(-2.0, 2.0), 4.0);
        assert_eq!(pow(2.0, 10.0), 1024.0);
        assert_eq!(pow(2.0, -1074.0), f64::from_bits(1));
        assert_eq!(pow(10.0, 309.0), inf);
        assert_eq!(pow(3.0, 0.5), sqrt(3.0));
        assert_eq!(atan2(0.0, -0.0), std::f64::consts::PI);
        assert_eq!(atan2(-0.0, -0.0), -std::f64::consts::PI);
        assert_eq!(atan2(-0.0, 0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(atan2(1.0, 0.0), std::f64::consts::FRAC_PI_2);
        assert_eq!(atan2(inf, -inf), 3.0 * std::f64::consts::FRAC_PI_4);
        assert_eq!(atan2(-inf, inf), -std::f64::consts::FRAC_PI_4);
        assert_eq!(atan2(1.0, 1.0), std::f64::consts::FRAC_PI_4);
        assert_eq!(atan2(1.0, -inf), std::f64::consts::PI);
        assert_eq!(sqrt(-0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(sqrt(4.0), 2.0);
    }

    #[test]
    fn huge_arguments_reduce_consistently() {
        // sin^2 + cos^2 = 1 across the Payne-Hanek range, and the reduction is odd.
        for x in [1.7e6, 1e10, 1e22, 1e100, 1e300, f64::MAX] {
            let (s, c) = (sin(x), cos(x));
            assert!((s * s + c * c - 1.0).abs() < 1e-15, "x={x:e}");
            assert_eq!(sin(-x).to_bits(), (-s).to_bits());
            assert_eq!(cos(-x).to_bits(), c.to_bits());
        }
        // 1e22 is a classic: sin(1e22) = -0.8522008497671888...
        assert_eq!(sin(1e22), -0.852_200_849_767_188_8);
    }

    /// The rule `LN_TABLE` is built by: bucket `i` covers the bit patterns
    /// `[OFF + i 2^45, OFF + (i+1) 2^45)`; its `1/c` is `N / 2^20` with
    /// `N = round(2^20 / centre)`, except the bucket holding 1.0, where `1/c = 1` exactly
    /// (so `ln c = 0` and `ln x` near 1 keeps full relative accuracy). `ln c` comes from the
    /// bignum, never from memory.
    fn ln_table_rule() -> Vec<(f64, (u64, u64))> {
        use super::bignum::*;
        use super::ln_table::*;
        (0..LN_TABLE_SIZE)
            .map(|i| {
                let lo = f64::from_bits(LN_TABLE_OFF + ((i as u64) << 45));
                let hi = f64::from_bits(LN_TABLE_OFF + ((i as u64 + 1) << 45));
                let n = if i == LN_TABLE_ONE {
                    1u64 << 20
                } else {
                    // lo + hi is exact (both carry at most 8 significant bits).
                    (pow2(21) / (lo + hi)).round() as u64
                };
                let (mag, negative) = ln_ratio(1 << 20, n); // ln(1 / (1/c)) = ln c
                let (h, l) = mag.to_dd();
                let (h, l) = if negative { (-h, -l) } else { (h, l) };
                (n as f64 * pow2(-20), (h.to_bits(), l.to_bits()))
            })
            .collect()
    }

    #[test]
    #[ignore = "generator: prints the rows of det/ln_table.rs"]
    fn print_ln_table() {
        for (inv_c, (h, l)) in ln_table_rule() {
            let n = (inv_c * 1_048_576.0) as u64;
            if n == 1 << 20 {
                println!("    (1.0, 0.0, 0.0), // the bucket holding 1.0 (LN_TABLE_ONE)");
                continue;
            }
            println!(
                "    ({n}.0 / 1_048_576.0, f64::from_bits(0x{:04X}_{:04X}_{:04X}_{:04X}), f64::from_bits(0x{:04X}_{:04X}_{:04X}_{:04X})),",
                h >> 48,
                (h >> 32) & 0xFFFF,
                (h >> 16) & 0xFFFF,
                h & 0xFFFF,
                l >> 48,
                (l >> 32) & 0xFFFF,
                (l >> 16) & 0xFFFF,
                l & 0xFFFF
            );
        }
    }

    /// Every `LN_TABLE` row equals the rule's, to the bit.
    #[test]
    fn ln_table_matches_first_principles() {
        use super::ln_table::*;
        for (i, (&(inv_c, h, l), (rinv, (rh, rl)))) in
            LN_TABLE.iter().zip(ln_table_rule()).enumerate()
        {
            assert_eq!(inv_c.to_bits(), rinv.to_bits(), "LN_TABLE[{i}].0");
            assert_eq!((h.to_bits(), l.to_bits()), (rh, rl), "LN_TABLE[{i}] ln c");
        }
        let (c, h, l) = LN_TABLE[LN_TABLE_ONE];
        assert_eq!((c, h, l), (1.0, 0.0, 0.0));
    }

    /// `ln_dd`'s error bound assumes `|z / c - 1| <= 2^-8` over every bucket (the
    /// truncated series and the double-precision tail are sized for it), and exact products
    /// assume `1/c` has at most 21 significant bits.
    #[test]
    fn ln_table_reduction_bound_holds() {
        use super::ln_table::*;
        for (i, &(inv_c, _, _)) in LN_TABLE.iter().enumerate() {
            let n = inv_c * pow2(20);
            assert_eq!(n, n.trunc(), "bucket {i}: 1/c is not a multiple of 2^-20");
            assert!(n < pow2(21), "bucket {i}: 1/c has more than 21 bits");
            for edge in [i as u64, i as u64 + 1] {
                let z = f64::from_bits(LN_TABLE_OFF + (edge << 45));
                let r = (z * inv_c - 1.0).abs();
                assert!(r <= pow2(-8) * 1.0001, "bucket {i}: |r| = {r:e} at an edge");
            }
        }
    }

    #[test]
    fn positive_control_a_one_bit_error_in_the_ln_table_is_caught() {
        use super::ln_table::*;
        let rule = ln_table_rule();
        let mut row = LN_TABLE[37];
        row.2 = f64::from_bits(row.2.to_bits() ^ 1);
        assert_ne!(
            (row.1.to_bits(), row.2.to_bits()),
            rule[37].1,
            "the ln table re-derivation would not notice a flipped low bit"
        );
    }
}

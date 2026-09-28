//! `forge-num` — deterministic math (Ch.3). Depends on nothing: not another crate, and
//! not the platform's transcendental functions.
//!
//! * [`det`] — bit-reproducible `sin cos exp ln pow atan2` (vendored) and `sqrt`
//!   (forwarded: IEEE-exact).
//! * [`newton`] — exactly `N` Newton steps; the count is in the type (Ch.3.1 mode 2).
//! * [`hash3`], [`value_noise_i`], [`simplex_i`], [`fbm_i`] — the noise basis over integers
//!   (Ch.3.1 mode 1).
//! * [`tree_sum`] — the fixed-order reduction (Ch.3.3).
//! * [`DVec3`], [`DQuat`] — minimal bit-reproducible f64 vector and rotation types (ADR
//!   0004). A `DVec3` is never a position on its own: see `forge_frames::FramePos` (I1).
//!
//! The golden-hash gate over all of it is `tests/determinism/test_cross_platform_hash.rs`
//! (invariant I2).

#![forbid(unsafe_code)]

mod dd;
pub mod det;
pub mod linalg;
mod noise;
mod reduce;

pub use linalg::{DQuat, DVec2, DVec3};
pub use noise::{
    IVec3, MAX_OCTAVES, MAX_SCALE_LOG2, MIN_SCALE_LOG2, fbm_i, hash3, simplex_i, value_noise_i,
};
pub use reduce::{tree_split, tree_sum};

/// The engine's real number type below `forge-render` (Ch.1.5: no `f32` down here).
pub type Real = f64;

/// True in the `mutate-det` build — the Ch.3.4 positive control, which perturbs octave 0 of
/// [`fbm_i`] by 1 ulp. A shipping build that sees `true` here is misconfigured.
pub const MUTATE_DET: bool = cfg!(feature = "mutate-det");

/// Exactly `N` Newton-Raphson steps from `x0`, where `f(x)` returns `(f(x), f'(x))`.
///
/// There is no convergence test and no early exit: an iteration count that depends on a
/// computed value differs across compilers and platforms (Ch.3.1, failure mode 2), so the
/// count lives in the type where nothing can make it data-dependent. A zero derivative
/// yields an infinity or NaN, deterministically.
///
/// ```
/// // sqrt(2) as the root of x^2 - 2: five steps from 1.0.
/// let r = forge_num::newton::<5>(|x| (x * x - 2.0, 2.0 * x), 1.0);
/// assert_eq!(r, forge_num::det::sqrt(2.0));
/// ```
#[inline]
pub fn newton<const N: usize>(f: impl Fn(f64) -> (f64, f64), x0: f64) -> f64 {
    let mut x = x0;
    for _ in 0..N {
        let (fx, dfx) = f(x);
        x -= fx / dfx;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn newton_takes_exactly_n_steps_even_after_convergence() {
        let calls = Cell::new(0);
        let r = newton::<9>(
            |x| {
                calls.set(calls.get() + 1);
                (x * x - 4.0, 2.0 * x)
            },
            3.0,
        );
        assert_eq!(r, 2.0);
        assert_eq!(calls.get(), 9);
        let calls = Cell::new(0);
        newton::<0>(
            |x| {
                calls.set(calls.get() + 1);
                (x, 1.0)
            },
            1.0,
        );
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn newton_five_steps() {
        // The Foundations' rule: exactly five Newton steps in f64 (here on E - e sin E = M).
        let (e, m) = (0.3, 1.2);
        let big_e = newton::<5>(|x| (x - e * det::sin(x) - m, 1.0 - e * det::cos(x)), m);
        assert!((big_e - e * det::sin(big_e) - m).abs() < 1e-15);
    }

    #[test]
    fn mutate_det_flag_reflects_the_feature() {
        assert_eq!(MUTATE_DET, cfg!(feature = "mutate-det"));
    }
}

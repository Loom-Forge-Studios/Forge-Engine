//! Fixed-order reductions (Ch.3.3). Floating-point addition is not associative, so a
//! work-stealing reduction (`rayon::sum()`) returns different bits run to run. Generation
//! sums with `tree_sum`, whose bracketing is a pure function of the slice length — a
//! parallel implementation that splits at the same points returns the same bits.

/// The split point of a node covering `n >= 2` elements: the largest power of two
/// strictly below `n`. Public so a parallel reduction can split identically.
#[inline]
pub const fn tree_split(n: usize) -> usize {
    debug_assert!(n >= 2);
    1usize << (usize::BITS - 1 - (n - 1).leading_zeros())
}

/// Sum `xs` with the canonical pairwise bracketing:
/// `sum(xs) = sum(xs[..m]) + sum(xs[m..])`, `m = tree_split(len)`; empty sums to +0.
/// Also more accurate than a left fold (error grows with log n, not n).
pub fn tree_sum(xs: &[f64]) -> f64 {
    match xs {
        [] => 0.0,
        [x] => *x,
        _ => {
            let (l, r) = xs.split_at(tree_split(xs.len()));
            tree_sum(l) + tree_sum(r)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_is_the_largest_power_of_two_below_n() {
        assert_eq!(tree_split(2), 1);
        assert_eq!(tree_split(3), 2);
        assert_eq!(tree_split(4), 2);
        assert_eq!(tree_split(5), 4);
        assert_eq!(tree_split(8), 4);
        assert_eq!(tree_split(9), 8);
    }

    #[test]
    fn bracketing_is_canonical() {
        let xs = [1e16, 1.0, -1e16, 1.0, 3.0];
        // ((x0 + x1) + (x2 + x3)) + x4
        let want = ((xs[0] + xs[1]) + (xs[2] + xs[3])) + xs[4];
        assert_eq!(tree_sum(&xs).to_bits(), want.to_bits());
    }

    #[test]
    fn a_parallel_reduction_splitting_at_tree_split_matches() {
        let xs: Vec<f64> = (0..10_007)
            .map(|i| ((i * 7919) % 1000) as f64 * 1e-3 + (i as f64) * 1e7)
            .collect();
        let m = tree_split(xs.len());
        let (a, b) = std::thread::scope(|s| {
            let l = s.spawn(|| tree_sum(&xs[..m]));
            let r = s.spawn(|| tree_sum(&xs[m..]));
            (l.join().unwrap(), r.join().unwrap())
        });
        assert_eq!((a + b).to_bits(), tree_sum(&xs).to_bits());
    }

    #[test]
    fn positive_control_a_left_fold_differs() {
        // The guard above is not vacuous: order matters for this data.
        let xs = [1e16, 1.0, -1e16, 1.0, 3.0];
        let fold: f64 = xs.iter().fold(0.0, |a, &b| a + b);
        assert_ne!(fold.to_bits(), tree_sum(&xs).to_bits());
    }
}

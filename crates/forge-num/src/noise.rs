//! The noise basis (Ch.3.1 failure mode 1, Ch.3.2). **It takes integers.** There is no
//! float overload and there never will be: a float world position has 24 (f32) or a
//! distance-dependent number of (f64) fractional bits, and noise evaluated on it diverges
//! between machines and between distances from the origin.
//!
//! `simplex_i` goes further than hashing integers: the *whole* lattice computation —
//! skew, cell selection, simplex ordering, corner offsets, the kernel's support test —
//! is exact integer arithmetic (3D simplex's skew factors are exactly 1/3 and 1/6, so a
//! fixed-point denominator of `6 * 2^scale` makes every quantity an integer). Floating point
//! enters only for the final kernel weights, each produced by a fixed sequence of
//! correctly-rounded operations. A given `(p, scale, seed)` is therefore bit-identical on
//! every platform *and* equally precise at the origin and a billion units away.
//!
//! Seeds are plain `u64` here: `forge-num` depends on nothing, and `forge-seed`'s `Seed`
//! wraps and derives them (Ch.4).

/// An integer lattice point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IVec3 {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

impl IVec3 {
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }
}

/// The finaliser of SplitMix64 (Stafford's variant 13): a bijection on `u64` with full
/// avalanche.
#[inline(always)]
const fn mix(mut v: u64) -> u64 {
    v = (v ^ (v >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    v ^ (v >> 31)
}

/// A 64-bit hash of an integer lattice point under a seed. Every step is a bijection of
/// the running state keyed by one coordinate, so changing any single input changes the
/// output; the per-axis multipliers keep permuted coordinates from colliding.
#[inline]
pub const fn hash3(x: i64, y: i64, z: i64, seed: u64) -> u64 {
    let mut h = mix(seed ^ 0x9E37_79B9_7F4A_7C15);
    h = mix(h ^ (x as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93));
    h = mix(h ^ (y as u64).wrapping_mul(0xA076_1D64_78BD_642F));
    mix(h ^ (z as u64).wrapping_mul(0xE703_7ED1_A0B4_28DB))
}

/// Map a hash to [-1, 1) with 53 bits of resolution (exact conversion, one exact scale).
#[inline(always)]
fn unit(h: u64) -> f64 {
    const TWO_POW_M52: f64 = 1.0 / 4_503_599_627_370_496.0; // 2^-52
    (h >> 11) as f64 * TWO_POW_M52 - 1.0
}

/// White (lattice) value noise at an integer point: uniform in [-1, 1).
#[inline]
pub fn value_noise_i(p: IVec3, seed: u64) -> f64 {
    unit(hash3(p.x, p.y, p.z, seed))
}

/// The finest feature size `simplex_i` accepts, as `log2` lattice units (`2^-24`).
pub const MIN_SCALE_LOG2: i8 = -24;
/// The coarsest feature size `simplex_i` accepts (`2^56` lattice units). Beyond this the
/// exact 128-bit fixed-point kernel would overflow; arguments are clamped into range.
pub const MAX_SCALE_LOG2: i8 = 56;

const GRAD3: [[i32; 3]; 12] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
];

/// The integer width the exact simplex kernel runs in. Both widths do the same exact
/// integer arithmetic and correctly-rounded integer-to-float conversions, so they return
/// the same bits; `i64` is used whenever every intermediate provably fits, because i128
/// multiplication, division and conversion to f64 are several times slower (library calls).
trait Lattice:
    Copy
    + Ord
    + From<i32>
    + std::ops::Add<Output = Self>
    + std::ops::Sub<Output = Self>
    + std::ops::Mul<Output = Self>
    + std::ops::Shl<u32, Output = Self>
    + std::ops::Shr<u32, Output = Self>
{
    fn floor_div3(self) -> Self;
    /// The 64 bits the hash takes: for values that fit in i64 this is the same in both
    /// widths (low word XOR sign extension).
    fn fold(self) -> i64;
    fn to_f64(self) -> f64;
}

impl Lattice for i64 {
    #[inline(always)]
    fn floor_div3(self) -> Self {
        self.div_euclid(3)
    }
    #[inline(always)]
    fn fold(self) -> i64 {
        self ^ (self >> 63)
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }
}

impl Lattice for i128 {
    #[inline(always)]
    fn floor_div3(self) -> Self {
        self.div_euclid(3)
    }
    #[inline(always)]
    fn fold(self) -> i64 {
        (self as i64) ^ ((self >> 64) as i64)
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }
}

/// The exact simplex kernel at the sample point `num / 2^sh`.
#[inline(always)]
fn simplex_kernel<T: Lattice>(num: [T; 3], sh: u32, seed: u64) -> f64 {
    let k = |v: i32| T::from(v);
    let u: T = k(1) << sh;
    // Skew by F3 = 1/3: 3U * X'_c = 3 num_c + sum(num). Cell = floor(X') =
    // floor(floor(A / 2^sh) / 3) — nested floors compose for positive divisors.
    let sum = num[0] + num[1] + num[2];
    let cell = [
        ((k(3) * num[0] + sum) >> sh).floor_div3(),
        ((k(3) * num[1] + sum) >> sh).floor_div3(),
        ((k(3) * num[2] + sum) >> sh).floor_div3(),
    ];
    // Unskew by G3 = 1/6: offset from the cell origin, scaled by 6U, exactly.
    let t = cell[0] + cell[1] + cell[2];
    let six_u = k(6) * u;
    let d0 = [
        k(6) * num[0] - six_u * cell[0] + u * t,
        k(6) * num[1] - six_u * cell[1] + u * t,
        k(6) * num[2] - six_u * cell[2] + u * t,
    ];
    // Which of the six simplices: exact integer comparisons.
    let (e1, e2): ([i32; 3], [i32; 3]) = if d0[0] >= d0[1] {
        if d0[1] >= d0[2] {
            ([1, 0, 0], [1, 1, 0])
        } else if d0[0] >= d0[2] {
            ([1, 0, 0], [1, 0, 1])
        } else {
            ([0, 0, 1], [1, 0, 1])
        }
    } else if d0[1] < d0[2] {
        ([0, 0, 1], [0, 1, 1])
    } else if d0[0] < d0[2] {
        ([0, 1, 0], [0, 1, 1])
    } else {
        ([0, 1, 0], [1, 1, 0])
    };
    let corner = |e: [i32; 3], shift: i32| -> [T; 3] {
        [
            d0[0] - six_u * k(e[0]) + u * k(shift),
            d0[1] - six_u * k(e[1]) + u * k(shift),
            d0[2] - six_u * k(e[2]) + u * k(shift),
        ]
    };
    let corners: [([i32; 3], [T; 3]); 4] = [
        ([0, 0, 0], d0),
        (e1, corner(e1, 1)),
        (e2, corner(e2, 2)),
        ([1, 1, 1], corner([1, 1, 1], 3)),
    ];
    // Kernel (0.6 - |d|^2)^4 (g . d). With d = D / 6U:
    //   0.6 - |d|^2 = (108 U^2 - 5 |D|^2) / (180 U^2)   — support test in exact integers.
    let u2 = u * u;
    let support = k(108) * u2;
    let t_den = 180.0 * u2.to_f64(); // exact: a small integer times a power of two
    let d_den = six_u.to_f64(); // exact
    let mut acc = 0.0f64;
    for (off, d) in corners {
        let n2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        let tn = support - k(5) * n2;
        if tn <= k(0) {
            continue;
        }
        let h = hash3(
            (cell[0] + k(off[0])).fold(),
            (cell[1] + k(off[1])).fold(),
            (cell[2] + k(off[2])).fold(),
            seed,
        );
        let g = GRAD3[(((h >> 32) * 12) >> 32) as usize];
        let gd = k(g[0]) * d[0] + k(g[1]) * d[1] + k(g[2]) * d[2];
        let tt = tn.to_f64() / t_den;
        let t2 = tt * tt;
        acc += t2 * t2 * (gd.to_f64() / d_den);
    }
    32.0 * acc
}

/// Below these bounds every intermediate of the kernel fits in i64 with margin: with
/// |num| < 2^56 and U <= 2^20, `3 num + sum` < 2^58.6, `6U cell` ~ 2 (3 num + sum) < 2^59.6,
/// `U t` < 2^58.6, and |D| <= ~6U * 1.5, so `|D|^2` and `108 U^2` < 2^50 — all under 2^61.
const I64_NUM_LIMIT: i128 = 1 << 56;
const I64_MAX_SHIFT: u32 = 20;

/// 3D simplex noise at the integer point `p`, with features `2^scale_log2` lattice units
/// across (negative `scale_log2` = features smaller than one lattice step). Output is in
/// [-1, 1]. `scale_log2` is clamped to [`MIN_SCALE_LOG2`, `MAX_SCALE_LOG2`].
///
/// Exact for every `p` (any `i64`) and every accepted scale: the sample point `p / 2^s` is
/// never formed as a float.
pub fn simplex_i(p: IVec3, scale_log2: i8, seed: u64) -> f64 {
    let s = scale_log2.clamp(MIN_SCALE_LOG2, MAX_SCALE_LOG2) as i32;
    // Sample point X = num / U with U = 2^sh.
    let (num, sh) = if s >= 0 {
        ([p.x as i128, p.y as i128, p.z as i128], s as u32)
    } else {
        let k = (-s) as u32;
        (
            [(p.x as i128) << k, (p.y as i128) << k, (p.z as i128) << k],
            0,
        )
    };
    let fits = sh <= I64_MAX_SHIFT && num.iter().all(|&v| v.abs() < I64_NUM_LIMIT);
    if fits {
        simplex_kernel::<i64>(num.map(|v| v as i64), sh, seed)
    } else {
        simplex_kernel::<i128>(num, sh, seed)
    }
}

/// The most octaves `fbm_i` sums.
pub const MAX_OCTAVES: u32 = 24;

/// Fractal (fBm) sum of `octaves` simplex octaves at `p`: octave `o` has features
/// `2^(base_scale_log2 - o)` across, amplitude `2^-o`, and its own seed. Normalised by the
/// amplitude sum, so the output stays in [-1, 1]. Octaves are summed in a fixed order.
///
/// Under the `mutate-det` feature (the Ch.3.4 positive control ONLY), octave 0 is perturbed
/// by 1 ulp, which the golden-hash gate must detect.
pub fn fbm_i(p: IVec3, base_scale_log2: i8, octaves: u32, seed: u64) -> f64 {
    let mut sum = 0.0f64;
    let mut amp = 1.0f64;
    let mut norm = 0.0f64;
    for o in 0..octaves.min(MAX_OCTAVES) {
        let s = (base_scale_log2 as i32 - o as i32).clamp(i8::MIN as i32, i8::MAX as i32) as i8;
        let octave_seed = mix(seed ^ (o as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let v = simplex_i(p, s, octave_seed);
        #[cfg(feature = "mutate-det")]
        let v = if o == 0 {
            f64::from_bits(v.to_bits() ^ 1)
        } else {
            v
        };
        sum += amp * v;
        norm += amp;
        amp *= 0.5;
    }
    if norm > 0.0 { sum / norm } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash3_is_sensitive_to_every_input() {
        let base = hash3(1, 2, 3, 42);
        assert_ne!(base, hash3(2, 2, 3, 42));
        assert_ne!(base, hash3(1, 3, 3, 42));
        assert_ne!(base, hash3(1, 2, 4, 42));
        assert_ne!(base, hash3(1, 2, 3, 43));
        assert_ne!(
            hash3(1, 2, 3, 0),
            hash3(3, 2, 1, 0),
            "permutation collision"
        );
        assert_ne!(hash3(0, 0, 0, 0), 0);
    }

    #[test]
    fn hash3_avalanches() {
        // Flipping one input bit flips ~half the output bits, on average.
        let mut total = 0u32;
        let mut n = 0u32;
        for i in 0..64 {
            for b in 0..64 {
                let a = hash3(i, i * 7, -i, 9);
                let c = hash3(i ^ (1 << b), i * 7, -i, 9);
                total += (a ^ c).count_ones();
                n += 1;
            }
        }
        let mean = total as f64 / n as f64;
        assert!((mean - 32.0).abs() < 1.0, "mean flipped bits {mean}");
    }

    #[test]
    fn value_noise_is_in_range_and_roughly_uniform() {
        let mut mean = 0.0;
        for i in 0..10_000 {
            let v = value_noise_i(IVec3::new(i, -i, i * 3), 5);
            assert!((-1.0..1.0).contains(&v));
            mean += v;
        }
        assert!((mean / 10_000.0).abs() < 0.03);
    }

    #[test]
    fn simplex_is_bounded_continuous_and_nontrivial() {
        let mut lo = f64::MAX;
        let mut hi = f64::MIN;
        let mut max_step = 0.0f64;
        for z in 0..8 {
            for y in 0..64 {
                let mut prev = simplex_i(IVec3::new(0, y, z), 4, 11);
                for x in 1..64 {
                    let v = simplex_i(IVec3::new(x, y, z), 4, 11);
                    assert!((-1.0..=1.0).contains(&v), "{v}");
                    max_step = max_step.max((v - prev).abs());
                    prev = v;
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
        }
        assert!(lo < -0.5 && hi > 0.5, "range [{lo}, {hi}] is too narrow");
        // Continuity: the kernel slope is at most 32 * 0.6^4 * sqrt(2) ~= 5.9 per feature
        // unit per corner and the corners overlap little, so a 1/16-feature step moves the
        // value by well under 0.5; a lattice seam (a discontinuity) would jump by ~1.
        assert!(max_step < 0.5, "max step {max_step}");
    }

    #[test]
    fn simplex_is_exact_far_from_the_origin() {
        // Translation by a whole number of simplex-lattice periods is not a symmetry, but
        // noise far out must be as detailed as at the origin: same spread of values.
        let far = 1i64 << 50;
        let mut spread = 0.0f64;
        for x in 0..256 {
            let v = simplex_i(IVec3::new(far + x, far - 3 * x, far), 3, 1);
            let w = simplex_i(IVec3::new(far + x + 1, far - 3 * x, far), 3, 1);
            spread = spread.max((v - w).abs());
            assert!((-1.0..=1.0).contains(&v));
        }
        assert!(spread > 0.05, "noise far from the origin has collapsed");
        // Extreme coordinates and scales do not overflow or panic.
        for s in [i8::MIN, -24, -1, 0, 1, 30, 56, i8::MAX] {
            for p in [
                IVec3::new(i64::MAX, i64::MIN, 0),
                IVec3::new(i64::MIN, i64::MIN, i64::MIN),
                IVec3::new(-1, 0, 1),
            ] {
                let v = simplex_i(p, s, 3);
                assert!(v.is_finite() && (-1.0..=1.0).contains(&v), "{v}");
            }
        }
    }

    #[test]
    fn i64_and_i128_kernels_agree_bit_for_bit() {
        // The fast path must be an optimisation only: same bits as the i128 kernel, right
        // up to the fast-path bounds.
        let mut s = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            s = mix(s.wrapping_add(0x9E37_79B9_7F4A_7C15));
            s
        };
        for i in 0..20_000 {
            let lim = 1i64 << (8 + (i % 49)); // up to 2^56
            let num = [
                (next() as i64) % lim,
                (next() as i64) % lim,
                (next() as i64) % lim,
            ];
            let sh = (next() % (I64_MAX_SHIFT as u64 + 1)) as u32;
            let seed = next();
            let a = simplex_kernel::<i64>(num, sh, seed);
            let b = simplex_kernel::<i128>(num.map(i128::from), sh, seed);
            assert_eq!(a.to_bits(), b.to_bits(), "num={num:?} sh={sh}");
        }
    }

    #[test]
    fn positive_control_the_width_comparison_can_fail() {
        // Differing inputs give differing bits: the comparison above is not vacuous.
        let a = simplex_kernel::<i64>([5, 7, 9], 2, 1);
        let b = simplex_kernel::<i128>([5, 7, 9], 2, 2);
        assert_ne!(a.to_bits(), b.to_bits());
    }

    #[test]
    fn simplex_scale_is_consistent() {
        // Doubling both the point and the feature size samples the same continuous point.
        for i in -50..50 {
            let p = IVec3::new(i * 3, i * 5 - 7, 11 - i);
            let q = IVec3::new(p.x * 2, p.y * 2, p.z * 2);
            assert_eq!(simplex_i(p, 3, 77).to_bits(), simplex_i(q, 4, 77).to_bits());
            assert_eq!(
                simplex_i(p, -2, 77).to_bits(),
                simplex_i(q, -1, 77).to_bits()
            );
        }
    }

    #[test]
    fn fbm_is_bounded_and_seeded() {
        let p = IVec3::new(1000, -2000, 3000);
        let a = fbm_i(p, 10, 6, 1);
        let b = fbm_i(p, 10, 6, 2);
        assert_ne!(a, b);
        assert!((-1.0..=1.0).contains(&a));
        assert_eq!(fbm_i(p, 10, 0, 1), 0.0);
    }
}

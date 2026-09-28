//! Accuracy of `forge_num::det` against a ~2^-90 reference (tests/support/reference.rs).
//!
//! The contract (Ch.3.1: "correctly-rounded or at minimum bit-reproducible") is met by
//! construction — bit-reproducibility is guarded by the golden-hash gate. This test pins the
//! *accuracy* half: every function is faithfully rounded (|error| < 1 ulp) on every sampled
//! domain, and the table it prints (`cargo test -p forge-num --release --test accuracy --
//! --nocapture`, with `FORGE_ACCURACY_SCALE=20` for the full run) is the one recorded in
//! docs/adr/0003-forge-num-deterministic-math.md.

#[path = "support/bignum.rs"]
mod bignum;
#[path = "support/reference.rs"]
mod reference;

use forge_num::det;
use reference::{R, ulp_error};

/// The accuracy contract: faithful rounding.
const MAX_ULP: f64 = 1.0;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [a, b).
    fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }
    /// Log-uniform magnitude with binary exponent in [lo, hi], random mantissa.
    fn log_uniform(&mut self, lo: i32, hi: i32) -> f64 {
        let span = (hi - lo + 1) as u64;
        let e = lo + (self.next() % span) as i32;
        let mant = self.next() & ((1u64 << 52) - 1);
        reference::ldexp(f64::from_bits(0x3FF0_0000_0000_0000 | mant), e)
    }
    fn sgn(&mut self) -> f64 {
        if self.next() & 1 == 1 { -1.0 } else { 1.0 }
    }
}

fn scale() -> usize {
    std::env::var("FORGE_ACCURACY_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
}

#[derive(Default)]
struct Stats {
    n: usize,
    max: f64,
    worst_input: (f64, f64),
    sum: f64,
    correctly_rounded: usize,
    std_bit_equal: usize,
}

impl Stats {
    fn add(&mut self, err: f64, input: (f64, f64), std_equal: bool) {
        // A NaN error (e.g. inf - inf) must fail, not vanish from the max: NaN > x is false.
        let a = if err.is_nan() {
            f64::INFINITY
        } else {
            err.abs()
        };
        self.n += 1;
        self.sum += a;
        if a <= 0.5 {
            self.correctly_rounded += 1;
        }
        if std_equal {
            self.std_bit_equal += 1;
        }
        if a > self.max || self.n == 1 {
            self.max = a;
            self.worst_input = input;
        }
    }
}

struct Row {
    func: &'static str,
    domain: &'static str,
    stats: Stats,
}

fn row(
    func: &'static str,
    domain: &'static str,
    n: usize,
    mut sample: impl FnMut() -> (f64, f64),
    eval: impl Fn(f64, f64) -> (f64, f64, f64),
) -> Row {
    let mut stats = Stats::default();
    for _ in 0..n {
        let (a, b) = sample();
        let (got, err, std_val) = eval(a, b);
        stats.add(err, (a, b), got.to_bits() == std_val.to_bits());
    }
    Row {
        func,
        domain,
        stats,
    }
}

#[allow(clippy::disallowed_methods)] // std is the comparison column, never the answer
fn std_sin(x: f64) -> f64 {
    x.sin()
}
#[allow(clippy::disallowed_methods)]
fn std_cos(x: f64) -> f64 {
    x.cos()
}
#[allow(clippy::disallowed_methods)]
fn std_exp(x: f64) -> f64 {
    x.exp()
}
#[allow(clippy::disallowed_methods)]
fn std_ln(x: f64) -> f64 {
    x.ln()
}
#[allow(clippy::disallowed_methods)]
fn std_pow(x: f64, y: f64) -> f64 {
    x.powf(y)
}
#[allow(clippy::disallowed_methods)]
fn std_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

fn all_rows(k: usize) -> Vec<Row> {
    let mut rng = Rng(0x05EE_D0FF_046E);
    let mut rows = Vec::new();
    let n = 4_000 * k;
    let sin_eval = |x: f64, _| {
        let g = det::sin(x);
        (g, ulp_error(g, reference::sin_cos(x).0), std_sin(x))
    };
    let cos_eval = |x: f64, _| {
        let g = det::cos(x);
        (g, ulp_error(g, reference::sin_cos(x).1), std_cos(x))
    };
    let pio4 = std::f64::consts::FRAC_PI_4;
    for (func, eval) in [
        ("sin", &sin_eval as &dyn Fn(f64, f64) -> (f64, f64, f64)),
        ("cos", &cos_eval),
    ] {
        let r = &mut rng;
        rows.push(row(
            func,
            "[-pi/4, pi/4]",
            n,
            || (r.uniform(-pio4, pio4), 0.0),
            eval,
        ));
        let r = &mut rng;
        rows.push(row(
            func,
            "|x| in [2^-30, 2^20] log",
            n,
            || ((r.log_uniform(-30, 20) * r.sgn()), 0.0),
            eval,
        ));
        let r = &mut rng;
        rows.push(row(
            func,
            "|x| in [2^20, 2^1023] log (Payne-Hanek)",
            n / 2,
            || ((r.log_uniform(20, 1023) * r.sgn()), 0.0),
            eval,
        ));
        let r = &mut rng;
        rows.push(row(
            func,
            "x ~ n pi/2, n < 2^20 (cancellation)",
            n / 2,
            || {
                let m = (r.next() % (1 << 20)) as f64;
                (m * std::f64::consts::FRAC_PI_2, 0.0)
            },
            eval,
        ));
    }
    let exp_eval = |x: f64, _| {
        let g = det::exp(x);
        let (m, k) = reference::exp_parts(x);
        (g, reference::ulp_error_parts(g, m, k), std_exp(x))
    };
    let r = &mut rng;
    rows.push(row(
        "exp",
        "[-708, 709.7]",
        n,
        || (r.uniform(-708.0, 709.7), 0.0),
        exp_eval,
    ));
    let r = &mut rng;
    rows.push(row(
        "exp",
        "|x| in [2^-50, 1] log",
        n,
        || ((r.log_uniform(-50, 0) * r.sgn()), 0.0),
        exp_eval,
    ));
    let r = &mut rng;
    rows.push(row(
        "exp",
        "[-745, -708] (subnormal results)",
        n / 4,
        || (r.uniform(-745.0, -708.0), 0.0),
        exp_eval,
    ));
    let ln_eval = |x: f64, _| {
        let g = det::ln(x);
        (g, ulp_error(g, reference::ln(x)), std_ln(x))
    };
    let r = &mut rng;
    rows.push(row(
        "ln",
        "x in [2^-1074, 2^1023] log (all positive)",
        n,
        || {
            let x = r.log_uniform(-1074, 1023);
            (if x == 0.0 { 1e-300 } else { x }, 0.0)
        },
        ln_eval,
    ));
    let r = &mut rng;
    rows.push(row(
        "ln",
        "x = 1 +- 2^-k, k in [1, 52] (cancellation)",
        n,
        || {
            let d = r.log_uniform(-52, -1);
            (1.0 + (d * r.sgn()), 0.0)
        },
        ln_eval,
    ));
    let pow_eval = |x: f64, y: f64| {
        let g = det::pow(x, y);
        (g, ulp_error(g, reference::pow(x, y)), std_pow(x, y))
    };
    let r = &mut rng;
    rows.push(row(
        "pow",
        "x in [2^-20, 2^20] log, y in [-30, 30]",
        n,
        || (r.log_uniform(-20, 19), r.uniform(-30.0, 30.0)),
        pow_eval,
    ));
    let r = &mut rng;
    rows.push(row(
        "pow",
        "x = 1 +- 2^-k, |y ln x| up to ~650 (large y)",
        n,
        || {
            let d = r.log_uniform(-40, -4);
            let x = 1.0 + (d * r.sgn());
            let y = r.uniform(0.0, 650.0) / d * r.sgn();
            (x, y)
        },
        pow_eval,
    ));
    let r = &mut rng;
    rows.push(row(
        "pow",
        "x in [0.5, 2], y integer in [-1000, 1000]",
        n / 2,
        || {
            let y = ((r.next() % 2001) as i64 - 1000) as f64;
            let x = r.uniform(0.5, 2.0);
            // keep the result finite and normal
            let lim = 700.0 / y.abs().max(1.0);
            let x = if y == 0.0 {
                x
            } else {
                x.clamp((-lim).max(-0.69).exp_approx(), lim.min(0.69).exp_approx())
            };
            (x, y)
        },
        pow_eval,
    ));
    let atan_eval = |y: f64, x: f64| {
        let g = det::atan2(y, x);
        let s = std_atan2(y, x);
        (g, ulp_error(g, reference::atan2(y, x, s)), s)
    };
    let r = &mut rng;
    rows.push(row(
        "atan2",
        "|y|, |x| in [2^-60, 2^60] log, all quadrants",
        n,
        || {
            (
                (r.log_uniform(-60, 60) * r.sgn()),
                (r.log_uniform(-60, 60) * r.sgn()),
            )
        },
        atan_eval,
    ));
    let r = &mut rng;
    rows.push(row(
        "atan2",
        "|y| ~ |x| (t near 1, j = 8)",
        n / 2,
        || {
            let x = r.log_uniform(-10, 10);
            ((x * r.uniform(0.9, 1.1) * r.sgn()), (x * r.sgn()))
        },
        atan_eval,
    ));
    let r = &mut rng;
    rows.push(row(
        "atan2",
        "y/x in [2^-40, 2^-3] (small angles, j = 0)",
        n / 2,
        || {
            let x = r.log_uniform(-5, 5);
            (x * r.log_uniform(-40, -4), x)
        },
        atan_eval,
    ));
    rows
}

/// A crude, test-only e^x for picking sample bounds (not measured).
trait ExpApprox {
    fn exp_approx(self) -> f64;
}
impl ExpApprox for f64 {
    fn exp_approx(self) -> f64 {
        reference::exp(R::f(self)).hi
    }
}

#[test]
fn every_function_is_faithfully_rounded() {
    let rows = all_rows(scale());
    println!(
        "\n| function | domain | samples | max ulp | mean ulp | correctly rounded | bit-equal to this platform's std |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|");
    let mut failures = Vec::new();
    for r in &rows {
        let s = &r.stats;
        println!(
            "| {} | {} | {} | {:.3} | {:.4} | {:.2}% | {:.2}% |",
            r.func,
            r.domain,
            s.n,
            s.max,
            s.sum / s.n as f64,
            100.0 * s.correctly_rounded as f64 / s.n as f64,
            100.0 * s.std_bit_equal as f64 / s.n as f64,
        );
        if s.max >= MAX_ULP {
            failures.push(format!(
                "{} on {}: {:.3} ulp at input {:?}",
                r.func, r.domain, s.max, s.worst_input
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "not faithfully rounded:\n{}",
        failures.join("\n")
    );
}

/// The reference must agree with itself where identities are exact, or the table above
/// measures the reference's error instead of det's.
#[test]
fn reference_is_self_consistent() {
    // ln(2^k) = k ln 2; exp(ln x) = x; sin^2 + cos^2 = 1; atan2(1, 1) = pi/4.
    for k in [-1074, -1000, -1, 1, 10, 1023] {
        let l = reference::ln(reference::ldexp(1.0, k));
        let want = reference::ln(2.0).mulf(k as f64);
        assert!(l.sub(want).hi.abs() <= 1e-28 * want.hi.abs(), "ln 2^{k}");
    }
    for x in [0.3, 1.7, 55.5, 1e-7, 3e5] {
        let back = reference::exp(reference::ln(x));
        assert!((back.sub(R::f(x)).hi / x).abs() < 1e-28, "exp(ln {x})");
    }
    for x in [0.5, 2.0, 1e6, 1e22, 1e300] {
        let (s, c) = reference::sin_cos(x);
        let one = s.mul(s).add(c.mul(c)).sub(R::f(1.0));
        assert!(one.hi.abs() < 1e-28, "sin^2+cos^2 at {x}");
    }
    let q = reference::atan2(1.0, 1.0, 0.78).sub(reference::pi().divf(4.0));
    assert!(q.hi.abs() < 1e-30);
}

/// W2: the harness must be able to fail. A function 2 ulp off must be reported as such.
#[test]
fn positive_control_a_two_ulp_error_is_detected() {
    let mut worst = 0.0f64;
    for i in 1..2000 {
        let x = i as f64 * 1e-3;
        let bad = f64::from_bits(det::sin(x).to_bits() + 2);
        worst = worst.max(ulp_error(bad, reference::sin_cos(x).0).abs());
    }
    assert!(
        worst >= MAX_ULP,
        "a 2-ulp error measured as {worst} ulp: the harness is blind"
    );
}

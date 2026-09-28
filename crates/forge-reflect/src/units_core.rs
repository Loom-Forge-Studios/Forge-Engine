//! The unit grammar (Ch.6 "unit annotations are not cosmetic"), shared **verbatim** by
//! `forge-reflect` (run time: pin connection checks) and `forge-reflect-macros` (compile
//! time: an unknown unit in `#[forge(units = "...")]` is a build error). One file, compiled
//! twice through `#[path]`, so the two can never disagree about what a unit means.
//!
//! Grammar: `factor ((· | * | space) factor)* ( / factor (...)* )?` — at most one `/`, and
//! every factor after it is in the denominator (`W/m²·K` is W·m⁻²·K⁻¹; parentheses around the
//! denominator are allowed and mean the same). A factor is a symbol with an optional
//! exponent: `^2`, `^-1`, or superscripts `²`, `⁻¹`. `1` is dimensionless.
//!
//! Symbols: SI base (`m g s A K mol cd`), angle (`rad`, `deg`, `sr` — angle is its own
//! dimension so an angle pin never connects to a unitless one by accident), derived (`N J W
//! Pa Hz C V L eV bar`), time (`min h d`), long distances (`au ly pc`), `t` (tonne) and `%`.
//! SI prefixes `n u µ m c k M G T` apply to the prefixable symbols only (`mm`, `km`, `ms`,
//! `kN`, `MJ`), never to `min h d au ly pc deg % t`.

/// Dimension exponents over `[m, kg, s, A, K, mol, cd, rad]`.
pub type Dims = [i8; 8];

/// Names of the base dimensions, in [`Dims`] order.
pub const BASE_SYMBOLS: [&str; 8] = ["m", "kg", "s", "A", "K", "mol", "cd", "rad"];

/// A parsed unit: its dimension and its scale relative to the SI coherent unit of that
/// dimension (`km` is `[1,0,..]` at 1000).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParsedUnit {
    /// Exponent per base dimension.
    pub dims: Dims,
    /// Multiply a value in this unit by `scale` to get SI.
    pub scale: f64,
}

/// `(symbol, dims over [m, kg, s, A, K, mol, cd, rad], scale, prefixable)`.
const SYMBOLS: &[(&str, Dims, f64, bool)] = &[
    ("m", [1, 0, 0, 0, 0, 0, 0, 0], 1.0, true),
    ("g", [0, 1, 0, 0, 0, 0, 0, 0], 1e-3, true),
    ("s", [0, 0, 1, 0, 0, 0, 0, 0], 1.0, true),
    ("A", [0, 0, 0, 1, 0, 0, 0, 0], 1.0, true),
    ("K", [0, 0, 0, 0, 1, 0, 0, 0], 1.0, true),
    ("mol", [0, 0, 0, 0, 0, 1, 0, 0], 1.0, true),
    ("cd", [0, 0, 0, 0, 0, 0, 1, 0], 1.0, true),
    ("rad", [0, 0, 0, 0, 0, 0, 0, 1], 1.0, true),
    ("sr", [0, 0, 0, 0, 0, 0, 0, 2], 1.0, false),
    // pi / 180, written out so this file needs no float math beyond one multiply.
    (
        "deg",
        [0, 0, 0, 0, 0, 0, 0, 1],
        0.017_453_292_519_943_295,
        false,
    ),
    ("N", [1, 1, -2, 0, 0, 0, 0, 0], 1.0, true),
    ("J", [2, 1, -2, 0, 0, 0, 0, 0], 1.0, true),
    ("W", [2, 1, -3, 0, 0, 0, 0, 0], 1.0, true),
    ("Pa", [-1, 1, -2, 0, 0, 0, 0, 0], 1.0, true),
    ("Hz", [0, 0, -1, 0, 0, 0, 0, 0], 1.0, true),
    ("C", [0, 0, 1, 1, 0, 0, 0, 0], 1.0, true),
    ("V", [2, 1, -3, -1, 0, 0, 0, 0], 1.0, true),
    ("L", [3, 0, 0, 0, 0, 0, 0, 0], 1e-3, true),
    ("eV", [2, 1, -2, 0, 0, 0, 0, 0], 1.602_176_634e-19, true),
    ("bar", [-1, 1, -2, 0, 0, 0, 0, 0], 1e5, false),
    ("min", [0, 0, 1, 0, 0, 0, 0, 0], 60.0, false),
    ("h", [0, 0, 1, 0, 0, 0, 0, 0], 3600.0, false),
    ("d", [0, 0, 1, 0, 0, 0, 0, 0], 86_400.0, false),
    ("au", [1, 0, 0, 0, 0, 0, 0, 0], 1.495_978_707e11, false),
    (
        "ly",
        [1, 0, 0, 0, 0, 0, 0, 0],
        9.460_730_472_580_8e15,
        false,
    ),
    (
        "pc",
        [1, 0, 0, 0, 0, 0, 0, 0],
        3.085_677_581_491_367e16,
        false,
    ),
    ("t", [0, 1, 0, 0, 0, 0, 0, 0], 1e3, false),
    ("%", [0, 0, 0, 0, 0, 0, 0, 0], 1e-2, false),
    ("1", [0, 0, 0, 0, 0, 0, 0, 0], 1.0, false),
];

const PREFIXES: &[(&str, f64)] = &[
    ("n", 1e-9),
    ("u", 1e-6),
    ("µ", 1e-6),
    ("μ", 1e-6),
    ("m", 1e-3),
    ("c", 1e-2),
    ("k", 1e3),
    ("M", 1e6),
    ("G", 1e9),
    ("T", 1e12),
];

fn symbol(sym: &str) -> Option<(Dims, f64)> {
    if let Some(&(_, dims, scale, _)) = SYMBOLS.iter().find(|(s, ..)| *s == sym) {
        return Some((dims, scale));
    }
    for &(p, factor) in PREFIXES {
        if let Some(rest) = sym.strip_prefix(p)
            && let Some(&(_, dims, scale, true)) = SYMBOLS.iter().find(|(s, ..)| *s == rest)
        {
            return Some((dims, scale * factor));
        }
    }
    None
}

fn superscript_digit(c: char) -> Option<i32> {
    Some(match c {
        '⁰' => 0,
        '¹' => 1,
        '²' => 2,
        '³' => 3,
        '⁴' => 4,
        '⁵' => 5,
        '⁶' => 6,
        '⁷' => 7,
        '⁸' => 8,
        '⁹' => 9,
        _ => return None,
    })
}

/// Split a factor into its symbol and exponent.
fn split_exponent(f: &str) -> Result<(&str, i32), String> {
    if let Some((sym, exp)) = f.split_once('^') {
        let n: i32 = exp
            .parse()
            .map_err(|_| format!("bad exponent `{exp}` in `{f}`"))?;
        return Ok((sym, n));
    }
    let cut = f
        .char_indices()
        .find(|&(_, c)| c == '⁻' || superscript_digit(c).is_some())
        .map_or(f.len(), |(i, _)| i);
    let (sym, sup) = f.split_at(cut);
    if sup.is_empty() {
        return Ok((sym, 1));
    }
    let (neg, digits) = match sup.strip_prefix('⁻') {
        Some(rest) => (true, rest),
        None => (false, sup),
    };
    if digits.is_empty() {
        return Err(format!("superscript minus with no digits in `{f}`"));
    }
    let mut n = 0i32;
    for c in digits.chars() {
        let v = superscript_digit(c).ok_or_else(|| format!("bad exponent in `{f}`"))?;
        n = n * 10 + v;
    }
    Ok((sym, if neg { -n } else { n }))
}

fn accumulate(group: &str, sign: i32, out: &mut ParsedUnit) -> Result<(), String> {
    let mut any = false;
    for f in group.split(['·', '*', '⋅', ' ']).filter(|f| !f.is_empty()) {
        any = true;
        let (sym, exp) = split_exponent(f)?;
        if sym.is_empty() {
            return Err(format!("exponent with no unit in `{f}`"));
        }
        let (dims, scale) = symbol(sym).ok_or_else(|| format!("unknown unit symbol `{sym}`"))?;
        let exp = exp * sign;
        for (o, d) in out.dims.iter_mut().zip(dims) {
            let v = i32::from(*o) + i32::from(d) * exp;
            *o = i8::try_from(v).map_err(|_| format!("exponent overflow in `{f}`"))?;
        }
        out.scale *= scale.powi(exp);
    }
    if any {
        Ok(())
    } else {
        Err("empty unit group".to_string())
    }
}

/// Parse a unit string. `Err` carries a human-readable reason (it becomes a compile error in
/// the macro and `REFLECT-0001` at run time).
pub fn parse_unit(text: &str) -> Result<ParsedUnit, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty unit (write `1` for dimensionless)".to_string());
    }
    let mut out = ParsedUnit {
        dims: [0; 8],
        scale: 1.0,
    };
    let (num, den) = match text.split_once('/') {
        Some((n, d)) => (n, Some(d)),
        None => (text, None),
    };
    accumulate(num, 1, &mut out)?;
    if let Some(den) = den {
        if den.contains('/') {
            return Err(format!("more than one `/` in `{text}`"));
        }
        let den = den.trim();
        let den = den
            .strip_prefix('(')
            .and_then(|d| d.strip_suffix(')'))
            .unwrap_or(den);
        accumulate(den, -1, &mut out)?;
    }
    Ok(out)
}

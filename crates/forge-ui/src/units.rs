//! Physical units for numeric fields (Ch.21 §21.16 "numeric drag field with units",
//! Ch.6 unit annotations).
//!
//! A field is annotated with a unit (`N·s`, `m/s²`, `kg`); the user may type a value in
//! any **compatible** unit (`12 kN·s`, `3.5 km/h` into an `m/s` field) and it is converted
//! on entry. An incompatible unit is refused **with a reason** that names both
//! dimensions, never silently reinterpreted.
//!
//! Units are linear (value × scale). Affine scales (°C, °F) are deliberately not offered:
//! a temperature *difference* and a temperature *reading* convert differently, and a
//! field that guessed would be wrong half the time; engine temperatures are kelvin.

use std::fmt;

/// SI base-dimension exponents: m, kg, s, A, K, mol, cd.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Dim(pub [i8; 7]);

impl Dim {
    pub const NONE: Dim = Dim([0; 7]);
    fn mul(self, o: Dim, sign: i8) -> Dim {
        let mut d = self.0;
        for (a, b) in d.iter_mut().zip(o.0) {
            *a += b * sign;
        }
        Dim(d)
    }
    fn pow(self, p: i8) -> Dim {
        Dim(self.0.map(|e| e * p))
    }
}

impl fmt::Display for Dim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 7] = ["m", "kg", "s", "A", "K", "mol", "cd"];
        let mut parts = Vec::new();
        for (n, e) in NAMES.iter().zip(self.0) {
            match e {
                0 => {}
                1 => parts.push((*n).to_string()),
                e => parts.push(format!("{n}^{e}")),
            }
        }
        if parts.is_empty() {
            write!(f, "dimensionless")
        } else {
            write!(f, "{}", parts.join("·"))
        }
    }
}

/// A parsed unit expression: its dimension and its scale to SI.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    /// What the user wrote (normalised spacing), shown back in the field.
    pub symbol: String,
    pub dim: Dim,
    /// Multiply a value in this unit by `scale` to get SI.
    pub scale: f64,
}

/// Why a quantity was refused.
#[derive(Clone, Debug, PartialEq)]
pub enum UnitError {
    /// The text is not a number (optionally followed by a unit).
    NotANumber(String),
    /// A unit symbol is unknown.
    UnknownUnit(String),
    /// The unit's dimension does not match the field's.
    Incompatible {
        entered: String,
        entered_dim: Dim,
        field: String,
        field_dim: Dim,
    },
}

impl fmt::Display for UnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnitError::NotANumber(s) => write!(f, "\"{s}\" is not a number"),
            UnitError::UnknownUnit(u) => write!(f, "unknown unit \"{u}\""),
            UnitError::Incompatible {
                entered,
                entered_dim,
                field,
                field_dim,
            } => write!(
                f,
                "{entered} is {entered_dim}, but this field is {field} ({field_dim})"
            ),
        }
    }
}

/// Named units: symbol, dimension, scale to SI.
const UNITS: &[(&str, [i8; 7], f64)] = &[
    ("m", [1, 0, 0, 0, 0, 0, 0], 1.0),
    ("g", [0, 1, 0, 0, 0, 0, 0], 1e-3),
    ("s", [0, 0, 1, 0, 0, 0, 0], 1.0),
    ("A", [0, 0, 0, 1, 0, 0, 0], 1.0),
    ("K", [0, 0, 0, 0, 1, 0, 0], 1.0),
    ("mol", [0, 0, 0, 0, 0, 1, 0], 1.0),
    ("cd", [0, 0, 0, 0, 0, 0, 1], 1.0),
    ("N", [1, 1, -2, 0, 0, 0, 0], 1.0),
    ("J", [2, 1, -2, 0, 0, 0, 0], 1.0),
    ("W", [2, 1, -3, 0, 0, 0, 0], 1.0),
    ("Pa", [-1, 1, -2, 0, 0, 0, 0], 1.0),
    ("Hz", [0, 0, -1, 0, 0, 0, 0], 1.0),
    ("V", [2, 1, -3, -1, 0, 0, 0], 1.0),
    ("C", [0, 0, 1, 1, 0, 0, 0], 1.0),
    ("rad", [0; 7], 1.0),
    ("deg", [0; 7], std::f64::consts::PI / 180.0),
    ("°", [0; 7], std::f64::consts::PI / 180.0),
    ("%", [0; 7], 0.01),
    ("min", [0, 0, 1, 0, 0, 0, 0], 60.0),
    ("h", [0, 0, 1, 0, 0, 0, 0], 3600.0),
    ("L", [3, 0, 0, 0, 0, 0, 0], 1e-3),
    ("au", [1, 0, 0, 0, 0, 0, 0], 1.495_978_707e11),
    ("ly", [1, 0, 0, 0, 0, 0, 0], 9.460_730_472_580_8e15),
    ("pc", [1, 0, 0, 0, 0, 0, 0], 3.085_677_581_491_367e16),
];

const PREFIXES: &[(&str, f64)] = &[
    ("G", 1e9),
    ("M", 1e6),
    ("k", 1e3),
    ("c", 1e-2),
    ("m", 1e-3),
    ("µ", 1e-6),
    ("u", 1e-6),
    ("n", 1e-9),
];

/// One symbol with an optional SI prefix (`km`, `kN`, `mg`, `µs`).
fn symbol(s: &str) -> Option<(Dim, f64)> {
    if let Some((_, d, k)) = UNITS.iter().find(|(n, _, _)| *n == s) {
        return Some((Dim(*d), *k));
    }
    for (p, pk) in PREFIXES {
        if let Some(rest) = s.strip_prefix(p)
            && let Some((_, d, k)) = UNITS.iter().find(|(n, _, _)| *n == rest)
        {
            return Some((Dim(*d), k * pk));
        }
    }
    None
}

fn superscript(c: char) -> Option<i8> {
    match c {
        '⁰' => Some(0),
        '¹' => Some(1),
        '²' => Some(2),
        '³' => Some(3),
        '⁴' => Some(4),
        '⁻' => Some(-1), // sign marker, handled by the caller
        _ => None,
    }
}

/// Parse a unit expression: factors joined by `·`, `*` or spaces, one optional `/`
/// (everything after it is in the denominator), powers as `^2`, `^-1` or `²`.
pub fn parse_unit(expr: &str) -> Result<Unit, UnitError> {
    let t = expr.trim();
    if t.is_empty() {
        return Ok(Unit {
            symbol: String::new(),
            dim: Dim::NONE,
            scale: 1.0,
        });
    }
    let mut dim = Dim::NONE;
    let mut scale = 1.0f64;
    let mut sign = 1i8;
    let mut token = String::new();
    let apply = |tok: &str, sign: i8, dim: &mut Dim, scale: &mut f64| -> Result<(), UnitError> {
        if tok.is_empty() {
            return Ok(());
        }
        // Split a trailing power.
        let (base, pow) = if let Some((b, p)) = tok.split_once('^') {
            let p: i8 = p
                .parse()
                .map_err(|_| UnitError::UnknownUnit(tok.to_string()))?;
            (b.to_string(), p)
        } else {
            let mut base = tok.to_string();
            let mut digits = Vec::new();
            let mut neg = false;
            while let Some(c) = base.chars().last() {
                match superscript(c) {
                    Some(-1) => {
                        neg = true;
                        base.pop();
                    }
                    Some(d) => {
                        digits.push(d);
                        base.pop();
                    }
                    None => break,
                }
            }
            let mut p = 0i8;
            for d in digits.iter().rev() {
                p = p * 10 + d;
            }
            if digits.is_empty() {
                p = 1;
            }
            (base, if neg { -p } else { p })
        };
        let (d, k) = symbol(&base).ok_or_else(|| UnitError::UnknownUnit(base.clone()))?;
        *dim = dim.mul(d.pow(pow), sign);
        *scale *= k.powi(i32::from(pow) * i32::from(sign));
        Ok(())
    };
    for c in t.chars() {
        match c {
            '·' | '*' | ' ' | '⋅' => {
                apply(&token, sign, &mut dim, &mut scale)?;
                token.clear();
            }
            '/' => {
                apply(&token, sign, &mut dim, &mut scale)?;
                token.clear();
                sign = -1;
            }
            c => token.push(c),
        }
    }
    apply(&token, sign, &mut dim, &mut scale)?;
    Ok(Unit {
        symbol: t.split_whitespace().collect::<Vec<_>>().join(""),
        dim,
        scale,
    })
}

/// Split `"12.5 kN·s"` into the number and the unit text.
pub fn split_quantity(text: &str) -> Result<(f64, &str), UnitError> {
    let t = text.trim();
    // The longest numeric prefix (decimal comma accepted; `inf`/`NaN` are not numbers a
    // field accepts).
    for end in (1..=t.len()).rev() {
        if !t.is_char_boundary(end) {
            continue;
        }
        let head = &t[..end];
        if head
            .chars()
            .any(|c| c.is_alphabetic() && c != 'e' && c != 'E')
        {
            continue;
        }
        if let Ok(v) = head.trim().replace(',', ".").parse::<f64>()
            && v.is_finite()
        {
            return Ok((v, t[end..].trim()));
        }
    }
    Err(UnitError::NotANumber(text.to_string()))
}

/// Parse what the user typed into a field whose unit is `field` and return the value in
/// the field's unit. No unit typed: the value is already in the field's unit.
pub fn parse_in(text: &str, field: &Unit) -> Result<f64, UnitError> {
    let (v, unit_text) = split_quantity(text)?;
    if unit_text.is_empty() {
        return Ok(v);
    }
    let entered = parse_unit(unit_text)?;
    if entered.dim != field.dim {
        return Err(UnitError::Incompatible {
            entered: entered.symbol,
            entered_dim: entered.dim,
            field: field.symbol.clone(),
            field_dim: field.dim,
        });
    }
    Ok(v * entered.scale / field.scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Unit {
        parse_unit(s).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn dimensions_compose() {
        assert_eq!(u("N·s").dim, u("kg·m/s").dim);
        assert_eq!(u("m/s²").dim, u("m/s^2").dim);
        assert_eq!(u("J").dim, u("N m").dim);
        assert_eq!(u("Hz").dim, u("s⁻¹").dim);
    }

    #[test]
    fn converts_on_entry() {
        let field = u("N·s");
        let v = parse_in("12 kN·s", &field).unwrap_or(f64::NAN);
        assert!((v - 12_000.0).abs() < 1e-9);
        let kmh = parse_in("36 km/h", &u("m/s")).unwrap_or(f64::NAN);
        assert!((kmh - 10.0).abs() < 1e-9);
        assert_eq!(parse_in("2.5", &field), Ok(2.5));
        let deg = parse_in("180 deg", &u("rad")).unwrap_or(f64::NAN);
        assert!((deg - std::f64::consts::PI).abs() < 1e-12);
        assert_eq!(parse_in("1e3 g", &u("kg")), Ok(1.0));
    }

    #[test]
    fn refuses_incompatible_units_with_a_reason() {
        let e = parse_in("5 m", &u("N·s")).expect_err("metres into impulse");
        let msg = e.to_string();
        assert!(
            msg.contains("m") && msg.contains("N·s") && msg.contains("kg"),
            "{msg}"
        );
        assert!(matches!(
            parse_in("5 furlongs", &u("m")),
            Err(UnitError::UnknownUnit(_))
        ));
        assert!(matches!(
            parse_in("abc", &u("m")),
            Err(UnitError::NotANumber(_))
        ));
    }
}

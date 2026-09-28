//! Units (Ch.6): parsed, dimension-checked, and compared when two pins are connected.

use std::fmt;

#[path = "units_core.rs"]
mod grammar;

pub use self::grammar::{BASE_SYMBOLS, Dims};

use crate::ReflectError;

/// A physical unit such as `N·s`, `m/s` or `km`: the text as written (what the UI shows and
/// the schema carries) plus its dimension and SI scale (what compatibility is decided on).
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    text: &'static str,
    dims: Dims,
    scale: f64,
}

impl Unit {
    /// Parse a unit string (grammar in `units_core.rs`). `REFLECT-0001` if it does not parse.
    pub fn parse(text: &'static str) -> Result<Self, ReflectError> {
        let p = self::grammar::parse_unit(text).map_err(|why| ReflectError::InvalidUnit {
            units: text.to_string(),
            why,
        })?;
        Ok(Self {
            text,
            dims: p.dims,
            scale: p.scale,
        })
    }

    /// The unit as written in the annotation.
    #[must_use]
    pub fn text(&self) -> &'static str {
        self.text
    }

    /// Exponents over [`BASE_SYMBOLS`].
    #[must_use]
    pub fn dims(&self) -> Dims {
        self.dims
    }

    /// Multiply a value in this unit by `scale()` to get the coherent SI value.
    #[must_use]
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Same physical dimension (`km` and `m`, `N·s` and `kg·m/s`)?
    #[must_use]
    pub fn same_dimension(&self, other: &Unit) -> bool {
        self.dims == other.dims
    }

    /// The factor that converts a value in `self` into `to` (`km` → `m` is 1000), or
    /// `REFLECT-0008` if the dimensions differ (`N·s` → `m/s`).
    pub fn conversion_to(&self, to: &Unit) -> Result<f64, ReflectError> {
        if self.same_dimension(to) {
            Ok(self.scale / to.scale)
        } else {
            Err(ReflectError::IncompatibleUnits {
                from: self.text.to_string(),
                to: to.text.to_string(),
            })
        }
    }

    /// The dimension in SI base symbols, e.g. `kg·m·s^-1` — for messages and the schema.
    #[must_use]
    pub fn si_dimension(&self) -> String {
        let parts: Vec<String> = BASE_SYMBOLS
            .iter()
            .zip(self.dims)
            .filter(|&(_, e)| e != 0)
            .map(|(s, e)| {
                if e == 1 {
                    (*s).to_string()
                } else {
                    format!("{s}^{e}")
                }
            })
            .collect();
        if parts.is_empty() {
            "1".to_string()
        } else {
            parts.join("·")
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &'static str) -> Unit {
        Unit::parse(s).expect(s)
    }

    #[test]
    fn impulse_is_momentum_and_not_velocity() {
        assert!(u("N·s").same_dimension(&u("kg·m/s")));
        assert!(u("N*s").same_dimension(&u("kg m s^-1")));
        assert!(!u("N·s").same_dimension(&u("m/s")));
        assert_eq!(u("N·s").si_dimension(), "m·kg·s^-1");
    }

    #[test]
    fn prefixes_and_scales() {
        assert_eq!(u("km").conversion_to(&u("m")).expect("km->m"), 1000.0);
        assert_eq!(u("ms").conversion_to(&u("s")).expect("ms->s"), 1e-3);
        assert_eq!(u("kg").scale(), 1.0);
        assert_eq!(u("min").conversion_to(&u("s")).expect("min->s"), 60.0);
        assert!(
            (u("deg").conversion_to(&u("rad")).expect("deg") - 0.017_453_292_519_943_295).abs()
                < 1e-18
        );
        assert!(u("m/s²").same_dimension(&u("m·s^-2")));
        assert!(u("W/(m²·K)").same_dimension(&u("W/m^2·K")));
        assert!(u("W/(m²·K)").same_dimension(&u("kg·s⁻³·K⁻¹")));
        assert!(u("1").same_dimension(&u("%")));
        assert!(u("Hz").same_dimension(&u("1/s")));
    }

    #[test]
    fn angle_is_a_dimension() {
        assert!(!u("rad").same_dimension(&u("1")));
        assert!(u("rad/s").same_dimension(&u("deg/min")));
    }

    #[test]
    fn bad_units_are_rejected_with_a_code() {
        for bad in ["", "furlong", "m//s", "m/s/s", "kmin", "m^x", "²", "m⁻"] {
            let e = Unit::parse(bad).expect_err(bad);
            assert_eq!(
                crate::CodedError::error_code(&e).as_str(),
                "REFLECT-0001",
                "{bad}"
            );
        }
    }

    #[test]
    fn incompatible_conversion_is_refused() {
        let e = u("N·s").conversion_to(&u("m/s")).expect_err("must refuse");
        assert_eq!(crate::CodedError::error_code(&e).as_str(), "REFLECT-0008");
    }
}

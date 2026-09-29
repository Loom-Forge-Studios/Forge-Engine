//! Modifiers (Ch.28 §28.11): what shapes a control's value before it drives an action —
//! deadzones, response curves, scale, negation and swizzle. A modifier sits on a binding
//! (`Gamepad/LeftStick | Deadzone(Radial, 0.15, 0.95) | Curve(Power, 2)`) or on the action
//! (applied after the bindings are combined).
//!
//! Every modifier is a pure function of the value: no state, no allocation, so a binding's
//! chain costs a few multiplies per frame.
//!
//! **Deadzones.** `Radial` (the default for a stick) measures the stick's deflection as a
//! magnitude, so a diagonal is not cut earlier than an axis, and rescales `lower..upper` to
//! `0..1` keeping the direction: the smallest movement past the deadzone is a small value,
//! not a jump. `Axial` does the same per axis (a d-pad-like feel, or a single trigger).
//! `UnscaledRadial` cuts without rescaling (a legacy feel some ports need).
//!
//! **Curves** reshape the magnitude (a 2D value keeps its direction): `Linear`,
//! `Power(e)` (e > 1: fine aim near centre, full speed at the rim), `Smooth` (smoothstep) and
//! `Points(x y, ...)` (a piecewise-linear curve through authored points: any shape a designer
//! draws). Magnitudes past 1 are clamped to 1 first.

use std::fmt;

use crate::control::Shape;
use crate::faults::InputFaults;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadzoneKind {
    Radial,
    Axial,
    UnscaledRadial,
}

impl DeadzoneKind {
    fn name(self) -> &'static str {
        match self {
            DeadzoneKind::Radial => "Radial",
            DeadzoneKind::Axial => "Axial",
            DeadzoneKind::UnscaledRadial => "UnscaledRadial",
        }
    }
}

/// A response curve over the magnitude `0..=1`.
#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    Linear,
    Power(f32),
    Smooth,
    /// Sorted by `x`; evaluated piecewise-linearly, clamped at both ends.
    Points(Vec<(f32, f32)>),
}

impl Curve {
    /// The curve at `m` (`0..=1`).
    pub fn eval(&self, m: f32) -> f32 {
        let m = m.clamp(0.0, 1.0);
        match self {
            Curve::Linear => m,
            Curve::Power(e) => m.powf(*e),
            Curve::Smooth => m * m * (3.0 - 2.0 * m),
            Curve::Points(p) => {
                let Some(first) = p.first() else { return m };
                if m <= first.0 {
                    return first.1;
                }
                for w in p.windows(2) {
                    let ((x0, y0), (x1, y1)) = (w[0], w[1]);
                    if m <= x1 {
                        let t = if x1 > x0 { (m - x0) / (x1 - x0) } else { 1.0 };
                        return y0 + (y1 - y0) * t;
                    }
                }
                p.last().map_or(m, |l| l.1)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Modifier {
    Deadzone {
        kind: DeadzoneKind,
        lower: f32,
        upper: f32,
    },
    Curve(Curve),
    Scale(f32, f32),
    Negate {
        x: bool,
        y: bool,
    },
    /// Swap x and y (a 1D key driving the Y of a 2D action).
    Swizzle,
}

fn mag(v: [f32; 2]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

fn rescale(m: f32, lower: f32, upper: f32) -> f32 {
    if m <= lower {
        0.0
    } else if upper > lower {
        ((m.min(upper) - lower) / (upper - lower)).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Apply `f` to the magnitude of `v`, keeping its direction.
fn radial(v: [f32; 2], f: impl Fn(f32) -> f32) -> [f32; 2] {
    let m = mag(v);
    if m <= f32::EPSILON {
        return [0.0, 0.0];
    }
    let k = f(m) / m;
    [v[0] * k, v[1] * k]
}

impl Modifier {
    /// Apply to a value of `shape` (a 1D value is `[v, 0]`).
    pub fn apply(&self, v: [f32; 2], shape: Shape, faults: &InputFaults) -> [f32; 2] {
        match self {
            Modifier::Deadzone { kind, lower, upper } => {
                let (lo, up) = (*lower, *upper);
                let axial = |x: f32| {
                    let r = if faults.deadzone_no_rescale() {
                        if x.abs() <= lo { 0.0 } else { x.abs().min(1.0) }
                    } else {
                        rescale(x.abs(), lo, up)
                    };
                    r.copysign(x)
                };
                let kind = if faults.radial_as_axial() && *kind == DeadzoneKind::Radial {
                    DeadzoneKind::Axial
                } else {
                    *kind
                };
                match (kind, shape) {
                    (_, Shape::Button | Shape::Axis1D) | (DeadzoneKind::Axial, _) => {
                        [axial(v[0]), axial(v[1])]
                    }
                    (DeadzoneKind::Radial, Shape::Axis2D) => {
                        if faults.deadzone_no_rescale() {
                            if mag(v) <= lo { [0.0, 0.0] } else { v }
                        } else {
                            radial(v, |m| rescale(m, lo, up))
                        }
                    }
                    (DeadzoneKind::UnscaledRadial, Shape::Axis2D) => {
                        if mag(v) <= lo {
                            [0.0, 0.0]
                        } else {
                            v
                        }
                    }
                }
            }
            Modifier::Curve(c) => {
                if faults.curve_linear() {
                    return v;
                }
                match shape {
                    Shape::Axis2D => radial(v, |m| c.eval(m)),
                    _ => [c.eval(v[0].abs()).copysign(v[0]), v[1]],
                }
            }
            Modifier::Scale(x, y) => [v[0] * x, v[1] * y],
            Modifier::Negate { x, y } => {
                [if *x { -v[0] } else { v[0] }, if *y { -v[1] } else { v[1] }]
            }
            Modifier::Swizzle => [v[1], v[0]],
        }
    }

    /// Parse one modifier (`Deadzone(Radial, 0.15, 0.95)`, `Curve(Power, 2)`,
    /// `Curve(Points, 0 0, 0.5 0.2, 1 1)`, `Scale(2, 1)`, `Negate(y)`, `Swizzle`).
    pub fn parse(s: &str) -> Result<Modifier, String> {
        let s = s.trim();
        let (head, args) = match s.split_once('(') {
            Some((h, rest)) => (
                h.trim(),
                rest.strip_suffix(')')
                    .ok_or_else(|| format!("{s:?}: missing ')'"))?
                    .split(',')
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .collect::<Vec<_>>(),
            ),
            None => (s, Vec::new()),
        };
        let num = |a: &str| -> Result<f32, String> {
            a.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("{s:?}: {a:?} is not a number"))
        };
        match head {
            "Deadzone" => {
                let kind = match args.first().copied() {
                    Some("Radial") | None => DeadzoneKind::Radial,
                    Some("Axial") => DeadzoneKind::Axial,
                    Some("UnscaledRadial") => DeadzoneKind::UnscaledRadial,
                    Some(o) => return Err(format!("{s:?}: unknown deadzone kind {o:?}")),
                };
                let lower = args.get(1).map_or(Ok(0.15), |a| num(a))?;
                let upper = args.get(2).map_or(Ok(1.0), |a| num(a))?;
                if !(0.0..1.0).contains(&lower) || upper <= lower || upper > 1.0 {
                    return Err(format!("{s:?}: a deadzone needs 0 <= lower < upper <= 1"));
                }
                Ok(Modifier::Deadzone { kind, lower, upper })
            }
            "Curve" => match args.first().copied() {
                Some("Linear") | None => Ok(Modifier::Curve(Curve::Linear)),
                Some("Smooth") => Ok(Modifier::Curve(Curve::Smooth)),
                Some("Power") => {
                    let e = args.get(1).map_or(Ok(2.0), |a| num(a))?;
                    if e <= 0.0 {
                        return Err(format!("{s:?}: a power curve needs an exponent > 0"));
                    }
                    Ok(Modifier::Curve(Curve::Power(e)))
                }
                Some("Points") => {
                    let mut pts = Vec::new();
                    for a in &args[1..] {
                        let mut it = a.split_whitespace();
                        let (Some(x), Some(y), None) = (it.next(), it.next(), it.next()) else {
                            return Err(format!("{s:?}: a point is `x y`, got {a:?}"));
                        };
                        pts.push((num(x)?, num(y)?));
                    }
                    if pts.len() < 2 || pts.windows(2).any(|w| w[1].0 <= w[0].0) {
                        return Err(format!(
                            "{s:?}: a point curve needs two or more points with increasing x"
                        ));
                    }
                    Ok(Modifier::Curve(Curve::Points(pts)))
                }
                Some(o) => Err(format!("{s:?}: unknown curve {o:?}")),
            },
            "Scale" => {
                let x = args.first().map_or(Ok(1.0), |a| num(a))?;
                let y = args.get(1).map_or(Ok(x), |a| num(a))?;
                Ok(Modifier::Scale(x, y))
            }
            "Negate" => {
                let which = args.first().copied().unwrap_or("xy");
                let (x, y) = match which {
                    "x" => (true, false),
                    "y" => (false, true),
                    "xy" => (true, true),
                    o => return Err(format!("{s:?}: negate x, y or xy, not {o:?}")),
                };
                Ok(Modifier::Negate { x, y })
            }
            "Swizzle" => Ok(Modifier::Swizzle),
            o => Err(format!("unknown modifier {o:?}")),
        }
    }

    /// Parse a `|`-separated chain (empty text is no modifiers).
    pub fn parse_chain(s: &str) -> Result<Vec<Modifier>, String> {
        s.split('|')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(Modifier::parse)
            .collect()
    }

    /// A chain as text (the inverse of [`Modifier::parse_chain`]).
    pub fn chain_text(m: &[Modifier]) -> String {
        m.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

impl fmt::Display for Modifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Modifier::Deadzone { kind, lower, upper } => {
                write!(f, "Deadzone({}, {lower}, {upper})", kind.name())
            }
            Modifier::Curve(Curve::Linear) => write!(f, "Curve(Linear)"),
            Modifier::Curve(Curve::Smooth) => write!(f, "Curve(Smooth)"),
            Modifier::Curve(Curve::Power(e)) => write!(f, "Curve(Power, {e})"),
            Modifier::Curve(Curve::Points(p)) => {
                write!(f, "Curve(Points")?;
                for (x, y) in p {
                    write!(f, ", {x} {y}")?;
                }
                write!(f, ")")
            }
            Modifier::Scale(x, y) => write!(f, "Scale({x}, {y})"),
            Modifier::Negate { x, y } => {
                let w = match (x, y) {
                    (true, false) => "x",
                    (false, true) => "y",
                    _ => "xy",
                };
                write!(f, "Negate({w})")
            }
            Modifier::Swizzle => write!(f, "Swizzle"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chains_round_trip() {
        let s = "Deadzone(Radial, 0.15, 0.95) | Curve(Power, 2) | Curve(Points, 0 0, 0.5 0.2, 1 1) | Scale(2, 1) | Negate(y) | Swizzle";
        let m = Modifier::parse_chain(s).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(m.len(), 6);
        assert_eq!(Modifier::chain_text(&m), s);
        assert!(Modifier::parse("Deadzone(Radial, 0.5, 0.2)").is_err());
        assert!(Modifier::parse("Curve(Points, 0 0)").is_err());
        assert!(Modifier::parse("Wobble").is_err());
    }
}

//! [`Value`] — a property or setting value in the project document.

use std::fmt;
use std::hash::{Hash, Hasher};

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::EntityKey;

/// A property value. `f64` throughout (no `f32` below `forge-render`, Ch.1.5).
///
/// Equality and hashing compare floats **by bits**, so `-0.0` and `0.0` are different values:
/// undo must restore exactly what was there, and the state hash must see the difference.
/// Non-finite floats never enter the project (`CMD-0004`): they have no JSON form, so they
/// would break the command log and the remote wire.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect)]
pub enum Value {
    /// A boolean.
    Bool(bool),
    /// A signed integer.
    Int(i64),
    /// A float (finite).
    Float(f64),
    /// Text.
    Text(String),
    /// A 3-vector (finite components).
    Vec3([f64; 3]),
    /// A reference to another project entity.
    Entity(EntityKey),
}

impl Value {
    /// True unless a float in it is NaN or infinite.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Float(x) => x.is_finite(),
            Self::Vec3(v) => v.iter().all(|x| x.is_finite()),
            Self::Bool(_) | Self::Int(_) | Self::Text(_) | Self::Entity(_) => true,
        }
    }

    /// The variant's name (`"Float"`), for messages.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "Bool",
            Self::Int(_) => "Int",
            Self::Float(_) => "Float",
            Self::Text(_) => "Text",
            Self::Vec3(_) => "Vec3",
            Self::Entity(_) => "Entity",
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::Text(a), Self::Text(b)) => a == b,
            (Self::Vec3(a), Self::Vec3(b)) => a
                .iter()
                .zip(b.iter())
                .all(|(x, y)| x.to_bits() == y.to_bits()),
            (Self::Entity(a), Self::Entity(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Bool(b) => b.hash(state),
            Self::Int(i) => i.hash(state),
            Self::Float(x) => x.to_bits().hash(state),
            Self::Text(s) => s.hash(state),
            Self::Vec3(v) => {
                for x in v {
                    x.to_bits().hash(state);
                }
            }
            Self::Entity(e) => e.hash(state),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(i) => write!(f, "{i}"),
            Self::Float(x) => write!(f, "{x}"),
            Self::Text(s) => write!(f, "{s:?}"),
            Self::Vec3([x, y, z]) => write!(f, "({x}, {y}, {z})"),
            Self::Entity(e) => write!(f, "{e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_compare_by_bits() {
        assert_ne!(Value::Float(0.0), Value::Float(-0.0));
        assert_eq!(Value::Float(1.5), Value::Float(1.5));
        assert_ne!(Value::Int(1), Value::Float(1.0));
        assert_ne!(Value::Vec3([0.0, 0.0, 0.0]), Value::Vec3([0.0, -0.0, 0.0]));
    }

    #[test]
    fn finiteness() {
        assert!(Value::Float(1.0).is_finite());
        assert!(!Value::Float(f64::NAN).is_finite());
        assert!(!Value::Vec3([0.0, f64::INFINITY, 0.0]).is_finite());
        assert!(Value::Text("x".into()).is_finite());
    }
}

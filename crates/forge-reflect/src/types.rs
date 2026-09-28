//! `ForgeType` — what a value must be to sit on a pin, in a field, or in a schema.

use std::collections::BTreeMap;

use bevy_reflect::{GetTypeRegistration, Reflect, TypePath, TypeRegistry, Typed};
use serde_json::{Value, json};

/// The class of a pin: what widget the inspector draws, what colour the graph editor gives
/// the wire, and which annotations make sense on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PinKind {
    /// `bool`.
    Bool,
    /// Any integer primitive.
    Int,
    /// `f64` (there is no `f32` below `forge-render`, Ch.1.5).
    Float,
    /// `String`.
    Text,
    /// An `EntityId`.
    Entity,
    /// A `FrameId`: a handle to a frame.
    Id,
    /// A `Tick`: canonical time.
    Time,
    /// A `FramePos` / `FrameVel`: a frame plus a 3-vector.
    Vector,
    /// A `#[forge_api]` struct.
    Struct,
    /// A `#[forge_api]` enum.
    Enum,
}

impl PinKind {
    /// Stable lower-case name (schema `x-forge-kind`, messages).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Text => "text",
            Self::Entity => "entity",
            Self::Id => "id",
            Self::Time => "time",
            Self::Vector => "vector",
            Self::Struct => "struct",
            Self::Enum => "enum",
        }
    }

    /// May carry `units` (a physical quantity).
    #[must_use]
    pub fn takes_units(self) -> bool {
        matches!(self, Self::Int | Self::Float | Self::Vector)
    }

    /// May carry `min` / `max` / `step` (a scalar).
    #[must_use]
    pub fn takes_range(self) -> bool {
        matches!(self, Self::Int | Self::Float)
    }
}

/// A type that can appear on a pin, in a `#[forge_api]` struct field or enum variant, and
/// in the JSON Schema: it is reflectable (`bevy_reflect`) **and** it knows its pin class and
/// its schema.
///
/// Implemented here for the primitives and the engine's cross-cutting types, and by
/// `#[forge_api]` for every annotated struct and enum.
pub trait ForgeType: Reflect + Typed + TypePath + GetTypeRegistration {
    /// The pin class.
    fn pin_kind() -> PinKind;

    /// The type's JSON Schema. A named type (struct, enum) defines itself in `defs` and
    /// returns a `$ref` to that definition; a scalar returns its schema inline.
    fn json_schema(defs: &mut SchemaDefs) -> Value;
}

/// Everything forge-reflect needs about a type, as plain function pointers — so a
/// descriptor can be built, stored and compared without generics.
#[derive(Clone, Copy)]
pub struct TypeDesc {
    /// `TypePath::type_path` — the type's identity across all four outputs.
    pub type_path: fn() -> &'static str,
    /// `ForgeType::pin_kind`.
    pub pin_kind: fn() -> PinKind,
    /// `ForgeType::json_schema`.
    pub json_schema: fn(&mut SchemaDefs) -> Value,
    /// Registers the type (and what it contains) in a `bevy_reflect` `TypeRegistry`.
    pub register: fn(&mut TypeRegistry),
}

impl TypeDesc {
    /// The descriptor of `T`.
    #[must_use]
    pub fn of<T: ForgeType>() -> Self {
        Self {
            type_path: <T as TypePath>::type_path,
            pin_kind: T::pin_kind,
            json_schema: T::json_schema,
            register: |r| r.register::<T>(),
        }
    }
}

impl std::fmt::Debug for TypeDesc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str((self.type_path)())
    }
}

/// The `$defs` table of a schema document: named types, keyed by type path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaDefs {
    defs: BTreeMap<String, Value>,
}

impl SchemaDefs {
    /// The `$ref` value for a type path.
    #[must_use]
    pub fn ref_to(type_path: &str) -> Value {
        // JSON Pointer escaping (RFC 6901): `~` -> `~0`, `/` -> `~1`.
        let esc = type_path.replace('~', "~0").replace('/', "~1");
        json!({ "$ref": format!("#/$defs/{esc}") })
    }

    /// Define `type_path` (once; recursion-safe) with the schema `build` produces, and
    /// return a `$ref` to it.
    pub fn define(&mut self, type_path: &str, build: impl FnOnce(&mut Self) -> Value) -> Value {
        if !self.defs.contains_key(type_path) {
            // Placeholder first, so a type that contains itself terminates.
            self.defs.insert(type_path.to_string(), Value::Bool(true));
            let schema = build(self);
            self.defs.insert(type_path.to_string(), schema);
        }
        Self::ref_to(type_path)
    }

    /// The definition of `type_path`, if defined.
    #[must_use]
    pub fn get(&self, type_path: &str) -> Option<&Value> {
        self.defs.get(type_path)
    }

    /// Follow a `{"$ref": "#/$defs/..."}` (or return the schema itself if it is not one).
    #[must_use]
    pub fn resolve<'a>(&'a self, schema: &'a Value) -> &'a Value {
        match schema.get("$ref").and_then(Value::as_str) {
            Some(r) => r
                .strip_prefix("#/$defs/")
                .map(|p| p.replace("~1", "/").replace("~0", "~"))
                .and_then(|p| self.defs.get(&p))
                .unwrap_or(schema),
            None => schema,
        }
    }

    /// Set a definition (replacing any placeholder).
    pub(crate) fn insert(&mut self, type_path: String, schema: Value) {
        self.defs.insert(type_path, schema);
    }

    /// Merge another table in (same keys hold the same definition: they come from the same
    /// type).
    pub fn extend(&mut self, other: SchemaDefs) {
        for (k, v) in other.defs {
            self.defs.entry(k).or_insert(v);
        }
    }

    /// Every definition, by type path.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.defs.iter()
    }

    /// Number of definitions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    /// No definitions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
}

// ---- primitives -------------------------------------------------------------------------

macro_rules! int_type {
    ($($t:ty),*) => {$(
        impl ForgeType for $t {
            fn pin_kind() -> PinKind {
                PinKind::Int
            }
            fn json_schema(_: &mut SchemaDefs) -> Value {
                json!({ "type": "integer", "minimum": <$t>::MIN, "maximum": <$t>::MAX })
            }
        }
    )*};
}
int_type!(i8, i16, i32, i64, u8, u16, u32, u64);

impl ForgeType for bool {
    fn pin_kind() -> PinKind {
        PinKind::Bool
    }
    fn json_schema(_: &mut SchemaDefs) -> Value {
        json!({ "type": "boolean" })
    }
}

impl ForgeType for f64 {
    fn pin_kind() -> PinKind {
        PinKind::Float
    }
    fn json_schema(_: &mut SchemaDefs) -> Value {
        json!({ "type": "number" })
    }
}

impl ForgeType for String {
    fn pin_kind() -> PinKind {
        PinKind::Text
    }
    fn json_schema(_: &mut SchemaDefs) -> Value {
        json!({ "type": "string" })
    }
}

// ---- the cross-cutting types (Ch.1.3) ---------------------------------------------------
//
// Reflected opaque in their own crates (they sit below forge-reflect); their wire shape is
// defined here, once.

impl ForgeType for forge_core::EntityId {
    fn pin_kind() -> PinKind {
        PinKind::Entity
    }
    fn json_schema(_: &mut SchemaDefs) -> Value {
        json!({
            "type": "integer", "minimum": 0, "maximum": u64::MAX,
            "description": "An entity handle (EntityId::to_bits): valid within one session."
        })
    }
}

impl ForgeType for forge_frames::FrameId {
    fn pin_kind() -> PinKind {
        PinKind::Id
    }
    fn json_schema(_: &mut SchemaDefs) -> Value {
        json!({ "type": "integer", "minimum": 0, "maximum": u32::MAX, "description": "A frame (0: the world frame)." })
    }
}

impl ForgeType for forge_frames::Tick {
    fn pin_kind() -> PinKind {
        PinKind::Time
    }
    fn json_schema(_: &mut SchemaDefs) -> Value {
        json!({
            "type": "integer", "minimum": i64::MIN, "maximum": i64::MAX,
            "description": "Canonical time: microseconds from the epoch."
        })
    }
}

fn frame_vector(defs: &mut SchemaDefs, what: &str) -> Value {
    let frame = forge_frames::FrameId::json_schema(defs);
    json!({
        "type": "object",
        "description": what,
        "properties": {
            "frame": frame,
            "local": {
                "type": "object",
                "properties": {
                    "x": { "type": "number" }, "y": { "type": "number" }, "z": { "type": "number" }
                },
                "required": ["x", "y", "z"],
                "additionalProperties": false
            }
        },
        "required": ["frame", "local"],
        "additionalProperties": false
    })
}

impl ForgeType for forge_frames::FramePos {
    fn pin_kind() -> PinKind {
        PinKind::Vector
    }
    fn json_schema(defs: &mut SchemaDefs) -> Value {
        defs.define(<Self as TypePath>::type_path(), |d| {
            frame_vector(
                d,
                "A position: a frame and the offset from its origin (I1).",
            )
        })
    }
}

impl ForgeType for forge_frames::FrameVel {
    fn pin_kind() -> PinKind {
        PinKind::Vector
    }
    fn json_schema(defs: &mut SchemaDefs) -> Value {
        defs.define(<Self as TypePath>::type_path(), |d| {
            frame_vector(d, "A velocity as observed in a frame.")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_resolve_and_escape() {
        let mut d = SchemaDefs::default();
        let r = forge_frames::FramePos::json_schema(&mut d);
        assert_eq!(d.len(), 1);
        let def = d.resolve(&r);
        assert_eq!(def["type"], "object");
        assert_eq!(def["required"], json!(["frame", "local"]));
        // Defining twice is idempotent.
        let r2 = forge_frames::FramePos::json_schema(&mut d);
        assert_eq!(r, r2);
        assert_eq!(d.len(), 1);
        assert_eq!(SchemaDefs::ref_to("a/b~c")["$ref"], "#/$defs/a~1b~0c");
    }

    #[test]
    fn kinds_gate_annotations() {
        assert!(PinKind::Float.takes_units() && PinKind::Vector.takes_units());
        assert!(!PinKind::Bool.takes_units() && !PinKind::Text.takes_units());
        assert!(PinKind::Int.takes_range() && !PinKind::Vector.takes_range());
    }
}

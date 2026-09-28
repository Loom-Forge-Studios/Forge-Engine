//! The four outputs of `#[forge_api]` and the specs the macro emits to build them.
//!
//! The macro parses an item once and emits, **as separate token lists**, the pins for the
//! node descriptor, the properties for the JSON Schema fragment and (for a mutating fn) the
//! fields of the command variant, next to the `bevy_reflect` `TypeInfo` of every type in the
//! signature. [`ApiItem`] builds the outputs from those lists; [`crate::check_agreement`]
//! proves they still describe the same thing.

use bevy_reflect::TypeInfo;
use serde_json::{Map, Value, json};

use crate::{PinKind, ReflectError, SchemaDefs, TypeDesc, Unit};

/// The per-field / per-pin annotation set (Ch.6 §6.2). It drives the inspector (WP-U6
/// generates it from these), the node pins and the schema keywords.
///
/// Written as `#[forge(...)]` on a struct field, an enum-variant field or a fn parameter,
/// and as `#[forge_api(returns(...))]` for a fn's return value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldMeta {
    /// Label: `name = "..."`, else the identifier humanised (`max_speed` → `Max Speed`).
    pub display_name: &'static str,
    /// Tooltip: the field's doc comment (`doc = "..."` for fn parameters, which cannot carry
    /// doc comments). Also the schema `description`.
    pub doc: &'static str,
    /// Inspector group: `category = "..."`.
    pub category: Option<&'static str>,
    /// `units = "N·s"` — checked at compile time and on every pin connection.
    pub units: Option<&'static str>,
    /// `min = ..` / `range = a..=b` — inclusive lower bound.
    pub min: Option<f64>,
    /// `max = ..` / `range = a..=b` — inclusive upper bound.
    pub max: Option<f64>,
    /// `step = ..` — drag/spin increment.
    pub step: Option<f64>,
    /// `read_only` — shown, never edited (no command is emitted for it).
    pub read_only: bool,
    /// `hidden` — not shown in the inspector (still serialised and still a pin).
    pub hidden: bool,
    /// `widget = "slider" | "color" | "angle" | ...` — an editor hint; unknown hints fall back
    /// to the default editor for the kind.
    pub widget: Option<&'static str>,
    /// `entity` — the value is an entity reference (pickers, hierarchy drag-drop).
    pub entity: bool,
}

/// One pin / field / property as the macro emits it.
#[derive(Clone, Debug)]
pub struct PinSpec {
    /// Rust identifier (`"0"`, `"1"` never occur: tuple fields are not supported).
    pub name: &'static str,
    /// Its type.
    pub ty: TypeDesc,
    /// Its annotations.
    pub meta: FieldMeta,
}

/// A resolved pin (node input/output, command field, inspector row).
#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    /// Rust identifier.
    pub name: &'static str,
    /// `TypePath::type_path` of its type.
    pub type_path: &'static str,
    /// Its class.
    pub kind: PinKind,
    /// Parsed `units` (`None` if absent, or if it failed to parse — registration reports it).
    pub unit: Option<Unit>,
    /// Its annotations.
    pub meta: FieldMeta,
}

impl Pin {
    /// Resolve a spec.
    #[must_use]
    pub fn from_spec(s: &PinSpec) -> Self {
        Self {
            name: s.name,
            type_path: (s.ty.type_path)(),
            kind: (s.ty.pin_kind)(),
            unit: s.meta.units.and_then(|u| Unit::parse(u).ok()),
            meta: s.meta.clone(),
        }
    }
}

/// What connecting two pins does to the value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Connection {
    /// Multiply the source value by this (1 when the units are equal or absent).
    pub scale: f64,
}

/// May an output pin feed an input pin? The graph editor calls this on every wire (Ch.6:
/// "the graph editor refuses to connect an `N·s` pin to an `m/s` pin").
///
/// * Types must be identical (`REFLECT-0007`) — no implicit int/float promotion.
/// * If **both** pins carry units, their dimensions must match (`REFLECT-0008`); the
///   returned scale converts between them (`km` → `m` is 1000). A pin with no unit
///   annotation is unchecked (a literal constant, a generic helper).
pub fn connect(from: &Pin, to: &Pin) -> Result<Connection, ReflectError> {
    if from.type_path != to.type_path {
        return Err(ReflectError::IncompatibleTypes {
            from: from.type_path.to_string(),
            to: to.type_path.to_string(),
        });
    }
    match (&from.unit, &to.unit) {
        (Some(a), Some(b)) => Ok(Connection {
            scale: a.conversion_to(b)?,
        }),
        _ => Ok(Connection { scale: 1.0 }),
    }
}

/// Does a fn change project state?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Purity {
    /// No context parameter: same inputs, same outputs, no effects. The graph compiler may
    /// fold, reorder or cache it.
    Pure,
    /// Reads project state through a `&` context parameter (`&World`); changes nothing.
    Read,
    /// Takes a `&mut` context parameter: changes project state, so it is a **command**
    /// (I7) — undoable, provenance-tagged, dry-runnable.
    Mutate,
}

impl Purity {
    /// `"pure" | "read" | "mutate"` (schema `x-forge-purity`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Pure => "pure",
            Self::Read => "read",
            Self::Mutate => "mutate",
        }
    }
}

/// A reference parameter of a fn: the execution context, not a pin (`world: &mut World`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextParam {
    /// Parameter name.
    pub name: &'static str,
    /// The referenced type as written (`World`, `RegionView < '_ >`).
    pub type_name: &'static str,
    /// `&mut`.
    pub mutable: bool,
}

/// One by-value fn parameter as `bevy_reflect` sees its type.
#[derive(Clone, Debug)]
pub struct ReflectArg {
    /// Parameter name.
    pub name: &'static str,
    /// `<T as Typed>::type_info()`.
    pub info: &'static TypeInfo,
}

/// What `#[forge_api]` emits for a free fn.
#[derive(Clone, Debug)]
pub struct FnSpec {
    /// `module_path!()::ident`.
    pub path: &'static str,
    /// The fn's identifier.
    pub ident: &'static str,
    /// Node title: `#[forge_api(name = "...")]`, else humanised.
    pub display_name: &'static str,
    /// Palette category: `#[forge_api(category = "...")]`.
    pub category: Option<&'static str>,
    /// The doc comment: node tooltip **and** API description.
    pub doc: &'static str,
    /// Inferred from the context parameters (and checked against an explicit
    /// `pure` / `reads` / `mutates`).
    pub purity: Purity,
    /// `#[forge_api(mutates, destructive)]`: an automation session needs a capability grant
    /// (Ch.22.3).
    pub destructive: bool,
    /// Returns `Result<_, _>`.
    pub fallible: bool,
    /// Reference parameters.
    pub context: Vec<ContextParam>,
    /// Output 1: `bevy_reflect` view of the by-value parameters.
    pub reflect_args: Vec<ReflectArg>,
    /// Output 1: `bevy_reflect` view of the (success) return type, `None` for `()`.
    pub reflect_ret: Option<&'static TypeInfo>,
    /// Output 2: node input pins.
    pub node_inputs: Vec<PinSpec>,
    /// Output 2: node output pin.
    pub node_output: Option<PinSpec>,
    /// Output 3: schema properties.
    pub schema_props: Vec<PinSpec>,
    /// Output 3: schema return.
    pub schema_return: Option<PinSpec>,
    /// Output 4: command fields — `Some` exactly when the fn mutates.
    pub command_fields: Option<Vec<PinSpec>>,
}

/// What `#[forge_api]` emits for a struct with named fields.
#[derive(Clone, Debug)]
pub struct StructSpec {
    /// The struct itself.
    pub ty: TypeDesc,
    /// Output 1: `<Self as Typed>::type_info()`.
    pub info: &'static TypeInfo,
    /// Identifier.
    pub ident: &'static str,
    /// Inspector / node title.
    pub display_name: &'static str,
    /// Category.
    pub category: Option<&'static str>,
    /// Doc comment.
    pub doc: &'static str,
    /// Output 2: fields as make-node input pins (and inspector rows).
    pub node_fields: Vec<PinSpec>,
    /// Output 3: fields as schema properties.
    pub schema_fields: Vec<PinSpec>,
}

/// One enum variant (unit, or with named fields).
#[derive(Clone, Debug)]
pub struct VariantSpec {
    /// Identifier.
    pub name: &'static str,
    /// Title.
    pub display_name: &'static str,
    /// Doc comment.
    pub doc: &'static str,
    /// Named fields (empty for a unit variant).
    pub fields: Vec<PinSpec>,
}

/// What `#[forge_api]` emits for an enum.
#[derive(Clone, Debug)]
pub struct EnumSpec {
    /// The enum itself.
    pub ty: TypeDesc,
    /// Output 1.
    pub info: &'static TypeInfo,
    /// Identifier.
    pub ident: &'static str,
    /// Title.
    pub display_name: &'static str,
    /// Category.
    pub category: Option<&'static str>,
    /// Doc comment.
    pub doc: &'static str,
    /// Output 2.
    pub node_variants: Vec<VariantSpec>,
    /// Output 3.
    pub schema_variants: Vec<VariantSpec>,
}

/// What kind of item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// A free function: a graph node, an API-callable operation, a command if it mutates.
    Fn,
    /// A struct: an inspector-editable type, a make node, a schema definition.
    Struct,
    /// An enum: a select node, a schema `oneOf`.
    Enum,
}

/// Output 1 — the `bevy_reflect` registration.
#[derive(Clone, Debug)]
pub enum ReflectDesc {
    /// A fn: the `TypeInfo` of each by-value parameter and of the return type (each type is
    /// also registered in the registry's `TypeRegistry`).
    Fn {
        /// By-value parameters.
        args: Vec<ReflectArg>,
        /// Success return type.
        ret: Option<&'static TypeInfo>,
    },
    /// A struct or enum: its own `TypeInfo` (registered in the `TypeRegistry`).
    Type(&'static TypeInfo),
}

/// An enum variant on a select node / in the inspector's variant picker.
#[derive(Clone, Debug, PartialEq)]
pub struct VariantDesc {
    /// Identifier.
    pub name: &'static str,
    /// Title.
    pub display_name: &'static str,
    /// Tooltip.
    pub doc: &'static str,
    /// Its fields.
    pub fields: Vec<Pin>,
}

/// Output 2 — the blueprint node descriptor (Ch.24.2: "every `#[forge_api]` item is a node").
#[derive(Clone, Debug, PartialEq)]
pub struct NodeDesc {
    /// Item path.
    pub path: String,
    /// Title.
    pub display_name: &'static str,
    /// Palette category.
    pub category: Option<&'static str>,
    /// Tooltip (the doc comment).
    pub doc: &'static str,
    /// Purity (structs and enums are `Pure`).
    pub purity: Purity,
    /// Returns `Result`.
    pub fallible: bool,
    /// Needs a capability grant.
    pub destructive: bool,
    /// Context the node runs in (not pins).
    pub context: Vec<ContextParam>,
    /// Input pins (a struct's make node: its fields).
    pub inputs: Vec<Pin>,
    /// Output pins (0 or 1).
    pub outputs: Vec<Pin>,
    /// An enum's variants (empty otherwise).
    pub variants: Vec<VariantDesc>,
}

/// Output 4 — the command variant a mutating fn becomes on the bus (Ch.7, I7). `forge-cmd`
/// (WP-05) turns these into `EditorCommand` variants.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandDesc {
    /// `UpperCamelCase` of the fn name (`apply_impulse` → `ApplyImpulse`).
    pub variant: String,
    /// The fn it invokes.
    pub target: String,
    /// Description (the doc comment).
    pub doc: &'static str,
    /// Needs a capability grant.
    pub destructive: bool,
    /// Can be rejected.
    pub fallible: bool,
    /// Payload fields.
    pub fields: Vec<Pin>,
}

/// One `#[forge_api]` item and its four outputs.
#[derive(Clone, Debug)]
pub struct ApiItem {
    /// `module::path::ident` (a struct or enum: its `TypePath`).
    pub path: String,
    /// Identifier.
    pub ident: &'static str,
    /// Kind.
    pub kind: ItemKind,
    /// Doc comment.
    pub doc: &'static str,
    /// Output 1.
    pub reflect: ReflectDesc,
    /// Output 2.
    pub node: NodeDesc,
    /// Output 3: this item's schema fragment (a struct/enum: its definition).
    pub schema: Value,
    /// Named types the fragment references.
    pub schema_defs: SchemaDefs,
    /// Output 4 (mutating fns only).
    pub command: Option<CommandDesc>,
    /// Every type to register in the `TypeRegistry`.
    pub types: Vec<TypeDesc>,
}

/// Implemented by `#[forge_api]` for every annotated item (for a fn: on a hidden braced
/// struct with the fn's name, which lives in the type namespace, so `register::<my_fn>()`
/// names it).
pub trait ForgeApi {
    /// Build the item's four outputs.
    fn describe() -> ApiItem;
}

/// `apply_impulse` → `ApplyImpulse`.
#[must_use]
pub fn upper_camel(ident: &str) -> String {
    ident
        .split('_')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut c = s.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}

/// Doc-comment text as `bevy_reflect` stores it (raw `///` lines joined by `\n`) normalised
/// the way the macro normalises it: one leading space dropped per line, then trimmed.
#[must_use]
pub fn normalize_doc(raw: &str) -> String {
    raw.lines()
        .map(|l| l.strip_prefix(' ').unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// A pin's schema: its type's schema plus the annotation keywords.
pub(crate) fn pin_schema(s: &PinSpec, defs: &mut SchemaDefs) -> Value {
    let base = (s.ty.json_schema)(defs);
    let mut o = match base {
        Value::Object(o) => o,
        other => {
            let mut o = Map::new();
            o.insert("allOf".into(), json!([other]));
            o
        }
    };
    let m = &s.meta;
    o.insert("x-forge-type".into(), json!((s.ty.type_path)()));
    o.insert("x-forge-kind".into(), json!((s.ty.pin_kind)().name()));
    o.insert("title".into(), json!(m.display_name));
    if m.doc.is_empty() {
        o.remove("description");
    } else {
        o.insert("description".into(), json!(m.doc));
    }
    if let Some(u) = m.units {
        o.insert("x-forge-unit".into(), json!(u));
        if let Ok(unit) = Unit::parse(u) {
            o.insert("x-forge-dimension".into(), json!(unit.si_dimension()));
        }
    }
    if let Some(v) = m.min {
        o.insert("minimum".into(), json!(v));
    }
    if let Some(v) = m.max {
        o.insert("maximum".into(), json!(v));
    }
    if let Some(v) = m.step {
        o.insert("x-forge-step".into(), json!(v));
    }
    if m.read_only {
        o.insert("readOnly".into(), json!(true));
    }
    if m.hidden {
        o.insert("x-forge-hidden".into(), json!(true));
    }
    if let Some(c) = m.category {
        o.insert("x-forge-category".into(), json!(c));
    }
    if let Some(w) = m.widget {
        o.insert("x-forge-widget".into(), json!(w));
    }
    if m.entity {
        o.insert("x-forge-entity".into(), json!(true));
    }
    Value::Object(o)
}

/// `{type: object, properties, required (declaration order), additionalProperties: false}`.
fn object_schema(fields: &[PinSpec], defs: &mut SchemaDefs) -> Map<String, Value> {
    let mut props = Map::new();
    let mut required = Vec::new();
    for f in fields {
        props.insert(f.name.to_string(), pin_schema(f, defs));
        required.push(json!(f.name));
    }
    let mut o = Map::new();
    o.insert("type".into(), json!("object"));
    o.insert("properties".into(), Value::Object(props));
    o.insert("required".into(), Value::Array(required));
    o.insert("additionalProperties".into(), json!(false));
    o
}

fn set_doc(o: &mut Map<String, Value>, title: &str, doc: &str) {
    o.insert("title".into(), json!(title));
    if !doc.is_empty() {
        o.insert("description".into(), json!(doc));
    }
}

/// The schema definition of a `#[forge_api]` struct (its `ForgeType::json_schema` defines
/// this under its type path).
pub fn struct_schema(s: &StructSpec, defs: &mut SchemaDefs) -> Value {
    let mut o = object_schema(&s.schema_fields, defs);
    set_doc(&mut o, s.display_name, s.doc);
    o.insert("x-forge-type".into(), json!((s.ty.type_path)()));
    o.insert("x-forge-item".into(), json!("struct"));
    if let Some(c) = s.category {
        o.insert("x-forge-category".into(), json!(c));
    }
    Value::Object(o)
}

/// The schema definition of a `#[forge_api]` enum: `oneOf` in declaration order, serde's
/// externally-tagged shape (`"Variant"` or `{"Variant": {fields}}`).
pub fn enum_schema(s: &EnumSpec, defs: &mut SchemaDefs) -> Value {
    let one_of: Vec<Value> = s
        .schema_variants
        .iter()
        .map(|v| {
            let mut o = if v.fields.is_empty() {
                let mut o = Map::new();
                o.insert("const".into(), json!(v.name));
                o
            } else {
                let body = Value::Object(object_schema(&v.fields, defs));
                let mut o = Map::new();
                o.insert("type".into(), json!("object"));
                let mut props = Map::new();
                props.insert(v.name.to_string(), body);
                o.insert("properties".into(), Value::Object(props));
                o.insert("required".into(), json!([v.name]));
                o.insert("additionalProperties".into(), json!(false));
                o
            };
            o.insert("x-forge-variant".into(), json!(v.name));
            set_doc(&mut o, v.display_name, v.doc);
            Value::Object(o)
        })
        .collect();
    let mut o = Map::new();
    o.insert("oneOf".into(), Value::Array(one_of));
    set_doc(&mut o, s.display_name, s.doc);
    o.insert("x-forge-type".into(), json!((s.ty.type_path)()));
    o.insert("x-forge-item".into(), json!("enum"));
    if let Some(c) = s.category {
        o.insert("x-forge-category".into(), json!(c));
    }
    Value::Object(o)
}

fn fn_schema(s: &FnSpec, defs: &mut SchemaDefs) -> Value {
    let mut o = object_schema(&s.schema_props, defs);
    o.insert("$id".into(), json!(format!("schema://forge/{}", s.path)));
    set_doc(&mut o, s.display_name, s.doc);
    o.insert("x-forge-item".into(), json!("fn"));
    o.insert("x-forge-path".into(), json!(s.path));
    o.insert("x-forge-purity".into(), json!(s.purity.name()));
    o.insert("x-forge-fallible".into(), json!(s.fallible));
    if s.destructive {
        o.insert("x-forge-destructive".into(), json!(true));
    }
    if let Some(c) = s.category {
        o.insert("x-forge-category".into(), json!(c));
    }
    if let Some(r) = &s.schema_return {
        o.insert("x-forge-returns".into(), pin_schema(r, defs));
    }
    if s.purity == Purity::Mutate {
        o.insert("x-forge-command".into(), json!(upper_camel(s.ident)));
    }
    let ctx: Vec<Value> = s
        .context
        .iter()
        .map(|c| json!({ "name": c.name, "type": c.type_name, "mutable": c.mutable }))
        .collect();
    o.insert("x-forge-context".into(), Value::Array(ctx));
    Value::Object(o)
}

impl ApiItem {
    /// Build a fn's outputs.
    #[must_use]
    pub fn function(s: FnSpec) -> Self {
        let mut defs = SchemaDefs::default();
        let schema = fn_schema(&s, &mut defs);
        let pins = |v: &[PinSpec]| v.iter().map(Pin::from_spec).collect::<Vec<_>>();
        let mut types: Vec<TypeDesc> = s.node_inputs.iter().map(|p| p.ty).collect();
        types.extend(s.node_output.iter().map(|p| p.ty));
        let node = NodeDesc {
            path: s.path.to_string(),
            display_name: s.display_name,
            category: s.category,
            doc: s.doc,
            purity: s.purity,
            fallible: s.fallible,
            destructive: s.destructive,
            context: s.context.clone(),
            inputs: pins(&s.node_inputs),
            outputs: s.node_output.iter().map(Pin::from_spec).collect(),
            variants: Vec::new(),
        };
        let command = s.command_fields.as_ref().map(|f| CommandDesc {
            variant: upper_camel(s.ident),
            target: s.path.to_string(),
            doc: s.doc,
            destructive: s.destructive,
            fallible: s.fallible,
            fields: pins(f),
        });
        Self {
            path: s.path.to_string(),
            ident: s.ident,
            kind: ItemKind::Fn,
            doc: s.doc,
            reflect: ReflectDesc::Fn {
                args: s.reflect_args,
                ret: s.reflect_ret,
            },
            node,
            schema,
            schema_defs: defs,
            command,
            types,
        }
    }

    /// Build a struct's outputs.
    #[must_use]
    pub fn structure(s: StructSpec) -> Self {
        let mut defs = SchemaDefs::default();
        let schema = struct_schema(&s, &mut defs);
        let path = (s.ty.type_path)().to_string();
        let self_pin = PinSpec {
            name: "value",
            ty: s.ty,
            meta: FieldMeta {
                display_name: s.display_name,
                doc: s.doc,
                ..FieldMeta::default()
            },
        };
        let mut types = vec![s.ty];
        types.extend(s.node_fields.iter().map(|p| p.ty));
        let node = NodeDesc {
            path: path.clone(),
            display_name: s.display_name,
            category: s.category,
            doc: s.doc,
            purity: Purity::Pure,
            fallible: false,
            destructive: false,
            context: Vec::new(),
            inputs: s.node_fields.iter().map(Pin::from_spec).collect(),
            outputs: vec![Pin::from_spec(&self_pin)],
            variants: Vec::new(),
        };
        Self {
            path,
            ident: s.ident,
            kind: ItemKind::Struct,
            doc: s.doc,
            reflect: ReflectDesc::Type(s.info),
            node,
            schema,
            schema_defs: defs,
            command: None,
            types,
        }
    }

    /// Build an enum's outputs.
    #[must_use]
    pub fn enumeration(s: EnumSpec) -> Self {
        let mut defs = SchemaDefs::default();
        let schema = enum_schema(&s, &mut defs);
        let path = (s.ty.type_path)().to_string();
        let self_pin = PinSpec {
            name: "value",
            ty: s.ty,
            meta: FieldMeta {
                display_name: s.display_name,
                doc: s.doc,
                ..FieldMeta::default()
            },
        };
        let mut types = vec![s.ty];
        types.extend(
            s.node_variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|p| p.ty)),
        );
        let node = NodeDesc {
            path: path.clone(),
            display_name: s.display_name,
            category: s.category,
            doc: s.doc,
            purity: Purity::Pure,
            fallible: false,
            destructive: false,
            context: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![Pin::from_spec(&self_pin)],
            variants: s
                .node_variants
                .iter()
                .map(|v| VariantDesc {
                    name: v.name,
                    display_name: v.display_name,
                    doc: v.doc,
                    fields: v.fields.iter().map(Pin::from_spec).collect(),
                })
                .collect(),
        };
        Self {
            path,
            ident: s.ident,
            kind: ItemKind::Enum,
            doc: s.doc,
            reflect: ReflectDesc::Type(s.info),
            node,
            schema,
            schema_defs: defs,
            command: None,
            types,
        }
    }

    /// Every pin the item has (node inputs, outputs, variant fields) — what validation walks.
    pub fn all_pins(&self) -> impl Iterator<Item = &Pin> {
        self.node
            .inputs
            .iter()
            .chain(&self.node.outputs)
            .chain(self.node.variants.iter().flat_map(|v| &v.fields))
    }

    /// Check the annotations make sense for each pin's kind: `units` parses and is on a
    /// quantity; `min`/`max`/`step` are finite, ordered, positive-step and on a scalar;
    /// `entity` is on an `EntityId`.
    pub fn validate(&self) -> Result<(), ReflectError> {
        for p in self.all_pins() {
            let wrong = |attr: &'static str| ReflectError::MetaOnWrongKind {
                item: self.path.clone(),
                pin: p.name.to_string(),
                attr,
                kind: p.kind.name(),
            };
            let bad = |why: &'static str| ReflectError::BadRange {
                item: self.path.clone(),
                pin: p.name.to_string(),
                why,
            };
            if let Some(u) = p.meta.units {
                Unit::parse(u)?;
                if !p.kind.takes_units() {
                    return Err(wrong("units"));
                }
            }
            let m = &p.meta;
            if (m.min.is_some() || m.max.is_some() || m.step.is_some()) && !p.kind.takes_range() {
                return Err(wrong("range"));
            }
            if [m.min, m.max, m.step]
                .iter()
                .flatten()
                .any(|v| !v.is_finite())
            {
                return Err(bad("a bound is not finite"));
            }
            if let (Some(lo), Some(hi)) = (m.min, m.max)
                && lo > hi
            {
                return Err(bad("min > max"));
            }
            if m.step.is_some_and(|s| s <= 0.0) {
                return Err(bad("step must be positive"));
            }
            if m.entity && p.kind != PinKind::Entity {
                return Err(wrong("entity"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_and_docs() {
        assert_eq!(upper_camel("apply_impulse"), "ApplyImpulse");
        assert_eq!(upper_camel("x"), "X");
        assert_eq!(
            normalize_doc(" Line one.\n Line two.\n"),
            "Line one.\nLine two."
        );
        assert_eq!(normalize_doc(""), "");
    }

    fn pin(ty: TypeDesc, units: Option<&'static str>) -> Pin {
        Pin::from_spec(&PinSpec {
            name: "p",
            ty,
            meta: FieldMeta {
                units,
                ..FieldMeta::default()
            },
        })
    }

    #[test]
    fn connection_rules() {
        let f = TypeDesc::of::<f64>();
        // Same dimension: allowed, scaled.
        let c = connect(&pin(f, Some("km")), &pin(f, Some("m"))).expect("km->m");
        assert_eq!(c.scale, 1000.0);
        // Different dimension: refused.
        let e = connect(&pin(f, Some("N·s")), &pin(f, Some("m/s"))).expect_err("refused");
        assert!(matches!(e, ReflectError::IncompatibleUnits { .. }));
        // Unannotated side: unchecked.
        assert_eq!(
            connect(&pin(f, None), &pin(f, Some("m/s")))
                .expect("ok")
                .scale,
            1.0
        );
        // Different types: refused regardless of units.
        let e = connect(&pin(TypeDesc::of::<i64>(), Some("m")), &pin(f, Some("m")))
            .expect_err("refused");
        assert!(matches!(e, ReflectError::IncompatibleTypes { .. }));
    }
}

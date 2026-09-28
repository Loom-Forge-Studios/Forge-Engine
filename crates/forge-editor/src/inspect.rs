//! The inspector's model (Ch.21 §21.21 "Inspector", DoD M2-37), **generated from
//! reflection** (Ch.6 §6.2): every row, label, tooltip, unit, range, step, editor hint and
//! group comes from a type's `#[forge_api]` / `#[forge(...)]` metadata; nothing here is
//! written per type.
//!
//! * **Components.** A component is a registered `#[forge_api]` struct under a key
//!   (`transform`). On an entity it is the set of properties under `<key>.` — a field
//!   `max_thrust` is the property `thruster.max_thrust`, a nested struct's field
//!   `panel.size.w`, an enum field holds its variant name and its active variant's fields
//!   sit beside it (`light.kind`, `light.cone`), a `FramePos` is `<path>.frame` +
//!   `<path>.local` (I1: a position is never a bare vector). Adding a component sets every
//!   default (read through reflection from the type's `Default`); removing it removes every
//!   property under the key — each one transaction, one undo entry.
//! * **Every reflected kind has an editor** ([`EditorKind`], [`editor_for`]): bool, every
//!   integer, `f64`, `String`, entity references, frame/body ids, `Tick`, `FramePos` /
//!   `FrameVel`, nested structs and enums (unit and data variants).
//! * **Multi-object editing.** [`field_state`] reads a row across the selection and says
//!   whether the values are mixed; an edit sets the row on every selected entity in one
//!   transaction (a drag: one gesture).
//! * **Touch to promote (Ch.14).** An entity with a `gen.seed_path` property is generated
//!   content. Editing it through the inspector also sets `gen.promoted` in the same
//!   transaction, so the edit is what promotes it to authored content.
//! * **`InspectorWidget` (Ch.21 §21.19).** A plugin replaces the editor of a type, of a
//!   `widget = "..."` hint, or of a whole component, through the ordinary extension point:
//!   its build function gets an [`InspectorCx`] and queues live build steps.

use std::collections::BTreeMap;

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_plugin::Registry;
use forge_plugin::points::{InspectorWidget, InspectorWidgetDescriptor, WidgetTarget};
use forge_reflect::bevy_reflect::{PartialReflect, ReflectRef, TypePath};
use forge_reflect::{ForgeApi, ForgeRegistry, InspectorField, Pin, PinKind, forge_api};

use crate::EditorError;
use crate::mirror::{MirrorEntity, ProjectMirror};
use crate::panels::LiveStep;

/// Property holding a generated entity's seed path (Ch.4 §4.3).
pub const GEN_SEED_PATH: &str = "gen.seed_path";
/// Set once a generated entity has been edited by hand (Ch.14 "touch to promote").
pub const GEN_PROMOTED: &str = "gen.promoted";

/// Which editor a row gets.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EditorKind {
    /// A (tri-state for mixed) checkbox.
    Toggle,
    /// An integer field (with units, bounds and step).
    Integer,
    /// A number field (with units, bounds and step).
    Number,
    /// A slider for a bounded number with `widget = "slider"` (a drag is one gesture).
    Slider,
    Text,
    /// An entity picker: a combo of the project's entities plus drag-and-drop from the
    /// hierarchy.
    EntityPicker,
    /// A frame or body id.
    Id,
    /// Canonical time (`Tick`, microseconds), shown in seconds.
    Time,
    /// A `FramePos` / `FrameVel`: frame id plus a 3-vector.
    FramePos,
    /// A choice of enum variant (its data variant's fields follow as rows).
    Variant,
    /// A nested struct: a group header; its fields follow as rows.
    Group,
}

/// The editor for a field (see [`EditorKind`]). Total over [`PinKind`].
pub fn editor_for(f: &InspectorField) -> EditorKind {
    match f.kind {
        PinKind::Bool => EditorKind::Toggle,
        PinKind::Int => EditorKind::Integer,
        PinKind::Float if f.widget == Some("slider") && f.min.is_some() && f.max.is_some() => {
            EditorKind::Slider
        }
        PinKind::Float => EditorKind::Number,
        PinKind::Text => EditorKind::Text,
        PinKind::Entity => EditorKind::EntityPicker,
        PinKind::Id => EditorKind::Id,
        PinKind::Time => EditorKind::Time,
        PinKind::Vector => EditorKind::FramePos,
        PinKind::Enum => EditorKind::Variant,
        PinKind::Struct => EditorKind::Group,
    }
}

// ---- exact integers ---------------------------------------------------------------------
//
// Integer-valued rows (`Int`, `Id`, `Time`) are edited as `i128`, never through `f64`
// (exact only to 2^53). A property holds a `Value::Int(i64)`; a `u64` above `i64::MAX`
// is stored as its two's-complement bit pattern and read back through the field's type,
// so all 64 bits of a seed or a hash survive a round trip.

/// The inclusive range a row's integer type holds (`u8`: 0..=255, `FrameId`: the `u32`
/// range, `Tick`: the `i64` range), narrowed by its `#[forge(range)]`. `None` for a
/// row that is not integer-valued.
pub fn int_range(f: &InspectorField) -> Option<(i128, i128)> {
    let (lo, hi): (i128, i128) = match f.kind {
        PinKind::Int => match f.type_path {
            "i8" => (i8::MIN.into(), i8::MAX.into()),
            "i16" => (i16::MIN.into(), i16::MAX.into()),
            "i32" => (i32::MIN.into(), i32::MAX.into()),
            "u8" => (0, u8::MAX.into()),
            "u16" => (0, u16::MAX.into()),
            "u32" => (0, u32::MAX.into()),
            "u64" => (0, u64::MAX.into()),
            _ => (i64::MIN.into(), i64::MAX.into()),
        },
        PinKind::Id => (0, u32::MAX.into()),
        PinKind::Time => (i64::MIN.into(), i64::MAX.into()),
        _ => return None,
    };
    // `as` saturates; the bounds are whole numbers in practice (ceil/floor otherwise).
    let lo = f.min.map_or(lo, |m| lo.max(m.ceil() as i128));
    let hi = f.max.map_or(hi, |m| hi.min(m.floor() as i128));
    Some((lo, hi.max(lo)))
}

/// A stored value as the row's integer (a `u64` row reads the bit pattern back as
/// unsigned). `None` for a non-integer value.
pub fn int_of_value(f: &InspectorField, v: &Value) -> Option<i128> {
    match v {
        Value::Int(i) if f.kind == PinKind::Int && f.type_path == "u64" => {
            Some(i128::from(*i as u64))
        }
        Value::Int(i) => Some(i128::from(*i)),
        _ => None,
    }
}

/// The value an integer row stores for `x`: snapped to the row's step (from its minimum)
/// and clamped into [`int_range`], so a `u8` row can never hold 300 or -1.
pub fn int_value(f: &InspectorField, x: i128) -> Value {
    let (lo, hi) = int_range(f).unwrap_or((i64::MIN.into(), i64::MAX.into()));
    let mut x = x;
    if f.kind == PinKind::Int
        && let Some(step) = f.step.filter(|s| *s >= 1.0)
    {
        let step = step.round() as i128;
        let base = if f.min.is_some() { lo } else { 0 };
        let (q, r) = ((x - base).div_euclid(step), (x - base).rem_euclid(step));
        x = base + (q + i128::from(2 * r >= step)) * step;
    }
    let x = x.clamp(lo, hi);
    let stored = if f.kind == PinKind::Int && f.type_path == "u64" {
        // Two's-complement bit pattern: in range 0..=u64::MAX, so the cast is lossless.
        (x as u64) as i64
    } else {
        i64::try_from(x).unwrap_or(if x < 0 { i64::MIN } else { i64::MAX })
    };
    Value::Int(stored)
}

/// Every pin kind (the coverage guard walks it).
pub const ALL_PIN_KINDS: [PinKind; 10] = [
    PinKind::Bool,
    PinKind::Int,
    PinKind::Float,
    PinKind::Text,
    PinKind::Entity,
    PinKind::Id,
    PinKind::Time,
    PinKind::Vector,
    PinKind::Struct,
    PinKind::Enum,
];

/// One inspector row of a component.
#[derive(Clone, Debug, PartialEq)]
pub struct InspectorRow {
    /// Path within the component (`size.w`, `kind`, `cone`).
    pub path: String,
    pub field: InspectorField,
    /// Nesting depth (0: a top-level field).
    pub depth: usize,
    /// Rows inside an enum's data variant: `(the enum row's path, variant name)`; shown
    /// only while that variant is active.
    pub variant_of: Option<(String, &'static str)>,
}

impl InspectorRow {
    pub fn editor(&self) -> EditorKind {
        editor_for(&self.field)
    }
    /// The property paths (relative to the component) holding this row's value.
    pub fn storage(&self) -> Vec<String> {
        match self.field.kind {
            PinKind::Struct => Vec::new(),
            PinKind::Vector => vec![
                format!("{}.frame", self.path),
                format!("{}.local", self.path),
            ],
            _ => vec![self.path.clone()],
        }
    }
    /// The label with its unit (`Max Thrust (kN)`).
    /// The row's label as shown: the field's label (a string key: the `#[forge_api]` field
    /// name the reflection gives, looked up) and its unit symbol.
    pub fn label(&self) -> String {
        let label = forge_ui::l10n::tr(self.field.label);
        match self.field.units {
            Some(unit) => forge_ui::trf!("{label} ({unit})", label, unit),
            None => label.to_string(),
        }
    }
}

/// A registered component (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentDef {
    /// Property prefix (`transform`).
    pub key: String,
    pub type_path: String,
    pub title: &'static str,
    pub doc: &'static str,
    pub rows: Vec<InspectorRow>,
    /// `(relative property path, value)` set when the component is added.
    pub defaults: Vec<(String, Value)>,
    /// Per data variant: the defaults of its fields (set when switching to it).
    variant_defaults: BTreeMap<(String, String), Vec<(String, Value)>>,
}

impl ComponentDef {
    /// The project property of a relative path.
    pub fn prop(&self, rel: &str) -> String {
        format!("{}.{rel}", self.key)
    }
    fn prefix(&self) -> String {
        format!("{}.", self.key)
    }
    /// True if the entity carries this component.
    pub fn present_on(&self, e: &MirrorEntity) -> bool {
        let p = self.prefix();
        e.properties
            .range(p.clone()..)
            .next()
            .is_some_and(|(k, _)| k.starts_with(&p))
    }
    /// The default of a relative storage path.
    pub fn default_of(&self, rel: &str) -> Option<&Value> {
        self.defaults
            .iter()
            .chain(self.variant_defaults.values().flatten())
            .find(|(p, _)| p == rel)
            .map(|(_, v)| v)
    }
    /// The variant an enum row currently holds on `e` (its default when unset).
    pub fn variant_on(&self, e: &MirrorEntity, enum_path: &str) -> Option<String> {
        match e
            .properties
            .get(&self.prop(enum_path))
            .or(self.default_of(enum_path))
        {
            Some(Value::Text(t)) => Some(t.clone()),
            _ => None,
        }
    }
    /// Is `row` shown for `e` (its enclosing variants all active)?
    pub fn row_active(&self, row: &InspectorRow, e: &MirrorEntity) -> bool {
        match &row.variant_of {
            None => true,
            Some((enum_path, v)) => {
                self.variant_on(e, enum_path).as_deref() == Some(v)
                    && self
                        .rows
                        .iter()
                        .find(|r| &r.path == enum_path)
                        .is_none_or(|r| self.row_active(r, e))
            }
        }
    }
    /// The commands adding this component to `entity` (every default).
    pub fn add_commands(&self, entity: EntityKey) -> Vec<EditorCommand> {
        self.defaults
            .iter()
            .map(|(p, v)| EditorCommand::SetProperty {
                entity,
                path: self.prop(p),
                value: v.clone(),
            })
            .collect()
    }
    /// The commands removing this component from `entity` (every property under its key).
    pub fn remove_commands(&self, entity: EntityKey, e: &MirrorEntity) -> Vec<EditorCommand> {
        let p = self.prefix();
        e.properties
            .range(p.clone()..)
            .take_while(|(k, _)| k.starts_with(&p))
            .map(|(k, _)| EditorCommand::RemoveProperty {
                entity,
                path: k.clone(),
            })
            .collect()
    }
    /// The commands switching the enum at `enum_path` to `variant` on `entity`: drop the
    /// old variant's fields, set the name, set the new variant's field defaults.
    pub fn variant_commands(
        &self,
        entity: EntityKey,
        e: &MirrorEntity,
        enum_path: &str,
        variant: &str,
    ) -> Vec<EditorCommand> {
        let mut out = Vec::new();
        let inner = format!("{enum_path}.");
        for r in self.rows.iter().filter(|r| {
            r.variant_of
                .as_ref()
                .is_some_and(|(p, v)| p == enum_path && *v != variant)
        }) {
            for s in r.storage() {
                let prop = self.prop(&s);
                if e.properties.contains_key(&prop) {
                    out.push(EditorCommand::RemoveProperty { entity, path: prop });
                }
            }
        }
        let _ = inner;
        out.push(EditorCommand::SetProperty {
            entity,
            path: self.prop(enum_path),
            value: Value::Text(variant.to_string()),
        });
        if let Some(defs) = self
            .variant_defaults
            .get(&(enum_path.to_string(), variant.to_string()))
        {
            for (p, v) in defs {
                out.push(EditorCommand::SetProperty {
                    entity,
                    path: self.prop(p),
                    value: v.clone(),
                });
            }
        }
        out
    }
}

/// The component types the inspector knows (see the module docs).
pub struct ComponentCatalog {
    reg: ForgeRegistry,
    comps: BTreeMap<String, ComponentDef>,
}

impl std::fmt::Debug for ComponentCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentCatalog")
            .field("components", &self.comps.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Default for ComponentCatalog {
    fn default() -> Self {
        Self::empty()
    }
}

fn reflect_err(e: impl std::fmt::Display) -> EditorError {
    EditorError::Settings(e.to_string())
}

impl ComponentCatalog {
    /// No components.
    pub fn empty() -> Self {
        Self {
            reg: ForgeRegistry::new(),
            comps: BTreeMap::new(),
        }
    }

    /// The editor's built-in components ([`Transform`], [`Light`], [`Tag`]).
    pub fn builtin() -> Result<Self, EditorError> {
        let mut c = Self::empty();
        c.register_type::<LightKind>()?;
        c.register::<Transform>("transform")?;
        c.register::<Light>("light")?;
        c.register::<Tag>("tag")?;
        Ok(c)
    }

    /// The editor's components: [`Self::builtin`] and the 2D pipeline's
    /// ([`Self::register_2d`]) — what every editor registers, under every preset.
    pub fn editor() -> Result<Self, EditorError> {
        let mut c = Self::builtin()?;
        c.register_2d()?;
        Ok(c)
    }

    /// The 2D pipeline's components (Ch.35 §35.2 "2D-specific inspector"): sprite, body,
    /// collider, light, camera, parallax, particles, tile map and cutout rig, each generated
    /// from its `#[forge_api]` type in `forge_2d::components`. Registered under every preset
    /// (I15); the 2D preset's layout is what puts them in front.
    pub fn register_2d(&mut self) -> Result<(), EditorError> {
        use forge_2d::components as c2;
        self.register_type::<c2::BodyKind2d>()?;
        self.register_type::<c2::Shape2d>()?;
        let [
            sprite,
            body,
            collider,
            light,
            camera,
            parallax,
            particles,
            tilemap,
            skeleton,
        ] = c2::KEYS;
        self.register::<c2::Sprite2d>(sprite)?;
        self.register::<c2::Body2d>(body)?;
        self.register::<c2::Collider2d>(collider)?;
        self.register::<c2::Light2dComponent>(light)?;
        self.register::<c2::Camera2dComponent>(camera)?;
        self.register::<c2::Parallax2d>(parallax)?;
        self.register::<c2::Particles2d>(particles)?;
        self.register::<c2::Tilemap2d>(tilemap)?;
        self.register::<c2::Skeleton2dComponent>(skeleton)?;
        Ok(())
    }

    /// Register a nested type (a struct or enum used by a component's fields) so its rows
    /// and variants resolve.
    pub fn register_type<T: ForgeApi>(&mut self) -> Result<(), EditorError> {
        self.reg.register::<T>().map_err(reflect_err)?;
        Ok(())
    }

    /// Register component `T` under `key` (its property prefix). Its defaults are read
    /// through reflection from `T::default()`.
    pub fn register<T>(&mut self, key: &str) -> Result<&ComponentDef, EditorError>
    where
        T: ForgeApi + Default + PartialReflect + TypePath,
    {
        forge_cmd::check_path(key).map_err(|e| EditorError::Settings(e.to_string()))?;
        self.reg.register::<T>().map_err(reflect_err)?;
        let dflt = T::default();
        let desc = self
            .reg
            .inspector(T::type_path())
            .ok_or_else(|| EditorError::Settings(format!("{} is not a struct", T::type_path())))?;
        let mut w = Walk {
            reg: &self.reg,
            rows: Vec::new(),
            defaults: Vec::new(),
            variant_defaults: BTreeMap::new(),
            showing: true,
        };
        w.fields(
            T::type_path(),
            Some(dflt.as_partial_reflect()),
            "",
            0,
            None,
            true,
        )?;
        let def = ComponentDef {
            key: key.to_string(),
            type_path: T::type_path().to_string(),
            title: desc.title,
            doc: desc.doc,
            rows: w.rows,
            defaults: w.defaults,
            variant_defaults: w.variant_defaults,
        };
        self.comps.insert(key.to_string(), def);
        self.comps
            .get(key)
            .ok_or_else(|| EditorError::Settings(key.to_string()))
    }

    /// The registry the components are registered in (the API schema a protocol server serves).
    pub fn registry(&self) -> &ForgeRegistry {
        &self.reg
    }

    /// Every component, by key.
    pub fn components(&self) -> impl Iterator<Item = &ComponentDef> {
        self.comps.values()
    }
    pub fn get(&self, key: &str) -> Option<&ComponentDef> {
        self.comps.get(key)
    }
    /// The components `e` carries, in key order.
    pub fn components_of<'a>(
        &'a self,
        e: &'a MirrorEntity,
    ) -> impl Iterator<Item = &'a ComponentDef> {
        self.comps.values().filter(move |c| c.present_on(e))
    }
    /// Properties of `e` no registered component claims (shown raw).
    pub fn unclaimed<'a>(
        &'a self,
        e: &'a MirrorEntity,
    ) -> impl Iterator<Item = (&'a String, &'a Value)> {
        e.properties.iter().filter(move |(k, _)| {
            !self.comps.keys().any(|c| {
                k.len() > c.len() && k.starts_with(c.as_str()) && k.as_bytes()[c.len()] == b'.'
            })
        })
    }
}

struct Walk<'r> {
    reg: &'r ForgeRegistry,
    rows: Vec<InspectorRow>,
    defaults: Vec<(String, Value)>,
    variant_defaults: BTreeMap<(String, String), Vec<(String, Value)>>,
    /// False inside a `hidden` field: its values are stored, it gets no rows.
    showing: bool,
}

/// An inspector field for a pin (what `ForgeRegistry::inspector` builds for struct fields,
/// here also for enum variant fields).
fn field_of(reg: &ForgeRegistry, p: &Pin) -> InspectorField {
    let variants = match (p.kind, reg.item(p.type_path)) {
        (PinKind::Enum, Ok(e)) => e.node.variants.iter().map(|v| v.name).collect(),
        _ => Vec::new(),
    };
    InspectorField {
        name: p.name,
        label: p.meta.display_name,
        tooltip: p.meta.doc,
        category: p.meta.category,
        kind: p.kind,
        type_path: p.type_path,
        units: p.meta.units,
        min: p.meta.min,
        max: p.meta.max,
        step: p.meta.step,
        read_only: p.meta.read_only,
        widget: p.meta.widget,
        entity: p.meta.entity,
        variants,
    }
}

impl Walk<'_> {
    fn fields(
        &mut self,
        type_path: &str,
        value: Option<&dyn PartialReflect>,
        prefix: &str,
        depth: usize,
        variant_of: Option<(String, &'static str)>,
        into_defaults: bool,
    ) -> Result<(), EditorError> {
        let not_struct = || {
            EditorError::Settings(format!(
                "{type_path} is not a registered #[forge_api] struct (register it with register_type)"
            ))
        };
        // Every field, hidden ones included (a hidden field is stored, just not shown), in
        // the inspector's order: uncategorised first, then categories in first-seen order.
        let pins = match self.reg.item(type_path) {
            Ok(i) if i.kind == forge_reflect::ItemKind::Struct => i.node.inputs.clone(),
            _ => return Err(not_struct()),
        };
        let mut cats: Vec<Option<&'static str>> = vec![None];
        for p in &pins {
            if !cats.contains(&p.meta.category) {
                cats.push(p.meta.category);
            }
        }
        for c in cats {
            for p in pins.iter().filter(|p| p.meta.category == c) {
                let f = field_of(self.reg, p);
                let fv = value.and_then(|v| match v.reflect_ref() {
                    ReflectRef::Struct(s) => s.field(f.name),
                    _ => None,
                });
                let was = self.showing;
                self.showing = was && !p.meta.hidden;
                let r = self.field(
                    f,
                    fv,
                    prefix,
                    depth,
                    variant_of.clone(),
                    into_defaults,
                    None,
                );
                self.showing = was;
                r?;
            }
        }
        Ok(())
    }

    fn push_row(&mut self, row: InspectorRow) {
        if self.showing {
            self.rows.push(row);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn field(
        &mut self,
        f: InspectorField,
        fv: Option<&dyn PartialReflect>,
        prefix: &str,
        depth: usize,
        variant_of: Option<(String, &'static str)>,
        into_defaults: bool,
        variant_sink: Option<&(String, String)>,
    ) -> Result<(), EditorError> {
        let path = format!("{prefix}{}", f.name);
        let row = InspectorRow {
            path: path.clone(),
            field: f.clone(),
            depth,
            variant_of: variant_of.clone(),
        };
        match f.kind {
            PinKind::Struct => {
                self.push_row(row);
                self.fields(
                    f.type_path,
                    fv,
                    &format!("{path}."),
                    depth + 1,
                    variant_of,
                    into_defaults,
                )?;
            }
            PinKind::Enum => {
                let item = self.reg.item(f.type_path).map_err(|_| {
                    EditorError::Settings(format!(
                        "{}: enum {} is not registered (register it with register_type)",
                        path, f.type_path
                    ))
                })?;
                let variants = item.node.variants.clone();
                let (active, ev) = match fv.map(PartialReflect::reflect_ref) {
                    Some(ReflectRef::Enum(e)) => (Some(e.variant_name().to_string()), Some(e)),
                    _ => (None, None),
                };
                let active = active
                    .or_else(|| variants.first().map(|v| v.name.to_string()))
                    .unwrap_or_default();
                self.push_default(
                    &path,
                    Value::Text(active.clone()),
                    into_defaults,
                    variant_sink,
                );
                self.push_row(row);
                for v in &variants {
                    let key = (path.clone(), v.name.to_string());
                    let is_active = v.name == active;
                    for p in &v.fields {
                        let pf = field_of(self.reg, p);
                        let pv = if is_active {
                            ev.and_then(|e| e.field(p.name))
                        } else {
                            None
                        };
                        self.field(
                            pf,
                            pv,
                            &format!("{path}."),
                            depth + 1,
                            Some((path.clone(), v.name)),
                            into_defaults && is_active,
                            Some(&key),
                        )?;
                    }
                    self.variant_defaults.entry(key).or_default();
                }
            }
            _ => {
                for (suffix, v) in leaf_values(&f, fv) {
                    let p = if suffix.is_empty() {
                        path.clone()
                    } else {
                        format!("{path}.{suffix}")
                    };
                    self.push_default(&p, v, into_defaults, variant_sink);
                }
                self.push_row(row);
            }
        }
        Ok(())
    }

    fn push_default(
        &mut self,
        path: &str,
        v: Value,
        into_defaults: bool,
        variant_sink: Option<&(String, String)>,
    ) {
        if let Some(k) = variant_sink {
            self.variant_defaults
                .entry(k.clone())
                .or_default()
                .push((path.to_string(), v.clone()));
        }
        if into_defaults {
            self.defaults.push((path.to_string(), v));
        }
    }
}

/// A leaf field's stored values (`("", v)`, or `("frame", ..), ("local", ..)` for a
/// vector), from its reflected default — or the zero value of its kind within its bounds.
fn leaf_values(f: &InspectorField, fv: Option<&dyn PartialReflect>) -> Vec<(&'static str, Value)> {
    let clamp = |x: f64| {
        let x = f.min.map_or(x, |m| x.max(m));
        f.max.map_or(x, |m| x.min(m))
    };
    match f.kind {
        PinKind::Bool => vec![(
            "",
            Value::Bool(
                fv.and_then(|v| v.try_downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
            ),
        )],
        PinKind::Int => vec![(
            "",
            Value::Int(fv.and_then(int_of).unwrap_or(clamp(0.0) as i64)),
        )],
        PinKind::Float => vec![(
            "",
            Value::Float(
                fv.and_then(|v| v.try_downcast_ref::<f64>())
                    .copied()
                    .unwrap_or(clamp(0.0)),
            ),
        )],
        PinKind::Text => vec![(
            "",
            Value::Text(
                fv.and_then(|v| v.try_downcast_ref::<String>())
                    .cloned()
                    .unwrap_or_default(),
            ),
        )],
        // No reference until the user picks one (an unset property reads "None").
        PinKind::Entity => Vec::new(),
        PinKind::Id => {
            let id = fv
                .and_then(|v| v.try_downcast_ref::<forge_frames::FrameId>().map(|i| i.0))
                .unwrap_or(0);
            vec![("", Value::Int(i64::from(id)))]
        }
        PinKind::Time => vec![(
            "",
            Value::Int(
                fv.and_then(|v| v.try_downcast_ref::<forge_frames::Tick>())
                    .map_or(0, |t| t.0),
            ),
        )],
        PinKind::Vector => {
            let (frame, l) = fv
                .and_then(|v| {
                    v.try_downcast_ref::<forge_frames::FramePos>()
                        .map(|p| (p.frame.0, [p.local.x, p.local.y, p.local.z]))
                        .or_else(|| {
                            v.try_downcast_ref::<forge_frames::FrameVel>()
                                .map(|p| (p.frame.0, [p.local.x, p.local.y, p.local.z]))
                        })
                })
                .unwrap_or((0, [0.0; 3]));
            vec![
                ("frame", Value::Int(i64::from(frame))),
                ("local", Value::Vec3(l)),
            ]
        }
        PinKind::Struct | PinKind::Enum => Vec::new(),
    }
}

fn int_of(v: &dyn PartialReflect) -> Option<i64> {
    macro_rules! try_int {
        ($($t:ty),*) => {$(
            if let Some(x) = v.try_downcast_ref::<$t>() {
                return i64::try_from(*x).ok();
            }
        )*};
    }
    // A u64 is stored as its bit pattern (see `int_value`): every u64 fits.
    if let Some(x) = v.try_downcast_ref::<u64>() {
        return Some(*x as i64);
    }
    try_int!(i64, i32, i16, i8, u32, u16, u8);
    None
}

// ---- multi-object state and edits -------------------------------------------------------

/// A row's value across a selection.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldState {
    /// Per storage path: the first entity's value (its default when unset; `None` for an
    /// unset entity reference).
    pub values: Vec<Option<Value>>,
    /// The selection disagrees.
    pub mixed: bool,
    /// How many selected entities carry the component.
    pub carriers: usize,
}

impl FieldState {
    pub fn first(&self) -> Option<&Value> {
        self.values.first().and_then(Option::as_ref)
    }
}

/// Read `row` of `comp` across `entities` (those carrying the component).
pub fn field_state(
    mirror: &ProjectMirror,
    entities: &[EntityKey],
    comp: &ComponentDef,
    row: &InspectorRow,
) -> FieldState {
    let storage = row.storage();
    let mut values: Vec<Option<Value>> = Vec::new();
    let mut mixed = false;
    let mut carriers = 0;
    for e in entities {
        let Some(me) = mirror.entity(*e) else {
            continue;
        };
        if !comp.present_on(me) {
            continue;
        }
        carriers += 1;
        let vals: Vec<Option<Value>> = storage
            .iter()
            .map(|s| {
                me.properties
                    .get(&comp.prop(s))
                    .or_else(|| comp.default_of(s))
                    .cloned()
            })
            .collect();
        if carriers == 1 {
            values = vals;
        } else if vals != values {
            mixed = true;
        }
    }
    if values.is_empty() {
        values = storage
            .iter()
            .map(|s| comp.default_of(s).cloned())
            .collect();
    }
    FieldState {
        values,
        mixed,
        carriers,
    }
}

/// The commands setting storage path `storage` (relative) of `comp` to `value` on every
/// selected entity that carries it — plus "touch to promote" for generated entities.
pub fn set_commands(
    mirror: &ProjectMirror,
    entities: &[EntityKey],
    comp: &ComponentDef,
    storage: &str,
    value: &Value,
) -> Vec<EditorCommand> {
    let mut out = Vec::new();
    for e in entities {
        let Some(me) = mirror.entity(*e) else {
            continue;
        };
        if !comp.present_on(me) {
            continue;
        }
        out.push(EditorCommand::SetProperty {
            entity: *e,
            path: comp.prop(storage),
            value: value.clone(),
        });
        out.extend(promote_command(*e, me));
    }
    out
}

/// "Touch to promote": the command marking a generated, not yet promoted entity as
/// authored (`None` for anything else).
pub fn promote_command(entity: EntityKey, me: &MirrorEntity) -> Option<EditorCommand> {
    (me.properties.contains_key(GEN_SEED_PATH)
        && me.properties.get(GEN_PROMOTED) != Some(&Value::Bool(true)))
    .then(|| EditorCommand::SetProperty {
        entity,
        path: GEN_PROMOTED.into(),
        value: Value::Bool(true),
    })
}

// ---- the InspectorWidget extension point ---------------------------------------------

/// What an `InspectorWidget` build function gets (Ch.21 §21.19). It describes the row (or
/// the whole component) being edited and the selection; the widget queues live build
/// steps (they run in the inspector with a [`crate::panel_rt::PanelBuilder`], so they can
/// register handlers and sync steps and emit commands like any panel).
pub struct InspectorCx {
    /// The component's key and type path.
    pub component: String,
    pub type_path: String,
    /// The row being edited; `None` when the widget replaces the whole component body.
    pub row: Option<InspectorRow>,
    /// The selected entities carrying the component.
    pub entities: Vec<EntityKey>,
    steps: Vec<LiveStep>,
}

impl InspectorCx {
    pub fn new(
        component: &str,
        type_path: &str,
        row: Option<InspectorRow>,
        entities: Vec<EntityKey>,
    ) -> Self {
        Self {
            component: component.to_string(),
            type_path: type_path.to_string(),
            row,
            entities,
            steps: Vec::new(),
        }
    }
    /// Queue a build step (see the type docs).
    pub fn add_live(
        &mut self,
        step: impl FnOnce(&mut crate::panel_rt::PanelBuilder) -> Result<(), forge_ui::UiError> + 'static,
    ) {
        self.steps.push(Box::new(step));
    }
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }
    /// The queued steps (the inspector runs them).
    pub fn take_steps(&mut self) -> Vec<LiveStep> {
        std::mem::take(&mut self.steps)
    }
}

/// The inspector-widget registry the editor defines (`forge.editor.inspector_widget`).
pub type InspectorWidgets = Registry<InspectorWidget<InspectorCx>>;

/// The custom widget for a whole component (`WidgetTarget::Type(component type path)`).
pub fn component_widget<'a>(
    reg: &'a InspectorWidgets,
    comp: &ComponentDef,
) -> Option<&'a InspectorWidgetDescriptor<InspectorCx>> {
    reg.iter()
        .map(|(_, d)| d)
        .find(|d| d.applies_to == WidgetTarget::Type(comp.type_path.clone()))
}

/// The custom widget for a row: by its `widget = "..."` hint first, then by its type.
pub fn row_widget<'a>(
    reg: &'a InspectorWidgets,
    row: &InspectorRow,
) -> Option<&'a InspectorWidgetDescriptor<InspectorCx>> {
    let by_hint = row.field.widget.and_then(|w| {
        reg.iter()
            .map(|(_, d)| d)
            .find(|d| d.applies_to == WidgetTarget::Attribute(w.to_string()))
    });
    by_hint.or_else(|| {
        reg.iter()
            .map(|(_, d)| d)
            .find(|d| d.applies_to == WidgetTarget::Type(row.field.type_path.to_string()))
    })
}

// ---- the built-in components ----------------------------------------------------------

/// Where an entity is, how it is turned and how big it is.
#[forge_api(name = "Transform", category = "Scene")]
#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    /// Position: a frame and the offset from its origin (I1).
    #[forge(units = "m")]
    pub position: forge_frames::FramePos,
    /// Turn about the up axis.
    #[forge(units = "deg", range = -180.0..=180.0, step = 0.5, widget = "angle", category = "Rotation")]
    pub yaw: f64,
    /// Tilt up or down.
    #[forge(units = "deg", range = -90.0..=90.0, step = 0.5, widget = "angle", category = "Rotation")]
    pub pitch: f64,
    /// Roll about the forward axis.
    #[forge(units = "deg", range = -180.0..=180.0, step = 0.5, widget = "angle", category = "Rotation")]
    pub roll: f64,
    /// Uniform scale.
    #[forge(min = 0.001, max = 1000.0, step = 0.01)]
    pub scale: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: forge_frames::FramePos::origin_of(forge_frames::FrameId(0)),
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            scale: 1.0,
        }
    }
}

/// How a light spreads.
#[forge_api]
#[derive(Clone, Debug, PartialEq)]
pub enum LightKind {
    /// Shines in every direction from a point.
    Point,
    /// A cone from a point.
    Spot {
        /// Full opening angle of the cone.
        #[forge(units = "deg", range = 1.0..=179.0, step = 1.0)]
        cone: f64,
    },
    /// Parallel rays from far away (a sun).
    Directional,
}

/// A light source.
#[forge_api(name = "Light", category = "Rendering")]
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    /// How it spreads.
    pub kind: LightKind,
    /// Emitted power.
    #[forge(units = "W", range = 0.0..=100000.0, step = 1.0, widget = "slider")]
    pub power: f64,
    /// How far it reaches.
    #[forge(units = "m", min = 0.0, step = 0.1)]
    pub range: f64,
    /// Is it on?
    pub enabled: bool,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            kind: LightKind::Point,
            power: 800.0,
            range: 10.0,
            enabled: true,
        }
    }
}

/// A label and a layer for finding and grouping entities.
#[forge_api(name = "Tag", category = "Scene")]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Tag {
    /// A free-text label.
    pub label: String,
    /// Sorting / filtering layer.
    #[forge(range = -10..=10, step = 1)]
    pub layer: i32,
}

/// The editor's `#[forge_api]` registry: the built-in components (3D and 2D) and the
/// settings types, every type and function a client of the editor's API surface reads (the
/// graph editor's library, a protocol server's schema, the I15 guard). Built fresh; nothing
/// in it is project state.
pub fn editor_api_registry() -> Result<ForgeRegistry, EditorError> {
    use crate::settings::{
        EditorSettings, EditorTheme, GpuModeSetting, ProjectEditorSettings,
        ProjectGraphicsSettings, UpAxis,
    };
    use forge_2d::components as c2;
    let mut r = ForgeRegistry::new();
    let reflect = |e: forge_reflect::ReflectError| EditorError::Settings(e.to_string());
    r.register::<Transform>().map_err(reflect)?;
    r.register::<LightKind>().map_err(reflect)?;
    r.register::<Light>().map_err(reflect)?;
    r.register::<Tag>().map_err(reflect)?;
    r.register::<EditorTheme>().map_err(reflect)?;
    r.register::<GpuModeSetting>().map_err(reflect)?;
    r.register::<EditorSettings>().map_err(reflect)?;
    r.register::<UpAxis>().map_err(reflect)?;
    r.register::<ProjectEditorSettings>().map_err(reflect)?;
    r.register::<ProjectGraphicsSettings>().map_err(reflect)?;
    // The 2D components (WP-U15), read like the 3D ones.
    r.register::<c2::BodyKind2d>().map_err(reflect)?;
    r.register::<c2::Shape2d>().map_err(reflect)?;
    r.register::<c2::Sprite2d>().map_err(reflect)?;
    r.register::<c2::Body2d>().map_err(reflect)?;
    r.register::<c2::Collider2d>().map_err(reflect)?;
    r.register::<c2::Light2dComponent>().map_err(reflect)?;
    r.register::<c2::Camera2dComponent>().map_err(reflect)?;
    r.register::<c2::Parallax2d>().map_err(reflect)?;
    r.register::<c2::Particles2d>().map_err(reflect)?;
    r.register::<c2::Tilemap2d>().map_err(reflect)?;
    r.register::<c2::Skeleton2dComponent>().map_err(reflect)?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_components_are_generated_from_reflection() {
        let c = ComponentCatalog::builtin().unwrap_or_else(|e| panic!("{e}"));
        let t = c.get("transform").unwrap_or_else(|| panic!("transform"));
        let paths: Vec<&str> = t.rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["position", "scale", "yaw", "pitch", "roll"]);
        assert_eq!(t.rows[0].editor(), EditorKind::FramePos);
        assert_eq!(t.rows[2].label(), "Yaw (deg)");
        assert_eq!(t.rows[2].field.min, Some(-180.0));
        assert_eq!(t.rows[2].field.category, Some("Rotation"));
        assert_eq!(t.default_of("scale"), Some(&Value::Float(1.0)));
        assert_eq!(t.default_of("position.frame"), Some(&Value::Int(0)));
        assert_eq!(t.default_of("position.local"), Some(&Value::Vec3([0.0; 3])));
        let l = c.get("light").unwrap_or_else(|| panic!("light"));
        let kind = l
            .rows
            .iter()
            .find(|r| r.path == "kind")
            .unwrap_or_else(|| panic!("kind"));
        assert_eq!(kind.field.variants, vec!["Point", "Spot", "Directional"]);
        let cone = l
            .rows
            .iter()
            .find(|r| r.path == "kind.cone")
            .unwrap_or_else(|| panic!("cone"));
        assert_eq!(cone.variant_of, Some(("kind".to_string(), "Spot")));
        assert_eq!(l.default_of("kind"), Some(&Value::Text("Point".into())));
        assert!(
            !l.defaults.iter().any(|(p, _)| p == "kind.cone"),
            "an inactive variant's fields are not added"
        );
        assert_eq!(
            l.rows
                .iter()
                .find(|r| r.path == "power")
                .map(|r| r.editor()),
            Some(EditorKind::Slider)
        );
    }
}

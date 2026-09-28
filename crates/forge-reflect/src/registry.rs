//! `ForgeRegistry` — every registered `#[forge_api]` item, the `bevy_reflect` type registry
//! behind them, the schema document (`schema://`, Ch.22) and inspector descriptors (WP-U6).

use std::collections::BTreeMap;

use bevy_reflect::TypeRegistry;
use serde_json::{Map, Value, json};

use crate::desc::{ApiItem, ForgeApi, ItemKind, Pin};
use crate::{PinKind, ReflectError, SchemaDefs, check_agreement};

/// The registry. Registration **validates** each item (annotations fit their pin kinds) and
/// **proves its four outputs agree** before accepting it, so nothing drifted is ever served.
pub struct ForgeRegistry {
    types: TypeRegistry,
    items: BTreeMap<String, ApiItem>,
    defs: SchemaDefs,
}

impl Default for ForgeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ForgeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForgeRegistry")
            .field("items", &self.items.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// One inspector row, resolved for a UI (WP-U6 generates editors from these; nothing here
/// knows about widgets beyond the `widget` hint).
#[derive(Clone, Debug, PartialEq)]
pub struct InspectorField {
    /// Field name (the reflect path segment the edit command targets).
    pub name: &'static str,
    /// Label.
    pub label: &'static str,
    /// Tooltip (the field's doc comment).
    pub tooltip: &'static str,
    /// Group.
    pub category: Option<&'static str>,
    /// Editor class.
    pub kind: PinKind,
    /// Type path (nested `#[forge_api]` structs expand through [`ForgeRegistry::inspector`]).
    pub type_path: &'static str,
    /// Units, as written, for the suffix.
    pub units: Option<&'static str>,
    /// Inclusive bounds and increment.
    pub min: Option<f64>,
    /// Inclusive upper bound.
    pub max: Option<f64>,
    /// Drag/spin increment.
    pub step: Option<f64>,
    /// Display only.
    pub read_only: bool,
    /// Editor hint.
    pub widget: Option<&'static str>,
    /// Entity reference.
    pub entity: bool,
    /// For an enum-typed field: the variant names, in order.
    pub variants: Vec<&'static str>,
}

/// What the inspector shows for one type: visible fields, grouped by category in first-seen
/// order (fields with no category first). `hidden` fields are omitted.
#[derive(Clone, Debug, PartialEq)]
pub struct InspectorDesc {
    /// Type path.
    pub type_path: String,
    /// Title.
    pub title: &'static str,
    /// Tooltip.
    pub doc: &'static str,
    /// `(category, fields)` — `None` is the uncategorised group, always first.
    pub groups: Vec<(Option<&'static str>, Vec<InspectorField>)>,
}

impl ForgeRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            types: TypeRegistry::new(),
            items: BTreeMap::new(),
            defs: SchemaDefs::default(),
        }
    }

    /// Register `T` (a `#[forge_api]` struct or enum, or a `#[forge_api]` fn by its name).
    /// Idempotent. Refused with the item unregistered if its annotations are invalid
    /// (`REFLECT-0001..0003`), its outputs drift (`REFLECT-0006`) or a different item already
    /// holds its path (`REFLECT-0004`).
    pub fn register<T: ForgeApi>(&mut self) -> Result<&ApiItem, ReflectError> {
        self.insert(T::describe())
    }

    /// Register an already-built item (what [`ForgeRegistry::register`] does after
    /// `describe`). Exposed so tools and tests can offer hand-built items to the same checks.
    pub fn insert(&mut self, item: ApiItem) -> Result<&ApiItem, ReflectError> {
        item.validate()?;
        check_agreement(&item)?;
        if let Some(old) = self.items.get(&item.path)
            && (old.kind != item.kind || old.node != item.node || old.schema != item.schema)
        {
            return Err(ReflectError::DuplicateItem { path: item.path });
        }
        for t in &item.types {
            (t.register)(&mut self.types);
        }
        self.defs.extend(item.schema_defs.clone());
        if item.kind != ItemKind::Fn {
            // A struct / enum fragment *is* its definition.
            self.defs.insert(item.path.clone(), item.schema.clone());
        }
        let path = item.path.clone();
        Ok(self.items.entry(path).or_insert(item))
    }

    /// The item at `path` (`REFLECT-0005` if none).
    pub fn item(&self, path: &str) -> Result<&ApiItem, ReflectError> {
        self.items
            .get(path)
            .ok_or_else(|| ReflectError::UnknownItem {
                path: path.to_string(),
            })
    }

    /// Every item, by path.
    pub fn items(&self) -> impl ExactSizeIterator<Item = &ApiItem> {
        self.items.values()
    }

    /// The `bevy_reflect` registry holding every type any item uses — what the serialiser
    /// and reflection-driven tools read.
    #[must_use]
    pub fn type_registry(&self) -> &TypeRegistry {
        &self.types
    }

    /// The whole registry as one JSON Schema document (draft 2020-12): every named type under
    /// `$defs`, every fn under `x-forge-functions`, every struct/enum under `x-forge-types`
    /// (as `$ref`s), keys in path order. This is what an API schema reader (a protocol server)
    /// serves.
    #[must_use]
    pub fn schema_document(&self) -> Value {
        let mut functions = Map::new();
        let mut types = Map::new();
        for (path, item) in &self.items {
            match item.kind {
                ItemKind::Fn => {
                    functions.insert(path.clone(), item.schema.clone());
                }
                ItemKind::Struct | ItemKind::Enum => {
                    types.insert(path.clone(), SchemaDefs::ref_to(path));
                }
            }
        }
        let mut defs = Map::new();
        for (k, v) in self.defs.iter() {
            defs.insert(k.clone(), v.clone());
        }
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "schema://forge",
            "title": "Forge API",
            "description": "Every #[forge_api] item: fns (nodes, tools, commands) and types.",
            "$defs": Value::Object(defs),
            "x-forge-functions": Value::Object(functions),
            "x-forge-types": Value::Object(types),
        })
    }

    /// The inspector layout for a registered struct (`None` for an unknown path or a
    /// non-struct).
    #[must_use]
    pub fn inspector(&self, type_path: &str) -> Option<InspectorDesc> {
        let item = self.items.get(type_path)?;
        if item.kind != ItemKind::Struct {
            return None;
        }
        let mut groups: Vec<(Option<&'static str>, Vec<InspectorField>)> = vec![(None, vec![])];
        for p in item.node.inputs.iter().filter(|p| !p.meta.hidden) {
            let f = self.field(p);
            match groups.iter_mut().find(|(c, _)| *c == p.meta.category) {
                Some((_, v)) => v.push(f),
                None => groups.push((p.meta.category, vec![f])),
            }
        }
        if groups.first().is_some_and(|(_, v)| v.is_empty()) {
            groups.remove(0);
        }
        Some(InspectorDesc {
            type_path: item.path.clone(),
            title: item.node.display_name,
            doc: item.doc,
            groups,
        })
    }

    fn field(&self, p: &Pin) -> InspectorField {
        let variants = match (p.kind, self.items.get(p.type_path)) {
            (PinKind::Enum, Some(e)) => e.node.variants.iter().map(|v| v.name).collect(),
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
}

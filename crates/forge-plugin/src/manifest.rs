//! The plugin manifest (`plugin.ron`, Ch.32.4).
//!
//! ```ron
//! Plugin(
//!   id: "com.example.rivers",
//!   version: "0.3.1",
//!   engine: "^0.1",                     // semver against the kernel, checked at load
//!   kind: Wasm,                         // Wasm | Source
//!   provides: [ GeneratorNode("river_v2"), EditorPanel("rivers") ],
//!   replaces: [ GeneratorNode("forge.river") ],   // explicit; conflicts are errors
//!   capabilities: [ Fs(ProjectRead), Gpu(Compute) ],  // NOT granted by default
//! )
//! ```
//!
//! `removes: [...]` and `chains: [...]` are declared the same way. Every operation a plugin
//! performs is declared, so every conflict between plugins is found from the manifests
//! alone, before any plugin code runs.

use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;
use serde::de::{DeserializeSeed, Deserializer, EnumAccess, VariantAccess, Visitor};

use crate::id::check_key;
use crate::{Capability, PluginError, PluginId};

/// How a plugin is built and loaded (Ch.32.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
pub enum PluginKind {
    /// A WASM component: drop-in, sandboxed, capability-scoped (the host is M2-13).
    Wasm,
    /// A cargo dependency compiled into the engine: native, full trust.
    Source,
}

/// A reference to an item in a manifest: the point's manifest name and the item's key,
/// written `EditorPanel("rivers")`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ItemRef {
    /// The extension point's manifest name (`EditorPanel`).
    pub point: String,
    /// The item's key.
    pub key: String,
}

impl ItemRef {
    /// `point("key")`.
    #[must_use]
    pub fn new(point: &str, key: &str) -> Self {
        Self {
            point: point.to_string(),
            key: key.to_string(),
        }
    }
}

impl fmt::Display for ItemRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({:?})", self.point, self.key)
    }
}

/// A variant name read as an identifier (RON reads `String` as a quoted literal).
struct Ident;

impl<'de> DeserializeSeed<'de> for Ident {
    type Value = String;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<String, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = String;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an extension point name")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
                Ok(v.to_string())
            }
        }
        d.deserialize_identifier(V)
    }
}

impl<'de> Deserialize<'de> for ItemRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = ItemRef;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an item reference like EditorPanel(\"rivers\")")
            }
            fn visit_enum<A: EnumAccess<'de>>(self, a: A) -> Result<ItemRef, A::Error> {
                let (point, rest) = a.variant_seed(Ident)?;
                let key: String = rest.newtype_variant()?;
                Ok(ItemRef { point, key })
            }
        }
        // The point names are an open set (every chapter adds points), so no variant list.
        d.deserialize_enum("ItemRef", &[], V)
    }
}

#[derive(Deserialize)]
#[serde(rename = "Plugin", deny_unknown_fields)]
struct Raw {
    id: String,
    version: String,
    engine: String,
    kind: PluginKind,
    #[serde(default)]
    provides: Vec<ItemRef>,
    #[serde(default)]
    replaces: Vec<ItemRef>,
    #[serde(default)]
    removes: Vec<ItemRef>,
    #[serde(default)]
    chains: Vec<ItemRef>,
    #[serde(default)]
    capabilities: Vec<Capability>,
}

/// A validated manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    /// The plugin's id.
    pub id: PluginId,
    /// Its version.
    pub version: semver::Version,
    /// The kernel versions it works with.
    pub engine: semver::VersionReq,
    /// WASM or source.
    pub kind: PluginKind,
    /// Items it adds.
    pub provides: Vec<ItemRef>,
    /// Items it replaces.
    pub replaces: Vec<ItemRef>,
    /// Items it removes.
    pub removes: Vec<ItemRef>,
    /// Items it wraps.
    pub chains: Vec<ItemRef>,
    /// Capabilities it requests (granted only by a human, never by default).
    pub capabilities: Vec<Capability>,
}

impl Manifest {
    /// Parse and validate a `plugin.ron`.
    pub fn parse(text: &str) -> Result<Self, PluginError> {
        let raw: Raw = ron::from_str(text).map_err(|e| PluginError::Manifest {
            plugin: None,
            why: e.to_string(),
        })?;
        let bad = |why: String| PluginError::Manifest {
            plugin: Some(raw.id.clone()),
            why,
        };
        let id = PluginId::new(&raw.id)?;
        let version = semver::Version::parse(&raw.version)
            .map_err(|e| bad(format!("version {:?}: {e}", raw.version)))?;
        let engine = semver::VersionReq::parse(&raw.engine)
            .map_err(|e| bad(format!("engine {:?}: {e}", raw.engine)))?;
        let m = Manifest {
            id,
            version,
            engine,
            kind: raw.kind,
            provides: raw.provides,
            replaces: raw.replaces,
            removes: raw.removes,
            chains: raw.chains,
            capabilities: raw.capabilities,
        };
        m.validate()?;
        Ok(m)
    }

    /// A source-plugin manifest built in code (`engine` is a semver requirement).
    pub fn source(id: &str, version: &str, engine: &str) -> Result<Self, PluginError> {
        let bad = |why: String| PluginError::Manifest {
            plugin: Some(id.to_string()),
            why,
        };
        Ok(Manifest {
            id: PluginId::new(id)?,
            version: semver::Version::parse(version).map_err(|e| bad(e.to_string()))?,
            engine: semver::VersionReq::parse(engine).map_err(|e| bad(e.to_string()))?,
            kind: PluginKind::Source,
            provides: Vec::new(),
            replaces: Vec::new(),
            removes: Vec::new(),
            chains: Vec::new(),
            capabilities: Vec::new(),
        })
    }

    /// Every declared item appears once, item keys are valid, and no item is both provided
    /// and modified by the same plugin.
    pub fn validate(&self) -> Result<(), PluginError> {
        let bad = |why: String| PluginError::Manifest {
            plugin: Some(self.id.to_string()),
            why,
        };
        let mut seen: BTreeSet<(&str, &ItemRef)> = BTreeSet::new();
        for (list, name) in self.lists() {
            for r in list {
                check_key(&r.key)?;
                if r.point.is_empty() {
                    return Err(bad(format!("{r} has no extension point name")));
                }
                if !seen.insert((name, r)) {
                    return Err(bad(format!("{r} is listed twice in `{name}`")));
                }
            }
        }
        for r in &self.provides {
            for (list, name) in self.lists().into_iter().skip(1) {
                if list.contains(r) {
                    return Err(bad(format!(
                        "{r} is both in `provides` and `{name}`; a plugin modifies its own item by providing it as it wants it"
                    )));
                }
            }
        }
        for r in &self.replaces {
            if self.removes.contains(r) {
                return Err(bad(format!("{r} is both replaced and removed")));
            }
        }
        Ok(())
    }

    fn lists(&self) -> [(&[ItemRef], &'static str); 4] {
        [
            (&self.provides, "provides"),
            (&self.replaces, "replaces"),
            (&self.removes, "removes"),
            (&self.chains, "chains"),
        ]
    }

    /// Declares `r` in `provides` (builder for source plugins).
    #[must_use]
    pub fn provides(mut self, point: &str, key: &str) -> Self {
        self.provides.push(ItemRef::new(point, key));
        self
    }

    /// Declares `r` in `replaces`.
    #[must_use]
    pub fn replaces(mut self, point: &str, key: &str) -> Self {
        self.replaces.push(ItemRef::new(point, key));
        self
    }

    /// Declares `r` in `removes`.
    #[must_use]
    pub fn removes(mut self, point: &str, key: &str) -> Self {
        self.removes.push(ItemRef::new(point, key));
        self
    }

    /// Declares `r` in `chains`.
    #[must_use]
    pub fn chains(mut self, point: &str, key: &str) -> Self {
        self.chains.push(ItemRef::new(point, key));
        self
    }

    /// Requests a capability.
    #[must_use]
    pub fn requests(mut self, cap: Capability) -> Self {
        self.capabilities.push(cap);
        self
    }

    pub(crate) fn declares(&self, op: Op, r: &ItemRef) -> bool {
        match op {
            Op::Add => self.provides.contains(r),
            Op::Replace => self.replaces.contains(r),
            Op::Remove => self.removes.contains(r),
            Op::Chain => self.chains.contains(r),
        }
    }
}

/// A registry operation, in the order the loader applies them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Op {
    Add,
    Replace,
    Remove,
    Chain,
}

impl Op {
    pub(crate) fn verb(self) -> &'static str {
        match self {
            Self::Add => "provide",
            Self::Replace => "replace",
            Self::Remove => "remove",
            Self::Chain => "chain",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{FsScope, GpuUse};

    const RIVERS: &str = r#"
        Plugin(
          id: "com.example.rivers",
          version: "0.3.1",
          engine: "^0.2",
          kind: Wasm,
          provides: [ GeneratorNode("river_v2"), EditorPanel("rivers") ],
          replaces: [ GeneratorNode("forge.river") ],
          capabilities: [ Fs(ProjectRead), Gpu(Compute) ],
        )
    "#;

    #[test]
    fn the_plan_example_parses() {
        let m = Manifest::parse(RIVERS).expect("parses");
        assert_eq!(m.id.as_str(), "com.example.rivers");
        assert_eq!(m.version, semver::Version::new(0, 3, 1));
        assert!(m.engine.matches(&semver::Version::new(0, 2, 9)));
        assert!(!m.engine.matches(&semver::Version::new(0, 3, 0)));
        assert_eq!(m.kind, PluginKind::Wasm);
        assert_eq!(
            m.provides,
            [
                ItemRef::new("GeneratorNode", "river_v2"),
                ItemRef::new("EditorPanel", "rivers")
            ]
        );
        assert_eq!(m.replaces, [ItemRef::new("GeneratorNode", "forge.river")]);
        assert!(m.removes.is_empty() && m.chains.is_empty());
        assert_eq!(
            m.capabilities,
            [
                Capability::Fs(FsScope::ProjectRead),
                Capability::Gpu(GpuUse::Compute)
            ]
        );
    }

    #[test]
    fn invalid_manifests_are_refused_with_a_reason() {
        let cases = [
            (RIVERS.replace("0.3.1", "three"), "version"),
            (RIVERS.replace("^0.2", "most recent"), "engine"),
            (
                RIVERS.replace("com.example.rivers", "Rivers"),
                "PLUGIN-0001",
            ),
            (RIVERS.replace("kind: Wasm", "kind: Dylib"), "Dylib"),
            (
                RIVERS.replace("Gpu(Compute)", "Gpu(Everything)"),
                "Everything",
            ),
            (
                RIVERS.replace("engine:", "engine_version:"),
                "engine_version",
            ),
            (
                RIVERS.replace(
                    "EditorPanel(\"rivers\")",
                    "EditorPanel(\"rivers\"), EditorPanel(\"rivers\")",
                ),
                "twice",
            ),
            (
                RIVERS.replace("replaces: [", "replaces: [ EditorPanel(\"rivers\"), "),
                "both in `provides`",
            ),
            (RIVERS.replace("\"river_v2\"", "\"bad key\""), "PLUGIN-0002"),
        ];
        for (text, needle) in cases {
            let e = Manifest::parse(&text).expect_err(needle);
            let msg = e.to_string();
            assert!(msg.contains(needle), "{needle}: {msg}");
        }
    }
}

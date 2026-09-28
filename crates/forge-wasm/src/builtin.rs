//! The adapters every host has: `Command` and `Preset` (the points `forge-plugin` defines).
//!
//! ## `Command` — a plugin command on the bus (I7, I8)
//!
//! The guest **plans**; the bus applies. `call("Command", key, input)` gets
//!
//! ```json
//! { "args": <the Invoke arguments>, "chained": false }
//! ```
//!
//! and returns the edits it wants, which the host replays onto the command's `DiffBuilder`:
//!
//! ```json
//! { "ops": [
//!   { "op": "spawn", "name": "Tower", "parent": null },
//!   { "op": "set_property", "entity": "$0", "path": "height", "value": { "Float": 12.0 } },
//!   { "op": "rename", "entity": 7, "name": "Old tower" },
//!   { "op": "set_setting", "key": "rivers.enabled", "value": { "Bool": true } }
//! ] }
//! ```
//!
//! `entity` is an existing entity's key, or `"$N"`: the entity the plan's N-th `spawn`
//! created. Ops: `spawn`, `despawn`, `rename`, `reparent`, `set_property`,
//! `remove_property`, `set_setting` (`value: null` clears it). So a WASM command is
//! undoable, dry-runnable and provenance-tagged exactly like a built-in one: it never touches
//! project state itself.
//!
//! Capabilities are checked when the command is planned, against the shared grant table:
//! every plan needs `Command(Ordinary)`; a plan with `despawn`, `remove_property` or a
//! cleared setting also needs `Command(Destructive)` (Ch.22.3). A denial refuses the
//! command (`CMD-0014`) and nothing applies. A chained command runs the wrapped command's
//! plan first, then the guest's (`"chained": true`).
//!
//! ## `Preset` — a workspace preset (Ch.31)
//!
//! Read once, at install: `call("Preset", key, [])` returns RON
//! `(label: "Rivers", kind: ThreeD, defaults: {"k": "v"}, files: {})`.

use std::collections::BTreeMap;
use std::sync::Arc;

use forge_cmd::{CmdError, CommandHandler, CommandPolicy, DiffBuilder, EntityKey, Value};
use forge_plugin::points::{Command, CommandItem, Preset, PresetDescriptor, PresetKind};
use forge_plugin::{Capability, CommandClass};
use serde::Deserialize;

use crate::{GuestItem, WasmError, WasmHost};

pub(crate) fn install(host: &mut WasmHost) {
    host.adapt_chain::<Command>(
        |item| Ok(command(item, None)),
        |item, inner| command(item, Some(inner)),
    );
    host.adapt::<Preset>(preset);
}

/// An entity in a plan: an existing key, or `"$N"` (the N-th spawn of this plan).
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum EntityRef {
    Key(u64),
    New(String),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum PlanOp {
    Spawn {
        name: String,
        #[serde(default)]
        parent: Option<EntityRef>,
    },
    Despawn {
        entity: EntityRef,
    },
    Rename {
        entity: EntityRef,
        name: String,
    },
    Reparent {
        entity: EntityRef,
        #[serde(default)]
        parent: Option<EntityRef>,
    },
    SetProperty {
        entity: EntityRef,
        path: String,
        value: Value,
    },
    RemoveProperty {
        entity: EntityRef,
        path: String,
    },
    SetSetting {
        key: String,
        value: Option<Value>,
    },
}

impl PlanOp {
    fn destructive(&self) -> bool {
        matches!(
            self,
            Self::Despawn { .. }
                | Self::RemoveProperty { .. }
                | Self::SetSetting { value: None, .. }
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    ops: Vec<PlanOp>,
}

struct WasmCommand {
    item: GuestItem,
    inner: Option<Arc<dyn CommandHandler>>,
}

fn refuse(item: &GuestItem, e: &forge_plugin::PluginError) -> CmdError {
    CmdError::PolicyRefused {
        what: format!("{} (WASM command {}): {e}", item.plugin(), item.name()),
    }
}

impl WasmCommand {
    fn bad(&self, why: String) -> CmdError {
        CmdError::BadArgs {
            target: self.item.key().to_string(),
            why,
        }
    }

    fn resolve(&self, r: &EntityRef, spawned: &[EntityKey]) -> Result<EntityKey, CmdError> {
        match r {
            EntityRef::Key(k) => Ok(EntityKey(*k)),
            EntityRef::New(s) => s
                .strip_prefix('$')
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|n| spawned.get(n).copied())
                .ok_or_else(|| {
                    self.bad(format!(
                        "{}: entity {s:?} is neither a key nor \"$N\" naming one of the plan's {} spawns",
                        WasmError::BadOutput {
                            plugin: self.item.plugin().to_string(),
                            item: self.item.name(),
                            why: String::new(),
                        }
                        .code(),
                        spawned.len()
                    ))
                }),
        }
    }

    fn apply(&self, b: &mut DiffBuilder<'_>, ops: Vec<PlanOp>) -> Result<(), CmdError> {
        let mut spawned: Vec<EntityKey> = Vec::new();
        for op in ops {
            match op {
                PlanOp::Spawn { name, parent } => {
                    let parent = parent.map(|p| self.resolve(&p, &spawned)).transpose()?;
                    spawned.push(b.spawn(&name, parent)?);
                }
                PlanOp::Despawn { entity } => b.despawn(self.resolve(&entity, &spawned)?)?,
                PlanOp::Rename { entity, name } => {
                    b.rename(self.resolve(&entity, &spawned)?, &name)?;
                }
                PlanOp::Reparent { entity, parent } => {
                    let e = self.resolve(&entity, &spawned)?;
                    let parent = parent.map(|p| self.resolve(&p, &spawned)).transpose()?;
                    b.reparent(e, parent)?;
                }
                PlanOp::SetProperty {
                    entity,
                    path,
                    value,
                } => b.set_property(self.resolve(&entity, &spawned)?, &path, value)?,
                PlanOp::RemoveProperty { entity, path } => {
                    b.remove_property(self.resolve(&entity, &spawned)?, &path)?;
                }
                PlanOp::SetSetting { key, value } => b.set_setting(&key, value)?,
            }
        }
        Ok(())
    }
}

impl CommandHandler for WasmCommand {
    fn plan(&self, b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
        self.item
            .check(Capability::Command(CommandClass::Ordinary))
            .map_err(|e| refuse(&self.item, &e))?;
        if let Some(inner) = &self.inner {
            inner.plan(b, args)?;
        }
        let input = serde_json::json!({ "args": args, "chained": self.inner.is_some() });
        let input = serde_json::to_vec(&input).map_err(|e| self.bad(e.to_string()))?;
        let out = self.item.call(&input).map_err(|e| match e {
            WasmError::Trap { .. } => CmdError::Panicked {
                message: e.to_string(),
            },
            other => self.bad(other.to_string()),
        })?;
        let plan: Plan = serde_json::from_slice(&out).map_err(|e| {
            self.bad(
                WasmError::BadOutput {
                    plugin: self.item.plugin().to_string(),
                    item: self.item.name(),
                    why: e.to_string(),
                }
                .to_string(),
            )
        })?;
        if plan.ops.iter().any(PlanOp::destructive) {
            self.item
                .check(Capability::Command(CommandClass::Destructive))
                .map_err(|e| refuse(&self.item, &e))?;
        }
        self.apply(b, plan.ops)
    }
}

fn command(item: GuestItem, inner: Option<CommandItem>) -> CommandItem {
    let policy = inner.as_ref().map_or(CommandPolicy::ORDINARY, |i| i.policy);
    CommandItem {
        policy,
        handler: Arc::new(WasmCommand {
            item,
            inner: inner.map(|i| i.handler),
        }),
    }
}

#[derive(Deserialize)]
enum Kind {
    TwoD,
    ThreeD,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPreset {
    label: String,
    kind: Kind,
    #[serde(default)]
    defaults: BTreeMap<String, String>,
    #[serde(default)]
    files: BTreeMap<String, String>,
}

fn preset(item: GuestItem) -> Result<PresetDescriptor, WasmError> {
    let out = item.call(&[])?;
    let bad = |why: String| WasmError::BadOutput {
        plugin: item.plugin().to_string(),
        item: item.name(),
        why,
    };
    let text = String::from_utf8(out).map_err(|e| bad(e.to_string()))?;
    let raw: RawPreset = ron::from_str(&text).map_err(|e| bad(e.to_string()))?;
    Ok(PresetDescriptor {
        label: raw.label,
        kind: match raw.kind {
            Kind::TwoD => PresetKind::TwoD,
            Kind::ThreeD => PresetKind::ThreeD,
        },
        defaults: raw.defaults,
        files: raw.files,
    })
}

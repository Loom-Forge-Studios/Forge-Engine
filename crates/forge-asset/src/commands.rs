//! Asset commands on the bus (I7): import, remove, rename — undoable, dry-runnable,
//! provenance-tagged, and drivable by any client like every other command.
//!
//! A command records the **intent** as project state — a setting
//! `asset.import.p<hash of the source path>` whose value is the RON of an [`Intent`] (source,
//! import settings, and for a rename the path it came from). Planning never touches the
//! store, so `dry_run` is exactly `apply`. [`AssetServer::sync`] then makes the asset
//! database match the project: new intents import, vanished ones remove, changed settings
//! reimport, and a vanished/new pair linked by `renamed_from` is a rename (in either
//! direction — which is what makes undoing a rename move the file back).
//!
//! | target | args | effect |
//! |---|---|---|
//! | `forge.asset.import` | `{ "source": "models/ship.gltf", "settings": { "mips": "false" } }` | import, or change settings |
//! | `forge.asset.remove` | `{ "source": "models/ship.gltf" }` | remove the registration (the file stays) |
//! | `forge.asset.rename` | `{ "from": "models/ship.gltf", "to": "models/hull.gltf" }` | move; the id stays |
//! | `forge.asset.adopt` | `{ "assets": [{ "source": …, "settings": … }] }` | record assets already on disk (not undoable) |
//!
//! `adopt` exists because the asset database can learn of assets outside the bus (a project
//! opened with sidecars, a VCS pull): [`AssetServer::adopt_command`] builds one that records
//! them, so every later command has an intent to act on. It is not undoable — undoing it
//! would delete assets nobody asked to remove.

use std::collections::{BTreeMap, BTreeSet};

use forge_cmd::{Bus, CmdError, CommandPolicy, DiffBuilder, EditorCommand, Project, Value};
use forge_store::{Blake3, StorePath};
use serde::{Deserialize, Serialize};

use crate::sidecar::Settings;
use crate::{AssetEvent, AssetServer};

/// `Invoke` target: import a source (or change its settings).
pub const IMPORT: &str = "forge.asset.import";
/// `Invoke` target: remove a source's registration.
pub const REMOVE: &str = "forge.asset.remove";
/// `Invoke` target: rename a source.
pub const RENAME: &str = "forge.asset.rename";
/// `Invoke` target: record assets that exist outside the bus (not undoable).
pub const ADOPT: &str = "forge.asset.adopt";

const PREFIX: &str = "asset.import.";

/// The project-state record of one imported source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    /// The source path.
    pub source: String,
    /// Its import settings.
    #[serde(default)]
    pub settings: Settings,
    /// Set by a rename: the path it moved from.
    #[serde(default)]
    pub renamed_from: Option<String>,
}

/// The setting key holding `source`'s intent.
#[must_use]
pub fn intent_key(source: &str) -> String {
    format!("{PREFIX}p{}", &Blake3::of(source.as_bytes()).to_hex()[..24])
}

fn bad(target: &str, why: impl std::fmt::Display) -> CmdError {
    CmdError::BadArgs {
        target: target.to_string(),
        why: why.to_string(),
    }
}

fn path_arg(target: &str, args: &serde_json::Value, key: &str) -> Result<String, CmdError> {
    let s = args
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| bad(target, format!("missing string argument {key:?}")))?;
    StorePath::new(s).map_err(|e| bad(target, e))?;
    Ok(s.to_string())
}

fn settings_arg(target: &str, args: &serde_json::Value) -> Result<Settings, CmdError> {
    match args.get("settings") {
        None | Some(serde_json::Value::Null) => Ok(Settings::new()),
        Some(serde_json::Value::Object(m)) => m
            .iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => Ok((k.clone(), s.clone())),
                serde_json::Value::Bool(b) => Ok((k.clone(), b.to_string())),
                serde_json::Value::Number(n) => Ok((k.clone(), n.to_string())),
                _ => Err(bad(
                    target,
                    format!("setting {k:?} is not a string, bool or number"),
                )),
            })
            .collect(),
        Some(_) => Err(bad(target, "\"settings\" must be an object")),
    }
}

fn encode(i: &Intent) -> Value {
    Value::Text(ron::to_string(i).unwrap_or_default())
}

fn decode(v: &Value) -> Option<Intent> {
    match v {
        Value::Text(t) => ron::from_str(t).ok(),
        _ => None,
    }
}

fn current(b: &DiffBuilder<'_>, source: &str) -> Option<Intent> {
    b.setting(&intent_key(source)).as_ref().and_then(decode)
}

fn plan_import(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let source = path_arg(IMPORT, args, "source")?;
    let settings = settings_arg(IMPORT, args)?;
    let renamed_from = current(b, &source).and_then(|i| i.renamed_from);
    let intent = Intent {
        source: source.clone(),
        settings,
        renamed_from,
    };
    b.set_setting(&intent_key(&source), Some(encode(&intent)))
}

fn plan_remove(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let source = path_arg(REMOVE, args, "source")?;
    if current(b, &source).is_none() {
        return Err(bad(REMOVE, format!("{source} is not an imported asset")));
    }
    b.set_setting(&intent_key(&source), None)
}

fn plan_rename(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let from = path_arg(RENAME, args, "from")?;
    let to = path_arg(RENAME, args, "to")?;
    let Some(cur) = current(b, &from) else {
        return Err(bad(RENAME, format!("{from} is not an imported asset")));
    };
    if from == to {
        return Ok(());
    }
    if current(b, &to).is_some() {
        return Err(bad(RENAME, format!("{to} is already an imported asset")));
    }
    b.set_setting(&intent_key(&from), None)?;
    let moved = Intent {
        source: to.clone(),
        settings: cur.settings,
        renamed_from: Some(from),
    };
    b.set_setting(&intent_key(&to), Some(encode(&moved)))
}

fn plan_adopt(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    let list = args
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| bad(ADOPT, "missing array argument \"assets\""))?;
    for a in list {
        let source = path_arg(ADOPT, a, "source")?;
        let settings = settings_arg(ADOPT, a)?;
        b.set_setting(
            &intent_key(&source),
            Some(encode(&Intent {
                source,
                settings,
                renamed_from: None,
            })),
        )?;
    }
    if let Some(clear) = args.get("clear").and_then(serde_json::Value::as_array) {
        for c in clear {
            let s = c
                .as_str()
                .ok_or_else(|| bad(ADOPT, "\"clear\" holds non-strings"))?;
            b.set_setting(&intent_key(s), None)?;
        }
    }
    Ok(())
}

/// Register the asset commands on `bus`.
pub fn register_commands(bus: &mut Bus) -> Result<(), CmdError> {
    bus.register_handler(IMPORT, CommandPolicy::ORDINARY, plan_import)?;
    bus.register_handler(REMOVE, CommandPolicy::ORDINARY, plan_remove)?;
    bus.register_handler(RENAME, CommandPolicy::ORDINARY, plan_rename)?;
    bus.register_handler(
        ADOPT,
        CommandPolicy {
            human_only: false,
            undoable: false,
            redoable: false,
        },
        plan_adopt,
    )
}

/// An `Invoke` command for `target` with JSON `args`.
#[must_use]
pub fn invoke(target: &str, args: &serde_json::Value) -> EditorCommand {
    EditorCommand::Invoke {
        target: target.to_string(),
        args: args.to_string(),
    }
}

/// The intents recorded in a project.
#[must_use]
pub fn intents(project: &Project) -> BTreeMap<String, Intent> {
    intents_from_settings(project.settings())
}

/// The intents among project settings — from the project itself, or from a read model of
/// it (the editor's mirror, which follows the same settings through the `Applied` stream).
pub fn intents_from_settings<'a>(
    settings: impl Iterator<Item = (&'a str, &'a Value)>,
) -> BTreeMap<String, Intent> {
    settings
        .filter(|(k, _)| k.starts_with(PREFIX))
        .filter_map(|(k, v)| decode(v).map(|i| (k.to_string(), i)))
        .collect()
}

impl AssetServer {
    /// Make the asset database match the intents in `project` (call after the bus applies,
    /// undoes or redoes anything). Every failure is reported as an event; the rest proceeds.
    pub fn sync(&mut self, project: &Project) -> Vec<AssetEvent> {
        self.sync_intents(intents(project))
    }

    /// [`AssetServer::sync`] from intents already read (the editor reads them from its
    /// mirror of the project, [`intents_from_settings`]).
    pub fn sync_intents(&mut self, cur: BTreeMap<String, Intent>) -> Vec<AssetEvent> {
        let prev = std::mem::take(&mut self.intents);
        let mut events = Vec::new();
        let removed: Vec<&Intent> = prev
            .iter()
            .filter(|(k, _)| !cur.contains_key(*k))
            .map(|(_, v)| v)
            .collect();
        let added: Vec<&Intent> = cur
            .iter()
            .filter(|(k, _)| !prev.contains_key(*k))
            .map(|(_, v)| v)
            .collect();
        let mut used_r = BTreeSet::new();
        let mut used_a = BTreeSet::new();
        // Renames: a vanished and a new intent linked by `renamed_from`, either way round.
        for (ai, a) in added.iter().enumerate() {
            for (ri, r) in removed.iter().enumerate() {
                if used_r.contains(&ri) {
                    continue;
                }
                let linked = a.renamed_from.as_deref() == Some(r.source.as_str())
                    || r.renamed_from.as_deref() == Some(a.source.as_str());
                if !linked {
                    continue;
                }
                used_r.insert(ri);
                used_a.insert(ai);
                let (Ok(from), Ok(to)) = (StorePath::new(&r.source), StorePath::new(&a.source))
                else {
                    break;
                };
                if self.id_of(&from).is_some()
                    && let Err(error) = self.rename(&from, &to, &mut events)
                {
                    events.push(AssetEvent::Failed { path: from, error });
                } else if a.settings != r.settings
                    && let Err(error) = self.import(&to, a.settings.clone(), &mut events)
                {
                    events.push(AssetEvent::Failed { path: to, error });
                }
                break;
            }
        }
        for (ri, r) in removed.iter().enumerate() {
            if used_r.contains(&ri) {
                continue;
            }
            if let Ok(p) = StorePath::new(&r.source)
                && self.id_of(&p).is_some()
                && let Err(error) = self.remove(&p, &mut events)
            {
                events.push(AssetEvent::Failed { path: p, error });
            }
        }
        let changed: Vec<&Intent> = cur
            .iter()
            .filter(|(k, v)| prev.get(*k).is_some_and(|p| p.settings != v.settings))
            .map(|(_, v)| v)
            .collect();
        for (ai, a) in added.iter().enumerate() {
            if used_a.contains(&ai) {
                continue;
            }
            self.sync_import(a, &mut events);
        }
        for c in changed {
            self.sync_import(c, &mut events);
        }
        self.intents = cur;
        events
    }

    fn sync_import(&mut self, i: &Intent, events: &mut Vec<AssetEvent>) {
        let Ok(p) = StorePath::new(&i.source) else {
            return;
        };
        if let Err(error) = self.import(&p, i.settings.clone(), events) {
            events.push(AssetEvent::Failed { path: p, error });
        }
    }

    /// An `adopt` command recording every imported source the project's intents do not yet
    /// name (and clearing intents whose source the database no longer has), or `None` when
    /// they already agree.
    #[must_use]
    pub fn adopt_command(&self, project: &Project) -> Option<EditorCommand> {
        let cur = intents(project);
        let mut assets = Vec::new();
        for p in self.sources() {
            let key = intent_key(p.as_str());
            let settings = self
                .sidecar(&p)
                .map(|s| s.settings.clone())
                .unwrap_or_default();
            if cur.get(&key).map(|i| &i.settings) != Some(&settings) {
                assets.push(serde_json::json!({ "source": p.as_str(), "settings": settings }));
            }
        }
        let clear: Vec<&str> = cur
            .values()
            .filter(|i| StorePath::new(&i.source).map_or(true, |p| self.id_of(&p).is_none()))
            .map(|i| i.source.as_str())
            .collect();
        if assets.is_empty() && clear.is_empty() {
            return None;
        }
        Some(invoke(
            ADOPT,
            &serde_json::json!({ "assets": assets, "clear": clear }),
        ))
    }
}

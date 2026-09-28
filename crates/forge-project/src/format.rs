//! The project's files (Ch.33 §33.2 "project data": RON text, diffable, mergeable) and the
//! command that loads them into the core.
//!
//! | File | What |
//! |---|---|
//! | `forge-project.ron` | the [`Manifest`]: format version, name, preset, template, engine — what the launcher reads to list a project without loading it |
//! | `project/settings.ron` | every project setting, `key: Value`, sorted |
//! | `project/scene.ron` | every entity: its id in this file, name, parent, properties, sorted by id — except that a scene instance is stored as a reference: its root, its override records and the ids of its other nodes (`forge_scene::store`, WP-U20); decoding derives the rest, so a [`ProjectDoc`] in memory always holds every entity |
//!
//! Generated content is not stored (it has a `SeedPath`, Ch.33 §33.2) and assets are the
//! asset system's blobs; these three files are the reflect-tree part of a project.
//!
//! **Loading is a command.** [`ProjectDoc::load_command`] is `forge.project.load`
//! ([`LOAD_CMD`]), planned by [`plan_load`] on a `DiffBuilder` like any handler: it clears
//! the project and rebuilds it from the document in **one** diff — atomic (a document the
//! project refuses changes nothing), audited, and seen by every client through the
//! `Applied` stream, so a mirror follows an open or a pull exactly as it follows an edit.
//! Entity ids in a file are that file's own; the core allocates fresh keys and remaps
//! entity references (`Value::Entity`) through them.

use std::collections::{BTreeMap, BTreeSet};

use forge_cmd::{CmdError, DiffBuilder, EditorCommand, EntityKey, Project, Value};
use serde::{Deserialize, Serialize};

use crate::{NAME_SETTING, PRESET_SETTING, ProjectError, TEMPLATE_SETTING};

/// The manifest's path.
pub const MANIFEST_PATH: &str = "forge-project.ron";
/// The settings file's path.
pub const SETTINGS_PATH: &str = "project/settings.ron";
/// The scene file's path.
pub const SCENE_PATH: &str = "project/scene.ron";
/// The files format this build writes and reads.
pub const FORMAT_VERSION: u32 = 1;
/// The engine version written into manifests (and the attribution, Ch.38 §38.7).
pub const ENGINE_VERSION: &str = "0.1.0";
/// The project-load command (`Invoke` target) the core applies on open and pull.
pub const LOAD_CMD: &str = "forge.project.load";

/// `forge-project.ron`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub name: String,
    /// The preset directory (`2d`, `3d`, or a plugin preset's).
    pub preset: String,
    /// The template the project was made from (`3d`, `preset:<key>`).
    pub template: String,
    /// The engine version that last wrote it.
    pub engine: String,
}

/// One entity in `project/scene.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityDoc {
    /// Its id in this file (references use it).
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub parent: Option<u64>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
}

/// A whole project as files hold it (settings and entities).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectDoc {
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    #[serde(default)]
    pub entities: Vec<EntityDoc>,
}

#[derive(Serialize, Deserialize)]
struct SceneFile {
    entities: Vec<EntityDoc>,
}

fn to_flat(v: &[EntityDoc]) -> Vec<forge_scene::store::FlatEntity> {
    v.iter()
        .map(|e| forge_scene::store::FlatEntity {
            id: e.id,
            name: e.name.clone(),
            parent: e.parent,
            properties: e.properties.clone(),
        })
        .collect()
}

fn from_flat(v: Vec<forge_scene::store::FlatEntity>) -> Vec<EntityDoc> {
    v.into_iter()
        .map(|e| EntityDoc {
            id: e.id,
            name: e.name,
            parent: e.parent,
            properties: e.properties,
        })
        .collect()
}

/// The entities as `project/scene.ron` stores them: scene instances as references and
/// override records, never copies (`forge_scene::store::compact`, WP-U20).
#[must_use]
pub fn stored_entities(entities: &[EntityDoc]) -> Vec<EntityDoc> {
    from_flat(forge_scene::store::compact(to_flat(entities)))
}

fn text_setting(settings: &BTreeMap<String, Value>, key: &str) -> String {
    match settings.get(key) {
        Some(Value::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

impl ProjectDoc {
    /// The project as it is now (entities by key, settings by key).
    #[must_use]
    pub fn from_project(p: &Project) -> Self {
        let settings = p
            .settings()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let entities = p
            .entities()
            .map(|(k, e)| EntityDoc {
                id: k.0,
                name: e.name().to_string(),
                parent: e.parent().map(|p| p.0),
                properties: e
                    .properties()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
            })
            .collect();
        Self { settings, entities }
    }

    /// [`ProjectDoc::from_project`] without the settings `drop_setting` names: a host's
    /// per-run state that no file may hold (the editor's automation grants, whose value is the
    /// run's epoch, WP-33).
    #[must_use]
    pub fn from_project_without(p: &Project, drop_setting: &dyn Fn(&str) -> bool) -> Self {
        let mut doc = Self::from_project(p);
        doc.settings.retain(|k, _| !drop_setting(k));
        doc
    }

    /// The manifest this document writes.
    #[must_use]
    pub fn manifest(&self) -> Manifest {
        Manifest {
            format: FORMAT_VERSION,
            name: text_setting(&self.settings, NAME_SETTING),
            preset: text_setting(&self.settings, PRESET_SETTING),
            template: text_setting(&self.settings, TEMPLATE_SETTING),
            engine: ENGINE_VERSION.to_string(),
        }
    }

    /// The three files' text, `(path, text)`.
    pub fn encode(&self) -> Result<Vec<(&'static str, String)>, ProjectError> {
        let pretty = || ron::ser::PrettyConfig::new().struct_names(false);
        let enc = |what: &str, r: Result<String, ron::Error>| {
            r.map_err(|e| ProjectError::BadFiles(format!("{what}: {e}")))
        };
        let mut scene = SceneFile {
            entities: stored_entities(&self.entities),
        };
        scene.entities.sort_by_key(|e| e.id);
        Ok(vec![
            (
                MANIFEST_PATH,
                enc(
                    MANIFEST_PATH,
                    ron::ser::to_string_pretty(&self.manifest(), pretty()),
                )?,
            ),
            (
                SETTINGS_PATH,
                enc(
                    SETTINGS_PATH,
                    ron::ser::to_string_pretty(&self.settings, pretty()),
                )?,
            ),
            (
                SCENE_PATH,
                enc(SCENE_PATH, ron::ser::to_string_pretty(&scene, pretty()))?,
            ),
        ])
    }

    /// Read a manifest's text (the launcher's quick look).
    pub fn read_manifest(text: &str) -> Result<Manifest, ProjectError> {
        let m: Manifest = ron::from_str(text)
            .map_err(|e| ProjectError::BadFiles(format!("{MANIFEST_PATH}: {e}")))?;
        if m.format == 0 || m.format > FORMAT_VERSION {
            return Err(ProjectError::BadFiles(format!(
                "{MANIFEST_PATH}: format {} is not readable by this editor (1..={FORMAT_VERSION})",
                m.format
            )));
        }
        Ok(m)
    }

    /// Read the three files. A missing settings or scene file is empty; a malformed one
    /// is an error (nothing half-read is loaded).
    pub fn decode(
        manifest: &str,
        settings: Option<&str>,
        scene: Option<&str>,
    ) -> Result<(Manifest, ProjectDoc), ProjectError> {
        let m = Self::read_manifest(manifest)?;
        let settings: BTreeMap<String, Value> = match settings {
            Some(t) => ron::from_str(t)
                .map_err(|e| ProjectError::BadFiles(format!("{SETTINGS_PATH}: {e}")))?,
            None => BTreeMap::new(),
        };
        let entities = match scene {
            Some(t) => Self::decode_scene(SCENE_PATH, t)?,
            None => Vec::new(),
        };
        Ok((m, ProjectDoc { settings, entities }))
    }

    /// Read a scene file's text alone — `project/scene.ron`, or a preset's new-scene
    /// template, which is the same format (Ch.31 §31.3): its entities, ids unique. `what`
    /// names the file in errors.
    pub fn decode_scene(what: &str, text: &str) -> Result<Vec<EntityDoc>, ProjectError> {
        Self::decode_scene_report(what, text).map(|(e, _)| e)
    }

    /// [`ProjectDoc::decode_scene`], with what scene composition could not compose (an
    /// instance of a scene the file lacks, scenes instancing each other, an override record
    /// whose node left its scene). Nothing is dropped for any of these: they stay in the
    /// entities, where the editor shows them; the report is the list for a log.
    pub fn decode_scene_report(
        what: &str,
        text: &str,
    ) -> Result<(Vec<EntityDoc>, Vec<String>), ProjectError> {
        let entities = ron::from_str::<SceneFile>(text)
            .map_err(|e| ProjectError::BadFiles(format!("{what}: {e}")))?
            .entities;
        let mut seen = BTreeSet::new();
        for e in &entities {
            if !seen.insert(e.id) {
                return Err(ProjectError::BadFiles(format!(
                    "{what}: entity id {} appears twice",
                    e.id
                )));
            }
        }
        // The file holds instances as references: derive their mirrors again (WP-U20).
        let (expanded, warnings) = forge_scene::store::expand(to_flat(&entities));
        Ok((from_flat(expanded), warnings))
    }

    /// The command that replaces the project with this document (see the module docs).
    pub fn load_command(&self) -> Result<EditorCommand, ProjectError> {
        let doc = serde_json::to_value(self)
            .map_err(|e| ProjectError::BadFiles(format!("the document: {e}")))?;
        Ok(EditorCommand::Invoke {
            target: LOAD_CMD.into(),
            args: serde_json::json!({ "doc": doc }).to_string(),
        })
    }
}

fn bad(why: impl Into<String>) -> CmdError {
    CmdError::BadArgs {
        target: LOAD_CMD.into(),
        why: why.into(),
    }
}

fn remap(v: &Value, ids: &BTreeMap<u64, EntityKey>) -> Value {
    match v {
        Value::Entity(k) => ids.get(&k.0).map_or(v.clone(), |n| Value::Entity(*n)),
        other => other.clone(),
    }
}

/// Plan `forge.project.load` (see the module docs): clear the project, then build the
/// document — settings, entities parents first (an entity whose parent is missing, or in
/// a parent loop, becomes a root), then properties with entity references remapped.
pub fn plan_load(b: &mut DiffBuilder<'_>, args: &serde_json::Value) -> Result<(), CmdError> {
    plan_load_with(b, args, &|_| false)
}

/// [`plan_load`], leaving out every setting of the document for which `drop_setting` is
/// true: the load neither writes such a key nor keeps one the project had (a host's
/// per-run state that must never come from a file, e.g. the editor's automation grants).
pub fn plan_load_with(
    b: &mut DiffBuilder<'_>,
    args: &serde_json::Value,
    drop_setting: &dyn Fn(&str) -> bool,
) -> Result<(), CmdError> {
    let mut doc: ProjectDoc = serde_json::from_value(
        args.get("doc")
            .cloned()
            .ok_or_else(|| bad("\"doc\" is missing"))?,
    )
    .map_err(|e| bad(format!("\"doc\": {e}")))?;
    doc.settings.retain(|k, _| !drop_setting(k));
    let project = b.project();
    let roots: Vec<EntityKey> = project.roots().collect();
    let keys: Vec<String> = project.settings().map(|(k, _)| k.to_string()).collect();
    for r in roots {
        b.despawn(r)?;
    }
    for k in keys {
        if !doc.settings.contains_key(&k) {
            b.set_setting(&k, None)?;
        }
    }
    for (k, v) in &doc.settings {
        b.set_setting(k, Some(v.clone()))?;
    }
    let in_doc: BTreeSet<u64> = doc.entities.iter().map(|e| e.id).collect();
    let mut ids: BTreeMap<u64, EntityKey> = BTreeMap::new();
    let mut left: Vec<&EntityDoc> = doc.entities.iter().collect();
    left.sort_by_key(|e| e.id);
    while !left.is_empty() {
        let before = left.len();
        let mut rest = Vec::with_capacity(left.len());
        for e in left {
            let parent = match e.parent {
                None => Some(None),
                Some(p) if !in_doc.contains(&p) => Some(None),
                Some(p) => ids.get(&p).map(|k| Some(*k)),
            };
            match parent {
                Some(parent) => {
                    let k = b.spawn(&e.name, parent)?;
                    ids.insert(e.id, k);
                }
                None => rest.push(e),
            }
        }
        if rest.len() == before {
            // A parent loop in the file: break it by making the first one a root.
            let (first, others) = rest.split_at(1);
            for e in first {
                let k = b.spawn(&e.name, None)?;
                ids.insert(e.id, k);
            }
            rest = others.to_vec();
        }
        left = rest;
    }
    for e in &doc.entities {
        let Some(k) = ids.get(&e.id).copied() else {
            continue;
        };
        for (path, v) in &e.properties {
            b.set_property(k, path, remap(v, &ids))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_cmd::{Bus, CommandPolicy, CommandSink, Issuer};

    fn bus() -> Bus {
        let mut b = Bus::new();
        b.register_handler(LOAD_CMD, CommandPolicy::ORDINARY, plan_load)
            .unwrap_or_else(|e| panic!("{e}"));
        b
    }

    fn apply(b: &mut Bus, cmd: EditorCommand) {
        let e = b.envelope(Issuer::Test, cmd);
        b.apply(e).unwrap_or_else(|r| panic!("{r:?}"));
    }

    #[test]
    fn a_project_round_trips_through_its_files_and_the_load_command() {
        let mut a = bus();
        apply(
            &mut a,
            EditorCommand::Spawn {
                name: "Shelf".into(),
                parent: None,
            },
        );
        apply(
            &mut a,
            EditorCommand::Spawn {
                name: "Crate".into(),
                parent: Some(EntityKey(0)),
            },
        );
        apply(
            &mut a,
            EditorCommand::SetProperty {
                entity: EntityKey(1),
                path: "attach.to".into(),
                value: Value::Entity(EntityKey(0)),
            },
        );
        apply(
            &mut a,
            EditorCommand::SetProperty {
                entity: EntityKey(1),
                path: "shape.radius_m".into(),
                value: Value::Float(-0.0),
            },
        );
        apply(
            &mut a,
            EditorCommand::SetSetting {
                key: "project.name".into(),
                value: Some(Value::Text("Orbits".into())),
            },
        );
        let doc = ProjectDoc::from_project(a.project());
        let files = doc.encode().unwrap_or_else(|e| panic!("{e}"));
        let get = |p: &str| files.iter().find(|(q, _)| *q == p).map(|(_, t)| t.as_str());
        let (m, back) = ProjectDoc::decode(
            get(MANIFEST_PATH).unwrap_or_default(),
            get(SETTINGS_PATH),
            get(SCENE_PATH),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(m.name, "Orbits");
        assert_eq!(back, doc);
        // Load into a bus that already holds something else, with its keys offset.
        let mut b = bus();
        for i in 0..3 {
            apply(
                &mut b,
                EditorCommand::Spawn {
                    name: format!("Old {i}"),
                    parent: None,
                },
            );
        }
        apply(
            &mut b,
            EditorCommand::SetSetting {
                key: "stale.x".into(),
                value: Some(Value::Int(1)),
            },
        );
        apply(
            &mut b,
            back.load_command().unwrap_or_else(|e| panic!("{e}")),
        );
        let p = b.project();
        assert_eq!(p.len(), 2);
        assert!(p.setting("stale.x").is_none());
        let krate = p
            .entities()
            .find(|(_, e)| e.name() == "Crate")
            .map(|(k, _)| k)
            .unwrap_or_else(|| panic!("no crate"));
        let shelf = p
            .entities()
            .find(|(_, e)| e.name() == "Shelf")
            .map(|(k, _)| k)
            .unwrap_or_else(|| panic!("no shelf"));
        assert!(shelf.0 >= 3, "fresh keys: {shelf:?}");
        let e = p.entity(krate).unwrap_or_else(|| panic!("crate"));
        assert_eq!(e.parent(), Some(shelf));
        assert_eq!(e.property("attach.to"), Some(&Value::Entity(shelf)));
        assert_eq!(e.property("shape.radius_m"), Some(&Value::Float(-0.0)));
        // Loaded content equals the original content.
        let again = ProjectDoc::from_project(p);
        assert_eq!(again.settings, doc.settings);
        assert_eq!(again.entities.len(), doc.entities.len());
    }

    #[test]
    fn bad_files_are_refused_whole() {
        assert!(
            ProjectDoc::decode(
                "(format: 9, name: \"x\", preset: \"3d\", template: \"3d\", engine: \"9\")",
                None,
                None
            )
            .is_err()
        );
        let ok = "(format: 1, name: \"x\", preset: \"3d\", template: \"3d\", engine: \"0.1.0\")";
        assert!(ProjectDoc::decode(ok, Some("{"), None).is_err());
        let dup = "(entities: [(id: 1, name: \"a\"), (id: 1, name: \"b\")])";
        assert!(matches!(
            ProjectDoc::decode(ok, None, Some(dup)),
            Err(ProjectError::BadFiles(_))
        ));
        // A document the project refuses (an empty name) changes nothing.
        let mut b = bus();
        apply(
            &mut b,
            EditorCommand::Spawn {
                name: "Keep".into(),
                parent: None,
            },
        );
        let doc = ProjectDoc {
            settings: BTreeMap::new(),
            entities: vec![EntityDoc {
                id: 1,
                name: String::new(),
                parent: None,
                properties: BTreeMap::new(),
            }],
        };
        let cmd = doc.load_command().unwrap_or_else(|e| panic!("{e}"));
        let e = b.envelope(Issuer::Test, cmd);
        assert!(b.apply(e).is_err());
        assert_eq!(b.project().len(), 1);
    }

    #[test]
    fn a_parent_loop_in_a_file_loads_as_roots() {
        let doc = ProjectDoc {
            settings: BTreeMap::new(),
            entities: vec![
                EntityDoc {
                    id: 1,
                    name: "a".into(),
                    parent: Some(2),
                    properties: BTreeMap::new(),
                },
                EntityDoc {
                    id: 2,
                    name: "b".into(),
                    parent: Some(1),
                    properties: BTreeMap::new(),
                },
                EntityDoc {
                    id: 3,
                    name: "c".into(),
                    parent: Some(99),
                    properties: BTreeMap::new(),
                },
            ],
        };
        let mut b = bus();
        apply(&mut b, doc.load_command().unwrap_or_else(|e| panic!("{e}")));
        assert_eq!(b.project().len(), 3);
        assert_eq!(b.project().roots().count(), 2, "a (loop broken) and c");
    }
}

//! Instances store references, not copies (the store rule, WP-U20; gate
//! `C-scene-store-references`) — measured in the project's real files.
//!
//! One project, 100 instances of a 21-node scene (each instance moved, one in ten with a
//! recoloured node), is written through `ProjectDoc::encode`, the bytes the store keeps.
//! The same project with every instance made local (100 plain copies with identical
//! content) is written the same way. The instance form must be at least 10x smaller, and
//! decoding it must give back exactly the project (ids included), which then loads into a
//! fresh core with the same content.
//!
//! Positive control: with the store's `store_copies` fault (every mirror written whole)
//! the instance form is no smaller than the copies, so the size claim fails.

use std::collections::BTreeMap;

use forge_cmd::{Bus, Change, CommandSink, EditorCommand, EntityKey, Issuer, Value};
use forge_project::format::{EntityDoc, LOAD_CMD, ProjectDoc, SCENE_PATH, plan_load};
use forge_scene::SceneFaults;

fn bus() -> Bus {
    let mut b = Bus::new();
    b.register_handler(LOAD_CMD, forge_cmd::CommandPolicy::ORDINARY, plan_load)
        .unwrap_or_else(|e| panic!("{e}"));
    forge_scene::install(&mut b, &[LOAD_CMD]);
    b
}

fn run(b: &mut Bus, cmd: EditorCommand) -> std::sync::Arc<forge_cmd::Applied> {
    let e = b.envelope(Issuer::Test, cmd);
    b.apply(e).unwrap_or_else(|r| panic!("{r:?}"))
}

fn created(a: &forge_cmd::Applied) -> EntityKey {
    a.diff
        .changes
        .iter()
        .find_map(|c| match c {
            Change::Created { entity, .. } => Some(*entity),
            _ => None,
        })
        .unwrap_or_else(|| panic!("nothing created"))
}

fn set(b: &mut Bus, e: EntityKey, p: &str, v: Value) {
    run(
        b,
        EditorCommand::SetProperty {
            entity: e,
            path: p.into(),
            value: v,
        },
    );
}

fn invoke(
    b: &mut Bus,
    target: &str,
    args: serde_json::Value,
) -> std::sync::Arc<forge_cmd::Applied> {
    run(
        b,
        EditorCommand::Invoke {
            target: target.into(),
            args: args.to_string(),
        },
    )
}

/// A 21-node "Street lamp" scene with typical content, then 100 instances; returns the
/// bus and the instance roots.
fn hundred_instances() -> (Bus, Vec<EntityKey>) {
    let mut b = bus();
    invoke(
        &mut b,
        "forge.scene.new",
        serde_json::json!({"name": "Street lamp", "id": "street_lamp"}),
    );
    let root = forge_scene::model::scene_root(b.project(), "street_lamp")
        .unwrap_or_else(|| panic!("scene"));
    let props = |b: &mut Bus, e: EntityKey, i: usize| {
        set(
            b,
            e,
            "transform.position",
            Value::Vec3([0.0, i as f64 * 0.25, 0.0]),
        );
        set(b, e, "transform.rotation", Value::Vec3([0.0, 0.0, 0.0]));
        set(b, e, "transform.scale", Value::Vec3([1.0, 1.0, 1.0]));
        set(
            b,
            e,
            "render.mesh",
            Value::Text(format!("meshes/lamp/part_{i:02}.mesh")),
        );
        set(
            b,
            e,
            "render.material",
            Value::Text("materials/painted_steel.mat".into()),
        );
        set(b, e, "physics.collider", Value::Text("box".into()));
    };
    props(&mut b, root, 0);
    let mut parent = root;
    for i in 1..=20 {
        let a = run(
            &mut b,
            EditorCommand::Spawn {
                name: format!("Part {i}"),
                parent: Some(if i % 5 == 0 { root } else { parent }),
            },
        );
        let e = created(&a);
        props(&mut b, e, i);
        parent = e;
    }
    let mut roots = Vec::new();
    for i in 0..100 {
        let a = invoke(
            &mut b,
            "forge.scene.instance",
            serde_json::json!({"scene": "street_lamp", "name": format!("Lamp {i}")}),
        );
        let inst = created(&a);
        set(
            &mut b,
            inst,
            "transform.position",
            Value::Vec3([i as f64 * 12.0, 0.0, 3.5]),
        );
        if i % 10 == 0 {
            let first = b
                .project()
                .entity(inst)
                .and_then(|e| e.children().next())
                .unwrap_or_else(|| panic!("a part"));
            set(
                &mut b,
                first,
                "render.material",
                Value::Text("materials/rust.mat".into()),
            );
        }
        roots.push(inst);
    }
    (b, roots)
}

fn scene_bytes(doc: &ProjectDoc) -> usize {
    doc.encode()
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .find(|(p, _)| *p == SCENE_PATH)
        .map_or(0, |(_, t)| t.len())
}

fn sorted(mut v: Vec<EntityDoc>) -> Vec<EntityDoc> {
    v.sort_by_key(|e| e.id);
    v
}

#[test]
fn a_hundred_instances_store_as_references_not_copies() {
    let (b, roots) = hundred_instances();
    let doc = ProjectDoc::from_project(b.project());
    let instances = scene_bytes(&doc);

    // The same content as 100 plain copies: make every instance local.
    let (mut c, roots_c) = hundred_instances();
    for r in roots_c {
        invoke(
            &mut c,
            "forge.scene.make_local",
            serde_json::json!({"entity": r.0}),
        );
    }
    let copies_doc = ProjectDoc::from_project(c.project());
    let copies = scene_bytes(&copies_doc);
    let entities = doc.entities.len();
    let stored = forge_project::format::stored_entities(&doc.entities).len();
    println!("store size, project/scene.ron (21-node scene, 100 placed):");
    println!("  100 instances: {instances} bytes ({stored} stored entities of {entities})");
    println!(
        "  100 copies:    {copies} bytes ({} stored entities)",
        copies_doc.entities.len()
    );
    println!(
        "  saving: {:.1}x ({:.1} bytes per instance vs {:.1} per copy)",
        copies as f64 / instances as f64,
        instances as f64 / roots.len() as f64,
        copies as f64 / roots.len() as f64
    );
    assert!(
        instances * 10 <= copies,
        "instances ({instances} B) must store at least 10x smaller than copies ({copies} B)"
    );

    // Lossless through the real files: decode gives back exactly the project, ids included.
    let files = doc.encode().unwrap_or_else(|e| panic!("{e}"));
    let get = |p: &str| files.iter().find(|(q, _)| *q == p).map(|(_, t)| t.as_str());
    let (_, back) = ProjectDoc::decode(
        get(forge_project::format::MANIFEST_PATH).unwrap_or_default(),
        get(forge_project::format::SETTINGS_PATH),
        get(SCENE_PATH),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(sorted(back.entities.clone()), sorted(doc.entities.clone()));
    // And it loads into a fresh core with the same content (keys are the core's own).
    let mut fresh = bus();
    run(
        &mut fresh,
        back.load_command().unwrap_or_else(|e| panic!("{e}")),
    );
    assert_eq!(fresh.project().len(), b.project().len());
    let shape = |p: &forge_cmd::Project| {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for (_, e) in p.entities() {
            let content: Vec<String> = e
                .properties()
                .filter(|(k, _)| !forge_scene::model::is_meta(k))
                .map(|(k, v)| format!("{k}={v:?}"))
                .collect();
            *m.entry(format!("{}|{}", e.name(), content.join(";")))
                .or_default() += 1;
        }
        m
    };
    assert_eq!(shape(fresh.project()), shape(b.project()));
    // Loaded instances are still linked: a scene edit reaches all of them.
    let root = forge_scene::model::scene_root(fresh.project(), "street_lamp")
        .unwrap_or_else(|| panic!("scene"));
    set(
        &mut fresh,
        root,
        "physics.collider",
        Value::Text("capsule".into()),
    );
    let following = forge_scene::model::dependents(fresh.project(), root)
        .into_iter()
        .filter(|d| {
            fresh
                .project()
                .entity(*d)
                .and_then(|e| e.property("physics.collider"))
                == Some(&Value::Text("capsule".into()))
        })
        .count();
    assert_eq!(following, 100);
}

#[test]
fn positive_control_a_store_that_copies_fails_the_size_claim() {
    let (b, _) = hundred_instances();
    let doc = ProjectDoc::from_project(b.project());
    let (mut c, roots_c) = hundred_instances();
    for r in roots_c {
        invoke(
            &mut c,
            "forge.scene.make_local",
            serde_json::json!({"entity": r.0}),
        );
    }
    let copies = scene_bytes(&ProjectDoc::from_project(c.project()));
    // The instance project written by a store whose compaction keeps every mirror.
    let full: Vec<forge_scene::store::FlatEntity> = doc
        .entities
        .iter()
        .map(|e| forge_scene::store::FlatEntity {
            id: e.id,
            name: e.name.clone(),
            parent: e.parent,
            properties: e.properties.clone(),
        })
        .collect();
    let copied: Vec<EntityDoc> = forge_scene::store::compact_with(
        full,
        &SceneFaults {
            store_copies: true,
            ..SceneFaults::default()
        },
    )
    .into_iter()
    .map(|e| EntityDoc {
        id: e.id,
        name: e.name,
        parent: e.parent,
        properties: e.properties,
    })
    .collect();
    let faulty =
        ron::ser::to_string_pretty(&copied, ron::ser::PrettyConfig::new().struct_names(false))
            .unwrap_or_default()
            .len();
    println!("control: the copying store writes {faulty} bytes vs {copies} for plain copies");
    assert!(
        faulty * 10 > copies,
        "the size claim must fail for a store that copies ({faulty} B vs {copies} B)"
    );
}

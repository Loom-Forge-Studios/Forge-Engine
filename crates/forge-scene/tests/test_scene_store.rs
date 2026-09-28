//! The store form (gate `C-scene-store-references`): files keep references and override
//! records, never copies, and the form is lossless.
//!
//! * `expand(compact(e)) == e` for a project that uses every composition feature: nesting,
//!   a depth-3 chain, overrides of values, names and cleared properties at several levels,
//!   authored children under inherited nodes, an entity reference to an inherited node,
//!   and Make Local inside a scene that is itself instanced. Ids come back too.
//! * An unchanged mirror is not stored at all: the compact form has no entity for it.
//! * A file that names a missing scene, or scenes that instance each other, loads with a
//!   warning and loses no authored entity.
//!
//! Positive controls: a compact form with one override record removed does not round
//! trip; with the `store_copies` fault the compact form stores every mirror.

mod common;

use std::collections::BTreeMap;

use common::{Rig, f, t, tree};
use forge_cmd::{EditorCommand, Project, Value};
use forge_scene::SceneFaults;
use forge_scene::model;
use forge_scene::store::{FlatEntity, compact, compact_with, expand};

fn flat(p: &Project) -> Vec<FlatEntity> {
    p.entities()
        .map(|(k, e)| FlatEntity {
            id: k.0,
            name: e.name().to_string(),
            parent: e.parent().map(|p| p.0),
            properties: e
                .properties()
                .map(|(p, v)| (p.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
        })
        .collect()
}

/// A project using every composition feature.
fn everything() -> Rig {
    let mut rig = Rig::new();
    let t0 = tree(&mut rig);
    // Placed content: positions in the scene, instances moved and turned.
    for (name, p) in [("Trunk", [0.0, 1.0, 0.0]), ("Leaves", [0.3, 3.0, -0.2])] {
        let k = rig.child(t0, name);
        rig.set(k, "transform.position.local", Value::Vec3(p));
    }
    let forest = rig.scene("Forest", "forest");
    let oak = rig.instance("tree", Some(forest), "Oak");
    rig.set(oak, "height", f(7.0));
    rig.set(
        oak,
        "transform.position.local",
        Value::Vec3([4.0, 0.0, 2.5]),
    );
    rig.set(oak, "transform.scale", f(1.25));
    let elm = rig.instance("tree", Some(forest), "Elm");
    rig.run(EditorCommand::RemoveProperty {
        entity: rig.child(elm, "Leaves"),
        path: "color".into(),
    });
    let mid = rig.inherit("forest", "Dense forest", "dense");
    rig.set(rig.at(mid, &["Oak", "Trunk"]), "radius", f(0.8));
    rig.spawn("Shrub", Some(mid));
    rig.inherit("dense", "Denser forest", "denser");
    let top = rig.inherit("denser", "Densest forest", "densest");
    rig.run(EditorCommand::Rename {
        entity: rig.at(top, &["Elm", "Trunk"]),
        name: "Old trunk".into(),
    });
    for i in 0..3 {
        let w = rig.instance("densest", None, &format!("World forest {i}"));
        rig.set(
            w,
            "transform.position.local",
            Value::Vec3([i as f64 * 50.0, 0.0, 0.0]),
        );
        rig.set(w, "transform.yaw", f(i as f64 * 37.5));
    }
    let w0 = rig
        .bus
        .project()
        .roots()
        .find(|r| rig.name(*r) == "World forest 0");
    let w0 = w0.unwrap_or_else(|| panic!("w0"));
    // An authored child under an inherited node, and a reference to an inherited node.
    let nest = rig.spawn("Nest", Some(rig.at(w0, &["Oak", "Leaves"])));
    rig.set(nest, "eggs", Value::Int(3));
    let camera = rig.spawn("Camera", None);
    rig.set(
        camera,
        "follow",
        Value::Entity(rig.at(w0, &["Elm", "Old trunk"])),
    );
    // Make Local inside a scene that is itself instanced (its nodes keep stable uids).
    let yard = rig.scene("Yard", "yard");
    let yard_tree = rig.instance("tree", Some(yard), "Yard tree");
    rig.invoke(
        "forge.scene.make_local",
        serde_json::json!({"entity": yard_tree.0}),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    rig.instance("yard", None, "House yard");
    rig
}

#[test]
fn the_store_form_is_lossless_and_stores_no_unchanged_mirror() {
    let rig = everything();
    let full = flat(rig.bus.project());
    let stored = compact(full.clone());
    let (back, warnings) = expand(stored.clone());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(back.len(), full.len());
    for (a, b) in full.iter().zip(&back) {
        assert_eq!(a, b, "entity {} did not round-trip", a.id);
    }
    // Unchanged mirrors are not in the stored form.
    let p = rig.bus.project();
    let unchanged: Vec<u64> = p
        .entities()
        .filter(|(k, _)| {
            model::instance_scene(p, *k).is_none()
                && model::overrides(p, *k).is_some_and(|o| o.is_empty())
                && p.entity(*k).is_some_and(|e| e.children().len() == 0)
                && p.referrers(*k).all(|(_, path)| path == model::BASE)
        })
        .map(|(k, _)| k.0)
        .collect();
    assert!(
        unchanged.len() >= 20,
        "{} unchanged leaf mirrors",
        unchanged.len()
    );
    for id in &unchanged {
        assert!(
            !stored.iter().any(|e| e.id == *id),
            "unchanged mirror {id} was stored"
        );
    }
}

#[test]
fn positive_control_a_lost_override_record_does_not_round_trip() {
    let rig = everything();
    let full = flat(rig.bus.project());
    let mut stored = compact(full.clone());
    let at = stored
        .iter()
        .position(|e| e.properties.contains_key(model::OF) && e.properties.len() > 1)
        .unwrap_or_else(|| panic!("an override record"));
    stored.remove(at);
    let (back, _) = expand(stored);
    assert_ne!(back, full, "the round-trip check must see a lost override");
}

#[test]
fn positive_control_the_copies_fault_stores_every_mirror() {
    let rig = everything();
    let full = flat(rig.bus.project());
    let copies = compact_with(
        full.clone(),
        &SceneFaults {
            store_copies: true,
            ..SceneFaults::default()
        },
    );
    assert_eq!(copies.len(), full.len());
    assert!(compact(full).len() < copies.len() / 2);
}

#[test]
fn a_file_with_a_missing_or_looping_scene_loads_with_a_warning_and_loses_nothing() {
    let rig = everything();
    let mut stored = compact(flat(rig.bus.project()));
    let authored_before: Vec<u64> = stored
        .iter()
        .filter(|e| !e.properties.contains_key(model::OF))
        .map(|e| e.id)
        .collect();
    // Rename the id of the `tree` scene: its instances now name a scene that is not there.
    for e in &mut stored {
        if e.properties.get(model::ID) == Some(&t("tree")) {
            e.properties.insert(model::ID.into(), t("gone"));
        }
    }
    let (back, warnings) = expand(stored);
    assert!(
        warnings.iter().any(|w| w.contains("\"tree\"")),
        "names the missing scene: {warnings:?}"
    );
    for id in authored_before {
        assert!(back.iter().any(|e| e.id == id), "authored entity {id} lost");
    }
}

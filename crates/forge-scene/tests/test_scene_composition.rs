//! Scene composition, nesting and inheritance (DoD M5-9, gate `C-scene-composition`).
//!
//! * Base edits — properties, names, nodes added, removed and moved — reach every instance
//!   and every nested instance, except what an instance overrides at any level.
//!   **Positive control:** a deriver that ignores overrides fails the same scenario.
//! * An inheritance chain of depth 3 (and an instance of its last scene) resolves, and a
//!   base edit runs down it except where a level overrides.
//! * A cycle is refused with a readable error naming the loop, whether by instancing or by
//!   moving an instance. **Positive control:** a deriver that skips the check lets a move
//!   make a scene contain itself.
//! * Revert to base (per property and per node) is a command: undo and redo restore
//!   exactly.
//! * Make Local breaks the link, keeps the values, keeps nested scenes linked and names
//!   what it loses; instances refuse structural edits of what they inherit, `scene.*` is
//!   written only by the scene commands, and a scene in use cannot be deleted.
//! * The same command script gives the same project on every run (determinism).

mod common;

use common::{Rig, f, t, tree};
use forge_cmd::{CommandSink, EditorCommand, EntityKey, Value};
use forge_scene::{SceneFaults, model};

/// Check `cond`, or return the failure (the scenarios return their first failed claim so a
/// positive control can show which one its fault breaks).
macro_rules! claim {
    ($cond:expr, $($msg:tt)+) => {
        if !$cond {
            return Err(format!($($msg)+));
        }
    };
}

/// Tree ⊂ Forest ⊂ world (two forests, two trees each), overrides at the world and the
/// Forest level, then base edits of every kind.
fn nested_scenario(rig: &mut Rig) -> Result<(), String> {
    tree(rig);
    let forest = rig.scene("Forest", "forest");
    let oak = rig.instance("tree", Some(forest), "Oak");
    let elm = rig.instance("tree", Some(forest), "Elm");
    // Forest-level override: the Elm is taller.
    rig.set(elm, "height", f(9.0));
    let north = rig.instance("forest", None, "North");
    let south = rig.instance("forest", None, "South");
    // World-level override: the North Oak's leaves are red; the South Elm's trunk renamed.
    let north_oak_leaves = rig.at(north, &["Oak", "Leaves"]);
    rig.set(north_oak_leaves, "color", t("red"));
    let south_elm_trunk = rig.at(south, &["Elm", "Trunk"]);
    rig.run(EditorCommand::Rename {
        entity: south_elm_trunk,
        name: "Bole".into(),
    });

    // The resolved state before any base edit.
    claim!(
        rig.get(rig.at(south, &["Oak"]), "height") == Some(f(4.0)),
        "a nested instance resolves the base value"
    );
    claim!(
        rig.get(rig.at(north, &["Elm"]), "height") == Some(f(9.0)),
        "a nested instance resolves its container's override"
    );

    // 1. A property edit on the base.
    let tree_root = rig.root("tree");
    let leaves = rig.child(tree_root, "Leaves");
    rig.set(leaves, "color", t("gold"));
    rig.set(tree_root, "height", f(5.0));
    for (forest_i, name) in [
        (north, "Oak"),
        (north, "Elm"),
        (south, "Oak"),
        (south, "Elm"),
    ] {
        let l = rig.at(forest_i, &[name, "Leaves"]);
        let want = if l == north_oak_leaves { "red" } else { "gold" };
        claim!(
            rig.get(l, "color") == Some(t(want)),
            "{}/{name}/Leaves color is {:?}, want {want}",
            rig.name(forest_i),
            rig.get(l, "color")
        );
    }
    claim!(
        rig.get(oak, "height") == Some(f(5.0)),
        "the Forest's Oak follows the base height"
    );
    claim!(
        rig.get(elm, "height") == Some(f(9.0)),
        "the Forest's Elm keeps its override ({:?})",
        rig.get(elm, "height")
    );
    claim!(
        rig.get(rig.at(south, &["Oak"]), "height") == Some(f(5.0)),
        "a nested instance follows the base through its container"
    );
    claim!(
        rig.get(rig.at(south, &["Elm"]), "height") == Some(f(9.0)),
        "a nested instance keeps its container's override"
    );

    // 2. A rename on the base.
    let trunk = rig.child(tree_root, "Trunk");
    rig.run(EditorCommand::Rename {
        entity: trunk,
        name: "Stem".into(),
    });
    claim!(
        rig.try_child(rig.at(north, &["Oak"]), "Stem").is_some(),
        "the rename reaches a nested instance"
    );
    claim!(
        rig.name(south_elm_trunk) == "Bole",
        "a renamed node keeps its name (it is {:?})",
        rig.name(south_elm_trunk)
    );

    // 3. A node added to the base appears everywhere, with its properties.
    let fruit = rig.spawn("Fruit", Some(tree_root));
    rig.set(fruit, "mesh", t("apple.mesh"));
    for forest_i in [north, south] {
        for name in ["Oak", "Elm"] {
            let got = rig.try_child(rig.at(forest_i, &[name]), "Fruit");
            claim!(got.is_some(), "{}/{name} has no Fruit", rig.name(forest_i));
            claim!(
                got.and_then(|g| rig.get(g, "mesh")) == Some(t("apple.mesh")),
                "the new node carries its properties"
            );
        }
    }
    // 4. Moved in the base: moved in every instance.
    rig.run(EditorCommand::Reparent {
        entity: fruit,
        parent: Some(leaves),
    });
    claim!(
        rig.try_child(rig.at(north, &["Oak", "Leaves"]), "Fruit")
            .is_some()
            && rig.try_child(rig.at(north, &["Oak"]), "Fruit").is_none(),
        "a move in the base moves the node in every instance"
    );
    // 5. Removed from the base: removed from every instance.
    rig.run(EditorCommand::Despawn { entity: fruit });
    claim!(
        rig.try_child(rig.at(south, &["Elm", "Leaves"]), "Fruit")
            .is_none(),
        "a node removed from the base leaves every instance"
    );
    // 6. A property removed from the base is removed where it was inherited.
    rig.run(EditorCommand::RemoveProperty {
        entity: leaves,
        path: "color".into(),
    });
    claim!(
        rig.get(rig.at(south, &["Oak", "Leaves"]), "color")
            .is_none(),
        "an inherited property removed from the base goes"
    );
    claim!(
        rig.get(north_oak_leaves, "color") == Some(t("red")),
        "an overridden property stays when the base removes it"
    );
    Ok(())
}

#[test]
fn nested_instance_edits_propagate_from_the_base_except_overrides() {
    let mut rig = Rig::new();
    nested_scenario(&mut rig).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_deriver_ignoring_overrides_fails() {
    let mut rig = Rig::with_faults(SceneFaults {
        ignore_overrides: true,
        ..SceneFaults::default()
    });
    let e = nested_scenario(&mut rig).expect_err("overrides ignored: the scenario must fail");
    assert!(
        e.contains("red") || e.contains("override") || e.contains("keeps"),
        "it fails on an override: {e}"
    );
}

#[test]
fn positive_control_a_deriver_that_does_not_propagate_fails() {
    let mut rig = Rig::with_faults(SceneFaults {
        no_propagation: true,
        ..SceneFaults::default()
    });
    assert!(nested_scenario(&mut rig).is_err());
}

/// Base → Mid (overrides color, adds Bird) → Top (overrides height) → Leaf (overrides
/// nothing), and a world instance of Leaf.
fn chain_scenario(rig: &mut Rig) -> Result<(), String> {
    let base = tree(rig);
    let mid = rig.inherit("tree", "Autumn tree", "autumn_tree");
    let mid_leaves = rig.child(mid, "Leaves");
    rig.set(mid_leaves, "color", t("orange"));
    let bird = rig.spawn("Bird", Some(mid));
    rig.set(bird, "song", t("chirp"));
    let top = rig.inherit("autumn_tree", "Tall autumn tree", "tall_autumn_tree");
    rig.set(top, "height", f(12.0));
    let leaf = rig.inherit("tall_autumn_tree", "Park tree", "park_tree");
    let placed = rig.instance("park_tree", None, "Park tree 1");

    // Depth 3 resolves: every level's contribution is there.
    claim!(
        model::role(rig.bus.project(), leaf)
            == model::Role::SceneRoot {
                id: "park_tree".into(),
                inherits: Some("tall_autumn_tree".into())
            },
        "the third level is a derived scene"
    );
    claim!(
        rig.get(placed, "height") == Some(f(12.0)),
        "level 2's override resolves at depth 3"
    );
    claim!(
        rig.get(rig.child(placed, "Leaves"), "color") == Some(t("orange")),
        "level 1's override resolves at depth 3"
    );
    claim!(
        rig.get(rig.child(placed, "Bird"), "song") == Some(t("chirp")),
        "level 1's addition resolves at depth 3"
    );
    claim!(
        rig.get(rig.child(placed, "Trunk"), "mesh") == Some(t("trunk.mesh")),
        "the base resolves at depth 3"
    );
    // A base edit runs down the chain, except where a level overrides.
    let trunk = rig.child(base, "Trunk");
    rig.set(trunk, "radius", f(0.5));
    rig.set(base, "height", f(6.0));
    rig.set(rig.child(base, "Leaves"), "color", t("lime"));
    claim!(
        rig.get(rig.child(placed, "Trunk"), "radius") == Some(f(0.5)),
        "a base edit reaches depth 3"
    );
    claim!(
        rig.get(mid, "height") == Some(f(6.0)),
        "a base edit reaches depth 1"
    );
    claim!(
        rig.get(placed, "height") == Some(f(12.0)),
        "level 2's override holds below it"
    );
    claim!(
        rig.get(rig.child(placed, "Leaves"), "color") == Some(t("orange")),
        "level 1's override holds below it"
    );
    // An edit in the middle of the chain reaches below it only.
    rig.set(bird, "song", t("tweet"));
    claim!(
        rig.get(rig.child(placed, "Bird"), "song") == Some(t("tweet")),
        "a mid-chain edit reaches depth 3"
    );
    claim!(
        rig.try_child(base, "Bird").is_none(),
        "a derived scene's addition never reaches its base"
    );
    Ok(())
}

#[test]
fn an_inheritance_chain_of_depth_three_resolves() {
    let mut rig = Rig::new();
    chain_scenario(&mut rig).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_the_chain_fails_when_overrides_are_ignored() {
    let mut rig = Rig::with_faults(SceneFaults {
        ignore_overrides: true,
        ..SceneFaults::default()
    });
    assert!(chain_scenario(&mut rig).is_err());
}

#[test]
fn a_cycle_is_refused_with_a_readable_error() {
    let mut rig = Rig::new();
    let a = rig.scene("Alpha", "alpha");
    let b = rig.scene("Beta", "beta");
    let c = rig.scene("Gamma", "gamma");
    // A scene inside itself.
    let e = rig
        .try_instance("alpha", Some(a), "self")
        .expect_err("a scene inside itself");
    let text = e.to_string();
    assert!(text.contains("SCENE-0002"), "{text}");
    assert!(
        text.contains("Alpha \u{2192} Alpha"),
        "names the loop: {text}"
    );
    // alpha ⊃ beta ⊃ gamma; gamma ⊃ alpha would close the loop.
    rig.instance("beta", Some(a), "b in a");
    rig.instance("gamma", Some(b), "c in b");
    let hash = rig.bus.project().state_hash();
    let e = rig
        .try_instance("alpha", Some(c), "a in c")
        .expect_err("a three-scene loop");
    let text = e.to_string();
    assert!(text.contains("SCENE-0002"), "{text}");
    assert!(
        text.contains("Gamma \u{2192} Alpha \u{2192} Beta \u{2192} Gamma"),
        "names the whole loop: {text}"
    );
    assert_eq!(rig.bus.project().state_hash(), hash, "nothing applied");
    // Moving an instance from the world into a scene it contains is the same loop.
    move_refusal(&mut rig).unwrap_or_else(|e| panic!("{e}"));
}

/// Move a world instance of Alpha into Gamma (Alpha ⊃ Beta ⊃ Gamma): the move must be
/// refused as a readable `SCENE-0002` naming the loop, and change nothing.
fn move_refusal(rig: &mut Rig) -> Result<(), String> {
    let c = rig.root("gamma");
    let loose = rig.instance("alpha", None, "loose alpha");
    let hash = rig.bus.project().state_hash();
    let r = rig.try_run(EditorCommand::Reparent {
        entity: loose,
        parent: Some(c),
    });
    let Err(e) = r else {
        return Err("the move that closes the loop was accepted".into());
    };
    let text = e.to_string();
    claim!(
        text.contains("SCENE-0002"),
        "not the readable cycle error: {text}"
    );
    claim!(
        text.contains("Gamma \u{2192} Alpha \u{2192} Beta \u{2192} Gamma"),
        "does not name the loop: {text}"
    );
    claim!(
        rig.bus.project().state_hash() == hash,
        "something was applied"
    );
    Ok(())
}

#[test]
fn positive_control_a_deriver_skipping_the_cycle_check_fails_the_move_guard() {
    let mut rig = Rig::with_faults(SceneFaults {
        skip_cycle_check: true,
        ..SceneFaults::default()
    });
    let a = rig.scene("Alpha", "alpha");
    let b = rig.scene("Beta", "beta");
    rig.scene("Gamma", "gamma");
    rig.instance("beta", Some(a), "b in a");
    rig.instance("gamma", Some(b), "c in b");
    let e = move_refusal(&mut rig).expect_err("without the check the guard must fail");
    assert!(
        !e.contains("names the loop") || e.contains("SCENE-0002") || !e.is_empty(),
        "{e}"
    );
}

#[test]
fn revert_to_base_is_undoable() {
    let mut rig = Rig::new();
    tree(&mut rig);
    let inst = rig.instance("tree", None, "Tree 1");
    let leaves = rig.child(inst, "Leaves");
    rig.set(leaves, "color", t("blue"));
    rig.set(leaves, "mesh", t("pine.mesh"));
    rig.run(EditorCommand::Rename {
        entity: leaves,
        name: "Needles".into(),
    });
    let over = model::overrides(rig.bus.project(), leaves).unwrap_or_default();
    assert_eq!(
        over.paths().collect::<Vec<_>>(),
        vec!["color", "mesh"],
        "the override record is the property diff"
    );
    assert_eq!(over.name.as_deref(), Some("Needles"));
    let before = rig.bus.project().state_hash();

    // Per property.
    let a = rig
        .invoke(
            "forge.scene.revert",
            serde_json::json!({"entity": leaves.0, "path": "color"}),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rig.get(leaves, "color"), Some(t("green")));
    assert_eq!(
        rig.get(leaves, "mesh"),
        Some(t("pine.mesh")),
        "only that one"
    );
    rig.bus.undo(a.txn).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(
        rig.bus.project().state_hash(),
        before,
        "undo restores the override"
    );
    rig.bus.redo(a.txn).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(
        rig.get(leaves, "color"),
        Some(t("green")),
        "redo reverts again"
    );
    rig.bus.undo(a.txn).unwrap_or_else(|r| panic!("{r:?}"));

    // Per node: every property and the name.
    let a = rig
        .invoke(
            "forge.scene.revert",
            serde_json::json!({"entity": leaves.0}),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        model::overrides(rig.bus.project(), leaves).is_some_and(|o| o.is_empty()),
        "nothing overridden after reverting the node"
    );
    assert_eq!(rig.name(leaves), "Leaves");
    // A reverted node follows the base again.
    let base_leaves = rig.child(rig.root("tree"), "Leaves");
    rig.set(base_leaves, "color", t("teal"));
    assert_eq!(rig.get(leaves, "color"), Some(t("teal")));
    let after_edit = rig
        .bus
        .undo_target()
        .unwrap_or_else(|| panic!("an undo target"));
    rig.bus.undo(after_edit).unwrap_or_else(|r| panic!("{r:?}"));
    rig.bus.undo(a.txn).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(
        rig.bus.project().state_hash(),
        before,
        "undo restores every override"
    );
    // Reverting something that is not linked is refused, naming why.
    let plain = rig.spawn("Rock", None);
    let e = rig
        .invoke("forge.scene.revert", serde_json::json!({"entity": plain.0}))
        .expect_err("not linked");
    assert!(e.to_string().contains("SCENE-0005"), "{e}");
}

#[test]
fn make_local_breaks_the_link_keeps_values_and_names_the_loss() {
    let mut rig = Rig::new();
    tree(&mut rig);
    let yard = rig.scene("Yard", "yard");
    rig.instance("tree", Some(yard), "Yard tree");
    let house = rig.instance("yard", None, "House yard");
    let leaves = rig.at(house, &["Yard tree", "Leaves"]);
    rig.set(leaves, "color", t("red"));
    let loss = model::make_local_loss(rig.bus.project(), house).unwrap_or_else(|| panic!("loss"));
    assert_eq!(loss.scene, "Yard");
    assert_eq!(loss.nested, 1, "the nested tree stays linked");
    assert!(loss.nodes >= 4 && loss.overridden == 1, "{loss:?}");
    let a = rig
        .invoke(
            "forge.scene.make_local",
            serde_json::json!({"entity": house.0}),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(model::role(rig.bus.project(), house), model::Role::Plain);
    // Yard edits no longer reach it; the nested tree still follows Tree.
    let yard_root = rig.root("yard");
    rig.set(yard_root, "fence", t("white"));
    assert_eq!(rig.get(house, "fence"), None, "the link is broken");
    let nested = rig.at(house, &["Yard tree"]);
    assert_eq!(
        model::role(rig.bus.project(), nested),
        model::Role::InstanceRoot {
            scene: "tree".into()
        }
    );
    let trunk = rig.child(rig.root("tree"), "Trunk");
    rig.set(trunk, "radius", f(0.9));
    assert_eq!(rig.get(rig.at(nested, &["Trunk"]), "radius"), Some(f(0.9)));
    assert_eq!(rig.get(leaves, "color"), Some(t("red")), "values kept");
    // Undo restores the link exactly.
    for _ in 0..2 {
        let u = rig.bus.undo_target().unwrap_or_else(|| panic!("undo"));
        rig.bus.undo(u).unwrap_or_else(|r| panic!("{r:?}"));
    }
    rig.bus.undo(a.txn).unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(
        model::role(rig.bus.project(), house),
        model::Role::InstanceRoot {
            scene: "yard".into()
        }
    );
    // Make Local inside an instance is refused, naming what to do.
    let e = rig
        .invoke(
            "forge.scene.make_local",
            serde_json::json!({"entity": leaves.0}),
        )
        .expect_err("not a root");
    assert!(e.to_string().contains("SCENE-0008"), "{e}");
}

#[test]
fn instances_refuse_structural_edits_of_what_they_inherit() {
    let mut rig = Rig::new();
    tree(&mut rig);
    let inst = rig.instance("tree", None, "Tree 1");
    let trunk = rig.child(inst, "Trunk");
    let hash = rig.bus.project().state_hash();
    let e = rig
        .try_run(EditorCommand::Despawn { entity: trunk })
        .expect_err("inherited node deleted");
    assert!(e.to_string().contains("SCENE-0003"), "{e}");
    let e = rig
        .try_run(EditorCommand::Reparent {
            entity: trunk,
            parent: None,
        })
        .expect_err("inherited node moved");
    assert!(e.to_string().contains("SCENE-0003"), "{e}");
    let e = rig
        .try_run(EditorCommand::SetProperty {
            entity: trunk,
            path: "scene.base".into(),
            value: Value::Entity(EntityKey(0)),
        })
        .expect_err("bookkeeping written by a plain command");
    assert!(e.to_string().contains("SCENE-0006"), "{e}");
    let e = rig
        .try_run(EditorCommand::Despawn {
            entity: rig.root("tree"),
        })
        .expect_err("a scene in use deleted");
    assert!(e.to_string().contains("SCENE-0004"), "{e}");
    assert!(e.to_string().contains("Tree 1"), "names the user: {e}");
    assert_eq!(rig.bus.project().state_hash(), hash, "nothing applied");
    // Local additions under an inherited node, and deleting the whole instance, are fine.
    let nest = rig.spawn("Nest", Some(trunk));
    assert_eq!(
        rig.bus.project().entity(nest).and_then(|d| d.parent()),
        Some(trunk)
    );
    rig.run(EditorCommand::Despawn { entity: inst });
    rig.run(EditorCommand::Despawn {
        entity: rig.root("tree"),
    });
    assert!(model::scene_root(rig.bus.project(), "tree").is_none());
}

#[test]
fn pack_turns_a_branch_into_a_scene_and_an_instance() {
    let mut rig = Rig::new();
    let lamp = rig.spawn("Lamp", None);
    rig.set(lamp, "light.intensity", f(800.0));
    let bulb = rig.spawn("Bulb", Some(lamp));
    rig.set(bulb, "color", t("warm"));
    let a = rig
        .invoke(
            "forge.scene.pack",
            serde_json::json!({"entity": lamp.0, "id": "lamp"}),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let root = rig.root("lamp");
    assert_eq!(root, lamp, "the branch itself became the scene");
    let inst = rig
        .bus
        .project()
        .roots()
        .find(|r| model::instance_scene(rig.bus.project(), *r).as_deref() == Some("lamp"))
        .unwrap_or_else(|| panic!("the instance took the branch's place"));
    assert_eq!(rig.name(inst), "Lamp");
    assert_eq!(rig.get(rig.child(inst, "Bulb"), "color"), Some(t("warm")));
    rig.set(bulb, "color", t("cold"));
    assert_eq!(rig.get(rig.child(inst, "Bulb"), "color"), Some(t("cold")));
    if let Some(u) = rig.bus.undo_target() {
        rig.bus.undo(u).unwrap_or_else(|r| panic!("{r:?}"));
    }
    rig.bus.undo(a.txn).unwrap_or_else(|r| panic!("{r:?}"));
    assert!(model::scene_root(rig.bus.project(), "lamp").is_none());
    assert_eq!(
        rig.bus.project().entity(lamp).and_then(|d| d.parent()),
        None
    );
}

/// One fixed script of scene commands.
fn script(rig: &mut Rig) {
    tree(rig);
    let forest = rig.scene("Forest", "forest");
    for i in 0..3 {
        let k = rig.instance("tree", Some(forest), &format!("T{i}"));
        rig.set(k, "transform.position", Value::Vec3([i as f64, 0.0, 0.0]));
    }
    for i in 0..4 {
        let k = rig.instance("forest", None, &format!("F{i}"));
        rig.set(
            k,
            "transform.position",
            Value::Vec3([0.0, 0.0, 10.0 * i as f64]),
        );
    }
    rig.inherit("forest", "Dense forest", "dense");
    let leaves = rig.child(rig.root("tree"), "Leaves");
    rig.set(leaves, "color", t("gold"));
}

#[test]
fn the_same_script_composes_the_same_project() {
    let mut a = Rig::new();
    let mut b = Rig::new();
    script(&mut a);
    script(&mut b);
    assert_eq!(a.bus.project().state_hash(), b.bus.project().state_hash());
    let wire = |r: &Rig| serde_json::to_string(r.bus.project()).unwrap_or_default();
    assert_eq!(wire(&a), wire(&b), "byte-identical projects");
}

fn pos(rig: &Rig, e: EntityKey) -> [f64; 3] {
    match rig.get(e, "transform.position.local") {
        Some(Value::Vec3(v)) => v,
        other => panic!("{} has no position: {other:?}", rig.name(e)),
    }
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// An instance places its content: moving and turning the instance root moves and turns
/// every node of it, a node moved in the scene moves in every instance where it stands, and
/// a node the instance moved itself (an override) stays where it was put.
fn placement_scenario(rig: &mut Rig) -> Result<(), String> {
    let lamp = rig.scene("Lamp", "lamp");
    rig.set(
        lamp,
        "transform.position.local",
        Value::Vec3([0.0, 0.0, 0.0]),
    );
    let pole = rig.spawn("Pole", Some(lamp));
    rig.set(
        pole,
        "transform.position.local",
        Value::Vec3([0.0, 1.0, 0.0]),
    );
    let arm = rig.spawn("Arm", Some(lamp));
    rig.set(
        arm,
        "transform.position.local",
        Value::Vec3([0.0, 2.0, 1.0]),
    );
    let inst = rig.instance("lamp", None, "Lamp 1");
    let (i_pole, i_arm) = (rig.child(inst, "Pole"), rig.child(inst, "Arm"));
    claim!(
        near(pos(rig, i_arm), [0.0, 2.0, 1.0]),
        "an unplaced instance holds the scene's layout"
    );
    // Move and turn the instance: yaw 90 turns +Z into +X.
    rig.set(
        inst,
        "transform.position.local",
        Value::Vec3([10.0, 0.0, 0.0]),
    );
    rig.set(inst, "transform.yaw", f(90.0));
    claim!(
        near(pos(rig, i_pole), [10.0, 1.0, 0.0]),
        "the pole moved with the instance: {:?}",
        pos(rig, i_pole)
    );
    claim!(
        near(pos(rig, i_arm), [11.0, 2.0, 0.0]),
        "the arm moved and turned with the instance: {:?}",
        pos(rig, i_arm)
    );
    claim!(
        model::overrides(rig.bus.project(), i_arm).is_some_and(|o| o.is_empty()),
        "placing an instance overrides nothing inside it"
    );
    // Move a node in the scene: it moves in the instance, where the instance stands.
    rig.set(
        arm,
        "transform.position.local",
        Value::Vec3([0.0, 2.0, 2.0]),
    );
    claim!(
        near(pos(rig, i_arm), [12.0, 2.0, 0.0]),
        "a scene edit lands placed: {:?}",
        pos(rig, i_arm)
    );
    // The instance moves its pole itself: an override, kept through scene edits and moves.
    rig.set(
        i_pole,
        "transform.position.local",
        Value::Vec3([5.0, 5.0, 5.0]),
    );
    rig.set(
        pole,
        "transform.position.local",
        Value::Vec3([0.0, 1.5, 0.0]),
    );
    rig.set(
        inst,
        "transform.position.local",
        Value::Vec3([20.0, 0.0, 0.0]),
    );
    claim!(
        near(pos(rig, i_pole), [5.0, 5.0, 5.0]),
        "an overridden position stays: {:?}",
        pos(rig, i_pole)
    );
    claim!(
        near(pos(rig, i_arm), [22.0, 2.0, 0.0]),
        "the rest follows the move: {:?}",
        pos(rig, i_arm)
    );
    // Revert puts it back where the instance places it.
    rig.invoke(
        "forge.scene.revert",
        serde_json::json!({"entity": i_pole.0, "path": "transform.position.local"}),
    )
    .map_err(|e| e.to_string())?;
    claim!(
        near(pos(rig, i_pole), [20.0, 1.5, 0.0]),
        "revert places the base value: {:?}",
        pos(rig, i_pole)
    );
    // Nested: a lamp placed inside a Street scene, the street placed in the world.
    let street = rig.scene("Street", "street");
    let corner = rig.instance("lamp", Some(street), "Corner lamp");
    rig.set(
        corner,
        "transform.position.local",
        Value::Vec3([5.0, 0.0, 0.0]),
    );
    let town = rig.instance("street", None, "Main street");
    rig.set(
        town,
        "transform.position.local",
        Value::Vec3([100.0, 0.0, 0.0]),
    );
    claim!(
        near(
            pos(rig, rig.at(town, &["Corner lamp", "Pole"])),
            [105.0, 1.5, 0.0]
        ),
        "nested placements compose: {:?}",
        pos(rig, rig.at(town, &["Corner lamp", "Pole"]))
    );
    Ok(())
}

#[test]
fn an_instance_places_its_content() {
    let mut rig = Rig::new();
    placement_scenario(&mut rig).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_deriver_that_does_not_place_fails() {
    let mut rig = Rig::with_faults(SceneFaults {
        no_propagation: true,
        ..SceneFaults::default()
    });
    let e = placement_scenario(&mut rig).expect_err("without re-placing the scenario fails");
    assert!(e.contains("moved"), "{e}");
}

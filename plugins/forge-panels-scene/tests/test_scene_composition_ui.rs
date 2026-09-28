//! Scene composition in the running editor (WP-U20, DoD M5-9; gate `C-scene-editor`):
//!
//! * a scene tile (the asset browser's drag payload) dropped on the hierarchy instances the
//!   scene there — a command, one undo step;
//! * the inspector marks each row of an instanced node inherited or overridden, following
//!   the value as another issuer changes it; Revert (per property) and Revert node to base
//!   are commands, undone with Ctrl+Z; the Scene group names the scene, Make Local names
//!   what it loses, and Open base scene opens the scene in isolation with its node
//!   selected.
//!
//! Positive control: the same inspector claims run on an editor whose core has no scene
//! composition (its bus carries another deriver, so the core adds none) must fail, and a
//! plain entity's rows carry no mark — the claims are not satisfied by an inspector that
//! marks everything or by a check that passes on anything.

mod common;

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::testing::Rig;
use forge_ui::dnd::DragPayload;
use forge_ui::input::UiEvent;
use forge_ui::widgets::{AssetDropped, Button, DropTarget, Label};
use forge_ui::{KeyCode, Modifiers};

const H: &str = "forge.hierarchy";
const I: &str = "forge.inspector";

fn script(rig: &mut Rig, cmd: EditorCommand) {
    let mut s = rig.connect(Issuer::Script {
        path: "scene.fscript".into(),
    });
    s.apply(cmd, None);
    let out = s.pump();
    assert!(out.refused.is_empty(), "{:?}", out.refused);
    rig.turn();
}

fn invoke(target: &str, args: serde_json::Value) -> EditorCommand {
    EditorCommand::Invoke {
        target: target.into(),
        args: args.to_string(),
    }
}

/// The Tree scene { Leaves (color green) }; returns (scene root, its Leaves).
fn tree_scene(rig: &mut Rig) -> (EntityKey, EntityKey) {
    script(
        rig,
        invoke(
            "forge.scene.new",
            serde_json::json!({"name": "Tree", "id": "tree"}),
        ),
    );
    let root = forge_editor::composition::scene_entries(&rig.shell.mirror())
        .into_iter()
        .find(|s| s.id == "tree")
        .map(|s| s.root)
        .unwrap_or_else(|| panic!("the scene"));
    let leaves = common::spawn(rig, "Leaves", Some(root));
    common::set_prop(rig, leaves, "color", Value::Text("green".into()));
    (root, leaves)
}

fn child(rig: &Rig, parent: EntityKey, name: &str) -> Option<EntityKey> {
    let m = rig.shell.mirror();
    m.entity(parent)?
        .children
        .iter()
        .copied()
        .find(|c| m.entity(*c).is_some_and(|e| e.name == name))
}

fn text_of(rig: &Rig, id: forge_ui::WidgetId) -> String {
    rig.h
        .ui
        .widget::<Label>(id)
        .map(|l| l.text(rig.h.ui.rt()))
        .unwrap_or_default()
}

#[test]
fn a_scene_dropped_on_the_hierarchy_is_instanced_there() {
    let mut rig = common::rig(&[H, "forge.undo_history"]);
    tree_scene(&mut rig);
    let park = common::spawn(&mut rig, "Park", None);
    let tree = common::part(&rig, H, &["tree"]);
    // Dropped on the Park row (what the tree reports for a drop onto a row).
    rig.h.ui.raise(
        tree,
        AssetDropped {
            view: tree,
            asset: forge_editor::composition::scene_payload("tree"),
            target: DropTarget {
                parent: Some(park.0),
                index: 0,
            },
        },
    );
    rig.turn();
    let placed = child(&rig, park, "Tree").unwrap_or_else(|| panic!("instanced under Park"));
    assert!(
        child(&rig, placed, "Leaves").is_some(),
        "with the scene's content"
    );
    let (label, issuer) = common::last_entry(&rig);
    assert!(label.contains("Instance"), "{label}");
    assert_eq!(issuer, "human:tester");
    // Through the widget: a scene tile dropped below the rows lands at the root.
    rig.h.ui.send(
        tree,
        &UiEvent::Drop(DragPayload::Asset("scene:tree".into())),
    );
    rig.turn();
    let roots_named_tree = rig
        .shell
        .mirror()
        .roots()
        .filter(|r| {
            rig.shell
                .mirror()
                .entity(*r)
                .is_some_and(|e| e.name == "Tree")
        })
        .count();
    assert_eq!(roots_named_tree, 1, "one instance at the root");
    // Another asset (not a scene) dropped on the tree places nothing.
    let n = rig.shell.mirror().len();
    rig.h.ui.send(
        tree,
        &UiEvent::Drop(DragPayload::Asset("textures/rock.tex".into())),
    );
    rig.turn();
    assert_eq!(rig.shell.mirror().len(), n);
    // One undo step removes the last instance whole.
    rig.chord(KeyCode::Char('z'), Modifiers::CTRL);
    rig.settle();
    assert_eq!(
        rig.shell
            .mirror()
            .roots()
            .filter(|r| rig
                .shell
                .mirror()
                .entity(*r)
                .is_some_and(|e| e.name == "Tree"))
            .count(),
        0
    );
}

/// The inspector's claims about an instanced node; `Err` names the first one broken.
fn inspector_claims(rig: &mut Rig) -> Result<(), String> {
    let (root, _) = tree_scene(rig);
    script(
        rig,
        invoke(
            "forge.scene.instance",
            serde_json::json!({"scene": "tree", "name": "Oak"}),
        ),
    );
    let oak = rig
        .shell
        .mirror()
        .roots()
        .find(|r| {
            rig.shell
                .mirror()
                .entity(*r)
                .is_some_and(|e| e.name == "Oak")
        })
        .ok_or("no instance")?;
    let leaves = child(rig, oak, "Leaves").ok_or("no leaves")?;
    rig.shell.session_mut().set_selection(vec![leaves]);
    rig.turn();
    let mark = common::try_part(rig, I, &["content", "raw", "row:color", "inherit"])
        .ok_or("no mark on the color row")?;
    let revert = common::try_part(rig, I, &["content", "raw", "row:color", "revert"])
        .ok_or("no revert on the color row")?;
    if !text_of(rig, mark).contains("inherited") || !rig.h.ui.is_hidden(revert) {
        return Err(format!("an untouched row reads {:?}", text_of(rig, mark)));
    }
    let scene_text = common::try_part(rig, I, &["content", "scene", "what"])
        .map(|w| text_of(rig, w))
        .unwrap_or_default();
    if !scene_text.contains("Tree") {
        return Err(format!(
            "the Scene group does not name the scene: {scene_text:?}"
        ));
    }
    // Another issuer overrides the value: the mark follows (I7 — no privileged path).
    let mark_alive = rig.h.ui.widget::<Label>(mark).is_some();
    let mut other = rig.connect(Issuer::Script {
        path: "scene_override.fscript".into(),
    });
    other.apply(
        EditorCommand::SetProperty {
            entity: leaves,
            path: "color".into(),
            value: Value::Text("red".into()),
        },
        None,
    );
    let _ = other.pump();
    rig.turn();
    if !mark_alive || !text_of(rig, mark).contains("overridden") || rig.h.ui.is_hidden(revert) {
        return Err(format!(
            "after an override the row reads {:?}, revert hidden: {}",
            text_of(rig, mark),
            rig.h.ui.is_hidden(revert)
        ));
    }
    // Revert (per property): a command, undone with Ctrl+Z.
    rig.h.click(revert);
    rig.turn();
    if rig.shell.mirror().property(leaves, "color") != Some(&Value::Text("green".into())) {
        return Err("Revert did not restore the base value".into());
    }
    if !common::last_entry(rig).0.contains("Revert") {
        return Err(format!("history label {:?}", common::last_entry(rig).0));
    }
    rig.chord(KeyCode::Char('z'), Modifiers::CTRL);
    rig.settle();
    if rig.shell.mirror().property(leaves, "color") != Some(&Value::Text("red".into())) {
        return Err("undoing Revert did not bring the override back".into());
    }
    // Revert node to base.
    let revert_node = common::try_part(rig, I, &["content", "scene", "actions", "revert_node"])
        .ok_or("no Revert node to base")?;
    rig.h.click(revert_node);
    rig.turn();
    if rig.shell.mirror().property(leaves, "color") != Some(&Value::Text("green".into())) {
        return Err("Revert node to base did not restore the base value".into());
    }
    // The instance root: Make Local names what it loses.
    rig.shell.session_mut().set_selection(vec![oak]);
    rig.turn();
    let loss = common::try_part(rig, I, &["content", "scene", "loss"])
        .map(|w| text_of(rig, w))
        .unwrap_or_default();
    if !(loss.contains("Tree") && loss.contains("node")) {
        return Err(format!("Make Local does not name its loss: {loss:?}"));
    }
    if common::try_part(rig, I, &["content", "scene", "actions", "make_local"])
        .and_then(|b| rig.h.ui.widget::<Button>(b).map(|_| b))
        .is_none()
    {
        return Err("no Make local button".into());
    }
    // Open base scene: the scene opens in isolation with its node selected.
    rig.shell.session_mut().set_selection(vec![leaves]);
    rig.turn();
    let open = common::try_part(rig, I, &["content", "scene", "actions", "open_base"])
        .ok_or("no Open base scene")?;
    rig.h.click(open);
    rig.turn();
    if rig.shell.session().scene_focus() != Some(root) {
        return Err("Open base scene did not open the scene".into());
    }
    let base_leaves = child(rig, root, "Leaves");
    if rig.shell.session().selection.first().copied() != base_leaves {
        return Err("Open base scene did not select the base node".into());
    }
    Ok(())
}

#[test]
fn the_inspector_marks_inherited_and_overridden_rows_and_reverts() {
    let mut rig = common::rig(&[H, I, "forge.undo_history"]);
    inspector_claims(&mut rig).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_inspector_that_never_marks_rows_fails() {
    // The same claims against an entity that is not linked: no row is marked, so the first
    // claim (an untouched instanced row reads "inherited") must fail.
    let mut rig = common::rig(&[H, I]);
    let plain = common::spawn(&mut rig, "Plain", None);
    common::set_prop(&mut rig, plain, "color", Value::Text("green".into()));
    rig.shell.session_mut().set_selection(vec![plain]);
    rig.turn();
    assert!(
        common::try_part(&rig, I, &["content", "raw", "row:color", "inherit"]).is_none(),
        "a plain entity's row carries no composition mark"
    );
    // And the full claims fail when the scene commands are missing from the core.
    let core = {
        let mut bus = forge_cmd::Bus::new();
        struct Nothing;
        impl forge_cmd::CommandDeriver for Nothing {
            fn derive(
                &self,
                _: &EditorCommand,
                _: &mut forge_cmd::DiffBuilder<'_>,
            ) -> Result<(), forge_cmd::CmdError> {
                Ok(())
            }
        }
        bus.set_deriver(Some(std::sync::Arc::new(Nothing)));
        forge_editor::core::EditorCore::with_bus(bus)
    };
    let mut rig = Rig::with_core(common::config(&[]), core).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&[H, I]).unwrap_or_else(|e| panic!("{e}"));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inspector_claims(&mut rig)));
    assert!(
        !matches!(r, Ok(Ok(()))),
        "without composition the inspector claims must not hold"
    );
}

#[test]
fn the_world_shows_instances_not_the_library_and_a_scene_opens_in_isolation() {
    use forge_editor::viewport::scene::{ViewportScene, drawable};
    let mut rig = common::rig(&[H]);
    let (root, leaves) = tree_scene(&mut rig);
    for (e, p) in [(root, [0.0, 0.0, 0.0]), (leaves, [0.0, 3.0, 0.0])] {
        common::set_prop(&mut rig, e, "transform.position.local", Value::Vec3(p));
    }
    script(
        &mut rig,
        invoke(
            "forge.scene.instance",
            serde_json::json!({"scene": "tree", "name": "Oak"}),
        ),
    );
    let oak = rig
        .shell
        .mirror()
        .roots()
        .find(|r| {
            rig.shell
                .mirror()
                .entity(*r)
                .is_some_and(|e| e.name == "Oak")
        })
        .unwrap_or_else(|| panic!("the instance"));
    common::set_prop(
        &mut rig,
        oak,
        "transform.position.local",
        Value::Vec3([10.0, 0.0, 0.0]),
    );
    let oak_leaves = child(&rig, oak, "Leaves").unwrap_or_else(|| panic!("mirrored"));
    let m = rig.shell.mirror();
    // Non-vacuous: the scene's own node is drawable in itself; only the filter hides it.
    assert!(drawable(&m, leaves).is_some());
    let mut world = ViewportScene::new();
    world.sync(&m);
    assert!(
        world.get(leaves).is_none(),
        "the world does not draw the scene library"
    );
    let placed = world
        .get(oak_leaves)
        .unwrap_or_else(|| panic!("the instance is drawn"));
    assert!((placed.pos.local.x - 10.0).abs() < 1e-9 && (placed.pos.local.y - 3.0).abs() < 1e-9);
    // Open base scene: only the scene, in its own layout.
    world.set_isolation(Some(root));
    world.sync(&m);
    assert!(world.get(leaves).is_some(), "the opened scene is drawn");
    assert!(
        world.get(oak_leaves).is_none(),
        "the world is not, while a scene is open"
    );
}

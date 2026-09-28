//! `test_authoring_a11y` (Ch.21 §21.10, gate row `C-authoring-editors-a11y`; DoD
//! M2-63..M2-65): every interactive widget of the three authoring editors — with content,
//! a blend state selected so the blend view shows — reaches assistive technology with a
//! role, a name and the Focus action, and the custom widgets (timeline, blend view, graph)
//! have their own roles.
//!
//! Positive control (W2): `positive_control_an_unnamed_widget_fails` — a focusable widget
//! with no name added to a panel fails the audit.

mod common;

use accesskit::Role;
use forge_cmd::Value;
use forge_editor::testing::Rig;
use forge_ui::widgets::Button;
use forge_ui::{NodeStyle, WidgetId};

fn under(rig: &Rig, root: WidgetId, id: WidgetId) -> bool {
    let mut cur = Some(id);
    while let Some(c) = cur {
        if c == root {
            return true;
        }
        cur = rig.h.ui.parent(c);
    }
    false
}

/// Audit the visible interactive widgets under `root`; the roles seen.
fn audit(rig: &mut Rig, root: WidgetId) -> Result<Vec<Role>, String> {
    let tree = rig.h.ui.a11y_activate();
    rig.settle();
    let mut roles = Vec::new();
    for (id, _) in rig.h.ui.walk() {
        if !under(rig, root, id) || !rig.h.ui.is_visible(id) {
            continue;
        }
        let node = rig.h.ui.a11y_node(id).cloned().or_else(|| {
            tree.nodes
                .iter()
                .find(|(n, _)| *n == id.to_accesskit())
                .map(|(_, n)| n.clone())
        });
        let Some(node) = node else {
            if rig.h.ui.is_focusable(id) {
                return Err(format!("interactive widget {id:?} has no a11y node"));
            }
            continue;
        };
        if node.role() == Role::Unknown {
            return Err(format!("{id:?} has no role"));
        }
        roles.push(node.role());
        if !rig.h.ui.is_focusable(id) {
            continue;
        }
        let name = node.label().unwrap_or("").trim().to_string();
        if name.is_empty() {
            return Err(format!("{id:?} ({:?}) has no accessible name", node.role()));
        }
        if !node.supports_action(accesskit::Action::Focus) {
            return Err(format!("{id:?} is focusable but does not expose Focus"));
        }
    }
    Ok(roles)
}

fn audit_open(rig: &mut Rig, p: &str, roles: &mut Vec<Role>) {
    let root = rig
        .panel_frame(p)
        .unwrap_or_else(|| panic!("{p} is not open"));
    roles.extend(audit(rig, root).unwrap_or_else(|e| panic!("{p}: {e}")));
}

/// The three editors with content in them, each audited as it is shown (the state machine
/// with a blend state selected, so the blend view is on screen).
fn populated_audit() -> Vec<Role> {
    let mut roles = Vec::new();
    let (mut rig, _) = common::sequencer_with_keys(Default::default());
    audit_open(&mut rig, "forge.sequencer", &mut roles);
    // A machine with a float parameter and a 1D blend state, selected.
    rig.show_panels(&["forge.anim_graph"])
        .unwrap_or_else(|e| panic!("{e}"));
    rig.settle();
    let p = "forge.anim_graph";
    let n = common::part(&rig, p, &["bar", "name"]);
    common::type_into(&mut rig, n, "Loco");
    let b = common::part(&rig, p, &["bar", "new_machine"]);
    common::press(&mut rig, b);
    common::type_into(&mut rig, n, "speed");
    let b = common::part(&rig, p, &["bar", "add_param"]);
    common::press(&mut rig, b);
    common::type_into(&mut rig, n, "Move");
    let b = common::part(&rig, p, &["bar", "add_1d"]);
    common::press(&mut rig, b);
    assert_eq!(
        common::setting(&rig, "anim.machine.loco.state.move.blend"),
        Some(Value::Text("1D".into()))
    );
    let canvas = common::part(&rig, p, &["body", "canvas", "canvas"]);
    let mv = rig
        .h
        .ui
        .widget::<forge_ui::widgets::NodeCanvas>(canvas)
        .and_then(|w| {
            w.model()
                .nodes()
                .find(|(_, n)| n.title.starts_with("Move"))
                .map(|(k, _)| *k)
        })
        .unwrap_or_else(|| panic!("Move node"));
    common::raise(
        &mut rig,
        canvas,
        forge_ui::widgets::CanvasSelection {
            canvas,
            nodes: vec![mv],
        },
    );
    let blend = common::part(&rig, p, &["body", "right", "blend"]);
    assert!(rig.h.ui.is_visible(blend), "the blend view shows");
    audit_open(&mut rig, p, &mut roles);
    rig.show_panels(&["forge.localisation"])
        .unwrap_or_else(|e| panic!("{e}"));
    rig.settle();
    let p = "forge.localisation";
    for (f, text) in [("tag", "fr-FR"), ("locale_name", "Fr")] {
        let w = common::part(&rig, p, &["bar", f]);
        common::type_into(&mut rig, w, text);
    }
    let b = common::part(&rig, p, &["bar", "add_locale"]);
    common::press(&mut rig, b);
    for (f, text) in [("key", "menu.play"), ("text", "Play")] {
        let w = common::part(&rig, p, &["bar", f]);
        common::type_into(&mut rig, w, text);
    }
    let b = common::part(&rig, p, &["bar", "add"]);
    common::press(&mut rig, b);
    audit_open(&mut rig, p, &mut roles);
    roles
}

#[test]
fn test_authoring_a11y() {
    let roles = populated_audit();
    for want in [
        Role::Group,
        Role::Canvas,
        Role::Toolbar,
        Role::ListBox,
        Role::Button,
        Role::TextInput,
    ] {
        assert!(
            roles.contains(&want),
            "no {want:?} among the authoring editors' widgets: {roles:?}"
        );
    }
}

#[test]
fn positive_control_an_unnamed_widget_fails() {
    let mut rig = common::rig(&["forge.localisation"]);
    let root = rig
        .panel_frame("forge.localisation")
        .unwrap_or_else(|| panic!("not open"));
    let id = rig
        .h
        .ui
        .add(
            root,
            "unnamed",
            NodeStyle::leaf().min_size(40.0, 20.0),
            Button::new(""),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    rig.settle();
    assert!(rig.h.ui.is_visible(id));
    let e = audit(&mut rig, root).err().unwrap_or_default();
    assert!(
        e.contains("no accessible name"),
        "an unnamed focusable widget passed: {e:?}"
    );
}

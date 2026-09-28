//! `test_domain_a11y` (Ch.21 §21.10, gate row `C-domain-editors-a11y`; DoD M2-66..M2-68):
//! every interactive widget of the three domain editors — every tab of the 2D editors —
//! reaches assistive technology with a role, a name and the Focus action, and the custom
//! widgets (canvas, palette, sheet preview, rig view, meters, spatial pad, bind capture)
//! have their own roles.
//!
//! Positive control (W2): `positive_control_an_unnamed_widget_fails` — a focusable widget
//! with no name added to a panel fails the audit.

mod common;

use accesskit::Role;
use forge_editor::testing::Rig;
use forge_ui::widgets::Tabs;
use forge_ui::{KeyCode, Modifiers, NodeStyle, WidgetId};

/// Is `id` under `root`?
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

fn all_panels(rig: &mut Rig) -> Result<Vec<Role>, String> {
    let mut roles = Vec::new();
    for p in forge_panels_domain::panel_ids() {
        rig.show_panels(&[p]).map_err(|e| e.to_string())?;
        rig.settle();
        let root = rig.panel_frame(p).ok_or(format!("{p} did not open"))?;
        if p == "forge.editors_2d" {
            let tabs = common::part(rig, p, &["tabs"]);
            let n = rig
                .h
                .ui
                .widget::<Tabs>(tabs)
                .map_or(0, |t| t.titles().len());
            for i in 0..n {
                if i > 0 {
                    rig.h.ui.set_focus(Some(tabs), true);
                    rig.chord(KeyCode::Right, Modifiers::NONE);
                    rig.settle();
                }
                roles.extend(audit(rig, root).map_err(|e| format!("{p} tab {i}: {e}"))?);
            }
        } else {
            roles.extend(audit(rig, root).map_err(|e| format!("{p}: {e}"))?);
        }
    }
    Ok(roles)
}

#[test]
fn test_domain_a11y() {
    let mut rig = common::rig(&[]);
    let roles = all_panels(&mut rig).unwrap_or_else(|e| panic!("{e}"));
    for want in [
        Role::Grid,
        Role::Image,
        Role::Group,
        Role::Canvas,
        Role::Slider,
        Role::Tree,
        Role::Toolbar,
    ] {
        assert!(
            roles.contains(&want),
            "no {want:?} among the domain editors' widgets"
        );
    }
}

#[test]
fn positive_control_an_unnamed_widget_fails() {
    let mut rig = common::rig(&["forge.input_map"]);
    let root = rig
        .panel_frame("forge.input_map")
        .unwrap_or_else(|| panic!("not open"));
    let empty = rig.h.ui.rt_mut().signal(String::new());
    let input = rig.shell.handles().services().input.clone();
    let id = rig
        .h
        .ui
        .add(
            root,
            "unnamed",
            NodeStyle::leaf().min_size(40.0, 20.0),
            forge_panels_domain::widgets::BindCapture::new(input, empty),
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

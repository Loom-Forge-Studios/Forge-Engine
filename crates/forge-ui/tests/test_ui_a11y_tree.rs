//! `test_ui_a11y_tree` (Ch.21 §21.10, §21.23; DoD M2-22).
//!
//! * Every interactive widget in the gallery has a role and a non-empty name, and every
//!   icon-only button has a label.
//! * The tree's focus matches UI focus.
//! * The tree is built lazily (nothing before an assistive technology connects) and
//!   updated incrementally (only changed nodes are sent).
//!
//! Positive control: an unlabelled icon button fails.

use forge_ui::gallery::{self, Gallery, GalleryFaults};
use forge_ui::testing::Harness;
use forge_ui::{Role, UiConfig, WidgetId};

fn harness(faults: GalleryFaults) -> (Harness, Gallery) {
    let mut h = Harness::new(UiConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    let g = gallery::build(&mut h.ui, faults).unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    (h, g)
}

/// The rules, over the tree an assistive technology received.
fn audit(h: &mut Harness, g: &Gallery) -> Result<usize, String> {
    let tree = h.ui.a11y_activate();
    if tree.tree.is_none() {
        return Err("the activation update carries no tree info".into());
    }
    let mut checked = 0;
    for (id, depth) in h.ui.walk() {
        let _ = depth;
        if !h.ui.is_visible(id) || !h.ui.is_focusable(id) {
            continue;
        }
        let node = tree
            .nodes
            .iter()
            .find(|(n, _)| *n == id.to_accesskit())
            .map(|(_, n)| n)
            .ok_or_else(|| format!("interactive widget {id:?} has no a11y node"))?;
        if node.role() == Role::Unknown {
            return Err(format!("{id:?} has no role"));
        }
        let name = node.label().unwrap_or("").trim().to_string();
        if name.is_empty() {
            return Err(format!("{id:?} ({:?}) has no accessible name", node.role()));
        }
        if !node.supports_action(accesskit::Action::Focus) {
            return Err(format!("{id:?} is focusable but does not expose Focus"));
        }
        checked += 1;
    }
    if checked < g.interactive.len() {
        return Err(format!(
            "only {checked} of {} interactive widgets audited",
            g.interactive.len()
        ));
    }
    Ok(checked)
}

#[test]
fn every_interactive_widget_has_a_role_and_a_name() {
    let (mut h, g) = harness(GalleryFaults::default());
    let n = audit(&mut h, &g).unwrap_or_else(|e| panic!("{e}"));
    assert!(n >= 10, "only {n} widgets audited");
    let close =
        h.ui.a11y_node(g.close_icon)
            .and_then(|n| n.label().map(str::to_string));
    assert_eq!(
        close.as_deref(),
        Some("Close"),
        "icon-only buttons are labelled"
    );
}

#[test]
fn positive_control_unlabelled_icon_button_fails() {
    let (mut h, g) = harness(GalleryFaults {
        unlabelled_icon_button: true,
        ..GalleryFaults::default()
    });
    let e = audit(&mut h, &g).expect_err("an unlabelled icon button must fail");
    assert!(e.contains("no accessible name"), "{e}");
}

#[test]
fn tree_focus_matches_ui_focus() {
    let (mut h, g) = harness(GalleryFaults::default());
    let full = h.ui.a11y_activate();
    assert_eq!(
        full.focus,
        WidgetId::ROOT.to_accesskit(),
        "nothing focused yet: the root"
    );
    h.ui.set_focus(Some(g.slider), true);
    h.step();
    let up =
        h.ui.a11y_take_update()
            .expect("focus change produces an update");
    assert_eq!(up.focus, g.slider.to_accesskit());
    // An AT-requested focus moves UI focus.
    h.ui.a11y_action(&accesskit::ActionRequest {
        action: accesskit::Action::Focus,
        target_tree: accesskit::TreeId::ROOT,
        target_node: g.save.to_accesskit(),
        data: None,
    });
    h.step();
    assert_eq!(h.ui.focused(), Some(g.save));
    assert_eq!(
        h.ui.a11y_take_update().map(|u| u.focus),
        Some(g.save.to_accesskit())
    );
}

#[test]
fn tree_is_lazy_and_incremental() {
    let (mut h, g) = harness(GalleryFaults::default());
    assert_eq!(
        h.ui.a11y_nodes_built(),
        0,
        "no a11y work before an AT connects"
    );
    assert!(h.ui.a11y_take_update().is_none());
    h.ui.a11y_activate();
    h.step();
    let _ = h.ui.a11y_take_update();
    // Toggle the checkbox: the update carries the checkbox node and nothing else.
    let v = g.snap.get(h.ui.rt());
    g.snap.set(h.ui.rt_mut(), !v);
    h.step();
    let up =
        h.ui.a11y_take_update()
            .expect("a state change produces an update");
    let ids: Vec<u64> = up.nodes.iter().map(|(id, _)| id.0).collect();
    assert_eq!(
        ids,
        vec![g.checkbox.0],
        "incremental update carried {ids:?}"
    );
    let toggled = up.nodes[0].1.toggled();
    assert_eq!(
        toggled,
        Some(if v {
            accesskit::Toggled::False
        } else {
            accesskit::Toggled::True
        })
    );
    // Idle: nothing changes, nothing is sent.
    h.advance(std::time::Duration::from_secs(1));
    assert!(h.ui.a11y_take_update().is_none());
}

// ---- specific roles: toasts and the quaternion editor --------------------------------------

/// A toast is a live region with a role that says how urgent it is: success and info are a
/// polite `Status`, warnings and errors an assertive `Alert` (Ch.21 §21.10, §21.16).
#[test]
fn toasts_are_live_regions_with_an_urgency_role() {
    use forge_ui::overlay::Severity;
    let (mut h, _host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let ok =
        h.ui.toast("Saved", Severity::Success)
            .unwrap_or_else(|e| panic!("{e}"));
    let bad =
        h.ui.toast("Build failed", Severity::Error)
            .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let okn = h
        .node(ok)
        .unwrap_or_else(|| panic!("the success toast has no a11y node"));
    assert_eq!(okn.role(), Role::Status);
    assert_eq!(okn.live(), Some(accesskit::Live::Polite));
    assert!(
        okn.label().is_some_and(|l| l.contains("Saved")),
        "{:?}",
        okn.label()
    );
    let badn = h
        .node(bad)
        .unwrap_or_else(|| panic!("the error toast has no a11y node"));
    assert_eq!(badn.role(), Role::Alert);
    assert_eq!(badn.live(), Some(accesskit::Live::Assertive));
    assert!(
        badn.label().is_some_and(|l| l.contains("Build failed")),
        "{:?}",
        badn.label()
    );
}

/// The quaternion editor: a named group, three named spin buttons with their unit, and a
/// gimbal warning that is a polite live `Status` carrying the warning text — present in the
/// tree only while it shows.
#[test]
fn quaternion_editor_fields_and_gimbal_warning_have_their_roles() {
    use forge_frames::DQuat;
    use forge_ui::widgets::{euler_to_quat, quat_editor};
    let (mut h, host) = Harness::with_host().unwrap_or_else(|e| panic!("{e}"));
    let q = h.ui.rt_mut().signal(DQuat::IDENTITY);
    let p = quat_editor(&mut h.ui, host, "q", q, "Rotation").unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    let ed = h.node(p.editor).unwrap_or_else(|| panic!("no editor node"));
    assert_eq!(ed.role(), Role::Group);
    assert_eq!(ed.label(), Some("Rotation"));
    for (id, name) in [(p.yaw, "yaw"), (p.pitch, "pitch"), (p.roll, "roll")] {
        let n = h.node(id).unwrap_or_else(|| panic!("no {name} node"));
        assert_eq!(n.role(), Role::SpinButton, "{name}");
        let label = n.label().unwrap_or("");
        assert!(
            label.contains("Rotation") && label.contains(name),
            "{name}: {label:?}"
        );
        assert!(n.supports_action(accesskit::Action::Focus), "{name}");
        assert!(n.numeric_value().is_some(), "{name} exposes its value");
    }
    assert!(
        h.node(p.warning).is_none(),
        "no warning node away from the pole"
    );
    q.set(h.ui.rt_mut(), euler_to_quat(0.0, 90.0, 0.0));
    h.settle();
    let w = h
        .node(p.warning)
        .unwrap_or_else(|| panic!("the gimbal warning has no a11y node at the pole"));
    assert_eq!(w.role(), Role::Status);
    assert_eq!(w.live(), Some(accesskit::Live::Polite));
    assert!(
        w.label().is_some_and(|l| l.contains("Gimbal lock")),
        "{:?}",
        w.label()
    );
}

#[test]
fn a_radio_group_exposes_each_option_as_a_radio_button() {
    // WP-U12: over AT-SPI the gallery's radio group showed no options; each option is now a
    // virtual radio button with its state, position in the set and the Click action.
    let (mut h, g) = harness(GalleryFaults::default());
    let tree = h.ui.a11y_activate();
    let radio = g.radio.to_accesskit();
    let group = tree
        .nodes
        .iter()
        .find(|(id, _)| *id == radio)
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| panic!("the radio group has no node"));
    let kids = group.children().to_vec();
    assert_eq!(kids.len(), 3, "one radio button per option");
    let options: Vec<(String, Option<accesskit::Toggled>)> = kids
        .iter()
        .filter_map(|k| tree.nodes.iter().find(|(id, _)| id == k).map(|(_, n)| n))
        .map(|n| {
            assert_eq!(n.role(), Role::RadioButton);
            assert!(n.supports_action(accesskit::Action::Click));
            (n.label().unwrap_or("").to_string(), n.toggled())
        })
        .collect();
    assert_eq!(
        options,
        vec![
            ("Local".to_string(), Some(accesskit::Toggled::False)),
            ("World".to_string(), Some(accesskit::Toggled::True)),
            ("Frame".to_string(), Some(accesskit::Toggled::False)),
        ]
    );
}

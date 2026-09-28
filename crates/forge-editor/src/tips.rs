//! First-run tips (Ch.21 §21.21 "Requirements on every panel", DoD M2-70, WP-U12).
//!
//! A panel declares one short tip ([`crate::panels::PanelCx::first_run_tip`], a localisation
//! key). The first time the panel opens, the tip shows as a bar at the top of the panel: text
//! and a dismiss button. Rules:
//!
//! * **Tips need a user config directory** to remember a dismissal in; without one (a
//!   scripted or `--no-user-config` run) no tip shows.
//! * **"Seen" is user config**, never project state: the dismissed panels are kept in the
//!   editor settings (`tips_seen`), saved with them; Settings has **First-run tips** (off:
//!   never show tips) and **Reset tips**.
//! * **A tip never takes focus and never blocks input**: nothing focuses it when it appears
//!   (its dismiss button is reachable by Tab like any other control), and it is a row in the
//!   panel's flow, not a modal or an overlay.
//! * **A dismissed tip schedules nothing**: dismissing removes the bar (one frame to show
//!   that) and leaves no timer, animation or feed behind, so the zero-idle rule holds
//!   (`test_editor_panels_audit`'s tips check, gate row `C-first-run-tips`).

use forge_ui::dock::PanelId;
use forge_ui::widgets::{Container, IconButton, Label, LabelKind, Pressed};
use forge_ui::{NodeStyle, Role, UiError};

use crate::panel_rt::PanelBuilder;
use crate::settings::EditorSettings;

/// Whether `panel`'s tip should show: tips are on, it was not dismissed, and there is a
/// user config directory to remember a dismissal in (without one — a scripted or
/// `--no-user-config` run — a tip would come back every start, so none shows).
pub fn should_show(session: &crate::session::SessionState, panel: &PanelId) -> bool {
    let s: &EditorSettings = session.settings();
    session.user_config && s.first_run_tips && !s.tip_seen(panel.as_str())
}

/// Build `panel`'s tip bar under the builder's parent (see the module docs).
pub fn build(pb: &mut PanelBuilder, panel: &PanelId, text: &str) -> Result<(), UiError> {
    let space = pb.b.theme_ref().space;
    let bar = pb.b.add(
        pb.parent,
        "first_run_tip",
        NodeStyle::row(space[2])
            .padding(space[2])
            .background(forge_ui::ColorRole::BgRaised),
        Container::new(Role::Note).labelled(forge_ui::tr!("Tip")),
    )?;
    pb.b.add(
        bar,
        "text",
        NodeStyle::leaf().grow(1.0),
        Label::new(text).kind(LabelKind::Muted).wrapping(),
    )?;
    let close = pb.b.add(
        bar,
        "dismiss",
        NodeStyle::leaf(),
        IconButton::new("\u{2715}", forge_ui::tr!("Dismiss tip")),
    )?;
    let panel = panel.clone();
    pb.on(close, move |act, _: &Pressed| {
        let id = panel.as_str().to_string();
        act.session.update_settings(|s| s.mark_tip_seen(&id));
        #[cfg(any(test, feature = "controls"))]
        if act.services.faults.tip_keeps_timer {
            // W2 control: a "dismissed" tip that keeps animating (a fade that never ends).
            let _ = act.ui.set_hidden(bar, true);
            let live = act.ui.rt_mut().signal(true);
            let _ = act.ui.add(
                act.ui.parent(bar).unwrap_or(bar),
                "first_run_tip_fade",
                NodeStyle::leaf(),
                forge_ui::widgets::Spinner::new(live, forge_ui::tr!("Tip fading")),
            );
            return;
        }
        let _ = act.ui.remove(bar);
    });
    Ok(())
}

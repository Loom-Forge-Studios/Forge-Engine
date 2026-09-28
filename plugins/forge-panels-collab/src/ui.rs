//! Building blocks the collaboration panels share: groups, bars, labels, buttons, lists,
//! and the **frame** every one of them has — a live feed relay (§21.11: refreshed at most
//! [`crate::LIVE_HZ`] times a second, only while visible) and the notice line that says why
//! nothing can be done when the editor has no team server or the licence does not allow
//! collaboration (E-57: the panel greys; nothing else is locked).

use std::sync::Arc;

use forge_editor::feed::{FeedRelay, FeedTicked};
use forge_editor::panel_rt::{PanelBuilder, PanelSync};
use forge_editor::services::EditorServices;
use forge_ui::widgets::{Button, Container, Label, LabelKind, TextField};
use forge_ui::{Bind, LiveFeed, NodeStyle, Role, Signal, Ui, UiError, WidgetId};

/// A labelled column group.
pub fn group(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: &'static str,
    label: &str,
) -> Result<WidgetId, UiError> {
    let space = pb.b.theme_ref().space;
    pb.b.add(
        parent,
        key,
        NodeStyle::column(space[1]).padding(space[1]),
        Container::new(Role::Group).labelled(label),
    )
}

/// A labelled row of controls.
pub fn bar(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: &'static str,
    label: &str,
) -> Result<WidgetId, UiError> {
    let space = pb.b.theme_ref().space;
    pb.b.add(
        parent,
        key,
        NodeStyle::row(space[1]).padding(space[1]),
        Container::new(Role::Toolbar).labelled(label),
    )
}

/// A label (static text or a signal).
pub fn text(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: &'static str,
    t: impl Into<Bind<String>>,
    kind: LabelKind,
) -> Result<WidgetId, UiError> {
    let space = pb.b.theme_ref().space;
    pb.b.add(
        parent,
        key,
        NodeStyle::leaf().padding(space[1]),
        Label::new(t).kind(kind).wrapping(),
    )
}

/// A button.
pub fn button(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: &'static str,
    label: impl Into<Bind<String>>,
) -> Result<WidgetId, UiError> {
    pb.b.add(parent, key, NodeStyle::leaf(), Button::new(label))
}

/// The panel's main button.
pub fn primary(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: &'static str,
    label: &str,
) -> Result<WidgetId, UiError> {
    pb.b.add(parent, key, NodeStyle::leaf(), Button::new(label).primary())
}

/// A single-line text field over a new signal.
pub fn field(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: &'static str,
    label: &str,
    placeholder: &str,
    value: &str,
) -> Result<Signal<String>, UiError> {
    let s = pb.b.signal(value.to_string());
    pb.b.add(
        parent,
        key,
        NodeStyle::leaf().width(260.0),
        TextField::new(s, label).placeholder(placeholder),
    )?;
    Ok(s)
}

/// Set a text signal only when it changed (no damage for an unchanged label).
pub fn set(ui: &mut Ui, s: Signal<String>, v: String) {
    if s.get(ui.rt()) != v {
        s.set(ui.rt_mut(), v);
    }
}

/// Show or hide a part.
pub fn show(ui: &mut Ui, id: WidgetId, visible: bool) {
    if ui.is_hidden(id) == visible {
        let _ = ui.set_hidden(id, !visible);
    }
}

/// A read-only list (a `VirtualTree` list) that fills its parent's width.
pub fn list(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: &'static str,
    label: &str,
    height: f32,
) -> Result<WidgetId, UiError> {
    pb.b.add(
        parent,
        key,
        NodeStyle::leaf().grow(1.0).height(height),
        forge_ui::widgets::VirtualTree::list(label).single_select(),
    )
}

/// Replace a list's rows: `(key, label, muted)`; keeps the selection where it can.
pub fn fill(ui: &mut Ui, list: WidgetId, rows: Vec<(u64, String, bool)>) {
    forge_ui::widgets::VirtualTree::edit(ui, list, |t| {
        let selected = t.selected();
        t.clear();
        for (k, label, muted) in rows {
            t.push(None, k, forge_ui::widgets::RowItem::new(label).muted(muted));
        }
        let keep: Vec<u64> = selected.into_iter().filter(|k| t.contains(*k)).collect();
        t.select(&keep);
    });
}

/// The selected key of a list.
pub fn selected(ui: &mut Ui, list: WidgetId) -> Option<u64> {
    forge_ui::widgets::VirtualTree::edit(ui, list, |t| t.selected().first().copied()).flatten()
}

/// Why the collaboration actions are unavailable now (`None`: they are available).
pub fn unavailable(services: &EditorServices) -> Option<String> {
    let c = &services.collab;
    if !c.attached() {
        return Some(
            forge_ui::tr!(
                "No team server is attached to this editor: teams and collaboration are not available here."
            )
            .into(),
        );
    }
    let l = c.licence_state();
    if l.collaboration {
        None
    } else {
        Some(forge_ui::trf!(
            "Collaboration is paused \u{2014} {why}. Open Licence to renew.",
            why = l.why_not.unwrap_or_default()
        ))
    }
}

/// The frame parts of a collaboration panel.
pub struct Frame {
    pub root: WidgetId,
    pub relay: WidgetId,
    /// Why nothing can be done (empty when everything can).
    pub notice: Signal<String>,
    pub notice_label: WidgetId,
    /// The latest outcome of this panel's commands.
    pub status: Signal<String>,
}

/// The column every collaboration panel lives in (see the module docs).
pub fn frame(pb: &mut PanelBuilder, title: &str) -> Result<Frame, UiError> {
    let space = pb.b.theme_ref().space;
    let root = pb.b.add(
        pb.parent,
        "content",
        NodeStyle::column(space[1])
            .padding(space[2])
            .grow(1.0)
            .scrollable(),
        Container::new(Role::Group).labelled(title),
    )?;
    let relay =
        pb.b.add(root, "feed", NodeStyle::leaf(), FeedRelay::new())?;
    pb.on(relay, |act, _: &FeedTicked| act.want_turn());
    let notice = pb.b.signal(String::new());
    // Why the panel has nothing it can do (no team server, no team, collaboration paused):
    // its empty state, from the catalogue widget (M2-70).
    let notice_label = pb.b.add(
        root,
        "notice",
        NodeStyle::leaf(),
        forge_ui::widgets::EmptyState::new(notice),
    )?;
    pb.b.hide(notice_label, true);
    let status = pb.b.signal(String::new());
    Ok(Frame {
        root,
        relay,
        notice,
        notice_label,
        status,
    })
}

/// Register the frame's feed the first time the sync step runs.
pub fn feed_once(s: &mut PanelSync, relay: WidgetId, done: &mut bool, hz: u8) {
    let faults = s.services.collab.faults;
    if faults.poll() {
        // W2 control only: a panel that looks every turn instead of following its feed.
        s.want_turn();
    }
    if *done {
        return;
    }
    *done = true;
    let cell = s.services.collab.feed();
    s.ui.add_feed(
        relay,
        LiveFeed {
            source: cell as Arc<dyn forge_ui::LiveSource>,
            // W2 control only: an uncapped feed.
            max_hz: if faults.feed_uncapped() { 60 } else { hz },
            self_ui: false,
        },
    );
}

/// Update the notice line; whether actions are available.
pub fn notice(ui: &mut Ui, services: &EditorServices, f: &Frame) -> bool {
    let why = unavailable(services);
    let ok = why.is_none();
    set(ui, f.notice, why.unwrap_or_default());
    show(ui, f.notice_label, !ok);
    ok
}

/// The latest outcome of any of `targets`, as a status line.
pub fn last_outcome(services: &EditorServices, targets: &[&str]) -> String {
    services
        .collab
        .status()
        .and_then(|s| {
            s.outcomes
                .iter()
                .rev()
                .find(|o| targets.contains(&o.what.as_str()))
                // The message is the core's report: the core is headless (it may run on
                // another machine, I7), so it words its outcome in the source language and
                // the panel shows it as the data of a looked-up line (ADR 0046 §6).
                .map(|o| {
                    if o.ok {
                        forge_ui::trf!("\u{2714} {message}", message = o.message)
                    } else {
                        forge_ui::trf!("\u{26a0} {message}", message = o.message)
                    }
                })
        })
        .unwrap_or_default()
}

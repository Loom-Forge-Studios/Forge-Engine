//! Small building blocks the connect panels share.

use forge_editor::panel_rt::PanelBuilder;
use forge_ui::widgets::{Button, Container, Label, LabelKind};
use forge_ui::{Bind, NodeStyle, Role, Signal, Ui, UiError, WidgetId};

/// A labelled column group.
pub fn group(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: impl Into<forge_ui::Key>,
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
    key: impl Into<forge_ui::Key>,
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
    key: impl Into<forge_ui::Key>,
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
    key: impl Into<forge_ui::Key>,
    label: &str,
) -> Result<WidgetId, UiError> {
    pb.b.add(parent, key, NodeStyle::leaf(), Button::new(label))
}

/// Set a text signal only when it changed (no damage for an unchanged label).
pub fn set(ui: &mut Ui, s: Signal<String>, v: String) {
    if s.get(ui.rt()) != v {
        s.set(ui.rt_mut(), v);
    }
}

/// Show or hide a part (errors only for a widget already gone: nothing to show).
pub fn show(ui: &mut Ui, id: WidgetId, visible: bool) {
    if ui.is_hidden(id) == visible {
        let _ = ui.set_hidden(id, !visible);
    }
}

/// A read-only list (a `VirtualTree` list) that fills its parent's width.
pub fn list(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: impl Into<forge_ui::Key>,
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

/// A list's empty state (the catalogue widget, M2-70), shown until the list has rows.
pub fn empty(
    pb: &mut PanelBuilder,
    parent: WidgetId,
    key: impl Into<forge_ui::Key>,
    message: &str,
) -> Result<WidgetId, UiError> {
    pb.b.add(
        parent,
        key,
        NodeStyle::leaf(),
        forge_ui::widgets::EmptyState::new(message),
    )
}

/// Show `empty` exactly when there is nothing else to show.
pub fn show_empty(ui: &mut Ui, empty: WidgetId, nothing: bool) {
    let _ = ui.set_hidden(empty, !nothing);
}

/// Replace a list's rows: `(key, label, muted, parent)`.
pub fn fill(ui: &mut Ui, list: WidgetId, rows: Vec<(u64, String, bool, Option<u64>)>) {
    forge_ui::widgets::VirtualTree::edit(ui, list, |t| {
        let selected = t.selected();
        t.clear();
        for (k, label, muted, parent) in rows {
            t.push(
                parent,
                k,
                forge_ui::widgets::RowItem::new(label).muted(muted),
            );
            if let Some(p) = parent {
                t.set_expanded(p, true);
            }
        }
        let keep: Vec<u64> = selected.into_iter().filter(|k| t.contains(*k)).collect();
        t.select(&keep);
    });
}

/// The selected key of a list.
pub fn selected(ui: &mut Ui, list: WidgetId) -> Option<u64> {
    forge_ui::widgets::VirtualTree::edit(ui, list, |t| t.selected().first().copied()).flatten()
}

/// Tell the person why nothing happened (a warning toast with the code).
pub fn warn(
    session: &mut forge_editor::session::SessionState,
    title: &str,
    e: &forge_editor::EditorError,
) {
    session.problem(forge_editor::notify::Problem::coded(
        forge_editor::notify::Severity::Warning,
        e.code(),
        title,
        &e.to_string(),
    ));
}

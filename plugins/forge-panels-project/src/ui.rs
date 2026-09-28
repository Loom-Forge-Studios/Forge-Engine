//! Small building blocks the lifecycle panels share.

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

/// A label shown verbatim: text a build or a file carries (never translated).
pub fn verbatim_text(
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
        Label::new(t).kind(kind).wrapping().verbatim(),
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

/// How a panel follows the project with a [`MirrorView`]: incrementally, adding its work to
/// the edit-cost probe when a test sets one (or, under the W2 fault, rebuilding the view on
/// every change — the cost the probe must catch).
#[derive(Clone)]
pub struct Follow {
    full: bool,
    probe: Option<std::rc::Rc<std::cell::Cell<usize>>>,
}

impl Follow {
    pub fn new(f: &forge_editor::services::PanelFaults) -> Self {
        Follow {
            full: f.project_view_full_rebuild(),
            probe: f.project_view_probe(),
        }
    }
    /// The W2 fault is on: follow the whole project on every change.
    pub fn always(&self) -> bool {
        self.full
    }
    /// Bring `v` up to date with `m`; whether something a dialog shows may have changed.
    pub fn update(
        &self,
        v: &mut forge_editor::project::promote::MirrorView,
        m: &forge_editor::mirror::ProjectMirror,
    ) -> bool {
        if self.full {
            *v = Default::default();
        }
        let r = v.update(m);
        self.add(r.work);
        r.changed
    }
    /// Count `n` units of work on the probe.
    pub fn add(&self, n: usize) {
        if let Some(p) = &self.probe {
            p.set(p.get() + n);
        }
    }
}

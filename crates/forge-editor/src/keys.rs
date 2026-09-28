//! The keymap in the widget tree: [`KeymapHost`] sits above everything in an editor window
//! and receives every key the focused widget (and its ancestors) did not handle. It builds
//! the focus path's contexts — the focused widget's role, the dock panel it is in, then
//! window and global — resolves the stroke through the [`KeyMap`], and raises
//! [`ActionTriggered`] for the shell, which runs the action (commands go on the bus, I7).
//! Resolution thus walks the focus path from the innermost context outwards: widgets
//! first, then role, panel, window, global (Ch.21 §21.17).

use std::cell::RefCell;
use std::rc::Rc;

use forge_ui::dock::DockTabStrip;
use forge_ui::widget::{A11yCx, EventCx, PaintCx};
use forge_ui::{Handled, Key, NodeStyle, Role, Ui, UiError, UiEvent, Widget, WidgetId};

use crate::keymap::{KeyContext, KeyMap, Resolution};

/// Action: a chord resolved to this action id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionTriggered {
    pub action: String,
}

/// Action: the first stroke of a two-stroke chord is waiting (the status bar shows it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordPending {
    pub first: String,
}

/// The keymap host widget (see the module docs). Its children are the window's content.
pub struct KeymapHost {
    keymap: Rc<RefCell<KeyMap>>,
}

impl KeymapHost {
    pub fn new(keymap: Rc<RefCell<KeyMap>>) -> Self {
        Self { keymap }
    }
}

/// Add a keymap host under `parent` and make it the window's key sink, so it also gets
/// keys while nothing is focused. The window's content goes under the returned widget.
pub fn keymap_host(
    ui: &mut Ui,
    parent: WidgetId,
    key: impl Into<Key>,
    style: NodeStyle,
    keymap: Rc<RefCell<KeyMap>>,
) -> Result<WidgetId, UiError> {
    let id = ui.add(parent, key, style, KeymapHost::new(keymap))?;
    ui.set_key_sink(Some(id));
    Ok(id)
}

/// The key contexts of the focus path at `focused`, innermost first: the focused widget's
/// role, then the dock panel it is in (a focused tab strip counts as its active panel).
pub fn focus_contexts(cx: &mut EventCx, focused: Option<WidgetId>) -> Vec<KeyContext> {
    let mut out = Vec::new();
    let Some(f) = focused else {
        return out;
    };
    if let Some(r) = cx.role_of(f) {
        out.push(KeyContext::Role(format!("{r:?}")));
    }
    let mut cur = Some(f);
    while let Some(w) = cur {
        if let Some(Key::Str(s)) = cx.key_of(w)
            && let Some(p) = s.strip_prefix("panel:")
        {
            out.push(KeyContext::Panel(p.to_string()));
            break;
        }
        if let Some(active) = cx.widget_mut::<DockTabStrip>(w).and_then(|s| {
            let panels = s.panels();
            panels.get(s.active()).cloned()
        }) {
            out.push(KeyContext::Panel(active.to_string()));
            break;
        }
        cur = cx.parent_of(w);
    }
    out
}

impl Widget for KeymapHost {
    fn role(&self) -> Role {
        Role::Group
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let UiEvent::Key(k) = ev else {
            return Handled::No;
        };
        if !k.pressed {
            return Handled::No;
        }
        let focused = cx.focused();
        let path = focus_contexts(cx, focused);
        let r = self.keymap.borrow_mut().resolve(k, &path);
        match r {
            Resolution::Action(action) => {
                cx.action(ActionTriggered { action });
                Handled::Yes
            }
            Resolution::Pending(first) => {
                cx.action(ChordPending {
                    first: first.to_string(),
                });
                Handled::Yes
            }
            Resolution::Unbound => Handled::No,
        }
    }
    fn paint(&self, _cx: &mut PaintCx) {}
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(forge_ui::tr!("Forge editor"));
    }
}

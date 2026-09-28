//! Overlays: popups (menus, dropdowns, popovers, dialogs), tooltips and toasts
//! (Ch.21 §21.9, §21.16).
//!
//! * Every overlay lives in one **overlay layer**: a full-window, pointer-transparent node
//!   that is always the root's last child, so it paints above everything and hit-tests
//!   first.
//! * A **popup** is owned by the widget that opened it. It is placed from an [`Anchor`],
//!   kept inside the window (flipped above/left when it would overflow), receives focus,
//!   and is a focus scope. An outside press or Escape closes it (a modal dialog closes
//!   only by its own buttons or Escape), focus returns to the owner, and the owner gets
//!   `UiEvent::PopupClosed`. Popups stack: a submenu is a popup of a menu.
//! * A **modal dialog** sits on a scrim that covers the window and traps focus
//!   (`FocusScope::Modal`, §21.9).
//! * **Tooltips** come from `Widget::tooltip`, after [`TOOLTIP_DELAY`] of hover or
//!   keyboard focus — one timer in the heap, nothing polled (§21.11). They take no focus
//!   and no pointer events.
//! * **Toasts** stack at the bottom right, announce themselves as a live region, expire
//!   after [`TOAST_DURATION`] (one timer each) and can be dismissed by keyboard.

use std::time::Duration;

use accesskit::Role;

use crate::UiError;

use crate::geom::{Point, Rect, Size};
use crate::id::{Key, WidgetId};
use crate::input::{Handled, KeyCode, UiEvent};
use crate::layout::{FocusScope, NodeStyle};
use crate::style::{ColorRole, Radius};
use crate::text::TextStyle;
use crate::ui::{NodeKey, Ui};
use crate::widget::{A11yCx, EventCx, MeasureCx, PaintCx, Widget};

/// Hover (or keyboard focus) this long before a tooltip shows.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);
/// A toast stays this long unless dismissed.
pub const TOAST_DURATION: Duration = Duration::from_secs(5);
/// The timer tag the overlay uses for tooltips (never a widget's own tag).
pub(crate) const TOOLTIP_TAG: u64 = u64::MAX - 7;
const TOAST_TAG: u64 = 1;

/// Where a popup goes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Anchor {
    /// Below `rect`, left-aligned (dropdowns, menus from a menu bar); flips above.
    Below(Rect),
    /// To the right of `rect`, top-aligned (submenus); flips left.
    Right(Rect),
    /// At a point (context menus); shifted inside the window.
    At(Point),
    /// Centred in the window (dialogs, the command palette).
    Center,
    /// Near the top centre of the window (the command palette).
    TopCenter,
}

/// How a popup behaves.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PopupSpec {
    pub anchor: Anchor,
    /// A modal dialog: a scrim covers the window and focus is trapped.
    pub modal: bool,
    /// Close on a press outside the popup (and outside its owner).
    pub dismiss_on_outside: bool,
    /// Escape closes it.
    pub dismiss_on_escape: bool,
    /// Move keyboard focus into the popup when it opens.
    pub take_focus: bool,
    /// Size in logical px; `None`: the popup widget's measured size.
    pub size: Option<Size>,
    /// Minimum width (a dropdown is at least as wide as its field).
    pub min_width: f32,
}

impl PopupSpec {
    /// A menu or dropdown list.
    pub fn menu(anchor: Anchor) -> Self {
        Self {
            anchor,
            modal: false,
            dismiss_on_outside: true,
            dismiss_on_escape: true,
            take_focus: true,
            size: None,
            min_width: 0.0,
        }
    }
    /// A non-modal popover panel.
    pub fn popover(anchor: Anchor, size: Size) -> Self {
        Self {
            size: Some(size),
            ..Self::menu(anchor)
        }
    }
    /// A modal dialog (centred on a scrim).
    pub fn dialog() -> Self {
        Self {
            anchor: Anchor::Center,
            modal: true,
            dismiss_on_outside: false,
            dismiss_on_escape: true,
            take_focus: true,
            size: None,
            min_width: 0.0,
        }
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    pub fn sized(mut self, s: Size) -> Self {
        self.size = Some(s);
        self
    }
}

/// Toast severity (colour is never the only carrier: each has its own word and glyph).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

impl Severity {
    fn role(self) -> ColorRole {
        match self {
            Severity::Info => ColorRole::Accent,
            Severity::Success => ColorRole::Success,
            Severity::Warning => ColorRole::Warning,
            Severity::Error => ColorRole::Danger,
        }
    }
    fn word(self) -> &'static str {
        match self {
            Severity::Info => crate::tr!("Info"),
            Severity::Success => crate::tr!("Done"),
            Severity::Warning => crate::tr!("Warning"),
            Severity::Error => crate::tr!("Error"),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Popup {
    /// The popup widget.
    pub(crate) id: WidgetId,
    /// The node removed on close (the scrim for a modal, else the popup itself).
    pub(crate) root: WidgetId,
    pub(crate) owner: WidgetId,
    pub(crate) spec: PopupSpec,
    /// Focus to restore on close.
    pub(crate) restore: Option<WidgetId>,
}

/// Place a `size` popup for `anchor` inside a `win` window.
pub fn place(anchor: Anchor, size: Size, win: Size) -> Point {
    let clamp_x = |x: f32| x.min(win.w - size.w).max(0.0);
    let clamp_y = |y: f32| y.min(win.h - size.h).max(0.0);
    match anchor {
        Anchor::Below(r) => {
            let below = r.bottom() + 2.0;
            let y = if below + size.h > win.h && r.y - size.h - 2.0 >= 0.0 {
                r.y - size.h - 2.0
            } else {
                below
            };
            Point::new(clamp_x(r.x), clamp_y(y))
        }
        Anchor::Right(r) => {
            let right = r.right() + 2.0;
            let x = if right + size.w > win.w && r.x - size.w - 2.0 >= 0.0 {
                r.x - size.w - 2.0
            } else {
                right
            };
            Point::new(clamp_x(x), clamp_y(r.y))
        }
        Anchor::At(p) => Point::new(clamp_x(p.x), clamp_y(p.y)),
        Anchor::Center => Point::new(
            clamp_x((win.w - size.w) * 0.5),
            clamp_y((win.h - size.h) * 0.5),
        ),
        Anchor::TopCenter => Point::new(clamp_x((win.w - size.w) * 0.5), clamp_y(win.h * 0.12)),
    }
}

fn absolute(at: Point, size: Size) -> NodeStyle {
    let mut s = NodeStyle::default();
    s.layout.position = taffy::Position::Absolute;
    s.layout.inset = taffy::Rect {
        left: taffy::prelude::length(at.x),
        top: taffy::prelude::length(at.y),
        right: taffy::prelude::auto(),
        bottom: taffy::prelude::auto(),
    };
    s.layout.size = taffy::Size {
        width: taffy::prelude::length(size.w),
        height: taffy::prelude::length(size.h),
    };
    s
}

impl Ui {
    /// The overlay layer (created on first use; always the root's last child).
    pub(crate) fn overlay_key(&mut self) -> Result<NodeKey, UiError> {
        if let Some(k) = self.overlay
            && self.nodes.contains_key(k)
        {
            return Ok(k);
        }
        let mut style = NodeStyle::default().no_hit_test();
        style.layout.position = taffy::Position::Absolute;
        style.layout.inset = taffy::Rect {
            left: taffy::prelude::length(0.0),
            top: taffy::prelude::length(0.0),
            right: taffy::prelude::length(0.0),
            bottom: taffy::prelude::length(0.0),
        };
        let root = self.root();
        let id = self.add_unordered(
            root,
            Key::Static("forge.overlay"),
            style,
            Box::new(OverlayLayer),
        )?;
        let k = self
            .index
            .get(&id)
            .copied()
            .ok_or(UiError::UnknownWidget(id))?;
        self.overlay = Some(k);
        Ok(k)
    }

    /// Open a popup owned by `owner` (see the module docs).
    pub fn open_popup(
        &mut self,
        owner: WidgetId,
        key: impl Into<Key>,
        spec: PopupSpec,
        widget: impl Widget,
    ) -> Result<WidgetId, UiError> {
        self.open_popup_boxed(owner, key.into(), spec, Box::new(widget))
    }

    pub(crate) fn open_popup_boxed(
        &mut self,
        owner: WidgetId,
        key: Key,
        spec: PopupSpec,
        mut widget: Box<dyn Widget>,
    ) -> Result<WidgetId, UiError> {
        let ok = self.overlay_key()?;
        let overlay = self.nodes[ok].id;
        // Re-opening the same popup replaces it.
        let probe = overlay.child(&key);
        if self.index.contains_key(&probe) {
            self.close_popup(probe);
        }
        let size = match spec.size {
            Some(s) => s,
            None => {
                let mut res = self.res.borrow_mut();
                res.text.set_scale(self.scale);
                let mut cx = MeasureCx {
                    text: &mut res.text,
                    rt: &self.rt,
                    theme: &self.theme,
                };
                widget.measure(
                    &mut cx,
                    taffy::Size {
                        width: None,
                        height: None,
                    },
                    taffy::Size {
                        width: taffy::AvailableSpace::MaxContent,
                        height: taffy::AvailableSpace::MaxContent,
                    },
                )
            }
        };
        let size = Size::new(size.w.max(spec.min_width), size.h);
        let win = self.size();
        let size = Size::new(size.w.min(win.w), size.h.min(win.h));
        let restore = self.focused;
        let (id, root) = if spec.modal {
            let mut scrim = NodeStyle::default()
                .background(ColorRole::Scrim)
                .scope(FocusScope::Modal);
            scrim.layout.position = taffy::Position::Absolute;
            scrim.layout.inset = taffy::Rect {
                left: taffy::prelude::length(0.0),
                top: taffy::prelude::length(0.0),
                right: taffy::prelude::length(0.0),
                bottom: taffy::prelude::length(0.0),
            };
            scrim.layout.display = taffy::Display::Flex;
            scrim.layout.justify_content = Some(taffy::JustifyContent::CENTER);
            scrim.layout.align_items = Some(taffy::AlignItems::CENTER);
            let sid = self.add_boxed(overlay, key, scrim, Box::new(Scrim))?;
            let mut style = NodeStyle::default()
                .background(ColorRole::BgRaised)
                .rounded(Radius::Lg);
            if spec.size.is_some() {
                style.layout.size = taffy::Size {
                    width: taffy::prelude::length(size.w),
                    height: taffy::prelude::length(size.h),
                };
            }
            let id = self.add_boxed(sid, Key::Static("dialog"), style, widget)?;
            (id, sid)
        } else {
            let at = place(spec.anchor, size, win);
            let style = absolute(at, size).scope(FocusScope::Panel);
            let id = self.add_boxed(overlay, key, style, widget)?;
            (id, id)
        };
        self.popups.push(crate::overlay::Popup {
            id,
            root,
            owner,
            spec,
            restore,
        });
        self.hide_tooltip();
        if spec.take_focus {
            if self.is_focusable(id) {
                self.set_focus(Some(id), true);
            } else {
                // Focus the popup's first focusable descendant once it is laid out.
                self.pending_popup_focus = Some(id);
            }
        }
        Ok(id)
    }

    /// Close a popup now (its subtree is removed, focus returns to its owner, and the owner
    /// receives `UiEvent::PopupClosed`). Closing a popup closes the popups above it.
    pub fn close_popup(&mut self, id: WidgetId) {
        let Some(pos) = self.popups.iter().position(|p| p.id == id || p.root == id) else {
            // Not a popup (a toast, a tooltip): just remove it.
            if self.index.contains_key(&id) {
                let _ = self.remove(id);
            }
            return;
        };
        // Detach the closing popups first: an owner handling `PopupClosed` may open a new
        // popup (a menu bar switching menus), which must survive this close.
        let closing = self.popups.split_off(pos);
        for p in closing.into_iter().rev() {
            let had_focus = self
                .focused
                .is_some_and(|f| f == p.id || self.is_descendant(f, p.root));
            let _ = self.remove(p.root);
            if had_focus || self.focused.is_none() {
                let back = p
                    .restore
                    .filter(|r| self.index.contains_key(r))
                    .or(Some(p.owner));
                self.set_focus(back, true);
            }
            if let Some(k) = self.index.get(&p.owner).copied() {
                self.dispatch(k, &UiEvent::PopupClosed(p.id));
            }
        }
    }

    /// Open popups, bottom to top.
    pub fn popups(&self) -> Vec<WidgetId> {
        self.popups.iter().map(|p| p.id).collect()
    }

    pub(crate) fn is_descendant(&self, id: WidgetId, ancestor: WidgetId) -> bool {
        let (Some(a), Some(b)) = (self.index.get(&ancestor), self.index.get(&id)) else {
            return false;
        };
        self.is_ancestor_or_self_key(*a, *b)
    }

    /// The popup `id` belongs to (itself or an ancestor popup).
    pub(crate) fn popup_containing(&self, id: WidgetId) -> Option<WidgetId> {
        self.popups
            .iter()
            .rev()
            .find(|p| id == p.id || self.is_descendant(id, p.root))
            .map(|p| p.id)
    }

    /// Process closes requested during event dispatch.
    pub(crate) fn flush_pending_close(&mut self) {
        while let Some(id) = self.pending_close.pop() {
            self.close_popup(id);
        }
        // Focus a popup's first part in *visual* order, so only once it is laid out (its
        // content may have been built in the same event that opened it).
        if self.layout_dirty {
            return;
        }
        if let Some(p) = self.pending_popup_focus.take()
            && self.index.contains_key(&p)
        {
            let pk = self.index[&p];
            if let Some(first) = self.first_focusable_in(pk) {
                self.set_focus(Some(first), true);
            }
        }
    }

    /// A press at `hit` closes the popups it lands outside of (top first). Returns true if
    /// the press landed on a modal's scrim (it must not reach anything beneath).
    pub(crate) fn dismiss_outside(&mut self, hit: Option<NodeKey>) -> bool {
        let hit_id = hit.and_then(|k| self.nodes.get(k)).map(|n| n.id);
        while let Some(top) = self.popups.last().cloned() {
            let inside = hit_id.is_some_and(|h| h == top.root || self.is_descendant(h, top.root));
            if inside {
                return false;
            }
            if top.spec.modal {
                return true;
            }
            let on_owner =
                hit_id.is_some_and(|h| h == top.owner || self.is_descendant(h, top.owner));
            if !top.spec.dismiss_on_outside || on_owner {
                return false;
            }
            self.close_popup(top.id);
        }
        false
    }

    /// Escape with nothing handling it closes the top popup.
    pub(crate) fn escape_popup(&mut self) -> bool {
        match self.popups.last() {
            Some(p) if p.spec.dismiss_on_escape => {
                let id = p.id;
                self.close_popup(id);
                true
            }
            _ => false,
        }
    }

    // ---- tooltips ----------------------------------------------------------------------

    /// The pointer (or keyboard focus) moved to `target`: arm its tooltip timer.
    pub(crate) fn tooltip_target(&mut self, target: Option<WidgetId>) {
        if let Some((t, _)) = self.tooltip
            && Some(t) == target
        {
            return;
        }
        self.hide_tooltip();
        if let Some(old) = self.tooltip_armed.take() {
            self.scheduler.cancel_timer(old, TOOLTIP_TAG);
        }
        let Some(t) = target else { return };
        let has = self
            .index
            .get(&t)
            .and_then(|k| self.nodes[*k].widget.as_ref())
            .and_then(|w| w.tooltip(&self.rt))
            .is_some();
        if has {
            let at = self.now + TOOLTIP_DELAY;
            self.scheduler.set_timer(t, at, TOOLTIP_TAG);
            self.tooltip_armed = Some(t);
        }
    }

    /// The tooltip timer fired for `target`.
    pub(crate) fn show_tooltip(&mut self, target: WidgetId) {
        self.tooltip_armed = None;
        if self.hover != Some(target) && self.focused != Some(target) {
            return;
        }
        let Some(text) = self
            .index
            .get(&target)
            .and_then(|k| self.nodes[*k].widget.as_ref())
            .and_then(|w| w.tooltip(&self.rt))
        else {
            return;
        };
        let Some(r) = self.rect(target) else { return };
        let Ok(ok) = self.overlay_key() else { return };
        let overlay = self.nodes[ok].id;
        let size = {
            let mut res = self.res.borrow_mut();
            res.text.set_scale(self.scale);
            let (_, t) =
                res.text
                    .layout(&text, &TextStyle::body(self.theme.type_scale.small), None);
            let pad = self.theme.space[2];
            Size::new(t.w + 2.0 * pad, t.h + pad)
        };
        let at = place(Anchor::Below(r), size, self.size());
        let style = absolute(at, size).no_hit_test();
        if let Ok(id) = self.add_boxed(
            overlay,
            Key::Static("forge.tooltip"),
            style,
            Box::new(TooltipBubble { text }),
        ) {
            self.tooltip = Some((target, id));
            // The owner's accessible description now points at the bubble.
            self.invalidate(target, crate::damage::Dirty::A11Y);
        }
    }

    pub(crate) fn hide_tooltip(&mut self) {
        if let Some((owner, id)) = self.tooltip.take() {
            let _ = self.remove(id);
            self.invalidate(owner, crate::damage::Dirty::A11Y);
        }
    }

    /// The tooltip showing now: `(target, bubble)`.
    pub fn tooltip_shown(&self) -> Option<(WidgetId, WidgetId)> {
        self.tooltip
    }

    // ---- toasts ------------------------------------------------------------------------

    /// Show a toast (bottom right; expires after [`TOAST_DURATION`]).
    pub fn toast(&mut self, text: &str, severity: Severity) -> Result<WidgetId, UiError> {
        let ok = self.overlay_key()?;
        let overlay = self.nodes[ok].id;
        let stack_id = overlay.child(&Key::Static("forge.toasts"));
        if !self.index.contains_key(&stack_id) {
            let space = self.theme.space;
            let mut s = NodeStyle::column(space[2])
                .scope(FocusScope::Panel)
                .no_hit_test();
            s.layout.position = taffy::Position::Absolute;
            s.layout.inset = taffy::Rect {
                left: taffy::prelude::auto(),
                top: taffy::prelude::auto(),
                right: taffy::prelude::length(space[4]),
                bottom: taffy::prelude::length(space[4]),
            };
            s.layout.align_items = Some(taffy::AlignItems::END);
            self.add_boxed(
                overlay,
                Key::Static("forge.toasts"),
                s,
                Box::new(
                    crate::widgets::Container::new(Role::Group)
                        .labelled(crate::tr!("Notifications")),
                ),
            )?;
        }
        self.toast_seq += 1;
        let key = Key::Id(self.toast_seq);
        self.add_boxed(
            stack_id,
            key,
            NodeStyle::leaf()
                .background(ColorRole::BgRaised)
                .rounded(Radius::Md),
            Box::new(Toast {
                text: text.to_string(),
                severity,
                armed: false,
            }),
        )
    }

    /// Live toasts, oldest first.
    pub fn toasts(&self) -> Vec<WidgetId> {
        let Some(ok) = self.overlay else {
            return Vec::new();
        };
        let Some(n) = self.nodes.get(ok) else {
            return Vec::new();
        };
        let stack = n.id.child(&Key::Static("forge.toasts"));
        self.children(stack)
    }

    pub(crate) fn first_focusable_in(&self, k: NodeKey) -> Option<WidgetId> {
        let mut stack = vec![k];
        let mut found = Vec::new();
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if node.hidden {
                continue;
            }
            if node.widget.as_ref().is_some_and(|w| w.focusable()) {
                found.push(crate::focus::Focusable {
                    id: node.id,
                    rect: node.rect,
                });
            }
            stack.extend(node.children.iter().copied());
        }
        crate::focus::visual_order(found).first().map(|f| f.id)
    }
}

/// The overlay layer's widget: paints nothing, takes no pointer events itself.
struct OverlayLayer;
impl Widget for OverlayLayer {
    fn role(&self) -> Role {
        Role::Group
    }
    fn paint(&self, _cx: &mut PaintCx) {}
}

/// A modal's scrim: swallows pointer events outside the dialog.
struct Scrim;
impl Widget for Scrim {
    fn role(&self) -> Role {
        Role::Group
    }
    fn event(&mut self, _cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::PointerDown { .. } | UiEvent::PointerUp { .. } | UiEvent::Wheel { .. } => {
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, _cx: &mut PaintCx) {}
}

/// The tooltip bubble.
struct TooltipBubble {
    text: String,
}
impl Widget for TooltipBubble {
    fn role(&self) -> Role {
        Role::Tooltip
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let radius = cx.theme().radius.sm;
        let shadow = cx.theme().shadows[1];
        cx.panel(
            r,
            Some(ColorRole::BgRaised),
            Some(ColorRole::Border),
            radius,
            Some(shadow),
        );
        let style = TextStyle::body(cx.theme().type_scale.small);
        let pad = cx.theme().space[2];
        let (_, t) = cx.shape(&self.text, &style);
        cx.text(
            &self.text,
            &style,
            Point::new(r.x + pad, r.y + (r.h - t.h) * 0.5),
            ColorRole::FgPrimary,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.text.as_str());
    }
}

/// One toast. Escape, Enter or Delete dismisses it when focused.
struct Toast {
    text: String,
    severity: Severity,
    armed: bool,
}

impl Toast {
    fn line(&self) -> String {
        format!("{}: {}", self.severity.word(), self.text)
    }
}

impl Widget for Toast {
    fn role(&self) -> Role {
        match self.severity {
            Severity::Error | Severity::Warning => Role::Alert,
            _ => Role::Status,
        }
    }
    fn focusable(&self) -> bool {
        true
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        cx: &mut MeasureCx,
        _k: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let (_, t) = cx.text.layout(
            &self.line(),
            &TextStyle::body(cx.theme.type_scale.body),
            None,
        );
        let pad = cx.theme.space[3];
        Size::new(t.w + 2.0 * pad + 8.0, t.h + 2.0 * pad)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::VisibilityChanged(true) if !self.armed => {
                self.armed = true;
                cx.set_timer(TOAST_DURATION, TOAST_TAG);
                Handled::Yes
            }
            UiEvent::Timer(TOAST_TAG) => {
                let id = cx.id();
                cx.close_popup(id);
                Handled::Yes
            }
            UiEvent::Key(k)
                if k.pressed
                    && matches!(k.code, KeyCode::Escape | KeyCode::Enter | KeyCode::Delete) =>
            {
                let id = cx.id();
                cx.close_popup(id);
                Handled::Yes
            }
            UiEvent::PointerUp { inside: true, .. } => {
                let id = cx.id();
                cx.close_popup(id);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let shadow = cx.theme().shadows[2];
        let radius = cx.theme().radius.md;
        cx.panel(
            r,
            Some(ColorRole::BgRaised),
            Some(ColorRole::Border),
            radius,
            Some(shadow),
        );
        let bar = Rect::new(r.x, r.y, 4.0, r.h);
        cx.mark(bar, self.severity.role(), ColorRole::BgRaised, 2.0);
        let pad = cx.theme().space[3];
        let style = TextStyle::body(cx.theme().type_scale.body);
        cx.text(
            &self.line(),
            &style,
            Point::new(r.x + pad + 8.0, r.y + pad),
            ColorRole::FgPrimary,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.line());
        // Errors and warnings interrupt (an alert); success and info wait their turn.
        node.set_live(match self.severity {
            Severity::Error | Severity::Warning => accesskit::Live::Assertive,
            _ => accesskit::Live::Polite,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_flips_and_clamps() {
        let win = Size::new(400.0, 300.0);
        let field = Rect::new(10.0, 250.0, 100.0, 24.0);
        let p = place(Anchor::Below(field), Size::new(120.0, 100.0), win);
        assert!(
            p.y + 100.0 <= field.y,
            "a dropdown near the bottom opens above: {p:?}"
        );
        let m = Rect::new(350.0, 10.0, 40.0, 20.0);
        let s = place(Anchor::Right(m), Size::new(100.0, 50.0), win);
        assert!(
            s.x + 100.0 <= m.x,
            "a submenu at the right edge opens left: {s:?}"
        );
        let c = place(
            Anchor::At(Point::new(390.0, 290.0)),
            Size::new(80.0, 60.0),
            win,
        );
        assert!(c.x + 80.0 <= 400.0 && c.y + 60.0 <= 300.0);
    }
}

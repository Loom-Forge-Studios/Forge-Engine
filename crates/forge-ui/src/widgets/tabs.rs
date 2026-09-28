//! Tabs (§21.16 "tabs (closable, reorderable)").
//!
//! A tab strip shows one page and hides the rest. Hidden pages draw nothing, take no
//! focus, and their spinners and live feeds stop (§21.3, §21.11). The strip is one tab
//! stop (WAI-ARIA tabs):
//!
//! * Left/Right (Home/End) switch tabs;
//! * **reorder**: Ctrl+Shift+Left/Right, or drag a tab along the strip ([`TabMoved`]);
//! * **close** (when closable): Ctrl+W or Delete, a middle click, or the tab's ✕
//!   ([`TabClosed`]). Closing is the owner's decision (a document may have unsaved
//!   changes): the strip asks, and the owner calls [`Tabs::remove_tab`].

use accesskit::{Action, Role};

use crate::damage::Dirty;
use crate::geom::{Point, Rect, Size};
use crate::id::WidgetId;
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::{CONTROL_MIN_H, body, control_pad_x};

/// Raised when the active tab changes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TabSelected {
    pub tabs: WidgetId,
    pub index: usize,
}

/// A tab asked to close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabClosed {
    pub tabs: WidgetId,
    pub index: usize,
    pub title: String,
}

/// A tab moved (its page moved with it).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TabMoved {
    pub tabs: WidgetId,
    pub from: usize,
    pub to: usize,
}

const CLOSE_W: f32 = 18.0;

/// A tab strip (see the module docs).
pub struct Tabs {
    titles: Vec<String>,
    pages: Vec<WidgetId>,
    selected: Signal<usize>,
    closable: bool,
    reorderable: bool,
    drag: Option<(usize, f32)>,
}

impl Tabs {
    /// `pages[i]` is shown when tab `i` is selected. Attach pages with
    /// [`Tabs::set_pages`] after adding them to the tree.
    pub fn new(titles: &[&str], selected: Signal<usize>) -> Self {
        Self {
            titles: titles.iter().map(|s| s.to_string()).collect(),
            pages: Vec::new(),
            selected,
            closable: false,
            reorderable: false,
            drag: None,
        }
    }
    pub fn closable(mut self) -> Self {
        self.closable = true;
        self
    }
    pub fn reorderable(mut self) -> Self {
        self.reorderable = true;
        self
    }
    pub fn set_pages(&mut self, pages: Vec<WidgetId>) {
        self.pages = pages;
    }
    pub fn pages(&self) -> &[WidgetId] {
        &self.pages
    }
    pub fn titles(&self) -> &[String] {
        &self.titles
    }
    /// Remove tab `i` (the owner, after `TabClosed`). Returns its page, which the owner
    /// removes from the tree. Call `Ui::invalidate(tabs, LAYOUT)` afterwards.
    pub fn remove_tab(&mut self, i: usize) -> Option<WidgetId> {
        if i >= self.titles.len() {
            return None;
        }
        self.titles.remove(i);
        (i < self.pages.len()).then(|| self.pages.remove(i))
    }

    fn widths(&self, text: &mut crate::text::TextSystem, theme: &crate::style::Theme) -> Vec<f32> {
        let pad = control_pad_x(theme);
        let extra = if self.closable { CLOSE_W } else { 0.0 };
        self.titles
            .iter()
            .map(|t| text.layout(t, &body(theme), None).1.w + 2.0 * pad + extra)
            .collect()
    }

    fn tab_rects(&self, r: Rect, widths: &[f32]) -> Vec<Rect> {
        let mut x = r.x;
        widths
            .iter()
            .map(|w| {
                let rr = Rect::new(x, r.y, *w, r.h);
                x += w;
                rr
            })
            .collect()
    }

    fn select(&self, cx: &mut EventCx, i: usize) {
        self.selected.set(cx.rt_mut(), i);
        self.apply(cx, i);
        let id = cx.id();
        cx.action(TabSelected { tabs: id, index: i });
    }

    /// Show page `i`, hide the others.
    pub fn apply(&self, cx: &mut EventCx, i: usize) {
        for (j, p) in self.pages.iter().enumerate() {
            cx.set_hidden(*p, j != i);
        }
    }

    fn close(&self, cx: &mut EventCx, i: usize) {
        if self.closable
            && let Some(t) = self.titles.get(i)
        {
            let id = cx.id();
            cx.action(TabClosed {
                tabs: id,
                index: i,
                title: t.clone(),
            });
        }
    }

    fn move_tab(&mut self, cx: &mut EventCx, from: usize, to: usize) {
        let n = self.titles.len();
        if !self.reorderable || from >= n || to >= n || from == to {
            return;
        }
        let t = self.titles.remove(from);
        self.titles.insert(to, t);
        if from < self.pages.len() && to < self.pages.len() {
            let p = self.pages.remove(from);
            self.pages.insert(to, p);
        }
        self.selected.set(cx.rt_mut(), to);
        let id = cx.id();
        cx.action(TabMoved { tabs: id, from, to });
        cx.request_layout();
        cx.request_a11y();
    }
}

impl Widget for Tabs {
    fn role(&self) -> Role {
        Role::TabList
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(Some(self.selected.any()), Dirty::PAINT | Dirty::A11Y);
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
        let w: f32 = self.widths(cx.text, cx.theme).iter().sum();
        Size::new(w, CONTROL_MIN_H + 4.0)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let n = self.titles.len();
        if n == 0 {
            return Handled::No;
        }
        let cur = self.selected.get(cx.rt()).min(n - 1);
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let (ctrl, shift) = (k.mods.ctrl, k.mods.shift);
                match k.code {
                    KeyCode::Right if ctrl && shift => self.move_tab(cx, cur, (cur + 1).min(n - 1)),
                    KeyCode::Left if ctrl && shift => self.move_tab(cx, cur, cur.saturating_sub(1)),
                    KeyCode::Right => self.select(cx, (cur + 1) % n),
                    KeyCode::Left => self.select(cx, (cur + n - 1) % n),
                    KeyCode::Home => self.select(cx, 0),
                    KeyCode::End => self.select(cx, n - 1),
                    KeyCode::Char('w') if ctrl && self.closable => self.close(cx, cur),
                    KeyCode::Delete if self.closable => self.close(cx, cur),
                    _ => return Handled::No,
                }
                Handled::Yes
            }
            UiEvent::PointerDown { pos, button, .. } => {
                let theme = cx.theme().clone();
                let r = cx.rect();
                let widths = self.widths(&mut cx.text(), &theme);
                let rects = self.tab_rects(r, &widths);
                let Some(i) = rects.iter().position(|q| q.contains(*pos)) else {
                    return Handled::Yes;
                };
                match button {
                    PointerButton::Middle => self.close(cx, i),
                    PointerButton::Primary
                        if self.closable && pos.x >= rects[i].right() - CLOSE_W =>
                    {
                        self.close(cx, i)
                    }
                    PointerButton::Primary => {
                        if i != cur {
                            self.select(cx, i);
                        }
                        if self.reorderable {
                            self.drag = Some((i, pos.x));
                            cx.capture_pointer();
                        }
                    }
                    PointerButton::Secondary => {}
                }
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                if let Some((from, _)) = self.drag {
                    let theme = cx.theme().clone();
                    let r = cx.rect();
                    let widths = self.widths(&mut cx.text(), &theme);
                    let rects = self.tab_rects(r, &widths);
                    let to = rects
                        .iter()
                        .position(|q| pos.x >= q.x && pos.x < q.right())
                        .unwrap_or(if pos.x < r.x { 0 } else { n - 1 });
                    if to != from {
                        self.move_tab(cx, from, to);
                        self.apply(cx, to);
                        self.drag = Some((to, pos.x));
                    }
                }
                Handled::Yes
            }
            UiEvent::PointerUp { .. } => {
                if self.drag.take().is_some() {
                    cx.release_pointer();
                }
                Handled::Yes
            }
            UiEvent::A11yChildAction(i, Action::Click) => {
                self.select(cx, (*i as usize).min(n - 1));
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let sel = self.selected.get(cx.rt());
        let pad = control_pad_x(cx.theme());
        let style = body(cx.theme());
        let theme = cx.theme().clone();
        let widths: Vec<f32> = {
            let extra = if self.closable { CLOSE_W } else { 0.0 };
            self.titles
                .iter()
                .map(|t| cx.shape(t, &style).1.w + 2.0 * pad + extra)
                .collect()
        };
        let bg = cx.parent_bg();
        let _ = theme;
        for (i, (t, tr)) in self
            .titles
            .iter()
            .zip(self.tab_rects(r, &widths))
            .enumerate()
        {
            let active = i == sel;
            let (run, ts) = cx.shape(t, &style);
            let o = Point::new(tr.x + pad, tr.y + (tr.h - ts.h) * 0.5);
            if active {
                cx.run(run, o, ColorRole::FgPrimary);
                cx.mark(
                    Rect::new(tr.x + 4.0, tr.bottom() - 3.0, tr.w - 8.0, 3.0),
                    ColorRole::Accent,
                    bg,
                    1.5,
                );
            } else {
                cx.run(run, o, ColorRole::FgMuted);
            }
            if self.closable {
                let (run, cs) = cx.shape("✕", &style);
                cx.run(
                    run,
                    Point::new(
                        tr.right() - CLOSE_W - 2.0 + (CLOSE_W - cs.w) * 0.5,
                        tr.y + (tr.h - cs.h) * 0.5,
                    ),
                    ColorRole::FgMuted,
                );
            }
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Tabs"));
        if let Some(t) = self.titles.get(self.selected.get(cx.rt)) {
            node.set_value(t.as_str());
        }
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let sel = self.selected.get(cx.rt);
        let n = self.titles.len();
        for (i, t) in self.titles.iter().enumerate() {
            let mut node = accesskit::Node::new(Role::Tab);
            node.set_label(t.as_str());
            node.set_selected(i == sel);
            node.set_position_in_set(i + 1);
            node.set_size_of_set(n);
            node.add_action(Action::Click);
            if let Some(p) = self.pages.get(i) {
                node.set_controls(vec![p.to_accesskit()]);
            }
            out.push((i as u64, node));
        }
    }
    fn a11y_focus(&self, rt: &crate::state::Runtime) -> Option<u64> {
        (!self.titles.is_empty()).then(|| self.selected.get(rt).min(self.titles.len() - 1) as u64)
    }
}

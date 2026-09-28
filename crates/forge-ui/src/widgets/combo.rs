//! Choice widgets (§21.16): combo / dropdown with filter, autocomplete, multi-select chips.
//!
//! * **Combo**: one tab stop showing the choice. Down, Alt+Down, Enter or Space opens a
//!   popup list with a filter line on top (typing filters, fuzzy); Up/Down/PageUp/
//!   PageDown/Home/End move; Enter chooses; Escape closes. Closed, a letter selects the
//!   next option starting with it (type-ahead).
//! * **Autocomplete**: a text entry whose suggestions (fuzzy-ranked) appear below as you
//!   type; focus stays in the entry, Up/Down pick a suggestion, Enter or Tab accepts it.
//! * **Chips**: the selected values as removable chips (Left/Right move between them,
//!   Delete/Backspace removes, Enter or Down opens the list of the others to add).
//!
//! The popups write their result into the signals the widgets were given, so the owner
//! repaints through its binding like any other change.

use std::cell::RefCell;
use std::rc::Rc;

use accesskit::{Action, Role, Toggled};

use crate::damage::Dirty;
use crate::fuzzy::rank;
use crate::geom::{Point, Rect, Size};
use crate::id::{Key, WidgetId};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::overlay::{Anchor, PopupSpec};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::line_edit::{EditOutcome, LineEdit};
use super::{CONTROL_MIN_H, body, control_pad_x, small};

const ROW_H: f32 = 26.0;
const MAX_ROWS: usize = 10;

/// What a choice popup does with the chosen option.
enum Sink {
    Single(Signal<usize>),
    Add(Signal<Vec<usize>>),
}

/// The popup list behind combos and chips: a filter line and the (filtered) options.
pub struct ChoiceList {
    options: Rc<Vec<String>>,
    /// Indices into `options` still offered (chips hide the selected ones).
    offered: Vec<usize>,
    filter: LineEdit,
    shown: Vec<usize>,
    active: usize,
    top: usize,
    sink: Sink,
    label: String,
}

impl ChoiceList {
    fn new(
        options: Rc<Vec<String>>,
        offered: Vec<usize>,
        sink: Sink,
        current: Option<usize>,
        label: &str,
    ) -> Self {
        let mut s = Self {
            options,
            offered,
            filter: LineEdit::default(),
            shown: Vec::new(),
            active: 0,
            top: 0,
            sink,
            label: label.to_string(),
        };
        s.refilter();
        if let Some(c) = current
            && let Some(p) = s.shown.iter().position(|i| *i == c)
        {
            s.active = p;
            s.top = p.saturating_sub(MAX_ROWS / 2);
        }
        s
    }
    /// The options currently listed (indices into the options).
    pub fn shown(&self) -> &[usize] {
        &self.shown
    }
    fn refilter(&mut self) {
        let texts: Vec<&str> = self
            .offered
            .iter()
            .map(|i| self.options[*i].as_str())
            .collect();
        self.shown = rank(&self.filter.text, &texts, |s| s)
            .into_iter()
            .map(|(i, _)| self.offered[i])
            .collect();
        self.active = 0;
        self.top = 0;
    }
    fn ensure_visible(&mut self) {
        if self.active < self.top {
            self.top = self.active;
        } else if self.active >= self.top + MAX_ROWS {
            self.top = self.active + 1 - MAX_ROWS;
        }
    }
    fn choose(&mut self, cx: &mut EventCx) {
        let Some(&opt) = self.shown.get(self.active) else {
            return;
        };
        match &self.sink {
            Sink::Single(s) => s.set(cx.rt_mut(), opt),
            Sink::Add(s) => {
                let mut v = s.get(cx.rt());
                if !v.contains(&opt) {
                    v.push(opt);
                }
                s.set(cx.rt_mut(), v);
            }
        }
        let id = cx.id();
        cx.close_popup(id);
    }
}

impl Widget for ChoiceList {
    fn role(&self) -> Role {
        Role::ListBox
    }
    fn focusable(&self) -> bool {
        true
    }
    fn accepts_text(&self) -> bool {
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
        let w = self
            .offered
            .iter()
            .map(|i| cx.text.layout(&self.options[*i], &body(cx.theme), None).1.w)
            .fold(120.0f32, f32::max);
        let rows = self.offered.len().clamp(1, MAX_ROWS) as f32;
        Size::new(w + 32.0, CONTROL_MIN_H + 8.0 + rows * ROW_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let n = self.shown.len();
        match ev {
            UiEvent::Key(k) if k.pressed => {
                match k.code {
                    KeyCode::Down if n > 0 => self.active = (self.active + 1).min(n - 1),
                    KeyCode::Up => self.active = self.active.saturating_sub(1),
                    KeyCode::PageDown if n > 0 => self.active = (self.active + MAX_ROWS).min(n - 1),
                    KeyCode::PageUp => self.active = self.active.saturating_sub(MAX_ROWS),
                    KeyCode::Home if k.mods.ctrl => self.active = 0,
                    KeyCode::End if k.mods.ctrl && n > 0 => self.active = n - 1,
                    KeyCode::Tab => {
                        let id = cx.id();
                        cx.close_popup(id);
                        return Handled::No;
                    }
                    _ => match self.filter.key(k, cx.clipboard()) {
                        EditOutcome::Submit => {
                            self.choose(cx);
                            return Handled::Yes;
                        }
                        EditOutcome::Changed => self.refilter(),
                        EditOutcome::Cancel | EditOutcome::Ignored => return Handled::No,
                        EditOutcome::Moved => {}
                    },
                }
                self.ensure_visible();
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::Ime(i) => {
                if self.filter.ime(i) == EditOutcome::Changed {
                    self.refilter();
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::Wheel { dy, .. } => {
                if *dy < 0.0 && self.top + MAX_ROWS < n {
                    self.top += 1;
                } else if *dy > 0.0 && self.top > 0 {
                    self.top -= 1;
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                let r = cx.rect();
                let y0 = r.y + CONTROL_MIN_H + 8.0;
                if pos.y >= y0 {
                    let i = self.top + ((pos.y - y0) / ROW_H) as usize;
                    if i < n && i != self.active {
                        self.active = i;
                        cx.request_paint();
                    }
                }
                Handled::Yes
            }
            UiEvent::PointerUp {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let r = cx.rect();
                if pos.y >= r.y + CONTROL_MIN_H + 8.0 {
                    self.choose(cx);
                }
                Handled::Yes
            }
            UiEvent::PointerDown { .. } => Handled::Yes,
            UiEvent::A11yChildAction(opt, Action::Click) => {
                if let Some(p) = self.shown.iter().position(|i| *i as u64 == *opt) {
                    self.active = p;
                    self.choose(cx);
                }
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
        let style = body(cx.theme());
        let f = Rect::new(r.x + 4.0, r.y + 4.0, r.w - 8.0, CONTROL_MIN_H);
        cx.panel(
            f,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            cx.theme().radius.sm,
            None,
        );
        let pad = control_pad_x(cx.theme()) * 0.5;
        self.filter.paint_line(
            cx,
            Rect::new(f.x + pad, f.y, f.w - 2.0 * pad, f.h),
            &style,
            true,
            true,
            crate::tr!("Filter"),
            ColorRole::BgSunken,
        );
        let y0 = r.y + CONTROL_MIN_H + 8.0;
        if self.shown.is_empty() {
            cx.text(
                crate::tr!("No matches"),
                &style,
                Point::new(r.x + 12.0, y0 + 4.0),
                ColorRole::FgMuted,
            );
        }
        for (row, &opt) in self.shown.iter().enumerate().skip(self.top).take(MAX_ROWS) {
            let rr = Rect::new(
                r.x + 4.0,
                y0 + (row - self.top) as f32 * ROW_H,
                r.w - 8.0,
                ROW_H,
            );
            let active = row == self.active;
            let (fg, bg) = if active {
                cx.fill(rr, ColorRole::Accent, cx.theme().radius.sm);
                (ColorRole::FgOnAccent, ColorRole::Accent)
            } else {
                cx.fill(rr, ColorRole::BgRaised, 0.0);
                (ColorRole::FgPrimary, ColorRole::BgRaised)
            };
            let t = &self.options[opt];
            let (_, ts) = cx.shape(t, &style);
            cx.text_on(
                t,
                &style,
                Point::new(rr.x + 8.0, rr.y + (rr.h - ts.h) * 0.5),
                fg,
                bg,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
    }
    fn a11y_children(&self, _cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let n = self.shown.len();
        for (row, &opt) in self.shown.iter().enumerate().skip(self.top).take(MAX_ROWS) {
            let mut node = accesskit::Node::new(Role::ListBoxOption);
            node.set_label(self.options[opt].as_str());
            node.set_position_in_set(row + 1);
            node.set_size_of_set(n);
            node.set_selected(row == self.active);
            node.add_action(Action::Click);
            out.push((opt as u64, node));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        self.shown.get(self.active).map(|o| *o as u64)
    }
}

/// A dropdown with a filter.
pub struct ComboBox {
    label: String,
    options: Rc<Vec<String>>,
    selected: Signal<usize>,
    open: bool,
}

impl ComboBox {
    pub fn new(label: &str, options: &[&str], selected: Signal<usize>) -> Self {
        Self {
            label: label.to_string(),
            options: Rc::new(options.iter().map(|s| s.to_string()).collect()),
            selected,
            open: false,
        }
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    fn open_list(&mut self, cx: &mut EventCx) {
        let r = cx.rect();
        let offered: Vec<usize> = (0..self.options.len()).collect();
        let cur = self.selected.get(cx.rt());
        let list = ChoiceList::new(
            self.options.clone(),
            offered,
            Sink::Single(self.selected),
            Some(cur),
            &self.label,
        );
        if cx
            .open_popup(
                Key::Static("list"),
                PopupSpec::menu(Anchor::Below(r)).min_width(r.w),
                list,
            )
            .is_some()
        {
            self.open = true;
            cx.request_paint();
            cx.request_a11y();
        }
    }
}

impl Widget for ComboBox {
    fn role(&self) -> Role {
        Role::ComboBox
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
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let w = self
            .options
            .iter()
            .map(|o| cx.text.layout(o, &body(cx.theme), None).1.w)
            .fold(60.0f32, f32::max);
        Size::new(
            known
                .width
                .unwrap_or(w + 2.0 * control_pad_x(cx.theme) + 20.0),
            CONTROL_MIN_H,
        )
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::Key(k) if k.pressed => match k.code {
                KeyCode::Down | KeyCode::Enter | KeyCode::Space => {
                    self.open_list(cx);
                    Handled::Yes
                }
                KeyCode::Up if !self.options.is_empty() => {
                    let c = self.selected.get(cx.rt());
                    self.selected.set(cx.rt_mut(), c.saturating_sub(1));
                    Handled::Yes
                }
                KeyCode::Char(c) if !k.mods.ctrl => {
                    let n = self.options.len();
                    let start = self.selected.get(cx.rt()) + 1;
                    if let Some(i) = (0..n).map(|d| (start + d) % n).find(|&i| {
                        self.options[i]
                            .chars()
                            .next()
                            .is_some_and(|f| f.to_lowercase().eq(c.to_lowercase()))
                    }) {
                        self.selected.set(cx.rt_mut(), i);
                        return Handled::Yes;
                    }
                    Handled::No
                }
                _ => Handled::No,
            },
            UiEvent::PointerDown {
                button: PointerButton::Primary,
                ..
            }
            | UiEvent::A11yAction(Action::Expand | Action::Click) => {
                if !self.open {
                    self.open_list(cx);
                }
                Handled::Yes
            }
            UiEvent::PopupClosed(_) => {
                self.open = false;
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let st = crate::style::button_style(crate::style::Variant::Secondary, cx.state());
        let radius = cx.theme().radius.md;
        cx.panel(r, st.bg, st.border, radius, None);
        let style = body(cx.theme());
        let sel = self.selected.get(cx.rt());
        let text = self.options.get(sel).map_or("", String::as_str);
        let (_, t) = cx.shape(text, &style);
        let pad = control_pad_x(cx.theme());
        cx.text(
            text,
            &style,
            Point::new(r.x + pad, r.y + (r.h - t.h) * 0.5),
            st.fg,
        );
        let g = if self.open { "▴" } else { "▾" };
        let (_, gs) = cx.shape(g, &style);
        cx.text(
            g,
            &style,
            Point::new(r.right() - gs.w - 8.0, r.y + (r.h - gs.h) * 0.5),
            st.fg,
        );
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        if let Some(o) = self.options.get(self.selected.get(cx.rt)) {
            node.set_value(o.as_str());
        }
        node.set_expanded(self.open);
        node.set_has_popup(accesskit::HasPopup::Listbox);
        node.add_action(Action::Expand);
    }
}

/// Shared between an autocomplete entry and its suggestion popup.
#[derive(Default)]
struct Suggest {
    items: Vec<usize>,
    active: Option<usize>,
}

/// The autocomplete's suggestion popup (never takes focus; the entry drives it).
struct SuggestList {
    options: Rc<Vec<String>>,
    state: Rc<RefCell<Suggest>>,
}

impl Widget for SuggestList {
    fn role(&self) -> Role {
        Role::ListBox
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let shadow = cx.theme().shadows[1];
        cx.panel(
            r,
            Some(ColorRole::BgRaised),
            Some(ColorRole::Border),
            cx.theme().radius.md,
            Some(shadow),
        );
        let style = body(cx.theme());
        let s = self.state.borrow();
        for (row, &opt) in s.items.iter().enumerate().take(MAX_ROWS) {
            let rr = Rect::new(r.x + 4.0, r.y + 4.0 + row as f32 * ROW_H, r.w - 8.0, ROW_H);
            let active = s.active == Some(row);
            let (fg, bg) = if active {
                cx.fill(rr, ColorRole::Accent, cx.theme().radius.sm);
                (ColorRole::FgOnAccent, ColorRole::Accent)
            } else {
                cx.fill(rr, ColorRole::BgRaised, 0.0);
                (ColorRole::FgPrimary, ColorRole::BgRaised)
            };
            let t = &self.options[opt];
            let (_, ts) = cx.shape(t, &style);
            cx.text_on(
                t,
                &style,
                Point::new(rr.x + 8.0, rr.y + (rr.h - ts.h) * 0.5),
                fg,
                bg,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Suggestions"));
    }
    fn a11y_children(&self, _cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let s = self.state.borrow();
        for (row, &opt) in s.items.iter().enumerate().take(MAX_ROWS) {
            let mut node = accesskit::Node::new(Role::ListBoxOption);
            node.set_label(self.options[opt].as_str());
            node.set_selected(s.active == Some(row));
            out.push((opt as u64, node));
        }
    }
}

/// A text entry with fuzzy suggestions.
pub struct Autocomplete {
    label: String,
    options: Rc<Vec<String>>,
    value: Signal<String>,
    edit: LineEdit,
    state: Rc<RefCell<Suggest>>,
    popup: Option<WidgetId>,
}

impl Autocomplete {
    pub fn new(label: &str, options: &[&str], value: Signal<String>) -> Self {
        Self {
            label: label.to_string(),
            options: Rc::new(options.iter().map(|s| s.to_string()).collect()),
            value,
            edit: LineEdit::default(),
            state: Rc::new(RefCell::new(Suggest::default())),
            popup: None,
        }
    }
    pub fn suggestions(&self) -> Vec<String> {
        self.state
            .borrow()
            .items
            .iter()
            .map(|i| self.options[*i].clone())
            .collect()
    }
    fn update(&mut self, cx: &mut EventCx) {
        let q = self.edit.text.clone();
        let items: Vec<usize> = if q.is_empty() {
            Vec::new()
        } else {
            rank(&q, &self.options, |s| s.as_str())
                .into_iter()
                .map(|(i, _)| i)
                .filter(|i| self.options[*i] != q)
                .take(MAX_ROWS)
                .collect()
        };
        let n = items.len();
        {
            let mut s = self.state.borrow_mut();
            s.items = items;
            s.active = None;
        }
        if n == 0 {
            if let Some(p) = self.popup.take() {
                cx.close_popup(p);
            }
            return;
        }
        match self.popup {
            Some(p) => cx.invalidate(p, Dirty::PAINT | Dirty::A11Y),
            None => {
                let r = cx.rect();
                let spec = PopupSpec {
                    take_focus: false,
                    ..PopupSpec::menu(Anchor::Below(r))
                        .sized(Size::new(r.w, 8.0 + MAX_ROWS as f32 * ROW_H))
                };
                self.popup = cx.open_popup(
                    Key::Static("suggest"),
                    spec,
                    SuggestList {
                        options: self.options.clone(),
                        state: self.state.clone(),
                    },
                );
            }
        }
    }
    fn accept(&mut self, cx: &mut EventCx) -> bool {
        let pick = {
            let s = self.state.borrow();
            s.active.and_then(|a| s.items.get(a).copied())
        };
        if let Some(i) = pick {
            let t = self.options[i].clone();
            self.edit.set_text(&t);
            self.value.set(cx.rt_mut(), t);
            if let Some(p) = self.popup.take() {
                cx.close_popup(p);
            }
            self.state.borrow_mut().items.clear();
            cx.request_paint();
            return true;
        }
        false
    }
}

impl Widget for Autocomplete {
    fn role(&self) -> Role {
        Role::ComboBox
    }
    fn focusable(&self) -> bool {
        true
    }
    fn accepts_text(&self) -> bool {
        true
    }
    fn hover_sensitive(&self) -> bool {
        false
    }
    fn measured(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        _cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::new(known.width.unwrap_or(220.0), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::Key(k) if k.pressed => {
                let n = self.state.borrow().items.len();
                match k.code {
                    KeyCode::Down if n > 0 => {
                        let mut s = self.state.borrow_mut();
                        s.active = Some(s.active.map_or(0, |a| (a + 1).min(n - 1)));
                    }
                    KeyCode::Up if n > 0 => {
                        let mut s = self.state.borrow_mut();
                        s.active = s.active.and_then(|a| a.checked_sub(1));
                    }
                    KeyCode::Tab if n > 0 && self.state.borrow().active.is_some() => {
                        self.accept(cx);
                    }
                    KeyCode::Enter => {
                        if !self.accept(cx) {
                            let t = self.edit.text.clone();
                            self.value.set(cx.rt_mut(), t);
                        }
                        return Handled::Yes;
                    }
                    KeyCode::Escape if self.popup.is_some() => {
                        if let Some(p) = self.popup.take() {
                            cx.close_popup(p);
                        }
                        return Handled::Yes;
                    }
                    _ => match self.edit.key(k, cx.clipboard()) {
                        EditOutcome::Changed => {
                            self.update(cx);
                            cx.request_paint();
                            return Handled::Yes;
                        }
                        EditOutcome::Ignored | EditOutcome::Cancel | EditOutcome::Submit => {
                            return Handled::No;
                        }
                        EditOutcome::Moved => {
                            cx.request_paint();
                            return Handled::Yes;
                        }
                    },
                }
                if let Some(p) = self.popup {
                    cx.invalidate(p, Dirty::PAINT | Dirty::A11Y);
                }
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::Ime(i) => {
                if self.edit.ime(i) == EditOutcome::Changed {
                    self.update(cx);
                }
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PopupClosed(_) => {
                self.popup = None;
                Handled::Yes
            }
            UiEvent::FocusLost => {
                if let Some(p) = self.popup.take() {
                    cx.close_popup(p);
                }
                let t = self.edit.text.clone();
                self.value.set(cx.rt_mut(), t);
                cx.request_paint();
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.panel(
            r,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            cx.theme().radius.md,
            None,
        );
        let pad = control_pad_x(cx.theme());
        let style = body(cx.theme());
        let focused = cx.state().focused;
        self.edit.paint_line(
            cx,
            Rect::new(r.x + pad, r.y, r.w - 2.0 * pad, r.h),
            &style,
            focused,
            true,
            &self.label,
            ColorRole::BgSunken,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_value(self.edit.text.as_str());
        node.set_expanded(self.popup.is_some());
        node.set_has_popup(accesskit::HasPopup::Listbox);
    }
}

/// Multi-select chips.
pub struct Chips {
    label: String,
    options: Rc<Vec<String>>,
    selected: Signal<Vec<usize>>,
    /// Focused chip; `len` = the "Add" affordance.
    focus: usize,
}

impl Chips {
    pub fn new(label: &str, options: &[&str], selected: Signal<Vec<usize>>) -> Self {
        Self {
            label: label.to_string(),
            options: Rc::new(options.iter().map(|s| s.to_string()).collect()),
            selected,
            focus: 0,
        }
    }
    fn chip_rects(
        &self,
        text: &mut crate::text::TextSystem,
        theme: &crate::style::Theme,
        r: Rect,
        sel: &[usize],
    ) -> Vec<Rect> {
        let mut x = r.x + 4.0;
        let mut out = Vec::new();
        for &i in sel {
            let w = text.layout(&self.options[i], &small(theme), None).1.w + 30.0;
            out.push(Rect::new(x, r.y + 4.0, w, r.h - 8.0));
            x += w + 4.0;
        }
        let w = text.layout("+ Add", &small(theme), None).1.w + 16.0;
        out.push(Rect::new(x, r.y + 4.0, w, r.h - 8.0));
        out
    }
    fn open_add(&mut self, cx: &mut EventCx) {
        let sel = self.selected.get(cx.rt());
        let offered: Vec<usize> = (0..self.options.len())
            .filter(|i| !sel.contains(i))
            .collect();
        if offered.is_empty() {
            return;
        }
        let r = cx.rect();
        let list = ChoiceList::new(
            self.options.clone(),
            offered,
            Sink::Add(self.selected),
            None,
            &self.label,
        );
        cx.open_popup(Key::Static("add"), PopupSpec::menu(Anchor::Below(r)), list);
    }
    fn remove(&mut self, cx: &mut EventCx, at: usize) {
        let mut sel = self.selected.get(cx.rt());
        if at < sel.len() {
            sel.remove(at);
            self.focus = self.focus.min(sel.len());
            self.selected.set(cx.rt_mut(), sel);
        }
    }
}

impl Widget for Chips {
    fn role(&self) -> Role {
        Role::List
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
        _cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        Size::new(known.width.unwrap_or(300.0), CONTROL_MIN_H + 4.0)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let sel = self.selected.get(cx.rt());
        let n = sel.len();
        match ev {
            UiEvent::Key(k) if k.pressed => {
                match k.code {
                    KeyCode::Right => self.focus = (self.focus + 1).min(n),
                    KeyCode::Left => self.focus = self.focus.saturating_sub(1),
                    KeyCode::Home => self.focus = 0,
                    KeyCode::End => self.focus = n,
                    KeyCode::Delete | KeyCode::Backspace if self.focus < n => {
                        let f = self.focus;
                        self.remove(cx, f);
                    }
                    KeyCode::Enter | KeyCode::Down | KeyCode::Space => self.open_add(cx),
                    _ => return Handled::No,
                }
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let theme = cx.theme().clone();
                let r = cx.rect();
                let rects = self.chip_rects(&mut cx.text(), &theme, r, &sel);
                if let Some(i) = rects.iter().position(|r| r.contains(*pos)) {
                    if i == n {
                        self.focus = n;
                        self.open_add(cx);
                    } else if pos.x > rects[i].right() - 20.0 {
                        self.remove(cx, i);
                    } else {
                        self.focus = i;
                    }
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::A11yChildAction(i, Action::Click) => {
                if (*i as usize) < n {
                    self.remove(cx, *i as usize);
                } else {
                    self.open_add(cx);
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        cx.panel(
            r,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            cx.theme().radius.md,
            None,
        );
        let sel = self.selected.get(cx.rt());
        let theme = cx.theme().clone();
        let rects = self.chip_rects(cx.text, &theme, r, &sel);
        let st = small(&theme);
        let focused = cx.state().focus_visible;
        for (i, rr) in rects.iter().enumerate() {
            let is_add = i == sel.len();
            let label = if is_add {
                "+ Add"
            } else {
                self.options[sel[i]].as_str()
            };
            cx.panel(
                *rr,
                Some(ColorRole::BgRaised),
                Some(ColorRole::Border),
                rr.h * 0.5,
                None,
            );
            let (_, t) = cx.shape(label, &st);
            cx.text(
                label,
                &st,
                Point::new(rr.x + 8.0, rr.y + (rr.h - t.h) * 0.5),
                ColorRole::FgPrimary,
            );
            if !is_add {
                cx.text(
                    "✕",
                    &st,
                    Point::new(rr.right() - 16.0, rr.y + (rr.h - t.h) * 0.5),
                    ColorRole::FgMuted,
                );
            }
            if focused && i == self.focus {
                cx.panel(
                    rr.outset(2.0),
                    None,
                    Some(ColorRole::FocusRing),
                    rr.h * 0.5 + 2.0,
                    None,
                );
            }
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        let sel = self.selected.get(cx.rt);
        node.set_value(
            sel.iter()
                .map(|i| self.options[*i].as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    fn a11y_children(&self, cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let sel = self.selected.get(cx.rt);
        for (i, opt) in sel.iter().enumerate() {
            let mut node = accesskit::Node::new(Role::ListItem);
            node.set_label(crate::trf!(
                "{option} (press Delete to remove)",
                option = self.options[*opt]
            ));
            node.add_action(Action::Click);
            out.push((i as u64, node));
        }
        let mut add = accesskit::Node::new(Role::Button);
        add.set_label(crate::tr!("Add"));
        add.set_toggled(Toggled::False);
        add.add_action(Action::Click);
        out.push((sel.len() as u64, add));
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        Some(self.focus as u64)
    }
}

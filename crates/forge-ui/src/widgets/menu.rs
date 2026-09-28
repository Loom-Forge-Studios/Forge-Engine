//! Menus (§21.16): menu bar, dropdown menus, submenus and context menus.
//!
//! Keyboard (WAI-ARIA menu / menubar): in the bar, Left/Right move between titles and
//! Down, Enter or Space opens one; in a menu, Up/Down move (skipping separators and
//! disabled items), Home/End jump, a letter jumps to the next item starting with it,
//! Enter/Space choose, Right opens a submenu, Left or Escape closes it, and Left/Right on
//! a top-level menu switch to the neighbouring menu of the bar. Shift+F10 or the
//! context-menu key opens a context menu from the keyboard. Choosing raises
//! [`MenuChosen`] with the item id and closes the menu chain; focus returns to where it was.

use std::cell::Cell;
use std::rc::Rc;

use accesskit::{Action, Role, Toggled};

use crate::geom::{Point, Rect, Size};
use crate::id::{Key, WidgetId};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::overlay::{Anchor, PopupSpec};
use crate::style::ColorRole;
use crate::widget::{A11yCx, EventCx, MeasureCx, PaintCx, Widget};

use super::{CONTROL_MIN_H, body, small};

/// One menu entry.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Action {
        id: String,
        label: String,
        shortcut: Option<String>,
        enabled: bool,
        /// `Some`: a checkable item.
        checked: Option<bool>,
    },
    Submenu {
        label: String,
        items: Vec<MenuItem>,
    },
    Separator,
}

impl MenuItem {
    pub fn action(id: &str, label: &str) -> Self {
        MenuItem::Action {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            enabled: true,
            checked: None,
        }
    }
    pub fn shortcut(mut self, s: &str) -> Self {
        if let MenuItem::Action { shortcut, .. } = &mut self {
            *shortcut = Some(s.into());
        }
        self
    }
    pub fn disabled(mut self) -> Self {
        if let MenuItem::Action { enabled, .. } = &mut self {
            *enabled = false;
        }
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        if let MenuItem::Action { checked, .. } = &mut self {
            *checked = Some(on);
        }
        self
    }
    pub fn submenu(label: &str, items: Vec<MenuItem>) -> Self {
        MenuItem::Submenu {
            label: label.into(),
            items,
        }
    }
    fn label(&self) -> &str {
        match self {
            MenuItem::Action { label, .. } | MenuItem::Submenu { label, .. } => label,
            MenuItem::Separator => "",
        }
    }
    fn selectable(&self) -> bool {
        match self {
            MenuItem::Action { enabled, .. } => *enabled,
            MenuItem::Submenu { .. } => true,
            MenuItem::Separator => false,
        }
    }
}

/// Raised when a menu item is chosen. `owner` is the widget that opened the menu chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuChosen {
    pub owner: WidgetId,
    pub id: String,
}

const ROW_H: f32 = 26.0;
const SEP_H: f32 = 9.0;

/// A dropdown / context / sub menu (lives in a popup).
pub struct MenuList {
    items: Vec<MenuItem>,
    owner: WidgetId,
    active: Option<usize>,
    /// Set to ±1 to ask the menu bar to open its neighbouring menu.
    switch: Option<Rc<Cell<i8>>>,
    submenu_open: Option<usize>,
}

impl MenuList {
    pub fn new(items: Vec<MenuItem>, owner: WidgetId) -> Self {
        let active = items.iter().position(MenuItem::selectable);
        Self {
            items,
            owner,
            active,
            switch: None,
            submenu_open: None,
        }
    }
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    fn row_rects(&self, r: Rect) -> Vec<Rect> {
        let mut y = r.y + 4.0;
        self.items
            .iter()
            .map(|it| {
                let h = if matches!(it, MenuItem::Separator) {
                    SEP_H
                } else {
                    ROW_H
                };
                let rr = Rect::new(r.x + 4.0, y, r.w - 8.0, h);
                y += h;
                rr
            })
            .collect()
    }

    fn step(&self, from: Option<usize>, dir: i32) -> Option<usize> {
        let n = self.items.len() as i32;
        if n == 0 {
            return None;
        }
        let mut i = from.map_or(if dir > 0 { -1 } else { n }, |f| f as i32);
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if self.items[i as usize].selectable() {
                return Some(i as usize);
            }
        }
        from
    }

    fn open_submenu(&mut self, cx: &mut EventCx, i: usize) {
        let Some(MenuItem::Submenu { items, .. }) = self.items.get(i) else {
            return;
        };
        let rect = self.row_rects(cx.rect())[i];
        let sub = MenuList::new(items.clone(), self.owner);
        self.submenu_open = Some(i);
        cx.open_popup(
            Key::Index(i as u32),
            PopupSpec::menu(Anchor::Right(rect)),
            sub,
        );
    }

    fn choose(&mut self, cx: &mut EventCx, i: usize) {
        match self.items.get(i).cloned() {
            Some(MenuItem::Action {
                id, enabled: true, ..
            }) => {
                cx.action(MenuChosen {
                    owner: self.owner,
                    id,
                });
                // Close the whole chain: the root menu is the owner's popup.
                cx.close_popup(self.owner_popup_root(cx));
            }
            Some(MenuItem::Submenu { .. }) => self.open_submenu(cx, i),
            _ => {}
        }
    }

    /// The bottom-most popup of this chain (closing it closes the submenus above it).
    fn owner_popup_root(&self, cx: &EventCx) -> WidgetId {
        cx.ui
            .popups
            .iter()
            .find(|p| p.owner == self.owner)
            .map_or(cx.id(), |p| p.id)
    }
}

impl Widget for MenuList {
    fn role(&self) -> Role {
        Role::Menu
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
        let mut w: f32 = 120.0;
        let mut h = 8.0;
        for it in &self.items {
            match it {
                MenuItem::Separator => h += SEP_H,
                MenuItem::Action {
                    label, shortcut, ..
                } => {
                    let lw = cx.text.layout(label, &body(cx.theme), None).1.w;
                    let sw = shortcut.as_ref().map_or(0.0, |s| {
                        cx.text.layout(s, &small(cx.theme), None).1.w + 24.0
                    });
                    w = w.max(lw + sw + 48.0);
                    h += ROW_H;
                }
                MenuItem::Submenu { label, .. } => {
                    w = w.max(cx.text.layout(label, &body(cx.theme), None).1.w + 64.0);
                    h += ROW_H;
                }
            }
        }
        Size::new(w, h)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::Key(k) if k.pressed => {
                match k.code {
                    KeyCode::Down => self.active = self.step(self.active, 1),
                    KeyCode::Up => self.active = self.step(self.active, -1),
                    KeyCode::Home => self.active = self.step(None, 1),
                    KeyCode::End => self.active = self.step(None, -1),
                    KeyCode::Enter | KeyCode::Space => {
                        if let Some(i) = self.active {
                            self.choose(cx, i);
                        }
                    }
                    KeyCode::Right => match self.active {
                        Some(i) if matches!(self.items.get(i), Some(MenuItem::Submenu { .. })) => {
                            self.open_submenu(cx, i)
                        }
                        _ => {
                            if let Some(s) = &self.switch {
                                s.set(1);
                                let id = cx.id();
                                cx.close_popup(id);
                            }
                        }
                    },
                    KeyCode::Left => {
                        if let Some(s) = &self.switch {
                            s.set(-1);
                        }
                        // A submenu closes; a top-level bar menu closes and switches.
                        let id = cx.id();
                        cx.close_popup(id);
                    }
                    KeyCode::Tab => {
                        let id = self.owner_popup_root(cx);
                        cx.close_popup(id);
                        return Handled::No;
                    }
                    KeyCode::Char(c) => {
                        // Type-ahead: the next item whose label starts with `c`.
                        let n = self.items.len();
                        let start = self.active.map_or(0, |a| a + 1);
                        let hit = (0..n).map(|d| (start + d) % n).find(|&i| {
                            self.items[i].selectable()
                                && self.items[i]
                                    .label()
                                    .chars()
                                    .next()
                                    .is_some_and(|f| f.to_lowercase().eq(c.to_lowercase()))
                        });
                        match hit {
                            Some(i) => self.active = Some(i),
                            None => return Handled::No,
                        }
                    }
                    _ => return Handled::No,
                }
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::PointerMove { pos } => {
                let rows = self.row_rects(cx.rect());
                let hit = rows
                    .iter()
                    .position(|r| r.contains(*pos))
                    .filter(|i| self.items[*i].selectable());
                if hit != self.active && hit.is_some() {
                    self.active = hit;
                    if let Some(open) = self.submenu_open.take()
                        && Some(open) != hit
                    {
                        let overlay_child = cx
                            .ui
                            .popups
                            .iter()
                            .find(|p| p.owner == cx.id())
                            .map(|p| p.id);
                        if let Some(p) = overlay_child {
                            cx.close_popup(p);
                        }
                    }
                    if let Some(i) = hit
                        && matches!(self.items[i], MenuItem::Submenu { .. })
                    {
                        self.open_submenu(cx, i);
                    }
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::PointerUp {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                let rows = self.row_rects(cx.rect());
                if let Some(i) = rows.iter().position(|r| r.contains(*pos)) {
                    self.choose(cx, i);
                }
                Handled::Yes
            }
            UiEvent::PointerDown { .. } => Handled::Yes,
            UiEvent::PopupClosed(_) => {
                self.submenu_open = None;
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::A11yChildAction(i, Action::Click) => {
                self.choose(cx, *i as usize);
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
        let rows = self.row_rects(r);
        let style = body(cx.theme());
        let st_small = small(cx.theme());
        for (i, (it, rr)) in self.items.iter().zip(rows).enumerate() {
            match it {
                MenuItem::Separator => {
                    cx.mark(
                        Rect::new(rr.x + 4.0, rr.y + rr.h * 0.5, rr.w - 8.0, 1.0),
                        ColorRole::Border,
                        ColorRole::BgRaised,
                        0.0,
                    );
                }
                _ => {
                    let active = self.active == Some(i);
                    let enabled = it.selectable();
                    let (fg, bg) = if active {
                        cx.fill(rr, ColorRole::Accent, cx.theme().radius.sm);
                        (ColorRole::FgOnAccent, ColorRole::Accent)
                    } else {
                        cx.fill(rr, ColorRole::BgRaised, 0.0);
                        (
                            if enabled {
                                ColorRole::FgPrimary
                            } else {
                                ColorRole::FgMuted
                            },
                            ColorRole::BgRaised,
                        )
                    };
                    let (_, t) = cx.shape(it.label(), &style);
                    let y = rr.y + (rr.h - t.h) * 0.5;
                    if let MenuItem::Action {
                        checked: Some(true),
                        ..
                    } = it
                    {
                        cx.text_on("✓", &style, Point::new(rr.x + 6.0, y), fg, bg);
                    }
                    cx.text_on(it.label(), &style, Point::new(rr.x + 26.0, y), fg, bg);
                    match it {
                        MenuItem::Action {
                            shortcut: Some(s), ..
                        } => {
                            let (_, st) = cx.shape(s, &st_small);
                            let muted = if active { fg } else { ColorRole::FgMuted };
                            cx.text_on(
                                s,
                                &st_small,
                                Point::new(rr.right() - st.w - 8.0, rr.y + (rr.h - st.h) * 0.5),
                                muted,
                                bg,
                            );
                        }
                        MenuItem::Submenu { .. } => {
                            cx.text_on("▸", &style, Point::new(rr.right() - 16.0, y), fg, bg);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Menu"));
    }
    fn a11y_children(&self, _cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let n = self
            .items
            .iter()
            .filter(|i| !matches!(i, MenuItem::Separator))
            .count();
        let mut pos = 0usize;
        for (i, it) in self.items.iter().enumerate() {
            let node = match it {
                MenuItem::Separator => continue,
                MenuItem::Action {
                    label,
                    shortcut,
                    enabled,
                    checked,
                    ..
                } => {
                    let mut node = accesskit::Node::new(if checked.is_some() {
                        Role::MenuItemCheckBox
                    } else {
                        Role::MenuItem
                    });
                    node.set_label(label.as_str());
                    if let Some(s) = shortcut {
                        node.set_keyboard_shortcut(s.as_str());
                    }
                    if !enabled {
                        node.set_disabled();
                    }
                    if let Some(c) = checked {
                        node.set_toggled(if *c { Toggled::True } else { Toggled::False });
                    }
                    node
                }
                MenuItem::Submenu { label, .. } => {
                    let mut node = accesskit::Node::new(Role::MenuItem);
                    node.set_label(label.as_str());
                    node.set_has_popup(accesskit::HasPopup::Menu);
                    node.set_expanded(self.submenu_open == Some(i));
                    node
                }
            };
            pos += 1;
            let mut node = node;
            node.set_position_in_set(pos);
            node.set_size_of_set(n);
            node.add_action(Action::Click);
            out.push((i as u64, node));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        self.active.map(|a| a as u64)
    }
}

/// Open a context menu at `at` for the widget handling the event.
pub fn open_context_menu(cx: &mut EventCx, at: Point, items: Vec<MenuItem>) -> Option<WidgetId> {
    let owner = cx.id();
    cx.open_popup(
        Key::Static("context"),
        PopupSpec::menu(Anchor::At(at)),
        MenuList::new(items, owner),
    )
}

/// True for the gestures that open a context menu: a secondary press, the context-menu
/// key, or Shift+F10. Returns where to open it.
pub fn context_request(cx: &EventCx, ev: &UiEvent) -> Option<Point> {
    match ev {
        UiEvent::PointerDown {
            pos,
            button: PointerButton::Secondary,
            ..
        } => Some(*pos),
        UiEvent::Key(k)
            if k.pressed
                && (k.code == KeyCode::ContextMenu || (k.code == KeyCode::F10 && k.mods.shift)) =>
        {
            let r = cx.rect();
            Some(Point::new(r.x + 8.0, r.bottom()))
        }
        _ => None,
    }
}

/// The application menu bar.
pub struct MenuBar {
    menus: Vec<(String, Vec<MenuItem>)>,
    active: usize,
    open: Option<usize>,
    switch: Rc<Cell<i8>>,
}

impl MenuBar {
    pub fn new(menus: Vec<(&str, Vec<MenuItem>)>) -> Self {
        Self {
            menus: menus.into_iter().map(|(t, i)| (t.to_string(), i)).collect(),
            active: 0,
            open: None,
            switch: Rc::new(Cell::new(0)),
        }
    }
    pub fn open_menu(&self) -> Option<usize> {
        self.open
    }
    /// Replace the menus (an action's enablement or chord changed). The caller marks the
    /// widget for layout (`Ui::invalidate`); a menu already open keeps its items.
    pub fn set_menus(&mut self, menus: Vec<(String, Vec<MenuItem>)>) {
        self.active = self.active.min(menus.len().saturating_sub(1));
        self.menus = menus;
    }
    /// The menus as shown.
    pub fn menus(&self) -> &[(String, Vec<MenuItem>)] {
        &self.menus
    }

    fn title_rects(
        &self,
        cx_text: &mut crate::text::TextSystem,
        theme: &crate::style::Theme,
        r: Rect,
    ) -> Vec<Rect> {
        let mut x = r.x;
        self.menus
            .iter()
            .map(|(t, _)| {
                let w = cx_text.layout(t, &body(theme), None).1.w + 20.0;
                let rr = Rect::new(x, r.y, w, r.h);
                x += w;
                rr
            })
            .collect()
    }

    fn open(&mut self, cx: &mut EventCx, i: usize) {
        let theme = cx.theme().clone();
        let r = cx.rect();
        let rects = self.title_rects(&mut cx.text(), &theme, r);
        let Some(rect) = rects.get(i).copied() else {
            return;
        };
        self.active = i;
        self.open = Some(i);
        self.switch.set(0);
        let mut list = MenuList::new(self.menus[i].1.clone(), cx.id());
        list.switch = Some(self.switch.clone());
        cx.open_popup(
            Key::Static("menu"),
            PopupSpec::menu(Anchor::Below(rect)),
            list,
        );
        cx.request_paint();
        cx.request_a11y();
    }
}

impl Widget for MenuBar {
    fn role(&self) -> Role {
        Role::MenuBar
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
        let w: f32 = self
            .menus
            .iter()
            .map(|(t, _)| cx.text.layout(t, &body(cx.theme), None).1.w + 20.0)
            .sum();
        Size::new(known.width.unwrap_or(w), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let n = self.menus.len();
        if n == 0 {
            return Handled::No;
        }
        match ev {
            UiEvent::Key(k) if k.pressed => {
                match k.code {
                    KeyCode::Right => self.active = (self.active + 1) % n,
                    KeyCode::Left => self.active = (self.active + n - 1) % n,
                    KeyCode::Home => self.active = 0,
                    KeyCode::End => self.active = n - 1,
                    KeyCode::Down | KeyCode::Enter | KeyCode::Space => {
                        let a = self.active;
                        self.open(cx, a);
                        return Handled::Yes;
                    }
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
                let rects = self.title_rects(&mut cx.text(), &theme, r);
                if let Some(i) = rects.iter().position(|r| r.contains(*pos)) {
                    self.open(cx, i);
                }
                Handled::Yes
            }
            UiEvent::PopupClosed(_) => {
                self.open = None;
                let s = self.switch.replace(0);
                if s != 0 {
                    let next = (self.active as i32 + i32::from(s)).rem_euclid(n as i32) as usize;
                    self.open(cx, next);
                }
                cx.request_paint();
                cx.request_a11y();
                Handled::Yes
            }
            UiEvent::A11yChildAction(i, Action::Click | Action::Expand) => {
                self.open(cx, (*i as usize).min(n - 1));
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let theme = cx.theme().clone();
        let rects = self.title_rects(cx.text, &theme, r);
        let style = body(&theme);
        let focused = cx.state().focus_visible;
        for (i, ((t, _), rr)) in self.menus.iter().zip(rects).enumerate() {
            let highlighted = self.open == Some(i) || (focused && self.active == i);
            let (fg, bg) = if highlighted {
                cx.fill(rr.outset(-2.0), ColorRole::BgPressed, theme.radius.sm);
                (ColorRole::FgPrimary, ColorRole::BgPressed)
            } else {
                (ColorRole::FgPrimary, cx.parent_bg())
            };
            let (_, ts) = cx.shape(t, &style);
            cx.text_on(
                t,
                &style,
                Point::new(rr.x + 10.0, rr.y + (rr.h - ts.h) * 0.5),
                fg,
                bg,
            );
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Menu bar"));
    }
    fn a11y_children(&self, _cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        for (i, (t, _)) in self.menus.iter().enumerate() {
            let mut node = accesskit::Node::new(Role::MenuItem);
            node.set_label(t.as_str());
            node.set_has_popup(accesskit::HasPopup::Menu);
            node.set_expanded(self.open == Some(i));
            node.add_action(Action::Click);
            out.push((i as u64, node));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        Some(self.active as u64)
    }
}

/// A region that offers a context menu (right click, the context-menu key, Shift+F10).
pub struct ContextMenuArea {
    label: String,
    items: Vec<MenuItem>,
}

impl ContextMenuArea {
    pub fn new(label: &str, items: Vec<MenuItem>) -> Self {
        Self {
            label: label.to_string(),
            items,
        }
    }
}

impl Widget for ContextMenuArea {
    fn role(&self) -> Role {
        Role::Group
    }
    fn focusable(&self) -> bool {
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
        cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let (_, t) = cx.text.layout(&self.label, &body(cx.theme), None);
        Size::new(
            known.width.unwrap_or(t.w + 32.0),
            known.height.unwrap_or(t.h + 24.0),
        )
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        if let Some(at) = context_request(cx, ev) {
            open_context_menu(cx, at, self.items.clone());
            return Handled::Yes;
        }
        Handled::No
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let radius = cx.theme().radius.md;
        cx.panel(
            r,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            radius,
            None,
        );
        let style = body(cx.theme());
        let (_, t) = cx.shape(&self.label, &style);
        cx.text(
            &self.label,
            &style,
            Point::new(r.x + (r.w - t.w) * 0.5, r.y + (r.h - t.h) * 0.5),
            ColorRole::FgMuted,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_has_popup(accesskit::HasPopup::Menu);
    }
}

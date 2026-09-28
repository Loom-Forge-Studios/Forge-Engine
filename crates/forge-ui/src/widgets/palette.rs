//! Filtered pick lists: the **command palette** (§21.16, §21.17) and the **asset
//! reference picker** (§21.16 "asset reference picker (with drag-drop)").
//!
//! Both open a [`PickList`] popup: a filter line and the fuzzy-ranked items with the
//! matched characters in bold; Up/Down/PageUp/PageDown/Home/End move, Enter picks,
//! Escape closes, and each visible item is an AccessKit option. Filtering 10,000 items
//! per keystroke stays well under a frame (`crate::fuzzy`).
//!
//! * The palette lists commands with their shortcut and category and raises
//!   [`CommandInvoked`]. It is a view over the commands it is given; WP-U3 feeds it
//!   every registered command.
//! * The asset picker edits a `Signal<Option<String>>` (an asset path) of one asset kind.
//!   It opens the list with Enter/Space/Alt+Down or a click, clears with Delete, and
//!   accepts a dragged asset of its kind — refusing any other kind **with the reason** in
//!   the drop preview. Assets come from an [`AssetIndex`]; [`MemoryAssetIndex`] is the
//!   in-memory stand-in (D-4) until the asset database (Ch.8) lands.

use std::rc::Rc;

use accesskit::{Action, Role};

use crate::dnd::{DragPayload, DropVerdict};
use crate::fuzzy::rank;
use crate::geom::{Point, Rect, Size};
use crate::id::{Key, WidgetId};
use crate::input::{Handled, KeyCode, PointerButton, UiEvent};
use crate::overlay::{Anchor, PopupSpec};
use crate::state::Signal;
use crate::style::ColorRole;
use crate::text::RichSpan;
use crate::widget::{A11yCx, Binder, EventCx, MeasureCx, PaintCx, Widget};

use super::line_edit::{EditOutcome, LineEdit};
use super::{CONTROL_MIN_H, body, control_pad_x, small};

const ROW_H: f32 = 28.0;
const MAX_ROWS: usize = 12;

/// One pickable item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickItem {
    pub id: String,
    pub label: String,
    /// Right-aligned detail (a shortcut, an asset kind).
    pub detail: String,
    /// A muted prefix (a category, a folder).
    pub hint: String,
    /// Added to the fuzzy score: recently used commands rank first (Ch.21.17).
    pub boost: i32,
}

impl PickItem {
    pub fn new(id: &str, label: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            detail: String::new(),
            hint: String::new(),
            boost: 0,
        }
    }
    pub fn detail(mut self, d: &str) -> Self {
        self.detail = d.to_string();
        self
    }
    pub fn hint(mut self, h: &str) -> Self {
        self.hint = h.to_string();
        self
    }
    pub fn boost(mut self, b: i32) -> Self {
        self.boost = b;
        self
    }
}

/// The palette ran a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandInvoked {
    pub id: String,
}

/// A filter line over ranked items (see the module docs).
pub struct PickList {
    items: Rc<Vec<PickItem>>,
    filter: LineEdit,
    shown: Vec<(usize, Vec<usize>)>,
    active: usize,
    top: usize,
    sink: Option<Signal<Option<String>>>,
    invoke: bool,
    label: String,
    placeholder: String,
}

impl PickList {
    fn new(items: Rc<Vec<PickItem>>, label: &str, placeholder: &str) -> Self {
        let mut s = Self {
            items,
            filter: LineEdit::default(),
            shown: Vec::new(),
            active: 0,
            top: 0,
            sink: None,
            invoke: false,
            label: label.to_string(),
            placeholder: placeholder.to_string(),
        };
        s.refilter();
        s
    }
    /// A pick list that writes the chosen item's id into `sink` and closes (the node
    /// canvas's node search opens one at the pointer). Open it with
    /// [`crate::widget::EventCx::open_popup`].
    pub fn into_sink(
        items: Rc<Vec<PickItem>>,
        label: &str,
        placeholder: &str,
        sink: Signal<Option<String>>,
    ) -> Self {
        let mut s = Self::new(items, label, placeholder);
        s.sink = Some(sink);
        s
    }
    /// Items shown now (indices into the items), best first.
    pub fn shown(&self) -> Vec<usize> {
        self.shown.iter().map(|(i, _)| *i).collect()
    }
    pub fn query(&self) -> &str {
        &self.filter.text
    }
    fn refilter(&mut self) {
        let items = self.items.clone();
        let mut ranked = rank(&self.filter.text, &items, |it| it.label.as_str());
        // Recency (and any other boost) on top of the match score; ties keep item order.
        ranked.sort_by_key(|(i, m)| (std::cmp::Reverse(m.score + items[*i].boost), *i));
        self.shown = ranked.into_iter().map(|(i, m)| (i, m.positions)).collect();
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
    fn pick(&mut self, cx: &mut EventCx) {
        let Some(item) = self
            .shown
            .get(self.active)
            .and_then(|(i, _)| self.items.get(*i))
        else {
            return;
        };
        let id = item.id.clone();
        if let Some(s) = self.sink {
            s.set(cx.rt_mut(), Some(id.clone()));
        }
        if self.invoke {
            cx.action(CommandInvoked { id });
        }
        let me = cx.id();
        cx.close_popup(me);
    }
    fn spans(label: &str, matched: &[usize]) -> Vec<(std::ops::Range<usize>, bool)> {
        let mut out: Vec<(std::ops::Range<usize>, bool)> = Vec::new();
        for (b, ch) in label.char_indices() {
            let m = matched.contains(&b);
            let end = b + ch.len_utf8();
            match out.last_mut() {
                Some((r, mm)) if *mm == m => r.end = end,
                _ => out.push((b..end, m)),
            }
        }
        out
    }
}

impl Widget for PickList {
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
        _cx: &mut MeasureCx,
        known: taffy::Size<Option<f32>>,
        _a: taffy::Size<taffy::AvailableSpace>,
    ) -> Size {
        let rows = self.items.len().clamp(1, MAX_ROWS) as f32;
        Size::new(
            known.width.unwrap_or(420.0),
            CONTROL_MIN_H + 12.0 + rows * ROW_H,
        )
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        let n = self.shown.len();
        match ev {
            UiEvent::Key(k) if k.pressed => {
                match k.code {
                    KeyCode::Down if n > 0 => self.active = (self.active + 1) % n,
                    KeyCode::Up if n > 0 => self.active = (self.active + n - 1) % n,
                    KeyCode::PageDown if n > 0 => self.active = (self.active + MAX_ROWS).min(n - 1),
                    KeyCode::PageUp => self.active = self.active.saturating_sub(MAX_ROWS),
                    KeyCode::Home if k.mods.ctrl => self.active = 0,
                    KeyCode::End if k.mods.ctrl && n > 0 => self.active = n - 1,
                    KeyCode::Tab => {
                        let me = cx.id();
                        cx.close_popup(me);
                        return Handled::No;
                    }
                    _ => match self.filter.key(k, cx.clipboard()) {
                        EditOutcome::Submit => {
                            self.pick(cx);
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
                cx.request_a11y();
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
                let y0 = cx.rect().y + CONTROL_MIN_H + 12.0;
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
                if pos.y >= cx.rect().y + CONTROL_MIN_H + 12.0 {
                    self.pick(cx);
                }
                Handled::Yes
            }
            UiEvent::PointerDown { .. } => Handled::Yes,
            UiEvent::A11yChildAction(i, Action::Click) => {
                if let Some(p) = self.shown.iter().position(|(x, _)| *x as u64 == *i) {
                    self.active = p;
                    self.pick(cx);
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let theme = cx.theme().clone();
        cx.panel(
            r,
            Some(ColorRole::BgRaised),
            Some(ColorRole::Border),
            theme.radius.md,
            Some(theme.shadows[2]),
        );
        let style = body(&theme);
        let f = Rect::new(r.x + 6.0, r.y + 6.0, r.w - 12.0, CONTROL_MIN_H);
        cx.panel(
            f,
            Some(ColorRole::BgSunken),
            Some(ColorRole::Border),
            theme.radius.sm,
            None,
        );
        let pad = control_pad_x(&theme) * 0.5;
        self.filter.paint_line(
            cx,
            Rect::new(f.x + pad, f.y, f.w - 2.0 * pad, f.h),
            &style,
            true,
            true,
            &self.placeholder,
            ColorRole::BgSunken,
        );
        let y0 = r.y + CONTROL_MIN_H + 12.0;
        if self.shown.is_empty() {
            cx.text(
                crate::tr!("No matches"),
                &style,
                Point::new(r.x + 12.0, y0 + 4.0),
                ColorRole::FgMuted,
            );
        }
        let st = small(&theme);
        for (row, (idx, matched)) in self.shown.iter().enumerate().skip(self.top).take(MAX_ROWS) {
            let Some(item) = self.items.get(*idx) else {
                continue;
            };
            let rr = Rect::new(
                r.x + 6.0,
                y0 + (row - self.top) as f32 * ROW_H,
                r.w - 12.0,
                ROW_H,
            );
            let active = row == self.active;
            let (fg, muted, bg) = if active {
                cx.fill(rr, ColorRole::Accent, theme.radius.sm);
                (
                    ColorRole::FgOnAccent,
                    ColorRole::FgOnAccent,
                    ColorRole::Accent,
                )
            } else {
                cx.fill(rr, ColorRole::BgRaised, 0.0);
                (
                    ColorRole::FgPrimary,
                    ColorRole::FgMuted,
                    ColorRole::BgRaised,
                )
            };
            let mut x = rr.x + 8.0;
            if !item.hint.is_empty() {
                let hint = format!("{}: ", item.hint);
                let (run, hs) = cx.shape(&hint, &st);
                cx.run_on(run, Point::new(x, rr.y + (rr.h - hs.h) * 0.5), muted, bg);
                x += hs.w;
            }
            let spans: Vec<RichSpan<'_>> = Self::spans(&item.label, matched)
                .into_iter()
                .map(|(range, m)| RichSpan {
                    weight: m.then_some(700),
                    ..RichSpan::plain(&item.label[range])
                })
                .collect();
            let h = cx.shape(&item.label, &style).1.h;
            // The row fill above made `bg` the current background, so the rich text's
            // colour is contrast-checked against it.
            cx.rich_text(
                &spans,
                &style,
                Point::new(x, rr.y + (rr.h - h) * 0.5),
                &[fg],
            );
            if !item.detail.is_empty() {
                let (run, ds) = cx.shape(&item.detail, &st);
                cx.run_on(
                    run,
                    Point::new(rr.right() - ds.w - 8.0, rr.y + (rr.h - ds.h) * 0.5),
                    muted,
                    bg,
                );
            }
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        node.set_value(self.filter.text.as_str());
    }
    fn a11y_children(&self, _cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        let n = self.shown.len();
        for (row, (idx, _)) in self.shown.iter().enumerate().skip(self.top).take(MAX_ROWS) {
            let Some(item) = self.items.get(*idx) else {
                continue;
            };
            let mut node = accesskit::Node::new(Role::ListBoxOption);
            let mut name = item.label.clone();
            if !item.detail.is_empty() {
                name.push_str(&format!(" ({})", item.detail));
            }
            node.set_label(name);
            node.set_selected(row == self.active);
            node.set_position_in_set(row + 1);
            node.set_size_of_set(n);
            node.add_action(Action::Click);
            out.push((*idx as u64, node));
        }
    }
    fn a11y_focus(&self, _rt: &crate::state::Runtime) -> Option<u64> {
        self.shown.get(self.active).map(|(i, _)| *i as u64)
    }
}

/// Open the command palette over `commands` (top centre of the window, focused). Picking
/// raises [`CommandInvoked`].
pub fn open_command_palette(cx: &mut EventCx, commands: Rc<Vec<PickItem>>) -> Option<WidgetId> {
    let mut list = PickList::new(
        commands,
        crate::tr!("Command palette"),
        crate::tr!("Type a command"),
    );
    list.invoke = true;
    cx.open_popup(
        Key::Static("command-palette"),
        PopupSpec::menu(Anchor::TopCenter).min_width(480.0),
        list,
    )
}

/// Open the command palette from application code (a global shortcut).
pub fn open_command_palette_ui(
    ui: &mut crate::Ui,
    owner: WidgetId,
    commands: Rc<Vec<PickItem>>,
) -> Option<WidgetId> {
    let mut list = PickList::new(
        commands,
        crate::tr!("Command palette"),
        crate::tr!("Type a command"),
    );
    list.invoke = true;
    ui.open_popup(
        owner,
        Key::Static("command-palette"),
        PopupSpec::menu(Anchor::TopCenter).min_width(480.0),
        list,
    )
    .ok()
}

/// A button that opens the palette (Ctrl+Shift+P in the editor is bound by WP-U3's keymap).
pub struct CommandPaletteButton {
    commands: Rc<Vec<PickItem>>,
    popup: Option<WidgetId>,
}

impl CommandPaletteButton {
    pub fn new(commands: Vec<PickItem>) -> Self {
        Self {
            commands: Rc::new(commands),
            popup: None,
        }
    }
    pub fn is_open(&self) -> bool {
        self.popup.is_some()
    }
}

impl Widget for CommandPaletteButton {
    fn role(&self) -> Role {
        Role::Button
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
        let (_, t) = cx
            .text
            .layout(crate::tr!("Commands…  Ctrl+Shift+P"), &body(cx.theme), None);
        Size::new(t.w + 2.0 * control_pad_x(cx.theme), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::PopupClosed(p) if Some(*p) == self.popup => {
                self.popup = None;
                Handled::Yes
            }
            e if super::activation(e) => {
                self.popup = open_command_palette(cx, self.commands.clone());
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let bg = if cx.state().hovered {
            ColorRole::BgHover
        } else {
            ColorRole::BgSunken
        };
        cx.panel(
            r,
            Some(bg),
            Some(ColorRole::Border),
            cx.theme().radius.sm,
            None,
        );
        let style = body(cx.theme());
        let (run, t) = cx.shape(crate::tr!("Commands…"), &style);
        let pad = control_pad_x(cx.theme());
        cx.run(
            run,
            Point::new(r.x + pad, r.y + (r.h - t.h) * 0.5),
            ColorRole::FgMuted,
        );
        let st = small(cx.theme());
        let (run, s) = cx.shape("Ctrl+Shift+P", &st);
        cx.run(
            run,
            Point::new(r.right() - s.w - pad, r.y + (r.h - s.h) * 0.5),
            ColorRole::FgMuted,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(crate::tr!("Command palette"));
        node.set_keyboard_shortcut("Ctrl+Shift+P");
        node.set_has_popup(accesskit::HasPopup::Listbox);
        node.add_action(Action::Click);
    }
}

// ---- asset references --------------------------------------------------------------------

/// An asset the picker can reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetEntry {
    /// The asset's path (its identity in a reference).
    pub path: String,
    /// Its kind (`Texture`, `Mesh`, `Material`, ...).
    pub kind: String,
}

impl AssetEntry {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// Where the picker finds assets (the asset database, Ch.8).
pub trait AssetIndex {
    fn get(&self, path: &str) -> Option<AssetEntry>;
    /// Every asset of `kind`.
    fn of_kind(&self, kind: &str) -> Vec<AssetEntry>;
}

/// **In-memory stand-in (D-4)** for the asset database until Ch.8 lands. Not the real
/// index: it holds what it is given.
#[derive(Clone, Debug, Default)]
pub struct MemoryAssetIndex {
    pub entries: Vec<AssetEntry>,
}

impl MemoryAssetIndex {
    pub fn new(entries: &[(&str, &str)]) -> Self {
        Self {
            entries: entries
                .iter()
                .map(|(p, k)| AssetEntry {
                    path: p.to_string(),
                    kind: k.to_string(),
                })
                .collect(),
        }
    }
}

impl AssetIndex for MemoryAssetIndex {
    fn get(&self, path: &str) -> Option<AssetEntry> {
        self.entries.iter().find(|e| e.path == path).cloned()
    }
    fn of_kind(&self, kind: &str) -> Vec<AssetEntry> {
        self.entries
            .iter()
            .filter(|e| e.kind == kind)
            .cloned()
            .collect()
    }
}

/// The reference changed (pick, drop or clear). The owner emits the command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRefChanged {
    pub picker: WidgetId,
    pub path: Option<String>,
}

/// An asset reference field of one kind.
pub struct AssetRefPicker {
    value: Signal<Option<String>>,
    /// Written by the pick list; bound at insertion.
    choice: Option<Signal<Option<String>>>,
    kind: String,
    index: Rc<dyn AssetIndex>,
    label: String,
    popup: Option<WidgetId>,
    drop_state: Option<DropVerdict>,
}

impl AssetRefPicker {
    /// `choice` is a scratch signal the picker owns (`rt.signal(None)`).
    pub fn new(
        value: Signal<Option<String>>,
        choice: Signal<Option<String>>,
        kind: &str,
        index: Rc<dyn AssetIndex>,
        label: &str,
    ) -> Self {
        Self {
            value,
            choice: Some(choice),
            kind: kind.to_string(),
            index,
            label: label.to_string(),
            popup: None,
            drop_state: None,
        }
    }
    pub fn is_open(&self) -> bool {
        self.popup.is_some()
    }
    fn set(&mut self, cx: &mut EventCx, path: Option<String>) {
        if self.value.get(cx.rt()) == path {
            return;
        }
        self.value.set(cx.rt_mut(), path.clone());
        let id = cx.id();
        cx.action(AssetRefChanged { picker: id, path });
    }
    fn open(&mut self, cx: &mut EventCx) {
        let items: Vec<PickItem> = self
            .index
            .of_kind(&self.kind)
            .into_iter()
            .map(|e| {
                PickItem::new(&e.path, e.name())
                    .detail(&e.kind)
                    .hint(e.path.rsplit_once('/').map_or("", |(d, _)| d))
            })
            .collect();
        let mut list = PickList::new(
            Rc::new(items),
            &format!("{} ({})", self.label, self.kind),
            crate::tr!("Filter assets"),
        );
        if let Some(c) = self.choice {
            c.set(cx.rt_mut(), None);
            list.sink = Some(c);
        }
        let r = cx.rect();
        self.popup = cx.open_popup(
            Key::Static("assets"),
            PopupSpec::menu(Anchor::Below(r)).min_width(r.w.max(320.0)),
            list,
        );
    }
    /// The verdict for dropping `path` here.
    pub fn verdict(&self, path: &str) -> DropVerdict {
        match self.index.get(path) {
            Some(e) if e.kind == self.kind => DropVerdict::Accepted,
            Some(e) => DropVerdict::Refused(crate::trf!(
                "expects a {want}, not a {got}",
                want = self.kind,
                got = e.kind
            )),
            None => DropVerdict::Refused(crate::tr!("not in the asset index").into()),
        }
    }
}

impl Widget for AssetRefPicker {
    fn role(&self) -> Role {
        Role::ComboBox
    }
    fn bind(&self, b: &mut Binder) {
        b.watch(
            Some(self.value.any()),
            crate::damage::Dirty::PAINT | crate::damage::Dirty::A11Y,
        );
        b.watch(self.choice.map(|c| c.any()), crate::damage::Dirty::NONE);
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
        Size::new(known.width.unwrap_or(240.0), CONTROL_MIN_H)
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::BindingChanged => {
                if let Some(c) = self.choice
                    && let Some(path) = c.get(cx.rt())
                {
                    c.set(cx.rt_mut(), None);
                    self.set(cx, Some(path));
                }
                Handled::Yes
            }
            UiEvent::PopupClosed(p) if Some(*p) == self.popup => {
                self.popup = None;
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed => {
                match k.code {
                    KeyCode::Enter | KeyCode::Space => self.open(cx),
                    KeyCode::Down if k.mods.alt => self.open(cx),
                    KeyCode::Delete | KeyCode::Backspace => self.set(cx, None),
                    _ => return Handled::No,
                }
                Handled::Yes
            }
            UiEvent::PointerDown {
                button: PointerButton::Primary,
                pos,
                ..
            } => {
                let r = cx.rect();
                if self.value.get(cx.rt()).is_some() && pos.x > r.right() - 24.0 {
                    self.set(cx, None);
                } else {
                    self.open(cx);
                }
                Handled::Yes
            }
            UiEvent::A11yAction(Action::Click | Action::Expand) => {
                self.open(cx);
                Handled::Yes
            }
            UiEvent::DragOver(DragPayload::Asset(path)) => {
                let v = self.verdict(path);
                cx.set_drop_verdict(v.clone());
                if self.drop_state.as_ref() != Some(&v) {
                    self.drop_state = Some(v);
                    cx.request_paint();
                }
                Handled::Yes
            }
            UiEvent::DragOver(_) => {
                cx.set_drop_verdict(DropVerdict::Refused(crate::trf!(
                    "drop a {kind} asset here",
                    kind = self.kind
                )));
                Handled::Yes
            }
            UiEvent::Drop(DragPayload::Asset(path)) => {
                if self.verdict(path) == DropVerdict::Accepted {
                    self.set(cx, Some(path.clone()));
                }
                self.drop_state = None;
                cx.request_paint();
                Handled::Yes
            }
            UiEvent::PointerLeave => {
                if self.drop_state.take().is_some() {
                    cx.request_paint();
                }
                Handled::No
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let theme = cx.theme().clone();
        let border = match &self.drop_state {
            Some(DropVerdict::Accepted) => ColorRole::Accent,
            Some(DropVerdict::Refused(_)) => ColorRole::Danger,
            _ => ColorRole::Border,
        };
        let bg = if cx.state().hovered {
            ColorRole::BgHover
        } else {
            ColorRole::BgSunken
        };
        cx.panel(r, Some(bg), Some(border), theme.radius.sm, None);
        let style = body(&theme);
        let pad = control_pad_x(&theme) * 0.75;
        let (text, fg) = match self.value.get(cx.rt()) {
            Some(p) => (
                self.index
                    .get(&p)
                    .map_or_else(|| crate::trf!("{p} (missing)", p), |e| e.name().to_string()),
                ColorRole::FgPrimary,
            ),
            None => (
                crate::trf!("None ({kind})", kind = self.kind),
                ColorRole::FgMuted,
            ),
        };
        let (run, t) = cx.shape(&text, &style);
        cx.run(run, Point::new(r.x + pad, r.y + (r.h - t.h) * 0.5), fg);
        let glyph = if self.value.get(cx.rt()).is_some() {
            "✕"
        } else {
            "▾"
        };
        let (run, g) = cx.shape(glyph, &style);
        cx.run(
            run,
            Point::new(r.right() - g.w - pad, r.y + (r.h - g.h) * 0.5),
            ColorRole::FgMuted,
        );
        if let Some(DropVerdict::Refused(why)) = &self.drop_state {
            let st = small(&theme);
            let (run, s) = cx.shape(why, &st);
            cx.run(
                run,
                Point::new(r.right() - s.w - pad - 20.0, r.y + (r.h - s.h) * 0.5),
                ColorRole::FgMuted,
            );
        }
    }
    fn a11y(&self, cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.label.as_str());
        match self.value.get(cx.rt) {
            Some(p) => node.set_value(p),
            None => node.set_value(crate::trf!("None ({kind})", kind = self.kind)),
        }
        node.set_has_popup(accesskit::HasPopup::Listbox);
        node.set_expanded(self.popup.is_some());
        node.add_action(Action::Click);
        node.add_action(Action::Expand);
    }
}

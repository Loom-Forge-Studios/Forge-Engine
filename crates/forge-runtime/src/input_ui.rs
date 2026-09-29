//! Game-facing input UI (Ch.28 §28.13, §28.15, §28.17; DoD M7-12) on `forge-ui`, the same
//! retained layer as the rest of the game UI, linking no editor crate:
//!
//! * [`ControlsScreen`] — the player's **rebinding screen**: every action with its keyboard
//!   and gamepad bindings shown as platform glyphs (a composite shows one button per
//!   direction). Pressing one listens for the next fitting control (`forge-input`'s
//!   interactive rebinding, conflicts swapped and named), shows the result, and **saves the
//!   overrides** to the player's profile at once, so a crash never loses a rebinding. Reset
//!   restores the authored bindings. Every visible string is a localisation key
//!   ([`controls_strings`]); action names come from `input.<context>.<action>` keys when the
//!   game's tables have them, else the authored name.
//! * [`OnScreenControlsView`] — draws `forge-input`'s on-screen controls (virtual sticks and
//!   buttons driving a virtual gamepad) and lets a mouse stand in for a finger on desktop.
//! * [`InputPadSource`] — the menus' gamepad source (`forge_ui::game::GamepadSource`) read
//!   from the input runtime's real pads, with the connected pad's glyph family.

use std::cell::RefCell;
use std::rc::Rc;

use forge_input::control::{Binding, ControlRef, Device, control_index};
use forge_input::glyph::{GlyphContext, PadFamily};
use forge_input::runtime::{DeviceFilter, RebindRequest, RebindState};
use forge_input::{
    ActionHandle, ActionKind, InputRuntime, OnScreenControls, OverrideStore, PlayerId,
};
use forge_ui::game::{GamepadSource, GlyphSet, PadButton, PadInput};
use forge_ui::l10n::Localiser;
use forge_ui::text::TextStyle;
use forge_ui::widget::{A11yCx, EventCx, PaintCx};
use forge_ui::widgets::{Button, Container, Label, LabelKind, Pressed};
use forge_ui::{
    ActionEnvelope, ColorRole, Handled, NodeStyle, Point, Rect, Role, Signal, Ui, UiError, UiEvent,
    Widget, WidgetId,
};

/// The rebinding screen's string table (source locale `en`; the pseudo-locale is derived).
pub fn controls_strings() -> Localiser {
    Localiser::new("en")
        .with(
            "en",
            &[
                ("controls.title", "Controls"),
                ("controls.keyboard", "Keyboard and mouse"),
                ("controls.gamepad", "Gamepad"),
                ("controls.reset", "Reset to defaults"),
                ("controls.unbound", "\u{2014}"),
                (
                    "controls.prompt",
                    "Press a key or button for {action} ({cancel} cancels)",
                ),
                ("controls.bound", "{action}: {binding}"),
                (
                    "controls.swapped",
                    "{action}: {binding} (moved from {other})",
                ),
                ("controls.canceled", "Unchanged."),
                ("controls.timeout", "No button was pressed; unchanged."),
                ("controls.refused", "Not bound: {reason}"),
                (
                    "controls.saved_failed",
                    "The bindings could not be saved: {reason}",
                ),
                (
                    "controls.reset_done",
                    "All bindings are back to their defaults.",
                ),
                ("controls.part.0", "up"),
                ("controls.part.1", "down"),
                ("controls.part.2", "left"),
                ("controls.part.3", "right"),
                ("controls.part.neg", "negative"),
                ("controls.part.pos", "positive"),
                ("onscreen.controls", "On-screen controls"),
                ("onscreen.move", "Move"),
                ("onscreen.south", "Jump"),
                ("onscreen.east", "Back"),
                ("onscreen.start", "Pause"),
            ],
        )
        .with(
            "fr",
            &[
                ("controls.title", "Commandes"),
                ("controls.keyboard", "Clavier et souris"),
                ("controls.gamepad", "Manette"),
                ("controls.reset", "Valeurs par d\u{e9}faut"),
                ("controls.unbound", "\u{2014}"),
                (
                    "controls.prompt",
                    "Appuyez sur une touche pour {action} ({cancel} annule)",
                ),
                ("controls.bound", "{action} : {binding}"),
                (
                    "controls.swapped",
                    "{action} : {binding} (pris \u{e0} {other})",
                ),
                ("controls.canceled", "Inchang\u{e9}."),
                ("controls.timeout", "Aucune touche ; inchang\u{e9}."),
                ("controls.refused", "Non assign\u{e9} : {reason}"),
                (
                    "controls.saved_failed",
                    "Les commandes n\u{2019}ont pas pu \u{ea}tre enregistr\u{e9}es : {reason}",
                ),
                (
                    "controls.reset_done",
                    "Toutes les commandes sont revenues par d\u{e9}faut.",
                ),
                ("controls.part.0", "haut"),
                ("controls.part.1", "bas"),
                ("controls.part.2", "gauche"),
                ("controls.part.3", "droite"),
                ("controls.part.neg", "n\u{e9}gatif"),
                ("controls.part.pos", "positif"),
                ("onscreen.controls", "Commandes tactiles"),
                ("onscreen.move", "D\u{e9}placer"),
                ("onscreen.south", "Sauter"),
                ("onscreen.east", "Retour"),
                ("onscreen.start", "Pause"),
            ],
        )
}

/// Which bindings a column shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    KeyboardMouse,
    Gamepad,
}

impl Column {
    fn holds(self, d: Device) -> bool {
        match self {
            Column::KeyboardMouse => matches!(d, Device::Keyboard | Device::Mouse),
            Column::Gamepad => d == Device::Gamepad,
        }
    }
    /// A slot a column's first binding goes in when the action has none there (above the
    /// authored slots, so it never collides with one).
    fn spare_slot(self) -> u32 {
        match self {
            Column::KeyboardMouse => 1000,
            Column::Gamepad => 1001,
        }
    }
}

/// One rebindable button.
#[derive(Clone, Debug)]
struct Cell {
    button: WidgetId,
    label: Signal<String>,
    action: ActionHandle,
    column: Column,
    slot: u32,
    part: Option<u8>,
}

/// What the screen tells the game.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlsEvent {
    /// The player's bindings changed (and were saved).
    Changed,
}

/// The rebinding screen (see the module docs).
pub struct ControlsScreen {
    root: WidgetId,
    cells: Vec<Cell>,
    reset: WidgetId,
    status: Signal<String>,
    player: PlayerId,
    loc: Localiser,
    glyphs: GlyphContext,
    listening: Option<usize>,
    events: Vec<ControlsEvent>,
}

impl ControlsScreen {
    /// Build under `parent`: a title, one row per action of the map, Reset, and a status line.
    pub fn build(
        ui: &mut Ui,
        parent: WidgetId,
        rt: &InputRuntime,
        player: PlayerId,
        loc: Localiser,
        glyphs: GlyphContext,
    ) -> Result<ControlsScreen, UiError> {
        let gap = ui.theme().space[2];
        let root = ui.add(
            parent,
            "controls",
            NodeStyle::column(gap),
            Container::pane(&loc.get("controls.title", &[])),
        )?;
        ui.add(
            root,
            "title",
            NodeStyle::leaf(),
            Label::new(loc.get("controls.title", &[])).heading(),
        )?;
        let head = ui.add(root, "head", NodeStyle::row(gap), Container::group())?;
        ui.add(
            head,
            "action",
            NodeStyle::leaf().width(200.0),
            Container::group(),
        )?;
        for (k, key) in [("kbm", "controls.keyboard"), ("pad", "controls.gamepad")] {
            ui.add(
                head,
                k,
                NodeStyle::leaf().width(260.0),
                Label::new(loc.get(key, &[])).kind(LabelKind::Small),
            )?;
        }
        let mut s = ControlsScreen {
            root,
            cells: Vec::new(),
            reset: root,
            status: ui.rt_mut().signal(String::new()),
            player,
            loc,
            glyphs,
            listening: None,
            events: Vec::new(),
        };
        let map = rt.map();
        for a in map.actions() {
            let path = map.path(a).to_string();
            let key = format!("action_{}", path.replace('/', "_"));
            let row = ui.add(
                root,
                forge_ui::Key::Str(key.as_str().into()),
                NodeStyle::row(gap),
                Container::group(),
            )?;
            ui.add(
                row,
                "name",
                NodeStyle::leaf().width(200.0),
                Label::new(s.action_name(rt, a)),
            )?;
            for column in [Column::KeyboardMouse, Column::Gamepad] {
                let colkey = match column {
                    Column::KeyboardMouse => "kbm",
                    Column::Gamepad => "pad",
                };
                let cell = ui.add(
                    row,
                    colkey,
                    NodeStyle::row(4.0).width(260.0),
                    Container::group(),
                )?;
                for (i, (slot, part)) in s.slots(rt, a, column).into_iter().enumerate() {
                    let label = ui.rt_mut().signal(String::new());
                    let b = ui.add(
                        cell,
                        forge_ui::Key::Index(u32::try_from(i).unwrap_or(u32::MAX)),
                        NodeStyle::leaf().min_size(56.0, 0.0),
                        Button::new(label),
                    )?;
                    s.cells.push(Cell {
                        button: b,
                        label,
                        action: a,
                        column,
                        slot,
                        part,
                    });
                }
            }
        }
        s.reset = ui.add(
            root,
            "reset",
            NodeStyle::leaf().width(260.0),
            Button::new(s.loc.get("controls.reset", &[])),
        )?;
        ui.add(
            root,
            "status",
            NodeStyle::leaf(),
            Label::new(s.status).kind(LabelKind::Small),
        )?;
        s.relabel(ui, rt);
        Ok(s)
    }

    pub fn root(&self) -> WidgetId {
        self.root
    }

    /// The button for an action's column (the first one; a composite has one per part).
    pub fn button(&self, action: ActionHandle, column: Column) -> Option<WidgetId> {
        self.cells
            .iter()
            .find(|c| c.action == action && c.column == column)
            .map(|c| c.button)
    }

    pub fn buttons(&self, action: ActionHandle, column: Column) -> Vec<WidgetId> {
        self.cells
            .iter()
            .filter(|c| c.action == action && c.column == column)
            .map(|c| c.button)
            .collect()
    }

    pub fn reset_button(&self) -> WidgetId {
        self.reset
    }

    /// What a button shows now.
    pub fn label_of(&self, ui: &Ui, button: WidgetId) -> Option<String> {
        self.cells
            .iter()
            .find(|c| c.button == button)
            .map(|c| c.label.get(ui.rt()))
    }

    pub fn status(&self, ui: &Ui) -> String {
        self.status.get(ui.rt())
    }

    pub fn take_events(&mut self) -> Vec<ControlsEvent> {
        std::mem::take(&mut self.events)
    }

    fn action_name(&self, rt: &InputRuntime, a: ActionHandle) -> String {
        let key = format!("input.{}", rt.map().path(a).replace('/', "."));
        if self.loc.raw(self.loc.source(), &key).is_some() {
            self.loc.get(&key, &[])
        } else {
            rt.map().name(a).to_string()
        }
    }

    /// The slots (and composite parts) a column shows for an action.
    fn slots(&self, rt: &InputRuntime, a: ActionHandle, column: Column) -> Vec<(u32, Option<u8>)> {
        let found = rt
            .bindings(self.player, a)
            .into_iter()
            .find(|(_, b)| b.binding.controls().iter().all(|c| column.holds(c.device)));
        match found {
            Some((slot, b)) => match b.binding {
                Binding::Composite2D(_) => (0..4).map(|p| (slot, Some(p))).collect(),
                Binding::Composite1D(..) => (0..2).map(|p| (slot, Some(p))).collect(),
                Binding::Control(_) => vec![(slot, None)],
            },
            None => vec![(column.spare_slot(), None)],
        }
    }

    fn glyph(&self, rt: &InputRuntime, c: &Cell) -> String {
        let b = rt
            .bindings(self.player, c.action)
            .into_iter()
            .find(|(s, _)| *s == c.slot)
            .map(|(_, b)| b.binding);
        let ctl: Option<ControlRef> = match (&b, c.part) {
            (Some(Binding::Control(x)), None) => Some(x.clone()),
            (Some(Binding::Composite2D(p)), Some(k)) => p.get(usize::from(k)).cloned(),
            (Some(Binding::Composite1D(n, p)), Some(k)) => Some(if k == 0 { n } else { p }.clone()),
            _ => None,
        };
        match ctl {
            Some(x) => self.glyphs.control(&x).label,
            None => self.loc.get("controls.unbound", &[]),
        }
    }

    fn relabel(&self, ui: &mut Ui, rt: &InputRuntime) {
        for c in &self.cells {
            let t = self.glyph(rt, c);
            if c.label.get(ui.rt()) != t {
                c.label.set(ui.rt_mut(), t);
            }
        }
    }

    fn part_name(&self, c: &Cell, kind: ActionKind) -> Option<String> {
        let k = c.part?;
        let key = match (kind, k) {
            (ActionKind::Axis1D, 0) => "controls.part.neg".to_string(),
            (ActionKind::Axis1D, _) => "controls.part.pos".to_string(),
            _ => format!("controls.part.{k}"),
        };
        Some(self.loc.get(&key, &[]))
    }

    /// Handle the window's actions (a binding button starts listening; Reset resets).
    pub fn on_actions(&mut self, ui: &mut Ui, rt: &mut InputRuntime, actions: &[ActionEnvelope]) {
        for env in actions {
            let Some(Pressed(id)) = env.action.downcast_ref::<Pressed>() else {
                continue;
            };
            if *id == self.reset {
                rt.reset_bindings(self.player, None);
                rt.cancel_rebind();
                self.listening = None;
                self.status
                    .set(ui.rt_mut(), self.loc.get("controls.reset_done", &[]));
                self.relabel(ui, rt);
                self.events.push(ControlsEvent::Changed);
                continue;
            }
            let Some(ix) = self.cells.iter().position(|c| c.button == *id) else {
                continue;
            };
            let c = self.cells[ix].clone();
            let mut req = RebindRequest::new(self.player, c.action, c.slot);
            req.part = c.part;
            req.devices = match c.column {
                Column::KeyboardMouse => {
                    DeviceFilter::Classes(vec![Device::Keyboard, Device::Mouse])
                }
                Column::Gamepad => DeviceFilter::Classes(vec![Device::Gamepad]),
            };
            if c.column == Column::KeyboardMouse {
                req.cancel = vec![ControlRef::new(Device::Keyboard, "Escape")];
            }
            rt.start_rebind(req);
            self.listening = Some(ix);
            let mut name = self.action_name(rt, c.action);
            if let Some(p) = self.part_name(&c, rt.map().kind(c.action)) {
                name = format!("{name} ({p})");
            }
            let cancel = match c.column {
                Column::KeyboardMouse => {
                    self.glyphs
                        .control(&ControlRef::new(Device::Keyboard, "Escape"))
                        .label
                }
                Column::Gamepad => {
                    self.glyphs
                        .control(&ControlRef::new(Device::Gamepad, "Select"))
                        .label
                }
            };
            self.status.set(
                ui.rt_mut(),
                self.loc
                    .get("controls.prompt", &[("action", &name), ("cancel", &cancel)]),
            );
        }
    }

    /// After each [`InputRuntime::update`]: finish a rebinding that completed, relabel, and
    /// save the overrides to `store` (the player's profile) at once.
    pub fn update(
        &mut self,
        ui: &mut Ui,
        rt: &mut InputRuntime,
        store: Option<(&dyn OverrideStore, &str)>,
    ) {
        let Some(ix) = self.listening else { return };
        let Some(c) = self.cells.get(ix).cloned() else {
            return;
        };
        let Some(done) = rt.take_rebind() else { return };
        self.listening = None;
        let name = self.action_name(rt, c.action);
        let msg = match done {
            RebindState::Bound { binding, swapped } => {
                let g = self.glyphs.binding(&binding);
                self.events.push(ControlsEvent::Changed);
                let saved = store.map(|(s, profile)| rt.save_bindings(self.player, s, profile));
                if let Some(Err(e)) = saved {
                    self.loc
                        .get("controls.saved_failed", &[("reason", &e.to_string())])
                } else {
                    match swapped {
                        Some((other, _)) => {
                            let o = self.action_name(rt, other);
                            self.loc.get(
                                "controls.swapped",
                                &[("action", &name), ("binding", &g), ("other", &o)],
                            )
                        }
                        None => self
                            .loc
                            .get("controls.bound", &[("action", &name), ("binding", &g)]),
                    }
                }
            }
            RebindState::Canceled => self.loc.get("controls.canceled", &[]),
            RebindState::TimedOut => self.loc.get("controls.timeout", &[]),
            RebindState::Refused(r) => self.loc.get("controls.refused", &[("reason", &r)]),
            RebindState::Waiting => return,
        };
        self.status.set(ui.rt_mut(), msg);
        self.relabel(ui, rt);
    }
}

// ---- on-screen controls ---------------------------------------------------------------------

/// Draws the on-screen controls and forwards the mouse as a finger (see the module docs).
/// Fingers from the platform go to `OnScreenControls::touch` directly (multi-touch); the
/// view repaints when their state changes.
pub struct OnScreenControlsView {
    controls: Rc<RefCell<OnScreenControls>>,
    rt: Rc<RefCell<InputRuntime>>,
    labels: Vec<String>,
    name: String,
    seen: u64,
    mouse_down: bool,
}

/// The mouse's finger id (platform touch ids are never this).
pub const MOUSE_FINGER: u64 = u64::MAX;

impl OnScreenControlsView {
    /// `loc` names the controls (their `label` keys) for assistive technology.
    pub fn new(
        controls: Rc<RefCell<OnScreenControls>>,
        rt: Rc<RefCell<InputRuntime>>,
        loc: &Localiser,
    ) -> Self {
        let labels = controls
            .borrow()
            .controls()
            .iter()
            .map(|c| loc.get(&c.label, &[]))
            .collect();
        Self {
            controls,
            rt,
            labels,
            name: loc.get("onscreen.controls", &[]),
            seen: 0,
            mouse_down: false,
        }
    }

    fn finger(&mut self, cx: &mut EventCx, phase: forge_input::TouchPhase, pos: Point) -> bool {
        let r = cx.rect();
        let mut os = self.controls.borrow_mut();
        os.set_screen(r.w, r.h);
        let t = forge_input::TouchInput::new(MOUSE_FINGER, phase, pos.x - r.x, pos.y - r.y);
        let took = os.touch(&mut self.rt.borrow_mut(), t);
        if os.revision() != self.seen {
            self.seen = os.revision();
            cx.request_paint();
        }
        took
    }
}

impl Widget for OnScreenControlsView {
    fn role(&self) -> Role {
        Role::Group
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        use forge_input::TouchPhase as P;
        let took = match ev {
            UiEvent::PointerDown { pos, .. } => {
                let t = self.finger(cx, P::Started, *pos);
                self.mouse_down = t;
                t
            }
            UiEvent::PointerMove { pos } if self.mouse_down => self.finger(cx, P::Moved, *pos),
            UiEvent::PointerUp { pos, .. } if self.mouse_down => {
                self.mouse_down = false;
                self.finger(cx, P::Ended, *pos)
            }
            _ => false,
        };
        if took { Handled::Yes } else { Handled::No }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        let mut os = self.controls.borrow_mut();
        os.set_screen(r.w, r.h);
        let style = TextStyle::body(cx.theme().type_scale.small);
        for i in 0..os.controls().len() {
            let st = os.state(i);
            let rad = os.radius(i);
            let c = os.center(i);
            let base = if st.pressed { st.origin } else { c };
            let disc = Rect::new(
                r.x + base[0] - rad,
                r.y + base[1] - rad,
                rad * 2.0,
                rad * 2.0,
            );
            let fill = if st.pressed {
                ColorRole::Accent
            } else {
                ColorRole::BgSunken
            };
            cx.panel(disc, Some(fill), Some(ColorRole::Border), rad, None);
            match &os.controls()[i].kind {
                forge_input::OnScreenKind::Stick { .. } => {
                    let k = rad * 0.45;
                    let kx = r.x + base[0] + st.knob[0] * rad - k;
                    let ky = r.y + base[1] - st.knob[1] * rad - k;
                    let knob = Rect::new(kx, ky, k * 2.0, k * 2.0);
                    cx.panel(
                        knob,
                        Some(ColorRole::BgRaised),
                        Some(ColorRole::Border),
                        k,
                        None,
                    );
                }
                forge_input::OnScreenKind::Button { .. } => {
                    let label = self.labels.get(i).map_or("", String::as_str);
                    let fg = if st.pressed {
                        ColorRole::FgOnAccent
                    } else {
                        ColorRole::FgPrimary
                    };
                    cx.text_on(
                        label,
                        &style,
                        Point::new(disc.x + 6.0, disc.y + rad - 8.0),
                        fg,
                        fill,
                    );
                }
            }
        }
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut accesskit::Node) {
        node.set_label(self.name.clone());
    }
    fn a11y_children(&self, _cx: &A11yCx, out: &mut Vec<(u64, accesskit::Node)>) {
        for (i, l) in self.labels.iter().enumerate() {
            let mut n = accesskit::Node::new(Role::Button);
            n.set_label(l.clone());
            out.push((i as u64, n));
        }
    }
}

// ---- the menus' gamepad source ---------------------------------------------------------------

/// The pad buttons the menus read, with the runtime's control names.
const MENU_BUTTONS: [(PadButton, &str); 12] = [
    (PadButton::South, "South"),
    (PadButton::East, "East"),
    (PadButton::West, "West"),
    (PadButton::North, "North"),
    (PadButton::DPadUp, "DPadUp"),
    (PadButton::DPadDown, "DPadDown"),
    (PadButton::DPadLeft, "DPadLeft"),
    (PadButton::DPadRight, "DPadRight"),
    (PadButton::LeftShoulder, "LeftShoulder"),
    (PadButton::RightShoulder, "RightShoulder"),
    (PadButton::Start, "Start"),
    (PadButton::Select, "Select"),
];

/// The menus' gamepad source over the input runtime (see the module docs): a player's pads,
/// read after each [`InputRuntime::update`].
pub struct InputPadSource {
    rt: Rc<RefCell<InputRuntime>>,
    player: PlayerId,
    down: [bool; 12],
    stick: [f32; 2],
}

impl InputPadSource {
    pub fn new(rt: Rc<RefCell<InputRuntime>>, player: PlayerId) -> Self {
        Self {
            rt,
            player,
            down: [false; 12],
            stick: [0.0; 2],
        }
    }
}

impl GamepadSource for InputPadSource {
    fn describe(&self) -> String {
        let rt = self.rt.borrow();
        let n = rt
            .player_devices(self.player)
            .iter()
            .filter(|d| rt.device(**d).is_some_and(|x| x.class == Device::Gamepad))
            .count();
        format!(
            "forge-input: {n} gamepad(s) for player {}",
            self.player.0 + 1
        )
    }
    fn poll(&mut self) -> Vec<PadInput> {
        let rt = self.rt.borrow();
        let pads: Vec<_> = rt
            .player_devices(self.player)
            .iter()
            .copied()
            .filter(|d| rt.device(*d).is_some_and(|x| x.class == Device::Gamepad))
            .collect();
        let read = |name: &str| -> [f32; 2] {
            let Some(i) = control_index(Device::Gamepad, name) else {
                return [0.0; 2];
            };
            pads.iter()
                .map(|d| rt.control_value(*d, i))
                .max_by(|a, b| (a[0].abs() + a[1].abs()).total_cmp(&(b[0].abs() + b[1].abs())))
                .unwrap_or([0.0; 2])
        };
        let mut out = Vec::new();
        for (k, (b, name)) in MENU_BUTTONS.iter().enumerate() {
            let now = read(name)[0] >= forge_input::runtime::PRESS_POINT;
            if now != self.down[k] {
                self.down[k] = now;
                out.push(PadInput::Button {
                    button: *b,
                    pressed: now,
                });
            }
        }
        let s = read("LeftStick");
        if s != self.stick {
            self.stick = s;
            out.push(PadInput::Stick { x: s[0], y: s[1] });
        }
        out
    }
    fn glyphs(&self) -> GlyphSet {
        let rt = self.rt.borrow();
        let fam = rt
            .player_devices(self.player)
            .iter()
            .filter_map(|d| rt.device(*d))
            .find(|d| d.class == Device::Gamepad)
            .map_or(PadFamily::Generic, |d| d.family);
        match fam {
            PadFamily::Xbox => GlyphSet::Xbox,
            PadFamily::PlayStation => GlyphSet::PlayStation,
            PadFamily::Nintendo => GlyphSet::Nintendo,
            PadFamily::Generic => GlyphSet::Generic,
        }
    }
}

//! The **Input debugger** (`forge.input_debugger`, Ch.28 §28.18, DoD M7-12): what the input
//! runtime sees, against the project's own action map — the devices and every control not at
//! rest, each local player with their devices, contexts and every action's phase and value,
//! and the recent raw events. It reads the input layer's live runtime
//! (`InputActions::debug_snapshot`; `forge-input` behind [`forge_editor::domain::input::DeviceInput`]).
//!
//! * **Opening it reads no device.** The panel shows the runtime as it is; **Refresh** reads
//!   the devices once and runs one update; the **Live** toggle does that 20 times a second
//!   while it is on and the panel is visible, and stops by itself when the panel is hidden.
//!   With Live off the panel schedules nothing (D-5: an idle editor draws nothing).
//! * **Read-only.** It edits nothing (no command): the input map panel authors, this one
//!   observes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use accesskit::{Node, Toggled};
use forge_editor::domain::input::runtime_settings;
use forge_editor::mirror::ProjectMirror;
use forge_editor::panels::PanelCx;
use forge_editor::services::EditorServices;
use forge_input::debug::DebugSnapshot;
use forge_input::runtime::Phase;
use forge_ui::text::TextStyle;
use forge_ui::widget::{A11yCx, EventCx, PaintCx};
use forge_ui::widgets::{Button, Container, Label, LabelKind, Pressed, RowItem, VirtualTree};
use forge_ui::{
    ColorRole, Handled, KeyCode, NodeStyle, Point, Role, Signal, Ui, UiEvent, Widget, WidgetId,
};

use crate::common::{Row, Rows, key_of, show_rows, watch};

/// Refreshes a second while Live is on.
pub const DEBUGGER_HZ: u64 = 20;
const TICK: u64 = 0x1d_ebb9;

/// Live polling fired: refresh the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebugTick;

/// The Live toggle: a switch that, while on and visible, raises [`DebugTick`] every
/// 1/[`DEBUGGER_HZ`] s — and nothing at all while off.
pub struct LiveToggle {
    on: bool,
    /// W2 control (`PanelFaults::input_debugger_keeps_polling`): ticking survives Off.
    keep: bool,
    /// Ticks raised (tests read it).
    pub ticks: u64,
}

impl LiveToggle {
    pub fn new() -> Self {
        Self {
            on: false,
            keep: false,
            ticks: 0,
        }
    }
    /// The W2 control's toggle (its timer survives Off).
    pub fn keeping_its_timer(mut self, keep: bool) -> Self {
        self.keep = keep;
        self
    }
    pub fn is_on(&self) -> bool {
        self.on
    }
    fn every() -> Duration {
        Duration::from_millis(1000 / DEBUGGER_HZ)
    }
    fn flip(&mut self, cx: &mut EventCx) {
        self.on = !self.on;
        if self.on {
            cx.action(DebugTick);
            cx.set_timer(Self::every(), TICK);
        } else if !self.keep {
            cx.cancel_timer(TICK);
        }
        cx.request_paint();
        cx.request_a11y();
    }
    fn label(&self) -> &'static str {
        if self.on {
            forge_ui::tr!("Live: on")
        } else {
            forge_ui::tr!("Live: off")
        }
    }
}

impl Default for LiveToggle {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for LiveToggle {
    fn role(&self) -> Role {
        Role::Switch
    }
    fn focusable(&self) -> bool {
        true
    }
    fn measure(
        &mut self,
        _cx: &mut forge_ui::widget::MeasureCx,
        _known: taffy::Size<Option<f32>>,
        _avail: taffy::Size<taffy::AvailableSpace>,
    ) -> forge_ui::Size {
        forge_ui::Size::new(96.0, 28.0)
    }
    fn measured(&self) -> bool {
        true
    }
    fn event(&mut self, cx: &mut EventCx, ev: &UiEvent) -> Handled {
        match ev {
            UiEvent::PointerDown { .. } => {
                self.flip(cx);
                Handled::Yes
            }
            UiEvent::Key(k) if k.pressed && matches!(k.code, KeyCode::Enter | KeyCode::Space) => {
                self.flip(cx);
                Handled::Yes
            }
            UiEvent::Timer(TICK) if self.on || self.keep => {
                if cx.is_visible() || self.keep {
                    self.ticks += 1;
                    cx.action(DebugTick);
                    cx.set_timer(Self::every(), TICK);
                } else {
                    // Hidden: stop, so a debugger left in a background tab costs nothing.
                    self.on = false;
                    cx.request_paint();
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
    fn paint(&self, cx: &mut PaintCx) {
        let r = cx.rect();
        // Painted like the catalogue Switch: an accent fill when on, a bordered sunken fill
        // when off (the border carries the boundary contrast).
        let (bg, fg) = if self.on {
            cx.mark(r, ColorRole::Accent, cx.parent_bg(), 4.0);
            (ColorRole::Accent, ColorRole::FgOnAccent)
        } else {
            cx.panel(
                r,
                Some(ColorRole::BgSunken),
                Some(ColorRole::Border),
                4.0,
                None,
            );
            (ColorRole::BgSunken, ColorRole::FgPrimary)
        };
        let style = TextStyle::body(cx.theme().type_scale.body);
        cx.text_on(
            self.label(),
            &style,
            Point::new(r.x + 8.0, r.y + 5.0),
            fg,
            bg,
        );
    }
    fn a11y(&self, _cx: &A11yCx, node: &mut Node) {
        node.set_label(forge_ui::tr!("Live"));
        node.set_toggled(if self.on {
            Toggled::True
        } else {
            Toggled::False
        });
        node.add_action(accesskit::Action::Click);
    }
}

fn phase_name(p: Phase) -> &'static str {
    match p {
        Phase::Idle => forge_ui::tr!("Idle"),
        Phase::Ongoing => forge_ui::tr!("Ongoing"),
        Phase::Triggered => forge_ui::tr!("Triggered"),
    }
}

fn value_text(v: [f32; 2]) -> String {
    if v[1] == 0.0 {
        format!("{:.2}", v[0])
    } else {
        format!("({:.2}, {:.2})", v[0], v[1])
    }
}

/// The rows of a snapshot: devices, players/contexts/actions, events.
pub fn snapshot_rows(s: &DebugSnapshot) -> (Vec<Row>, Vec<Row>, Vec<Row>) {
    let mut devs = Vec::new();
    for d in &s.devices {
        let k = key_of(&["dev", &d.id.0.to_string()]);
        let class = forge_ui::l10n::tr_str(d.class.name()).into_owned();
        let owner = match d.player {
            Some(p) => forge_ui::trf!("player {n}", n = p.0 + 1),
            None => forge_ui::tr!("no player").to_string(),
        };
        let state = if d.connected {
            owner
        } else {
            forge_ui::tr!("disconnected").to_string()
        };
        let mut item = RowItem::new(forge_ui::trf!(
            "{class}: {name} ({state})",
            class,
            name = d.name,
            state
        ));
        if !d.connected {
            item = item.muted(true);
        }
        devs.push((None, k, item));
        for (c, v) in &d.active {
            let kk = key_of(&["dev", &d.id.0.to_string(), c]);
            devs.push((Some(k), kk, RowItem::new(format!("{c} {}", value_text(*v)))));
        }
    }
    let mut acts = Vec::new();
    for p in &s.players {
        let pk = key_of(&["player", &p.id.0.to_string()]);
        acts.push((
            None,
            pk,
            RowItem::new(forge_ui::trf!(
                "Player {n}: {devices} device(s), {overrides} rebinding(s)",
                n = p.id.0 + 1,
                devices = p.devices.len(),
                overrides = p.overrides
            )),
        ));
        for c in &p.contexts {
            let ck = key_of(&["player", &p.id.0.to_string(), &c.id]);
            let mut item = RowItem::new(forge_ui::trf!(
                "{name} (priority {priority})",
                name = c.name,
                priority = c.priority
            ));
            if !c.enabled {
                item = item.muted(true);
            }
            acts.push((Some(pk), ck, item));
            for a in &c.actions {
                let ak = key_of(&["player", &p.id.0.to_string(), &a.path]);
                let mut item = RowItem::new(forge_ui::trf!(
                    "{name}: {phase} {value}",
                    name = a.name,
                    phase = phase_name(a.state.phase),
                    value = value_text(a.state.value)
                ));
                if a.state.phase == Phase::Idle {
                    item = item.muted(true);
                }
                acts.push((Some(ck), ak, item));
            }
        }
    }
    let evs = s
        .events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            (
                None,
                key_of(&["ev", &e.frame.to_string(), &i.to_string()]),
                RowItem::new(forge_ui::trf!(
                    "frame {frame}: {control} = {value}",
                    frame = e.frame,
                    control = e.control,
                    value = value_text(e.value)
                )),
            )
        })
        .collect();
    (devs, acts, evs)
}

struct Dbg {
    devices: WidgetId,
    actions: WidgetId,
    events: WidgetId,
    dev_rows: Rows<()>,
    act_rows: Rows<()>,
    ev_rows: Rows<()>,
    status: Signal<String>,
    problems: Signal<String>,
    seen: u64,
    /// Snapshots taken (tests read it through the status line's frame count).
    refreshes: u64,
}

impl Dbg {
    fn refresh(&mut self, ui: &mut Ui, sv: &EditorServices, m: &ProjectMirror, poll: bool) {
        self.refreshes += 1;
        let settings = runtime_settings(m);
        let Some(s) = sv.input.debug_snapshot(&settings, poll) else {
            self.status.set(
                ui.rt_mut(),
                forge_ui::trf!(
                    "No input runtime: {note}",
                    note = forge_ui::l10n::tr_str(&sv.input.backend().note)
                ),
            );
            return;
        };
        let actions: usize = s
            .players
            .first()
            .map_or(0, |p| p.contexts.iter().map(|c| c.actions.len()).sum());
        let status = if s.devices.is_empty() && !s.has_map {
            forge_ui::tr!(
                "No devices and no action map yet: press Refresh or turn Live on, and author actions in the Input map."
            )
            .to_string()
        } else {
            forge_ui::trf!(
                "Frame {frame}: {devices} device(s), {players} player(s), {actions} action(s)",
                frame = s.frame,
                devices = s.devices.len(),
                players = s.players.len(),
                actions
            )
        };
        self.status.set(ui.rt_mut(), status);
        let problems = if s.problems.is_empty() {
            String::new()
        } else {
            forge_ui::trf!(
                "\u{26a0} {n} map problem(s): {why}",
                n = s.problems.len(),
                why = s.problems.join("; ")
            )
        };
        self.problems.set(ui.rt_mut(), problems);
        let (d, a, e) = snapshot_rows(&s);
        let unit =
            |rows: &Vec<Row>| -> HashMap<u64, ()> { rows.iter().map(|r| (r.1, ())).collect() };
        let (md, ma, me) = (unit(&d), unit(&a), unit(&e));
        show_rows(ui, self.devices, d, md, &mut self.dev_rows);
        show_rows(ui, self.actions, a, ma, &mut self.act_rows);
        show_rows(ui, self.events, e, me, &mut self.ev_rows);
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!(
        "the input runtime's status line and its Refresh and Live controls"
    ));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space[1];
        pb.want_turn();
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space).padding(space).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Input debugger actions")),
        )?;
        let refresh = pb.b.add(
            bar,
            "refresh",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Refresh")),
        )?;
        let keep = pb.services().faults.input_debugger_keeps_polling();
        let live = pb.b.add(
            bar,
            "live",
            NodeStyle::leaf(),
            LiveToggle::new().keeping_its_timer(keep),
        )?;
        let status = pb.b.signal(String::new());
        pb.b.add(
            pb.parent,
            "status",
            NodeStyle::leaf().padding(space),
            Label::new(status).wrapping(),
        )?;
        let problems = pb.b.signal(String::new());
        pb.b.add(
            pb.parent,
            "problems",
            NodeStyle::leaf().padding(space),
            Label::new(problems).kind(LabelKind::Warning).wrapping(),
        )?;
        let body = pb.b.add(
            pb.parent,
            "body",
            NodeStyle::row(space).grow(1.0).padding(space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Input runtime")),
        )?;
        let devices = pb.b.add(
            body,
            "devices",
            NodeStyle::leaf().width(240.0).min_size(0.0, 80.0),
            VirtualTree::tree(forge_ui::tr!("Devices and active controls"))
                .read_only()
                .single_select(),
        )?;
        let actions = pb.b.add(
            body,
            "actions",
            NodeStyle::leaf().grow(1.0).min_size(0.0, 80.0),
            VirtualTree::tree(forge_ui::tr!("Players, contexts and actions"))
                .read_only()
                .single_select(),
        )?;
        let events = pb.b.add(
            body,
            "events",
            NodeStyle::leaf().width(240.0).min_size(0.0, 80.0),
            VirtualTree::list(forge_ui::tr!("Recent raw events"))
                .read_only()
                .single_select(),
        )?;
        let st = Rc::new(RefCell::new(Dbg {
            devices,
            actions,
            events,
            dev_rows: Rows::default(),
            act_rows: Rows::default(),
            ev_rows: Rows::default(),
            status,
            problems,
            seen: u64::MAX,
            refreshes: 0,
        }));
        let s = st.clone();
        pb.on(refresh, move |act, _: &Pressed| {
            s.borrow_mut()
                .refresh(act.ui, act.services, act.mirror, true);
        });
        let s = st.clone();
        pb.on(live, move |act, _: &DebugTick| {
            s.borrow_mut()
                .refresh(act.ui, act.services, act.mirror, true);
        });
        let s = st;
        pb.sync(devices, move |sy| {
            // The map changed (or the panel opened): show it, reading no device.
            let rev = watch(sy.mirror, "input.");
            let mut d = s.borrow_mut();
            if rev != d.seen {
                d.seen = rev;
                d.refresh(sy.ui, sy.services, sy.mirror, false);
            }
            Ok(())
        });
        Ok(())
    });
}

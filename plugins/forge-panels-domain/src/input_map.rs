//! The **Input action map** (`forge.input_map`, Ch.21 §21.21, DoD M2-68; Ch.28): the
//! game's action maps, actions and default bindings over `forge_editor::domain::input` —
//! **distinct from the editor keymap** (the keybindings editor edits user config; this
//! edits project state the shipped game reads).
//!
//! * Maps (contexts) and actions (button, 1D axis, 2D axis): add, rename in place, remove.
//! * Bindings: **press to bind** (the next key, mouse button or gamepad control), a
//!   composite from two or four presses (A/D for a 1D axis, W/S/A/D for a 2D one), or pick
//!   any control from the input layer's list. A binding that does not fit its action, or a
//!   control another action of the map already uses, is refused with the reason — a
//!   conflict names both actions. Conflicts written by another issuer are listed.
//!
//! Every project edit is a command (I7).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use forge_cmd::Value;
use forge_editor::domain::input::{
    self as inp, ActionKind, Binding, Control, ControlRef, InputDoc, MAP, action_key, map_key,
};
use forge_editor::domain::{clear, clear_under, set};
use forge_editor::mirror::ProjectMirror;
use forge_editor::panel_rt::PanelAct;
use forge_editor::panels::PanelCx;
use forge_editor::services::EditorServices;
use forge_ui::widgets::{
    Button, Container, Label, LabelKind, Pressed, RadioGroup, RowActivated, RowItem, RowRenamed,
    RowsDeleteRequested, SelectionChanged, TextField, VirtualTree,
};
use forge_ui::{NodeStyle, Role, Signal, Ui, WidgetId};

use crate::common::{Row, Rows, key_of, refuse, select, selected, show_rows, watch};
use crate::widgets::{BindCapture, BindCaptured};

const PREFIX: &str = "input.";

#[derive(Clone, Debug, PartialEq)]
enum ActRow {
    Action(String),
    Binding(String, u32),
}

struct Im {
    doc: InputDoc,
    seen: u64,
    status: Signal<String>,
    problems: Signal<String>,
    conflicts: Signal<String>,
    maps: WidgetId,
    map_rows: Rows<String>,
    actions: WidgetId,
    action_rows: Rows<ActRow>,
    control_rows: Rows<Control>,
    capture: WidgetId,
    prompt: Signal<String>,
    sel_map: Option<String>,
    /// The action a capture is for.
    capturing: Option<String>,
}

impl Im {
    fn say(&self, ui: &mut Ui, s: impl Into<String>) {
        self.status.set(ui.rt_mut(), s.into());
    }

    fn reread(&mut self, ui: &mut Ui, m: &ProjectMirror) {
        self.doc = InputDoc::read(m);
        let p = if self.doc.problems.is_empty() {
            String::new()
        } else {
            forge_ui::trf!(
                "\u{26a0} {n} value(s) skipped: {why}",
                n = self.doc.problems.len(),
                why = self.doc.problems.join("; ")
            )
        };
        self.problems.set(ui.rt_mut(), p);
        if self
            .sel_map
            .as_ref()
            .is_none_or(|s| !self.doc.maps.contains_key(s))
        {
            self.sel_map = self.doc.maps.keys().next().cloned();
        }
        self.show(ui);
    }

    fn show(&mut self, ui: &mut Ui) {
        let rows: Vec<Row> = self
            .doc
            .maps
            .values()
            .map(|mp| {
                (
                    None,
                    key_of(&["map", &mp.id]),
                    RowItem::new(forge_ui::trf!(
                        "{name} ({actions_count} actions)",
                        name = mp.name,
                        actions_count = mp.actions.len()
                    )),
                )
            })
            .collect();
        let map = self
            .doc
            .maps
            .keys()
            .map(|id| (key_of(&["map", id]), id.clone()))
            .collect();
        show_rows(ui, self.maps, rows, map, &mut self.map_rows);
        if let Some(s) = &self.sel_map {
            select(ui, self.maps, key_of(&["map", s]));
        }
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        let mut conflicts = String::new();
        if let Some(mp) = self.sel_map.as_ref().and_then(|s| self.doc.maps.get(s)) {
            for a in mp.actions.values() {
                let k = key_of(&["act", &mp.id, &a.id]);
                rows.push((None, k, RowItem::new(action_label(a))));
                map.insert(k, ActRow::Action(a.id.clone()));
                for (slot, b) in &a.bindings {
                    let kk = key_of(&["act", &mp.id, &a.id, &slot.to_string()]);
                    rows.push((
                        Some(k),
                        kk,
                        RowItem::new(inp::binding_label(b, a.bindmods.get(slot))),
                    ));
                    map.insert(kk, ActRow::Binding(a.id.clone(), *slot));
                }
            }
            let c = mp.conflicts();
            if !c.is_empty() {
                conflicts = forge_ui::trf!(
                    "\u{26a0} Conflicts: {conflicts}",
                    conflicts = c
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            }
        }
        show_rows(ui, self.actions, rows, map, &mut self.action_rows);
        self.conflicts.set(ui.rt_mut(), conflicts);
    }

    /// The action the selection is on (an action row or one of its bindings).
    fn sel_action(&self, ui: &mut Ui) -> Option<String> {
        match selected(ui, self.actions).and_then(|k| self.action_rows.get(k))? {
            ActRow::Action(a) | ActRow::Binding(a, _) => Some(a),
        }
    }

    fn add(&self, act: &mut PanelAct, action: &str, binding: Binding) {
        let Some(mp) = self.sel_map.as_ref().and_then(|s| self.doc.maps.get(s)) else {
            return;
        };
        match inp::add_binding(mp, action, &binding, &*act.services.input) {
            Ok(cmd) => {
                act.cmd.emit(cmd);
                self.say(act.ui, forge_ui::trf!("Bound {binding}.", binding));
            }
            Err(why) => {
                refuse(act.session, forge_ui::tr!("Binding refused"), &why);
                self.say(act.ui, format!("\u{26a0} {why}"));
            }
        }
    }
}

/// An action row: its name and kind, then its triggers and modifiers when it has any.
fn action_label(a: &inp::Action) -> String {
    let base = forge_ui::trf!(
        "{name} ({kind})",
        name = a.name,
        kind = forge_ui::l10n::tr(a.kind.name())
    );
    let mut rules: Vec<String> = Vec::new();
    if !a.triggers.is_empty() {
        rules.push(inp::Trigger::chain_text(&a.triggers));
    }
    if !a.modifiers.is_empty() {
        rules.push(inp::Modifier::chain_text(&a.modifiers));
    }
    if rules.is_empty() {
        base
    } else {
        format!("{base} \u{2014} {}", rules.join(" \u{b7} "))
    }
}

fn controls_rows(sv: &EditorServices) -> (Vec<Row>, HashMap<u64, Control>) {
    let mut rows: Vec<Row> = Vec::new();
    let mut map = HashMap::new();
    for d in sv.input.devices() {
        let k = key_of(&["dev", d.name()]);
        rows.push((None, k, RowItem::new(forge_ui::l10n::tr(d.name()))));
        for c in sv.input.controls(d) {
            let kk = key_of(&["dev", d.name(), &c.name]);
            rows.push((
                Some(k),
                kk,
                RowItem::new(format!("{} ({:?})", c.name, c.shape)),
            ));
            map.insert(kk, c);
        }
    }
    (rows, map)
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("the standard input actions"));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space[1];
        // The sync step fills the lists from the mirror: run it right after the build.
        pb.want_turn();
        let sv = pb.services();
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space).padding(space).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Input map actions")),
        )?;
        let name = pb.b.signal(String::new());
        pb.b.add(
            bar,
            "name",
            NodeStyle::leaf().width(180.0),
            TextField::new(name, forge_ui::tr!("New name")).placeholder(forge_ui::tr!("Jump")),
        )?;
        let new_map = pb.b.add(
            bar,
            "new_map",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New map")),
        )?;
        let kind = pb.b.signal(0usize);
        let kinds: Vec<&str> = ActionKind::ALL
            .iter()
            .map(|k| forge_ui::l10n::tr(k.name()))
            .collect();
        pb.b.add(
            bar,
            "kind",
            NodeStyle::leaf(),
            RadioGroup::new(forge_ui::tr!("Action kind"), &kinds, kind),
        )?;
        let new_action = pb.b.add(
            bar,
            "new_action",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("New action")),
        )?;
        let press = pb.b.add(
            bar,
            "press",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Press to bind\u{2026}")),
        )?;
        let composite = pb.b.add(
            bar,
            "composite",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Press a composite\u{2026}")),
        )?;
        let remove = pb.b.add(
            bar,
            "remove",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Remove")),
        )?;
        // Triggers and modifiers: one chain of text, parsed by the runtime's own parser.
        let rules_bar = pb.b.add(
            pb.parent,
            "rules",
            NodeStyle::row(space).padding(space).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Triggers and modifiers")),
        )?;
        let rules = pb.b.signal(String::new());
        pb.b.add(
            rules_bar,
            "text",
            NodeStyle::leaf().width(320.0),
            TextField::new(rules, forge_ui::tr!("Triggers or modifiers"))
                .placeholder(forge_ui::tr!("Hold(0.5) | Chord(gameplay/aim)")),
        )?;
        let set_triggers = pb.b.add(
            rules_bar,
            "set_triggers",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Set triggers")),
        )?;
        let set_modifiers = pb.b.add(
            rules_bar,
            "set_modifiers",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Set modifiers")),
        )?;
        let prompt =
            pb.b.signal(forge_ui::tr!("Press to bind: select an action first.").to_string());
        let capture = pb.b.add(
            pb.parent,
            "capture",
            NodeStyle::leaf().padding(space).min_size(0.0, 30.0),
            BindCapture::new(sv.input.clone(), prompt),
        )?;
        pb.b.hide(capture, true);
        let body = pb.b.add(
            pb.parent,
            "body",
            NodeStyle::row(space).grow(1.0).padding(space),
            Container::new(Role::Group).labelled(forge_ui::tr!("Action map")),
        )?;
        let maps = pb.b.add(
            body,
            "maps",
            NodeStyle::leaf().width(180.0).min_size(0.0, 80.0),
            VirtualTree::list(forge_ui::tr!("Action maps")).single_select(),
        )?;
        let actions = pb.b.add(
            body,
            "actions",
            NodeStyle::leaf().grow(1.0).min_size(0.0, 80.0),
            VirtualTree::tree(forge_ui::tr!("Actions and default bindings")).single_select(),
        )?;
        let mut ctl = VirtualTree::tree(forge_ui::tr!("Controls (Enter binds)"))
            .read_only()
            .single_select();
        let (crows, cmap) = controls_rows(&sv);
        for (p, k, item) in crows {
            ctl.push(p, k, item);
        }
        let controls = pb.b.add(
            body,
            "controls",
            NodeStyle::leaf().width(220.0).min_size(0.0, 80.0),
            ctl,
        )?;
        let conflicts = pb.b.signal(String::new());
        pb.b.add(
            pb.parent,
            "conflicts",
            NodeStyle::leaf().padding(space),
            Label::new(conflicts).kind(LabelKind::Warning).wrapping(),
        )?;
        let status = pb.b.signal(
            forge_ui::tr!("Game input actions and their default bindings (not the editor keymap).")
                .to_string(),
        );
        pb.b.add(
            pb.parent,
            "status",
            NodeStyle::leaf().padding(space),
            Label::new(status).kind(LabelKind::Warning).wrapping(),
        )?;
        let problems = pb.b.signal(String::new());
        pb.b.add(
            pb.parent,
            "problems",
            NodeStyle::leaf().padding(space),
            Label::new(problems).kind(LabelKind::Warning).wrapping(),
        )?;
        pb.b.add(
            pb.parent,
            "backend",
            NodeStyle::leaf().padding(space),
            Label::new(forge_ui::l10n::tr_str(&sv.input.backend().note).into_owned())
                .kind(LabelKind::Small)
                .wrapping(),
        )?;
        let mut control_rows = Rows::default();
        control_rows.map = cmap;

        let st = Rc::new(RefCell::new(Im {
            doc: InputDoc::default(),
            seen: u64::MAX,
            status,
            problems,
            conflicts,
            maps,
            map_rows: Rows::default(),
            actions,
            action_rows: Rows::default(),
            control_rows,
            capture,
            prompt,
            sel_map: None,
            capturing: None,
        }));

        let s = st.clone();
        pb.on(new_map, move |act, _: &Pressed| {
            let im = s.borrow();
            let n = name.get(act.ui.rt());
            let n = if n.trim().is_empty() {
                forge_ui::tr!("Gameplay").to_string()
            } else {
                n.trim().to_string()
            };
            let (id, cmds) = inp::new_map(&im.doc, &n);
            act.cmd.emit_all(forge_ui::tr!("New action map"), cmds);
            im.say(act.ui, forge_ui::trf!("Created action map {id}.", id));
        });
        let s = st.clone();
        pb.on(new_action, move |act, _: &Pressed| {
            let im = s.borrow();
            let Some(mp) = im.sel_map.as_ref().and_then(|m| im.doc.maps.get(m)) else {
                refuse(
                    act.session,
                    forge_ui::tr!("New action"),
                    forge_ui::tr!("Create or select an action map first."),
                );
                return;
            };
            let n = name.get(act.ui.rt());
            let n = if n.trim().is_empty() {
                forge_ui::tr!("Action").to_string()
            } else {
                n.trim().to_string()
            };
            let k = ActionKind::ALL
                .get(kind.get(act.ui.rt()))
                .copied()
                .unwrap_or(ActionKind::Button);
            let (id, cmds) = inp::new_action(mp, &n, k);
            act.cmd.emit_all(forge_ui::tr!("New input action"), cmds);
            im.say(
                act.ui,
                forge_ui::trf!("Created {k} action {id}.", k = k.name(), id),
            );
        });
        let start = {
            let s = st.clone();
            Rc::new(move |act: &mut PanelAct, composite: bool| {
                let mut im = s.borrow_mut();
                let Some(a) = im.sel_action(act.ui) else {
                    refuse(
                        act.session,
                        forge_ui::tr!("Press to bind"),
                        forge_ui::tr!("Select an action first."),
                    );
                    return;
                };
                let Some(action) = im
                    .sel_map
                    .as_ref()
                    .and_then(|m| im.doc.maps.get(m))
                    .and_then(|m| m.actions.get(&a))
                    .cloned()
                else {
                    return;
                };
                let need = match (composite, action.kind) {
                    (false, _) => 1,
                    (true, ActionKind::Axis1D) => 2,
                    (true, ActionKind::Axis2D) => 4,
                    (true, ActionKind::Button) => {
                        refuse(
                            act.session,
                            forge_ui::tr!("Press a composite"),
                            forge_ui::tr!(
                                "A button action takes one control; composites drive axes."
                            ),
                        );
                        return;
                    }
                };
                let what = match need {
                    2 => forge_ui::tr!("negative, then positive"),
                    4 => forge_ui::tr!("up, down, left, right"),
                    _ => forge_ui::tr!("one control"),
                };
                im.prompt.set(
                    act.ui.rt_mut(),
                    forge_ui::trf!(
                        "Press {what} for \u{201c}{name}\u{201d} (Esc cancels)",
                        what,
                        name = action.name
                    ),
                );
                im.capturing = Some(a);
                let cap = im.capture;
                if let Some(w) = act.ui.widget_mut::<BindCapture>(cap) {
                    w.start(need);
                }
                let _ = act.ui.set_hidden(cap, false);
                act.ui.set_focus(Some(cap), true);
            })
        };
        let st2 = start.clone();
        pb.on(press, move |act, _: &Pressed| st2(act, false));
        let st2 = start;
        pb.on(composite, move |act, _: &Pressed| st2(act, true));
        // Triggers go on the selected action; modifiers on the selected binding, or on the
        // action when an action row is selected. Empty text clears them.
        let s = st.clone();
        pb.on(set_triggers, move |act, _: &Pressed| {
            let im = s.borrow();
            let (Some(mp), Some(a)) = (
                im.sel_map.as_ref().and_then(|m| im.doc.maps.get(m)),
                im.sel_action(act.ui),
            ) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Set triggers"),
                    forge_ui::tr!("Select an action first."),
                );
                return;
            };
            match inp::set_triggers(&im.doc, mp, &a, &rules.get(act.ui.rt())) {
                Ok(cmd) => {
                    act.cmd.emit(cmd);
                    im.say(act.ui, forge_ui::tr!("Triggers set."));
                }
                Err(why) => {
                    refuse(act.session, forge_ui::tr!("Triggers refused"), &why);
                    im.say(act.ui, format!("\u{26a0} {why}"));
                }
            }
        });
        let s = st.clone();
        pb.on(set_modifiers, move |act, _: &Pressed| {
            let im = s.borrow();
            let Some(mp) = im.sel_map.as_ref().and_then(|m| im.doc.maps.get(m)) else {
                return;
            };
            let text = rules.get(act.ui.rt());
            let res = match selected(act.ui, im.actions).and_then(|k| im.action_rows.get(k)) {
                Some(ActRow::Binding(a, slot)) => inp::set_binding_modifiers(mp, &a, slot, &text),
                Some(ActRow::Action(a)) => inp::set_modifiers(mp, &a, &text),
                None => Err(forge_ui::tr!("Select an action or a binding first.").to_string()),
            };
            match res {
                Ok(cmd) => {
                    act.cmd.emit(cmd);
                    im.say(act.ui, forge_ui::tr!("Modifiers set."));
                }
                Err(why) => {
                    refuse(act.session, forge_ui::tr!("Modifiers refused"), &why);
                    im.say(act.ui, format!("\u{26a0} {why}"));
                }
            }
        });
        let s = st.clone();
        pb.on(capture, move |act, e: &BindCaptured| {
            let mut im = s.borrow_mut();
            let cap = im.capture;
            let _ = act.ui.set_hidden(cap, true);
            act.ui.set_focus(Some(im.actions), true);
            let Some(a) = im.capturing.take() else { return };
            let Some(cs) = e.controls.clone() else {
                im.say(act.ui, forge_ui::tr!("Unchanged."));
                return;
            };
            let refs: Vec<ControlRef> = cs
                .iter()
                .map(|c| ControlRef {
                    device: c.device,
                    name: c.name.clone(),
                })
                .collect();
            let binding = match <[ControlRef; 4]>::try_from(refs.clone()) {
                Ok(four) => Binding::Composite2D(four),
                Err(v) => match <[ControlRef; 2]>::try_from(v) {
                    Ok([n, p]) => Binding::Composite1D(n, p),
                    Err(v) => match v.into_iter().next() {
                        Some(c) => Binding::Control(c),
                        None => return,
                    },
                },
            };
            drop(im);
            s.borrow().add(act, &a, binding);
        });
        let s = st.clone();
        pb.on(controls, move |act, e: &RowActivated| {
            let im = s.borrow();
            let Some(c) = im.control_rows.get(e.key) else {
                return;
            };
            let Some(a) = im.sel_action(act.ui) else {
                refuse(
                    act.session,
                    forge_ui::tr!("Bind control"),
                    forge_ui::tr!("Select an action first."),
                );
                return;
            };
            im.add(
                act,
                &a,
                Binding::Control(ControlRef {
                    device: c.device,
                    name: c.name,
                }),
            );
        });
        let s = st.clone();
        let remove_sel = Rc::new(move |act: &mut PanelAct| {
            let im = s.borrow();
            let Some(mp) = im.sel_map.clone() else { return };
            match selected(act.ui, im.actions).and_then(|k| im.action_rows.get(k)) {
                Some(ActRow::Binding(a, slot)) => {
                    act.cmd.emit_all(
                        forge_ui::tr!("Remove binding"),
                        vec![clear(action_key(&mp, &a, &format!("bind.{slot}")))],
                    );
                }
                Some(ActRow::Action(a)) => {
                    act.cmd.emit_all(
                        forge_ui::tr!("Remove input action"),
                        clear_under(act.mirror, &format!("{MAP}.{mp}.action.{a}")),
                    );
                }
                None => {
                    act.cmd.emit_all(
                        forge_ui::tr!("Remove action map"),
                        clear_under(act.mirror, &format!("{MAP}.{mp}")),
                    );
                }
            }
        });
        let r = remove_sel.clone();
        pb.on(remove, move |act, _: &Pressed| r(act));
        let r = remove_sel;
        pb.on(actions, move |act, _: &RowsDeleteRequested| r(act));
        let s = st.clone();
        pb.on(maps, move |act, e: &SelectionChanged| {
            let mut im = s.borrow_mut();
            if let Some(m) = e.keys.first().and_then(|k| im.map_rows.get(*k))
                && im.sel_map.as_deref() != Some(m.as_str())
            {
                im.sel_map = Some(m);
                im.show(act.ui);
            }
        });
        let s = st.clone();
        pb.on(maps, move |act, e: &RowRenamed| {
            if let Some(m) = s.borrow().map_rows.get(e.key) {
                act.cmd
                    .emit(set(map_key(&m, "name"), Value::Text(e.name.clone())));
            }
        });
        let s = st.clone();
        pb.on(actions, move |act, e: &RowRenamed| {
            let im = s.borrow();
            let (Some(mp), Some(ActRow::Action(a))) =
                (im.sel_map.clone(), im.action_rows.get(e.key))
            else {
                refuse(
                    act.session,
                    forge_ui::tr!("Rename"),
                    forge_ui::tr!("Only an action is renamed; a binding is re-bound."),
                );
                return;
            };
            act.cmd.emit(set(
                action_key(&mp, &a, "name"),
                Value::Text(e.name.clone()),
            ));
        });

        let s = st;
        pb.sync(maps, move |sy| {
            let rev = watch(sy.mirror, PREFIX);
            let mut im = s.borrow_mut();
            if rev != im.seen {
                im.seen = rev;
                im.reread(sy.ui, sy.mirror);
            }
            Ok(())
        });
        Ok(())
    });
}

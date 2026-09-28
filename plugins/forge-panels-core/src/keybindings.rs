//! The **Keybindings** editor (`forge.keybindings`, Ch.21 §21.21, DoD M2-44): every
//! action with its chords, searchable; "Change binding…" captures the new chord by
//! pressing it; a chord already used in an overlapping context is refused with both
//! actions named (the `KeyMap` conflict rule, `test_keybinding_conflicts`); reset per
//! binding or all. The keymap is user config: nothing here is a command.

use std::cell::RefCell;
use std::rc::Rc;

use forge_editor::keybind::{KeyRow, rows};
use forge_editor::keymap::KeyContext;
use forge_editor::panels::PanelCx;
use forge_editor::session::SessionState;
use forge_ui::widgets::{
    Button, Container, Label, LabelKind, Pressed, SearchChanged, SearchField, VirtualTree,
};
use forge_ui::{NodeStyle, Role};

use crate::capture::{ChordCapture, ChordCaptured};
use crate::rows::sync_rows;

fn list_rows(s: &SessionState, query: &str) -> (Vec<KeyRow>, Vec<(u64, String)>) {
    let km = s.keymap().borrow();
    let r = rows(s.actions(), &km, s.base_keymap(), query);
    let keyed = r
        .iter()
        .map(|row| {
            let key = s
                .actions()
                .iter()
                .position(|a| a.id == row.action)
                .unwrap_or(0) as u64;
            (
                key,
                format!(
                    "{} \u{2014} {}{}   ({})",
                    forge_ui::l10n::tr_str(&row.title),
                    row.chord_text(),
                    if row.modified {
                        format!("  \u{2022} {}", forge_ui::tr!("changed"))
                    } else {
                        String::new()
                    },
                    forge_ui::l10n::tr_str(&row.category)
                ),
            )
        })
        .collect();
    (r, keyed)
}

struct State {
    query: String,
    /// The action being rebound.
    capturing: Option<String>,
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!("every action and its keys"));
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let query = pb.b.signal(String::new());
        let search = pb.b.add(
            pb.parent,
            "search",
            NodeStyle::leaf().padding(space[1]),
            SearchField::new(query, forge_ui::tr!("Search keybindings")),
        )?;
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space[1]).padding(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Keybinding actions")),
        )?;
        let change = pb.b.add(
            bar,
            "change",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Change binding\u{2026}")),
        )?;
        let reset = pb.b.add(
            bar,
            "reset",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Reset")),
        )?;
        let reset_all = pb.b.add(
            bar,
            "reset_all",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Reset all")),
        )?;
        let prompt = pb.b.signal(String::new());
        let capture = pb.b.add(
            pb.parent,
            "capture",
            NodeStyle::leaf().padding(space[1]),
            ChordCapture::new(prompt),
        )?;
        pb.b.hide(capture, true);
        let status = pb.b.signal(
            forge_ui::tr!("Select an action, then Change binding\u{2026} and press the new keys.")
                .to_string(),
        );
        pb.b.add(
            pb.parent,
            "status",
            NodeStyle::leaf().padding(space[1]),
            Label::new(status).kind(LabelKind::Warning).wrapping(),
        )?;
        let (_, initial) = list_rows(&pb.session(), "");
        let mut t = VirtualTree::list(forge_ui::tr!("Keybindings")).single_select();
        sync_rows(&mut t, &initial);
        let list =
            pb.b.add(pb.parent, "list", NodeStyle::leaf().grow(1.0), t)?;
        let state = Rc::new(RefCell::new(State {
            query: String::new(),
            capturing: None,
        }));

        let selected = move |ui: &mut forge_ui::Ui, s: &SessionState| -> Option<(String, String)> {
            let key = VirtualTree::edit(ui, list, |t| {
                t.active().or_else(|| t.selected().first().copied())
            })??;
            s.actions()
                .get(key as usize)
                .map(|a| (a.id.clone(), a.title.clone()))
        };

        let st = state.clone();
        pb.on(search, move |act, e: &SearchChanged| {
            st.borrow_mut().query = e.query.clone();
            let (_, r) = list_rows(act.session, &e.query);
            VirtualTree::edit(act.ui, list, |t| sync_rows(t, &r));
        });
        let st = state.clone();
        pb.on(change, move |act, _: &Pressed| {
            let Some((id, title)) = selected(act.ui, act.session) else {
                status.set(
                    act.ui.rt_mut(),
                    forge_ui::tr!("Select an action in the list first.").into(),
                );
                return;
            };
            st.borrow_mut().capturing = Some(id);
            prompt.set(
                act.ui.rt_mut(),
                forge_ui::trf!(
                    "Press the new keys for \u{201c}{title}\u{201d} (Esc cancels)",
                    title
                ),
            );
            let _ = act.ui.set_hidden(capture, false);
            act.ui.set_focus(Some(capture), true);
        });
        let st = state.clone();
        pb.on(capture, move |act, e: &ChordCaptured| {
            let action = st.borrow_mut().capturing.take();
            let _ = act.ui.set_hidden(capture, true);
            act.ui.set_focus(Some(list), true);
            let (Some(action), Some(chord)) = (action, e.chord.clone()) else {
                status.set(act.ui.rt_mut(), forge_ui::tr!("Unchanged.").into());
                return;
            };
            let context = act
                .session
                .keymap()
                .borrow()
                .bindings()
                .iter()
                .find(|b| b.action == action)
                .map_or(KeyContext::Window, |b| b.context.clone());
            let title = act
                .session
                .actions()
                .iter()
                .find(|a| a.id == action)
                .map_or(action.clone(), |a| {
                    forge_ui::l10n::tr_str(&a.title).into_owned()
                });
            let msg = match act.session.rebind(&action, context, chord.clone()) {
                Ok(()) => forge_ui::trf!(
                    "\u{201c}{title}\u{201d} is now {chord}.",
                    title,
                    chord = chord.label()
                ),
                Err(err) => format!("\u{26a0} {err}"),
            };
            status.set(act.ui.rt_mut(), msg);
        });
        pb.on(reset, move |act, _: &Pressed| {
            let Some((id, title)) = selected(act.ui, act.session) else {
                return;
            };
            let msg = match act.session.reset_binding(&id) {
                Ok(()) => forge_ui::trf!(
                    "\u{201c}{title}\u{201d} is back to its default keys.",
                    title = forge_ui::l10n::tr_str(&title)
                ),
                Err(err) => format!("\u{26a0} {err}"),
            };
            status.set(act.ui.rt_mut(), msg);
        });
        pb.on(reset_all, move |act, _: &Pressed| {
            act.session.reset_all_bindings();
            status.set(
                act.ui.rt_mut(),
                forge_ui::tr!("Every binding is back to its default.").into(),
            );
        });
        let mut seen = pb.session().keymap_revision();
        let st = state;
        pb.sync(list, move |s| {
            let rev = s.session.keymap_revision();
            if rev != seen {
                seen = rev;
                let q = st.borrow().query.clone();
                let (_, r) = list_rows(s.session, &q);
                VirtualTree::edit(s.ui, list, |t| sync_rows(t, &r));
            }
            Ok(())
        });
        Ok(())
    });
}

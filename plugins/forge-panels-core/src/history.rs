//! The **Undo history** panel (`forge.undo_history`, Ch.21 §21.18, DoD M2-41): every
//! transaction with its issuer (human / automation:session / script), label, time and state.
//! Activating an entry (double click or Enter) takes the project to the state right after
//! it — one bus call per transaction, through the emitter; activating "Initial state"
//! undoes everything listed. The list follows the mirror, so an automation session's edit appears
//! here in the next frame, tagged `automation:<session>`.

use forge_cmd::{TxnId, TxnState};
use forge_editor::mirror::{HistoryEntry, ProjectMirror};
use forge_editor::panels::PanelCx;
use forge_ui::widgets::{Button, Container, Label, LabelKind, Pressed, RowActivated, VirtualTree};
use forge_ui::{NodeStyle, Role};

use crate::rows::sync_rows;

/// The list key of the "Initial state" row.
pub const INITIAL: u64 = u64::MAX;

fn clock(ms: u64) -> String {
    let s = (ms / 1000) % 86_400;
    forge_ui::trf!(
        "{time} UTC",
        time = format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    )
}

/// One history row's text: position marker, label, state, issuer, time.
pub fn row_label(h: &HistoryEntry, current: bool) -> String {
    let state = match h.state {
        TxnState::Committed => "",
        TxnState::Undone => forge_ui::tr!(" (undone)"),
        TxnState::Open => forge_ui::tr!(" (in progress)"),
        TxnState::Expired => forge_ui::tr!(" (no longer undoable)"),
        TxnState::Cancelled => forge_ui::tr!(" (cancelled)"),
    };
    format!(
        "{}{}{} \u{2014} {} \u{b7} {}",
        if current { "\u{25cf} " } else { "" },
        forge_editor::command_labels::history_label(&h.label),
        state,
        h.issuer_tag(),
        clock(h.at_ms)
    )
}

fn rows(m: &ProjectMirror) -> Vec<(u64, String)> {
    let current = m.undo_target();
    let mut out = Vec::with_capacity(m.history().len() + 1);
    out.push((
        INITIAL,
        if current.is_none() {
            forge_ui::tr!("\u{25cf} Initial state").to_string()
        } else {
            forge_ui::tr!("Initial state").to_string()
        },
    ));
    out.extend(
        m.history()
            .iter()
            .map(|h| (h.txn.0, row_label(h, current == Some(h.txn)))),
    );
    out
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space[1]).padding(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Undo history actions")),
        )?;
        let undo = pb.b.add(
            bar,
            "undo",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Undo")),
        )?;
        let redo = pb.b.add(
            bar,
            "redo",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Redo")),
        )?;
        pb.b.add(
            pb.parent,
            "hint",
            NodeStyle::leaf().padding(space[1]),
            Label::new(forge_ui::tr!(
                "Double-click or press Enter on an entry to go back to it."
            ))
            .kind(LabelKind::Muted),
        )?;
        // M2-70: nothing done yet (only the initial state) is the empty state.
        let empty = pb.b.add(
            pb.parent,
            "empty",
            NodeStyle::leaf(),
            forge_ui::widgets::EmptyState::new(forge_ui::tr!(
                "Nothing to undo yet: your edits, your teammates' and your automation sessions' appear here."
            )),
        )?;
        let mut t = VirtualTree::list(forge_ui::tr!("Undo history")).single_select();
        let initial = rows(&pb.mirror());
        pb.b.hide(empty, initial.len() > 1);
        crate::rows::sync_rows(&mut t, &initial);
        let list =
            pb.b.add(pb.parent, "list", NodeStyle::leaf().grow(1.0), t)?;
        pb.on(undo, |act, _: &Pressed| {
            if act.cmd.undo().is_none() {
                act.session.notify(
                    forge_editor::notify::Level::Info,
                    forge_ui::tr!("Nothing to undo"),
                    "",
                );
            }
        });
        pb.on(redo, |act, _: &Pressed| {
            if act.cmd.redo().is_none() {
                act.session.notify(
                    forge_editor::notify::Level::Info,
                    forge_ui::tr!("Nothing to redo"),
                    "",
                );
            }
        });
        pb.on(list, |act, a: &RowActivated| {
            let target = (a.key != INITIAL).then_some(TxnId(a.key));
            let plan = act.mirror.plan_travel(target);
            act.cmd.travel(&plan);
        });
        let mut seen = pb.mirror().history_revision();
        let mut seen_reveal = pb.session().pending_reveal().map_or(0, |(s, _)| *s);
        pb.sync(list, move |s| {
            let rev = s.mirror.history_revision();
            if rev != seen {
                seen = rev;
                let r = rows(s.mirror);
                let _ = s.ui.set_hidden(empty, r.len() > 1);
                VirtualTree::edit(s.ui, list, |t| sync_rows(t, &r));
            }
            // A console entry's click-through to a transaction selects it here.
            if let Some((seq, forge_editor::session::Reveal::Txn(t))) = s.session.pending_reveal()
                && *seq != seen_reveal
            {
                seen_reveal = *seq;
                let key = t.0;
                VirtualTree::edit(s.ui, list, |v| {
                    v.select(&[key]);
                    if let Some(r) = v.row_of(key) {
                        v.scroll_to_row(r);
                    }
                });
            }
            Ok(())
        });
        Ok(())
    });
}

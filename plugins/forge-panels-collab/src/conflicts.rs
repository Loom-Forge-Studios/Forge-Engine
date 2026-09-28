//! **Conflicts** (`forge.conflicts`, Ch.37 §37.4; DoD M2-61, M6-13): a **three-way view on
//! the reflect tree**.
//!
//! When a pull meets an edit of mine on something the baseline also changed — the same
//! property, a name, a parent, something one side deleted and the other edited — the pull
//! stops and nothing of it is applied. Each conflict is listed with its **base, mine and
//! theirs** values; choose a side for each (or all at once), then *Apply*: the pull goes
//! through with those choices as one command. A failed precondition when the patch applies
//! is shown here too. Nothing is ever dropped silently (E-37: concurrent same-property
//! editing is not promised; this panel is how it is resolved when it happens anyway).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use forge_cmd::Value;
use forge_editor::collab as c;
use forge_editor::panels::PanelCx;
use forge_project::collab::short;
use forge_project::merge::{Conflict, Side, Subject};
use forge_ui::widgets::{LabelKind, Pressed};

use crate::ui::{
    bar, button, feed_once, fill, frame, last_outcome, list, notice, primary, selected, set, show,
    text,
};

/// A value as the conflict row shows it.
#[must_use]
pub fn show_value(subject: &Subject, v: Option<&Value>) -> String {
    match (subject, v) {
        (Subject::Parent(_), None) => forge_ui::tr!("(top level)").into(),
        (_, None) => forge_ui::tr!("(none)").into(),
        (Subject::Parent(_), Some(Value::Entity(k))) => forge_ui::trf!("under {k}", k),
        (_, Some(Value::Text(t))) => format!("\u{201c}{t}\u{201d}"),
        (_, Some(Value::Float(x))) => format!("{x}"),
        (_, Some(Value::Vec3([x, y, z]))) => format!("({x}, {y}, {z})"),
        (_, Some(v)) => format!("{v:?}"),
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let f = frame(pb, forge_ui::tr!("Conflicts"))?;
        let root = f.root;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;
        let problem = pb.b.signal(String::new());
        let problem_label = text(pb, root, "problem", problem, LabelKind::Warning)?;
        let list_id = list(
            pb,
            root,
            "conflicts",
            forge_ui::tr!("Conflicts: base, mine, theirs"),
            160.0,
        )?;
        let cbar = bar(pb, root, "choose_bar", forge_ui::tr!("Choose a side"))?;
        let mine = button(pb, cbar, "keep_mine", forge_ui::tr!("Keep mine"))?;
        let theirs = button(pb, cbar, "take_theirs", forge_ui::tr!("Take theirs"))?;
        let all_mine = button(pb, cbar, "all_mine", forge_ui::tr!("Keep all mine"))?;
        let all_theirs = button(pb, cbar, "all_theirs", forge_ui::tr!("Take all theirs"))?;
        let abar = bar(pb, root, "apply_bar", forge_ui::tr!("Apply"))?;
        let apply = primary(pb, abar, "apply", forge_ui::tr!("Apply the choices"))?;
        text(pb, root, "e37", forge_ui::l10n::tr(c::E37_STATEMENT), LabelKind::Small)?;
        text(pb, root, "status", f.status, LabelKind::Muted)?;

        // The conflicts shown, and the sides chosen here (session state until Apply).
        let shown: Rc<RefCell<Vec<Conflict>>> = Rc::default();
        let chosen: Rc<RefCell<BTreeMap<String, Side>>> = Rc::default();
        let render = {
            let shown = shown.clone();
            let chosen = chosen.clone();
            move |ui: &mut forge_ui::Ui| {
                let rows = shown
                    .borrow()
                    .iter()
                    .enumerate()
                    .map(|(i, x)| {
                        let pick = chosen.borrow().get(&x.subject.key()).copied().or(x.choice);
                        (
                            i as u64,
                            forge_ui::trf!(
                                "{what} \u{2014} base {base} \u{b7} mine {mine} \u{b7} theirs {theirs} \u{2014} {choice}",
                                what = x.label,
                                base = show_value(&x.subject, x.base.as_ref()),
                                mine = show_value(&x.subject, x.mine.as_ref()),
                                theirs = show_value(&x.subject, x.theirs.as_ref()),
                                choice = match pick {
                                    Some(Side::Mine) => forge_ui::tr!("keeping mine"),
                                    Some(Side::Theirs) => forge_ui::tr!("taking theirs"),
                                    None => forge_ui::tr!("choose a side"),
                                }
                            ),
                            pick.is_some(),
                        )
                    })
                    .collect();
                fill(ui, list_id, rows);
            }
        };
        for (btn, side) in [(mine, Side::Mine), (theirs, Side::Theirs)] {
            let (shown, chosen, render) = (shown.clone(), chosen.clone(), render.clone());
            pb.on(btn, move |act, _: &Pressed| {
                let key = selected(act.ui, list_id)
                    .and_then(|k| shown.borrow().get(k as usize).map(|x| x.subject.key()));
                if let Some(k) = key {
                    chosen.borrow_mut().insert(k, side);
                    render(act.ui);
                }
            });
        }
        for (btn, side) in [(all_mine, Side::Mine), (all_theirs, Side::Theirs)] {
            let (shown, chosen, render) = (shown.clone(), chosen.clone(), render.clone());
            pb.on(btn, move |act, _: &Pressed| {
                for x in shown.borrow().iter() {
                    chosen.borrow_mut().insert(x.subject.key(), side);
                }
                render(act.ui);
            });
        }
        {
            let chosen = chosen.clone();
            pb.on(apply, move |act, _: &Pressed| {
                let choices = chosen.borrow().clone();
                act.cmd.emit(c::resolve_command(&choices));
                act.want_turn();
            });
        }

        let mut feeds = false;
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            feed_once(s, f.relay, &mut feeds, crate::LIVE_HZ);
            let rev = s.services.collab.revision() ^ s.mirror.revision().rotate_left(17);
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let ok = notice(s.ui, s.services, &f);
            let st = s.services.collab.status();
            let pending = st.as_ref().and_then(|x| x.pending.clone());
            match &pending {
                Some(p) => set(
                    s.ui,
                    head,
                    forge_ui::trf!("Pulling {incoming} revision(s) up to {short} stopped at {conflicts_count} conflict(s).", incoming = p.incoming, short = short(Some(p.target)), conflicts_count = p.conflicts.len()),
                ),
                None => set(s.ui, head, forge_ui::tr!("No conflicts.").into()),
            }
            let why = pending
                .as_ref()
                .and_then(|p| p.hierarchy_loop.clone())
                .or_else(|| st.as_ref().and_then(|x| x.precondition.clone()));
            set(s.ui, problem, why.clone().unwrap_or_default());
            show(s.ui, problem_label, why.is_some());
            let have = pending.is_some();
            show(s.ui, cbar, ok && have);
            show(s.ui, abar, ok && have);
            let list_now = pending.map(|p| p.conflicts).unwrap_or_default();
            // Choices made here stay until the conflict is gone.
            chosen
                .borrow_mut()
                .retain(|k, _| list_now.iter().any(|x| x.subject.key() == *k));
            *shown.borrow_mut() = list_now;
            render(s.ui);
            set(
                s.ui,
                f.status,
                last_outcome(s.services, &[c::RESOLVE, c::PULL]),
            );
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}

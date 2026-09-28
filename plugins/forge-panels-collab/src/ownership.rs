//! **Ownership claims** (`forge.ownership`, Ch.37 §37.4; DoD M2-69).
//!
//! Claim a subtree — a scene root, a prefab, a body — and it is **read-only for everyone
//! else**: the core refuses their edits inside it (the UI does not just hide them), the
//! inspector shows its fields read-only and the hierarchy the holder's name. Claims and
//! releases are commands, audited; a Reviewer or a Viewer is refused with a reason; a
//! Maintainer or an Owner can release anyone's. Most real conflicts are prevented here, not
//! merged later.

use std::cell::RefCell;
use std::rc::Rc;

use forge_cmd::EntityKey;
use forge_editor::collab as c;
use forge_editor::panels::PanelCx;
use forge_project::collab::Claim;
use forge_ui::widgets::{LabelKind, Pressed};

use crate::ui::{
    bar, button, feed_once, fill, frame, group, last_outcome, list, notice, primary, selected, set,
    show, text,
};

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let f = frame(pb, forge_ui::tr!("Ownership"))?;
        let root = f.root;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;
        let cbar = bar(pb, root, "claim_bar", forge_ui::tr!("Claim"))?;
        let claim = primary(
            pb,
            cbar,
            "claim_selection",
            forge_ui::tr!("Claim the selection"),
        )?;
        let mg = group(pb, root, "mine_group", forge_ui::tr!("Your claims"))?;
        let mine = list(pb, mg, "mine", forge_ui::tr!("Your claims"), 70.0)?;
        let tg = group(pb, root, "team_group", forge_ui::tr!("The team's claims"))?;
        let team = list(
            pb,
            tg,
            "team",
            forge_ui::tr!("The team's claims, by path"),
            90.0,
        )?;
        let rbar = bar(pb, root, "release_bar", forge_ui::tr!("Release"))?;
        let release_mine = button(
            pb,
            rbar,
            "release_mine",
            forge_ui::tr!("Release mine (selected)"),
        )?;
        let release_team = button(
            pb,
            rbar,
            "release_team",
            forge_ui::tr!("Release the selected team claim"),
        )?;
        text(
            pb,
            root,
            "hint",
            forge_ui::tr!(
                "While someone holds a claim, what it covers is read-only for everyone else."
            ),
            LabelKind::Small,
        )?;
        text(pb, root, "status", f.status, LabelKind::Muted)?;

        let rows: Rc<RefCell<(Vec<Claim>, Vec<Claim>)>> = Rc::default();
        pb.on(claim, move |act, _: &Pressed| {
            let sel = act.session.selection.clone();
            if sel.is_empty() {
                act.session.notify(
                    forge_editor::notify::Level::Info,
                    forge_ui::tr!("Nothing to claim"),
                    forge_ui::tr!("Select what you are working on in the hierarchy first."),
                );
            }
            // Claim the topmost selected entities (a claim covers the subtree).
            let top: Vec<EntityKey> = sel
                .iter()
                .copied()
                .filter(|k| {
                    let mut cur = act.mirror.entity(*k).and_then(|e| e.parent);
                    while let Some(p) = cur {
                        if sel.contains(&p) {
                            return false;
                        }
                        cur = act.mirror.entity(p).and_then(|e| e.parent);
                    }
                    true
                })
                .collect();
            for k in top {
                act.cmd.emit(c::claim_command(k));
            }
            act.want_turn();
        });
        {
            let rows = rows.clone();
            pb.on(release_mine, move |act, _: &Pressed| {
                let k = selected(act.ui, mine)
                    .and_then(|i| rows.borrow().0.get(i as usize).map(|c| c.entity));
                if let Some(k) = k {
                    act.cmd.emit(c::release_command(EntityKey(k)));
                }
                act.want_turn();
            });
        }
        {
            let rows = rows.clone();
            pb.on(release_team, move |act, _: &Pressed| {
                let k = selected(act.ui, team)
                    .and_then(|i| rows.borrow().1.get(i as usize).map(|c| c.entity));
                if let Some(k) = k {
                    act.cmd.emit(c::release_command(EntityKey(k)));
                }
                act.want_turn();
            });
        }

        let mut feeds = false;
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            feed_once(s, f.relay, &mut feeds, crate::LIVE_HZ);
            let rev = s.services.collab.revision() ^ s.mirror.revision().rotate_left(13);
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let ok = notice(s.ui, s.services, &f);
            let collab = &s.services.collab;
            let me = collab.me();
            let joined = collab.status().is_some_and(|st| st.joined);
            let edits = collab
                .my_role()
                .is_some_and(forge_project::collab::Role::edits);
            show(s.ui, cbar, ok && joined && edits);
            show(s.ui, rbar, ok && joined);
            let all = collab.claims();
            let (m, t): (Vec<Claim>, Vec<Claim>) = all.into_iter().partition(|c| c.holder == me);
            set(
                s.ui,
                head,
                forge_ui::trf!(
                    "You hold {m_count} claim(s); the team holds {t_count} more.",
                    m_count = m.len(),
                    t_count = t.len()
                ),
            );
            fill(
                s.ui,
                mine,
                m.iter()
                    .enumerate()
                    .map(|(i, c)| (i as u64, c.label.clone(), !ok))
                    .collect(),
            );
            let mut by_path = t.clone();
            by_path.sort_by(|a, b| a.label.cmp(&b.label));
            fill(
                s.ui,
                team,
                by_path
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (i as u64, format!("{} \u{2014} {}", c.label, c.holder), !ok))
                    .collect(),
            );
            *rows.borrow_mut() = (m, by_path);
            set(
                s.ui,
                f.status,
                last_outcome(s.services, &[c::CLAIM, c::RELEASE]),
            );
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}

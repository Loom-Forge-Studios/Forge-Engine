//! The **Notifications** panel (`forge.notifications`, Ch.21 §21.18, DoD M2-42): the
//! history drawer behind the toasts. Every entry shows its level, its stable `ErrorCode`
//! and its text; activating one runs its first action ("Open console", "Retry").
//! Notifications are session state: reading or clearing them changes nothing in the
//! project.

use forge_editor::notify::{NoticeAction, Notification};
use forge_editor::panels::PanelCx;
use forge_editor::session::{SessionState, ShellRequest};
use forge_ui::dock::PanelId;
use forge_ui::widgets::{Button, Container, EmptyState, Pressed, RowActivated, VirtualTree};
use forge_ui::{NodeStyle, Role};

use crate::rows::sync_rows;

/// One entry's row text.
pub fn row_label(n: &Notification) -> String {
    let code = n.code.as_deref().map_or(String::new(), |c| format!("{c} "));
    let action = n
        .actions
        .first()
        .map_or(String::new(), |a| format!(" [{}]", a.label()));
    let detail = if n.detail.is_empty() {
        String::new()
    } else {
        format!(" \u{2014} {}", n.detail)
    };
    // M2-70: a problem's next step follows what happened.
    let next = n
        .next
        .as_deref()
        .map_or(String::new(), |x| format!(" \u{2014} {x}"));
    // The template is the key; the code, the detail (an error's own message) and the
    // action's label are data filled into it (ADR 0046 §6).
    forge_ui::trf!(
        "{unread}{level}: {code}{title}{detail}{next}{action}",
        unread = if n.read { "" } else { "\u{2022} " },
        level = n.level.word(),
        code,
        title = n.title,
        detail,
        next,
        action
    )
}

fn rows(s: &SessionState) -> Vec<(u64, String)> {
    // Newest first: the most recent problem is the one the user is looking for. A new
    // entry changes the prefix, so the list rebuilds; notifications are few (bounded at
    // 500) and rare, so that is cheap.
    s.notifications
        .history()
        .rev()
        .map(|n| (n.id, row_label(n)))
        .collect()
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space;
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space[1]).padding(space[1]),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Notification actions")),
        )?;
        let read = pb.b.add(
            bar,
            "read",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Mark all read")),
        )?;
        let clear = pb.b.add(
            bar,
            "clear",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Clear")),
        )?;
        let empty = pb.b.add(
            pb.parent,
            "empty",
            NodeStyle::leaf().grow(1.0),
            EmptyState::new(forge_ui::tr!(
                "No notifications. Refused commands and errors appear here with their codes."
            )),
        )?;
        let mut t = VirtualTree::list(forge_ui::tr!("Notifications")).single_select();
        let initial = rows(&pb.session());
        let has_any = !initial.is_empty();
        sync_rows(&mut t, &initial);
        let list =
            pb.b.add(pb.parent, "list", NodeStyle::leaf().grow(1.0), t)?;
        pb.b.hide(empty, has_any);
        pb.b.hide(list, !has_any);
        pb.on(read, |act, _: &Pressed| {
            act.session.notifications.mark_all_read()
        });
        pb.on(clear, |act, _: &Pressed| act.session.notifications.clear());
        pb.on(list, |act, a: &RowActivated| {
            let Some(n) = act.session.notifications.get(a.key).cloned() else {
                return;
            };
            match n.actions.first() {
                Some(NoticeAction::OpenPanel { panel, .. }) => {
                    act.session
                        .request(ShellRequest::OpenPanel(PanelId::new(panel)));
                }
                Some(NoticeAction::Run { action, .. }) => {
                    act.session.request(ShellRequest::RunAction(action.clone()));
                }
                Some(NoticeAction::Reveal { reveal, panel, .. }) => {
                    act.session.reveal(reveal.clone(), panel);
                }
                None => {}
            }
        });
        let mut seen = pb.session().notifications.revision();
        pb.sync(list, move |s| {
            let rev = s.session.notifications.revision();
            if rev != seen {
                seen = rev;
                let r = rows(s.session);
                let any = !r.is_empty();
                VirtualTree::edit(s.ui, list, |t| sync_rows(t, &r));
                let _ = s.ui.set_hidden(empty, any);
                let _ = s.ui.set_hidden(list, !any);
            }
            Ok(())
        });
        Ok(())
    });
}

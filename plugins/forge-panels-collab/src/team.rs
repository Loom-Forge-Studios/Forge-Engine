//! **Team and members** (`forge.team`, Ch.37 §37.7; DoD M2-57, M5-17).
//!
//! **Create Team → Add Member in three clicks**: *Create team* (the name is filled in from
//! the project), *Add member…*, *Send invite* (an email, or a minted join code; the role is
//! Developer unless you pick another; path scopes optional). A pending invite is listed with
//! who made it, and *Revoke* drops it. The member list shows each member's role, whether they
//! are online, their sandbox and what they have claimed; Owners and Maintainers change roles
//! and scopes and remove members — never above their own rank (the core refuses it).
//!
//! Every one of these is a command (I7): human-only, checked per command against the issuer's
//! role, audited. Someone without a team joins with a code, or accepts an email invite for
//! their account.

use std::cell::RefCell;
use std::rc::Rc;

use forge_editor::collab as c;
use forge_editor::panels::PanelCx;
use forge_project::collab::{Invite, Member, Role};
use forge_ui::widgets::{LabelKind, Pressed, SegmentedControl};
use forge_ui::{NodeStyle, Signal, Ui, WidgetId};

use crate::ui::{
    bar, button, feed_once, field, fill, frame, group, last_outcome, list, notice, primary,
    selected, set, show, text,
};

/// The roles an invite offers (an Owner is made by changing a member's role).
pub const INVITE_ROLES: [Role; 4] = [
    Role::Maintainer,
    Role::Developer,
    Role::Reviewer,
    Role::Viewer,
];

const TARGETS: &[&str] = &[
    c::TEAM_CREATE,
    c::TEAM_INVITE,
    c::TEAM_REVOKE_INVITE,
    c::TEAM_ACCEPT,
    c::TEAM_SET_ROLE,
    c::TEAM_SET_PATHS,
    c::TEAM_REMOVE,
];

#[derive(Default)]
struct Rows {
    pending: Vec<Invite>,
    members: Vec<Member>,
    mine: Vec<(String, String, Invite)>,
}

struct Parts {
    head: Signal<String>,
    create_group: WidgetId,
    join_group: WidgetId,
    members_group: WidgetId,
    add_bar: WidgetId,
    add_group: WidgetId,
    pending_group: WidgetId,
    pending_bar: WidgetId,
    member_bar: WidgetId,
    paths_bar: WidgetId,
    pending: WidgetId,
    members: WidgetId,
    my_invites: WidgetId,
    accept_invite: WidgetId,
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let f = frame(pb, forge_ui::tr!("Team"))?;
        let root = f.root;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;

        // ---- no team yet: create one, or join one ----
        let create_group = group(pb, root, "create_group", forge_ui::tr!("Create a team"))?;
        text(
            pb,
            create_group,
            "create_text",
            forge_ui::tr!("Make this project a team project: you become the team's Owner and this project its shared baseline. Then add members."),
            LabelKind::Muted,
        )?;
        let name0 = match pb.mirror().setting(forge_project::NAME_SETTING) {
            Some(forge_cmd::Value::Text(t)) if !t.is_empty() => forge_ui::trf!("{t} team", t),
            _ => forge_ui::tr!("My team").to_string(),
        };
        let cbar = bar(pb, create_group, "create_bar", forge_ui::tr!("Create a team"))?;
        let team_name = field(pb, cbar, "team_name", forge_ui::tr!("Team name"), forge_ui::tr!("Team name"), &name0)?;
        let create = primary(pb, cbar, "create", forge_ui::tr!("Create team"))?;
        let join_group = group(pb, root, "join_group", forge_ui::tr!("Join a team"))?;
        let jbar = bar(pb, join_group, "join_bar", forge_ui::tr!("Join with a code"))?;
        let code = field(pb, jbar, "join_code", forge_ui::tr!("Join code"), forge_ui::tr!("ABCD-EFGH"), "")?;
        let join = button(pb, jbar, "join", forge_ui::tr!("Join"))?;
        let my_invites = list(pb, join_group, "my_invites", forge_ui::tr!("Invites for your account"), 50.0)?;
        let accept_invite = button(pb, join_group, "accept_invite", forge_ui::tr!("Accept the selected invite"))?;

        // ---- a team: add members, pending invites, members ----
        let members_group = group(pb, root, "members_group", forge_ui::tr!("Members"))?;
        let add_bar = bar(pb, members_group, "add_bar", forge_ui::tr!("Add a member"))?;
        let add_member = primary(pb, add_bar, "add_member", forge_ui::tr!("Add member\u{2026}"))?;
        let add_group = group(pb, members_group, "add_group", forge_ui::tr!("Invite a member"))?;
        let kind = pb.b.signal(0usize);
        pb.b.add(
            add_group,
            "invite_kind",
            NodeStyle::leaf(),
            SegmentedControl::new(forge_ui::tr!("Invite by"), &[forge_ui::tr!("Email"), forge_ui::tr!("Join code")], kind),
        )?;
        let email = field(
            pb,
            add_group,
            "email",
            forge_ui::tr!("Email"),
            forge_ui::tr!("name@studio.example (or pick Join code)"),
            "",
        )?;
        let role = pb.b.signal(1usize);
        let role_names: Vec<&str> = INVITE_ROLES.iter().map(|r| forge_ui::l10n::tr(r.label())).collect();
        pb.b.add(
            add_group,
            "role",
            NodeStyle::leaf(),
            SegmentedControl::new(forge_ui::tr!("Role"), &role_names, role),
        )?;
        let paths = field(
            pb,
            add_group,
            "paths",
            forge_ui::tr!("Publish scope (optional)"),
            forge_ui::tr!("scene/Levels/**, settings/**"),
            "",
        )?;
        let ibar = bar(pb, add_group, "invite_bar", forge_ui::tr!("Invite"))?;
        let invite = primary(pb, ibar, "invite", forge_ui::tr!("Send invite"))?;
        let cancel = button(pb, ibar, "cancel_add", forge_ui::tr!("Cancel"))?;
        pb.b.hide(add_group, true);
        let members = list(pb, members_group, "members", forge_ui::tr!("Members"), 110.0)?;
        let member_bar = bar(pb, members_group, "member_bar", forge_ui::tr!("Change the selected member"))?;
        let member_role = pb.b.signal(2usize);
        let all_roles: Vec<&str> = Role::ALL.iter().map(|r| forge_ui::l10n::tr(r.label())).collect();
        pb.b.add(
            member_bar,
            "member_role",
            NodeStyle::leaf(),
            SegmentedControl::new(forge_ui::tr!("New role"), &all_roles, member_role),
        )?;
        let set_role = button(pb, member_bar, "set_role", forge_ui::tr!("Change role"))?;
        let remove = button(pb, member_bar, "remove", forge_ui::tr!("Remove from team"))?;
        let paths_bar = bar(pb, members_group, "paths_bar", forge_ui::tr!("Publish scope"))?;
        let member_paths = field(
            pb,
            paths_bar,
            "member_paths",
            forge_ui::tr!("Publish scope"),
            forge_ui::tr!("empty: everywhere"),
            "",
        )?;
        let set_paths = button(pb, paths_bar, "set_paths", forge_ui::tr!("Set scope"))?;
        let pending_group = group(pb, root, "pending_group", forge_ui::tr!("Pending invites"))?;
        let pending = list(pb, pending_group, "pending", forge_ui::tr!("Pending invites"), 60.0)?;
        let pending_bar = bar(pb, pending_group, "pending_bar", forge_ui::tr!("Pending invite"))?;
        let revoke = button(pb, pending_bar, "revoke", forge_ui::tr!("Revoke"))?;
        text(pb, root, "status", f.status, LabelKind::Muted)?;

        let rows = Rc::new(RefCell::new(Rows::default()));
        let parts = Rc::new(Parts {
            head,
            create_group,
            join_group,
            members_group,
            add_bar,
            add_group,
            pending_group,
            pending_bar,
            member_bar,
            paths_bar,
            pending,
            members,
            my_invites,
            accept_invite,
        });

        // Click 1.
        pb.on(create, move |act, _: &Pressed| {
            let n = team_name.get(act.ui.rt());
            act.cmd.emit(c::create_command(n.trim()));
            act.want_turn();
        });
        pb.on(join, move |act, _: &Pressed| {
            let t = code.get(act.ui.rt());
            if !t.trim().is_empty() {
                act.cmd.emit(c::accept_command(t.trim()));
            }
            act.want_turn();
        });
        {
            let rows = rows.clone();
            pb.on(accept_invite, move |act, _: &Pressed| {
                let inv = selected(act.ui, my_invites)
                    .and_then(|k| rows.borrow().mine.get(k as usize).map(|m| m.2.id.clone()));
                if let Some(id) = inv {
                    act.cmd.emit(c::accept_command(&id));
                }
                act.want_turn();
            });
        }
        // Click 2.
        pb.on(add_member, move |act, _: &Pressed| {
            show(act.ui, add_group, true);
            act.want_turn();
        });
        pb.on(cancel, move |act, _: &Pressed| show(act.ui, add_group, false));
        // Click 3.
        pb.on(invite, move |act, _: &Pressed| {
            let by_code = kind.get(act.ui.rt()) == 1;
            let mail = email.get(act.ui.rt());
            let r = INVITE_ROLES
                .get(role.get(act.ui.rt()))
                .copied()
                .unwrap_or(Role::Developer);
            let scopes = c::parse_paths(&paths.get(act.ui.rt()));
            let to = (!by_code && !mail.trim().is_empty()).then(|| mail.trim().to_string());
            act.cmd.emit(c::invite_command(to.as_deref(), r, &scopes));
            email.set(act.ui.rt_mut(), String::new());
            show(act.ui, add_group, false);
            act.want_turn();
        });
        {
            let rows = rows.clone();
            pb.on(revoke, move |act, _: &Pressed| {
                let id = selected(act.ui, pending)
                    .and_then(|k| rows.borrow().pending.get(k as usize).map(|i| i.id.clone()));
                if let Some(id) = id {
                    act.cmd.emit(c::revoke_invite_command(&id));
                }
                act.want_turn();
            });
        }
        let member_of = {
            let rows = rows.clone();
            move |ui: &mut Ui| {
                selected(ui, members)
                    .and_then(|k| rows.borrow().members.get(k as usize).map(|m| m.user.clone()))
            }
        };
        {
            let who = member_of.clone();
            pb.on(set_role, move |act, _: &Pressed| {
                if let Some(u) = who(act.ui) {
                    let r = Role::ALL
                        .get(member_role.get(act.ui.rt()))
                        .copied()
                        .unwrap_or(Role::Developer);
                    act.cmd.emit(c::set_role_command(&u, r));
                }
                act.want_turn();
            });
        }
        {
            let who = member_of.clone();
            pb.on(remove, move |act, _: &Pressed| {
                if let Some(u) = who(act.ui) {
                    act.cmd.emit(c::remove_command(&u));
                }
                act.want_turn();
            });
        }
        {
            let who = member_of;
            pb.on(set_paths, move |act, _: &Pressed| {
                if let Some(u) = who(act.ui) {
                    let p = c::parse_paths(&member_paths.get(act.ui.rt()));
                    act.cmd.emit(c::set_paths_command(&u, &p));
                }
                act.want_turn();
            });
        }

        let mut feeds = false;
        let mut seen = u64::MAX;
        pb.sync(root, move |s| {
            feed_once(s, f.relay, &mut feeds, crate::LIVE_HZ);
            let rev = s.services.collab.revision() ^ s.mirror.revision().rotate_left(7);
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let ok = notice(s.ui, s.services, &f);
            refresh(s.ui, s.services, &parts, &rows, ok);
            set(s.ui, f.status, last_outcome(s.services, TARGETS));
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}

fn refresh(
    ui: &mut Ui,
    services: &forge_editor::services::EditorServices,
    p: &Parts,
    rows: &Rc<RefCell<Rows>>,
    ok: bool,
) {
    let collab = &services.collab;
    let me = collab.me();
    let team = collab.team();
    let my_role = team.as_ref().and_then(|t| t.role_of(&me));
    let manages = ok && my_role.is_some_and(Role::manages_members);
    let bound_name = collab.status().and_then(|s| s.team_name.clone());
    match (&team, &bound_name) {
        (Some(t), _) => set(
            ui,
            p.head,
            forge_ui::trf!(
                "Team \u{201c}{name}\u{201d} \u{b7} you are {role} \u{b7} {members_count} member(s), {invites_count} pending invite(s)",
                name = t.name,
                role = my_role.map_or(forge_ui::tr!("not a member"), Role::label),
                members_count = t.members.len(),
                invites_count = t.invites.len()
            ),
        ),
        (None, Some(n)) => set(
            ui,
            p.head,
            forge_ui::trf!(
                "This project belongs to team \u{201c}{n}\u{201d}, which this team server does not know.",
                n
            ),
        ),
        (None, None) => set(
            ui,
            p.head,
            forge_ui::tr!("This project does not belong to a team.").to_string(),
        ),
    }
    let has_team = team.is_some();
    show(ui, p.create_group, !has_team && ok);
    show(ui, p.join_group, !has_team && ok);
    show(ui, p.members_group, has_team);
    // A join code is a credential for its role: only those who could mint one see the
    // pending list (the identity view withholds the code from everyone else, too).
    let sees_invites = my_role.is_some_and(Role::manages_members);
    show(ui, p.pending_group, has_team && sees_invites);
    show(ui, p.add_bar, manages);
    if !manages {
        show(ui, p.add_group, false);
    }
    show(ui, p.pending_bar, manages);
    show(ui, p.member_bar, manages);
    show(ui, p.paths_bar, manages);
    // Email invites waiting for my account.
    let mine = collab
        .identity
        .as_ref()
        .map(|i| i.invites_for(&me))
        .unwrap_or_default();
    show(ui, p.my_invites, !mine.is_empty());
    show(ui, p.accept_invite, !mine.is_empty());
    fill(
        ui,
        p.my_invites,
        mine.iter()
            .enumerate()
            .map(|(i, (_, name, inv))| {
                (
                    i as u64,
                    forge_ui::trf!(
                        "\u{201c}{name}\u{201d} invites you as {role} (from {by})",
                        name,
                        role = forge_ui::l10n::tr(inv.role.label()),
                        by = inv.by
                    ),
                    false,
                )
            })
            .collect(),
    );
    let Some(t) = team else {
        let mut r = rows.borrow_mut();
        r.pending.clear();
        r.members.clear();
        r.mine = mine;
        return;
    };
    let online: std::collections::BTreeSet<String> = collab
        .server
        .as_ref()
        .map(|s| s.presence().into_iter().map(|p| p.user).collect())
        .unwrap_or_default();
    let sandboxes = collab
        .server
        .as_ref()
        .map(|s| s.sandboxes(&me))
        .unwrap_or_default();
    let claims = forge_project::collab::claims_by_holder(&collab.claims());
    let pending = if sees_invites { t.invites } else { Vec::new() };
    fill(
        ui,
        p.pending,
        pending
            .iter()
            .enumerate()
            .map(|(i, inv)| {
                let scope = if inv.paths.is_empty() {
                    String::new()
                } else {
                    forge_ui::trf!(", publishing to {paths}", paths = inv.paths.join(", "))
                };
                (
                    i as u64,
                    forge_ui::trf!(
                        "{who} \u{2014} {role}{scope} \u{2014} invited by {by}",
                        who = inv.target(),
                        role = forge_ui::l10n::tr(inv.role.label()),
                        scope,
                        by = inv.by
                    ),
                    !ok,
                )
            })
            .collect(),
    );
    fill(
        ui,
        p.members,
        t.members
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let on = m.user == me || online.contains(&m.user);
                let sandbox = sandboxes.iter().find(|r| r.owner == m.user).map(|r| {
                    if r.withheld {
                        forge_ui::tr!("sandbox private").to_string()
                    } else {
                        forge_ui::trf!("{n} unpublished", n = r.unpublished)
                    }
                });
                let mut line = forge_ui::trf!(
                    "{user}{you} \u{2014} {role} \u{2014} {online}",
                    user = m.user,
                    you = if m.user == me {
                        forge_ui::tr!(" (you)")
                    } else {
                        ""
                    },
                    role = forge_ui::l10n::tr(m.role.label()),
                    online = if on {
                        forge_ui::tr!("online")
                    } else {
                        forge_ui::tr!("offline")
                    }
                );
                if let Some(sb) = sandbox {
                    line.push_str(&format!(" \u{2014} {sb}"));
                }
                if let Some(cl) = claims.get(&m.user) {
                    line.push_str(&forge_ui::trf!(
                        " \u{2014} claims {what}",
                        what = cl.join(", ")
                    ));
                }
                if !m.paths.is_empty() {
                    line.push_str(&forge_ui::trf!(
                        " \u{2014} publishes to {paths}",
                        paths = m.paths.join(", ")
                    ));
                }
                (i as u64, line, !ok)
            })
            .collect(),
    );
    let mut r = rows.borrow_mut();
    r.pending = pending;
    r.members = t.members;
    r.mine = mine;
}

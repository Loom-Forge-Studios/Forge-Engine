//! `test_team_panel` (Ch.37 §37.7; DoD M2-57, M5-17 on the in-memory identity database):
//!
//! * **Create Team → Add Member in three clicks** — *Create team*, *Add member…*, *Send
//!   invite* (the email is typed; the role defaults to Developer): the team exists with this
//!   person as its Owner, and the invite is pending, listed with who made it;
//! * a pending invite is **revocable**; a **join code** invite is listed with its code and a
//!   teammate joins with it (their editor rebuilds from the team baseline);
//! * members are listed with role, online state, sandbox and claims; changing a role is one
//!   selection and one click;
//! * every one of these is a **command**, in the audit with who did it; someone without the
//!   rights sees no management controls, and the core refuses the command anyway.

mod common;

use common::*;
use forge_editor::connect::audit::AuditQuery;
use forge_editor::core::EditorCore;
use forge_project::collab::{CollabView, IdentityBackend, IdentityView, InviteTo, Role};
use forge_ui::{KeyCode, Modifiers};

const T: &str = "forge.team";

#[test]
fn create_team_then_add_member_in_three_clicks() {
    let s = server();
    let mut rig = rig(&s, &[T]);
    feed(&mut rig);
    let head = at(&rig, T, &["head"]);
    assert!(
        label(&rig, head).contains("does not belong to a team"),
        "{}",
        label(&rig, head)
    );
    let mut clicks = 0;
    // 1: Create team (the name is filled in).
    tap(&mut rig, T, &["create_group", "create_bar", "create"]);
    clicks += 1;
    feed(&mut rig);
    let team = s
        .server
        .team()
        .unwrap_or_else(|| panic!("no team was created"));
    let t = s
        .identity
        .team(&team)
        .unwrap_or_else(|| panic!("the team is unknown"));
    assert_eq!(t.role_of("tester"), Some(Role::Owner));
    assert!(
        label(&rig, head).contains("you are Owner"),
        "{}",
        label(&rig, head)
    );
    // 2: Add member…
    let add = at(&rig, T, &["members_group", "add_bar", "add_member"]);
    press(&mut rig, add);
    clicks += 1;
    let add_group = at(&rig, T, &["members_group", "add_group"]);
    assert!(!hidden(&rig, add_group), "the invite form opens");
    write(
        &mut rig,
        T,
        &["members_group", "add_group", "email"],
        "bob@studio.example",
    );
    // 3: Send invite.
    tap(
        &mut rig,
        T,
        &["members_group", "add_group", "invite_bar", "invite"],
    );
    clicks += 1;
    feed(&mut rig);
    assert_eq!(clicks, 3);
    let pending = at(&rig, T, &["pending_group", "pending"]);
    let r = rows(&mut rig, pending);
    assert!(
        r.iter().any(|l| l.contains("bob@studio.example")
            && l.contains("Developer")
            && l.contains("invited by tester")),
        "{r:?}"
    );
    // Revocable.
    select_containing(&mut rig, pending, "bob@studio.example");
    tap(&mut rig, T, &["pending_group", "pending_bar", "revoke"]);
    feed(&mut rig);
    assert!(rows(&mut rig, pending).is_empty(), "the invite was revoked");
    assert!(s.identity.team(&team).is_some_and(|t| t.invites.is_empty()));
    // A join code invite (Invite by: Join code), redeemed by a teammate.
    press(&mut rig, add);
    let kind = at(&rig, T, &["members_group", "add_group", "invite_kind"]);
    rig.h.focus(kind);
    rig.h.press(KeyCode::Right, Modifiers::NONE);
    rig.turn();
    rig.settle();
    tap(
        &mut rig,
        T,
        &["members_group", "add_group", "invite_bar", "invite"],
    );
    feed(&mut rig);
    let r = rows(&mut rig, pending);
    let code = s
        .identity
        .team_record(&team)
        .and_then(|t| {
            t.invites.iter().find_map(|i| match &i.to {
                InviteTo::Code(c) => Some(c.clone()),
                InviteTo::Email(_) => None,
            })
        })
        .unwrap_or_else(|| panic!("no join code: {r:?}"));
    assert!(
        r.iter().any(|l| l.contains(&format!("join code {code}"))),
        "{r:?}"
    );
    let mut bob = mate(&s, "bob");
    mate_send(&mut bob, forge_editor::collab::accept_command(&code))
        .unwrap_or_else(|e| panic!("{e}"));
    feed(&mut rig);
    let members = at(&rig, T, &["members_group", "members"]);
    let m = rows(&mut rig, members);
    assert!(m.iter().any(|l| l.starts_with("bob — Developer")), "{m:?}");
    assert!(
        m.iter()
            .any(|l| l.starts_with("tester (you) — Owner — online")),
        "{m:?}"
    );
    // Change bob's role: select him, pick Viewer, one click.
    select_containing(&mut rig, members, "bob");
    let role = at(&rig, T, &["members_group", "member_bar", "member_role"]);
    rig.h.focus(role);
    rig.h.press(KeyCode::Right, Modifiers::NONE);
    rig.h.press(KeyCode::Right, Modifiers::NONE);
    rig.turn();
    rig.settle();
    tap(&mut rig, T, &["members_group", "member_bar", "set_role"]);
    feed(&mut rig);
    assert_eq!(
        s.identity.team(&team).and_then(|t| t.role_of("bob")),
        Some(Role::Viewer)
    );
    // Every step was a command, audited with who did it.
    let book = EditorCore::audit_book(&rig.core);
    let events: Vec<String> = book
        .query(&AuditQuery::parse("origin:team user:tester"))
        .into_iter()
        .map(|r| r.event)
        .collect();
    for e in ["create", "invite", "revoke_invite", "set_role"] {
        assert!(
            events.iter().any(|x| x == e),
            "{e} is not audited: {events:?}"
        );
    }
}

#[test]
fn a_viewer_sees_no_management_and_the_core_refuses_it_anyway() {
    let s = server();
    // Ada owns the team; the rig's person joins as a Viewer by typing a code.
    let mut ada = mate(&s, "ada");
    mate_send(&mut ada, forge_editor::collab::create_command("Studio"))
        .unwrap_or_else(|e| panic!("{e}"));
    let team = s.server.team().unwrap_or_else(|| panic!("no team"));
    mate_send(
        &mut ada,
        forge_editor::collab::invite_command(None, Role::Viewer, &[]),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let code = s
        .identity
        .team_record(&team)
        .and_then(|t| {
            t.invites.iter().find_map(|i| match &i.to {
                InviteTo::Code(c) => Some(c.clone()),
                InviteTo::Email(_) => None,
            })
        })
        .unwrap_or_else(|| panic!("no code"));
    let mut rig = rig(&s, &[T]);
    feed(&mut rig);
    write(&mut rig, T, &["join_group", "join_bar", "join_code"], &code);
    tap(&mut rig, T, &["join_group", "join_bar", "join"]);
    feed(&mut rig);
    assert_eq!(
        s.identity.team(&team).and_then(|t| t.role_of("tester")),
        Some(Role::Viewer)
    );
    assert!(hidden(&rig, at(&rig, T, &["members_group", "add_bar"])));
    assert!(hidden(&rig, at(&rig, T, &["members_group", "member_bar"])));
    // The core refuses the command whatever the UI shows.
    rig.shell
        .emitter()
        .emit(forge_editor::collab::invite_command(None, Role::Owner, &[]));
    rig.turn();
    rig.settle();
    let n = notices(&rig);
    assert!(n.iter().any(|l| l.contains("CMD-0014")), "{n:?}");
    assert!(s.identity.team(&team).is_some_and(|t| t.invites.is_empty()));
}

//! `test_join_code_exposure` (Ch.37 §37.7, §37.6; WP-U10): **a join code reaches only
//! someone who could have minted it.**
//!
//! A join code is a bearer credential for its role: whoever types it joins as that role. If a
//! Developer could read a live Owner code, a second account of theirs would join as an Owner —
//! an escalation the per-command role check never sees, because the accept itself is valid.
//! So:
//!
//! * the identity view the panels read (`CollabServices::team`, `IdentityView::team_as`)
//!   carries a code's text only where the reader could have minted that invite
//!   (`may_assign`: an Owner every code, a Maintainer the codes below Maintainer); the
//!   viewer-less `IdentityView::team` carries none;
//! * the Team panel lists pending invites only to those who manage members, and a listed
//!   invite whose code the reader may not see says *a join code*, never the code.
//!
//! Checked from every role, with an Owner code and a Viewer code pending.
//!
//! Positive controls (W2): an identity view that hands every reader the whole record (the
//! defect as found) must fail the check — for a Maintainer on the Team panel (it lists the
//! Owner code), and for a Developer on the team view the panels read (the panel hides the
//! list, the view still carries the code).

mod common;

use std::sync::Arc;

use common::*;
use forge_editor::collab::CollabServices;
use forge_project::collab::{
    Account, CollabView, IdentityBackend, IdentityView, Invite, InviteTo, MemoryIdentity, Observer,
    Role, Team, may_assign,
};

const T: &str = "forge.team";

/// The defect as found: every reader gets the whole record, codes included.
struct LeakyView(Arc<MemoryIdentity>);

impl IdentityView for LeakyView {
    fn backend(&self) -> &'static str {
        self.0.backend()
    }
    fn generation(&self) -> u64 {
        self.0.generation()
    }
    fn account(&self, user: &str) -> Option<Account> {
        self.0.account(user)
    }
    fn team(&self, id: &str) -> Option<Team> {
        self.0.team_record(id)
    }
    fn team_as(&self, id: &str, _viewer: &str) -> Option<Team> {
        self.0.team_record(id)
    }
    fn invites_for(&self, user: &str) -> Vec<(String, String, Invite)> {
        self.0.invites_for(user)
    }
    fn observe(&self, f: Observer) -> u64 {
        self.0.observe(f)
    }
    fn unobserve(&self, id: u64) {
        self.0.unobserve(id);
    }
}

/// The newest join code in the team.
fn newest_code(s: &Server, team: &str) -> Result<String, String> {
    s.identity
        .team_record(team)
        .and_then(|t| {
            t.invites.iter().rev().find_map(|i| match &i.to {
                InviteTo::Code(c) => Some(c.clone()),
                InviteTo::Email(_) => None,
            })
        })
        .ok_or_else(|| "no join code".to_string())
}

fn mint(s: &Server, ada: &mut Mate, team: &str, role: Role) -> Result<String, String> {
    mate_send(ada, forge_editor::collab::invite_command(None, role, &[]))
        .map_err(|e| format!("minting a {} code: {e}", role.label()))?;
    newest_code(s, team)
}

fn codes_stay_with_their_minters(role: Role, leaky: bool) -> Result<(), String> {
    let s = server();
    let mut ada = mate(&s, "ada");
    mate_send(&mut ada, forge_editor::collab::create_command("Studio"))
        .map_err(|e| format!("create: {e}"))?;
    let team = s.server.team().ok_or("no team")?;
    // The rig's person joins as `role` by typing a code.
    let mine = mint(&s, &mut ada, &team, role)?;
    let view = Arc::new(LeakyView(s.identity.clone()));
    let panel_view = view.clone();
    let mut rig = rig_with(&s, &[T], move |cfg| {
        if leaky {
            cfg.services.collab.identity = Some(panel_view);
        }
    });
    feed(&mut rig);
    write(&mut rig, T, &["join_group", "join_bar", "join_code"], &mine);
    tap(&mut rig, T, &["join_group", "join_bar", "join"]);
    feed(&mut rig);
    if s.identity.team(&team).and_then(|t| t.role_of("tester")) != Some(role) {
        return Err(format!(
            "the rig's person did not join as a {}",
            role.label()
        ));
    }
    // Two live codes: the most valuable one, and the least.
    let codes = [
        (Role::Owner, mint(&s, &mut ada, &team, Role::Owner)?),
        (Role::Viewer, mint(&s, &mut ada, &team, Role::Viewer)?),
    ];
    feed(&mut rig);
    let who = role.label();

    // The Team panel.
    let group = at(&rig, T, &["pending_group"]);
    let listed = !hidden(&rig, group);
    if listed != role.manages_members() {
        return Err(format!(
            "the Team panel {} a {who} the pending invites",
            if listed { "lists" } else { "does not list" }
        ));
    }
    let pending = at(&rig, T, &["pending_group", "pending"]);
    let rows = rows(&mut rig, pending);
    let head = label(&rig, at(&rig, T, &["head"]));
    for (r, code) in &codes {
        let may = may_assign(Some(role), None, *r).is_ok();
        let shown = rows.iter().any(|l| l.contains(code.as_str())) || head.contains(code.as_str());
        if shown != may {
            return Err(format!(
                "the Team panel {} a {who} the {} join code: {rows:?}",
                if shown { "shows" } else { "does not show" },
                r.label()
            ));
        }
    }
    if role.manages_members() && !rows.iter().any(|l| l.contains("Owner")) {
        return Err(format!(
            "the Owner invite is not listed for a {who}: {rows:?}"
        ));
    }

    // The team view the panels read.
    let mut probe = CollabServices::default();
    probe.attach(&rig.core);
    if leaky {
        probe.identity = Some(view);
    }
    let t = probe.team().ok_or("the panels' services see no team")?;
    for (r, code) in &codes {
        let may = may_assign(Some(role), None, *r).is_ok();
        let reads = t
            .invites
            .iter()
            .any(|i| i.to == InviteTo::Code(code.clone()));
        if reads != may {
            return Err(format!(
                "a {who} {} the {} join code in its team view",
                if reads { "reads" } else { "cannot read" },
                r.label()
            ));
        }
        if !reads && !t.invites.iter().any(|i| i.role == *r && i.code_withheld()) {
            return Err(format!(
                "the {} invite is not listed (withheld) for a {who}",
                r.label()
            ));
        }
    }
    // Nobody's viewer-less read carries a code.
    let bare = s.identity.team(&team).ok_or("no team")?;
    if bare
        .invites
        .iter()
        .any(|i| matches!(&i.to, InviteTo::Code(c) if !c.is_empty()))
    {
        return Err("IdentityView::team carries a join code".into());
    }
    Ok(())
}

#[test]
fn join_codes_reach_only_those_who_could_mint_them() {
    for role in Role::ALL {
        codes_stay_with_their_minters(role, false)
            .unwrap_or_else(|e| panic!("as a {}: {e}", role.label()));
    }
}

#[test]
fn positive_control_a_leaky_view_shows_a_maintainer_the_owner_code() {
    let e = codes_stay_with_their_minters(Role::Maintainer, true)
        .expect_err("a view that hands out every code must fail the check");
    assert!(
        e.contains("the Team panel shows a Maintainer the Owner join code"),
        "{e}"
    );
}

#[test]
fn positive_control_a_leaky_view_lets_a_developer_read_the_owner_code() {
    let e = codes_stay_with_their_minters(Role::Developer, true)
        .expect_err("a view that hands out every code must fail the check");
    assert!(
        e.contains("a Developer reads the Owner join code in its team view"),
        "{e}"
    );
}

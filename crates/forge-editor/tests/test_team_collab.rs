//! Teams, sandboxes and collaboration in the core (Ch.37, Ch.38 §38.2; WP-U10, ADR 0039),
//! against the labelled in-memory identity database and team server (D-4) over a real
//! baseline store. Several editors — one core per person, each its own sandbox — share one
//! team server, as they will share `forge-server`.
//!
//! * `test_role_enforcement`: **authorisation per command** — a Viewer cannot edit, a
//!   Developer cannot invite, a Maintainer cannot make anyone a Maintainer or themselves an
//!   Owner, and a role reduced mid-session is felt by the very next command; team commands
//!   are human-only and audited. Control: a core without its guard ignores roles (the
//!   Viewer's edit goes through first; the Maintainer's escalation would follow).
//! * `test_ownership_claims`: a claimed subtree is **read-only for everyone else, enforced by
//!   the core** — an edit, a spawn under it, a move into it, an automation session's edit and an
//!   undo are all refused; a Viewer cannot claim; an Owner can release anyone's claim. Control: a
//!   core without its guard lets the edit through.
//! * `test_live_pull`: the same publishes reach a **Live** sandbox as they land and a **Pull**
//!   sandbox when it pulls, to the **same final state with the same keys** as the baseline
//!   (I20's shape on the in-memory server). Control: pulls that replace the project
//!   (`forge.project.load`) instead of patching it lose the shared keys.
//! * `test_rebase_conflicts`: two edits of one property meet: the pull **stops**, mine is
//!   kept, the conflict is listed with base / mine / theirs, publishing waits, and choosing a
//!   side applies it. Control: a pull that swallows the conflict loses my edit.
//! * `test_review_queue_and_scopes`: review-gated publishing, approval by a reviewer (never
//!   the author), a per-path override, and publish scopes.
//! * `test_lapse_degrades_never_locks` (E-57): with the Team licence lapsed, every edit,
//!   save and build still works; only collaboration commands are refused, with the reason.
//!   Control: a guard that locks edits on lapse fails it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_cmd::{EditorCommand, EntityKey, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::collab::licence::{Entitlement, MemoryEntitlement, SeatKind, Tier};
use forge_editor::collab::{self as c, CollabFaults};
use forge_editor::connect::audit::AuditQuery;
use forge_editor::core::{CollabAttach, EditorCore, LocalBus, SharedCore};
use forge_project::collab::{
    CollabView, IdentityBackend, IdentityView, InviteTo, MemoryCollab, MemoryIdentity, Policy, Role,
};
use forge_project::format::ProjectDoc;
use forge_project::merge::Side;

const DAY: u64 = 86_400_000;
const NOW: u64 = 20_000 * DAY;

struct Server {
    identity: Arc<MemoryIdentity>,
    server: Arc<MemoryCollab>,
    licence: Arc<MemoryEntitlement>,
}

fn server() -> Server {
    static N: AtomicU64 = AtomicU64::new(0);
    let identity = Arc::new(MemoryIdentity::new());
    let name = format!(
        "team-collab-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let store = forge_project::memory::open_named(&name, "forge-server");
    let server = Arc::new(MemoryCollab::new(store, identity.clone()));
    let licence = Arc::new(MemoryEntitlement::team_for_a_year(NOW));
    licence.set_now(Some(NOW));
    Server {
        identity,
        server,
        licence,
    }
}

/// One person's editor: a core attached to the team server, and their client.
struct Editor {
    core: SharedCore,
    ui: LocalBus,
}

fn editor(s: &Server, user: &str) -> Editor {
    let core = EditorCore::new();
    EditorCore::attach_collab(
        &core,
        CollabAttach {
            identity: s.identity.clone(),
            server: s.server.clone(),
            licence: s.licence.clone(),
            owner: user.into(),
            email: Some(format!("{user}@studio.example")),
        },
    );
    let ui = EditorCore::connect(&core, Issuer::Human { user: user.into() });
    Editor { core, ui }
}

/// Send `cmd`; `Err` with the refusal (its code first) or the failed outcome.
fn send(e: &mut Editor, cmd: EditorCommand) -> Result<(), String> {
    let before = EditorCore::collab_status(&e.core).map_or(0, |s| s.outcomes.len());
    e.ui.apply(cmd, None);
    let p = e.ui.pump();
    if let Some(r) = p.refused.first() {
        return Err(format!(
            "{}: {}",
            r.rejection.code().as_str(),
            r.rejection.error
        ));
    }
    let st = EditorCore::collab_status(&e.core);
    match st.as_ref().and_then(|s| s.outcomes.last()) {
        Some(o) if st.as_ref().is_some_and(|s| s.outcomes.len() != before) && !o.ok => {
            Err(o.message.clone())
        }
        _ => Ok(()),
    }
}

fn spawn(e: &mut Editor, name: &str, parent: Option<EntityKey>) -> EntityKey {
    let next = EditorCore::read(&e.core, |p| p.next_key());
    send(
        e,
        EditorCommand::Spawn {
            name: name.into(),
            parent,
        },
    )
    .unwrap_or_else(|r| panic!("spawn {name}: {r}"));
    next
}

fn set(e: &mut Editor, k: EntityKey, path: &str, v: f64) -> Result<(), String> {
    send(
        e,
        EditorCommand::SetProperty {
            entity: k,
            path: path.into(),
            value: Value::Float(v),
        },
    )
}

fn prop(e: &Editor, k: EntityKey, path: &str) -> Option<Value> {
    EditorCore::read(&e.core, |p| {
        p.entity(k).and_then(|x| x.property(path)).cloned()
    })
}

fn team_id(s: &Server) -> String {
    s.server.team().unwrap_or_else(|| panic!("no team"))
}

/// Mint a join code for `role` (as `by`), and have `user` join with it in a fresh editor.
fn join(s: &Server, by: &mut Editor, user: &str, role: Role) -> Editor {
    send(by, c::invite_command(None, role, &[])).unwrap_or_else(|r| panic!("invite: {r}"));
    let code = s
        .identity
        .team_record(&team_id(s))
        .and_then(|t| {
            t.invites.iter().rev().find_map(|i| match &i.to {
                InviteTo::Code(c) => Some(c.clone()),
                InviteTo::Email(_) => None,
            })
        })
        .unwrap_or_else(|| panic!("no code"));
    let mut e = editor(s, user);
    send(&mut e, c::accept_command(&code)).unwrap_or_else(|r| panic!("{user} joins: {r}"));
    e
}

fn sorted_doc(core: &SharedCore) -> ProjectDoc {
    let mut d = EditorCore::read(core, ProjectDoc::from_project);
    d.entities.sort_by_key(|e| e.id);
    d
}

fn head_doc(s: &Server) -> ProjectDoc {
    use forge_project::collab::CollabBackend;
    let mut d = (*s
        .server
        .doc_at(s.server.head())
        .unwrap_or_else(|e| panic!("{e}")))
    .clone();
    d.entities.sort_by_key(|e| e.id);
    d
}

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

// ---- roles --------------------------------------------------------------------------------

fn roles_hold(faults: CollabFaults) -> Result<(), String> {
    let s = server();
    let mut ada = editor(&s, "ada");
    let lamp = spawn(&mut ada, "Lamp", None);
    send(&mut ada, c::create_command("Studio")).map_err(|r| format!("create: {r}"))?;
    let mut bob = join(&s, &mut ada, "bob", Role::Developer);
    let mut carol = join(&s, &mut ada, "carol", Role::Viewer);
    let mut dave = join(&s, &mut ada, "dave", Role::Maintainer);
    for e in [&bob, &carol, &dave] {
        EditorCore::set_collab_faults(&e.core, faults);
    }
    check(
        prop(&bob, lamp, "x").is_none() && EditorCore::read(&bob.core, |p| p.contains(lamp)),
        "a joining sandbox holds the baseline, with its keys",
    )?;
    // A Viewer does not edit.
    check(set(&mut carol, lamp, "x", 1.0).is_err(), "a Viewer edited")?;
    // A Developer does not invite.
    check(
        send(&mut bob, c::invite_command(None, Role::Viewer, &[])).is_err(),
        "a Developer invited",
    )?;
    // A Maintainer never raises anyone to its own rank or above, itself included.
    let esc = send(&mut dave, c::set_role_command("bob", Role::Maintainer));
    check(esc.is_err(), "a Maintainer made a Maintainer")?;
    check(
        send(&mut dave, c::set_role_command("dave", Role::Owner)).is_err(),
        "a Maintainer made itself an Owner",
    )?;
    check(
        s.identity.team(&team_id(&s)).and_then(|t| t.role_of("bob")) == Some(Role::Developer),
        "bob's role changed by an escalation",
    )?;
    // Below its rank it may.
    send(&mut dave, c::set_role_command("bob", Role::Viewer))
        .map_err(|r| format!("demote: {r}"))?;
    // Reduced mid-session: bob's very next command feels it.
    check(
        set(&mut bob, lamp, "x", 2.0).is_err(),
        "a demoted Developer still edited",
    )?;
    send(&mut ada, c::set_role_command("bob", Role::Developer))
        .map_err(|r| format!("restore: {r}"))?;
    set(&mut bob, lamp, "x", 3.0).map_err(|r| format!("a Developer's edit: {r}"))?;
    // Team commands are human-only.
    let mut auto = EditorCore::connect(
        &ada.core,
        Issuer::Automation {
            session: "auto-1".into(),
            tool: "apply".into(),
        },
    );
    auto.apply(c::invite_command(None, Role::Owner, &[]), None);
    let refused = auto.pump().refused;
    check(
        refused
            .first()
            .is_some_and(|r| r.rejection.code().as_str() == "CMD-0014"),
        "an automation session sent a team command",
    )?;
    // Audited, with who did it.
    let book = EditorCore::audit_book(&dave.core);
    let lines = book.query(&AuditQuery::parse("origin:team user:dave"));
    check(
        lines
            .iter()
            .any(|r| r.event == "set_role" && r.detail.contains("Viewer")),
        "the role change is not in the audit",
    )?;
    Ok(())
}

#[test]
fn test_role_enforcement() {
    roles_hold(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_unguarded_core_ignores_roles() {
    let r = roles_hold(CollabFaults {
        no_guard: true,
        ..CollabFaults::default()
    });
    let e = r.err().unwrap_or_default();
    assert!(
        e.contains("a Viewer edited"),
        "the role check must fail on the Viewer: {e}"
    );
}

// ---- claims -------------------------------------------------------------------------------

fn claims_hold(faults: CollabFaults) -> Result<(), String> {
    let s = server();
    let mut ada = editor(&s, "ada");
    let level = spawn(&mut ada, "Level", None);
    let krate = spawn(&mut ada, "Crate", Some(level));
    let rock = spawn(&mut ada, "Rock", None);
    send(&mut ada, c::create_command("Studio")).map_err(|r| format!("create: {r}"))?;
    let mut bob = join(&s, &mut ada, "bob", Role::Developer);
    let mut carol = join(&s, &mut ada, "carol", Role::Viewer);
    EditorCore::set_collab_faults(&ada.core, faults);
    // Ada edits the crate before anyone claims it (her undo comes later).
    set(&mut ada, krate, "mass", 1.0).map_err(|r| format!("early edit: {r}"))?;
    let early = ada.ui.undo_target().ok_or("no undo target")?;
    send(&mut bob, c::claim_command(level)).map_err(|r| format!("claim: {r}"))?;
    check(
        s.server
            .claims()
            .iter()
            .any(|c| c.holder == "bob" && c.entity == level.0),
        "the claim is not held",
    )?;
    // Everything touching bob's subtree is refused in ada's sandbox.
    check(
        set(&mut ada, krate, "mass", 2.0).is_err(),
        "an edit inside a claim",
    )?;
    check(
        send(
            &mut ada,
            EditorCommand::Spawn {
                name: "Barrel".into(),
                parent: Some(level),
            },
        )
        .is_err(),
        "a spawn under a claim",
    )?;
    check(
        send(
            &mut ada,
            EditorCommand::Reparent {
                entity: rock,
                parent: Some(krate),
            },
        )
        .is_err(),
        "a move into a claim",
    )?;
    let mut auto = EditorCore::connect(
        &ada.core,
        Issuer::Automation {
            session: "auto-1".into(),
            tool: "apply".into(),
        },
    );
    auto.apply(
        EditorCommand::Rename {
            entity: krate,
            name: "Box".into(),
        },
        None,
    );
    check(
        !auto.pump().refused.is_empty(),
        "an automation session's edit inside a claim",
    )?;
    ada.ui.undo(early);
    check(!ada.ui.pump().refused.is_empty(), "an undo inside a claim")?;
    check(
        prop(&ada, krate, "mass") == Some(Value::Float(1.0)),
        "the claimed crate changed",
    )?;
    // Outside the claim, and for its holder, edits go on.
    set(&mut ada, rock, "mass", 5.0).map_err(|r| format!("outside a claim: {r}"))?;
    set(&mut bob, krate, "mass", 9.0).map_err(|r| format!("the holder: {r}"))?;
    // A Viewer cannot claim; someone else cannot take or release the claim.
    check(
        send(&mut carol, c::claim_command(rock)).is_err(),
        "a Viewer claimed",
    )?;
    check(
        send(&mut ada, c::claim_command(krate)).is_err(),
        "an overlapping claim",
    )?;
    // An Owner may release anyone's claim (Maintainers manage claims); then ada edits.
    send(&mut ada, c::release_command(level)).map_err(|r| format!("release: {r}"))?;
    set(&mut ada, krate, "mass", 3.0).map_err(|r| format!("after release: {r}"))?;
    Ok(())
}

#[test]
fn test_ownership_claims() {
    claims_hold(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_unguarded_core_edits_inside_a_claim() {
    let r = claims_hold(CollabFaults {
        no_guard: true,
        ..CollabFaults::default()
    });
    let e = r.err().unwrap_or_default();
    assert!(
        e.contains("an edit inside a claim"),
        "the claim check must fail: {e}"
    );
}

// ---- Live / Pull --------------------------------------------------------------------------

fn live_and_pull_agree(faults: CollabFaults) -> Result<(), String> {
    let s = server();
    let mut ada = editor(&s, "ada");
    let lamp = spawn(&mut ada, "Lamp", None);
    send(&mut ada, c::create_command("Studio")).map_err(|r| format!("create: {r}"))?;
    let mut bob = join(&s, &mut ada, "bob", Role::Developer);
    let mut carol = join(&s, &mut ada, "carol", Role::Developer);
    for e in [&bob, &carol] {
        EditorCore::set_collab_faults(&e.core, faults);
    }
    s.server.set_policy("carol", Policy::Live);
    check(
        s.server.policy("bob") == Policy::Pull,
        "Pull is the default",
    )?;
    let mut first_tree = None;
    for step in 0..3 {
        match step {
            0 => first_tree = Some(spawn(&mut ada, "Tree", None)),
            1 => set(&mut ada, lamp, "colour", 2.0)?,
            _ => send(
                &mut ada,
                EditorCommand::Rename {
                    entity: first_tree.ok_or("no tree")?,
                    name: "Oak".into(),
                },
            )?,
        }
        send(&mut ada, c::publish_command(&format!("step {step}")))
            .map_err(|r| format!("publish {step}: {r}"))?;
        // Live: carol's next pump brings it in.
        let _ = carol.ui.pump();
        check(
            sorted_doc(&carol.core) == head_doc(&s),
            "a Live sandbox did not follow the publish as it landed",
        )?;
    }
    // Pull: nothing moved until bob asks.
    check(
        EditorCore::read(&bob.core, |p| p.len()) == 1,
        "a Pull sandbox moved on its own",
    )?;
    let incoming = s.server.revisions(
        EditorCore::collab_status(&bob.core).and_then(|x| x.base),
        100,
    );
    check(
        incoming.len() == 3,
        "the Pull sandbox does not see 3 incoming",
    )?;
    send(&mut bob, c::pull_command(None)).map_err(|r| format!("pull: {r}"))?;
    let (b, l) = (sorted_doc(&bob.core), sorted_doc(&carol.core));
    check(b == l, "Live and Pull ended in different states")?;
    check(
        b == head_doc(&s),
        "the sandboxes do not hold the baseline's keys",
    )?;
    check(
        EditorCore::read(&bob.core, |p| p.state_hash())
            == EditorCore::read(&carol.core, |p| p.state_hash()),
        "the state hashes differ",
    )?;
    // The switch is one call either way and changes nothing by itself.
    let before = sorted_doc(&carol.core);
    s.server.set_policy("carol", Policy::Pull);
    s.server.set_policy("carol", Policy::Live);
    let _ = carol.ui.pump();
    check(
        sorted_doc(&carol.core) == before,
        "toggling the policy changed the state",
    )?;
    Ok(())
}

#[test]
fn test_live_pull() {
    live_and_pull_agree(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_pulls_that_reload_the_project_lose_the_shared_keys() {
    let r = live_and_pull_agree(CollabFaults {
        pull_by_load: true,
        ..CollabFaults::default()
    });
    let e = r.err().unwrap_or_default();
    assert!(
        e.contains("did not follow"),
        "a pull by reload must fail the check: {e}"
    );
}

// ---- conflicts ----------------------------------------------------------------------------

fn conflicts_surface(faults: CollabFaults) -> Result<(), String> {
    let s = server();
    let mut ada = editor(&s, "ada");
    let lamp = spawn(&mut ada, "Lamp", None);
    set(&mut ada, lamp, "colour", 1.0)?;
    send(&mut ada, c::create_command("Studio")).map_err(|r| format!("create: {r}"))?;
    let mut bob = join(&s, &mut ada, "bob", Role::Developer);
    EditorCore::set_collab_faults(&bob.core, faults);
    set(&mut ada, lamp, "colour", 2.0)?;
    set(&mut ada, lamp, "size", 4.0)?;
    send(&mut ada, c::publish_command("red")).map_err(|r| format!("publish: {r}"))?;
    set(&mut bob, lamp, "colour", 3.0)?;
    // The pull meets bob's edit of the same property: it stops, nothing of it applied.
    let pulled = send(&mut bob, c::pull_command(None));
    check(pulled.is_err(), "the pull did not stop at the conflict")?;
    check(
        pulled
            .as_ref()
            .err()
            .is_some_and(|m| m.contains("PROJECT-0018")),
        "the stop does not say why",
    )?;
    check(
        prop(&bob, lamp, "colour") == Some(Value::Float(3.0)),
        "my edit was lost",
    )?;
    check(
        prop(&bob, lamp, "size").is_none(),
        "half a pull was applied",
    )?;
    let st = EditorCore::collab_status(&bob.core).ok_or("no status")?;
    let pending = st.pending.as_ref().ok_or("the conflict was not kept")?;
    let conflict = pending
        .conflicts
        .iter()
        .find(|x| x.label.contains("colour"))
        .ok_or("the conflict is not listed")?;
    check(
        (
            conflict.base.clone(),
            conflict.mine.clone(),
            conflict.theirs.clone(),
        ) == (
            Some(Value::Float(1.0)),
            Some(Value::Float(3.0)),
            Some(Value::Float(2.0)),
        ),
        "the three values are wrong",
    )?;
    check(
        send(&mut bob, c::publish_command("mine")).is_err(),
        "published over an unresolved conflict",
    )?;
    // Choosing a side applies the pull: mine for the colour, theirs for the rest.
    let choices = [(conflict.subject.key(), Side::Mine)].into_iter().collect();
    send(&mut bob, c::resolve_command(&choices)).map_err(|r| format!("resolve: {r}"))?;
    check(
        prop(&bob, lamp, "colour") == Some(Value::Float(3.0))
            && prop(&bob, lamp, "size") == Some(Value::Float(4.0)),
        "the resolution did not apply",
    )?;
    send(&mut bob, c::publish_command("blue")).map_err(|r| format!("publish mine: {r}"))?;
    send(&mut ada, c::pull_command(None)).map_err(|r| format!("ada pulls: {r}"))?;
    check(
        prop(&ada, lamp, "colour") == Some(Value::Float(3.0)),
        "the resolution did not reach the team",
    )?;
    Ok(())
}

#[test]
fn test_rebase_conflicts() {
    conflicts_surface(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_pull_that_swallows_conflicts_fails() {
    let r = conflicts_surface(CollabFaults {
        swallow_conflicts: true,
        ..CollabFaults::default()
    });
    let e = r.err().unwrap_or_default();
    assert!(
        e.contains("did not stop"),
        "a swallowed conflict must fail the check: {e}"
    );
}

// ---- review queue and scopes --------------------------------------------------------------

#[test]
fn test_review_queue_and_scopes() {
    let s = server();
    let mut ada = editor(&s, "ada");
    let levels = spawn(&mut ada, "Levels", None);
    let sandbox = spawn(&mut ada, "Sandbox", None);
    send(&mut ada, c::create_command("Studio")).unwrap_or_else(|r| panic!("{r}"));
    let mut rita = join(&s, &mut ada, "rita", Role::Reviewer);
    // Bob may publish only under Levels and Sandbox.
    let scopes = vec![
        "scene/Levels/**".to_string(),
        "scene/Sandbox/**".to_string(),
    ];
    send(
        &mut ada,
        c::invite_command(Some("bob@studio.example"), Role::Developer, &scopes),
    )
    .unwrap_or_else(|r| panic!("{r}"));
    let mut bob = editor(&s, "bob");
    let invite = s
        .identity
        .invites_for("bob")
        .first()
        .map(|(_, _, i)| i.id.clone())
        .unwrap_or_else(|| panic!("the email invite is not waiting for bob"));
    send(&mut bob, c::accept_command(&invite)).unwrap_or_else(|r| panic!("{r}"));
    // Review required, except under Sandbox (O-16's per-path override).
    send(
        &mut ada,
        c::review_policy_command(true, &[("scene/Sandbox/**".into(), false)]),
    )
    .unwrap_or_else(|r| panic!("{r}"));
    // The rules are the baseline's: they apply to the team once published.
    send(&mut ada, c::publish_command("review rules")).unwrap_or_else(|r| panic!("{r}"));
    let _ = bob.ui.pump();
    send(&mut bob, c::pull_command(None)).unwrap_or_else(|r| panic!("{r}"));
    // Outside his scope: refused, naming the path.
    send(
        &mut bob,
        EditorCommand::SetSetting {
            key: "editor.grid_size".into(),
            value: Some(Value::Float(2.0)),
        },
    )
    .unwrap_or_else(|r| panic!("{r}"));
    let out = send(&mut bob, c::publish_command("grid"));
    assert!(
        out.as_ref()
            .err()
            .is_some_and(|m| m.contains("PROJECT-0014") && m.contains("settings/editor/grid_size")),
        "{out:?}"
    );
    send(
        &mut bob,
        EditorCommand::SetSetting {
            key: "editor.grid_size".into(),
            value: None,
        },
    )
    .unwrap_or_else(|r| panic!("{r}"));
    // Under Levels: gated.
    let head = s.server.head();
    let tree = spawn(&mut bob, "Tree", Some(levels));
    send(&mut bob, c::publish_command("a tree")).unwrap_or_else(|r| panic!("{r}"));
    assert_eq!(s.server.head(), head, "a gated publish waits for review");
    let req = s
        .server
        .requests()
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("no request"));
    assert_eq!(req.author, "bob");
    assert!(
        send(&mut bob, c::approve_command(req.id)).is_err(),
        "self-approval"
    );
    send(&mut rita, c::approve_command(req.id)).unwrap_or_else(|r| panic!("{r}"));
    assert_ne!(s.server.head(), head, "an approval publishes");
    send(&mut bob, c::pull_command(None)).unwrap_or_else(|r| panic!("{r}"));
    let st = EditorCore::collab_status(&bob.core).unwrap_or_else(|| panic!("status"));
    assert_eq!(st.unpublished, 0, "approved work is no longer unpublished");
    assert!(EditorCore::read(&ada.core, |p| !p.contains(tree)));
    // Under Sandbox: straight to the baseline.
    let head = s.server.head();
    spawn(&mut bob, "Scratch", Some(sandbox));
    send(&mut bob, c::publish_command("scratch")).unwrap_or_else(|r| panic!("{r}"));
    assert_ne!(s.server.head(), head, "the override publishes directly");
    // A reject carries its reason.
    spawn(&mut bob, "Wall", Some(levels));
    send(&mut bob, c::publish_command("a wall")).unwrap_or_else(|r| panic!("{r}"));
    let req = s
        .server
        .requests()
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("no request"));
    send(&mut ada, c::reject_command(req.id, "not in this level"))
        .unwrap_or_else(|r| panic!("{r}"));
    let state = s
        .server
        .requests()
        .last()
        .map(|r| r.state.label())
        .unwrap_or_default();
    assert!(state.contains("not in this level"), "{state}");
}

// ---- licence lapse ------------------------------------------------------------------------

/// The latest lifecycle outcome of `op` in `core`'s status: `Ok(message)` or the error.
fn lifecycle(e: &mut Editor, cmd: EditorCommand, op: &str) -> Result<String, String> {
    e.ui.apply(cmd, None);
    let p = e.ui.pump();
    if let Some(r) = p.refused.first() {
        return Err(format!("refused: {}", r.rejection.error));
    }
    EditorCore::wait_transfers(&e.core);
    let st = EditorCore::project_status(&e.core);
    st.outcomes
        .iter()
        .rev()
        .find(|o| o.op == op)
        .map(|o| o.result.clone().map_err(|(c, m)| format!("{c}: {m}")))
        .unwrap_or_else(|| Err(format!("no {op} outcome")))
}

fn lapse_never_locks(faults: CollabFaults) -> Result<(), String> {
    use forge_editor::project::{ProjectOp, Template};
    let s = server();
    let mut ada = editor(&s, "ada");
    let loc = format!("memory:lapse-{}-{}", std::process::id(), faults.lapse_locks);
    lifecycle(
        &mut ada,
        ProjectOp::Create {
            location: loc.clone(),
            name: "Lapsed".into(),
            template: Template::ThreeD,
            discard_unsaved: true,
        }
        .command(),
        forge_editor::project::CREATE_CMD,
    )?;
    let lamp = spawn(&mut ada, "Lamp", None);
    send(&mut ada, c::create_command("Studio")).map_err(|r| format!("create: {r}"))?;
    EditorCore::set_collab_faults(&ada.core, faults);
    // The Team term ended long ago (past the grace period).
    s.licence.set(Some(Entitlement {
        tier: Tier::Team { over_100k: false },
        seat: SeatKind::Purchaser,
        bound: None,
        expires_ms: Some(NOW - 60 * DAY),
        fallback: None,
    }));
    // Editing, saving, reopening and building are untouched.
    set(&mut ada, lamp, "x", 1.0).map_err(|r| format!("an edit after lapse: {r}"))?;
    lifecycle(
        &mut ada,
        forge_editor::project::save_command("after lapse"),
        forge_editor::project::SAVE_CMD,
    )
    .map_err(|e| format!("a save after lapse: {e}"))?;
    lifecycle(
        &mut ada,
        ProjectOp::Open {
            location: loc,
            discard_unsaved: true,
        }
        .command(),
        forge_editor::project::OPEN_CMD,
    )
    .map_err(|e| format!("an open after lapse: {e}"))?;
    lifecycle(
        &mut ada,
        ProjectOp::Build {
            target: forge_project::packager::Target::Windows,
        }
        .command(),
        forge_editor::project::BUILD_CMD,
    )
    .map_err(|e| format!("a build after lapse: {e}"))?;
    let reopened = EditorCore::read(&ada.core, |p| {
        p.entities()
            .find(|(_, e)| e.name() == "Lamp")
            .and_then(|(_, e)| e.property("x").cloned())
    });
    check(
        reopened == Some(Value::Float(1.0)),
        "the saved edit did not reopen",
    )?;
    let lamp = EditorCore::read(&ada.core, |p| {
        p.entities()
            .find(|(_, e)| e.name() == "Lamp")
            .map(|(k, _)| k)
    })
    .ok_or("no lamp after reopening")?;
    set(&mut ada, lamp, "x", 2.0).map_err(|r| format!("an edit after reopening: {r}"))?;
    // Only collaboration is refused, with the reason.
    let publish = send(&mut ada, c::publish_command("x"));
    check(
        publish.as_ref().err().is_some_and(|m| m.contains("lapsed")),
        "a publish after lapse was not refused with the reason",
    )?;
    check(
        send(&mut ada, c::claim_command(lamp)).is_err(),
        "a claim after lapse",
    )?;
    // Renewing (opportunistic, never a precondition) brings collaboration back.
    use forge_editor::collab::licence::EntitlementSource;
    s.licence.renew();
    send(&mut ada, c::claim_command(lamp)).map_err(|r| format!("a claim after renewal: {r}"))?;
    Ok(())
}

#[test]
fn test_lapse_degrades_never_locks() {
    lapse_never_locks(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_a_lapse_that_locks_edits_fails() {
    let r = lapse_never_locks(CollabFaults {
        lapse_locks: true,
        ..CollabFaults::default()
    });
    let e = r.err().unwrap_or_default();
    assert!(
        e.contains("an edit after lapse"),
        "a lock-out on lapse must fail the check: {e}"
    );
}

// ---- an automation session's team pull (WP-21, ADR 0043 Amendment 2's follow-up)
// ------------------------
//
// `forge.collab.pull` is a performed command an automation session working in its person's sandbox
// may send (Ch.37 §37.8), but the pull's patch (`forge.collab.sync`, core-only) was human-only: an
// automation session's pull could not succeed at all. It now succeeds, and — like a non-human's
// load (WP-33) — what teammates published that widens plugin grants or the default automation
// policy waits for the person, while a restriction takes effect and is audited as `narrowed`.
// Control: a core that lets a non-human's load take effect like a person's
// (`CoreFaults::automation_load_unheld`) widens the grant on the session's pull.

fn automation_pull(unheld: bool) -> Result<(), String> {
    use forge_editor::security;
    use forge_plugin::{Capability, FsScope, PluginId, Principal};
    let read = Capability::Fs(FsScope::ProjectRead);
    let rivers = Principal::Plugin(PluginId::new("com.example.rivers").map_err(|e| e.to_string())?);
    let lakes = Principal::Plugin(PluginId::new("com.example.lakes").map_err(|e| e.to_string())?);
    let s = server();
    let mut ada = editor(&s, "ada");
    spawn(&mut ada, "Lamp", None);
    send(&mut ada, c::create_command("Studio")).map_err(|r| format!("create: {r}"))?;
    let mut bob = join(&s, &mut ada, "bob", Role::Developer);
    if unheld {
        EditorCore::set_faults(
            &bob.core,
            forge_editor::core::CoreFaults {
                automation_load_unheld: true,
                ..forge_editor::core::CoreFaults::default()
            },
        );
    }
    // ada grants lakes and publishes; bob (a person) pulls it: lakes holds in bob's editor.
    send(&mut ada, security::grant_command(&lakes, read)).map_err(|r| format!("grant: {r}"))?;
    send(&mut ada, c::publish_command("lakes may read")).map_err(|r| format!("publish: {r}"))?;
    send(&mut bob, c::pull_command(None)).map_err(|r| format!("bob's pull: {r}"))?;
    check(
        EditorCore::grants(&bob.core).has(&lakes, read),
        "a person's pull did not bring the teammate's grant",
    )?;
    // ada now grants rivers, revokes lakes, adds a tree, and publishes.
    send(&mut ada, security::grant_command(&rivers, read)).map_err(|r| format!("grant: {r}"))?;
    send(&mut ada, security::revoke_command(&lakes, read)).map_err(|r| format!("revoke: {r}"))?;
    spawn(&mut ada, "Tree", None);
    send(&mut ada, c::publish_command("rivers, not lakes")).map_err(|r| format!("publish: {r}"))?;
    // bob's automation session pulls: it succeeds and brings the tree...
    let mut auto = EditorCore::connect(
        &bob.core,
        Issuer::Automation {
            session: "auto-1".into(),
            tool: "apply".into(),
        },
    );
    auto.apply(c::pull_command(None), None);
    if let Some(r) = auto.pump().refused.first() {
        return Err(format!("the session's pull was refused: {}", r.rejection));
    }
    let st = EditorCore::collab_status(&bob.core).ok_or("no collab status")?;
    match st.outcomes.last() {
        Some(o) if o.ok => {}
        Some(o) => return Err(format!("the session's pull failed: {}", o.message)),
        None => return Err("the session's pull reported nothing".into()),
    }
    check(
        sorted_doc(&bob.core)
            .entities
            .iter()
            .any(|e| e.name == "Tree"),
        "the session's pull did not bring the teammate's tree",
    )?;
    // ...the widening waits for bob, and the restriction took effect, audited.
    check(
        !EditorCore::grants(&bob.core).has(&rivers, read),
        "an automation session's pull widened a plugin grant without a person",
    )?;
    let held =
        EditorCore::held_security(&bob.core).ok_or("nothing is held after the session's pull")?;
    let rivers_key = security::grant_key(&rivers, read).ok_or("no grant key")?;
    check(
        held.items.iter().any(|i| i.key == rivers_key),
        "the teammate's grant is not the held proposal",
    )?;
    check(
        !EditorCore::grants(&bob.core).has(&lakes, read),
        "the revoke the session pulled did not take effect",
    )?;
    let events: Vec<(String, String)> = EditorCore::audit_book(&bob.core)
        .query(&AuditQuery::parse(""))
        .into_iter()
        .map(|r| (r.event, r.detail))
        .collect();
    check(
        events
            .iter()
            .any(|(e, d)| e == "narrowed" && d.contains("com.example.lakes")),
        "the grant the session's pull dropped is not audited as narrowed",
    )?;
    check(
        events.iter().any(|(e, _)| e == "held"),
        "the held grant is not audited",
    )?;
    // bob accepts: the grant takes effect.
    send(&mut bob, security::accept_held_command(&held)).map_err(|r| format!("accept: {r}"))?;
    check(
        EditorCore::grants(&bob.core).has(&rivers, read),
        "bob's accept did not grant the plugin",
    )
}

#[test]
fn test_automation_team_pull() {
    automation_pull(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_unheld_automation_pull_widens_a_grant() {
    let e = automation_pull(true).expect_err("an unheld automation pull must fail the check");
    assert!(e.contains("widened a plugin grant"), "{e}");
}

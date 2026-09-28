//! `test_collab_pump_alloc` (Ch.37; WP-U14, owner rule 2, D-5): **following the team costs
//! no allocation per pump.**
//!
//! Every client pump follows the baseline (`EditorCore::collab_follow`, what `LocalBus::pump`
//! runs first): is this sandbox still in its team, should Live pull, is the sandbox row due?
//! The first question used to clone the whole team record (every member, every invite) on
//! every pump — work that grows with the team, per frame, to answer a question whose answer
//! changes only when the identity database, the server or the project's team setting does.
//!
//! The check: in a team of 60 people, 200 idle follows make **zero** heap allocations —
//! measured with a counting allocator — and the sandbox still knows it is joined; after a
//! member is removed (the identity database moved) the next follow re-reads the team once
//! and the idle follows after it are zero again.
//!
//! Positive control (W2): `positive_control_an_uncached_membership_check_allocates` — the
//! membership recomputed on every pump (`CollabFaults::joined_uncached`, the code as found)
//! allocates on every follow, growing with the team, and the check fails.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use forge_cmd::{EditorCommand, Issuer};
use forge_editor::client::BusClient;
use forge_editor::collab::licence::MemoryEntitlement;
use forge_editor::collab::{self as c, CollabFaults};
use forge_editor::core::{CollabAttach, EditorCore, SharedCore};
use forge_project::collab::{
    CollabView, IdentityBackend, InviteSpec, InviteTo, MemoryCollab, MemoryIdentity, Role,
};

const NOW: u64 = 20_000 * 86_400_000;
const TEAM: usize = 60;
const PUMPS: u64 = 200;

struct Rig {
    core: SharedCore,
    identity: Arc<MemoryIdentity>,
    server: Arc<MemoryCollab>,
}

fn rig(faults: CollabFaults) -> Result<Rig, String> {
    static N: AtomicU64 = AtomicU64::new(0);
    let identity = Arc::new(MemoryIdentity::new());
    let name = format!(
        "collab-pump-alloc-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let store = forge_project::memory::open_named(&name, "forge-server");
    let server = Arc::new(MemoryCollab::new(store, identity.clone()));
    let licence = Arc::new(MemoryEntitlement::team_for_a_year(NOW));
    licence.set_now(Some(NOW));
    let core = EditorCore::new();
    EditorCore::attach_collab(
        &core,
        CollabAttach {
            identity: identity.clone(),
            server: server.clone(),
            licence,
            owner: "ada".into(),
            email: Some("ada@studio.example".into()),
        },
    );
    EditorCore::set_collab_faults(&core, faults);
    let mut ui = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    ui.apply(
        EditorCommand::Spawn {
            name: "Lamp".into(),
            parent: None,
        },
        None,
    );
    ui.apply(c::create_command("Studio"), None);
    if let Some(r) = ui.pump().refused.first() {
        return Err(format!("set-up refused: {}", r.rejection.error));
    }
    let team = server.team().ok_or("no team")?;
    for i in 0..TEAM {
        let user = format!("member{i:02}");
        identity.ensure_account(&user, Some(&format!("{user}@studio.example")));
        let inv = identity
            .invite(
                &team,
                InviteSpec {
                    email: None,
                    role: Role::Developer,
                    paths: vec![format!("scene/Area{i}/**")],
                },
                "ada",
                NOW,
            )
            .map_err(|e| e.to_string())?;
        let InviteTo::Code(code) = inv.to else {
            return Err("no join code minted".into());
        };
        identity
            .accept(&user, &code, NOW)
            .map_err(|e| e.to_string())?;
    }
    // Settle: the row for the new team, the first membership answer.
    for _ in 0..3 {
        EditorCore::collab_follow(&core);
    }
    Ok(Rig {
        core,
        identity,
        server,
    })
}

/// Heap allocations made by `PUMPS` idle follows.
fn idle_follows(r: &Rig) -> u64 {
    allocation_counter::measure(|| {
        for _ in 0..PUMPS {
            EditorCore::collab_follow(&r.core);
        }
    })
    .count_total
}

fn joined(r: &Rig) -> bool {
    EditorCore::collab_status(&r.core).is_some_and(|s| s.joined)
}

fn check(faults: CollabFaults) -> Result<(), String> {
    let r = rig(faults)?;
    if !joined(&r) {
        return Err("the sandbox does not follow its team".into());
    }
    let n = idle_follows(&r);
    if n != 0 {
        return Err(format!(
            "{PUMPS} idle follows in a team of {} made {n} heap allocations ({:.1} per pump)",
            TEAM + 1,
            n as f64 / PUMPS as f64
        ));
    }
    // The identity database moves: the next follow re-reads the team, then zero again.
    let team = r.server.team().ok_or("no team")?;
    r.identity
        .remove_member(&team, "member00")
        .map_err(|e| e.to_string())?;
    EditorCore::collab_follow(&r.core);
    if !joined(&r) {
        return Err("ada is no longer joined after someone else left".into());
    }
    let n = idle_follows(&r);
    if n != 0 {
        return Err(format!(
            "after a membership change, {PUMPS} idle follows made {n} heap allocations"
        ));
    }
    // Ada herself removed (by another Owner's hand, directly on the database): the cached
    // answer must not survive the change.
    r.identity
        .set_role(&team, "member01", Role::Owner)
        .map_err(|e| e.to_string())?;
    r.identity
        .remove_member(&team, "ada")
        .map_err(|e| e.to_string())?;
    EditorCore::collab_follow(&r.core);
    if joined(&r) {
        return Err("a removed member's sandbox still reads as joined".into());
    }
    Ok(())
}

#[test]
fn following_the_team_allocates_nothing_per_pump() {
    check(CollabFaults::default()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn positive_control_an_uncached_membership_check_allocates() {
    let e = check(CollabFaults {
        joined_uncached: true,
        ..CollabFaults::default()
    })
    .expect_err("recomputing membership every pump must fail");
    assert!(e.contains("heap allocations"), "{e}");
    eprintln!("control (the code as found): {e}");
}

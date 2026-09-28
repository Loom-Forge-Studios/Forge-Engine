//! `test_licence_backend` (WP-47, gate `C-licence-backend`; Ch.38 §38.2, E-57, I21): **the
//! editor's licence is the signed entitlement file, verified offline by `forge-licence`** — the
//! backend the labelled in-memory stand-in held the place of (D-4).
//!
//! * The committed `forge-licence` fixtures, signed with the dev key: a Team file gives the Team
//!   entitlement and collaboration; an Individual file gives no collaboration; a lapsed Team
//!   file is lapsed (collaboration paused); no file is no entitlement; a forged file (a Team
//!   entitlement re-signed by another key) is **no entitlement**, with the reason kept.
//! * Through the editor as the binary wires it (`wiring::attach_team_server_with`, the
//!   collaboration services): the licence the panels read is the file's, a forged file greys
//!   collaboration — creating a team is refused — and ordinary edits still apply (E-57: degrade,
//!   never lock).
//! * Renewal is offline: a new file on disk is read by `renew`, observers hear of it.
//! * No path makes a network call (`network_calls` stays 0; I21's spirit in the editor).
//!
//! Positive control (W2): `positive_control_the_in_memory_stand_in_fails_the_backend_check` —
//! the same check run over the in-memory stand-in (which reports a Team entitlement whatever the
//! file holds: an Individual file and a forged one alike) fails, so the check cannot pass on a
//! backend that does not read and verify the file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use forge_editor::collab::licence::{
    EntitlementSource, MemoryEntitlement, SeatKind, Tier, evaluate,
};
use forge_editor_bin::licence::{Rejected, SignedFileEntitlement};

const VERSION: &str = "0.1.0";

fn fixture(name: &str) -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "crates",
        "forge-licence",
        "tests",
        "fixtures",
        name,
    ]
    .iter()
    .collect()
}

/// Now, on the machine's clock (the licence is evaluated at now, as the editor does).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The real backend over the fixture `name`, verified against the dev key the fixtures are
/// signed with.
fn signed(name: &str) -> Arc<dyn EntitlementSource> {
    Arc::new(SignedFileEntitlement::at(
        Some(fixture(name)),
        forge_licence::DEV_PUBLIC_KEY,
    ))
}

/// The backend check (see the module docs): `make` builds a source over a file; `Err` names the
/// first file whose licence is wrong.
fn check(make: &dyn Fn(&Path) -> Arc<dyn EntitlementSource>) -> Result<(), String> {
    let now = now_ms();
    let state = |name: &str| {
        let s = make(&fixture(name));
        (evaluate(s.entitlement().as_ref(), now, VERSION, None), s)
    };
    let (team, s) = state("dev_team.json");
    if !team.collaboration {
        return Err(format!(
            "dev_team.json: no collaboration: {:?}",
            team.why_not
        ));
    }
    if s.network_calls() != 0 {
        return Err("reading the licence made a network call".into());
    }
    let (individual, _) = state("dev_individual.json");
    if individual.collaboration
        || individual.entitlement.as_ref().map(|e| e.tier) != Some(Tier::Individual)
    {
        return Err(format!("dev_individual.json: {individual:?}"));
    }
    let (lapsed, _) = state("dev_lapsed.json");
    if !lapsed.lapsed || lapsed.collaboration {
        return Err(format!("dev_lapsed.json is not lapsed: {lapsed:?}"));
    }
    let (none, _) = state("no-such-entitlement.json");
    if none.entitlement.is_some() {
        return Err(format!("no file, yet an entitlement: {none:?}"));
    }
    let (forged, _) = state("forged_team.json");
    if forged.entitlement.is_some() || forged.collaboration {
        return Err(format!(
            "forged_team.json (re-signed by another key) was accepted: {:?}",
            forged.entitlement
        ));
    }
    Ok(())
}

#[test]
fn the_licence_is_the_signed_file_verified_offline() {
    check(&|p| {
        Arc::new(SignedFileEntitlement::at(
            Some(p.to_path_buf()),
            forge_licence::DEV_PUBLIC_KEY,
        ))
    })
    .unwrap_or_else(|e| panic!("{e}"));
    // The file's fields reach the editor's model as they are.
    let e = signed("dev_team.json")
        .entitlement()
        .unwrap_or_else(|| panic!("dev_team.json verifies"));
    assert_eq!(
        (e.tier, e.seat, e.bound.clone(), e.expires_ms),
        (
            Tier::Team { over_100k: false },
            SeatKind::Purchaser,
            None,
            Some(4_102_444_800_000)
        )
    );
    // A forged file's reason is kept for the panel: its signature did not verify.
    let forged = SignedFileEntitlement::at(
        Some(fixture("forged_team.json")),
        forge_licence::DEV_PUBLIC_KEY,
    );
    assert_eq!(
        forged.rejected(),
        Some(Rejected::Unverified(
            forge_licence::VerifyError::BadSignature
        ))
    );
    // No file is not a rejection (an un-activated machine), nor is no path at all.
    let none = SignedFileEntitlement::at(
        Some(fixture("no-such-entitlement.json")),
        forge_licence::DEV_PUBLIC_KEY,
    );
    assert_eq!(none.rejected(), None);
    assert!(
        SignedFileEntitlement::at(None, forge_licence::DEV_PUBLIC_KEY)
            .entitlement()
            .is_none()
    );
}

/// The editor as the binary wires it over `licence`: its collaboration services, and a human
/// client of its core.
fn editor(
    licence: Arc<dyn EntitlementSource>,
    tag: &str,
) -> (
    forge_editor::core::SharedCore,
    forge_editor::collab::CollabServices,
) {
    let core = forge_editor::core::EditorCore::new();
    let baseline = format!("licence-backend-{tag}-{}", std::process::id());
    forge_editor_bin::wiring::attach_team_server_with(&core, "tester".into(), &baseline, licence);
    let mut collab = forge_editor::collab::CollabServices::default();
    collab.attach(&core);
    (core, collab)
}

#[test]
fn a_forged_file_greys_collaboration_and_never_locks_the_editor() {
    use forge_cmd::{EditorCommand, Issuer};
    use forge_editor::client::BusClient;
    let (team_core, team) = editor(signed("dev_team.json"), "team");
    assert!(
        team.licence_state().collaboration,
        "{:?}",
        team.licence_state()
    );
    let (core, collab) = editor(signed("forged_team.json"), "forged");
    let state = collab.licence_state();
    assert!(
        !state.collaboration && state.entitlement.is_none(),
        "{state:?}"
    );
    let mut me = forge_editor::core::EditorCore::connect(
        &core,
        Issuer::Human {
            user: "tester".into(),
        },
    );
    // Collaboration is refused...
    me.apply(forge_editor::collab::create_command("team_01"), None);
    let _ = me.pump();
    let has_team = |c: &forge_editor::core::SharedCore| {
        forge_editor::core::EditorCore::collab_status(c).is_some_and(|s| s.team.is_some())
    };
    assert!(!has_team(&core), "a forged licence created a team");
    // ...and everything else works (E-57).
    me.apply(
        EditorCommand::Spawn {
            name: "crate_01".into(),
            parent: None,
        },
        None,
    );
    let _ = me.pump();
    let spawned = forge_editor::core::EditorCore::read(&core, |p| {
        p.entities().any(|(_, e)| e.name() == "crate_01")
    });
    assert!(
        spawned,
        "an ordinary edit was refused under a forged licence"
    );
    // The Team file's editor creates the team.
    let mut them = forge_editor::core::EditorCore::connect(
        &team_core,
        Issuer::Human {
            user: "tester".into(),
        },
    );
    them.apply(forge_editor::collab::create_command("team_01"), None);
    let _ = them.pump();
    assert!(
        has_team(&team_core),
        "a Team licence could not create a team"
    );
}

#[test]
fn renewal_reads_the_new_file_offline() {
    let dir = std::env::temp_dir().join(format!(
        "forge-licence-backend-renew-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("entitlement.json");
    std::fs::copy(fixture("dev_individual.json"), &path).unwrap_or_else(|e| panic!("{e}"));
    let src = SignedFileEntitlement::at(Some(path.clone()), forge_licence::DEV_PUBLIC_KEY);
    let heard = Arc::new(AtomicUsize::new(0));
    let h = heard.clone();
    src.observe(Arc::new(move || {
        h.fetch_add(1, Ordering::AcqRel);
    }));
    assert_eq!(src.entitlement().map(|e| e.tier), Some(Tier::Individual));
    let g = src.generation();
    // Activation wrote a Team file: renewing reads it.
    std::fs::copy(fixture("dev_team.json"), &path).unwrap_or_else(|e| panic!("{e}"));
    let said = src.renew();
    assert!(!said.is_empty());
    assert_eq!(
        src.entitlement().map(|e| e.tier),
        Some(Tier::Team { over_100k: false })
    );
    assert!(src.generation() > g, "the generation did not move");
    assert_eq!(
        heard.load(Ordering::Acquire),
        1,
        "the observer was not told"
    );
    // Nothing changed: nothing is told.
    let _ = src.renew();
    assert_eq!(heard.load(Ordering::Acquire), 1);
    assert_eq!(src.network_calls(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn positive_control_the_in_memory_stand_in_fails_the_backend_check() {
    let now = now_ms();
    let e = check(&|_| Arc::new(MemoryEntitlement::team_for_a_year(now)))
        .expect_err("the in-memory stand-in passed the backend check");
    assert!(
        e.contains("dev_individual.json") || e.contains("forged"),
        "{e}"
    );
    // The forged case alone, as the stand-in answers it: Team, whatever the file holds.
    let stand_in = MemoryEntitlement::team_for_a_year(now);
    assert!(
        evaluate(stand_in.entitlement().as_ref(), now, VERSION, None).collaboration,
        "the stand-in no longer reports a Team licence: the control proves nothing"
    );
}

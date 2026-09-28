//! `test_licence_panel` (Ch.38 §38.2, E-57, I21; DoD M2-62):
//!
//! * the panel shows tier, seat, bound team and expiry, **evaluated on this machine: no
//!   network call** anywhere in the status path (the source counts its network attempts);
//! * **on lapse: a banner and a Renew button, the collaboration panels grey (their actions
//!   go, a notice says why), nothing is locked** — an ordinary edit still applies;
//! * *Renew* (opportunistic, pressed by a person) brings collaboration back.

mod common;

use common::*;
use forge_cmd::{EditorCommand, Value};
use forge_editor::collab::licence::{Entitlement, EntitlementSource, SeatKind, Tier};
use forge_editor::core::EditorCore;
use forge_project::collab::CollabView;

const L: &str = "forge.licence";
const T: &str = "forge.team";

#[test]
fn lapse_shows_a_banner_greys_collaboration_and_locks_nothing() {
    let s = server();
    let mut rig = rig(&s, &[L, T]);
    rig.shell
        .emitter()
        .emit(forge_editor::collab::create_command("Studio"));
    rig.settle();
    feed(&mut rig);
    let details = label(&rig, at(&rig, L, &["details"]));
    assert!(details.starts_with("Tier: Team"), "{details}");
    assert!(details.contains("Seat: purchaser"), "{details}");
    assert!(
        hidden(&rig, at(&rig, L, &["banner_group"])),
        "no banner while valid"
    );
    let footer = label(&rig, at(&rig, L, &["footer"]));
    assert!(
        footer.contains("no network call") && footer.contains("in-memory stand-in"),
        "{footer}"
    );
    assert!(!hidden(&rig, at(&rig, T, &["members_group", "add_bar"])));
    // The Team term ended long ago.
    s.licence.set(Some(Entitlement {
        tier: Tier::Team { over_100k: false },
        seat: SeatKind::Purchaser,
        bound: None,
        expires_ms: Some(NOW - 60 * DAY),
        fallback: None,
    }));
    feed(&mut rig);
    let banner = label(&rig, at(&rig, L, &["banner_group", "banner"]));
    assert!(
        banner.contains("lapsed") && banner.contains("unaffected"),
        "{banner}"
    );
    assert!(!hidden(
        &rig,
        at(&rig, L, &["banner_group", "renew_bar", "renew"])
    ));
    // The team panel greys: a notice, and no management actions.
    let notice = label(&rig, at(&rig, T, &["notice"]));
    assert!(notice.contains("Collaboration is paused"), "{notice}");
    assert!(hidden(&rig, at(&rig, T, &["members_group", "add_bar"])));
    // Nothing is locked: an ordinary edit applies.
    let k = EditorCore::read(&rig.core, |p| p.next_key());
    rig.shell.emitter().emit(EditorCommand::Spawn {
        name: "Still editable".into(),
        parent: None,
    });
    rig.shell.emitter().emit(EditorCommand::SetProperty {
        entity: k,
        path: "x".into(),
        value: Value::Float(1.0),
    });
    rig.settle();
    assert_eq!(
        EditorCore::read(&rig.core, |p| p
            .entity(k)
            .and_then(|e| e.property("x"))
            .cloned()),
        Some(Value::Float(1.0))
    );
    // No network call in the status path, however often it refreshed.
    assert_eq!(s.licence.network_calls(), 0);
    tap(&mut rig, L, &["banner_group", "renew_bar", "renew"]);
    assert_eq!(
        s.licence.network_calls(),
        1,
        "only a person's Renew reaches out"
    );
    feed(&mut rig);
    assert!(hidden(&rig, at(&rig, L, &["banner_group"])), "renewed");
    assert!(!hidden(&rig, at(&rig, T, &["members_group", "add_bar"])));
}

#[test]
fn an_individual_licence_says_why_and_the_panels_offer_nothing() {
    let s = server();
    s.licence.set(Some(Entitlement {
        tier: Tier::Individual,
        seat: SeatKind::Purchaser,
        bound: None,
        expires_ms: None,
        fallback: None,
    }));
    let mut rig = rig(&s, &[L, T]);
    feed(&mut rig);
    let banner = label(&rig, at(&rig, L, &["banner_group", "banner"]));
    assert!(banner.contains("Individual"), "{banner}");
    assert!(
        hidden(&rig, at(&rig, T, &["create_group"])),
        "no team can be created"
    );
    rig.shell
        .emitter()
        .emit(forge_editor::collab::create_command("Studio"));
    rig.settle();
    let n = notices(&rig);
    assert!(
        n.iter()
            .any(|l| l.contains("CMD-0014") && l.contains("Individual")),
        "{n:?}"
    );
    assert!(s.server.team().is_none());
}

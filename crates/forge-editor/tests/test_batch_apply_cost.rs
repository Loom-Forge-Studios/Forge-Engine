//! Guard (WP-20, owner rule 2): **an automation session's small batch costs the same whatever the
//! size of the project** — no copy of the project, one planning pass, under the editor core's
//! lock.
//!
//! An automation session's `apply` is a batch of commands that is all or nothing. Before WP-20 the
//! core previewed every multi-command batch on a deep copy of the whole project
//! (`Bus::dry_run_batch`) and then planned it a second time to apply it (and a batch with `$name`
//! bindings was previewed a third time by the server): two O(project) copies and three planning
//! passes while holding the lock the GUI's frame needs, so an automation session's four-command
//! edit stalled the UI of a large project. Now the bus applies the batch in place inside the open
//! transaction and reverts it on a refusal (`Bus::apply_batch`), and a preview applies and reverts
//! in place (`Bus::preview_batch`).
//!
//! * `a_small_batch_allocates_the_same_on_a_large_project` — the same four-command batch
//!   (a spawn, two property sets on it, a rename), applied and previewed through the session's
//!   `LocalBus` on projects of 1,000 and 16,000 entities, measured with a counting allocator
//!   (thread-local): the larger project may not allocate more than [`ALLOWANCE`] bytes more.
//!   Deterministic: a byte count, not a clock.
//! * `apply_latency_does_not_grow_with_the_project` — the latency, measured alone
//!   (`forge_trace::timed`), median of [`RUNS`] batches on 1,000 / 16,000 / 64,000 entities;
//!   printed, and the 64k median may not exceed [`LATENCY_RATIO`] times the 1k one.
//!
//! Positive control (W2): `positive_control_copying_the_project_scales_with_it` — the
//! identical allocation measurement with the pre-WP-20 path switched back on
//! (`CoreFaults::batch_copies_project`) allocates in proportion to the project and fails it.

// A test harness: its helpers panic on a broken fixture by design (Ch.1.2 governs engine code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use forge_cmd::{EditorCommand, EntityKey, Issuer, TxnId, Value};
use forge_editor::client::BusClient;
use forge_editor::core::{CoreFaults, EditorCore, LocalBus, SharedCore};

/// Allowed growth of a batch's allocated bytes from 1,000 to 16,000 entities. A copy of the
/// larger project is megabytes; a B-tree one level deeper is nothing.
const ALLOWANCE: u64 = 4096;
/// Batches per timed median.
const RUNS: usize = 41;
/// The 64k-entity median over the 1k-entity one: a B-tree search two levels deeper, never a
/// copy (the copying path measured over 100x).
const LATENCY_RATIO: f64 = 3.0;

fn human() -> Issuer {
    Issuer::Human {
        user: "tester".into(),
    }
}

fn auto() -> Issuer {
    Issuer::Automation {
        session: "auto-1".into(),
        tool: "apply".into(),
    }
}

/// A core holding `n` entities (one committed transaction), and an automation session's client with
/// a preview transaction open.
fn project(n: usize, faults: CoreFaults) -> (SharedCore, LocalBus, TxnId) {
    let core = EditorCore::new();
    let mut h = EditorCore::connect(&core, human());
    let t = h.begin("populate");
    let cmds: Vec<EditorCommand> = (0..n)
        .map(|i| EditorCommand::Spawn {
            name: format!("Rock {i}"),
            parent: None,
        })
        .collect();
    h.apply_batch(cmds, Some(t), None).expect("populates");
    h.commit_now(t).expect("commits");
    drop(h);
    EditorCore::set_faults(&core, faults);
    let mut a = EditorCore::connect(&core, auto());
    let t = a.begin("automation work");
    (core, a, t)
}

/// The four-command batch: spawn, set two properties on the new entity, rename it.
fn batch(next: EntityKey, i: usize) -> Vec<EditorCommand> {
    vec![
        EditorCommand::Spawn {
            name: format!("Crate {i}"),
            parent: None,
        },
        EditorCommand::SetProperty {
            entity: next,
            path: "transform.scale".into(),
            value: Value::Float(2.0),
        },
        EditorCommand::SetProperty {
            entity: next,
            path: "light.power".into(),
            value: Value::Float(500.0),
        },
        EditorCommand::Rename {
            entity: next,
            name: format!("Big crate {i}"),
        },
    ]
}

fn next_key(core: &SharedCore) -> EntityKey {
    EditorCore::read(core, forge_cmd::Project::next_key)
}

/// Bytes allocated by one preview and one apply of the batch on a project of `n` entities
/// (after a warm-up batch that sizes every reused buffer).
fn bytes(n: usize, faults: CoreFaults) -> u64 {
    let (core, mut a, t) = project(n, faults);
    let k = next_key(&core);
    a.apply_batch(batch(k, 0), Some(t), None).expect("warm-up");
    let k = next_key(&core);
    let cmds = batch(k, 1);
    let preview = allocation_counter::measure(|| {
        a.preview_batch(&cmds).expect("previews");
    });
    let apply = allocation_counter::measure(|| {
        a.apply_batch(cmds, Some(t), None).expect("applies");
    });
    assert_eq!(EditorCore::read(&core, forge_cmd::Project::len), n + 2);
    preview.bytes_total + apply.bytes_total
}

fn check(faults: CoreFaults) -> Result<String, String> {
    let small = bytes(1_000, faults);
    let large = bytes(16_000, faults);
    let msg =
        format!("a 4-command batch (preview + apply): {small} B at 1k entities, {large} B at 16k");
    if large > small + ALLOWANCE {
        return Err(format!(
            "{msg}: grows with the project (allowance {ALLOWANCE} B)"
        ));
    }
    Ok(msg)
}

#[test]
fn a_small_batch_allocates_the_same_on_a_large_project() {
    let r = check(CoreFaults::default());
    println!("{r:?}");
    assert!(r.is_ok(), "{r:?}");
}

#[test]
fn positive_control_copying_the_project_scales_with_it() {
    let r = check(CoreFaults {
        batch_copies_project: true,
        ..CoreFaults::default()
    });
    println!("{r:?}");
    assert!(r.is_err(), "the copying path must fail the guard: {r:?}");
}

/// Median microseconds of one batch apply on a project of `n` entities.
fn median_us(n: usize, faults: CoreFaults) -> f64 {
    let (core, mut a, t) = project(n, faults);
    let mut v = Vec::with_capacity(RUNS);
    for i in 0..RUNS + 3 {
        let k = next_key(&core);
        let cmds = batch(k, i);
        let at = std::time::Instant::now();
        a.apply_batch(cmds, Some(t), None).expect("applies");
        if i >= 3 {
            v.push(at.elapsed().as_secs_f64() * 1e6);
        }
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
fn apply_latency_does_not_grow_with_the_project() {
    let r = forge_trace::timed::run_timed_alone(|| {
        let us: Vec<(usize, f64)> = [1_000, 16_000, 64_000]
            .into_iter()
            .map(|n| (n, median_us(n, CoreFaults::default())))
            .collect();
        let line = us
            .iter()
            .map(|(n, t)| format!("{n} entities: {t:.1} us"))
            .collect::<Vec<_>>()
            .join(", ");
        let ratio = us[2].1 / us[0].1;
        if ratio > LATENCY_RATIO {
            return Err(format!(
                "a 4-command automation batch: {line} (64k/1k = {ratio:.2} > {LATENCY_RATIO})"
            ));
        }
        // For the record (not asserted): the pre-WP-20 copying path at the ends.
        let copying = CoreFaults {
            batch_copies_project: true,
            ..CoreFaults::default()
        };
        let (c1, c64) = (median_us(1_000, copying), median_us(64_000, copying));
        Ok(format!(
            "a 4-command automation batch: {line} (64k/1k = {ratio:.2}); the copying path: \
             1000 entities: {c1:.1} us, 64000 entities: {c64:.1} us"
        ))
    });
    println!("{r:?}");
    assert!(r.is_ok(), "{r:?}");
}

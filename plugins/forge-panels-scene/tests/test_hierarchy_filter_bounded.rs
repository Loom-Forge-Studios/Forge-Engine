//! `test_hierarchy_filter_bounded` (Ch.21 §21.21, gate row `C-ui-hierarchy-filter-typing`,
//! DoD M2-35): **every slice of the hierarchy's filter job is strictly bounded.** The job
//! walks its rows, the mirror's roots and each parent's children by cursor, so no step
//! between two slice checks touches more than one item and its ancestors — at a phase
//! change too, where a snapshot of every row (O(n), uninterruptible) used to sit. The
//! probe records the largest step; the bound does not grow with the entity count.
//!
//! The rows each filter produces are checked against the mirror (matches plus their
//! ancestors, in key order) for narrowing, broadening and unrelated queries, so the cursor
//! walks are also shown to visit everything.
//!
//! Positive control (W2): `positive_control_filter_snapshots_fail` — the job snapshots
//! the rows and roots at each phase change; its largest step is then the row count.

mod common;

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use forge_cmd::{Bus, CommandSink, EditorCommand, Issuer};
use forge_editor::core::EditorCore;
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;
use forge_ui::WidgetId;
use forge_ui::widgets::{SearchChanged, VirtualTree};

const GROUPS: u64 = 200;
const CHILDREN: u64 = 99;
/// One item plus the ancestors it marks (depth 2 here), with room to spare; far below
/// the 20,000 rows a snapshot touches.
const STEP_BOUND: usize = 8;

fn core() -> forge_editor::core::SharedCore {
    let mut bus = Bus::new();
    for g in 0..GROUPS {
        let group = bus.project().next_key();
        let e = bus.envelope(
            Issuer::Test,
            EditorCommand::Spawn {
                name: format!("Group {g}"),
                parent: None,
            },
        );
        bus.apply(e).unwrap_or_else(|e| panic!("{e}"));
        for c in 0..CHILDREN {
            let e = bus.envelope(
                Issuer::Test,
                EditorCommand::Spawn {
                    name: format!("Entity {g}.{c}"),
                    parent: Some(group),
                },
            );
            bus.apply(e).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    EditorCore::with_bus(bus)
}

/// What the filter should show: matches and their ancestors.
fn expected(rig: &Rig, q: &str) -> BTreeSet<u64> {
    let m = rig.shell.mirror();
    let q = q.trim().to_lowercase();
    let mut out = BTreeSet::new();
    for (k, e) in m.entities() {
        if q.is_empty() || e.name.to_lowercase().contains(&q) {
            out.insert(k.0);
            let mut cur = e.parent;
            while let Some(p) = cur {
                out.insert(p.0);
                cur = m.entity(p).and_then(|pe| pe.parent);
            }
        }
    }
    out
}

fn run(faults: PanelFaults) -> Result<usize, String> {
    let probe = Rc::new(Cell::new(0usize));
    let mut cfg = common::config(&[]);
    cfg.services.faults = PanelFaults {
        hierarchy_step_probe: Some(probe.clone()),
        ..faults
    };
    let mut rig = Rig::with_core(cfg, core()).map_err(|e| e.to_string())?;
    rig.show_panels(&["forge.hierarchy"])
        .map_err(|e| e.to_string())?;
    let tree: WidgetId = common::part(&rig, "forge.hierarchy", &["tree"]);
    let search: WidgetId = common::part(&rig, "forge.hierarchy", &["bar", "search"]);
    let spinner: WidgetId = common::part(&rig, "forge.hierarchy", &["bar", "filtering"]);
    rig.settle();
    // Narrowing, narrowing, broadening, unrelated, narrowing, cleared.
    for q in [
        "e",
        "en",
        "entity 1",
        "entity 12",
        "entity 1",
        "group 3",
        "group 33",
        "entity 7.",
        "",
    ] {
        rig.h.ui.raise(
            search,
            SearchChanged {
                field: search,
                query: q.into(),
            },
        );
        rig.turn();
        let mut turns = 0;
        while !rig.h.ui.is_hidden(spinner) {
            rig.turn();
            turns += 1;
            if turns > 100_000 {
                return Err(format!("the filter {q:?} never finished"));
            }
        }
        let got: BTreeSet<u64> =
            VirtualTree::edit(&mut rig.h.ui, tree, |t| t.keys().collect()).unwrap_or_default();
        let want = expected(&rig, q);
        if got != want {
            return Err(format!(
                "filter {q:?}: {} rows shown, {} expected ({} missing, {} extra)",
                got.len(),
                want.len(),
                want.difference(&got).count(),
                got.difference(&want).count()
            ));
        }
        // Siblings stay in key order (the cursor walks rely on it).
        let ordered = VirtualTree::edit(&mut rig.h.ui, tree, |t| {
            got.iter().chain(std::iter::once(&u64::MAX)).all(|p| {
                let parent = (*p != u64::MAX).then_some(*p);
                let n = t.child_count(parent);
                (1..n).all(|i| t.child_at(parent, i - 1) < t.child_at(parent, i))
            })
        })
        .unwrap_or(false);
        if !ordered {
            return Err(format!("filter {q:?}: siblings out of key order"));
        }
    }
    let worst = probe.get();
    if worst > STEP_BOUND {
        return Err(format!(
            "a filter slice ran {worst} items between two deadline checks (bound {STEP_BOUND})"
        ));
    }
    Ok(worst)
}

#[test]
fn test_hierarchy_filter_bounded() {
    let worst = run(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    println!(
        "test_hierarchy_filter_bounded: largest step {worst} items over {} entities",
        GROUPS * (CHILDREN + 1)
    );
}

#[test]
fn positive_control_filter_snapshots_fail() {
    let e = run(PanelFaults {
        hierarchy_filter_snapshots: true,
        ..PanelFaults::default()
    })
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("between two deadline checks"),
        "a filter job snapshotting every row at a phase change passed: {e:?}"
    );
}

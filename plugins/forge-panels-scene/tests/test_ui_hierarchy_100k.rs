//! `ui_hierarchy_100k` (Ch.21 §21.22, D-5, gate rows `C-ui-hierarchy-100k`,
//! `C-ui-hierarchy-filtered`, `C-ui-hierarchy-filter-typing`; DoD M2-35): the **real**
//! hierarchy panel, in the running shell, over 100,000 mirrored entities meets the list
//! budget — live rows ≤ ⌈viewport ÷ row⌉ + 16, and ≤ 2.0 ms per loop turn (layout + paint +
//! the shell's per-window work) at p95 over 240 scrolled turns. A rename by another issuer
//! costs one incremental update (≤ 2.0 ms p95 over 60), **with a filter open too** (≤ 2.0 ms
//! p95 over 60 renames leaving, joining or staying in the filter, and the rows then equal a
//! fresh filter row for row). Typing, deleting and retyping the filter: every keystroke's
//! turn and every turn finishing it fits one frame (16.7 ms) at p95.
//!
//! Positive controls (W2): `positive_control_hierarchy_without_virtualisation_fails` (the
//! tree realises every row: the live-row bound fails),
//! `positive_control_hierarchy_rebuilding_under_a_filter_fails` (every change under a filter
//! rebuilds: the filtered-rename budget fails), `positive_control_hierarchy_unbounded_filter_fails`
//! (a filter applied in one turn: the keystroke budget fails).
//!
//! Each run is timed alone (`forge_ui::testing::run_timed_alone`): in a process of its own
//! that runs only that test, at the High priority class. At normal priority a concurrent
//! workspace build time-slices the one UI thread with a dozen compilers (filtered rename
//! 2.0-2.7 ms p95, WP-U8); and under `cargo test` the gate's three positive controls, each
//! building its own 100k-entity rig, ran as sibling threads of the same High-priority process
//! (2.7-3.0 ms beside a build, WP-U8 verifier). Scheduling, not a tolerance: every budget
//! above is unchanged (W5).

mod common;

use std::time::Instant;

use forge_cmd::{Bus, CommandSink, EditorCommand, EntityKey, Issuer};
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;
use forge_ui::widgets::{SearchChanged, VirtualTree};
use forge_ui::{InputEvent, WidgetId};

const ROOTS: u64 = 100;
const CHILDREN: u64 = 999;
const TURNS: usize = 240;
const BUDGET_MS: f64 = 2.0;
const ROW_H: f32 = 24.0;
/// One frame at 60 Hz: D-5 asks for one-frame typing latency.
const FRAME_MS: f64 = 16.7;
/// The filter typed (matches 10,989 of the 100,000 names).
const FILTER: &str = "entity 5";
const FILTERED_EDITS: usize = 60;
/// Renames by another issuer with no filter open.
const RENAMES: usize = 60;
/// Times the filter is typed quickly (every keystroke cutting the last one short).
const FAST_TYPING: usize = 5;

/// The 95th percentile (0 for no samples: a filter applied in one turn has no finishing turns).
fn p95(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * 0.95).ceil() as usize).min(v.len()) - 1]
}

/// A core holding 100 roots × 999 children = 100,000 entities.
fn big_core() -> forge_editor::core::SharedCore {
    let mut bus = Bus::new();
    let who = Issuer::Test;
    for r in 0..ROOTS {
        let root = bus.project().next_key();
        let e = bus.envelope(
            who.clone(),
            EditorCommand::Spawn {
                name: format!("Group {r}"),
                parent: None,
            },
        );
        bus.apply(e).unwrap_or_else(|e| panic!("{e}"));
        for c in 0..CHILDREN {
            let e = bus.envelope(
                who.clone(),
                EditorCommand::Spawn {
                    name: format!("Entity {r}.{c}"),
                    parent: Some(root),
                },
            );
            bus.apply(e).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    assert_eq!(bus.project().len() as u64, ROOTS * (CHILDREN + 1));
    EditorCore::with_bus(bus)
}

struct Measured {
    p95_ms: f64,
    max_live: usize,
    bound: usize,
    rows: usize,
    rename_ms: f64,
    keystroke_ms: f64,
    finish_ms: f64,
    settle_turns: usize,
    filtered_edit_ms: f64,
}

/// One gate run, alone: a process of its own running only this test, at the High priority
/// class (`forge_ui::testing::run_timed_alone`). Returns the summary line.
fn run(faults: PanelFaults) -> Result<String, String> {
    forge_ui::testing::run_timed_alone(|| {
        let m = measure(faults)?;
        Ok(format!(
            "{} rows, p95 {:.3} ms per turn, {} live rows (bound {}), incremental rename p95 {:.3} ms, filter keystroke turn p95 {:.3} ms, finishing turn p95 {:.3} ms (a filter applies within {} turns), filtered rename p95 {:.3} ms",
            m.rows,
            m.p95_ms,
            m.max_live,
            m.bound,
            m.rename_ms,
            m.keystroke_ms,
            m.finish_ms,
            m.settle_turns,
            m.filtered_edit_ms
        ))
    })
}

fn measure(faults: PanelFaults) -> Result<Measured, String> {
    let mut cfg = common::config(&[]);
    cfg.services.faults = faults;
    let mut rig = Rig::with_core(cfg, big_core()).map_err(|e| e.to_string())?;
    rig.show_panels(&["forge.hierarchy"])
        .map_err(|e| e.to_string())?;
    let tree: WidgetId = common::part(&rig, "forge.hierarchy", &["tree"]);
    // Open every group: 100,000 rows.
    VirtualTree::edit(&mut rig.h.ui, tree, |t| {
        for r in 0..ROOTS {
            let key = r * (CHILDREN + 1);
            t.set_expanded(key, true);
        }
    });
    rig.settle();
    let rows = VirtualTree::edit(&mut rig.h.ui, tree, |t| t.row_count()).unwrap_or(0);
    if rows as u64 != ROOTS * (CHILDREN + 1) {
        return Err(format!("{rows} rows shown, expected every entity"));
    }
    let r = rig.h.ui.rect(tree).unwrap_or_default();
    let bound = (r.h / ROW_H).ceil() as usize + 16;
    let at = r.center();
    rig.h.input(InputEvent::PointerMoved(at));
    rig.turn();
    let live =
        |rig: &mut Rig| VirtualTree::edit(&mut rig.h.ui, tree, |t| t.live_rows()).unwrap_or(0);
    let mut times = Vec::with_capacity(TURNS);
    let mut max_live = live(&mut rig);
    for f in 0..TURNS {
        let dy = match ((f / 60) % 2, f % 2) {
            (0, 0) => -8.0,
            (0, _) => -1.0,
            (_, 0) => 8.0,
            _ => 1.0,
        };
        rig.h.input(InputEvent::Wheel {
            pos: at,
            dx: 0.0,
            dy,
        });
        let t0 = Instant::now();
        rig.turn();
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
        max_live = max_live.max(live(&mut rig));
        if max_live > bound {
            return Err(format!(
                "{max_live} live rows over 100k entities (bound {bound})"
            ));
        }
    }
    let p95_ms = p95(times);
    if p95_ms > BUDGET_MS {
        return Err(format!(
            "p95 {p95_ms:.3} ms per turn over {TURNS} turns (budget {BUDGET_MS} ms)"
        ));
    }
    // Another issuer renames entities deep in the tree, one per loop turn: each is one
    // incremental update (p95 over RENAMES turns, like every D-5 budget).
    let mut script = rig.connect(Issuer::Script { path: "s".into() });
    let mut rename_times = Vec::with_capacity(RENAMES);
    for i in 0..RENAMES as u64 {
        let target = EntityKey((40 + i % 20) * (CHILDREN + 1) + 400 + i);
        let name = format!("Renamed {i}");
        script.apply(
            EditorCommand::Rename {
                entity: target,
                name: name.clone(),
            },
            None,
        );
        let _ = script.pump();
        let t0 = Instant::now();
        rig.turn();
        rename_times.push(t0.elapsed().as_secs_f64() * 1000.0);
        let label = VirtualTree::edit(&mut rig.h.ui, tree, |t| {
            t.label_of(target.0).map(str::to_string)
        })
        .flatten();
        if label.as_deref() != Some(name.as_str()) {
            return Err(format!("the rename did not reach the row: {label:?}"));
        }
    }
    let rename_ms = p95(rename_times);
    if rename_ms > BUDGET_MS {
        return Err(format!(
            "an incremental rename took {rename_ms:.3} ms at p95 over {RENAMES} renames (budget {BUDGET_MS} ms)"
        ));
    }
    // ---- the same panel with a filter open -----------------------------------------
    // Type the filter one keystroke per loop turn. The keystroke's turn and each turn that
    // finishes applying it (a bounded slice of the filter over 100,000 names, the rows
    // diffed, layout and paint) fit one frame, at p95 like every D-5 budget.
    let search: WidgetId = common::part(&rig, "forge.hierarchy", &["bar", "search"]);
    let spinner: WidgetId = common::part(&rig, "forge.hierarchy", &["bar", "filtering"]);
    let mut keystroke_times = Vec::new();
    let mut finish_times = Vec::new();
    let mut settle_turns = 0usize;
    let prefixes: Vec<String> = (1..=FILTER.len())
        .map(|n| FILTER[..n].to_string())
        .collect();
    // Type it (each keystroke applied fully); delete it one character at a time, and type
    // it quickly several times over, without waiting (a new keystroke cuts the running
    // one short); clear it; and type it once more, applying each keystroke fully.
    let mut keystrokes: Vec<(String, bool)> = prefixes.iter().map(|q| (q.clone(), true)).collect();
    keystrokes.extend(prefixes.iter().rev().skip(1).map(|q| (q.clone(), false)));
    for _ in 0..FAST_TYPING {
        keystrokes.push((String::new(), false));
        keystrokes.extend(prefixes.iter().map(|q| (q.clone(), false)));
    }
    keystrokes.push((String::new(), true));
    keystrokes.extend(prefixes.iter().map(|q| (q.clone(), true)));
    for (q, finish) in &keystrokes {
        let t0 = Instant::now();
        rig.h.ui.raise(
            search,
            SearchChanged {
                field: search,
                query: q.clone(),
            },
        );
        rig.turn();
        keystroke_times.push(t0.elapsed().as_secs_f64() * 1000.0);
        let mut turns = 0;
        while *finish && !rig.h.ui.is_hidden(spinner) {
            let t0 = Instant::now();
            rig.turn();
            finish_times.push(t0.elapsed().as_secs_f64() * 1000.0);
            turns += 1;
            if turns > 10_000 {
                return Err(format!("the filter {q:?} never finished applying"));
            }
        }
        settle_turns = settle_turns.max(turns);
    }
    let keystroke_n = keystroke_times.len();
    let finish_n = finish_times.len();
    let keystroke_ms = p95(keystroke_times);
    let finish_ms = p95(finish_times);
    if keystroke_ms > FRAME_MS || finish_ms > FRAME_MS {
        return Err(format!(
            "a filter keystroke's turn took {keystroke_ms:.3} ms at p95 over {keystroke_n} keystrokes, a turn finishing one {finish_ms:.3} ms at p95 over {finish_n} turns, over 100k entities (budget one frame, {FRAME_MS} ms)"
        ));
    }
    // The filter shows the matches (`Entity 5.*` and `Entity 50.*` to `Entity 59.*`, less
    // the one renamed above: 10,988) with their groups as context rows.
    let expect_matches = rig
        .shell
        .mirror()
        .entities()
        .filter(|(_, e)| e.name.to_lowercase().contains(FILTER))
        .count();
    let (shown, matched) = shown_rows(&mut rig, tree);
    if matched != expect_matches {
        return Err(format!(
            "the filter shows {matched} matches, expected {expect_matches} ({shown} rows)"
        ));
    }
    // Another issuer streams renames while the filter is open: an entity leaves the filter,
    // another joins it, one inside it changes. Each is one incremental update.
    let mut filtered_times = Vec::with_capacity(FILTERED_EDITS);
    for i in 0..FILTERED_EDITS as u64 {
        let (entity, name) = match i % 3 {
            // A match renamed away: its row goes.
            0 => (
                EntityKey(5 * (CHILDREN + 1) + 1 + i),
                format!("Moved away {i}"),
            ),
            // A non-match renamed into the filter: its row (and group) appears.
            1 => (
                EntityKey(77 * (CHILDREN + 1) + 1 + i),
                format!("Entity 5 joined {i}"),
            ),
            // A match renamed but still matching.
            _ => (
                EntityKey(5 * (CHILDREN + 1) + 500 + i),
                format!("Entity 5 renamed {i}"),
            ),
        };
        script.apply(EditorCommand::Rename { entity, name }, None);
        let _ = script.pump();
        let t0 = Instant::now();
        rig.turn();
        filtered_times.push(t0.elapsed().as_secs_f64() * 1000.0);
        let shown_now = VirtualTree::edit(&mut rig.h.ui, tree, |t| t.contains(entity.0));
        if shown_now != Some(i % 3 != 0) {
            return Err(format!(
                "rename {i} under the filter: row shown {shown_now:?}, expected {}",
                i % 3 != 0
            ));
        }
    }
    let filtered_edit_ms = p95(filtered_times);
    if filtered_edit_ms > BUDGET_MS {
        return Err(format!(
            "a rename by another issuer under a filter took {filtered_edit_ms:.3} ms at p95 over {FILTERED_EDITS} edits (budget {BUDGET_MS} ms)"
        ));
    }
    // The incremental result equals a from-scratch filter of the same mirror: row for
    // row, in order, with the same dimming.
    let incremental = snapshot(&mut rig, tree);
    for q in ["", FILTER] {
        rig.h.ui.raise(
            search,
            SearchChanged {
                field: search,
                query: q.into(),
            },
        );
        rig.turn();
        while !rig.h.ui.is_hidden(spinner) {
            rig.turn();
        }
    }
    let fresh = snapshot(&mut rig, tree);
    if incremental != fresh {
        return Err(format!(
            "incremental filtered rows ({} rows) differ from a fresh filter ({} rows)",
            incremental.len(),
            fresh.len()
        ));
    }
    Ok(Measured {
        p95_ms,
        max_live,
        bound,
        rows,
        rename_ms,
        keystroke_ms,
        finish_ms,
        settle_turns,
        filtered_edit_ms,
    })
}

/// (rows in the tree at any depth, rows that are matches rather than context).
fn shown_rows(rig: &mut Rig, tree: WidgetId) -> (usize, usize) {
    VirtualTree::edit(&mut rig.h.ui, tree, |t| {
        let n = t.keys().count();
        let matched = t
            .keys()
            .filter(|k| t.item(*k).is_some_and(|i| !i.muted))
            .count();
        (n, matched)
    })
    .unwrap_or_default()
}

/// Every visible row in order: (key, depth, label, dimmed).
fn snapshot(rig: &mut Rig, tree: WidgetId) -> Vec<(u64, u32, String, bool)> {
    VirtualTree::edit(&mut rig.h.ui, tree, |t| {
        (0..t.row_count())
            .filter_map(|r| {
                let (k, depth) = t.index().node_at_row(r as u64)?;
                let item = t.item(k)?;
                Some((k, depth, item.label.clone(), item.muted))
            })
            .collect()
    })
    .unwrap_or_default()
}

#[test]
fn ui_hierarchy_100k() {
    let m = run(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    println!("ui_hierarchy_100k: {m}");
}

#[test]
fn positive_control_hierarchy_without_virtualisation_fails() {
    let e = run(PanelFaults {
        hierarchy_no_virtualisation: true,
        ..PanelFaults::default()
    })
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("live rows"),
        "the unvirtualised hierarchy passed: {e:?}"
    );
}

#[test]
fn positive_control_hierarchy_rebuilding_under_a_filter_fails() {
    let e = run(PanelFaults {
        hierarchy_filter_rebuilds: true,
        ..PanelFaults::default()
    })
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("under a filter took"),
        "the hierarchy rebuilding every row per change under a filter passed: {e:?}"
    );
}

#[test]
fn positive_control_hierarchy_unbounded_filter_fails() {
    let e = run(PanelFaults {
        hierarchy_filter_unbounded: true,
        ..PanelFaults::default()
    })
    .err()
    .unwrap_or_default();
    assert!(
        e.contains("keystroke's turn took"),
        "the hierarchy applying a filter in one turn passed: {e:?}"
    );
}

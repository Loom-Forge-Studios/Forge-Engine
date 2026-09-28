//! `test_console_search_alloc` (Ch.21 §21.21 "Console", DoD M2-39, gate row
//! `C-console-constant-work`): filtering the console by a search allocates **nothing**
//! per entry. The query is folded once (`forge_ui::fuzzy::ContainsQuery`) and each entry's
//! link text is rendered once when it is logged, so re-filtering 10,000 entries on a
//! keystroke is a scan, not 30,000 lower-cased copies.
//!
//! Positive control (W2): `positive_control_lowercasing_search_allocates` runs the same
//! measure over the old matcher (lower-case the query, message, target and link per entry)
//! and must see allocations — the counter is not vacuous.

use forge_editor::console::{
    CONSOLE_CAP, ConsoleFilter, ConsoleLog, LogEntry, LogLevel, LogSource,
};
use forge_ui::fuzzy::ContainsQuery;

fn full_log() -> ConsoleLog {
    let mut l = ConsoleLog::new();
    for i in 0..CONSOLE_CAP {
        let source = match i % 4 {
            0 => LogSource::None,
            1 => LogSource::SeedPath(format!("universe/world:{}", i % 97)),
            2 => LogSource::Entity(forge_cmd::EntityKey(i as u64)),
            _ => LogSource::Command {
                what: format!("Rename {i}"),
                code: None,
                txn: None,
            },
        };
        l.push(
            LogLevel::Info,
            "forge.gen",
            &format!("Ärger line {i}"),
            source,
        );
    }
    l
}

/// Heap allocations made while `f` filters the log with each query, and what it counted.
fn allocations(log: &ConsoleLog, f: impl Fn(&LogEntry, &str) -> bool) -> (u64, usize) {
    let queries = [
        "WORLD:3",
        "ärger LINE 99",
        "entity 12",
        "nothing matches this",
        "",
    ];
    let mut shown = 0;
    let info = allocation_counter::measure(|| {
        for q in queries {
            shown += log.entries().filter(|e| f(e, q)).count();
        }
    });
    (info.count_total, shown)
}

#[test]
fn test_console_search_alloc() {
    let log = full_log();
    // The filters are built (folded) outside the measure, as the panel does once per
    // keystroke; the per-entry pass is what is measured.
    let filters: Vec<(String, ConsoleFilter)> = [
        "WORLD:3",
        "ärger LINE 99",
        "entity 12",
        "nothing matches this",
        "",
    ]
    .iter()
    .map(|q| {
        (
            (*q).to_string(),
            ConsoleFilter {
                levels: [true; 5],
                search: ContainsQuery::new(q),
            },
        )
    })
    .collect();
    let (n, shown) = allocations(&log, |e, q| {
        filters
            .iter()
            .find(|(s, _)| s == q)
            .is_some_and(|(_, f)| f.shows(e))
    });
    assert!(
        shown > CONSOLE_CAP,
        "the queries matched too little to measure: {shown}"
    );
    assert_eq!(
        n, 0,
        "filtering {CONSOLE_CAP} console entries by search made {n} heap allocations; the query \
         must be folded once and each entry's link text rendered when it is logged"
    );
    // And it finds what the old matcher found (case-folded, link text included).
    let (_, old_shown) = allocations(&log, lowercasing);
    assert_eq!(
        shown, old_shown,
        "the folded search disagrees with lower-casing"
    );
}

/// The matcher before WP-U13: lower-case everything, per entry.
fn lowercasing(e: &LogEntry, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty()
        || e.message.to_lowercase().contains(&q)
        || e.target.to_lowercase().contains(&q)
        || e.source
            .link_text()
            .is_some_and(|l| l.to_lowercase().contains(&q))
}

#[test]
fn positive_control_lowercasing_search_allocates() {
    let log = full_log();
    let (n, _) = allocations(&log, lowercasing);
    assert!(
        n > 0,
        "the allocation counter saw nothing from a matcher that lower-cases every entry"
    );
}

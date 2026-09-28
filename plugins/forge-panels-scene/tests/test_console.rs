//! The console through the running shell (DoD M2-39): level filters, search, grouping of
//! repeated lines, entries from other threads, click-through to a `SeedPath` (revealed in
//! the panel a plugin declared for seed paths; no click-through without one), to an entity,
//! to a command's transaction, and a refused command's "Details" opening the console at its
//! entry.

mod common;

use forge_cmd::{EditorCommand, EntityKey};
use forge_editor::console::{CONSOLE_CAP, ConsoleFilter, LogLevel, LogSource};
use forge_editor::services::EditorServices;
use forge_editor::services::PanelFaults;
use forge_editor::session::{Reveal, SeedPathPanel};
use forge_editor::testing::Rig;
use forge_ui::dock::PanelId;
use forge_ui::widgets::{RowActivated, SearchChanged, VirtualTree};

const P: &str = "forge.console";

/// The panel these tests declare for seed paths (a base panel standing in for a plugin's).
const SEED_PANEL: &str = "forge.inspector";

/// What a plugin whose panel shows seed paths does from its service provider.
fn declare_seed_path_panel(s: &mut EditorServices) {
    s.extensions.insert(std::rc::Rc::new(SeedPathPanel {
        panel: PanelId::new(SEED_PANEL),
        title: "Seeds".into(),
    }));
}

fn rows(rig: &mut Rig) -> Vec<(u64, String)> {
    let l = common::part(rig, P, &["list"]);
    VirtualTree::edit(&mut rig.h.ui, l, |t| {
        (0..t.row_count())
            .filter_map(|r| {
                t.key_at(r)
                    .map(|k| (k, t.label_of(k).unwrap_or("").to_string()))
            })
            .collect()
    })
    .unwrap_or_default()
}

fn log(rig: &mut Rig, level: LogLevel, msg: &str, src: LogSource) -> u64 {
    let id = rig.shell.log().borrow_mut().push(level, "test", msg, src);
    rig.turn();
    id
}

#[test]
fn levels_search_and_grouping() {
    let mut rig = common::rig(&[P]);
    log(&mut rig, LogLevel::Info, "loaded", LogSource::None);
    log(&mut rig, LogLevel::Warn, "slow frame", LogSource::None);
    log(&mut rig, LogLevel::Warn, "slow frame", LogSource::None);
    log(
        &mut rig,
        LogLevel::Error,
        "bad slope",
        LogSource::SeedPath("universe/world:1".into()),
    );
    log(&mut rig, LogLevel::Trace, "noise", LogSource::None);
    let r = rows(&mut rig);
    assert_eq!(
        r.len(),
        3,
        "trace is off by default; the repeat is grouped: {r:?}"
    );
    assert!(
        r[1].1.contains("\u{d7}2"),
        "the grouped row shows its count: {}",
        r[1].1
    );
    assert!(
        r[2].1.contains(&format!("seed {}", "universe/world:1")),
        "the link is shown: {}",
        r[2].1
    );
    // Hide warnings with the level box (a real click).
    rig.h.click(common::part(&rig, P, &["bar", "warn"]));
    rig.turn();
    let r = rows(&mut rig);
    assert!(r.iter().all(|(_, l)| !l.starts_with("Warning")), "{r:?}");
    assert_eq!(r.len(), 2);
    // Search matches the message, the subsystem and the link.
    let s = common::part(&rig, P, &["bar", "search"]);
    rig.h.ui.raise(
        s,
        SearchChanged {
            field: s,
            query: "WORLD".into(),
        },
    );
    rig.turn();
    let r = rows(&mut rig);
    assert_eq!(r.len(), 1);
    assert!(r[0].1.contains("bad slope"));
    // A new line while filtered is appended only if it matches.
    log(&mut rig, LogLevel::Error, "world lost", LogSource::None);
    log(&mut rig, LogLevel::Error, "unrelated", LogSource::None);
    assert_eq!(rows(&mut rig).len(), 2);
    // Clearing the console is session state: the project does not change.
    let h = rig.state_hash();
    rig.h.click(common::part(&rig, P, &["bar", "clear"]));
    rig.turn();
    assert_eq!(rows(&mut rig).len(), 0);
    assert_eq!(rig.state_hash(), h);
}

#[test]
fn entries_from_other_threads_arrive() {
    let mut rig = common::rig(&[P]);
    let sender = rig.shell.log().borrow().sender();
    let t = std::thread::spawn(move || {
        for i in 0..5 {
            sender.log(
                LogLevel::Info,
                "worker",
                &format!("chunk {i} generated"),
                LogSource::None,
            );
        }
    });
    let _ = t.join();
    rig.turn();
    let r = rows(&mut rig);
    assert_eq!(r.len(), 5);
    assert!(r[4].1.contains("chunk 4 generated"));
}

#[test]
fn click_through_goes_to_the_source() {
    let mut rig = common::rig_with(
        &[P, "forge.hierarchy", "forge.undo_history"],
        PanelFaults::default(),
        declare_seed_path_panel,
        &[],
    );
    let seed = "universe/world:2/zone:77";
    let id = log(
        &mut rig,
        LogLevel::Error,
        "cliff too steep",
        LogSource::SeedPath(seed.into()),
    );
    let l = common::part(&rig, P, &["list"]);
    rig.h.ui.raise(l, RowActivated { view: l, key: id });
    rig.turn();
    rig.turn();
    let revealed = rig.shell.session().pending_reveal().map(|(_, r)| r.clone());
    assert_eq!(revealed, Some(Reveal::SeedPath(seed.into())));
    assert!(
        rig.panel_frame(SEED_PANEL).is_some(),
        "the declared seed-path panel opens"
    );
    // An entity source selects the entity.
    let e = common::spawn(&mut rig, "Rover", None);
    let id = log(&mut rig, LogLevel::Warn, "stuck", LogSource::Entity(e));
    rig.h.ui.raise(l, RowActivated { view: l, key: id });
    rig.turn();
    assert_eq!(rig.shell.session().selection, vec![e]);
    // A command with a transaction opens the undo history at it.
    rig.shell.emitter().emit(EditorCommand::Rename {
        entity: e,
        name: "Rover 2".into(),
    });
    rig.turn();
    let txn = rig
        .shell
        .mirror()
        .history()
        .last()
        .map(|h| h.txn)
        .unwrap_or_else(|| panic!("txn"));
    let id = log(
        &mut rig,
        LogLevel::Info,
        "renamed",
        LogSource::Command {
            what: "Rename".into(),
            code: None,
            txn: Some(txn),
        },
    );
    rig.h.ui.raise(l, RowActivated { view: l, key: id });
    rig.turn();
    rig.turn();
    let hist = common::part(&rig, "forge.undo_history", &["list"]);
    let sel = VirtualTree::edit(&mut rig.h.ui, hist, |t| t.selected()).unwrap_or_default();
    assert_eq!(
        sel,
        vec![txn.0],
        "the history row of that transaction is selected"
    );
}

/// With no panel declared for seed paths there is nowhere to show one: a seed-path entry's
/// click-through reveals nothing and opens no panel (the editor never asks the shell for a
/// panel that does not exist).
#[test]
fn a_seed_path_is_no_click_through_without_a_seed_path_panel() {
    let mut rig = common::rig_with(&[P], PanelFaults::default(), |_| {}, &[]);
    assert!(SeedPathPanel::of(rig.shell.handles().services()).is_none());
    let id = log(
        &mut rig,
        LogLevel::Error,
        "cliff too steep",
        LogSource::SeedPath("universe/world:2/zone:77".into()),
    );
    let before = rig.shell.layout().panels();
    let l = common::part(&rig, P, &["list"]);
    rig.h.ui.raise(l, RowActivated { view: l, key: id });
    rig.turn();
    rig.turn();
    assert_eq!(rig.shell.session().pending_reveal(), None);
    assert_eq!(rig.shell.layout().panels(), before, "no panel opened");
}

#[test]
fn a_refusal_opens_the_console_at_its_entry() {
    let mut rig = common::rig(&[P]);
    // Filter the list so the entry is hidden: the reveal must still find it.
    let s = common::part(&rig, P, &["bar", "search"]);
    rig.h.ui.raise(
        s,
        SearchChanged {
            field: s,
            query: "zzz".into(),
        },
    );
    rig.turn();
    rig.shell.emitter().emit(EditorCommand::Despawn {
        entity: EntityKey(999),
    });
    rig.turn();
    let entry = rig
        .shell
        .log()
        .borrow()
        .entries()
        .last()
        .map(|e| (e.id, e.source.clone()))
        .unwrap_or_else(|| panic!("the refusal is logged"));
    assert!(matches!(&entry.1, LogSource::Command { code: Some(c), .. } if c.starts_with("CMD-")));
    rig.shell.session_mut().reveal(Reveal::LogEntry(entry.0), P);
    rig.turn();
    let l = common::part(&rig, P, &["list"]);
    let sel = VirtualTree::edit(&mut rig.h.ui, l, |t| t.selected()).unwrap_or_default();
    assert_eq!(
        sel,
        vec![entry.0],
        "the entry is selected (the filter was lifted to show it)"
    );
}

/// Fill the log to its cap, then add lines one loop turn at a time (new lines, repeats and
/// a filtered-out one). Returns the most rows the list wrote for one line, or an error
/// naming a row that disagrees with the log.
fn rows_written_per_line_at_the_cap(faults: PanelFaults) -> Result<u64, String> {
    let mut rig = common::rig_with(&[P], faults, |_| {}, &[]);
    {
        let log = rig.shell.log();
        let mut log = log.borrow_mut();
        for i in 0..CONSOLE_CAP + 50 {
            log.push(
                LogLevel::Info,
                "fill",
                &format!("line {i}"),
                LogSource::None,
            );
        }
    }
    rig.turn();
    let l = common::part(&rig, P, &["list"]);
    let written =
        |rig: &mut Rig| VirtualTree::edit(&mut rig.h.ui, l, |t| t.rows_written()).unwrap_or(0);
    let mut worst = 0;
    for i in 0..60 {
        let (level, msg) = match i % 4 {
            0 | 1 => (LogLevel::Warn, format!("chatty {}", i / 2)),
            2 => (LogLevel::Trace, "hidden by the default filter".to_string()),
            _ => (LogLevel::Error, format!("refused {i}")),
        };
        let before = written(&mut rig);
        log(&mut rig, level, &msg, LogSource::None);
        worst = worst.max(written(&mut rig) - before);
    }
    // The rows are exactly what the filter shows of the log, in order, with counts.
    let want: Vec<(u64, String)> = {
        let log = rig.shell.log();
        let log = log.borrow();
        let f = ConsoleFilter::default();
        log.filtered(&f).map(|e| (e.id, e.row_text())).collect()
    };
    let got = rows(&mut rig);
    if got != want {
        let at = got.iter().zip(&want).position(|(a, b)| a != b);
        return Err(format!(
            "{} rows shown for {} filtered entries; first difference at {at:?}",
            got.len(),
            want.len()
        ));
    }
    Ok(worst)
}

#[test]
fn console_follows_in_constant_work() {
    let worst =
        rows_written_per_line_at_the_cap(PanelFaults::default()).unwrap_or_else(|e| panic!("{e}"));
    println!("console at the {CONSOLE_CAP}-entry cap: at most {worst} rows written per new line");
    assert!(
        worst <= 2,
        "one new line at the cap wrote {worst} rows (append one, trim one)"
    );
}

#[test]
fn positive_control_console_rebuilding_every_line_fails() {
    let worst = rows_written_per_line_at_the_cap(PanelFaults {
        console_rebuild_every_line: true,
        ..PanelFaults::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        worst > 2,
        "the console rebuilding every row per line passed the bound ({worst} rows)"
    );
}

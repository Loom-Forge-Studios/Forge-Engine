//! `test_layout_roundtrip` (Ch.21 §21.17, §21.23; DoD M2-26): layouts are RON data that
//! round-trip exactly — every preset's default, arbitrary edited layouts (splits, tabs,
//! floating windows on named monitors, a maximised panel), and user layouts saved and
//! restored by name — and the autosave is debounced and crash-safe.
//!
//! Positive control (W2): `positive_control_a_serializer_dropping_unknown_ids_fails` — a
//! serializer that writes only the panels it knows (what an editor that validates ids
//! against its registry does) fails the round-trip check.

mod common;

use std::time::Duration;

use forge_editor::layout_store::{AUTOSAVE_DEBOUNCE, AUTOSAVE_MAX_DELAY, Autosave, LayoutStore};
use forge_editor::presets::builtin_presets;
use forge_editor::stand_in::PANELS;
use forge_editor::user_config::{temp_path, write_atomic};
use forge_ui::Rect;
use forge_ui::dock::{AreaId, Axis, DockNode, DropTarget, DropZone, Layout, PanelId, Side};

/// Round-trip `l` through `save` and the real parser: `Err` names what changed.
fn check_roundtrip(l: &Layout, save: &dyn Fn(&Layout) -> String) -> Result<(), String> {
    let text = save(l);
    let back = Layout::from_ron(&text).map_err(|e| format!("does not parse back: {e}"))?;
    if back != *l {
        let lost: Vec<PanelId> = l
            .panels()
            .into_iter()
            .filter(|p| !back.contains(p))
            .collect();
        return Err(format!(
            "round trip changed the layout; panels lost: {lost:?}"
        ));
    }
    // And it is a fixed point: writing it again gives the same text.
    let again = save(&back);
    if again != text {
        return Err("a second round trip wrote different text".into());
    }
    Ok(())
}

fn real_save(l: &Layout) -> String {
    l.to_ron().unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn every_preset_layout_round_trips() {
    let presets = builtin_presets().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(presets.len(), forge_editor::presets::BUILTIN_PRESETS.len());
    for p in &presets {
        check_roundtrip(&p.layout, &real_save)
            .unwrap_or_else(|e| panic!("{}: {e}", p.workspace.id));
    }
}

/// A small deterministic generator (no dependency, reproducible failures by seed).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn start() -> Layout {
    Layout::new(DockNode::split(
        Axis::Horizontal,
        vec![
            (
                0.2,
                DockNode::tabs(&["forge.hierarchy", "forge.undo_history"]),
            ),
            (
                0.6,
                DockNode::split(
                    Axis::Vertical,
                    vec![
                        (0.7, DockNode::tabs(&["forge.viewport"])),
                        (0.3, DockNode::tabs(&["forge.console", "forge.assets"])),
                    ],
                ),
            ),
            (
                0.2,
                DockNode::tabs(&["forge.inspector", "com.example.rivers"]),
            ),
        ],
    ))
}

/// Apply one random edit, as a user's drag/drop/menu would.
fn random_edit(l: &mut Layout, r: &mut Rng, closed: &mut Vec<PanelId>) {
    let panels = l.panels();
    let pick = |r: &mut Rng, v: &[PanelId]| v[r.below(v.len())].clone();
    let sides = [Side::Left, Side::Right, Side::Top, Side::Bottom];
    match r.below(8) {
        0 | 1 if panels.len() > 1 => {
            let p = pick(r, &panels);
            let anchor = pick(r, &panels);
            let area = l.find(&anchor).map_or(AreaId::Main, |f| f.0);
            let zone = if r.below(2) == 0 {
                DropZone::Tab(Some(r.below(3)))
            } else {
                DropZone::Side(sides[r.below(4)])
            };
            let _ = l.move_panel(&p, &DropTarget::Group { area, anchor, zone });
        }
        2 if panels.len() > 1 => {
            let p = pick(r, &panels);
            let _ = l.move_panel(
                &p,
                &DropTarget::AreaEdge {
                    area: AreaId::Main,
                    side: sides[r.below(4)],
                },
            );
        }
        3 if panels.len() > 1 => {
            let p = pick(r, &panels);
            let mon = ["DISPLAY1", "DISPLAY2", "\\\\.\\DISPLAY3"][r.below(3)];
            let rect = Rect::new(
                r.below(3000) as f32 - 500.0,
                r.below(1400) as f32,
                300.0 + r.below(600) as f32,
                200.0 + r.below(400) as f32,
            );
            let _ = l.float(&p, rect, Some(mon.to_string()));
        }
        4 => {
            if let Some(w) = l.floating.first().map(|w| w.id) {
                l.dock_back(w);
            }
        }
        5 if panels.len() > 2 => {
            let p = pick(r, &panels);
            l.close(&p);
            closed.push(p);
        }
        6 => {
            if let Some(p) = closed.pop() {
                let target = match l.main.panels().first() {
                    Some(a) => DropTarget::Group {
                        area: AreaId::Main,
                        anchor: a.clone(),
                        zone: DropZone::Tab(None),
                    },
                    None => DropTarget::EmptyArea(AreaId::Main),
                };
                let _ = l.insert(p, &target);
            }
        }
        _ => {
            if let DockNode::Split { children, .. } = &l.main {
                let n = children.len();
                let ratios: Vec<f32> = (0..n).map(|_| 0.05 + r.below(100) as f32 / 100.0).collect();
                let _ = l.set_ratios(AreaId::Main, &[], ratios);
            }
            if r.below(3) == 0 && !panels.is_empty() {
                let p = pick(r, &panels);
                l.toggle_maximise(&p);
            }
        }
    }
}

#[test]
fn arbitrary_edited_layouts_round_trip() {
    for seed in 1..=300u64 {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0x2545_F491_4F6C_DD1D));
        let mut l = start();
        let mut closed = Vec::new();
        let total = l.panels().len();
        for _ in 0..(5 + r.below(30)) {
            random_edit(&mut l, &mut r, &mut closed);
            // Every edit keeps every panel exactly once (closed ones aside).
            let mut ps = l.panels();
            ps.sort();
            let n = ps.len();
            ps.dedup();
            assert_eq!(ps.len(), n, "seed {seed}: a panel appears twice: {l:?}");
            assert_eq!(
                n + closed.len(),
                total,
                "seed {seed}: a panel was lost: {l:?}"
            );
        }
        check_roundtrip(&l, &real_save).unwrap_or_else(|e| panic!("seed {seed}: {e}\n{l:?}"));
    }
}

/// W2: a serializer that drops panels it does not know (here: anything the stand-in set
/// lacks, like a disabled plugin's `com.example.rivers`) must fail the round trip.
#[test]
fn positive_control_a_serializer_dropping_unknown_ids_fails() {
    let known: Vec<&str> = PANELS.iter().map(|p| p.0).collect();
    let dropping = |l: &Layout| {
        let mut c = l.clone();
        for p in l.panels() {
            if !known.contains(&p.as_str()) {
                c.close(&p);
            }
        }
        c.to_ron().unwrap_or_default()
    };
    let e = check_roundtrip(&start(), &dropping).expect_err("a dropping serializer passed");
    assert!(e.contains("com.example.rivers"), "{e}");
}

#[test]
fn user_layouts_are_saved_and_restored_by_name() {
    let dir = common::scratch("layouts");
    let store = LayoutStore::new(dir.join("layouts"));
    assert!(store.list().unwrap_or_default().is_empty());
    let mut l = start();
    l.float(
        &PanelId::new("forge.console"),
        Rect::new(1920.0, 100.0, 640.0, 400.0),
        Some("DISPLAY2".into()),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    store
        .save("Level design", &l)
        .unwrap_or_else(|e| panic!("{e}"));
    store
        .save("compact_2", &start())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        store.list().unwrap_or_default(),
        vec!["Level design".to_string(), "compact_2".to_string()]
    );
    assert_eq!(store.load("Level design").ok().flatten(), Some(l.clone()));
    // Saving again replaces it.
    store
        .save("Level design", &start())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(store.load("Level design").ok().flatten(), Some(start()));
    assert_eq!(store.load("nope").ok().flatten(), None);
    for bad in ["", "../escape", "a/b", " lead", "autosave", &"x".repeat(65)] {
        assert!(store.save(bad, &l).is_err(), "{bad:?} was accepted");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_autosave_is_debounced_and_bounded() {
    let dir = common::scratch("autosave");
    let store = LayoutStore::new(dir.clone());
    let mut a = Autosave::new();
    let l = start();
    let s = |ms: u64| Duration::from_millis(ms);
    assert_eq!(a.deadline(), None, "nothing unsaved, nothing scheduled");
    a.changed(s(0));
    a.changed(s(1500)); // a second change pushes the write out
    assert_eq!(a.deadline(), Some(s(1500) + AUTOSAVE_DEBOUNCE));
    assert!(!a.tick(s(3000), &store, &l).unwrap_or(true));
    assert!(a.tick(s(3600), &store, &l).unwrap_or(false));
    assert_eq!(store.load_autosave().ok().flatten(), Some(l.clone()));
    assert_eq!(a.deadline(), None);
    // A change every second for a minute (a long drag) still saves within the bound.
    let mut writes_at = Vec::new();
    for t in 0..60u64 {
        a.changed(s(10_000 + t * 1000));
        if a.tick(s(10_000 + t * 1000), &store, &l).unwrap_or(false) {
            writes_at.push(t);
        }
    }
    assert!(!writes_at.is_empty() && writes_at[0] * 1000 <= AUTOSAVE_MAX_DELAY.as_millis() as u64);
    // Exit flushes what is unsaved.
    a.changed(s(99_000));
    a.flush(&store, &l).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(a.deadline(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_autosave_survives_a_crash_mid_write() {
    let dir = common::scratch("crash");
    let store = LayoutStore::new(dir.clone());
    let good = start();
    store.save_autosave(&good).unwrap_or_else(|e| panic!("{e}"));
    let file = dir.join("autosave.ron");
    assert!(
        !temp_path(&file).exists(),
        "a completed write leaves no temporary file"
    );
    // A crash after writing half of the temporary file, before the rename.
    std::fs::write(temp_path(&file), "(version: 1, main: Split(axis: Horiz").unwrap_or_default();
    assert_eq!(
        store.load_autosave().ok().flatten(),
        Some(good.clone()),
        "the last complete autosave loads; the torn temporary is ignored"
    );
    // The next write replaces both.
    let mut next = good.clone();
    next.toggle_maximise(&PanelId::new("forge.viewport"));
    store.save_autosave(&next).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(store.load_autosave().ok().flatten(), Some(next));
    assert!(!temp_path(&file).exists());
    // write_atomic over an existing file replaces it in one step.
    write_atomic(&file, b"(version: 1, main: Tabs(panels: [], active: 0))")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        store.load_autosave().ok().flatten(),
        Some(Layout::default())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

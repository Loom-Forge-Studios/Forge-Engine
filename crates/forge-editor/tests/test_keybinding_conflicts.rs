//! `test_keybinding_conflicts` (Ch.21 §21.17, §21.23; DoD M2-26/M2-44 engine): two
//! bindings of one chord in overlapping contexts are reported, when the binding is made,
//! with both action names — never silently last-wins. Non-overlapping contexts (two
//! different panels) may share a chord; an inner context may override an outer binding
//! only by naming it; a two-stroke chord conflicts with its own first stroke; a user or
//! preset layer that collides with a default is reported and changes nothing.
//!
//! Positive control (W2): `positive_control_a_last_wins_map_fails` — the same checks
//! against a map that simply overwrites (a `HashMap` insert) fail.

use std::collections::HashMap;

use forge_editor::EditorError;
use forge_editor::keymap::{Chord, KeyContext, KeyMap, KeymapFile, Resolution, default_keymap};
use forge_ui::{KeyCode, KeyEvent, Modifiers};

fn c(s: &str) -> Chord {
    Chord::parse(s).unwrap_or_else(|e| panic!("{e}"))
}
fn panel(p: &str) -> KeyContext {
    KeyContext::Panel(p.into())
}

/// What the conflict checks need of a keymap.
trait Bindable {
    fn bind(&mut self, chord: Chord, ctx: KeyContext, action: &str) -> Result<(), EditorError>;
    fn action_at(&mut self, chord: &str, path: &[KeyContext]) -> Option<String>;
}

impl Bindable for KeyMap {
    fn bind(&mut self, chord: Chord, ctx: KeyContext, action: &str) -> Result<(), EditorError> {
        KeyMap::bind(self, chord, ctx, action)
    }
    fn action_at(&mut self, chord: &str, path: &[KeyContext]) -> Option<String> {
        let mut last = None;
        for s in c(chord).0 {
            let code = match s.key.as_str() {
                "Delete" => KeyCode::Delete,
                "F2" => KeyCode::F2,
                "F5" => KeyCode::F5,
                k => KeyCode::Char(k.chars().next().unwrap_or('?').to_ascii_lowercase()),
            };
            let ev = KeyEvent::press(
                code,
                Modifiers {
                    ctrl: s.ctrl,
                    shift: s.shift,
                    alt: s.alt,
                    meta: s.meta,
                },
            );
            last = Some(self.resolve(&ev, path));
        }
        match last {
            Some(Resolution::Action(a)) => Some(a),
            _ => None,
        }
    }
}

/// A last-wins map: what the guard exists to reject.
#[derive(Default)]
struct LastWins(HashMap<(String, KeyContext), String>);

impl Bindable for LastWins {
    fn bind(&mut self, chord: Chord, ctx: KeyContext, action: &str) -> Result<(), EditorError> {
        self.0.insert((chord.to_string(), ctx), action.to_string());
        Ok(())
    }
    fn action_at(&mut self, chord: &str, path: &[KeyContext]) -> Option<String> {
        let chord = c(chord).to_string();
        path.iter()
            .chain([KeyContext::Window, KeyContext::Global].iter())
            .find_map(|ctx| self.0.get(&(chord.clone(), ctx.clone())).cloned())
    }
}

/// The conflict rules, against any keymap. Empty = all hold.
fn conflict_problems(m: &mut dyn Bindable) -> Vec<String> {
    let mut problems = Vec::new();
    fn expect_conflict(
        problems: &mut Vec<String>,
        m: &mut dyn Bindable,
        chord: &str,
        ctx: KeyContext,
        action: &str,
        existing: &str,
        why: &str,
    ) {
        match m.bind(c(chord), ctx, action) {
            Err(EditorError::KeyConflict {
                existing: e, new, ..
            }) if e == existing && new == action => {}
            other => problems.push(format!(
                "{why}: binding {chord} to {action} gave {other:?}, want a conflict naming {existing} and {action}"
            )),
        }
    }
    if let Err(e) = m.bind(c("Ctrl+S"), KeyContext::Window, "forge.file.save") {
        return vec![format!("the first binding failed: {e}")];
    }
    // Same chord, overlapping contexts (window vs a panel inside it).
    expect_conflict(
        &mut problems,
        m,
        "Ctrl+S",
        panel("forge.console"),
        "forge.console.save_log",
        "forge.file.save",
        "window vs panel",
    );
    // Same chord, same context.
    expect_conflict(
        &mut problems,
        m,
        "Ctrl+S",
        KeyContext::Window,
        "forge.layout.save",
        "forge.file.save",
        "same context",
    );
    // Global overlaps everything.
    expect_conflict(
        &mut problems,
        m,
        "Ctrl+S",
        KeyContext::Global,
        "forge.snapshot",
        "forge.file.save",
        "global vs window",
    );
    // The earlier binding still resolves: the conflict changed nothing.
    if m.action_at("Ctrl+S", &[panel("forge.console")]).as_deref() != Some("forge.file.save") {
        problems.push("a refused binding changed what Ctrl+S runs".into());
    }
    // Two different panels are never focused together: both may use F2.
    for (p, a) in [
        ("forge.hierarchy", "forge.hierarchy.rename"),
        ("forge.assets", "forge.assets.rename"),
    ] {
        if let Err(e) = m.bind(c("F2"), panel(p), a) {
            problems.push(format!("F2 in {p} was refused: {e}"));
        }
    }
    if m.action_at("F2", &[panel("forge.assets")]).as_deref() != Some("forge.assets.rename") {
        problems.push("F2 in the assets panel does not rename assets".into());
    }
    if m.action_at("F2", &[panel("forge.hierarchy")]).as_deref() != Some("forge.hierarchy.rename") {
        problems.push("F2 in the hierarchy does not rename entities".into());
    }
    // A two-stroke chord and its own first stroke are ambiguous.
    if let Err(e) = m.bind(
        c("Ctrl+K Ctrl+S"),
        KeyContext::Window,
        "forge.layout.save_as",
    ) {
        problems.push(format!("Ctrl+K Ctrl+S was refused: {e}"));
    }
    expect_conflict(
        &mut problems,
        m,
        "Ctrl+K",
        KeyContext::Global,
        "forge.kill_line",
        "forge.layout.save_as",
        "prefix",
    );
    if m.action_at("Ctrl+K Ctrl+S", &[]).as_deref() != Some("forge.layout.save_as") {
        problems.push("the two-stroke chord does not resolve".into());
    }
    problems
}

#[test]
fn test_keybinding_conflicts() {
    let problems = conflict_problems(&mut KeyMap::new());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn positive_control_a_last_wins_map_fails() {
    let problems = conflict_problems(&mut LastWins::default());
    assert!(
        problems.iter().any(|p| p.contains("window vs panel")),
        "a last-wins map passed the conflict checks: {problems:#?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("changed what Ctrl+S runs")),
        "{problems:#?}"
    );
}

#[test]
fn the_conflict_message_names_both_actions_and_contexts() {
    let mut m = KeyMap::new();
    m.bind(c("Delete"), KeyContext::Window, "forge.edit.delete")
        .unwrap_or_else(|e| panic!("{e}"));
    let e = m
        .bind(
            c("Delete"),
            KeyContext::Role("TextInput".into()),
            "forge.text.delete",
        )
        .expect_err("conflict");
    assert_eq!(e.code(), "EDITOR-0001");
    let text = e.to_string();
    for part in [
        "Delete",
        "forge.edit.delete",
        "forge.text.delete",
        "TextInput",
        "editor windows",
    ] {
        assert!(text.contains(part), "{part:?} missing from {text:?}");
    }
    // Overriding deliberately, by naming the shadowed action, is allowed and wins inside.
    m.bind_shadowing(
        c("Delete"),
        KeyContext::Role("TextInput".into()),
        "forge.text.delete",
        "forge.edit.delete",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let ev = KeyEvent::press(KeyCode::Delete, Modifiers::NONE);
    assert_eq!(
        m.resolve(
            &ev,
            &[KeyContext::Role("TextInput".into()), panel("forge.console")]
        ),
        Resolution::Action("forge.text.delete".into())
    );
    assert_eq!(
        m.resolve(&ev, &[KeyContext::Role("Button".into())]),
        Resolution::Action("forge.edit.delete".into())
    );
    // Naming the wrong action is not an acknowledgement.
    assert!(
        m.bind_shadowing(c("Ctrl+S"), panel("x"), "a", "b").is_ok(),
        "no binding to shadow: fine"
    );
    let mut m2 = KeyMap::new();
    m2.bind(c("Ctrl+D"), KeyContext::Window, "forge.duplicate")
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        m2.bind_shadowing(
            c("Ctrl+D"),
            panel("forge.console"),
            "forge.console.detach",
            "forge.other"
        )
        .is_err()
    );
}

#[test]
fn rebinding_moves_a_chord_and_refuses_a_taken_one() {
    let mut m = KeyMap::new();
    m.bind(c("Ctrl+S"), KeyContext::Window, "forge.file.save")
        .unwrap_or_else(|e| panic!("{e}"));
    m.bind(c("Ctrl+P"), KeyContext::Window, "forge.play.toggle")
        .unwrap_or_else(|e| panic!("{e}"));
    m.rebind("forge.play.toggle", KeyContext::Window, c("F5"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(m.chords_for("forge.play.toggle"), vec![&c("F5")]);
    let e = m
        .rebind("forge.play.toggle", KeyContext::Window, c("Ctrl+S"))
        .expect_err("taken");
    assert!(e.to_string().contains("forge.file.save"), "{e}");
    assert_eq!(
        m.chords_for("forge.play.toggle"),
        vec![&c("F5")],
        "unchanged on conflict"
    );
}

#[test]
fn a_user_layer_colliding_with_a_default_is_reported_and_skipped() {
    let defaults = default_keymap().unwrap_or_else(|e| panic!("{e}"));
    let user = KeymapFile::from_ron(
        r#"(version: 1,
            bindings: [
                (chord: "Ctrl+S", context: Window, action: "forge.play.toggle"),
                (chord: "F5", context: Window, action: "forge.file.save"),
            ],
            unbind: [])"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let (m, errs) = KeyMap::from_layers(&defaults, &[&user]);
    // Ctrl+S for play collides with save's default: reported, naming both, and skipped.
    assert_eq!(errs.len(), 1, "{errs:?}");
    let t = errs[0].to_string();
    assert!(
        t.contains("forge.play.toggle") && t.contains("forge.file.save"),
        "{t}"
    );
    assert_eq!(m.chords_for("forge.play.toggle"), vec![&c("Ctrl+P")]);
    // F5 for save rebinds save (its default chord moves) — no conflict.
    assert_eq!(m.chords_for("forge.file.save"), vec![&c("F5")]);
    assert!(m.audit().is_empty());
    // Unbinding first makes room.
    let user2 = KeymapFile::from_ron(
        r#"(version: 1,
            unbind: [ (chord: "Ctrl+S", context: Window) ],
            bindings: [ (chord: "Ctrl+S", context: Window, action: "forge.play.toggle") ])"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let (m2, errs2) = KeyMap::from_layers(&defaults, &[&user2]);
    assert!(errs2.is_empty(), "{errs2:?}");
    assert_eq!(m2.chords_for("forge.play.toggle"), vec![&c("Ctrl+S")]);
    assert!(m2.chords_for("forge.file.save").is_empty());
}

#[test]
fn every_preset_keymap_layers_cleanly_over_the_defaults() {
    let defaults = default_keymap().unwrap_or_else(|e| panic!("{e}"));
    for p in forge_editor::presets::builtin_presets().unwrap_or_else(|e| panic!("{e}")) {
        let layers: Vec<&KeymapFile> = p.keymap.iter().collect();
        let (m, errs) = KeyMap::from_layers(&defaults, &layers);
        assert!(errs.is_empty(), "{}: {errs:?}", p.workspace.id);
        assert!(m.audit().is_empty());
        let text = m.to_file().to_ron().unwrap_or_default();
        let back = KeymapFile::from_ron(&text).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back.bindings, m.bindings(), "keymaps round-trip");
    }
}

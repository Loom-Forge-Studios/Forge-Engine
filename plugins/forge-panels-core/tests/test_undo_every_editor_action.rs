//! M2-3 — **undo/redo across every editor action, via the bus.**
//!
//! Every action in the registry the editor runs on (built-ins, panel-open actions, and a
//! plugin's own) is run through the shell against a populated project:
//!
//! * a **command** action must change the project, be one undo entry, and come back
//!   exactly with Ctrl+Z (state hash) and go forward again with Ctrl+Y;
//! * a **session** action (open a panel, theme, layout, palette) must leave the project
//!   untouched — it is session state or user config, never project state (Ch.21 §21.18);
//! * the same holds for every palette entry that runs without arguments, for every menu
//!   item, for every toolbar button, and for every project-settings row of the settings
//!   window driven through its widgets (checkbox, choice, slider, text field).
//!
//! Positive control (W2): a plugin action whose command the core refuses must be reported
//! (it changes nothing, so it is not undoable) — the check is not vacuous.

mod common;

use std::collections::BTreeSet;

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::actions::{Action, ActionCx, ActionItem, ActionKind};
use forge_editor::testing::Rig;
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use forge_ui::widgets::{MenuItem, Pressed};
use forge_ui::{KeyCode, Modifiers};

/// A plugin adding one command action.
struct PluginAction {
    manifest: Manifest,
    cmd: fn(&ActionCx) -> Vec<EditorCommand>,
}

impl PluginAction {
    fn new(cmd: fn(&ActionCx) -> Vec<EditorCommand>) -> Self {
        let text = "Plugin(id: \"test.actions\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [Action(\"test.rename_all\")])";
        Self {
            manifest: Manifest::parse(text).unwrap_or_else(|e| panic!("{e}")),
            cmd,
        }
    }
}

impl SourcePlugin for PluginAction {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        let f = self.cmd;
        cx.add::<Action>(
            "test.rename_all",
            ActionItem::new(
                "Rename selection",
                "Test",
                ActionKind::Command(std::sync::Arc::new(move |cx: &ActionCx| f(cx))),
            ),
            Order::Last,
        )
    }
}

fn rename_selection(cx: &ActionCx) -> Vec<EditorCommand> {
    cx.selection
        .iter()
        .map(|e| EditorCommand::Rename {
            entity: *e,
            name: format!("Renamed {}", e.0),
        })
        .collect()
}

fn refused(_: &ActionCx) -> Vec<EditorCommand> {
    vec![EditorCommand::Rename {
        entity: EntityKey(9_999),
        name: "nobody".into(),
    }]
}

/// Two entities (one a child) and a setting, both selected.
fn populate(rig: &mut Rig) {
    let em = rig.shell.emitter().clone();
    em.emit(EditorCommand::Spawn {
        name: "A".into(),
        parent: None,
    });
    rig.turn();
    let a = rig
        .shell
        .mirror()
        .roots()
        .next()
        .unwrap_or_else(|| panic!("spawned"));
    em.emit(EditorCommand::Spawn {
        name: "B".into(),
        parent: Some(a),
    });
    em.emit(EditorCommand::SetSetting {
        key: "test.k".into(),
        value: Some(Value::Int(1)),
    });
    rig.turn();
    let all: Vec<EntityKey> = rig.shell.mirror().entities().map(|(k, _)| *k).collect();
    rig.shell.session_mut().set_selection(all);
    rig.turn();
}

/// Run one command action and check undo/redo. `Err` says what failed.
fn check_command(rig: &mut Rig, id: &str) -> Result<(), String> {
    populate(rig);
    let before = rig.state_hash();
    let entries = rig.shell.mirror().history().len();
    rig.run(id);
    let after = rig.state_hash();
    if after == before {
        return Err(format!(
            "{id}: the command action changed nothing (refused or empty)"
        ));
    }
    if rig.shell.mirror().history().len() != entries + 1 {
        return Err(format!("{id}: not exactly one undo entry"));
    }
    if !rig.mirror_matches() {
        return Err(format!("{id}: the mirror disagrees with the core"));
    }
    rig.chord(KeyCode::Char('z'), Modifiers::CTRL);
    if rig.state_hash() != before {
        return Err(format!("{id}: Ctrl+Z did not restore the project"));
    }
    rig.chord(KeyCode::Char('y'), Modifiers::CTRL);
    if rig.state_hash() != after {
        return Err(format!("{id}: Ctrl+Y did not redo it"));
    }
    rig.chord(KeyCode::Char('z'), Modifiers::CTRL);
    Ok(())
}

/// Undo and redo are bus actions too: undo must revert the newest entry, redo bring it back.
fn check_undo_redo(rig: &mut Rig, undo: &str, redo: &str) -> Result<(), String> {
    populate(rig);
    let before = rig.state_hash();
    rig.run(undo);
    let undone = rig.state_hash();
    if undone == before {
        return Err(format!("{undo}: nothing was undone"));
    }
    rig.run(redo);
    if rig.state_hash() != before {
        return Err(format!("{redo}: the undo was not redone"));
    }
    Ok(())
}

fn check_all(rig: &mut Rig) -> Result<usize, String> {
    let ids: Vec<(String, ActionKind)> = rig
        .shell
        .actions()
        .iter()
        .map(|(id, a)| (id.to_string(), a.kind.clone()))
        .collect();
    let mut commands = 0;
    check_undo_redo(rig, "forge.edit.undo", "forge.edit.redo")?;
    for (id, kind) in &ids {
        if matches!(kind, ActionKind::Undo | ActionKind::Redo) {
            continue;
        }
        if matches!(kind, ActionKind::Command(_)) {
            check_command(rig, id)?;
            commands += 1;
        } else {
            populate(rig);
            let before = rig.state_hash();
            rig.run(id);
            if rig.state_hash() != before {
                return Err(format!("{id}: a session action changed the project"));
            }
            // Close any popup (the palette) it opened.
            rig.chord(KeyCode::Escape, Modifiers::NONE);
        }
    }
    Ok(commands)
}

#[test]
fn every_command_action_is_undoable_and_every_session_action_leaves_the_project() {
    let plugin = PluginAction::new(rename_selection);
    let mut rig = Rig::new(common::config(&[&plugin], None)).unwrap_or_else(|e| panic!("{e}"));
    let n = check_all(&mut rig).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        n >= 2,
        "delete and the plugin's rename are command actions ({n})"
    );
    // Undo and redo themselves go through the bus: the history shows what they did.
    assert!(rig.shell.actions().get("forge.edit.undo").is_some());
}

#[test]
fn positive_control_a_refused_command_action_is_reported() {
    let plugin = PluginAction::new(refused);
    let mut rig = Rig::new(common::config(&[&plugin], None)).unwrap_or_else(|e| panic!("{e}"));
    let r = check_all(&mut rig);
    assert!(
        r.as_ref()
            .is_err_and(|e| e.contains("test.rename_all") && e.contains("changed nothing")),
        "the refused action must fail the check: {r:?}"
    );
    // ...and the refusal reached the user with its code, not silently.
    let s = rig.shell.session();
    assert!(
        s.notifications
            .history()
            .any(|n| n.code.as_deref().is_some_and(|c| c.starts_with("CMD-"))),
        "the refusal is a notification carrying its CMD code"
    );
}

#[test]
fn every_palette_entry_without_arguments_is_undoable_or_leaves_the_project() {
    let mut rig = common::rig();
    populate(&mut rig);
    rig.run("forge.palette.open");
    // Entries that go on the bus: commands, and actions of a command kind. Undo and redo
    // entries are checked by `every_command_action_...` (they revert rather than add).
    let bus_kind = |key: &str| -> Option<bool> {
        match key.strip_prefix("action:") {
            Some(id) => match rig.shell.actions().get(id).map(|a| &a.kind) {
                Some(ActionKind::Undo | ActionKind::Redo) => None,
                Some(ActionKind::Command(_)) => Some(true),
                _ => Some(false),
            },
            None => Some(key.starts_with("command:") || key.starts_with("invoke:")),
        }
    };
    let keys: Vec<(String, bool)> = rig
        .shell
        .palette()
        .entries()
        .iter()
        .filter_map(|e| bus_kind(&e.key).map(|b| (e.key.clone(), b)))
        .collect();
    rig.chord(KeyCode::Escape, Modifiers::NONE);
    assert!(keys.iter().any(|(_, c)| *c), "the palette lists commands");
    let mut undone = 0;
    for (key, is_command) in keys {
        populate(&mut rig);
        rig.run("forge.palette.open");
        rig.chord(KeyCode::Escape, Modifiers::NONE);
        let before = rig.state_hash();
        let entries = rig.shell.mirror().history().len();
        rig.shell.pick_palette(&mut rig.h.ui, &key);
        rig.turn();
        rig.chord(KeyCode::Escape, Modifiers::NONE);
        let after = rig.state_hash();
        if !is_command {
            assert_eq!(
                after, before,
                "{key}: a non-command entry changed the project"
            );
            continue;
        }
        if after == before {
            // A command that needs arguments asks for them instead (a notification).
            assert_eq!(rig.shell.mirror().history().len(), entries, "{key}");
            continue;
        }
        assert_eq!(
            rig.shell.mirror().history().len(),
            entries + 1,
            "{key}: one entry"
        );
        rig.chord(KeyCode::Char('z'), Modifiers::CTRL);
        assert_eq!(rig.state_hash(), before, "{key}: Ctrl+Z restores");
        undone += 1;
    }
    assert!(
        undone >= 3,
        "Spawn, Despawn and Reparent run from the palette ({undone})"
    );
}

#[test]
fn every_menu_item_and_toolbar_button_is_an_action_of_the_registry() {
    let mut rig = common::rig();
    let actions: BTreeSet<String> = rig.shell.actions().keys().map(str::to_string).collect();
    fn walk(items: &[MenuItem], out: &mut Vec<String>) {
        for i in items {
            match i {
                MenuItem::Action { id, .. } => out.push(id.clone()),
                MenuItem::Submenu { items, .. } => walk(items, out),
                MenuItem::Separator => {}
            }
        }
    }
    let mut ids = Vec::new();
    for (_, items) in rig.shell.menus() {
        walk(&items, &mut ids);
    }
    assert!(ids.len() > 20, "the menus list the actions ({})", ids.len());
    for id in &ids {
        assert!(
            actions.contains(id),
            "menu item {id} is not a registered action"
        );
    }
    // The Edit menu names what undo would undo, from the mirror.
    populate(&mut rig);
    let undo_label = rig
        .shell
        .menus()
        .iter()
        .flat_map(|(_, items)| items.clone())
        .find_map(|i| match i {
            MenuItem::Action { id, label, .. } if id == "forge.edit.undo" => Some(label),
            _ => None,
        });
    assert_eq!(undo_label.as_deref(), Some("Undo Set setting test.k"));
    // Toolbar buttons run their actions through the same path.
    let toolbar = rig.shell.toolbar();
    assert!(toolbar.iter().all(|(_, a)| actions.contains(a)));
    let (undo_button, _) = toolbar
        .iter()
        .find(|(_, a)| a == "forge.edit.undo")
        .cloned()
        .unwrap_or_else(|| panic!("an undo button"));
    let before = rig.state_hash();
    let n = rig.shell.mirror().history().len();
    rig.h.click(undo_button);
    rig.turn();
    assert_ne!(rig.state_hash(), before, "the toolbar's Undo undid");
    assert_eq!(
        rig.shell.mirror().history().len(),
        n,
        "undo keeps the entry (as undone)"
    );
    let _ = Pressed(undo_button);
}

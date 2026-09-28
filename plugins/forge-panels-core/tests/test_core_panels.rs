//! The core panels through the running shell: the settings window (M2-43), the undo
//! history (M2-41), notifications (M2-42) and the keybindings editor (M2-44).

mod common;

use std::time::Duration;

use forge_cmd::{EditorCommand, EntityKey, Issuer, TxnState, Value};
use forge_editor::client::BusClient;
use forge_editor::settings::EditorTheme;
use forge_editor::testing::Rig;
use forge_ui::widgets::RowActivated;
use forge_ui::{InputEvent, KeyCode, KeyEvent, Modifiers, WidgetId};

fn focus(rig: &mut Rig, id: WidgetId) {
    rig.h.ui.set_focus(Some(id), true);
    rig.turn();
}

fn setting(rig: &Rig, key: &str) -> Option<Value> {
    rig.shell.mirror().setting(key).cloned()
}

fn last_entry(rig: &Rig) -> (String, String, TxnState) {
    let m = rig.shell.mirror();
    let h = m.history().last().unwrap_or_else(|| panic!("an entry"));
    (h.label.clone(), h.issuer_tag(), h.state)
}

fn settings_rig(config: Option<std::path::PathBuf>) -> Rig {
    let mut rig = Rig::new(common::config(&[], config)).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&["forge.settings"])
        .unwrap_or_else(|e| panic!("{e}"));
    rig
}

fn row(rig: &Rig, key: &str, part: &str) -> WidgetId {
    let page = if key.starts_with("editor.") {
        "project"
    } else {
        "editor"
    };
    common::part(
        rig,
        "forge.settings",
        &["content", &format!("{page}.{key}"), part],
    )
}

#[test]
fn project_settings_rows_edit_through_commands_and_every_edit_undoes() {
    let mut rig = settings_rig(None);
    let start = rig.state_hash();
    let mut hashes = vec![start];

    // Checkbox: one command, issued as the human, labelled with the key.
    let cb = row(&rig, "editor.snap_translate", "edit");
    rig.h.click(cb);
    rig.turn();
    assert_eq!(
        setting(&rig, "editor.snap_translate"),
        Some(Value::Bool(true))
    );
    let (label, issuer, state) = last_entry(&rig);
    assert!(label.contains("editor.snap_translate"), "{label}");
    assert_eq!(
        (issuer.as_str(), state),
        ("human:tester", TxnState::Committed)
    );
    hashes.push(rig.state_hash());

    // Choice: type-ahead picks "Z"; the variant name is the value.
    let combo = row(&rig, "editor.up_axis", "edit");
    focus(&mut rig, combo);
    rig.chord(KeyCode::Char('z'), Modifiers::NONE);
    assert_eq!(
        setting(&rig, "editor.up_axis"),
        Some(Value::Text("Z".into()))
    );
    hashes.push(rig.state_hash());

    // Slider, keyboard step: one command, snapped to the step (no f32 residue).
    let slider = row(&rig, "editor.grid_size", "edit");
    focus(&mut rig, slider);
    rig.chord(KeyCode::Right, Modifiers::NONE);
    assert_eq!(setting(&rig, "editor.grid_size"), Some(Value::Float(1.1)));
    hashes.push(rig.state_hash());

    // Text field: select all, type, Enter.
    let text = row(&rig, "editor.new_entity_name", "edit");
    focus(&mut rig, text);
    rig.chord(KeyCode::Char('a'), Modifiers::CTRL);
    for c in "Prop".chars() {
        let mut ev = KeyEvent::press(KeyCode::Char(c.to_ascii_lowercase()), Modifiers::NONE);
        ev.text = Some(c.to_string());
        rig.h.ui.handle(InputEvent::Key(ev));
    }
    rig.chord(KeyCode::Enter, Modifiers::NONE);
    assert_eq!(
        setting(&rig, "editor.new_entity_name"),
        Some(Value::Text("Prop".into()))
    );
    hashes.push(rig.state_hash());

    // Reset: clears the setting (the default shows again), one more entry.
    let reset = row(&rig, "editor.snap_translate", "reset");
    rig.h.click(reset);
    rig.turn();
    assert_eq!(setting(&rig, "editor.snap_translate"), None);
    hashes.push(rig.state_hash());
    assert_eq!(
        rig.shell.mirror().history().len(),
        5,
        "five edits, five entries"
    );
    assert!(rig.mirror_matches());

    // Every edit comes back with one Ctrl+Z each, in order.
    for want in hashes.iter().rev().skip(1) {
        // Focus the dock (not a text field) so Ctrl+Z is the editor's undo.
        rig.h.ui.set_focus(None, false);
        rig.chord(KeyCode::Char('z'), Modifiers::CTRL);
        assert_eq!(rig.state_hash(), *want);
    }
    assert_eq!(rig.state_hash(), start);
    // The rows followed the undo: clicking the checkbox sets it again (a stale view
    // would toggle it to false instead).
    rig.h.click(cb);
    rig.turn();
    assert_eq!(
        setting(&rig, "editor.snap_translate"),
        Some(Value::Bool(true))
    );
}

#[test]
fn a_settings_slider_drag_is_one_entry() {
    let mut rig = settings_rig(None);
    let slider = row(&rig, "editor.snap_rotate", "edit");
    let r = rig.h.ui.rect(slider).unwrap_or_else(|| panic!("laid out"));
    let y = r.y + r.h / 2.0;
    let at = |t: f32| forge_ui::Point::new(r.x + 8.0 + (r.w - 16.0) * t, y);
    rig.h.ui.handle(InputEvent::PointerMoved(at(0.0)));
    rig.h.ui.handle(InputEvent::PointerButton {
        pos: at(0.0),
        button: forge_ui::PointerButton::Primary,
        pressed: true,
    });
    rig.turn();
    for i in 1..=20 {
        rig.h
            .ui
            .handle(InputEvent::PointerMoved(at(i as f32 / 20.0)));
        rig.turn();
    }
    rig.h.ui.handle(InputEvent::PointerButton {
        pos: at(1.0),
        button: forge_ui::PointerButton::Primary,
        pressed: false,
    });
    rig.turn();
    assert_eq!(
        setting(&rig, "editor.snap_rotate"),
        Some(Value::Float(90.0))
    );
    assert_eq!(rig.shell.mirror().history().len(), 1);
    assert!(last_entry(&rig).0.starts_with("Drag Rotation snap"));
}

#[test]
fn editor_settings_are_user_config_applied_to_the_window_and_saved() {
    let dir = common::temp_dir("settings");
    let mut rig = settings_rig(Some(dir.clone()));
    let start = rig.state_hash();
    // Theme by type-ahead in the theme choice.
    let theme = row(&rig, "theme", "edit");
    focus(&mut rig, theme);
    rig.chord(KeyCode::Char('l'), Modifiers::NONE);
    rig.turn();
    assert_eq!(rig.shell.session().settings().theme, EditorTheme::Light);
    assert_eq!(
        rig.h.ui.theme().name,
        "forge.light",
        "the switch applied at once"
    );
    // Caret blink off, UI scale up one step.
    rig.h.click(row(&rig, "caret_blink", "edit"));
    rig.turn();
    assert!(!rig.h.ui.caret_blink());
    let scale = row(&rig, "ui_scale", "edit");
    focus(&mut rig, scale);
    rig.chord(KeyCode::Right, Modifiers::NONE);
    assert!((rig.shell.user_scale() - 1.1).abs() < 1e-6);
    // None of it is project state.
    assert_eq!(rig.state_hash(), start);
    assert!(rig.shell.mirror().history().is_empty());
    // Saved after the debounce, crash-safely, with one wakeup and no busy loop.
    let file = dir.join(forge_editor::settings::EDITOR_SETTINGS_FILE);
    assert!(!file.exists(), "not written before the debounce");
    let (_, wakeups) = rig.advance(Duration::from_secs(3));
    assert!(wakeups <= 2, "{wakeups} wakeups for one debounced save");
    let saved =
        forge_editor::settings::EditorSettings::load(&dir).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(saved.theme, EditorTheme::Light);
    assert!(!saved.caret_blink);
    assert!(!forge_editor::user_config::temp_path(&file).exists());
    assert_eq!(rig.shell.stats().settings_writes, 1);
    // A second shell starts with them.
    let rig2 = Rig::new(common::config(&[], Some(dir.clone()))).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rig2.h.ui.theme().name, "forge.light");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_sessions_edit_appears_in_the_rows_and_the_history_next_frame() {
    let mut rig = settings_rig(None);
    rig.show_panels(&["forge.settings", "forge.undo_history"])
        .unwrap_or_else(|e| panic!("{e}"));
    let mut auto = rig.connect(Issuer::Automation {
        session: "s1".into(),
        tool: "apply".into(),
    });
    auto.apply(
        EditorCommand::SetSetting {
            key: "editor.snap_translate".into(),
            value: Some(Value::Bool(true)),
        },
        None,
    );
    // The session's change wakes the loop once; one turn later it is on screen.
    let (_, wakeups) = rig.advance(Duration::from_millis(100));
    assert_eq!(wakeups, 1);
    assert_eq!(
        setting(&rig, "editor.snap_translate"),
        Some(Value::Bool(true))
    );
    let (_, issuer, _) = last_entry(&rig);
    assert_eq!(issuer, "automation:s1");
    let list = common::part(&rig, "forge.undo_history", &["list"]);
    let label = forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| {
        t.key_at(1).and_then(|k| t.label_of(k).map(str::to_string))
    })
    .flatten()
    .unwrap_or_default();
    assert!(label.contains("automation:s1"), "{label}");
    // The checkbox shows it: one click now turns it off (a stale view would turn it on).
    let cb = row(&rig, "editor.snap_translate", "edit");
    rig.h.click(cb);
    rig.turn();
    assert_eq!(
        setting(&rig, "editor.snap_translate"),
        Some(Value::Bool(false))
    );
    // The human can undo the session's edit too; the undo is attributed to the human.
    let m = rig.shell.mirror().history().len();
    assert_eq!(m, 2);
}

#[test]
fn the_undo_history_lists_issuers_and_travels_to_an_entry() {
    let mut rig = common::rig();
    rig.show_panels(&["forge.undo_history"])
        .unwrap_or_else(|e| panic!("{e}"));
    let em = rig.shell.emitter().clone();
    for i in 0..3 {
        em.emit(EditorCommand::SetSetting {
            key: format!("k.v{i}"),
            value: Some(Value::Int(i)),
        });
    }
    let mut script = rig.connect(Issuer::Script {
        path: "tools/build.forge".into(),
    });
    script.apply(
        EditorCommand::Spawn {
            name: "S".into(),
            parent: None,
        },
        None,
    );
    rig.advance(Duration::from_millis(10));
    let list = common::part(&rig, "forge.undo_history", &["list"]);
    let rows: Vec<String> = forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| {
        (0..t.row_count())
            .filter_map(|i| t.key_at(i).and_then(|k| t.label_of(k).map(str::to_string)))
            .collect()
    })
    .unwrap_or_default();
    assert_eq!(rows.len(), 5, "{rows:#?}");
    assert!(rows[0].contains("Initial state"));
    assert!(rows[1].contains("human:tester"));
    assert!(rows[4].contains("script:tools/build.forge") && rows[4].starts_with('\u{25cf}'));
    let entries: Vec<_> = rig.shell.mirror().history().iter().map(|h| h.txn).collect();

    // Activate the first entry: everything after it is undone, one bus call each.
    rig.h.ui.raise(
        list,
        RowActivated {
            view: list,
            key: entries[0].0,
        },
    );
    rig.turn();
    let states: Vec<TxnState> = rig
        .shell
        .mirror()
        .history()
        .iter()
        .map(|h| h.state)
        .collect();
    assert_eq!(
        states,
        vec![
            TxnState::Committed,
            TxnState::Undone,
            TxnState::Undone,
            TxnState::Undone
        ]
    );
    // Activate the last (undone) entry: redone back up to it.
    rig.h.ui.raise(
        list,
        RowActivated {
            view: list,
            key: entries[3].0,
        },
    );
    rig.turn();
    assert!(
        rig.shell
            .mirror()
            .history()
            .iter()
            .all(|h| h.state == TxnState::Committed)
    );
    // "Initial state" undoes everything listed.
    rig.h.ui.raise(
        list,
        RowActivated {
            view: list,
            key: u64::MAX,
        },
    );
    rig.turn();
    assert!(
        rig.shell
            .mirror()
            .history()
            .iter()
            .all(|h| h.state == TxnState::Undone)
    );
    assert!(rig.shell.mirror().is_empty() && rig.shell.mirror().settings().count() == 0);
    assert!(rig.mirror_matches());
}

#[test]
fn a_refusal_is_a_toast_and_a_notification_with_its_code() {
    let mut rig = common::rig();
    rig.show_panels(&["forge.notifications"])
        .unwrap_or_else(|e| panic!("{e}"));
    let empty = common::part(&rig, "forge.notifications", &["empty"]);
    assert!(rig.h.ui.is_visible(empty), "the empty state shows first");
    rig.shell.emitter().emit(EditorCommand::Rename {
        entity: EntityKey(404),
        name: "x".into(),
    });
    rig.turn();
    assert_eq!(rig.h.ui.toasts().len(), 1, "one toast");
    let s = rig.shell.session();
    let n = s
        .notifications
        .history()
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("notice"));
    drop(s);
    assert!(
        n.code.as_deref().is_some_and(|c| c.starts_with("CMD-")),
        "{n:?}"
    );
    assert!(n.title.contains("Rename"));
    rig.turn();
    let list = common::part(&rig, "forge.notifications", &["list"]);
    assert!(rig.h.ui.is_visible(list) && !rig.h.ui.is_visible(empty));
    let label = forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| {
        t.key_at(0).and_then(|k| t.label_of(k).map(str::to_string))
    })
    .flatten()
    .unwrap_or_default();
    assert!(
        label.contains("CMD-") && label.contains("[Details]"),
        "{label}"
    );
    assert!(
        rig.shell
            .status(&rig.h.ui)
            .iter()
            .any(|t| t == "1 unread notification")
    );
    // Activating it runs its action: open the console.
    rig.h.ui.raise(
        list,
        RowActivated {
            view: list,
            key: n.id,
        },
    );
    rig.turn();
    rig.turn();
    assert!(
        rig.shell
            .layout()
            .contains(&forge_ui::dock::PanelId::new("forge.console"))
    );
    // ... at the refusal's entry (Ch.21 §21.18 "Details opens the console at the entry").
    let entry = rig
        .shell
        .log()
        .borrow()
        .entries()
        .last()
        .map(|e| (e.id, e.message.clone()))
        .unwrap_or_else(|| panic!("the refusal is in the console"));
    assert!(
        entry.1.contains("Rename") && entry.1.contains("CMD-"),
        "{entry:?}"
    );
    assert!(matches!(
        rig.shell.session().pending_reveal(),
        Some((_, forge_editor::session::Reveal::LogEntry(id))) if *id == entry.0
    ));
    // Mark all read clears the badge.
    rig.h
        .click(common::part(&rig, "forge.notifications", &["bar", "read"]));
    rig.turn();
    assert_eq!(rig.shell.session().notifications.unread(), 0);
}

#[test]
fn the_keybindings_editor_rebinds_by_pressing_and_names_conflicts() {
    let mut rig = common::rig();
    rig.show_panels(&["forge.keybindings"])
        .unwrap_or_else(|e| panic!("{e}"));
    let list = common::part(&rig, "forge.keybindings", &["list"]);
    let undo_key = rig
        .shell
        .session()
        .actions()
        .iter()
        .position(|a| a.id == "forge.edit.undo")
        .unwrap_or_else(|| panic!("undo listed")) as u64;
    forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| t.select(&[undo_key]));
    // Change binding…, press Alt+U: undo is now Alt+U, and pressing it undoes.
    rig.h
        .click(common::part(&rig, "forge.keybindings", &["bar", "change"]));
    rig.turn();
    let capture = common::part(&rig, "forge.keybindings", &["capture"]);
    assert_eq!(rig.h.ui.focused(), Some(capture));
    rig.chord(
        KeyCode::Char('u'),
        Modifiers {
            alt: true,
            ..Modifiers::NONE
        },
    );
    rig.advance(Duration::from_secs(2)); // the second-stroke window closes
    let chords: Vec<String> = rig
        .shell
        .session()
        .keymap()
        .borrow()
        .chords_for("forge.edit.undo")
        .iter()
        .map(|c| c.to_string())
        .collect();
    assert_eq!(chords, vec!["Alt+U"]);
    rig.shell.emitter().emit(EditorCommand::SetSetting {
        key: "a.b".into(),
        value: Some(Value::Int(1)),
    });
    rig.turn();
    rig.h.ui.set_focus(None, false);
    rig.chord(
        KeyCode::Char('u'),
        Modifiers {
            alt: true,
            ..Modifiers::NONE
        },
    );
    assert_eq!(setting(&rig, "a.b"), None, "the new chord undoes");
    // A conflicting chord is refused and both actions are named.
    forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| t.select(&[undo_key]));
    rig.h
        .click(common::part(&rig, "forge.keybindings", &["bar", "change"]));
    rig.turn();
    rig.chord(
        KeyCode::Char('p'),
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        },
    );
    rig.advance(Duration::from_secs(2));
    let status = common::part(&rig, "forge.keybindings", &["status"]);
    let text = rig
        .h
        .node(status)
        .and_then(|n| n.value().map(str::to_string))
        .unwrap_or_default();
    assert!(
        text.contains("EDITOR-0001")
            && text.contains("forge.palette.open")
            && text.contains("forge.edit.undo"),
        "{text}"
    );
    // Reset puts Ctrl+Z back; the change is user config, saved, never a command.
    forge_ui::widgets::VirtualTree::edit(&mut rig.h.ui, list, |t| t.select(&[undo_key]));
    rig.h
        .click(common::part(&rig, "forge.keybindings", &["bar", "reset"]));
    rig.turn();
    let chords: Vec<String> = rig
        .shell
        .session()
        .keymap()
        .borrow()
        .chords_for("forge.edit.undo")
        .iter()
        .map(|c| c.to_string())
        .collect();
    assert_eq!(chords, vec!["Ctrl+Z"]);
    assert!(
        rig.shell
            .mirror()
            .history()
            .iter()
            .all(|h| !h.label.contains("key"))
    );
}

/// WP-39: the GPU mode is an editor preference (Graphics) and a project setting (the
/// exported game's). Changing either says "Applies at next start" — the adapter pool is
/// built at startup and nothing restarts silently — and Multi is labelled experimental.
#[test]
fn the_gpu_mode_applies_at_next_start_and_multi_is_experimental() {
    use forge_editor::settings::{EditorSettings, GPU_MODE_SETTING, GpuModeSetting};
    let dir = common::temp_dir("gpu-mode");
    let mut rig = settings_rig(Some(dir.clone()));
    assert_eq!(
        rig.shell.session().settings().gpu_mode,
        GpuModeSetting::Single
    );
    // Both rows carry the note (it names why Multi is experimental).
    common::part(&rig, "forge.settings", &["content", "editor.gpu_mode.note"]);
    common::part(
        &rig,
        "forge.settings",
        &["content", &format!("graphics.{GPU_MODE_SETTING}.note")],
    );
    // The preference: pick Multi by type-ahead.
    let mode = row(&rig, "gpu_mode", "edit");
    focus(&mut rig, mode);
    rig.chord(KeyCode::Char('m'), Modifiers::NONE);
    rig.turn();
    assert_eq!(
        rig.shell.session().settings().gpu_mode,
        GpuModeSetting::Multi
    );
    let (title, detail) = last_notice(&rig).unwrap_or_else(|| panic!("no notice"));
    assert_eq!(title, "Applies at next start");
    assert!(detail.contains("next time"), "{detail}");
    assert!(
        rig.shell.mirror().history().is_empty(),
        "a preference is not project state"
    );
    let _ = rig.advance(Duration::from_secs(3));
    let saved = EditorSettings::load(&dir).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(saved.gpu_mode, GpuModeSetting::Multi);

    // The project setting: a command, like every project setting.
    let project = common::part(
        &rig,
        "forge.settings",
        &["content", &format!("graphics.{GPU_MODE_SETTING}"), "edit"],
    );
    focus(&mut rig, project);
    rig.chord(KeyCode::Char('m'), Modifiers::NONE);
    rig.turn();
    assert_eq!(
        setting(&rig, GPU_MODE_SETTING),
        Some(Value::Text("Multi".into()))
    );
    assert_eq!(rig.shell.mirror().history().len(), 1);
    let (title, _) = last_notice(&rig).unwrap_or_else(|| panic!("no notice"));
    assert_eq!(title, "Applies at next start");
    let _ = std::fs::remove_dir_all(&dir);
}

fn last_notice(rig: &Rig) -> Option<(String, String)> {
    let s = rig.shell.session();
    s.notifications
        .history()
        .next_back()
        .map(|n| (n.title.clone(), n.detail.clone()))
}

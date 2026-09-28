//! The shell as a bus client (Ch.21 §21.18, Ch.34 §34.2/§34.4; DoD M2-1, M2-28):
//!
//! * the mirror is fed only by the stream, for every issuer, and resyncs from a snapshot
//!   when it falls behind (a `Gap`) instead of drifting;
//! * the headless runner and the GUI are clients of the same core: the same commands give
//!   the same project state hash;
//! * the layout autosave is crash-safe (write-then-rename, a torn temporary file is
//!   ignored) and flushed on exit, and the next start restores it.

use std::time::Duration;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::EditorCore;
use forge_editor::presets::builtin_preset;
use forge_editor::shell::{ShellConfig, assemble};
use forge_editor::stand_in::StandInPanels;
use forge_editor::testing::Rig;
use forge_ui::dock::PanelId;

fn config(dir: Option<std::path::PathBuf>) -> ShellConfig {
    let stand_in = StandInPanels::new().unwrap_or_else(|e| panic!("{e}"));
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    assemble(preset, &[&stand_in], &[], dir).unwrap_or_else(|e| panic!("{e}"))
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("forge-editor-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn the_mirror_resyncs_after_falling_behind_the_stream() {
    let mut rig = Rig::new(config(None)).unwrap_or_else(|e| panic!("{e}"));
    let mut script = rig.connect(Issuer::Script {
        path: "flood.forge".into(),
    });
    // More events than a subscription holds, while the UI does not pump.
    for i in 0..(forge_cmd::DEFAULT_SUBSCRIPTION_CAPACITY + 50) {
        script.apply(
            EditorCommand::SetSetting {
                key: format!("flood.k{i}"),
                value: Some(Value::Int(i as i64)),
            },
            None,
        );
    }
    rig.turn();
    assert!(
        rig.mirror_matches(),
        "after a gap the mirror equals the core"
    );
    assert_eq!(
        rig.shell.mirror().settings().count(),
        forge_cmd::DEFAULT_SUBSCRIPTION_CAPACITY + 50,
        "the dropped early events are in the snapshot"
    );
    assert!(rig.shell.mirror().history().len() > 100);
    assert!(
        rig.shell
            .mirror()
            .history()
            .iter()
            .all(|h| h.issuer_tag() == "script:flood.forge")
    );
    // And it keeps following afterwards.
    script.apply(
        EditorCommand::Spawn {
            name: "After".into(),
            parent: None,
        },
        None,
    );
    rig.advance(Duration::from_millis(10));
    assert!(rig.mirror_matches());
    assert_eq!(rig.shell.mirror().len(), 1);
}

#[test]
fn headless_and_gui_clients_reach_the_same_state() {
    let script = "{\"Spawn\":{\"name\":\"Cube\",\"parent\":null}}\n\
        {\"SetSetting\":{\"key\":\"editor.grid_size\",\"value\":{\"Float\":2.5}}}\n\
        {\"Rename\":{\"entity\":0,\"name\":\"Box\"}}\n\
        undo\n\
        redo\n";
    let headless = EditorCore::new();
    let mut out = Vec::new();
    let sum = forge_editor::headless::run_script(&headless, "s.forge", script.as_bytes(), &mut out)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(sum.refused, 0, "{}", String::from_utf8_lossy(&out));

    let mut rig = Rig::new(config(None)).unwrap_or_else(|e| panic!("{e}"));
    let em = rig.shell.emitter().clone();
    for line in script.lines() {
        match line.trim() {
            "undo" => {
                rig.chord(forge_ui::KeyCode::Char('z'), forge_ui::Modifiers::CTRL);
            }
            "redo" => {
                rig.chord(forge_ui::KeyCode::Char('y'), forge_ui::Modifiers::CTRL);
            }
            json => {
                let cmd: EditorCommand =
                    serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"));
                em.emit(cmd);
                rig.turn();
            }
        }
    }
    assert_eq!(
        rig.state_hash(),
        sum.state_hash,
        "same commands, same project"
    );
    assert!(rig.mirror_matches());
}

#[test]
fn the_layout_autosave_is_crash_safe_flushed_on_exit_and_restored() {
    let dir = temp_dir("autosave");
    let mut rig = Rig::new(config(Some(dir.clone()))).unwrap_or_else(|e| panic!("{e}"));
    rig.run("forge.panel.open.forge.undo_history");
    assert!(
        rig.shell
            .layout()
            .contains(&PanelId::new("forge.undo_history"))
    );
    let file = dir.join("layouts").join("autosave.ron");
    assert!(!file.exists(), "debounced: not written at once");
    // The loop exits before the debounce fires: exit flushes.
    rig.shell.exiting();
    assert!(file.exists(), "flushed on exit");
    assert!(
        !forge_editor::user_config::temp_path(&file).exists(),
        "no torn temporary left"
    );
    let saved = rig.shell.layout().clone();
    drop(rig);

    // A crash mid-write leaves a torn temporary file: the last complete layout loads.
    std::fs::write(
        forge_editor::user_config::temp_path(&file),
        b"(version: 1, main: Spl",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let rig = Rig::new(config(Some(dir.clone()))).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        rig.shell.layout(),
        &saved,
        "the next start restores the autosave"
    );
    assert!(
        rig.shell.session().notifications.is_empty(),
        "a torn temporary is not an error: {:?}",
        rig.shell
            .session()
            .notifications
            .history()
            .collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_user_config_file_is_reported_and_the_editor_still_starts() {
    let dir = temp_dir("bad-config");
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(dir.join("keymap.ron"), b"(version: 99)").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        dir.join(forge_editor::settings::EDITOR_SETTINGS_FILE),
        b"not ron",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let rig = Rig::new(config(Some(dir.clone()))).unwrap_or_else(|e| panic!("{e}"));
    let codes: Vec<String> = rig
        .shell
        .session()
        .notifications
        .history()
        .filter_map(|n| n.code.clone())
        .collect();
    assert!(codes.contains(&"EDITOR-0008".to_string()), "{codes:?}");
    assert!(codes.contains(&"EDITOR-0006".to_string()), "{codes:?}");
    assert_eq!(rig.h.ui.theme().name, "forge.dark", "defaults are used");
    let _ = std::fs::remove_dir_all(&dir);
}

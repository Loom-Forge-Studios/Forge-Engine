//! D-5 for the whole editor shell (`ui_editor_idle_zero_redraw`; acceptance "idle CPU ~0"):
//! the main window with the menu bar, toolbar, dock, status bar and every core panel open,
//! a text field focused and its caret timeout expired, runs 10 s of injected-clock idle
//! with **0 frames and 0 loop wakeups**. A pending debounced save costs exactly one wakeup
//! at its deadline and nothing after. Another client's edit costs one wakeup and one
//! frame.
//!
//! Positive control (W2): the same shell with a panel that animates (a live spinner) must
//! show frames and wakeups in the idle window — the measurement can see work.

mod common;

use std::sync::Arc;
use std::time::Duration;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::panels::PanelCx;
use forge_editor::testing::Rig;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use forge_ui::NodeStyle;
use forge_ui::widgets::Spinner;

const CORE: &[&str] = &[
    "forge.undo_history",
    "forge.notifications",
    "forge.settings",
    "forge.keybindings",
    "forge.command_palette",
];

fn idle_rig(extra: &[&dyn SourcePlugin], panels: &[&str]) -> Rig {
    let mut rig = Rig::new(common::config(extra, None)).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(panels).unwrap_or_else(|e| panic!("{e}"));
    // A focused text field whose caret has stopped blinking (WCAG 2.2.2: after 5 s).
    let text = common::part(
        &rig,
        "forge.settings",
        &["content", "project.editor.new_entity_name", "edit"],
    );
    rig.h.ui.set_focus(Some(text), true);
    rig.turn();
    rig.advance(Duration::from_secs(6));
    // Let the layout autosave (none configured) and anything else settle.
    rig.advance(Duration::from_secs(3));
    rig
}

#[test]
fn ui_editor_idle_zero_redraw() {
    let mut rig = idle_rig(&[], CORE);
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    assert_eq!(
        (frames, wakeups),
        (0, 0),
        "an idle editor draws nothing and never wakes"
    );
    let passes = rig.shell.stats().sync_passes;
    rig.advance(Duration::from_secs(10));
    assert_eq!(
        rig.shell.stats().sync_passes,
        passes,
        "no panel sync work while idle"
    );

    // Another client's edit: one wakeup, the frames to show it, then idle again.
    let mut auto = rig.connect(Issuer::Automation {
        session: "s".into(),
        tool: "apply".into(),
    });
    auto.apply(
        EditorCommand::SetSetting {
            key: "editor.snap_translate".into(),
            value: Some(Value::Bool(true)),
        },
        None,
    );
    let (frames, wakeups) = rig.advance(Duration::from_secs(1));
    assert_eq!(wakeups, 1, "one wakeup for the session's edit");
    assert!(frames >= 1, "the change is drawn");
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    assert_eq!((frames, wakeups), (0, 0), "idle again after showing it");
}

#[test]
fn a_debounced_save_is_one_wakeup_not_a_poll() {
    let dir = common::temp_dir("idle-save");
    let mut rig =
        Rig::new(common::config(&[], Some(dir.clone()))).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(&["forge.undo_history", "forge.settings"])
        .unwrap_or_else(|e| panic!("{e}"));
    rig.advance(Duration::from_secs(5)); // the layout change above is saved
    let writes = rig.shell.stats().layout_writes;
    assert_eq!(writes, 1, "the layout change was autosaved once");
    rig.run("forge.panel.open.forge.keybindings");
    let due = rig
        .shell
        .next_deadline()
        .unwrap_or_else(|| panic!("a save is due"));
    assert!(due > rig.h.now(), "debounced, not immediate");
    let (_, wakeups) = rig.advance(Duration::from_secs(10));
    assert_eq!(wakeups, 1, "exactly one wakeup, at the deadline");
    assert_eq!(rig.shell.stats().layout_writes, writes + 1);
    assert_eq!(rig.shell.next_deadline(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A panel with a live spinner: it animates, so the idle window must see frames.
struct Busy {
    manifest: Manifest,
}

impl SourcePlugin for Busy {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<EditorPanel<PanelCx>>(
            "test.busy",
            PanelDescriptor {
                title: "Busy".into(),
                icon: None,
                default_dock: Dock::Center,
                build: Arc::new(|cx: &mut PanelCx| {
                    cx.add(|b, parent| {
                        let live = b.signal(true);
                        b.add(
                            parent,
                            "spin",
                            NodeStyle::leaf(),
                            Spinner::new(live, "Working"),
                        )?;
                        Ok(())
                    });
                }),
            },
            Order::Last,
        )
    }
}

#[test]
fn positive_control_an_animating_panel_is_seen_by_the_idle_measurement() {
    let busy = Busy {
        manifest: Manifest::parse(
            "Plugin(id: \"test.busy\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [EditorPanel(\"test.busy\")])",
        )
        .unwrap_or_else(|e| panic!("{e}")),
    };
    let mut panels = CORE.to_vec();
    panels.push("test.busy");
    let mut rig = idle_rig(&[&busy], &panels);
    let (frames, wakeups) = rig.advance(Duration::from_secs(10));
    assert!(
        frames > 0 && wakeups > 0,
        "the spinner must be seen: {frames} frames, {wakeups} wakeups"
    );
}

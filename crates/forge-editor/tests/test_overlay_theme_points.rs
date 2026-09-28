//! The `EditorOverlay` and `Theme` extension points in the running editor (Ch.21 §21.19,
//! I16; DoD M2-27): through the ordinary plugin loader, a third-party plugin
//!
//! * **adds** a theme — it gets an action (View menu, palette) and choosing it restyles
//!   the window and is remembered in the editor settings;
//! * **chains** the built-in dark theme — the window uses the chained tokens;
//! * **removes** the toasts overlay — a notification then shows no toast (it is still in
//!   the notification centre) — and **adds** an overlay of its own, built over the dock.
//!
//! Positive control (W2): `positive_control_a_shell_ignoring_the_registry_fails` runs the
//! same checks on a shell assembled without the plugin — the one each check would see if
//! the shell ignored the registry and hard-coded its themes and toasts — and each must fail.

use std::sync::Arc;

use forge_editor::notify::Level;
use forge_editor::overlay::{EditorOverlay, OverlayDescriptor, ThemeItem, ThemePoint};
use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble;
use forge_editor::testing::Rig;
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use forge_ui::widgets::Label;
use forge_ui::{Key, NodeStyle};

struct ThemesAndOverlays(Manifest);

impl ThemesAndOverlays {
    fn new() -> Self {
        Self(
            Manifest::parse(
                r#"Plugin(id: "com.test.look", version: "1.0.0", engine: "^0.1", kind: Source,
                    provides: [ Theme("com.test.solar"), EditorOverlay("com.test.badge") ],
                    chains: [ Theme("forge.dark") ],
                    removes: [ EditorOverlay("forge.overlay.toasts") ])"#,
            )
            .unwrap_or_else(|e| panic!("{e}")),
        )
    }
}

impl SourcePlugin for ThemesAndOverlays {
    fn manifest(&self) -> &Manifest {
        &self.0
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        let mut solar = forge_ui::Theme::light();
        solar.name = "Solar".into();
        cx.add::<ThemePoint>(
            "com.test.solar",
            ThemeItem::new("Solar", solar),
            Order::Last,
        )?;
        cx.chain::<ThemePoint>("forge.dark", |t| {
            let mut tokens = (*t.tokens).clone();
            tokens.name = format!("{} (chained)", tokens.name);
            ThemeItem {
                tokens: Arc::new(tokens),
                ..t
            }
        })?;
        cx.remove::<EditorOverlay>("forge.overlay.toasts")?;
        cx.add::<EditorOverlay>(
            "com.test.badge",
            OverlayDescriptor {
                title: "Badge".into(),
                order: 10,
                build: Arc::new(|cx| {
                    cx.add(|b, parent| {
                        b.add(
                            parent,
                            "badge",
                            NodeStyle::absolute(None, Some(8.0), Some(8.0), None).no_hit_test(),
                            Label::new("BADGE"),
                        )?;
                        Ok(())
                    });
                }),
            },
            Order::Last,
        )
    }
}

fn rig(with_plugin: bool) -> Rig {
    let p = ThemesAndOverlays::new();
    let plugins: Vec<&dyn SourcePlugin> = if with_plugin { vec![&p] } else { vec![] };
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    let cfg = assemble(preset, &plugins, &[], None).unwrap_or_else(|e| panic!("{e}"));
    let mut rig = Rig::new(cfg).unwrap_or_else(|e| panic!("{e}"));
    rig.settle();
    rig
}

fn check_themes(rig: &mut Rig) -> Result<(), String> {
    let dark = rig.h.ui.theme().name.clone();
    if dark != "forge.dark (chained)" && !dark.ends_with("(chained)") {
        return Err(format!("the chained dark theme is not in use: {dark:?}"));
    }
    if rig
        .shell
        .actions()
        .get("forge.theme.com.test.solar")
        .is_none()
    {
        return Err("the plugin theme has no action".into());
    }
    rig.run("forge.theme.com.test.solar");
    rig.settle();
    if rig.h.ui.theme().name != "Solar" {
        return Err(format!(
            "choosing the plugin theme left {:?}",
            rig.h.ui.theme().name
        ));
    }
    if rig.shell.session().settings().theme_override != "com.test.solar" {
        return Err("the choice is not remembered in the editor settings".into());
    }
    rig.run("forge.theme.light");
    rig.settle();
    if !rig.shell.session().settings().theme_override.is_empty() {
        return Err("a built-in choice did not clear the override".into());
    }
    Ok(())
}

fn check_overlays(rig: &mut Rig) -> Result<(), String> {
    let toasts = rig.h.ui.toasts().len();
    rig.shell
        .session_mut()
        .problem(forge_editor::notify::Problem::coded(
            forge_editor::notify::Severity::Warning,
            "EDITOR-0006",
            "Disk full",
            "details",
        ));
    rig.settle();
    if rig.h.ui.toasts().len() != toasts {
        return Err("a removed toasts overlay still toasts".into());
    }
    if rig.shell.session().notifications.is_empty() {
        return Err("the notification was lost".into());
    }
    let badge = rig
        .h
        .ui
        .root()
        .child(&Key::Static("shell"))
        .child(&Key::Static("overlays"))
        .child(&Key::Str("overlay:com.test.badge".into()))
        .child(&Key::Static("badge"));
    if !rig.h.ui.contains(badge) {
        return Err("the plugin's overlay was not built".into());
    }
    Ok(())
}

#[test]
fn a_plugin_adds_chains_and_removes_themes_and_overlays() {
    let mut rig = rig(true);
    check_themes(&mut rig).unwrap_or_else(|e| panic!("{e}"));
    check_overlays(&mut rig).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn without_the_plugin_the_first_party_overlays_toast() {
    let mut rig = rig(false);
    let before = rig.h.ui.toasts().len();
    rig.shell
        .session_mut()
        .notify(Level::Info, "Hello", "details");
    rig.settle();
    assert_eq!(
        rig.h.ui.toasts().len(),
        before + 1,
        "the toasts overlay toasts"
    );
}

#[test]
fn positive_control_a_shell_ignoring_the_registry_fails() {
    let mut rig = rig(false);
    let e = check_themes(&mut rig).expect_err("no chained theme without the plugin");
    assert!(e.contains("chained"), "{e}");
    let e = check_overlays(&mut rig).expect_err("toasts still shown without the removal");
    assert!(e.contains("still toasts"), "{e}");
}

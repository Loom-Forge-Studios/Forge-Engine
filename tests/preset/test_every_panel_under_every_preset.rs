//! `test_every_panel_under_every_preset` (Ch.21 §21.19, §21.23; I15 for panels; DoD M2-27):
//! every registered `EditorPanel` opens — builds real content in a real dock — under the
//! built-in presets. Presets choose defaults and never gate a panel.
//!
//! The panels are the editor's registry as the ordinary loader fills it, exactly as the
//! `forge-editor` binary loads it: the real first-party panels (`plugins/forge-panels-core`,
//! WP-U4) plus the labelled stand-in plugin (`forge_editor::stand_in`) for every Ch.21
//! §21.21 panel a later work package builds. The check itself
//! (`forge_editor::panels::check_panels_under_presets`) runs on each real panel crate the
//! moment it is added here.
//!
//! Positive control (W2): `positive_control_a_preset_gated_panel_fails` — a panel whose
//! descriptor builds nothing under the 2D preset makes the check fail, naming that panel
//! and that preset.

use std::sync::Arc;

use forge_editor::panels::{PanelCx, check_panels_under_presets};
use forge_editor::presets::builtin_presets;
use forge_editor::stand_in::StandInPanels;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor, PresetKind};
use forge_plugin::{
    Extensions, Grants, InstallCx, Manifest, Order, PluginError, SourcePlugin, loader,
};
use forge_ui::NodeStyle;
use forge_ui::widgets::Label;

fn load(extra: &[&dyn SourcePlugin]) -> Extensions {
    let mut x = forge_editor::editor_extensions().expect("editor points");
    let core = forge_panels_core::PanelsCore::new().expect("forge-panels-core manifest");
    let scene = forge_panels_scene::PanelsScene::new().expect("forge-panels-scene manifest");
    let assets = forge_panels_assets::PanelsAssets::new().expect("forge-panels-assets manifest");
    let domain = forge_panels_domain::PanelsDomain::new().expect("forge-panels-domain manifest");
    let authoring =
        forge_panels_authoring::PanelsAuthoring::new().expect("forge-panels-authoring manifest");
    let project =
        forge_panels_project::PanelsProject::new().expect("forge-panels-project manifest");
    let mut real = forge_panels_core::panel_ids();
    real.extend(forge_panels_scene::panel_ids());
    real.extend(forge_panels_assets::panel_ids());
    real.extend(forge_panels_domain::panel_ids());
    real.extend(forge_panels_authoring::panel_ids());
    real.extend(forge_panels_project::panel_ids());
    let stand_in = StandInPanels::without(&real).expect("stand-in manifest");
    let mut all: Vec<&dyn SourcePlugin> = vec![
        &core, &scene, &assets, &domain, &authoring, &project, &stand_in,
    ];
    all.extend_from_slice(extra);
    loader::load(&mut x, &all, &[], &Grants::new()).expect("plugins load");
    x
}

#[test]
fn test_every_panel_under_every_preset() {
    let x = load(&[]);
    let reg = x.registry::<EditorPanel<PanelCx>>().expect("EditorPanel");
    assert!(
        reg.len() >= 30,
        "the full panel set is registered ({})",
        reg.len()
    );
    let presets = builtin_presets().expect("the built-in presets load");
    assert_eq!(presets.len(), forge_editor::presets::BUILTIN_PRESETS.len());
    let problems = check_panels_under_presets(reg, &presets);
    assert!(problems.is_empty(), "{problems:#?}");
}

/// A plugin whose panel hides itself under the 2D preset: the gating I15 forbids.
struct Gated {
    m: Manifest,
}

impl SourcePlugin for Gated {
    fn manifest(&self) -> &Manifest {
        &self.m
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<EditorPanel<PanelCx>>(
            "com.test.gated",
            PanelDescriptor {
                title: "Gated".into(),
                icon: None,
                default_dock: Dock::Right,
                build: Arc::new(|cx: &mut PanelCx| {
                    if cx.preset() == PresetKind::TwoD {
                        return; // "not for 2D projects"
                    }
                    cx.add(|b, parent| {
                        b.add(parent, "body", NodeStyle::leaf(), Label::new("3D only"))?;
                        Ok(())
                    });
                }),
            },
            Order::Last,
        )
    }
}

#[test]
fn positive_control_a_preset_gated_panel_fails() {
    let gated = Gated {
        m: Manifest::parse(
            r#"Plugin(id: "com.test.gated", version: "0.1.0", engine: "^0.1", kind: Source,
                provides: [ EditorPanel("com.test.gated") ])"#,
        )
        .expect("manifest"),
    };
    let x = load(&[&gated]);
    let reg = x.registry::<EditorPanel<PanelCx>>().expect("EditorPanel");
    let problems = check_panels_under_presets(reg, &builtin_presets().expect("presets"));
    assert!(!problems.is_empty(), "a preset-gated panel passed");
    assert!(
        problems
            .iter()
            .all(|p| p.starts_with("forge.preset.2d: com.test.gated")),
        "only the gated panel under 2D fails: {problems:#?}"
    );
}

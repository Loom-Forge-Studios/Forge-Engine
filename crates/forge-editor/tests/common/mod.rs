//! Shared set-up for the forge-editor guard tests.
#![allow(dead_code)]

use forge_editor::panels::{EditorPanelHost, PanelCatalog, PanelCx};
use forge_editor::stand_in::StandInPanels;
use forge_plugin::points::{EditorPanel, PresetKind};
use forge_plugin::{Extensions, Grants, Registry, SourcePlugin, loader};
use forge_ui::dock::{AreaId, DockTree, Layout, dock_area};
use forge_ui::testing::Harness;
use forge_ui::{NodeStyle, Signal, Size, UiConfig, WidgetId};

/// The editor's extension points with the stand-in panel set loaded through the ordinary
/// loader, plus any `extra` plugins.
pub fn load(extra: &[&dyn SourcePlugin]) -> Extensions {
    let mut x = forge_editor::editor_extensions().unwrap_or_else(|e| panic!("{e}"));
    let stand_in = StandInPanels::new().unwrap_or_else(|e| panic!("{e}"));
    let mut all: Vec<&dyn SourcePlugin> = vec![&stand_in];
    all.extend_from_slice(extra);
    loader::load(&mut x, &all, &[], &Grants::new()).unwrap_or_else(|e| panic!("{e}"));
    x
}

pub fn panels(x: &Extensions) -> &Registry<EditorPanel<PanelCx>> {
    x.registry::<EditorPanel<PanelCx>>()
        .unwrap_or_else(|| panic!("EditorPanel not defined"))
}

pub struct Editor {
    pub h: Harness,
    pub dock: WidgetId,
    pub tree: Signal<DockTree>,
}

/// A 1200x760 window with a dock over `layout`'s main tree, built from `reg`.
pub fn editor(
    layout: &Layout,
    reg: &Registry<EditorPanel<PanelCx>>,
    preset: PresetKind,
    catalog: PanelCatalog,
) -> Editor {
    let mut h = Harness::new(UiConfig {
        size: Size::new(1200.0, 760.0),
        ..UiConfig::default()
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let tree = h.ui.rt_mut().signal(DockTree {
        root: layout.main.clone(),
        maximised: layout.maximised.clone(),
    });
    let root = h.ui.root();
    let dock = dock_area(
        &mut h.ui,
        root,
        "dock",
        NodeStyle::default().fill(),
        AreaId::Main,
        tree,
        Box::new(EditorPanelHost::new(reg, preset, catalog)),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    h.settle();
    Editor { h, dock, tree }
}

/// A scratch directory for user-config files, unique per test.
pub fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "forge-editor-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

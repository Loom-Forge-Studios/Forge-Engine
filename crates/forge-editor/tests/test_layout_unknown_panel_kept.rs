//! `test_layout_unknown_panel_kept` (Ch.21 §21.17, §21.23; DoD M2-26): a layout naming a
//! panel whose plugin is disabled keeps that panel. It loads, shows a placeholder naming
//! the disabled plugin in the panel's place, saves with the id intact, and when the plugin
//! is enabled again the real panel appears exactly where it was. Disabling a plugin never
//! destroys a layout.
//!
//! Positive control (W2): `positive_control_a_serializer_dropping_unknown_ids_fails` — the
//! same check against a save path that drops ids the registry does not know fails.

mod common;

use std::sync::Arc;

use forge_editor::layout_store::LayoutStore;
use forge_editor::panels::{PanelCatalog, PanelCx};
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor, PresetKind};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, Registry, SourcePlugin};
use forge_ui::dock::{Axis, DockNode, Layout, PanelId, panel_frame_id};
use forge_ui::widgets::Label;
use forge_ui::{Key, NodeStyle};

/// A third-party plugin providing the `com.example.rivers` panel.
struct Rivers {
    m: Manifest,
}

impl Rivers {
    fn new() -> Self {
        Self {
            m: Manifest::parse(
                r#"Plugin(id: "com.example.rivers", version: "1.2.0", engine: "^0.1", kind: Source,
                    provides: [ EditorPanel("com.example.rivers") ])"#,
            )
            .unwrap_or_else(|e| panic!("{e}")),
        }
    }
}

impl SourcePlugin for Rivers {
    fn manifest(&self) -> &Manifest {
        &self.m
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.add::<EditorPanel<PanelCx>>(
            "com.example.rivers",
            PanelDescriptor {
                title: "Rivers".into(),
                icon: None,
                default_dock: Dock::Right,
                build: Arc::new(|cx: &mut PanelCx| {
                    cx.add(|b, parent| {
                        b.add(
                            parent,
                            "rivers_body",
                            NodeStyle::leaf(),
                            Label::new("River networks"),
                        )?;
                        Ok(())
                    });
                }),
            },
            Order::Last,
        )
    }
}

fn layout() -> Layout {
    Layout::new(DockNode::split(
        Axis::Horizontal,
        vec![
            (0.25, DockNode::tabs(&["forge.hierarchy"])),
            (0.5, DockNode::tabs(&["forge.viewport"])),
            (
                0.25,
                DockNode::tabs(&["com.example.rivers", "forge.inspector"]),
            ),
        ],
    ))
}

/// Load `saved` with `reg`, check the unknown panel's placeholder, save it back through
/// `save`, and check the id survived where it was.
fn check_unknown_kept(
    saved: &str,
    reg: &Registry<EditorPanel<PanelCx>>,
    catalog: PanelCatalog,
    save: &dyn Fn(&Layout, &Registry<EditorPanel<PanelCx>>) -> String,
) -> Result<(), String> {
    let l = Layout::from_ron(saved).map_err(|e| e.to_string())?;
    let rivers = PanelId::new("com.example.rivers");
    if !l.contains(&rivers) {
        return Err("loading dropped the unknown panel".into());
    }
    let mut ed = common::editor(&l, reg, PresetKind::ThreeD, catalog);
    let frame = panel_frame_id(ed.dock, &rivers);
    if !ed.h.ui.contains(frame) || ed.h.ui.is_hidden(frame) {
        return Err("the unknown panel has no visible place in the dock".into());
    }
    let msg =
        ed.h.node(frame.child(&Key::Static("missing")))
            .and_then(|n| n.label().map(str::to_string))
            .unwrap_or_default();
    if !msg.contains("com.example.rivers") || !msg.contains("disabled") {
        return Err(format!(
            "the placeholder does not name the disabled plugin: {msg:?}"
        ));
    }
    let again = save(&l, reg);
    let back = Layout::from_ron(&again).map_err(|e| e.to_string())?;
    if back.find(&rivers) != l.find(&rivers) {
        return Err(format!(
            "saving moved or dropped the unknown panel: {:?} -> {:?}",
            l.find(&rivers),
            back.find(&rivers)
        ));
    }
    Ok(())
}

fn real_save(l: &Layout, _: &Registry<EditorPanel<PanelCx>>) -> String {
    l.to_ron().unwrap_or_default()
}

#[test]
fn a_disabled_plugins_panel_keeps_its_place_and_comes_back() {
    let rivers = Rivers::new();
    // Rivers disabled: the registry lacks its panel; its manifest says who provides it.
    let x = common::load(&[]);
    let reg = common::panels(&x);
    let catalog = PanelCatalog::from_manifests(&[], &[rivers.manifest()]);
    let saved = layout().to_ron().unwrap_or_else(|e| panic!("{e}"));
    check_unknown_kept(&saved, reg, catalog.clone(), &real_save).unwrap_or_else(|e| panic!("{e}"));
    // Saved through the user layout store too.
    let dir = common::scratch("unknown");
    let store = LayoutStore::new(dir.clone());
    store
        .save("mine", &layout())
        .unwrap_or_else(|e| panic!("{e}"));
    let loaded = store.load("mine").ok().flatten().unwrap_or_default();
    assert!(loaded.contains(&PanelId::new("com.example.rivers")));
    let _ = std::fs::remove_dir_all(&dir);
    // Enabled again: the real panel is built in the same place.
    let x2 = common::load(&[&rivers]);
    let reg2 = common::panels(&x2);
    let mut ed = common::editor(&loaded, reg2, PresetKind::ThreeD, PanelCatalog::default());
    let frame = panel_frame_id(ed.dock, &PanelId::new("com.example.rivers"));
    assert!(
        ed.h.ui.contains(frame.child(&Key::Static("rivers_body"))),
        "the real panel is back"
    );
    assert!(!ed.h.ui.contains(frame.child(&Key::Static("missing"))));
    let label =
        ed.h.node(frame)
            .and_then(|n| n.label().map(str::to_string))
            .unwrap_or_default();
    assert_eq!(label, "Rivers");
}

/// W2: a save path that keeps only panels the registry knows loses the disabled plugin's
/// panel, and the check fails.
#[test]
fn positive_control_a_serializer_dropping_unknown_ids_fails() {
    let rivers = Rivers::new();
    let x = common::load(&[]);
    let reg = common::panels(&x);
    let catalog = PanelCatalog::from_manifests(&[], &[rivers.manifest()]);
    let saved = layout().to_ron().unwrap_or_else(|e| panic!("{e}"));
    let dropping = |l: &Layout, reg: &Registry<EditorPanel<PanelCx>>| {
        let mut c = l.clone();
        for p in l.panels() {
            if reg.get(p.as_str()).is_none() {
                c.close(&p);
            }
        }
        c.to_ron().unwrap_or_default()
    };
    let e = check_unknown_kept(&saved, reg, catalog, &dropping)
        .expect_err("a serializer dropping unknown ids passed");
    assert!(e.contains("dropped"), "{e}");
}

//! The `EditorPanel` extension point in the running editor (Ch.21 §21.19, I16; DoD M2-27):
//! through the ordinary plugin loader, a third-party plugin **replaces** the hierarchy
//! panel, **chains** the inspector (its banner above the original, which still builds
//! inside), **removes** the console and **adds** a panel of its own — and the dock the user
//! sees shows exactly that: the replacement's widgets where the hierarchy was, the wrapped
//! inspector, a placeholder naming the plugin where the console was (its place kept), and
//! the new panel. First-party panels are loaded by the same loader, through the same point.
//!
//! Positive control (W2): `positive_control_a_host_ignoring_replace_fails` — a dock host
//! over a registry that never applied the plugin's operations (a registry ignoring
//! `replace`) fails the check.

mod common;

use std::sync::Arc;

use forge_editor::panels::{PanelCatalog, PanelCx};
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor, PresetKind};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, PluginId, Registry, SourcePlugin};
use forge_ui::dock::{Axis, DockNode, Layout, PanelId, panel_frame_id};
use forge_ui::widgets::Label;
use forge_ui::{Key, NodeStyle};

struct TestPanels {
    m: Manifest,
}

impl TestPanels {
    fn new() -> Self {
        Self {
            m: Manifest::parse(
                r#"Plugin(id: "com.test.panels", version: "1.0.0", engine: "^0.1", kind: Source,
                    provides: [ EditorPanel("com.test.minimap") ],
                    replaces: [ EditorPanel("forge.hierarchy") ],
                    chains: [ EditorPanel("forge.inspector") ],
                    removes: [ EditorPanel("forge.console") ])"#,
            )
            .unwrap_or_else(|e| panic!("{e}")),
        }
    }
}

fn label_panel(
    title: &'static str,
    key: &'static str,
    text: &'static str,
) -> PanelDescriptor<PanelCx> {
    PanelDescriptor {
        title: title.into(),
        icon: None,
        default_dock: Dock::Left,
        build: Arc::new(move |cx: &mut PanelCx| {
            cx.add(move |b, parent| {
                b.add(parent, key, NodeStyle::leaf(), Label::new(text))?;
                Ok(())
            });
        }),
    }
}

impl SourcePlugin for TestPanels {
    fn manifest(&self) -> &Manifest {
        &self.m
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        cx.replace::<EditorPanel<PanelCx>>(
            "forge.hierarchy",
            label_panel("Outliner", "my_hierarchy", "A better hierarchy"),
        )?;
        cx.chain::<EditorPanel<PanelCx>>("forge.inspector", |inner| {
            let original = Arc::clone(&inner.build);
            PanelDescriptor {
                build: Arc::new(move |cx: &mut PanelCx| {
                    cx.add(|b, parent| {
                        b.add(
                            parent,
                            "banner",
                            NodeStyle::leaf(),
                            Label::new("Linted by com.test"),
                        )?;
                        Ok(())
                    });
                    original(cx);
                }),
                ..inner
            }
        })?;
        cx.remove::<EditorPanel<PanelCx>>("forge.console")?;
        cx.add::<EditorPanel<PanelCx>>(
            "com.test.minimap",
            label_panel("Minimap", "minimap_body", "Minimap"),
            Order::Last,
        )
    }
}

fn layout() -> Layout {
    Layout::new(DockNode::split(
        Axis::Horizontal,
        vec![
            (0.25, DockNode::tabs(&["forge.hierarchy"])),
            (
                0.5,
                DockNode::split(
                    Axis::Vertical,
                    vec![
                        (0.6, DockNode::tabs(&["forge.viewport"])),
                        (0.4, DockNode::tabs(&["forge.console"])),
                    ],
                ),
            ),
            (
                0.25,
                DockNode::tabs(&["forge.inspector", "com.test.minimap"]),
            ),
        ],
    ))
}

/// What the user must see after the plugin loaded. `Err` names the first difference.
fn check_panels(reg: &Registry<EditorPanel<PanelCx>>, catalog: PanelCatalog) -> Result<(), String> {
    let mut ed = common::editor(&layout(), reg, PresetKind::ThreeD, catalog);
    let dock = ed.dock;
    let f = |p: &str| panel_frame_id(dock, &PanelId::new(p));
    let has = |ed: &common::Editor, frame: forge_ui::WidgetId, key: &'static str| {
        ed.h.ui.contains(frame.child(&Key::Static(key)))
    };
    // Replaced: the plugin's widgets, not the first-party ones.
    if !has(&ed, f("forge.hierarchy"), "my_hierarchy") {
        return Err("the hierarchy was not replaced by the plugin's panel".into());
    }
    if has(&ed, f("forge.hierarchy"), "title") {
        return Err("the first-party hierarchy still builds beside its replacement".into());
    }
    let tab_title =
        ed.h.node(f("forge.hierarchy"))
            .and_then(|n| n.label().map(str::to_string))
            .unwrap_or_default();
    if tab_title != "Outliner" {
        return Err(format!("the replaced panel is titled {tab_title:?}"));
    }
    // Chained: the plugin's banner and the original inspector, both.
    if !(has(&ed, f("forge.inspector"), "banner") && has(&ed, f("forge.inspector"), "title")) {
        return Err("the inspector is not the original wrapped by the plugin".into());
    }
    // Removed: a placeholder naming the plugin, and the panel keeps its place.
    let msg =
        ed.h.node(f("forge.console").child(&Key::Static("missing")))
            .and_then(|n| n.label().map(str::to_string))
            .unwrap_or_default();
    if !msg.contains("com.test.panels") {
        return Err(format!(
            "the removed console shows {msg:?}, not who removed it"
        ));
    }
    // Added: the plugin's own panel is built and docked like any other.
    if !has(&ed, f("com.test.minimap"), "minimap_body") {
        return Err("the plugin's new panel was not built".into());
    }
    Ok(())
}

#[test]
fn test_panel_extension_replaceable() {
    let plugin = TestPanels::new();
    let x = common::load(&[&plugin]);
    let reg = common::panels(&x);
    let prov = reg
        .provenance("forge.hierarchy")
        .unwrap_or_else(|| panic!("no provenance"));
    assert_eq!(prov.owner.as_str(), "forge.panels.stand_in");
    assert_eq!(
        prov.replaced_by.as_ref().map(PluginId::as_str),
        Some("com.test.panels")
    );
    let stand_in = forge_editor::stand_in::StandInPanels::new().unwrap_or_else(|e| panic!("{e}"));
    let catalog = PanelCatalog::from_manifests(&[plugin.manifest(), stand_in.manifest()], &[]);
    check_panels(reg, catalog).unwrap_or_else(|e| panic!("{e}"));
}

/// W2: a host over a registry that never applied the plugin's operations — what a
/// registry that ignores `replace` (and the rest) gives — fails the check.
#[test]
fn positive_control_a_host_ignoring_replace_fails() {
    let plugin = TestPanels::new();
    let x = common::load(&[]);
    let reg = common::panels(&x);
    let catalog = PanelCatalog::from_manifests(&[plugin.manifest()], &[]);
    let e = check_panels(reg, catalog).expect_err("a host ignoring replace passed");
    assert!(e.contains("not replaced"), "{e}");
}

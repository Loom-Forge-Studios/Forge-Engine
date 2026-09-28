//! **Stand-in panels** (D-4 labelled): a source plugin, `forge.panels.stand_in`, that
//! registers every panel of the Ch.21 §21.21 inventory through the ordinary `EditorPanel`
//! point, each showing its title and an empty state that says which work package builds
//! it. It exists so the dock, the presets and the guards (I15's
//! `test_every_panel_under_every_preset`, the layout tests) run against the full panel set
//! before the real `plugins/forge-panels-*` crates exist (WP-U4..WP-U13). It is never
//! presented as the real panels: its manifest id, its titles' empty states and this module
//! all say "stand-in".

use std::sync::Arc;

use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};
use forge_ui::NodeStyle;
use forge_ui::widgets::{Label, LabelKind};

use crate::panels::PanelCx;

/// `(id, title, default dock, the work package that builds the real panel)`.
pub const PANELS: &[(&str, &str, Dock, &str)] = &[
    (
        "forge.viewport",
        forge_ui::tr_key!("Viewport"),
        Dock::Center,
        "WP-U6",
    ),
    (
        "forge.play_controls",
        forge_ui::tr_key!("Play controls"),
        Dock::Center,
        "WP-U6",
    ),
    (
        "forge.hierarchy",
        forge_ui::tr_key!("Hierarchy"),
        Dock::Left,
        "WP-U5",
    ),
    (
        "forge.inspector",
        forge_ui::tr_key!("Inspector"),
        Dock::Right,
        "WP-U5",
    ),
    (
        "forge.assets",
        forge_ui::tr_key!("Assets"),
        Dock::Bottom,
        "WP-U5",
    ),
    (
        "forge.console",
        forge_ui::tr_key!("Console"),
        Dock::Bottom,
        "WP-U5",
    ),
    (
        "forge.profiler",
        forge_ui::tr_key!("Profiler"),
        Dock::Bottom,
        "WP-U6",
    ),
    (
        "forge.undo_history",
        forge_ui::tr_key!("Undo history"),
        Dock::Right,
        "WP-U4",
    ),
    (
        "forge.notifications",
        forge_ui::tr_key!("Notifications"),
        Dock::Right,
        "WP-U4",
    ),
    (
        "forge.settings",
        forge_ui::tr_key!("Settings"),
        Dock::Floating,
        "WP-U4",
    ),
    (
        "forge.keybindings",
        forge_ui::tr_key!("Keybindings"),
        Dock::Floating,
        "WP-U4",
    ),
    (
        "forge.command_palette",
        forge_ui::tr_key!("Command palette"),
        Dock::Floating,
        "WP-U4",
    ),
    (
        "forge.graph",
        forge_ui::tr_key!("Graph editor"),
        Dock::Center,
        "WP-U8",
    ),
    (
        "forge.launcher",
        forge_ui::tr_key!("Launcher"),
        Dock::Floating,
        "WP-U7",
    ),
    (
        "forge.presets",
        forge_ui::tr_key!("Presets"),
        Dock::Floating,
        "WP-U7",
    ),
    (
        "forge.history",
        forge_ui::tr_key!("Revision history"),
        Dock::Right,
        "WP-U7",
    ),
    (
        "forge.export",
        forge_ui::tr_key!("Build and export"),
        Dock::Floating,
        "WP-U7",
    ),
    (
        "forge.plugins",
        forge_ui::tr_key!("Plugins"),
        Dock::Floating,
        "WP-U9",
    ),
    (
        "forge.audit_log",
        forge_ui::tr_key!("Audit log"),
        Dock::Bottom,
        "WP-U9",
    ),
    (
        "forge.remote",
        forge_ui::tr_key!("Remote"),
        Dock::Floating,
        "WP-U9",
    ),
    (
        "forge.compute",
        forge_ui::tr_key!("Compute and farm"),
        Dock::Bottom,
        "WP-U9",
    ),
    (
        "forge.team",
        forge_ui::tr_key!("Team"),
        Dock::Right,
        "WP-U10",
    ),
    (
        "forge.sandbox",
        forge_ui::tr_key!("Sandbox"),
        Dock::Right,
        "WP-U10",
    ),
    (
        "forge.presence",
        forge_ui::tr_key!("Presence"),
        Dock::Right,
        "WP-U10",
    ),
    (
        "forge.ownership",
        forge_ui::tr_key!("Ownership"),
        Dock::Right,
        "WP-U10",
    ),
    (
        "forge.publish_queue",
        forge_ui::tr_key!("Publish queue"),
        Dock::Right,
        "WP-U10",
    ),
    (
        "forge.conflicts",
        forge_ui::tr_key!("Conflicts"),
        Dock::Center,
        "WP-U10",
    ),
    (
        "forge.licence",
        forge_ui::tr_key!("Licence"),
        Dock::Floating,
        "WP-U10",
    ),
    (
        "forge.sequencer",
        forge_ui::tr_key!("Sequencer"),
        Dock::Bottom,
        "WP-U11",
    ),
    (
        "forge.anim_graph",
        forge_ui::tr_key!("Animation state machine"),
        Dock::Center,
        "WP-U11",
    ),
    (
        "forge.localisation",
        forge_ui::tr_key!("Localisation"),
        Dock::Center,
        "WP-U11",
    ),
    (
        "forge.editors_2d",
        forge_ui::tr_key!("2D editors"),
        Dock::Right,
        "WP-U13",
    ),
    (
        "forge.audio_mixer",
        forge_ui::tr_key!("Audio mixer"),
        Dock::Bottom,
        "WP-U13",
    ),
    (
        "forge.input_map",
        forge_ui::tr_key!("Input map"),
        Dock::Center,
        "WP-U13",
    ),
];

/// The stand-in plugin (see the module docs).
pub struct StandInPanels {
    manifest: Manifest,
    panels: Vec<(&'static str, &'static str, Dock, &'static str)>,
}

impl StandInPanels {
    pub fn new() -> Result<Self, PluginError> {
        Self::without(&[])
    }

    /// The stand-ins for every panel except `real` — the ones a loaded plugin really
    /// provides (the editor binary passes `forge-panels-core`'s panel ids).
    pub fn without(real: &[&str]) -> Result<Self, PluginError> {
        let panels: Vec<_> = PANELS
            .iter()
            .filter(|(id, ..)| !real.contains(id))
            .copied()
            .collect();
        let provides: Vec<String> = panels
            .iter()
            .map(|(id, ..)| format!("EditorPanel(\"{id}\")"))
            .collect();
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.panels.stand_in\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
            panels,
        })
    }

    /// The panels this plugin stands in for.
    #[must_use]
    pub fn panel_ids(&self) -> Vec<&'static str> {
        self.panels.iter().map(|(id, ..)| *id).collect()
    }
}

/// A stand-in panel's descriptor.
pub fn descriptor(
    id: &'static str,
    title: &'static str,
    dock: Dock,
    wp: &'static str,
) -> PanelDescriptor<PanelCx> {
    PanelDescriptor {
        title: title.to_string(),
        icon: None,
        default_dock: dock,
        build: Arc::new(move |cx: &mut PanelCx| {
            cx.add(move |b, parent| {
                b.add(
                    parent,
                    "title",
                    NodeStyle::leaf(),
                    Label::new(forge_ui::l10n::tr_str(title).into_owned()).kind(LabelKind::Heading),
                )?;
                Ok(())
            });
            cx.empty_state(&forge_ui::trf!(
                "Stand-in for the {title} panel ({id}); the real panel is built in {wp}.",
                title = forge_ui::l10n::tr_str(title),
                id,
                wp
            ));
        }),
    }
}

impl SourcePlugin for StandInPanels {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, title, dock, wp) in &self.panels {
            cx.add::<EditorPanel<PanelCx>>(id, descriptor(id, title, *dock, wp), Order::Last)?;
        }
        Ok(())
    }
}

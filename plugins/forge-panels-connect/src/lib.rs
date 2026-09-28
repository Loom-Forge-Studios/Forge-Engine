//! `forge-panels-connect` — the first-party connect panels (Ch.21 §21.21), a source plugin
//! with the id `forge.panels.connect`:
//!
//! | Panel | Id | DoD | Backend (D-4) |
//! |---|---|---|---|
//! | Plugin manager | `forge.plugins` | M2-52 | the ordinary loader's manifests, `forge-wasm` hosted plugins, the project's grants; the index is the labelled in-memory `MemoryIndex` until M5-14 |
//! | Audit log | `forge.audit_log` | M2-54 | the core's audit book, and the audit feed a plugin attaches; teammates UNBUILT (`forge-collab`) |
//! | Remote connect | `forge.remote` | M2-55 | the labelled `LoopbackTransport` until `forge-remote` (M2-16); pairing is the real `RemotePairingStore` |
//! | Compute & farm | `forge.compute` | M2-56 | the labelled `MemoryComputePool` until `forge-jobs` / `forge-farm` |
//!
//! Registered through the ordinary `EditorPanel` point, exactly as a third-party plugin
//! would be (I16), and available under every preset (I15). **Every change to project state
//! is a command (I7, §21.18)**: plugin grants and revokes (human-only, never granted by undo
//! or redo), adding / removing / enabling / disabling a plugin, and accepting or discarding
//! held security settings. The two direct writes are user config, through the editor's
//! allow-listed writers: downloaded plugin bytes (`PluginCache`) and device pairing
//! (`RemotePairingStore`, audited).
//!
//! [`ui`] and [`relay`] are the connect panels' shared widget helpers, public so a plugin
//! adding a panel of the same kind builds it the same way.

#![forbid(unsafe_code)]

pub mod audit;
pub mod compute;
pub mod plugins;
pub mod relay;
pub mod remote;
pub mod ui;

use forge_editor::panels::PanelCx;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

/// The panels this plugin provides: `(id, title, default dock, first-run tip)`. The title
/// and the tip are localisation keys (M2-31).
pub const PANELS: &[(&str, &str, Dock, &str)] = &[
    (
        "forge.plugins",
        forge_ui::tr_key!("Plugins"),
        Dock::Floating,
        forge_ui::tr_key!("Plugins run in a sandbox with only the capabilities you grant them."),
    ),
    (
        "forge.audit_log",
        forge_ui::tr_key!("Audit log"),
        Dock::Bottom,
        forge_ui::tr_key!("Every security decision and every remote session is recorded here."),
    ),
    (
        "forge.remote",
        forge_ui::tr_key!("Remote"),
        Dock::Floating,
        forge_ui::tr_key!("Pair a device with a one-time code to edit this project from it."),
    ),
    (
        "forge.compute",
        forge_ui::tr_key!("Compute and farm"),
        Dock::Bottom,
        forge_ui::tr_key!("Heavy jobs run on this machine's adapters, or on farm nodes you add."),
    ),
];

/// Live panels refresh at most this often (Ch.21 §21.21: latency and compute, 2 Hz).
pub const LIVE_HZ: u8 = 2;

/// The panel ids (what the stand-in set leaves out when this plugin is loaded).
pub fn panel_ids() -> Vec<&'static str> {
    PANELS.iter().map(|(id, ..)| *id).collect()
}

/// The plugin (see the crate docs).
pub struct PanelsConnect {
    manifest: Manifest,
}

impl PanelsConnect {
    pub fn new() -> Result<Self, PluginError> {
        let provides: Vec<String> = PANELS
            .iter()
            .map(|(id, ..)| format!("EditorPanel(\"{id}\")"))
            .collect();
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.panels.connect\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
        })
    }
}

impl SourcePlugin for PanelsConnect {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, title, dock, tip) in PANELS {
            let build: fn(&mut PanelCx) = match *id {
                "forge.plugins" => plugins::build,
                "forge.audit_log" => audit::build,
                "forge.remote" => remote::build,
                _ => compute::build,
            };
            cx.add::<EditorPanel<PanelCx>>(
                id,
                PanelDescriptor {
                    title: forge_ui::l10n::tr(title).to_string(),
                    icon: None,
                    default_dock: *dock,
                    build: with_tip(build, tip),
                },
                Order::Last,
            )?;
        }
        Ok(())
    }
}

/// `build`, after declaring the panel's first-run tip (a localisation key; M2-70).
pub fn with_tip(
    build: fn(&mut forge_editor::panels::PanelCx),
    tip: &'static str,
) -> forge_plugin::points::BuildFn<forge_editor::panels::PanelCx> {
    std::sync::Arc::new(move |cx: &mut forge_editor::panels::PanelCx| {
        cx.first_run_tip(forge_ui::l10n::tr(tip));
        build(cx);
    })
}

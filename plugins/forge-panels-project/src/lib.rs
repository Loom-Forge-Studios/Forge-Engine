//! `forge-panels-project` — the first-party project lifecycle panels (Ch.21 §21.21), a
//! source plugin with the id `forge.panels.project`:
//!
//! | Panel | Id | DoD | Backend (D-4) |
//! |---|---|---|---|
//! | Launcher & new project | `forge.launcher` | M2-47 | the core's store host, which routes each location to its store — `LocalFs` folders, `Git` (`git:` projects, Git remotes over smart HTTP), `git-file:` repositories on a path, in-memory — through lifecycle commands (create, open, clone) |
//! | Preset & promotion | `forge.presets` | M2-48 | the editor's preset catalog (the built-in manifests in `presets/` and every preset plugin's), the project's own copies in its `presets/<dir>/` folder (read from the project status), the plugins' promotion rules, + `forge.project.promote` |
//! | Revision history & store | `forge.history` | M2-50 | save / push / pull commands, the transfers run off the core's lock; `Git` remotes over smart HTTP, GitHub sign-in and *Create on GitHub* (WP-16, M2-15) |
//! | Build & export | `forge.export` | M2-51 | `Packager` (in-memory until the HAL exporters, M8) |
//!
//! Registered through the ordinary `EditorPanel` point, exactly as a third-party plugin
//! would be (I16), and available under every preset (I15). The panels reach the project
//! only through what `PanelCx` hands them: every lifecycle action — create, open, save,
//! push, pull, build, link a remote, promote — is a **command** (I7); the store operations
//! are the core's (E-36). A lossy promotion shows what is lost and asks before it sends
//! anything, and the core refuses the command without that confirmation anyway.
//!
//! [`ui`] holds the small building blocks these panels share (a plugin's own lifecycle
//! panel may use them too).

#![forbid(unsafe_code)]

pub mod export;
pub mod history;
pub mod launcher;
pub mod presets;
pub mod ui;

use forge_editor::panels::PanelCx;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

/// The panels this plugin provides: `(id, title, default dock, first-run tip)`. The title
/// and the tip are localisation keys (M2-31).
pub const PANELS: &[(&str, &str, Dock, &str)] = &[
    (
        "forge.launcher",
        forge_ui::tr_key!("Launcher"),
        Dock::Floating,
        forge_ui::tr_key!("Start from a template, or open a project folder you already have."),
    ),
    (
        "forge.presets",
        forge_ui::tr_key!("Presets"),
        Dock::Floating,
        forge_ui::tr_key!("A preset sets defaults only: every panel works under every preset."),
    ),
    (
        "forge.history",
        forge_ui::tr_key!("Revision history"),
        Dock::Right,
        forge_ui::tr_key!(
            "Each save is a revision; link a remote to back the project up and share it."
        ),
    ),
    (
        "forge.export",
        forge_ui::tr_key!("Build and export"),
        Dock::Floating,
        forge_ui::tr_key!("A build writes the executable with its NOTICES and credits beside it."),
    ),
];

/// The panel ids (what the stand-in set leaves out when this plugin is loaded).
pub fn panel_ids() -> Vec<&'static str> {
    PANELS.iter().map(|(id, ..)| *id).collect()
}

/// The plugin (see the crate docs).
pub struct PanelsProject {
    manifest: Manifest,
}

impl PanelsProject {
    pub fn new() -> Result<Self, PluginError> {
        let provides: Vec<String> = PANELS
            .iter()
            .map(|(id, ..)| format!("EditorPanel(\"{id}\")"))
            .collect();
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.panels.project\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
        })
    }
}

impl SourcePlugin for PanelsProject {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, title, dock, tip) in PANELS {
            let build: fn(&mut PanelCx) = match *id {
                "forge.launcher" => launcher::build,
                "forge.presets" => presets::build,
                "forge.history" => history::build,
                _ => export::build,
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
fn with_tip(
    build: fn(&mut forge_editor::panels::PanelCx),
    tip: &'static str,
) -> forge_plugin::points::BuildFn<forge_editor::panels::PanelCx> {
    std::sync::Arc::new(move |cx: &mut forge_editor::panels::PanelCx| {
        cx.first_run_tip(forge_ui::l10n::tr(tip));
        build(cx);
    })
}

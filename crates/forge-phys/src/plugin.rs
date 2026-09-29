//! `forge.phys` — the first-party physics backends as a plugin (Ch.32: every capability
//! arrives through the ordinary loader, I16).
//!
//! Provides `PhysicsBackend("avian3d")` and `PhysicsBackend("rapier3d")` on
//! `forge.phys.backend` (each only when the build has it). The 3D preset names `avian3d` as
//! the point's default (`presets/3d/workspace.ron`); a project's `physics.backend` setting
//! picks another.

use forge_plugin::{Extensions, InstallCx, Manifest, Order, PluginError, SourcePlugin};

use crate::backend::{FIRST_PARTY, PhysicsBackendPoint, first_party_factory};

/// The plugin id.
pub const PLUGIN_ID: &str = "forge.phys";

/// See the module docs.
pub struct PhysPlugin {
    manifest: Manifest,
}

impl PhysPlugin {
    /// The plugin (its manifest lists the backends this build has).
    pub fn new() -> Result<Self, PluginError> {
        let provides: Vec<String> = FIRST_PARTY
            .iter()
            .map(|id| format!("PhysicsBackend(\"{id}\")"))
            .collect();
        let text = format!(
            "Plugin(id: \"{PLUGIN_ID}\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [ {} ])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
        })
    }
}

/// Define `forge.phys.backend` on a host's extensions (once; a second call is a no-op).
pub fn define_points(x: &mut Extensions) -> Result<(), PluginError> {
    x.define::<PhysicsBackendPoint>()
}

impl SourcePlugin for PhysPlugin {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for id in FIRST_PARTY {
            if let Some(f) = first_party_factory(id) {
                cx.add::<PhysicsBackendPoint>(id, f, Order::Last)?;
            }
        }
        Ok(())
    }
}

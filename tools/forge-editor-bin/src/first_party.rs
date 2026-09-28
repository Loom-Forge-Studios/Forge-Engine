//! The editor's first-party panel plugins, as the binary loads them (WP-U12): one place, so
//! the editor-wide audits (`tests/test_editor_panels_audit.rs`) open exactly the panels a
//! user gets — every first-party panel plugin, plus the labelled stand-ins for panels no
//! plugin builds yet (none today: `tests/test_first_party_panels_are_real.rs`, gate
//! `C-first-party-panels`; the stand-in plugin is not loaded when it has nothing to stand
//! in for).

use forge_editor::stand_in::StandInPanels;
use forge_plugin::SourcePlugin;

/// Every first-party panel plugin (see the module docs).
pub struct FirstParty {
    core: forge_panels_core::PanelsCore,
    scene: forge_panels_scene::PanelsScene,
    assets: forge_panels_assets::PanelsAssets,
    domain: forge_panels_domain::PanelsDomain,
    project: forge_panels_project::PanelsProject,
    connect: forge_panels_connect::PanelsConnect,
    collab: forge_panels_collab::PanelsCollab,
    authoring: forge_panels_authoring::PanelsAuthoring,
    /// The stand-ins for catalogue panels no plugin above builds (`None`: there are none).
    stand_in: Option<StandInPanels>,
}

impl FirstParty {
    pub fn new() -> Result<Self, String> {
        let s = |e: forge_plugin::PluginError| e.to_string();
        let stand_in = StandInPanels::without(&Self::real_panel_ids()).map_err(s)?;
        Ok(Self {
            core: forge_panels_core::PanelsCore::new().map_err(s)?,
            scene: forge_panels_scene::PanelsScene::new().map_err(s)?,
            assets: forge_panels_assets::PanelsAssets::new().map_err(s)?,
            domain: forge_panels_domain::PanelsDomain::new().map_err(s)?,
            project: forge_panels_project::PanelsProject::new().map_err(s)?,
            connect: forge_panels_connect::PanelsConnect::new().map_err(s)?,
            collab: forge_panels_collab::PanelsCollab::new().map_err(s)?,
            authoring: forge_panels_authoring::PanelsAuthoring::new().map_err(s)?,
            stand_in: (!stand_in.panel_ids().is_empty()).then_some(stand_in),
        })
    }

    /// The panels the real first-party plugins build.
    #[must_use]
    pub fn real_panel_ids() -> Vec<&'static str> {
        let mut real = forge_panels_core::panel_ids();
        real.extend(forge_panels_scene::panel_ids());
        real.extend(forge_panels_assets::panel_ids());
        real.extend(forge_panels_domain::panel_ids());
        real.extend(forge_panels_project::panel_ids());
        real.extend(forge_panels_connect::panel_ids());
        real.extend(forge_panels_collab::panel_ids());
        real.extend(forge_panels_authoring::panel_ids());
        real
    }

    /// The catalogue panels still served by the labelled stand-in plugin.
    #[must_use]
    pub fn stand_in_panels(&self) -> Vec<&'static str> {
        self.stand_in
            .as_ref()
            .map_or_else(Vec::new, StandInPanels::panel_ids)
    }

    /// The plugins in load order.
    pub fn plugins(&self) -> Vec<&dyn SourcePlugin> {
        let mut all: Vec<&dyn SourcePlugin> = vec![
            &self.core,
            &self.scene,
            &self.assets,
            &self.domain,
            &self.project,
            &self.connect,
            &self.collab,
            &self.authoring,
        ];
        if let Some(s) = &self.stand_in {
            all.push(s);
        }
        all
    }
}

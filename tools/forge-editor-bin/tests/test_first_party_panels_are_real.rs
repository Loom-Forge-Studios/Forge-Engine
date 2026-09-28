//! Every panel of the Ch.21.21 catalogue is built by a real first-party panel plugin
//! (`plugins/forge-panels-*`, loaded through the ordinary loader and checked by I7 and I15);
//! the labelled stand-in plugin (`forge_editor::stand_in`, D-4) serves none of them and is
//! not loaded (gate `C-first-party-panels`, DoD M2-27).
//!
//! Control: a first-party set missing one plugin (the authoring panels) falls back to the
//! stand-in for exactly that plugin's panels, and the check names them.

use forge_editor::presets::builtin_preset;
use forge_editor::shell::assemble;
use forge_editor::stand_in::StandInPanels;
use forge_editor_bin::first_party::FirstParty;

/// The catalogue panels a first-party set building `real` leaves to the stand-in.
fn stand_ins(real: &[&str]) -> Result<(), String> {
    let left = StandInPanels::without(real)
        .map_err(|e| e.to_string())?
        .panel_ids();
    if left.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "served by the stand-in plugin: {}",
            left.join(", ")
        ))
    }
}

#[test]
fn every_catalogue_panel_is_built_by_a_real_first_party_plugin() {
    stand_ins(&FirstParty::real_panel_ids()).unwrap_or_else(|e| panic!("{e}"));
    let fp = FirstParty::new().unwrap_or_else(|e| panic!("{e}"));
    assert!(
        fp.stand_in_panels().is_empty(),
        "{:?}",
        fp.stand_in_panels()
    );
    let plugins = fp.plugins();
    assert!(
        plugins
            .iter()
            .all(|p| p.manifest().id.to_string() != "forge.panels.stand_in"),
        "the stand-in plugin is loaded with nothing to stand in for"
    );
    // The editor the binary assembles offers every catalogue panel.
    let catalogue = StandInPanels::new()
        .unwrap_or_else(|e| panic!("{e}"))
        .panel_ids();
    assert!(
        catalogue.len() >= 34,
        "{} catalogue panels",
        catalogue.len()
    );
    let preset = builtin_preset("3d").unwrap_or_else(|e| panic!("{e}"));
    let cfg = assemble(preset, &plugins, &[], None).unwrap_or_else(|e| panic!("{e}"));
    let offered: Vec<String> = cfg.panels.iter().map(|(k, _)| k.to_string()).collect();
    for id in catalogue {
        assert!(offered.iter().any(|o| o == id), "{id} is not offered");
    }
}

#[test]
fn positive_control_a_missing_plugin_falls_back_to_the_stand_in() {
    let authoring = forge_panels_authoring::panel_ids();
    let real: Vec<&str> = FirstParty::real_panel_ids()
        .into_iter()
        .filter(|id| !authoring.contains(id))
        .collect();
    let err = stand_ins(&real).err().unwrap_or_default();
    for id in &authoring {
        assert!(err.contains(id), "{id} not named: {err:?}");
    }
}

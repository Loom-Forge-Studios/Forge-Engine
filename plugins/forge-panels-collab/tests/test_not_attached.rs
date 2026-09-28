//! Without a team server (an editor started with no collaboration backends), every
//! collaboration panel opens, says so, and offers nothing: nothing pretends to work.

mod common;

use common::*;
use forge_editor::testing::Rig;

#[test]
fn every_panel_says_there_is_no_team_server() {
    let mut rig = Rig::new(config()).unwrap_or_else(|e| panic!("{e}"));
    let panels = forge_panels_collab::panel_ids();
    rig.show_panels(&panels).unwrap_or_else(|e| panic!("{e}"));
    feed(&mut rig);
    for p in panels.iter().filter(|p| **p != "forge.licence") {
        let n = label(&rig, at(&rig, p, &["notice"]));
        assert!(n.contains("No team server is attached"), "{p}: {n:?}");
    }
    assert!(hidden(&rig, at(&rig, "forge.team", &["create_group"])));
    let banner = label(&rig, at(&rig, "forge.licence", &["banner_group", "banner"]));
    assert!(banner.contains("no licence is activated"), "{banner}");
}

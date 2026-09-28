//! The services the binary gives the panels beyond `assemble` (D-4 stand-ins, labelled as
//! such in each panel's footer): the asset database, the collaboration backends. In the
//! library so the editor-wide audits (`tests/test_editor_panels_audit.rs`) open the panels
//! over exactly what the binary wires, not over an editor with no services (WP-U12).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use forge_editor::assets::{AssetCatalog, ServerCatalog};
use forge_editor::core::{CollabAttach, EditorCore, SharedCore};
use forge_editor::shell::ShellConfig;

/// D-4: the asset database over an in-memory store until the launcher opens a project
/// folder (WP-U7); the panel says so in its footer. Its importers are the editor's plugin
/// registry's (WASM importers included, WP-21).
pub fn asset_catalog(cfg: &ShellConfig) -> Result<Rc<RefCell<dyn AssetCatalog>>, String> {
    let catalog = ServerCatalog::over_extensions(&[], cfg.services.hosting.borrow().extensions())
        .map_err(|e| e.to_string())?;
    Ok(Rc::new(RefCell::new(catalog)))
}

/// Teams and collaboration (WP-U10) with a labelled in-memory Team entitlement valid a year from
/// `now_ms`, so the collaboration panels show their content without an activated licence: what
/// the editor-wide audits' populated pass and the panels' tests open. The binary reads the real
/// licence instead ([`attach_team_server_with`] over [`crate::licence::SignedFileEntitlement`]).
pub fn attach_team_server(core: &SharedCore, owner: String, baseline: &str, now_ms: u64) {
    let licence =
        Arc::new(forge_editor::collab::licence::MemoryEntitlement::team_for_a_year(now_ms));
    attach_team_server_with(core, owner, baseline, licence);
}

/// Teams and collaboration (WP-U10): the labelled in-memory identity database and team
/// server (D-4) until `forge-identity` (M5-15) and `forge-collab` / `forge-server` (M5-18),
/// over an in-memory baseline store (`baseline`: its name), gated on `licence` (the binary's:
/// the signed entitlement file, verified offline by `forge-licence`, WP-47). The panels name
/// the stand-ins in their footers; nothing here is presented as a real server.
pub fn attach_team_server_with(
    core: &SharedCore,
    owner: String,
    baseline: &str,
    licence: Arc<dyn forge_editor::collab::licence::EntitlementSource>,
) {
    use forge_project::collab::{MemoryCollab, MemoryIdentity};
    let identity = Arc::new(MemoryIdentity::new());
    let store = forge_project::memory::open_named(baseline, "forge-server");
    let server = Arc::new(MemoryCollab::new(store, identity.clone()));
    EditorCore::attach_collab(
        core,
        CollabAttach {
            identity,
            server,
            licence,
            owner,
            email: None,
        },
    );
}

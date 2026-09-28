//! `forge-panels-collab` — the first-party collaboration panels (Ch.21 §21.21, Ch.37, Ch.38),
//! a source plugin with the id `forge.panels.collab`:
//!
//! | Panel | Id | DoD | Backend (D-4) |
//! |---|---|---|---|
//! | Team and members | `forge.team` | M2-57, M5-17 | `IdentityView`; in-memory until `forge-identity` (M5-15) |
//! | Sandbox and Live/Pull | `forge.sandbox` | M2-58 | `CollabView`; in-memory until `forge-collab` (M5-18, M6-10) |
//! | Presence | `forge.presence` | M2-59, M6-13 | `CollabView` presence (in-memory) |
//! | Ownership claims | `forge.ownership` | M2-69 | `CollabView` claims (in-memory) |
//! | Publish and review queue | `forge.publish_queue` | M2-60, M6-13 | `CollabView` over a real baseline store |
//! | Conflicts | `forge.conflicts` | M2-61, M6-13 | the core's three-way merge on the reflect tree |
//! | Licence | `forge.licence` | M2-62 | `EntitlementSource`; the binary reads the signed file via `forge-licence` (WP-47) |
//!
//! Registered through the ordinary `EditorPanel` point, exactly as a third-party plugin would
//! be (I16), available under every preset (I15). **Every change to project or team state is a
//! command (I7)**: creating a team, inviting, revoking, joining, changing roles and scopes,
//! removing a member, claiming and releasing, publishing, approving and rejecting, pulling,
//! resolving conflicts, and the review rules — the core checks each against the issuer's
//! role and performs it. The direct writes are session state a person sets for themselves:
//! their Live/Pull policy, their sandbox's visibility, and presence (the shell publishes it).
//!
//! Presence also shows where people work: the hierarchy, the inspector and the viewport
//! (`forge-panels-scene`) draw who else is looking at an entity and who claimed it.

#![forbid(unsafe_code)]

pub mod conflicts;
pub mod licence;
pub mod ownership;
pub mod presence;
pub mod queue;
pub mod sandbox;
pub mod team;
mod ui;

use forge_editor::panels::PanelCx;
use forge_plugin::points::{Dock, EditorPanel, PanelDescriptor};
use forge_plugin::{InstallCx, Manifest, Order, PluginError, SourcePlugin};

/// The panels this plugin provides: `(id, title, default dock, first-run tip)`. The title
/// and the tip are localisation keys (M2-31).
pub const PANELS: &[(&str, &str, Dock, &str)] = &[
    (
        "forge.team",
        forge_ui::tr_key!("Team"),
        Dock::Right,
        forge_ui::tr_key!("A team shares one baseline: publish your sandbox to share your work."),
    ),
    (
        "forge.sandbox",
        forge_ui::tr_key!("Sandbox"),
        Dock::Right,
        forge_ui::tr_key!("Your edits stay in your sandbox until you publish them."),
    ),
    (
        "forge.presence",
        forge_ui::tr_key!("Presence"),
        Dock::Right,
        forge_ui::tr_key!("Teammates' selections show where each of them is working."),
    ),
    (
        "forge.ownership",
        forge_ui::tr_key!("Ownership"),
        Dock::Right,
        forge_ui::tr_key!("Claim what you are working on so nobody else changes it meanwhile."),
    ),
    (
        "forge.publish_queue",
        forge_ui::tr_key!("Publish queue"),
        Dock::Right,
        forge_ui::tr_key!(
            "Publishing sends your sandbox to the team; a reviewer may approve it first."
        ),
    ),
    (
        "forge.conflicts",
        forge_ui::tr_key!("Conflicts"),
        Dock::Center,
        forge_ui::tr_key!(
            "When two edits meet, choose mine, theirs or the base for each property."
        ),
    ),
    (
        "forge.licence",
        forge_ui::tr_key!("Licence"),
        Dock::Floating,
        forge_ui::tr_key!("Everything but team collaboration works without a licence."),
    ),
];

/// Collaboration panels refresh at most this often, only while visible (§21.11: presence is
/// coalesced to 10 Hz).
pub const LIVE_HZ: u8 = 10;

/// The panel ids (what the stand-in set leaves out when this plugin is loaded).
pub fn panel_ids() -> Vec<&'static str> {
    PANELS.iter().map(|(id, ..)| *id).collect()
}

/// The plugin (see the crate docs).
pub struct PanelsCollab {
    manifest: Manifest,
}

impl PanelsCollab {
    pub fn new() -> Result<Self, PluginError> {
        let provides: Vec<String> = PANELS
            .iter()
            .map(|(id, ..)| format!("EditorPanel(\"{id}\")"))
            .collect();
        let text = format!(
            // l10n: the plugin manifest, parsed, never shown
            "Plugin(id: \"forge.panels.collab\", version: \"0.1.0\", engine: \"^0.1\", kind: Source, provides: [{}])",
            provides.join(", ")
        );
        Ok(Self {
            manifest: Manifest::parse(&text)?,
        })
    }
}

impl SourcePlugin for PanelsCollab {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        for (id, title, dock, tip) in PANELS {
            let build: fn(&mut PanelCx) = match *id {
                "forge.team" => team::build,
                "forge.sandbox" => sandbox::build,
                "forge.presence" => presence::build,
                "forge.ownership" => ownership::build,
                "forge.publish_queue" => queue::build,
                "forge.conflicts" => conflicts::build,
                _ => licence::build,
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

//! `forge-project` — the project lifecycle behind the editor's headless core (Ch.33,
//! Ch.31, Ch.38 §38.7; WP-U7, ADR 0032).
//!
//! * [`format`] — a project's files (`forge-project.ron`, `project/settings.ron`,
//!   `project/scene.ron`: RON, diffable) and `forge.project.load`, the command that
//!   replaces the project with a document in one diff.
//! * [`host`] — [`host::ProjectHost`], the core's store host: create, open, save (commit
//!   with the command envelopes, Ch.33 §33.4), push and pull (fast-forwards through the
//!   `ProjectStore` trait, [`sync`]), build (through [`packager::Packager`]). **E-36: the
//!   core performs the store operations the lifecycle commands request**; only the core
//!   may name the host (the I7 guard lists it as a project-state writer).
//! * [`summary`] — semantic history: "automation:sess-4: placed 37 “Tree”".
//! * [`packager`] — targets, the E-64 attribution the packager inserts, `NOTICES`, and the
//!   labelled in-memory packager (D-4) until the HAL exporters (M8).
//! * [`memory`] — `memory:<name>` stores shared within the process (D-4, labelled).
//! * [`status`] — what the core tells its clients.
//! * [`collab`] — the team server as traits with labelled in-memory stand-ins (D-4): the
//!   identity database, the team baseline over a real `ProjectStore`, sandbox rows, presence,
//!   claims, the review queue (WP-U10, ADR 0039).
//! * [`merge`] — the three-way merge on the reflect tree and the key-preserving,
//!   precondition-checked patch a sandbox is rebased with.

#![forbid(unsafe_code)]

pub mod collab;
mod error;
pub mod format;
pub mod host;
pub mod memory;
pub mod merge;
pub mod packager;
pub mod status;
pub mod summary;
pub mod sync;

pub use error::ProjectError;

/// The project's display name (project setting).
pub const NAME_SETTING: &str = "project.name";
/// The active workspace preset's directory, `2d` / `3d` / a plugin preset's (project setting).
pub const PRESET_SETTING: &str = "project.preset";
/// The template the project was made from (project setting).
pub const TEMPLATE_SETTING: &str = "project.template";
/// The linked remote's URL (project setting: shared, so a teammate who clones gets it).
pub const REMOTE_SETTING: &str = "project.remote";
/// The product version a build stamps (project setting; default `0.1.0`).
pub const VERSION_SETTING: &str = "project.version";
/// The exported game's GPU mode (project setting, the Graphics page; ADR 0054): `Single`
/// (default) or `Multi`. A build writes it into `forge.json` as `"gpu_mode"`.
pub const GPU_MODE_SETTING: &str = "graphics.gpu_mode";

/// The `forge.json` id of a GPU-mode setting value: `"multi"` for `Multi` (any case), else
/// `"single"` — the default, whatever else the value is.
#[must_use]
pub fn gpu_mode_id(setting: Option<&str>) -> &'static str {
    match setting {
        Some(s) if s.trim().eq_ignore_ascii_case("multi") => "multi",
        _ => "single",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_project_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        assert!(md.contains("| PROJECT | forge-project |"), "prefix");
        for e in ProjectError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-project |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code));
        }
    }
}

//! What the core tells its clients about the project's lifecycle: plain data, delivered
//! through the `BusClient` (the split editor's transport carries it the same way), read by
//! the launcher, revision history, build and Project settings panels.

use crate::packager::BuildReport;

/// One revision as the history panel lists it: semantic, from its command envelopes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RevisionInfo {
    /// The full revision id (64 hex digits).
    pub id: String,
    /// Its first 10 hex digits.
    pub short: String,
    pub message: String,
    pub at_ms: u64,
    /// Distinct issuers, first-seen order (`human:ada`, `automation:sess-4`).
    pub issuers: Vec<String>,
    pub commands: usize,
    /// What each issuer did (`summary::summarize`).
    pub summary: Vec<String>,
}

/// The open project.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpenProjectInfo {
    pub name: String,
    /// Where it is (`file:C:\Projects\Orbits`, `memory:demo`).
    pub location: String,
    /// The store backend (`local-fs`, `memory`).
    pub backend: String,
    /// Whether the store keeps nothing past this process (D-4 in-memory).
    pub in_memory: bool,
    /// The newest revision, if it was ever saved.
    pub head: Option<String>,
    /// Newest first, at most [`crate::host::HISTORY_SHOWN`].
    pub revisions: Vec<RevisionInfo>,
    /// Changed since the last open or save.
    pub dirty: bool,
    /// The first save just happened and no remote is linked: offer to link one (O-11).
    pub offer_remote: bool,
    /// The remote's head after the last push or pull, if one happened.
    pub remote_head: Option<String>,
    /// The project's own workspace presets (`presets/<dir>/` files, text; WP-16): a
    /// project copy of `3d` is what 3D means for this project (its defaults, layout,
    /// new-scene template).
    pub presets: crate::host::PresetFiles,
}

/// The result of one lifecycle command.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    /// Increasing per core.
    pub id: u64,
    /// The command target (`forge.project.save`).
    pub op: String,
    /// Who asked (`human:ada`).
    pub issuer: String,
    /// `Ok(what happened)` or `Err((code, message))`.
    pub result: Result<String, (String, String)>,
    /// This was the project's first save (the "link a remote" offer, O-11).
    pub first_save: bool,
}

/// The whole lifecycle state (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectStatus {
    /// Bumped on every change; a client compares it to know whether to look again.
    pub generation: u64,
    /// Bumped on every load (create, open, a pull that moved): the project was replaced
    /// and the undo history cleared, so a client re-reads both.
    pub epoch: u64,
    pub open: Option<OpenProjectInfo>,
    /// The newest outcomes, oldest first, at most [`crate::host::OUTCOMES_KEPT`].
    pub outcomes: Vec<Outcome>,
    pub last_build: Option<BuildReport>,
    /// The packager's name and whether it is the in-memory stand-in.
    pub packager: (String, bool),
    /// A sign-in in progress (the code to type and where) or just finished (WP-16).
    pub sign_in: Option<SignInStatus>,
    /// The accounts signed in to this session (`ada on github.com`): tokens are never here.
    pub accounts: Vec<String>,
    /// The transfer running now, off the core's lock (a push, pull, clone, sign-in or
    /// repository creation): the panels say so while it runs; its result arrives as an
    /// outcome.
    pub transfer: Option<TransferInfo>,
}

/// A lifecycle transfer in progress: its network part runs on its own thread, so every
/// client of the core — the UI's thread too — keeps working meanwhile.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransferInfo {
    /// The command target (`forge.project.push`).
    pub op: String,
    /// Who asked (`human:ada`).
    pub issuer: String,
    /// What it is doing, for the user (`Pushing to https://github.com/ada/orbits.git`).
    pub what: String,
}

/// A Git host sign-in (GitHub's device flow): what the user needs to see.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SignInStatus {
    /// `GitHub`.
    pub provider: String,
    /// While waiting: the code to type.
    pub user_code: Option<String>,
    /// While waiting: where to type it.
    pub verification_uri: Option<String>,
    /// Once approved: the account.
    pub account: Option<String>,
}

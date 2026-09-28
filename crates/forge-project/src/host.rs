//! The core's **project host** (E-36: the core performs the store operations the lifecycle
//! commands request). The editor's headless core owns exactly one; nothing else may name
//! it — `test_command_liveness` lists it as a project-state writer, so a panel that reached
//! for it would fail I7 exactly as one reaching for `ProjectStore` does.
//!
//! The host holds the open project's [`ProjectStore`] (opened by URL through a
//! [`StoreOpener`]: `file:` — `LocalFs`, the default, a plain folder — `git:` — a folder
//! whose history is a Git repository — and `memory:`, the labelled in-process stores; a remote is
//! any of those or a Git remote, `https:`, `http:`, `git-file:`, WP-16), the command envelopes
//! applied since the last save (the semantic history a commit carries, Ch.33 §33.4), whether
//! anything changed, and a [`ProjectStatus`] the core hands its clients. It never touches the bus:
//! the core reads the project for it and applies the documents it returns.

use std::collections::VecDeque;
use std::sync::Arc;

use forge_cmd::{CommandEnvelope, Project, TxnId, Value};
use forge_plugin::{Extensions, Grants, loader};
use forge_store::{Bytes, ProjectStore, RevId, RevRange, StoreBackend, StorePath};

use crate::format::{MANIFEST_PATH, ProjectDoc, SCENE_PATH, SETTINGS_PATH};
use crate::packager::{BuildReport, MemoryPackager, Packager, Product, Target};
use crate::status::{
    OpenProjectInfo, Outcome, ProjectStatus, RevisionInfo, SignInStatus, TransferInfo,
};
use crate::summary::summarize;
use crate::sync::{self, SyncReport};
use crate::{NAME_SETTING, ProjectError, VERSION_SETTING};
// The sign-in pieces a transfer carries off the editor core's lock.
pub use forge_store_backends::git::Credential;
use forge_store_backends::git::github::Poll;
pub use forge_store_backends::git::github::{DeviceCode, GitHub};
use forge_store_backends::git::{GitOptions, host_of};

/// Revisions the status lists (newest first).
pub const HISTORY_SHOWN: usize = 100;
/// Outcomes the status keeps.
pub const OUTCOMES_KEPT: usize = 32;
/// Envelopes kept for the next commit; beyond it the oldest are dropped and the commit
/// message says how many (the files are still complete — only their story is shortened).
pub const PENDING_CAP: usize = 200_000;

/// Opens a store by URL for an identity.
pub type StoreOpener =
    Arc<dyn Fn(&str, &str) -> Result<Box<dyn ProjectStore>, ProjectError> + Send + Sync>;

/// The HTTPS form of an SSH Git URL (`git@github.com:ada/orbits.git`,
/// `ssh://git@host/ada/orbits.git`), for the refusal's suggestion.
fn https_of_ssh(u: &str) -> Option<String> {
    let rest = u
        .strip_prefix("ssh://")
        .map(|r| r.split_once('@').map_or(r, |(_, h)| h).to_string())
        .or_else(|| u.split_once('@').map(|(_, h)| h.replacen(':', "/", 1)))?;
    let (host, path) = rest.split_once('/')?;
    let host = host.split(':').next().unwrap_or(host);
    Some(format!("https://{host}/{path}"))
}

/// Whether `url` can be a remote (the link and clone commands check it before it is
/// stored): `file:<folder>` (a Forge store in a folder — a NAS share too), a Git remote
/// (`https://…`, `http://…`: GitHub, GitLab, Gitea, Forgejo; `git-file:<path>`: a bare
/// repository on a share) and `memory:<name>` (in this process, D-4). SSH Git URLs are
/// refused with their HTTPS form; S3 and SQL remotes with what works instead (their backends
/// are `UNBUILT`). A syntactic check: nothing is contacted.
pub fn check_remote(url: &str) -> Result<(), ProjectError> {
    let unsupported = |why: &str| {
        Err(ProjectError::UnsupportedRemote {
            url: url.to_string(),
            why: why.to_string(),
        })
    };
    let u = url.trim();
    if let Some(rest) = u
        .strip_prefix("file:")
        .or_else(|| u.strip_prefix("memory:"))
        .or_else(|| u.strip_prefix("git-file:"))
    {
        if rest.trim().is_empty() {
            return unsupported("the URL names no location");
        }
        return Ok(());
    }
    let lower = u.to_ascii_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = lower.strip_prefix(scheme) {
            let path = rest.split_once('/').map_or("", |(_, p)| p);
            if rest.split('/').next().is_none_or(str::is_empty) || path.trim_matches('/').is_empty()
            {
                return unsupported("the URL names no repository (https://host/owner/name.git)");
            }
            return Ok(());
        }
    }
    if lower.starts_with("ssh://") || lower.starts_with("git@") {
        let hint = https_of_ssh(u).map_or_else(String::new, |h| format!(" ({h})"));
        return unsupported(&format!(
            "SSH Git remotes are not supported yet: use the repository's HTTPS URL{hint} and sign \
             in (GitHub: the device flow)."
        ));
    }
    if lower.starts_with("git:") {
        return unsupported(
            "git:<folder> is a project folder, not a remote: share it through a bare Git \
             repository (git-file:<path>), a Git host (https://…) or a Forge folder (file:<folder>).",
        );
    }
    if lower.starts_with("s3://") || lower.starts_with("minio://") {
        return unsupported(
            "S3-compatible remotes (MinIO, R2, B2, S3) need the S3 store backend, which is not \
             built yet (UNBUILT). Use a Git remote or file:<folder> for now.",
        );
    }
    if lower.starts_with("postgres://")
        || lower.starts_with("postgresql://")
        || lower.starts_with("sqlite:")
    {
        return unsupported(
            "SQL remotes (SQLite, Postgres) need the SQL store backend, which is not built yet \
             (UNBUILT). Use a Git remote or file:<folder> for now.",
        );
    }
    unsupported(
        "not a remote URL: use https://host/owner/name.git (a Git host), git-file:<path> (a bare \
         Git repository), file:<folder> (a Forge folder) or memory:<name> (in this process)",
    )
}

/// Whether `path` is a Git repository: a bare one (`HEAD` and `objects/`) or a work tree's
/// `.git`.
#[must_use]
pub fn is_git_repository(path: &std::path::Path) -> bool {
    (path.join("HEAD").is_file() && path.join("objects").is_dir()) || path.join(".git").is_dir()
}

/// The remote URL for what a user typed: a URL as it is; a path to a Git repository (or a
/// path ending in `.git`) as `git-file:<path>`; any other path as a Forge folder,
/// `file:<path>`. `file:<path>` naming a Git repository becomes `git-file:<path>` too.
/// (The UI calls this, with its one filesystem look; the commands check syntax only.)
#[must_use]
pub fn remote_url(typed: &str) -> String {
    let t = typed.trim();
    let path = t.strip_prefix("file:").unwrap_or(t);
    let has_scheme = |s: &str| {
        s.contains("://")
            || ["memory:", "git-file:", "git:", "git@", "sqlite:"]
                .iter()
                .any(|p| s.starts_with(p))
    };
    if has_scheme(t) && !t.starts_with("file:") {
        return t.to_string();
    }
    let p = std::path::Path::new(path);
    if is_git_repository(p) || path.trim_end_matches(['/', '\\']).ends_with(".git") {
        format!("git-file:{path}")
    } else {
        format!("file:{path}")
    }
}

/// The name a clone of `remote` gets by default: its last path segment without `.git`.
#[must_use]
pub fn clone_name(remote: &str) -> String {
    remote
        .rsplit(['/', '\\', ':'])
        .find(|s| !s.is_empty())
        .map(|s| s.strip_suffix(".git").unwrap_or(s).to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Cloned Project".to_string())
}

/// The first-party opener with the per-user Git mirror cache and no sign-in.
pub fn first_party_opener() -> Result<StoreOpener, ProjectError> {
    first_party_opener_with(forge_store_backends::git::GitOptions::new())
}

/// The first-party opener: every scheme the first-party store plugin registers on the
/// `StoreBackend` point (`file:`, `git:`, and the Git remotes `https:`, `http:`,
/// `git-file:`; loaded like any plugin, I16), with `git` for remote views (their mirror
/// cache and the credentials sign-in fills); `memory:` through the in-process registry.
pub fn first_party_opener_with(
    git: forge_store_backends::git::GitOptions,
) -> Result<StoreOpener, ProjectError> {
    let mut x = Extensions::new();
    let plugin =
        |e: forge_plugin::PluginError| ProjectError::BadFiles(format!("store plugins: {e}"));
    x.define::<StoreBackend>().map_err(plugin)?;
    let stores = forge_store_backends::StoreBackends::with_git(git).map_err(plugin)?;
    loader::load(&mut x, &[&stores], &[], &Grants::new()).map_err(plugin)?;
    // The registered backends by scheme (their open functions are shareable; the extension
    // set as a whole is not).
    let backends: std::collections::BTreeMap<String, forge_store::OpenFn> = x
        .registry::<StoreBackend>()
        .map(|r| {
            r.iter()
                .map(|(k, d)| (k.to_string(), d.open.clone()))
                .collect()
        })
        .unwrap_or_default();
    Ok(Arc::new(move |url: &str, who: &str| {
        let url = url.trim();
        if let Some(name) = url.strip_prefix("memory:") {
            return Ok(crate::memory::open_named(name, who));
        }
        let opened = url
            .split_once(':')
            .and_then(|(scheme, loc)| backends.get(scheme).map(|open| open(loc, who)))
            .unwrap_or_else(|| Err(forge_store::StoreError::UnknownBackend(url.to_string())));
        opened.map_err(|e| match e {
            forge_store::StoreError::UnknownBackend(_) => ProjectError::UnsupportedRemote {
                url: url.to_string(),
                why: "a project location is file:<folder>, git:<folder> or memory:<name>; a \
                      remote also https://…, http://… or git-file:<path>"
                    .into(),
            },
            other => ProjectError::from(other),
        })
    }))
}

/// What happens to a pending envelope at a save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// Its transaction is committed: it is part of this revision's story.
    Include,
    /// Its transaction is still open (a gesture in progress): keep it for the next save.
    Hold,
    /// Undone or cancelled: it changed nothing that is saved.
    Drop,
}

/// What a save did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveInfo {
    /// `None`: nothing had changed, nothing was committed.
    pub rev: Option<RevId>,
    /// The project's first revision.
    pub first: bool,
    pub commands: usize,
}

struct Open {
    location: String,
    name: String,
    store: Box<dyn ProjectStore>,
    revisions: Vec<RevisionInfo>,
    offer_remote: bool,
    remote_head: Option<RevId>,
    presets: PresetFiles,
}

/// See the module docs.
pub struct ProjectHost {
    opener: StoreOpener,
    packager: Box<dyn Packager>,
    open: Option<Open>,
    pending: Vec<CommandEnvelope>,
    pending_dropped: u64,
    dirty: bool,
    outcomes: VecDeque<Outcome>,
    next_outcome: u64,
    generation: u64,
    epoch: u64,
    last_build: Option<BuildReport>,
    status: Arc<ProjectStatus>,
    /// Git remote views' options: the credentials a sign-in fills (`None`: a host built
    /// with a custom opener; sign-in then says it is not available).
    git: Option<GitOptions>,
    /// The GitHub instance and OAuth app to sign in with (`None`: none configured).
    github: Option<GitHub>,
    sign_in: Option<(GitHub, DeviceCode)>,
    sign_in_status: Option<SignInStatus>,
    accounts: Vec<String>,
    /// The transfer running off the core's lock, if any (one at a time).
    transfer: Option<TransferInfo>,
    /// Settings no save or build writes (the host's per-run state, WP-33).
    per_run: Option<SettingFilter>,
    /// Settings every save and build writes as the project's files hold them, not as the
    /// project in memory does (the editor's held security proposal, WP-33).
    file_settings: Vec<FileSetting>,
}

/// A setting the host writes as the project's files hold it while the project in memory
/// still holds what the load wrote in its place (see [`ProjectHost::set_file_settings`]).
#[derive(Clone, Debug, PartialEq)]
pub struct FileSetting {
    pub key: String,
    /// What the files hold (`None`: no such key).
    pub file: Option<Value>,
    /// What the load wrote instead (`None`: no such key). Once the project holds anything
    /// else (a person changed it), the project's value is written.
    pub loaded: Option<Value>,
}

/// Names the settings a host keeps out of every file it writes (see
/// [`ProjectHost::set_per_run_settings`]).
pub type SettingFilter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

impl std::fmt::Debug for ProjectHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectHost")
            .field("open", &self.open.as_ref().map(|o| &o.location))
            .field("dirty", &self.dirty)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

fn revision_info(store: &dyn ProjectStore, r: &forge_store::Rev) -> RevisionInfo {
    let summary = store
        .commands(&r.id)
        .map(|c| summarize(&c))
        .unwrap_or_else(|e| vec![format!("(its command log does not read: {e})")]);
    let id = r.id.to_string();
    RevisionInfo {
        short: id.chars().take(10).collect(),
        id,
        message: r.message.clone(),
        at_ms: r.at_ms,
        issuers: r.issuers.clone(),
        commands: r.commands,
        summary,
    }
}

fn revisions(store: &dyn ProjectStore) -> Result<Vec<RevisionInfo>, ProjectError> {
    Ok(store
        .history(RevRange::last(HISTORY_SHOWN))?
        .iter()
        .map(|r| revision_info(store, r))
        .collect())
}

fn read_text(store: &dyn ProjectStore, path: &str) -> Result<Option<String>, ProjectError> {
    let p = StorePath::new(path)?;
    match store.read(&p) {
        Ok(b) => String::from_utf8(b.to_vec())
            .map(Some)
            .map_err(|_| ProjectError::BadFiles(format!("{path} is not UTF-8 text"))),
        Err(forge_store::StoreError::NotFound(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// A project's own workspace presets: `presets/<dir>/…` files in its store, as text, by
/// preset directory then file name relative to it (Ch.31 §31.3: "users author their own
/// presets by copying one" — into the project, where they travel with it).
pub type PresetFiles =
    std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>;

/// Where a project keeps its own presets.
pub const PRESETS_DIR: &str = "presets";

/// Read a project's own preset files (text files under `presets/<dir>/`; anything that is
/// not UTF-8 is not a preset file and is skipped).
fn read_presets(store: &dyn ProjectStore) -> Result<PresetFiles, ProjectError> {
    let mut out = PresetFiles::new();
    for p in store.list()? {
        let Some(rest) = p
            .as_str()
            .strip_prefix(PRESETS_DIR)
            .and_then(|r| r.strip_prefix('/'))
        else {
            continue;
        };
        let Some((dir, file)) = rest.split_once('/') else {
            continue;
        };
        if let Ok(text) = String::from_utf8(store.read(&p)?.to_vec()) {
            out.entry(dir.to_string())
                .or_default()
                .insert(file.to_string(), text);
        }
    }
    Ok(out)
}

/// Read a project's files from its store.
fn read_doc(
    store: &dyn ProjectStore,
    location: &str,
) -> Result<(String, ProjectDoc), ProjectError> {
    let manifest = read_text(store, MANIFEST_PATH)?
        .ok_or_else(|| ProjectError::NotAProject(location.to_string()))?;
    let settings = read_text(store, SETTINGS_PATH)?;
    let scene = read_text(store, SCENE_PATH)?;
    let (m, doc) = ProjectDoc::decode(&manifest, settings.as_deref(), scene.as_deref())?;
    Ok((m.name, doc))
}

/// Write the document's files, skipping any whose bytes are unchanged; returns
/// `(path, size)` of each.
fn write_doc(
    store: &mut dyn ProjectStore,
    doc: &ProjectDoc,
) -> Result<Vec<(String, u64)>, ProjectError> {
    let mut sizes = Vec::new();
    for (path, text) in doc.encode()? {
        let p = StorePath::new(path)?;
        let bytes = Bytes::from(text.into_bytes());
        sizes.push((path.to_string(), bytes.len() as u64));
        let same = store.read(&p).map(|b| b == bytes).unwrap_or(false);
        if !same {
            store.write(&p, bytes)?;
        }
    }
    Ok(sizes)
}

fn text_of(p: &Project, key: &str) -> Option<String> {
    match p.setting(key) {
        Some(Value::Text(t)) if !t.trim().is_empty() => Some(t.clone()),
        _ => None,
    }
}

impl ProjectHost {
    /// A host with `opener` and `packager`; no project is open.
    #[must_use]
    pub fn new(opener: StoreOpener, packager: Box<dyn Packager>) -> Self {
        let mut h = Self {
            opener,
            packager,
            open: None,
            pending: Vec::new(),
            pending_dropped: 0,
            dirty: false,
            outcomes: VecDeque::new(),
            next_outcome: 1,
            generation: 0,
            epoch: 0,
            last_build: None,
            status: Arc::new(ProjectStatus::default()),
            git: None,
            github: None,
            sign_in: None,
            sign_in_status: None,
            accounts: Vec::new(),
            transfer: None,
            per_run: None,
            file_settings: Vec::new(),
        };
        h.touch();
        h
    }

    /// The first-party opener and the in-memory packager, Git remotes mirrored under the
    /// per-user cache, GitHub sign-in with the OAuth app `FORGE_GITHUB_CLIENT_ID` names.
    pub fn first_party() -> Result<Self, ProjectError> {
        Self::first_party_with(GitOptions::new())
    }

    /// [`ProjectHost::first_party`] with explicit Git options (tests: a private cache, a
    /// client for a mock server).
    pub fn first_party_with(git: GitOptions) -> Result<Self, ProjectError> {
        let mut h = Self::new(
            first_party_opener_with(git.clone())?,
            Box::new(MemoryPackager),
        );
        h.github = GitHub::from_env(std::sync::Arc::clone(&git.http));
        h.git = Some(git);
        Ok(h)
    }

    /// Sign in to `gh` instead of the configured GitHub (tests: a mock server; GitHub
    /// Enterprise).
    pub fn set_github(&mut self, gh: Option<GitHub>) {
        self.github = gh;
    }

    /// Bumped on every status change.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Bumped on every load (create, open, a pull that moved): clients re-read their
    /// history, which a load clears.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The status (cheap: shared).
    #[must_use]
    pub fn status(&self) -> Arc<ProjectStatus> {
        Arc::clone(&self.status)
    }

    /// Whether anything changed since the last open or save.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether a project is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Where the open project is (`file:<folder>`, `memory:<name>`), as soon as it is opened,
    /// created or adopted (the status follows at the next change).
    #[must_use]
    pub fn location(&self) -> Option<&str> {
        self.open.as_ref().map(|o| o.location.as_str())
    }

    fn touch(&mut self) {
        self.generation += 1;
        let (backend, in_mem) = self.packager.backend();
        let open = self.open.as_ref().map(|o| OpenProjectInfo {
            name: o.name.clone(),
            location: o.location.clone(),
            backend: o.store.backend().to_string(),
            in_memory: o.store.backend() == "memory",
            head: o.revisions.first().map(|r| r.id.clone()),
            revisions: o.revisions.clone(),
            dirty: self.dirty,
            offer_remote: o.offer_remote,
            remote_head: o.remote_head.map(|h| h.to_string()),
            presets: o.presets.clone(),
        });
        self.status = Arc::new(ProjectStatus {
            generation: self.generation,
            epoch: self.epoch,
            open,
            outcomes: self.outcomes.iter().cloned().collect(),
            last_build: self.last_build.clone(),
            packager: (backend, in_mem),
            sign_in: self.sign_in_status.clone(),
            accounts: self.accounts.clone(),
            transfer: self.transfer.clone(),
        });
    }

    /// A command that changed the project was applied: keep its envelope for the next
    /// commit's story.
    pub fn record(&mut self, e: CommandEnvelope) {
        if self.pending.len() >= PENDING_CAP {
            self.pending.remove(0);
            self.pending_dropped += 1;
        }
        self.pending.push(e);
    }

    /// Something changed the project (an edit, an undo, a redo, a cancel). Only the first
    /// change after a save changes the status.
    pub fn mark_dirty(&mut self) {
        if self.open.is_some() && !self.dirty {
            self.dirty = true;
            self.touch();
        }
    }

    /// The core applied a project document (create, open, pull): nothing is unsaved, and
    /// the envelopes before it are not this project's story.
    pub fn loaded(&mut self) {
        self.pending.clear();
        self.pending_dropped = 0;
        self.dirty = false;
        self.epoch += 1;
        self.touch();
    }

    /// Record a lifecycle command's outcome.
    pub fn finish(
        &mut self,
        op: &str,
        issuer: &str,
        result: Result<String, ProjectError>,
        first_save: bool,
    ) {
        let id = self.next_outcome;
        self.next_outcome += 1;
        if self.outcomes.len() >= OUTCOMES_KEPT {
            self.outcomes.pop_front();
        }
        self.outcomes.push_back(Outcome {
            id,
            op: op.to_string(),
            issuer: issuer.to_string(),
            result: result.map_err(|e| (e.code().to_string(), e.to_string())),
            first_save,
        });
        self.touch();
    }

    // ---- transfers (WP-16: the network runs off the core's lock) ------------------------

    /// Refused while a transfer runs: another push, pull, clone, sign-in or repository
    /// creation, and anything that would replace the open project under it (create, open).
    pub fn check_idle(&self) -> Result<(), ProjectError> {
        match &self.transfer {
            Some(t) => Err(ProjectError::Busy(t.what.clone())),
            None => Ok(()),
        }
    }

    /// A transfer starts: the status names it until [`ProjectHost::finish_transfer`].
    pub fn begin_transfer(
        &mut self,
        op: &str,
        issuer: &str,
        what: String,
    ) -> Result<(), ProjectError> {
        self.check_idle()?;
        self.transfer = Some(TransferInfo {
            op: op.to_string(),
            issuer: issuer.to_string(),
            what,
        });
        self.touch();
        Ok(())
    }

    /// The transfer ended: record its outcome and clear it from the status (one change).
    pub fn finish_transfer(
        &mut self,
        op: &str,
        issuer: &str,
        result: Result<String, ProjectError>,
    ) {
        self.transfer = None;
        self.finish(op, issuer, result, false);
    }

    /// The opener, for a transfer to open a remote without the host (opening a Git remote
    /// fetches it).
    #[must_use]
    pub fn opener(&self) -> StoreOpener {
        Arc::clone(&self.opener)
    }

    /// Create a project at `location` holding `doc` (its files written; not committed —
    /// the first save commits, and that is when a remote is offered, O-11). The caller
    /// loads `doc` into the core.
    pub fn create(
        &mut self,
        location: &str,
        identity: &str,
        doc: &ProjectDoc,
    ) -> Result<(), ProjectError> {
        let mut store = (self.opener)(location, identity)?;
        if read_text(store.as_ref(), MANIFEST_PATH)?.is_some() {
            return Err(ProjectError::Exists(location.to_string()));
        }
        write_doc(store.as_mut(), doc)?;
        self.open = Some(Open {
            location: location.to_string(),
            name: doc.manifest().name,
            revisions: revisions(store.as_ref())?,
            presets: read_presets(store.as_ref())?,
            store,
            offer_remote: false,
            remote_head: None,
        });
        Ok(())
    }

    /// Open the project at `location`; the caller loads the returned document.
    pub fn open_project(
        &mut self,
        location: &str,
        identity: &str,
    ) -> Result<ProjectDoc, ProjectError> {
        let store = (self.opener)(location, identity)?;
        let (name, doc) = read_doc(store.as_ref(), location)?;
        self.open = Some(Open {
            location: location.to_string(),
            name,
            revisions: revisions(store.as_ref())?,
            presets: read_presets(store.as_ref())?,
            store,
            offer_remote: false,
            remote_head: None,
        });
        Ok(doc)
    }

    /// Clone the project at `remote` into a new project at `location` (Ch.33 §33.5 "any
    /// remote, paste a URL"): every revision is replayed into the new store with its id
    /// intact, and the caller loads the returned document.
    pub fn clone_project(
        &mut self,
        remote: &str,
        location: &str,
        identity: &str,
    ) -> Result<ProjectDoc, ProjectError> {
        let c = clone_into(&self.opener, remote, location, identity)?;
        Ok(self.adopt_clone(c))
    }

    /// Make a finished [`clone_into`] the open project; the caller loads the returned
    /// document.
    pub fn adopt_clone(&mut self, c: Cloned) -> ProjectDoc {
        self.open = Some(c.open);
        c.doc
    }

    /// Keep the settings `filter` names out of every file a save or a build writes: per-run
    /// state such as the editor's automation grants, whose value is the run's epoch (WP-33; the
    /// editor's load drops them too). `None`: every setting is written.
    pub fn set_per_run_settings(&mut self, filter: Option<SettingFilter>) {
        self.per_run = filter;
    }

    /// Write `settings` as the project's files hold them, not as the project in memory does,
    /// while the project still holds each one's `loaded` value: a save or a build then leaves
    /// them on disk as they are. The editor holds a non-human's widened security settings for
    /// a person this way (WP-33): until the person answers, a save by anyone keeps the files'
    /// plugin grants and default automation policy, so no commit (and no push) reverts a teammate's
    /// grants. Empty: every setting is written as the project holds it.
    pub fn set_file_settings(&mut self, settings: Vec<FileSetting>) {
        self.file_settings = settings;
    }

    /// The project as a save, a build or a team publish writes it: without the per-run
    /// settings, and with the [`FileSetting`]s as the files hold them.
    #[must_use]
    pub fn written_doc(&self, project: &Project) -> ProjectDoc {
        let mut doc = match &self.per_run {
            Some(f) => ProjectDoc::from_project_without(project, f.as_ref()),
            None => ProjectDoc::from_project(project),
        };
        for f in &self.file_settings {
            if project.setting(&f.key) != f.loaded.as_ref() {
                continue; // changed since the load: the project's value is written
            }
            match &f.file {
                Some(v) => doc.settings.insert(f.key.clone(), v.clone()),
                None => doc.settings.remove(&f.key),
            };
        }
        doc
    }

    /// Save: write the project's files and commit them with the envelopes of committed
    /// transactions since the last save. `message` empty: the summary's first line.
    /// `remote_linked`: whether to offer a remote after a first save.
    pub fn save(
        &mut self,
        project: &Project,
        message: &str,
        remote_linked: bool,
        keep: &dyn Fn(TxnId) -> Keep,
    ) -> Result<SaveInfo, ProjectError> {
        let dirty = self.dirty;
        let open = self.open.as_mut().ok_or(ProjectError::NotOpen)?;
        let had_head = open.store.head()?.is_some();
        if had_head && !dirty {
            return Ok(SaveInfo {
                rev: None,
                first: false,
                commands: 0,
            });
        }
        let doc = self.written_doc(project);
        let open = self.open.as_mut().ok_or(ProjectError::NotOpen)?;
        write_doc(open.store.as_mut(), &doc)?;
        let mut story = Vec::new();
        let mut held = Vec::new();
        for e in self.pending.drain(..) {
            match keep(e.txn) {
                Keep::Include => story.push(e),
                Keep::Hold => held.push(e),
                Keep::Drop => {}
            }
        }
        let msg = if message.trim().is_empty() {
            summarize(&story)
                .first()
                .cloned()
                .unwrap_or_else(|| "Save".to_string())
        } else {
            message.trim().to_string()
        };
        let msg = if self.pending_dropped > 0 {
            format!(
                "{msg} ({} earlier command(s) not recorded: over {PENDING_CAP})",
                self.pending_dropped
            )
        } else {
            msg
        };
        let rev = match open.store.commit(&msg, &story) {
            Ok(r) => r,
            Err(e) => {
                // Nothing was committed: the story stays for the next try.
                story.extend(held);
                self.pending = story;
                return Err(e.into());
            }
        };
        self.pending = held;
        self.pending_dropped = 0;
        if let Some(r) = open.store.history(RevRange::last(1))?.first() {
            let info = revision_info(open.store.as_ref(), r);
            open.revisions.insert(0, info);
            open.revisions.truncate(HISTORY_SHOWN);
        }
        open.name = text_of(project, NAME_SETTING).unwrap_or_else(|| open.name.clone());
        let first = !had_head;
        open.offer_remote = first && !remote_linked;
        self.dirty = false;
        self.touch();
        Ok(SaveInfo {
            rev: Some(rev),
            first,
            commands: story.len(),
        })
    }

    /// The linked remote was changed (the offer is answered).
    pub fn remote_linked(&mut self) {
        if let Some(o) = self.open.as_mut()
            && o.offer_remote
        {
            o.offer_remote = false;
            self.touch();
        }
    }

    /// Push the project's revisions to `remote` (a fast-forward). The editor core runs the
    /// same steps with the remote's opening and [`sync::publish`] off its lock.
    pub fn push(&mut self, remote: &str, identity: &str) -> Result<SyncReport, ProjectError> {
        self.push_check(remote)?;
        let mut r = (self.opener)(remote.trim(), identity)?;
        let copied = self.push_replay(r.as_mut())?;
        let rep = sync::publish(r.as_mut(), copied)?;
        self.pushed(rep.head);
        Ok(rep)
    }

    /// A push's checks, before anything is contacted.
    pub fn push_check(&self, remote: &str) -> Result<(), ProjectError> {
        check_remote(remote)?;
        self.open.as_ref().map(drop).ok_or(ProjectError::NotOpen)
    }

    /// A push's replay onto an opened `remote` ([`sync::push_replay`]; reads the project's
    /// store, so the core does it under its lock).
    pub fn push_replay(&self, remote: &mut dyn ProjectStore) -> Result<Vec<RevId>, ProjectError> {
        let open = self.open.as_ref().ok_or(ProjectError::NotOpen)?;
        sync::push_replay(open.store.as_ref(), remote)
    }

    /// A push published: the remote's head is `head`.
    pub fn pushed(&mut self, head: Option<RevId>) {
        if let Some(open) = self.open.as_mut() {
            open.remote_head = head;
            open.offer_remote = false;
        }
        self.touch();
    }

    /// Pull `remote`'s revisions (a fast-forward). Refused with unsaved changes. Returns
    /// the document to load when something moved. The editor core opens the remote (the
    /// fetch) off its lock and calls [`ProjectHost::pull_from`] under it.
    pub fn pull(
        &mut self,
        remote: &str,
        identity: &str,
    ) -> Result<(SyncReport, Option<ProjectDoc>), ProjectError> {
        self.pull_check(remote)?;
        let r = (self.opener)(remote.trim(), identity)?;
        self.pull_from(r.as_ref())
    }

    /// A pull's checks, before anything is contacted.
    pub fn pull_check(&self, remote: &str) -> Result<(), ProjectError> {
        check_remote(remote)?;
        if self.dirty {
            return Err(ProjectError::Unsaved);
        }
        self.open.as_ref().map(drop).ok_or(ProjectError::NotOpen)
    }

    /// Pull from an opened remote `r` (local work only: the fetch happened when it
    /// opened). Checked again here: the project may have changed while it was fetched.
    pub fn pull_from(
        &mut self,
        r: &dyn ProjectStore,
    ) -> Result<(SyncReport, Option<ProjectDoc>), ProjectError> {
        if self.dirty {
            return Err(ProjectError::Unsaved);
        }
        let open = self.open.as_mut().ok_or(ProjectError::NotOpen)?;
        let rep = sync::pull(r, open.store.as_mut())?;
        open.remote_head = rep.head;
        let doc = if rep.revisions.is_empty() {
            None
        } else {
            open.revisions = revisions(open.store.as_ref())?;
            open.presets = read_presets(open.store.as_ref())?;
            let (name, doc) = read_doc(open.store.as_ref(), &open.location)?;
            open.name = name;
            Some(doc)
        };
        self.touch();
        Ok((rep, doc))
    }

    // ---- sign-in (Ch.33.5: the new-project dialog's GitHub option) ----------------------

    fn sign_in_unavailable(&self) -> ProjectError {
        ProjectError::SignIn(
            "GitHub sign-in needs Forge's OAuth app, which is not registered yet (UNBUILT, \
             C-github-oauth-app): set FORGE_GITHUB_CLIENT_ID to an OAuth app's client id. A \
             Git remote on your network or a bare repository on a share needs no sign-in."
                .into(),
        )
    }

    /// Start signing in to GitHub (the device flow): the status then shows the code to type
    /// and where. Returns what to tell the user.
    pub fn sign_in_start(&mut self) -> Result<String, ProjectError> {
        let gh = self.sign_in_client()?;
        let code = gh.start("repo")?;
        Ok(self.sign_in_started(gh, code))
    }

    /// The GitHub to sign in to (the core then asks it for a code off its lock).
    pub fn sign_in_client(&self) -> Result<GitHub, ProjectError> {
        self.github
            .clone()
            .filter(|_| self.git.is_some())
            .ok_or_else(|| self.sign_in_unavailable())
    }

    /// GitHub answered the start with `code`: show it. Returns what to tell the user.
    pub fn sign_in_started(&mut self, gh: GitHub, code: DeviceCode) -> String {
        let say = format!(
            "Enter {} at {}, then press Continue",
            code.user_code, code.verification_uri
        );
        self.sign_in_status = Some(SignInStatus {
            provider: "GitHub".into(),
            user_code: Some(code.user_code.clone()),
            verification_uri: Some(code.verification_uri.clone()),
            account: None,
        });
        self.sign_in = Some((gh, code));
        self.touch();
        say
    }

    /// Ask GitHub once whether the user has approved the sign-in (the caller decides when:
    /// the user presses Continue, or a script polls at the interval GitHub named). When
    /// approved, the token goes into the credentials Git remotes use — never into the
    /// status, a setting or a message.
    pub fn sign_in_continue(&mut self) -> Result<String, ProjectError> {
        let (gh, code) = self.sign_in_pending()?;
        let answer = sign_in_poll(&gh, &code)?;
        self.sign_in_answered(&gh, &code, answer)
    }

    /// The sign-in in progress (the core then polls GitHub off its lock).
    pub fn sign_in_pending(&self) -> Result<(GitHub, DeviceCode), ProjectError> {
        self.sign_in
            .clone()
            .ok_or_else(|| ProjectError::SignIn("no sign-in is in progress: start one".into()))
    }

    /// GitHub's answer to one poll of `code`: record it. What to tell the user.
    pub fn sign_in_answered(
        &mut self,
        gh: &GitHub,
        code: &DeviceCode,
        answer: SignInAnswer,
    ) -> Result<String, ProjectError> {
        let current = self.sign_in.as_ref().is_some_and(|(_, c)| c == code);
        if !current {
            return Err(ProjectError::SignIn(
                "that sign-in was replaced by a newer one: continue that one".into(),
            ));
        }
        match answer {
            SignInAnswer::Waiting => Ok(format!(
                "Waiting for GitHub: enter {} at {}, then press Continue",
                code.user_code, code.verification_uri
            )),
            SignInAnswer::Denied => {
                self.sign_in = None;
                self.sign_in_status = None;
                self.touch();
                Err(ProjectError::SignIn(
                    "the sign-in was declined on GitHub".into(),
                ))
            }
            SignInAnswer::Expired => {
                self.sign_in = None;
                self.sign_in_status = None;
                self.touch();
                Err(ProjectError::SignIn(
                    "the code expired before it was entered: sign in again".into(),
                ))
            }
            SignInAnswer::Approved { token, login } => {
                let host = host_of(&gh.web).unwrap_or_else(|| "github.com".into());
                if let Some(g) = &self.git {
                    g.credentials.set(&host, Credential::token(&token));
                }
                self.sign_in = None;
                let account = format!("{login} on {host}");
                self.accounts
                    .retain(|a| !a.ends_with(&format!(" on {host}")));
                self.accounts.push(account.clone());
                self.sign_in_status = Some(SignInStatus {
                    provider: "GitHub".into(),
                    user_code: None,
                    verification_uri: None,
                    account: Some(login.clone()),
                });
                self.touch();
                Ok(format!("Signed in to GitHub as {login}"))
            }
        }
    }

    /// Create a private GitHub repository named `name` for the signed-in account ("repo
    /// created for you", Ch.33.5); its clone URL, which the caller links as the remote.
    pub fn create_github_repo(&mut self, name: &str) -> Result<String, ProjectError> {
        let (gh, token) = self.github_token()?;
        Ok(gh.create_repo(&token.secret, name)?.clone_url)
    }

    /// The GitHub to create a repository on and the signed-in account's credential (the
    /// core then calls GitHub off its lock).
    pub fn github_token(&self) -> Result<(GitHub, Credential), ProjectError> {
        let gh = self
            .github
            .clone()
            .ok_or_else(|| self.sign_in_unavailable())?;
        let token = self
            .git
            .as_ref()
            .and_then(|g| g.credentials.for_url(&gh.web))
            .ok_or_else(|| ProjectError::SignIn("sign in to GitHub first".into()))?;
        Ok((gh, token))
    }

    /// Build `project` for `target` through the packager.
    pub fn build(
        &mut self,
        target: Target,
        project: &Project,
    ) -> Result<BuildReport, ProjectError> {
        let open = self.open.as_ref().ok_or(ProjectError::NotOpen)?;
        let product = Product {
            name: text_of(project, NAME_SETTING).unwrap_or_else(|| open.name.clone()),
            version: text_of(project, VERSION_SETTING).unwrap_or_else(|| "0.1.0".into()),
            gpu_mode: crate::gpu_mode_id(text_of(project, crate::GPU_MODE_SETTING).as_deref())
                .into(),
        };
        let files: Vec<(String, u64)> = self
            .written_doc(project)
            .encode()?
            .into_iter()
            .map(|(p, t)| (p.to_string(), t.len() as u64))
            .collect();
        let report = self.packager.package(target, &product, &files)?;
        self.last_build = Some(report.clone());
        self.touch();
        Ok(report)
    }
}

/// A clone made without the host ([`clone_into`]): its store and document, until
/// [`ProjectHost::adopt_clone`] makes it the open project.
pub struct Cloned {
    open: Open,
    doc: ProjectDoc,
}

impl std::fmt::Debug for Cloned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cloned")
            .field("location", &self.open.location)
            .finish_non_exhaustive()
    }
}

impl Cloned {
    /// The cloned project's document.
    #[must_use]
    pub fn doc(&self) -> &ProjectDoc {
        &self.doc
    }
}

/// Clone the project at `remote` into a new project at `location` (Ch.33 §33.5 "any
/// remote, paste a URL"): every revision is replayed into the new store with its id
/// intact. Touches no open project, so the editor core runs it — the fetch and the copy —
/// entirely off its lock.
pub fn clone_into(
    opener: &StoreOpener,
    remote: &str,
    location: &str,
    identity: &str,
) -> Result<Cloned, ProjectError> {
    check_remote(remote)?;
    let from = opener(remote.trim(), identity)?;
    if from.head()?.is_none() {
        return Err(ProjectError::NotAProject(format!(
            "{remote} (the remote has no saved revision)"
        )));
    }
    let mut store = opener(location, identity)?;
    if read_text(store.as_ref(), MANIFEST_PATH)?.is_some() {
        return Err(ProjectError::Exists(location.to_string()));
    }
    let rep = sync::pull(from.as_ref(), store.as_mut())?;
    let (name, doc) = read_doc(store.as_ref(), location)?;
    Ok(Cloned {
        open: Open {
            location: location.to_string(),
            name,
            revisions: revisions(store.as_ref())?,
            presets: read_presets(store.as_ref())?,
            store,
            offer_remote: false,
            remote_head: rep.head,
        },
        doc,
    })
}

/// GitHub's answer to one poll of a device code.
#[derive(Clone, PartialEq, Eq)]
pub enum SignInAnswer {
    /// Not answered yet (or polling too fast).
    Waiting,
    Denied,
    Expired,
    /// Approved: the token (never shown or logged) and the account's login.
    Approved {
        token: String,
        login: String,
    },
}

impl std::fmt::Debug for SignInAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Waiting => f.write_str("Waiting"),
            Self::Denied => f.write_str("Denied"),
            Self::Expired => f.write_str("Expired"),
            Self::Approved { login, .. } => write!(f, "Approved({login}, <redacted>)"),
        }
    }
}

/// Ask `gh` once about `code` (the network; no host needed).
pub fn sign_in_poll(gh: &GitHub, code: &DeviceCode) -> Result<SignInAnswer, ProjectError> {
    Ok(match gh.poll(code)? {
        Poll::Pending | Poll::SlowDown(_) => SignInAnswer::Waiting,
        Poll::Denied => SignInAnswer::Denied,
        Poll::Expired => SignInAnswer::Expired,
        Poll::Token(token) => {
            let login = gh.user(&token)?;
            SignInAnswer::Approved { token, login }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_cmd::{Bus, CommandSink, EditorCommand, Issuer};

    fn host() -> ProjectHost {
        ProjectHost::first_party().unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn git_remotes_are_accepted_and_what_is_not_built_is_refused_with_what_works() {
        for ok in [
            "file:D:/nas/orbits",
            "memory:team",
            "https://github.com/ada/orbits.git",
            "http://git.local:3000/ada/orbits",
            "git-file:D:/nas/orbits.git",
        ] {
            assert!(check_remote(ok).is_ok(), "{ok}");
        }
        for bad in [
            "git@github.com:ada/orbits.git",
            "ssh://git@github.com/ada/orbits.git",
            "https://github.com/",
            "git:C:/p/orbits",
            "s3://bucket/orbits",
            "postgres://db/orbits",
            "orbits",
            "file:",
        ] {
            let e = check_remote(bad).err().unwrap_or(ProjectError::NotOpen);
            assert_eq!(e.code(), "PROJECT-0006", "{bad}: {e}");
        }
        let e = check_remote("git@github.com:ada/orbits.git")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            e.contains("https://github.com/ada/orbits.git"),
            "an SSH URL is refused with its HTTPS form: {e}"
        );
        let e = check_remote("s3://b/x").err().map(|e| e.to_string());
        assert!(e.is_some_and(|m| m.contains("UNBUILT")));
        // What a user types becomes a URL.
        assert_eq!(remote_url("https://x/y.git"), "https://x/y.git");
        assert_eq!(
            remote_url("D:/nas/orbits.git"),
            "git-file:D:/nas/orbits.git"
        );
        assert_eq!(remote_url("D:/nas/orbits"), "file:D:/nas/orbits");
        assert_eq!(clone_name("https://github.com/ada/orbits.git"), "orbits");
        assert_eq!(clone_name("file:D:\\nas\\my-game"), "my-game");
    }

    #[test]
    fn create_save_push_pull_through_the_trait() {
        let mut bus = Bus::new();
        let e = bus.envelope(
            Issuer::Test,
            EditorCommand::SetSetting {
                key: NAME_SETTING.into(),
                value: Some(Value::Text("Orbits".into())),
            },
        );
        bus.apply(e.clone()).unwrap_or_else(|r| panic!("{r:?}"));
        let doc = ProjectDoc::from_project(bus.project());
        let mut h = host();
        let g0 = h.generation();
        h.create("memory:host-test-a", "test", &doc)
            .unwrap_or_else(|e| panic!("{e}"));
        h.loaded();
        assert!(h.generation() > g0);
        assert!(matches!(
            h.create("memory:host-test-a", "test", &doc),
            Err(ProjectError::Exists(_))
        ));
        // Saving before anything changed still makes the first revision.
        let s = h
            .save(bus.project(), "", false, &|_| Keep::Include)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(s.first && s.rev.is_some());
        assert!(h.status().open.as_ref().is_some_and(|o| o.offer_remote));
        // Nothing changed: nothing committed.
        let s = h
            .save(bus.project(), "again", false, &|_| Keep::Include)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(s.rev.is_none());
        h.record(e);
        h.mark_dirty();
        let s = h
            .save(bus.project(), "", true, &|_| Keep::Include)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!s.first);
        let st = h.status();
        let open = st.open.as_ref().unwrap_or_else(|| panic!("open"));
        assert_eq!(open.revisions.len(), 2);
        assert_eq!(
            open.revisions[0].message,
            "test: changed 1 setting (project)"
        );
        assert!(!open.offer_remote);
        h.push("memory:host-test-remote", "test")
            .unwrap_or_else(|e| panic!("{e}"));
        // Another host clones it by creating nothing and pulling.
        let mut other = host();
        assert!(matches!(
            other.pull("memory:host-test-remote", "bob"),
            Err(ProjectError::NotOpen)
        ));
        other
            .create("memory:host-test-b", "bob", &ProjectDoc::default())
            .unwrap_or_else(|e| panic!("{e}"));
        let (rep, doc) = other
            .pull("memory:host-test-remote", "bob")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(rep.revisions.len(), 2);
        let doc = doc.unwrap_or_else(|| panic!("a document to load"));
        assert_eq!(
            doc.settings.get(NAME_SETTING),
            Some(&Value::Text("Orbits".into()))
        );
        other.mark_dirty();
        assert!(matches!(
            other.pull("memory:host-test-remote", "bob"),
            Err(ProjectError::Unsaved)
        ));
        assert!(matches!(
            h.push("git@github.com:x/y.git", "t"),
            Err(ProjectError::UnsupportedRemote { .. })
        ));
    }

    #[test]
    fn open_refuses_a_folder_without_a_project() {
        let mut h = host();
        assert!(matches!(
            h.open_project("memory:host-test-empty", "t"),
            Err(ProjectError::NotAProject(_))
        ));
        assert!(matches!(
            h.save(&Project::new(), "", false, &|_| Keep::Include),
            Err(ProjectError::NotOpen)
        ));
    }

    #[test]
    fn file_settings_are_written_as_the_files_hold_them_until_the_project_changes_them() {
        let mut bus = Bus::new();
        let set = |bus: &mut Bus, k: &str, v: Option<Value>| {
            let e = bus.envelope(
                Issuer::Test,
                EditorCommand::SetSetting {
                    key: k.into(),
                    value: v,
                },
            );
            bus.apply(e).unwrap_or_else(|r| panic!("{r:?}"));
        };
        set(&mut bus, "a", Some(Value::Bool(false)));
        set(&mut bus, "b", Some(Value::Bool(false)));
        let mut h = host();
        h.set_file_settings(vec![
            FileSetting {
                key: "a".into(),
                file: Some(Value::Bool(true)),
                loaded: Some(Value::Bool(false)),
            },
            FileSetting {
                key: "b".into(),
                file: None,
                loaded: Some(Value::Bool(false)),
            },
        ]);
        let doc = h.written_doc(bus.project());
        assert_eq!(doc.settings.get("a"), Some(&Value::Bool(true)));
        assert_eq!(doc.settings.get("b"), None);
        // A change since the load is written as the project holds it.
        set(&mut bus, "a", Some(Value::Text("mine".into())));
        let doc = h.written_doc(bus.project());
        assert_eq!(doc.settings.get("a"), Some(&Value::Text("mine".into())));
        h.set_file_settings(Vec::new());
        assert_eq!(
            h.written_doc(bus.project()).settings.get("b"),
            Some(&Value::Bool(false))
        );
    }
}

//! The Git store backend (Ch.33.1 "`Git` — any remote", M2-15, ADR 0035).
//!
//! [`GitStore`] is a [`ProjectStore`] whose history is a Git repository, laid out as
//! [`layout`] describes: one commit per revision, text files diffable in the tree, binary
//! files as engine-native blobs on a second ref, generated ignore rules. Its revision ids
//! are exactly `LocalFs`'s and `MemoryStore`'s — it builds revisions with forge-store's
//! public kit — so `test_store_backend_parity` holds it to the same content hashes.
//!
//! Two ways to open one:
//!
//! * **A project folder** (`git:<folder>`): the working files are the folder (through
//!   `LocalFs`, so writes are atomic and locks are the folder's), the repository is
//!   `<folder>/.forge/git` (bare: the folder never looks like a half-checked-out work tree to
//!   other tools).
//! * **A remote** (`https://…`, `http://…`, `git-file:<path>`): a view of a Git remote
//!   through a local mirror in the cache directory. Opening fetches; the working files are
//!   the remote head's (read lazily); commits go to the mirror; [`ProjectStore::publish`]
//!   pushes them, moving the remote's refs only from the values it read. This is how push,
//!   pull and clone reach GitHub, GitLab, Gitea/Forgejo or a bare repository on a share:
//!   `forge_project::sync` replays revisions through the trait, with nothing Git-specific.

mod credentials;
pub mod github;
pub mod http;
mod index;
mod layout;
mod repo;
mod smart;
mod transport;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use forge_cmd::{Clock, CommandEnvelope, SystemClock};
use forge_store::kit::{CaseIndex, RevRecord, build_commit, decode_log};
use forge_store::{
    Blake3, Bytes, LocalFs, Lock, ProjectStore, Rev, RevId, RevRange, Stamp, StoreError, StorePath,
    Tree,
};
use gix::ObjectId;

pub use credentials::{Credential, Credentials, host_of};
pub use http::{HttpClient, HttpRequest, HttpResponse, UreqClient};

use index::{BlobIndex, Entry};
use layout::{BLOBS_REF, MAIN_REF, RevisionTree};
use repo::{RefSet, Repo};
use transport::{LocalTransport, RefUpdate, Transport};

/// What remote views need: where mirrors are cached, who signs in to what, and the HTTP
/// client.
#[derive(Clone)]
pub struct GitOptions {
    /// Remote mirrors live under here (one per remote URL). A cache: deleting it costs a
    /// fetch.
    pub cache_dir: PathBuf,
    /// Tokens by host (GitHub's device flow puts one here).
    pub credentials: Credentials,
    /// The HTTP client (tests hand in one pointed at a mock server's plain HTTP).
    pub http: Arc<dyn HttpClient>,
}

impl std::fmt::Debug for GitOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitOptions")
            .field("cache_dir", &self.cache_dir)
            .finish_non_exhaustive()
    }
}

impl GitOptions {
    /// Options with the per-user cache directory, no credentials and the default client.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache_dir: default_cache_dir(),
            credentials: Credentials::default(),
            http: Arc::new(UreqClient::new()),
        }
    }
}

impl Default for GitOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// The per-user cache directory for remote mirrors: `%LOCALAPPDATA%\Forge\cache\git` on
/// Windows, `$XDG_CACHE_HOME/forge/git` (or `~/.cache/forge/git`) elsewhere, the system
/// temporary directory when none of those is set.
#[must_use]
pub fn default_cache_dir() -> PathBuf {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows)
        && let Some(d) = env("LOCALAPPDATA")
    {
        return d.join("Forge").join("cache").join("git");
    }
    if let Some(d) = env("XDG_CACHE_HOME") {
        return d.join("forge").join("git");
    }
    if let Some(h) = env("HOME") {
        return h.join(".cache").join("forge").join("git");
    }
    std::env::temp_dir().join("forge-cache").join("git")
}

/// The URL with any `user:password@` removed, for messages and cache names.
#[must_use]
pub fn redact(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (auth, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
            let host = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
            if path.is_empty() && !rest.contains('/') {
                format!("{scheme}://{host}")
            } else {
                format!("{scheme}://{host}/{path}")
            }
        }
        None => url.to_string(),
    }
}

/// The working files of a remote view: the head's, read lazily; written ones held.
#[derive(Default)]
struct MemWork {
    files: BTreeMap<StorePath, (Blake3, Option<Bytes>)>,
    case: CaseIndex,
    locks: BTreeMap<StorePath, String>,
}

enum Working {
    Folder(LocalFs),
    Memory(MemWork),
}

struct Parsed {
    id: RevId,
    rec: RevRecord,
    parent: Option<ObjectId>,
}

#[derive(Default)]
struct RevCache {
    by_oid: HashMap<ObjectId, Arc<Parsed>>,
    by_id: HashMap<RevId, ObjectId>,
}

struct RemoteView {
    transport: Box<dyn Transport>,
    /// The remote's refs as last read or pushed.
    main: Option<ObjectId>,
    blobs: Option<ObjectId>,
}

/// See the module docs.
pub struct GitStore {
    identity: String,
    repo: Repo,
    index: BlobIndex,
    work: Working,
    revs: Mutex<RevCache>,
    /// Folder mode: path -> (stamp, address) at the last commit, so a commit reads only the
    /// files that changed.
    hashed: HashMap<StorePath, (Stamp, Blake3)>,
    clock: Box<dyn Clock>,
    remote: Option<RemoteView>,
}

impl std::fmt::Debug for GitStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitStore")
            .field("repo", &self.repo.git_dir())
            .field("remote", &self.remote.as_ref().map(|r| r.transport.name()))
            .finish_non_exhaustive()
    }
}

fn corrupt(what: impl Into<String>, why: impl Into<String>) -> StoreError {
    StoreError::Corrupt {
        what: what.into(),
        why: why.into(),
    }
}

impl GitStore {
    /// Open (creating if needed) the project folder `root` with its history in
    /// `root/.forge/git`, acting as `identity`.
    pub fn open_folder(root: impl Into<PathBuf>, identity: &str) -> Result<Self, StoreError> {
        Self::open_folder_with_clock(root, identity, Box::new(SystemClock))
    }

    /// [`GitStore::open_folder`] with an injected clock (commit times).
    pub fn open_folder_with_clock(
        root: impl Into<PathBuf>,
        identity: &str,
        clock: Box<dyn Clock>,
    ) -> Result<Self, StoreError> {
        let root = root.into();
        let work = LocalFs::open(&root, identity)?;
        let repo = Repo::open_or_init_bare(&root.join(".forge").join("git"))?;
        let mut s = Self::with(identity, repo, Working::Folder(work), clock, None)?;
        s.index_history()?;
        Ok(s)
    }

    /// Open a view of the Git remote at `url` (`https://…`, `http://…`, or
    /// `git-file:<path>`), fetching it into a mirror under `opts.cache_dir`.
    pub fn open_remote(url: &str, identity: &str, opts: &GitOptions) -> Result<Self, StoreError> {
        Self::open_remote_with_clock(url, identity, opts, Box::new(SystemClock))
    }

    /// [`GitStore::open_remote`] with an injected clock.
    pub fn open_remote_with_clock(
        url: &str,
        identity: &str,
        opts: &GitOptions,
        clock: Box<dyn Clock>,
    ) -> Result<Self, StoreError> {
        let url = url.trim();
        let transport: Box<dyn Transport> = if let Some(path) = url.strip_prefix("git-file:") {
            let path = path.strip_prefix("//").unwrap_or(path);
            Box::new(LocalTransport::new(Path::new(path)))
        } else if url.starts_with("https://") || url.starts_with("http://") {
            Box::new(smart::SmartHttp::new(
                url,
                opts.credentials.clone(),
                Arc::clone(&opts.http),
            ))
        } else {
            return Err(StoreError::UnknownBackend(url.to_string()));
        };
        let key = Blake3::of(redact(url).as_bytes()).to_hex();
        let mirror = opts.cache_dir.join(&key[..24]);
        let repo = Repo::open_or_init_bare(&mirror)?;
        let mut s = Self::with(
            identity,
            repo,
            Working::Memory(MemWork::default()),
            clock,
            Some(RemoteView {
                transport,
                main: None,
                blobs: None,
            }),
        )?;
        s.fetch()?;
        Ok(s)
    }

    fn with(
        identity: &str,
        repo: Repo,
        work: Working,
        clock: Box<dyn Clock>,
        remote: Option<RemoteView>,
    ) -> Result<Self, StoreError> {
        let index = BlobIndex::load(&repo)?;
        Ok(Self {
            identity: identity.to_string(),
            repo,
            index,
            work,
            revs: Mutex::new(RevCache::default()),
            hashed: HashMap::new(),
            clock,
            remote,
        })
    }

    /// The repository's git directory (a project folder's `.forge/git`, or a remote's
    /// mirror).
    #[must_use]
    pub fn git_dir(&self) -> &Path {
        self.repo.git_dir()
    }

    /// Blobs the index knows (diagnostic).
    #[must_use]
    pub fn indexed_blobs(&self) -> usize {
        self.index.len()
    }

    /// Index every commit of both refs not indexed yet (after a fetch, or a lost index).
    fn index_history(&mut self) -> Result<(), StoreError> {
        let mut cur = self.repo.ref_get(MAIN_REF)?;
        let mut todo = Vec::new();
        while let Some(c) = cur {
            if self.index.seen(&c) {
                break;
            }
            todo.push(c);
            cur = self.repo.commit(&c)?.parents.first().copied();
        }
        for c in todo.iter().rev() {
            self.index.index_revision(&self.repo, c)?;
        }
        let mut cur = self.repo.ref_get(BLOBS_REF)?;
        while let Some(c) = cur {
            if self.index.seen(&c) {
                break;
            }
            self.index.index_blobs_commit(&self.repo, &c)?;
            cur = self.repo.commit(&c)?.parents.first().copied();
        }
        self.index.flush()
    }

    /// A remote view: read the remote's refs, fetch what is new, and make the mirror's
    /// refs and working files the remote's.
    fn fetch(&mut self) -> Result<(), StoreError> {
        let Some(view) = self.remote.as_mut() else {
            return Ok(());
        };
        let refs = view.transport.refs()?;
        let main = refs.get(MAIN_REF).copied();
        let blobs = refs.get(BLOBS_REF).copied();
        let wants: Vec<ObjectId> = [main, blobs]
            .into_iter()
            .flatten()
            .filter(|w| !self.repo.has(w))
            .collect();
        let haves: Vec<ObjectId> = [self.repo.ref_get(MAIN_REF)?, self.repo.ref_get(BLOBS_REF)?]
            .into_iter()
            .flatten()
            .collect();
        if !wants.is_empty() {
            view.transport.fetch(&self.repo, &wants, &haves)?;
        }
        for (name, tip) in [(MAIN_REF, main), (BLOBS_REF, blobs)] {
            match tip {
                Some(t) => {
                    self.repo.ref_set(name, t, None, true)?;
                }
                None => self.repo.ref_delete(name)?,
            }
        }
        view.main = main;
        view.blobs = blobs;
        self.index_history()?;
        let mut work = MemWork::default();
        if let Some(head) = self.head()? {
            for (p, h) in self.tree(&head)?.entries {
                work.case.insert(&p);
                work.files.insert(p, (h, None));
            }
        }
        self.work = Working::Memory(work);
        Ok(())
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, RevCache> {
        // Entries are complete when inserted: a poisoned cache is still consistent.
        self.revs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The revision a commit carries (cached).
    fn parsed(&self, oid: &ObjectId) -> Result<Arc<Parsed>, StoreError> {
        if let Some(p) = self.cache().by_oid.get(oid) {
            return Ok(Arc::clone(p));
        }
        let rec = layout::record_of(&self.repo, oid)?;
        let parent = self.repo.commit(oid)?.parents.first().copied();
        let p = Arc::new(Parsed {
            id: rec.id(),
            rec,
            parent,
        });
        let mut c = self.cache();
        c.by_id.insert(p.id, *oid);
        c.by_oid.insert(*oid, Arc::clone(&p));
        Ok(p)
    }

    /// The commit of a revision id.
    fn find(&self, id: &RevId) -> Result<(ObjectId, Arc<Parsed>), StoreError> {
        let known = self.cache().by_id.get(id).copied();
        if let Some(oid) = known {
            return Ok((oid, self.parsed(&oid)?));
        }
        let mut cur = self.repo.ref_get(MAIN_REF)?;
        while let Some(oid) = cur {
            let p = self.parsed(&oid)?;
            if p.id == *id {
                return Ok((oid, p));
            }
            cur = p.parent;
        }
        Err(StoreError::NotFound(format!("revision {id}")))
    }

    fn head_parsed(&self) -> Result<Option<(ObjectId, Arc<Parsed>)>, StoreError> {
        match self.repo.ref_get(MAIN_REF)? {
            Some(oid) => Ok(Some((oid, self.parsed(&oid)?))),
            None => Ok(None),
        }
    }

    /// Store `b` as a Git blob and index it; its address.
    fn put(&mut self, b: &[u8], shared: bool) -> Result<Blake3, StoreError> {
        let h = Blake3::of(b);
        if self.index.get(&h).is_some() {
            if shared {
                self.index.share(&h);
            }
            return Ok(h);
        }
        let oid = self.repo.write_blob(b)?;
        self.index.add(
            h,
            Entry {
                oid,
                text: Some(layout::is_text(b)),
                shared,
            },
        );
        Ok(h)
    }

    /// The working files as `(path, address)`, every one stored as a blob.
    fn snapshot(&mut self) -> Result<Vec<(StorePath, Blake3)>, StoreError> {
        let mut out = Vec::new();
        match &self.work {
            Working::Folder(fs) => {
                let stamps = fs.stamps()?;
                let mut changed = Vec::new();
                for (p, st) in stamps {
                    match self.hashed.get(&p) {
                        Some((s, h)) if *s == st && self.index.get(h).is_some() => {
                            out.push((p, *h));
                        }
                        _ => changed.push((p, st)),
                    }
                }
                for (p, st) in changed {
                    let bytes = match &self.work {
                        Working::Folder(fs) => fs.read(&p)?,
                        Working::Memory(_) => {
                            return Err(corrupt("the working files", "changed kind"));
                        }
                    };
                    let h = self.put(&bytes, false)?;
                    self.hashed.insert(p.clone(), (st, h));
                    out.push((p, h));
                }
                out.sort_by(|a, b| a.0.cmp(&b.0));
            }
            Working::Memory(m) => {
                let files: Vec<(StorePath, Blake3, Option<Bytes>)> = m
                    .files
                    .iter()
                    .map(|(p, (h, b))| (p.clone(), *h, b.clone()))
                    .collect();
                for (p, h, b) in files {
                    if self.index.get(&h).is_none() {
                        let b = b.ok_or_else(|| {
                            corrupt(format!("working file {p}"), "its bytes are not stored")
                        })?;
                        self.put(&b, false)?;
                    }
                    out.push((p, h));
                }
            }
        }
        Ok(out)
    }

    /// Whether a stored blob is text (decided once, then remembered).
    fn text_of(&mut self, h: &Blake3) -> Result<(ObjectId, bool, bool), StoreError> {
        let e = self
            .index
            .get(h)
            .ok_or_else(|| StoreError::NotFound(format!("blob {h}")))?;
        let text = match e.text {
            Some(t) => t,
            None => {
                let t = layout::is_text(&self.blob_get(*h)?);
                self.index.add(*h, Entry { text: Some(t), ..e });
                t
            }
        };
        Ok((e.oid, text, e.shared))
    }

    fn check_lock_mem(m: &MemWork, identity: &str, path: &StorePath) -> Result<(), StoreError> {
        match m.locks.get(path) {
            Some(owner) if owner != identity => Err(StoreError::Locked {
                path: path.to_string(),
                owner: owner.clone(),
            }),
            _ => Ok(()),
        }
    }
}

impl ProjectStore for GitStore {
    fn backend(&self) -> &'static str {
        "git"
    }

    fn identity(&self) -> &str {
        &self.identity
    }

    fn read(&self, path: &StorePath) -> Result<Bytes, StoreError> {
        match &self.work {
            Working::Folder(fs) => fs.read(path),
            Working::Memory(m) => match m.files.get(path) {
                Some((_, Some(b))) => Ok(b.clone()),
                Some((h, None)) => self.blob_get(*h),
                None => Err(StoreError::NotFound(path.to_string())),
            },
        }
    }

    fn write(&mut self, path: &StorePath, b: Bytes) -> Result<(), StoreError> {
        match &mut self.work {
            Working::Folder(fs) => fs.write(path, b),
            Working::Memory(m) => {
                Self::check_lock_mem(m, &self.identity, path)?;
                if !m.files.contains_key(path) {
                    if let Some(existing) = m.case.collision(path) {
                        return Err(StoreError::CaseCollision {
                            path: path.to_string(),
                            existing,
                        });
                    }
                    m.case.insert(path);
                }
                m.files.insert(path.clone(), (Blake3::of(&b), Some(b)));
                Ok(())
            }
        }
    }

    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError> {
        match &mut self.work {
            Working::Folder(fs) => fs.delete(path),
            Working::Memory(m) => {
                Self::check_lock_mem(m, &self.identity, path)?;
                if m.files.remove(path).is_none() {
                    return Err(StoreError::NotFound(path.to_string()));
                }
                m.case.remove(path);
                Ok(())
            }
        }
    }

    fn list(&self) -> Result<Vec<StorePath>, StoreError> {
        match &self.work {
            Working::Folder(fs) => fs.list(),
            Working::Memory(m) => Ok(m.files.keys().cloned().collect()),
        }
    }

    fn blob_get(&self, h: Blake3) -> Result<Bytes, StoreError> {
        let e = self
            .index
            .get(&h)
            .ok_or_else(|| StoreError::NotFound(format!("blob {h}")))?;
        let (_, data) = self.repo.read(&e.oid)?;
        if Blake3::of(&data) != h {
            return Err(corrupt(
                format!("blob {h}"),
                format!("git object {} holds different bytes", e.oid),
            ));
        }
        Ok(Bytes::from(data))
    }

    fn blob_put(&mut self, b: Bytes) -> Result<Blake3, StoreError> {
        let h = self.put(&b, false)?;
        self.index.flush()?;
        Ok(h)
    }

    fn blob_has(&self, h: Blake3) -> Result<bool, StoreError> {
        Ok(self.index.get(&h).is_some())
    }

    fn commit(&mut self, msg: &str, envelopes: &[CommandEnvelope]) -> Result<RevId, StoreError> {
        let at = self.clock.now_ms();
        self.commit_at(msg, envelopes, at)
    }

    fn commit_at(
        &mut self,
        msg: &str,
        envelopes: &[CommandEnvelope],
        at: u64,
    ) -> Result<RevId, StoreError> {
        let files = self.snapshot()?;
        let head = self.head_parsed()?;
        let rec = build_commit(
            files.clone(),
            head.as_ref().map(|(_, p)| p.id),
            msg,
            envelopes,
            at,
            &mut |b| self.put(&b, true),
        )?;
        let id = rec.id();
        let rec_text = rec.encode()?;
        let rec_h = self.put(rec_text.as_bytes(), true)?;
        let meta_oid = |s: &Self, h: &str| -> Result<ObjectId, StoreError> {
            let h: Blake3 = h.parse()?;
            s.index
                .get(&h)
                .map(|e| e.oid)
                .ok_or_else(|| StoreError::NotFound(format!("blob {h}")))
        };
        let record = meta_oid(self, &rec_h.to_hex())?;
        let forge_tree = meta_oid(self, &rec.tree)?;
        let log = meta_oid(self, &rec.log)?;
        // Classify: text files at paths Git accepts go in the tree; the rest are blobs.
        let mut text = Vec::new();
        let mut binary = Vec::new();
        let mut chain: BTreeMap<Blake3, ObjectId> = BTreeMap::new();
        let mut in_tree: std::collections::HashSet<Blake3> = std::collections::HashSet::new();
        let mut classes = Vec::with_capacity(files.len());
        for (p, h) in &files {
            let (oid, is_text, shared) = self.text_of(h)?;
            let tree = is_text && layout::git_safe(p);
            if tree {
                in_tree.insert(*h);
            }
            classes.push((tree, oid, shared));
        }
        for ((p, h), (tree, oid, shared)) in files.iter().zip(&classes) {
            if *tree {
                text.push((p, *oid));
            } else {
                binary.push(p);
                if !shared && !in_tree.contains(h) {
                    chain.insert(*h, *oid);
                }
            }
        }
        let own_ignore = files.iter().any(|(p, _)| p.as_str() == layout::IGNORE);
        let tree = layout::write_tree(
            &self.repo,
            &RevisionTree {
                text,
                binary,
                own_ignore,
                record,
                forge_tree,
                log,
            },
        )?;
        let sig = layout::signature(&rec);
        let commit = gix::objs::Commit {
            tree,
            parents: head.as_ref().map(|(o, _)| *o).into_iter().collect(),
            author: sig.clone(),
            committer: sig.clone(),
            encoding: None,
            message: layout::message(&rec).into(),
            extra_headers: Vec::new(),
        };
        let coid = self.repo.write_commit(&commit)?;
        let moved = |what: &str| StoreError::Io {
            op: "commit to",
            path: self.repo.git_dir().display().to_string(),
            why: format!(
                "{what} moved while committing (another editor committed to this project); nothing was committed"
            ),
        };
        if !chain.is_empty() {
            let old = self.repo.ref_get(BLOBS_REF)?;
            let t = self
                .repo
                .write_tree(chain.iter().map(|(h, oid)| (h.to_hex(), false, *oid)))?;
            let side = gix::objs::Commit {
                tree: t,
                parents: old.into_iter().collect(),
                author: sig.clone(),
                committer: sig,
                encoding: None,
                message: format!("Forge blobs of revision {id}\n").into(),
                extra_headers: Vec::new(),
            };
            let soid = self.repo.write_commit(&side)?;
            if self.repo.ref_set(BLOBS_REF, soid, old, false)? == RefSet::Moved {
                return Err(moved("the blobs ref"));
            }
            self.index.made(soid);
        }
        if self
            .repo
            .ref_set(MAIN_REF, coid, head.as_ref().map(|(o, _)| *o), false)?
            == RefSet::Moved
        {
            return Err(moved("the history"));
        }
        for (_, h) in &files {
            self.index.share(h);
        }
        self.index.made(coid);
        self.index.flush()?;
        let parsed = Arc::new(Parsed {
            id,
            rec,
            parent: head.map(|(o, _)| o),
        });
        let mut c = self.cache();
        c.by_id.insert(id, coid);
        c.by_oid.insert(coid, parsed);
        drop(c);
        if let Working::Memory(m) = &mut self.work {
            // Committed bytes are in the object database now: hold addresses only.
            for (_, b) in m.files.values_mut() {
                *b = None;
            }
        }
        Ok(id)
    }

    fn publish(&mut self) -> Result<(), StoreError> {
        let main = self.repo.ref_get(MAIN_REF)?;
        let blobs = self.repo.ref_get(BLOBS_REF)?;
        let Some(view) = self.remote.as_mut() else {
            return Ok(());
        };
        if main == view.main && blobs == view.blobs {
            return Ok(());
        }
        let mut updates = Vec::new();
        let mut tips = Vec::new();
        // The blobs first: a push that stops between the two leaves only extra blobs.
        if let Some(b) = blobs
            && blobs != view.blobs
        {
            updates.push(RefUpdate {
                name: BLOBS_REF,
                old: view.blobs,
                new: b,
            });
            tips.push(b);
        }
        if let Some(m) = main
            && main != view.main
        {
            updates.push(RefUpdate {
                name: MAIN_REF,
                old: view.main,
                new: m,
            });
            tips.push(m);
        }
        let known: Vec<ObjectId> = [view.main, view.blobs].into_iter().flatten().collect();
        let objects = transport::missing_objects(&self.repo, &tips, &known)?;
        view.transport.push(&self.repo, &updates, &objects)?;
        view.main = main;
        view.blobs = blobs;
        Ok(())
    }

    fn head(&self) -> Result<Option<RevId>, StoreError> {
        Ok(self.head_parsed()?.map(|(_, p)| p.id))
    }

    fn history(&self, range: RevRange) -> Result<Vec<Rev>, StoreError> {
        let mut out = Vec::new();
        let limit = range.limit.unwrap_or(usize::MAX);
        let mut cur = match range.to {
            Some(id) => Some(self.find(&id)?.0),
            None => self.repo.ref_get(MAIN_REF)?,
        };
        let mut expect: Option<Option<String>> = None;
        while let Some(oid) = cur {
            let p = self.parsed(&oid)?;
            if let Some(want) = &expect
                && want.as_deref() != Some(p.id.to_string().as_str())
            {
                return Err(corrupt(
                    format!("git commit {oid}"),
                    "its revision is not the parent its child names",
                ));
            }
            if Some(p.id) == range.stop_at || out.len() >= limit {
                break;
            }
            out.push(p.rec.to_rev(p.id)?);
            expect = Some(p.rec.parent.clone());
            cur = p.parent;
        }
        if cur.is_none()
            && let Some(Some(parent)) = expect
            && out.len() < limit
        {
            return Err(corrupt(
                "the history",
                format!("revision {parent} is named as a parent but has no commit"),
            ));
        }
        Ok(out)
    }

    fn tree(&self, rev: &RevId) -> Result<Tree, StoreError> {
        let (_, p) = self.find(rev)?;
        let h: Blake3 = p.rec.tree.parse()?;
        Tree::decode(&self.blob_get(h)?)
    }

    fn commands(&self, rev: &RevId) -> Result<Vec<CommandEnvelope>, StoreError> {
        let (_, p) = self.find(rev)?;
        let h: Blake3 = p.rec.log.parse()?;
        decode_log(&self.blob_get(h)?)
    }

    fn lock(&mut self, path: &StorePath) -> Result<Lock, StoreError> {
        match &mut self.work {
            Working::Folder(fs) => fs.lock(path),
            Working::Memory(m) => {
                Self::check_lock_mem(m, &self.identity, path)?;
                m.locks.insert(path.clone(), self.identity.clone());
                Ok(Lock {
                    path: path.clone(),
                    owner: self.identity.clone(),
                })
            }
        }
    }

    fn unlock(&mut self, lock: &Lock) -> Result<(), StoreError> {
        match &mut self.work {
            Working::Folder(fs) => fs.unlock(lock),
            Working::Memory(m) => match m.locks.get(&lock.path) {
                None => Err(StoreError::NotFound(format!("a lock on {}", lock.path))),
                Some(owner) if *owner != self.identity => Err(StoreError::NotLockOwner {
                    path: lock.path.to_string(),
                    owner: owner.clone(),
                    by: self.identity.clone(),
                }),
                Some(_) => {
                    m.locks.remove(&lock.path);
                    Ok(())
                }
            },
        }
    }

    fn locks(&self) -> Result<Vec<Lock>, StoreError> {
        match &self.work {
            Working::Folder(fs) => fs.locks(),
            Working::Memory(m) => Ok(m
                .locks
                .iter()
                .map(|(p, o)| Lock {
                    path: p.clone(),
                    owner: o.clone(),
                })
                .collect()),
        }
    }

    fn stamps(&self) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        match &self.work {
            Working::Folder(fs) => fs.stamps(),
            Working::Memory(m) => Ok(m
                .files
                .iter()
                .map(|(p, (h, _))| (p.clone(), Stamp::of_content(h)))
                .collect()),
        }
    }

    fn stamps_of(&self, paths: &[StorePath]) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        match &self.work {
            Working::Folder(fs) => fs.stamps_of(paths),
            Working::Memory(m) => {
                let mut out: Vec<(StorePath, Stamp)> = paths
                    .iter()
                    .filter_map(|p| {
                        m.files
                            .get(p)
                            .map(|(h, _)| (p.clone(), Stamp::of_content(h)))
                    })
                    .collect();
                out.sort_by(|a, b| a.0.cmp(&b.0));
                out.dedup_by(|a, b| a.0 == b.0);
                Ok(out)
            }
        }
    }

    fn size(&self, path: &StorePath) -> Result<Option<u64>, StoreError> {
        match &self.work {
            Working::Folder(fs) => fs.size(path),
            Working::Memory(m) => match m.files.get(path) {
                Some((_, Some(b))) => Ok(Some(b.len() as u64)),
                Some((h, None)) => Ok(Some(self.blob_get(*h)?.len() as u64)),
                None => Ok(None),
            },
        }
    }
}

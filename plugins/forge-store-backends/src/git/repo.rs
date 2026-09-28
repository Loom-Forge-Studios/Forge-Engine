//! The Git object database and refs of one repository, through gitoxide (ADR 0035).
//!
//! Always opened **isolated**: no system or user git configuration is read, so what a
//! store writes never depends on the machine's `~/.gitconfig` (a determinism rule, not a
//! convenience). Objects are written only when absent — the id is computed first, so an
//! unchanged file costs a hash, not a compression and a file write.

use std::path::{Path, PathBuf};

use forge_store::StoreError;
use gix::ObjectId;
use gix::objs::Write as _;
use gix::objs::tree::{Entry, EntryKind};
use gix::refs::Target;
use gix::refs::transaction::PreviousValue;

/// Object names are SHA-1 (what every Git host speaks).
pub(crate) const HASH: gix::hash::Kind = gix::hash::Kind::Sha1;

/// A repository.
pub(crate) struct Repo {
    inner: gix::ThreadSafeRepository,
    git_dir: PathBuf,
}

/// What a commit says, decoded.
#[derive(Clone, Debug)]
pub(crate) struct CommitInfo {
    pub tree: ObjectId,
    pub parents: Vec<ObjectId>,
}

/// One tree entry, decoded.
#[derive(Clone, Debug)]
pub(crate) struct TreeItem {
    pub name: String,
    pub is_tree: bool,
    pub id: ObjectId,
}

fn io(op: &'static str, path: &Path, why: impl std::fmt::Display) -> StoreError {
    StoreError::Io {
        op,
        path: path.display().to_string(),
        why: why.to_string(),
    }
}

/// The outcome of a compare-and-swap ref update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefSet {
    Done,
    /// The ref did not hold the expected value: nothing changed.
    Moved,
}

impl Repo {
    /// Open the bare repository at `dir`, creating it if there is none.
    pub(crate) fn open_or_init_bare(dir: &Path) -> Result<Self, StoreError> {
        if dir.join("HEAD").is_file() {
            return Self::open(dir);
        }
        std::fs::create_dir_all(dir).map_err(|e| io("create", dir, e))?;
        let inner = gix::ThreadSafeRepository::init_opts(
            dir,
            gix::create::Kind::Bare,
            gix::create::Options::default(),
            gix::open::Options::isolated(),
        )
        .map_err(|e| io("init a git repository in", dir, e))?;
        Ok(Self {
            inner,
            git_dir: dir.to_path_buf(),
        })
    }

    /// Open an existing repository: a bare one at `dir`, or the `.git` of a work tree.
    pub(crate) fn open(dir: &Path) -> Result<Self, StoreError> {
        let git_dir = if dir.join(".git").is_dir() {
            dir.join(".git")
        } else {
            dir.to_path_buf()
        };
        let inner = gix::ThreadSafeRepository::open_opts(&git_dir, gix::open::Options::isolated())
            .map_err(|e| io("open the git repository", &git_dir, e))?;
        Ok(Self { inner, git_dir })
    }

    /// The repository's git directory.
    pub(crate) fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    fn local(&self) -> gix::Repository {
        self.inner.to_thread_local()
    }

    fn err(&self, op: &'static str, why: impl std::fmt::Display) -> StoreError {
        io(op, &self.git_dir, why)
    }

    /// Whether the object is stored.
    pub(crate) fn has(&self, id: &ObjectId) -> bool {
        self.local().has_object(id)
    }

    /// An object's kind and bytes.
    pub(crate) fn read(&self, id: &ObjectId) -> Result<(gix::objs::Kind, Vec<u8>), StoreError> {
        let r = self.local();
        let o = r.try_find_object(*id).map_err(|e| self.err("read", e))?;
        match o {
            Some(mut o) => Ok((o.kind, std::mem::take(&mut o.data))),
            None => Err(StoreError::NotFound(format!("git object {id}"))),
        }
    }

    /// Store an object (a no-op when it is already stored); its id.
    pub(crate) fn write(&self, kind: gix::objs::Kind, data: &[u8]) -> Result<ObjectId, StoreError> {
        let id = gix::objs::compute_hash(HASH, kind, data).map_err(|e| self.err("hash", e))?;
        let r = self.local();
        if !r.has_object(id) {
            let got = r
                .objects
                .write_buf(kind, data)
                .map_err(|e| self.err("write an object to", e))?;
            if got != id {
                return Err(StoreError::Corrupt {
                    what: format!("git object {id}"),
                    why: format!("the object database stored it as {got}"),
                });
            }
        }
        Ok(id)
    }

    /// Store a blob.
    pub(crate) fn write_blob(&self, data: &[u8]) -> Result<ObjectId, StoreError> {
        self.write(gix::objs::Kind::Blob, data)
    }

    /// Store a tree of `(name, is_tree, id)` (sorted here into Git's order).
    pub(crate) fn write_tree(
        &self,
        items: impl IntoIterator<Item = (String, bool, ObjectId)>,
    ) -> Result<ObjectId, StoreError> {
        let mut entries: Vec<Entry> = items
            .into_iter()
            .map(|(name, is_tree, oid)| Entry {
                mode: if is_tree {
                    EntryKind::Tree.into()
                } else {
                    EntryKind::Blob.into()
                },
                filename: name.into(),
                oid,
            })
            .collect();
        entries.sort();
        let mut buf = Vec::new();
        gix::objs::WriteTo::write_to(&gix::objs::Tree { entries }, &mut buf)
            .map_err(|e| self.err("encode a tree for", e))?;
        self.write(gix::objs::Kind::Tree, &buf)
    }

    /// Store a commit.
    pub(crate) fn write_commit(&self, c: &gix::objs::Commit) -> Result<ObjectId, StoreError> {
        let mut buf = Vec::new();
        gix::objs::WriteTo::write_to(c, &mut buf)
            .map_err(|e| self.err("encode a commit for", e))?;
        self.write(gix::objs::Kind::Commit, &buf)
    }

    /// A tree's entries.
    pub(crate) fn tree(&self, id: &ObjectId) -> Result<Vec<TreeItem>, StoreError> {
        let (kind, data) = self.read(id)?;
        if kind != gix::objs::Kind::Tree {
            return Err(StoreError::Corrupt {
                what: format!("git object {id}"),
                why: format!("a {kind} where a tree was expected"),
            });
        }
        let t = gix::objs::TreeRef::from_bytes(&data, HASH).map_err(|e| StoreError::Corrupt {
            what: format!("git tree {id}"),
            why: e.to_string(),
        })?;
        Ok(t.entries
            .iter()
            .map(|e| TreeItem {
                name: e.filename.to_string(),
                is_tree: e.mode.is_tree(),
                id: e.oid.to_owned(),
            })
            .collect())
    }

    /// A commit, decoded.
    pub(crate) fn commit(&self, id: &ObjectId) -> Result<CommitInfo, StoreError> {
        let (kind, data) = self.read(id)?;
        if kind != gix::objs::Kind::Commit {
            return Err(StoreError::Corrupt {
                what: format!("git object {id}"),
                why: format!("a {kind} where a commit was expected"),
            });
        }
        let c = gix::objs::CommitRef::from_bytes(&data, HASH).map_err(|e| StoreError::Corrupt {
            what: format!("git commit {id}"),
            why: e.to_string(),
        })?;
        Ok(CommitInfo {
            tree: c.tree(),
            parents: c.parents().collect(),
        })
    }

    /// A ref's target (`None`: no such ref).
    pub(crate) fn ref_get(&self, name: &str) -> Result<Option<ObjectId>, StoreError> {
        let r = self.local();
        let found = r
            .try_find_reference(name)
            .map_err(|e| self.err("read a ref in", e))?;
        Ok(found.and_then(|rf| rf.target().try_id().map(ToOwned::to_owned)))
    }

    /// Set a ref to `new` if it holds `expected` (`None`: it must not exist) — or
    /// unconditionally with `force`.
    pub(crate) fn ref_set(
        &self,
        name: &str,
        new: ObjectId,
        expected: Option<ObjectId>,
        force: bool,
    ) -> Result<RefSet, StoreError> {
        let now = self.ref_get(name)?;
        if !force && now != expected {
            return Ok(RefSet::Moved);
        }
        if now == Some(new) {
            return Ok(RefSet::Done);
        }
        let constraint = if force {
            PreviousValue::Any
        } else {
            match expected {
                Some(old) => PreviousValue::MustExistAndMatch(Target::Object(old)),
                None => PreviousValue::MustNotExist,
            }
        };
        match self.local().reference(name, new, constraint, "forge") {
            Ok(_) => Ok(RefSet::Done),
            // Another writer got there between the read and the locked update.
            Err(_) if !force && self.ref_get(name)? != expected => Ok(RefSet::Moved),
            Err(e) => Err(self.err("update a ref in", e)),
        }
    }

    /// Remove a ref (a no-op when there is none).
    pub(crate) fn ref_delete(&self, name: &str) -> Result<(), StoreError> {
        if self.ref_get(name)?.is_none() {
            return Ok(());
        }
        let full: gix::refs::FullName = name
            .try_into()
            .map_err(|e: gix::validate::reference::name::Error| self.err("name a ref in", e))?;
        self.local()
            .edit_reference(gix::refs::transaction::RefEdit {
                change: gix::refs::transaction::Change::Delete {
                    expected: PreviousValue::Any,
                    log: gix::refs::transaction::RefLog::AndReference,
                },
                name: full,
                deref: false,
            })
            .map(drop)
            .map_err(|e| self.err("delete a ref in", e))
    }

    /// Where received packs go.
    pub(crate) fn pack_dir(&self) -> PathBuf {
        self.git_dir.join("objects").join("pack")
    }
}

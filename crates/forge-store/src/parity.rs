//! Backend parity (I17): the checks `test_store_backend_parity` runs, as library code so
//! its positive controls can feed them deliberately broken backends.
//!
//! * [`snapshot`] reads everything a store exposes through the trait — working files and
//!   their hashes, every revision (id, parent, tree, log, message, issuers), every tree and
//!   command log, the locks — into one comparable value.
//! * [`copy_project`] moves a project between any two backends **through the trait only**,
//!   replaying its history revision by revision. Because a revision id is the hash of its
//!   content, the copy has exactly the source's revision ids — or the backend is broken.

use bytes::Bytes;

use crate::{Blake3, Lock, ProjectStore, RevId, RevRange, StoreError, StorePath};

/// Everything observable about a project, backend-independent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Working files and their content hashes, sorted.
    pub files: Vec<(StorePath, Blake3)>,
    /// Revisions newest first: id, parent, tree, log, message, issuers, command count.
    pub revs: Vec<RevSummary>,
    /// Locks.
    pub locks: Vec<Lock>,
}

/// The backend-independent part of a revision (everything but its commit time).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevSummary {
    /// Id.
    pub id: RevId,
    /// Parent.
    pub parent: Option<RevId>,
    /// Tree entries.
    pub tree: Vec<(StorePath, Blake3)>,
    /// The command log's hash.
    pub log: Blake3,
    /// Message.
    pub message: String,
    /// Issuers.
    pub issuers: Vec<String>,
    /// Command count (checked against the decoded log).
    pub commands: usize,
}

/// Read everything `s` exposes.
pub fn snapshot(s: &dyn ProjectStore) -> Result<Snapshot, StoreError> {
    let mut files = Vec::new();
    for p in s.list()? {
        let b = s.read(&p)?;
        files.push((p, Blake3::of(&b)));
    }
    let mut revs = Vec::new();
    for r in s.history(RevRange::all())? {
        let tree = s.tree(&r.id)?.entries;
        for (_, h) in &tree {
            // Every snapshot the tree names must be retrievable (and verified).
            s.blob_get(*h)?;
        }
        let commands = s.commands(&r.id)?.len();
        if commands != r.commands {
            return Err(StoreError::Corrupt {
                what: format!("revision {}", r.id),
                why: format!(
                    "it records {} commands, its log holds {commands}",
                    r.commands
                ),
            });
        }
        revs.push(RevSummary {
            id: r.id,
            parent: r.parent,
            tree,
            log: r.log,
            message: r.message,
            issuers: r.issuers,
            commands,
        });
    }
    Ok(Snapshot {
        files,
        revs,
        locks: s.locks()?,
    })
}

/// Copy `src`'s whole project into the empty `dst` through the trait: replay every
/// revision oldest first (its tree becomes `dst`'s working set, then commit with its
/// message and commands), then restore `src`'s current working files. Returns `dst`'s
/// revision ids, oldest first.
pub fn copy_project(
    src: &dyn ProjectStore,
    dst: &mut dyn ProjectStore,
) -> Result<Vec<RevId>, StoreError> {
    let mut history = src.history(RevRange::all())?;
    history.reverse();
    let mut ids = Vec::with_capacity(history.len());
    for r in &history {
        let tree = src.tree(&r.id)?.entries;
        set_working(
            dst,
            tree.iter().map(|(p, h)| (p.clone(), Source::Blob(*h))),
            src,
        )?;
        let cmds = src.commands(&r.id)?;
        ids.push(dst.commit(&r.message, &cmds)?);
    }
    let current: Vec<(StorePath, Source)> = src
        .list()?
        .into_iter()
        .map(|p| (p.clone(), Source::File(p)))
        .collect();
    set_working(dst, current.into_iter(), src)?;
    Ok(ids)
}

enum Source {
    Blob(Blake3),
    File(StorePath),
}

fn set_working(
    dst: &mut dyn ProjectStore,
    want: impl Iterator<Item = (StorePath, Source)>,
    src: &dyn ProjectStore,
) -> Result<(), StoreError> {
    let want: Vec<(StorePath, Source)> = want.collect();
    for p in dst.list()? {
        if !want.iter().any(|(w, _)| *w == p) {
            dst.delete(&p)?;
        }
    }
    for (p, s) in want {
        let bytes: Bytes = match s {
            Source::Blob(h) => src.blob_get(h)?,
            Source::File(f) => src.read(&f)?,
        };
        let same = dst
            .read(&p)
            .map(|b| Blake3::of(&b) == Blake3::of(&bytes))
            .unwrap_or(false);
        if !same {
            dst.write(&p, bytes)?;
        }
    }
    Ok(())
}

/// Human-readable differences between two snapshots (empty = parity).
#[must_use]
pub fn differences(a: &Snapshot, b: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    if a.files != b.files {
        out.push(format!(
            "working files differ:\n  {:?}\n  {:?}",
            a.files, b.files
        ));
    }
    if a.revs.len() != b.revs.len() {
        out.push(format!("{} revisions vs {}", a.revs.len(), b.revs.len()));
    }
    for (x, y) in a.revs.iter().zip(&b.revs) {
        if x != y {
            out.push(format!("revision differs:\n  {x:?}\n  {y:?}"));
        }
    }
    if a.locks != b.locks {
        out.push(format!("locks differ: {:?} vs {:?}", a.locks, b.locks));
    }
    out
}

//! Push and pull (Ch.33, E-36: they are store operations, not new concepts) — **through the
//! `ProjectStore` trait alone**, so any backend is a remote for any other: a Forge store in
//! a folder on a NAS (`file:`) and a Git remote (`https:`, `git-file:`, WP-16: a view whose
//! `publish` sends what a push replayed, never overwriting a remote that moved).
//!
//! Both are **fast-forwards**. A revision's id is the hash of its content (parent, tree,
//! command log, message), so replaying a missing revision on the other side — its tree as
//! the working files, then `commit` with its message and commands — reproduces the same id,
//! which is checked: a backend that cannot is not a remote. When the two histories do not
//! line up (each side has revisions the other lacks) nothing is copied and the refusal says
//! which side to bring up to date; merging is the conflicts flow (Ch.37 §37.4).

use forge_store::{Blake3, Bytes, ProjectStore, Rev, RevId, RevRange, StorePath};

use crate::ProjectError;

/// What a push or pull moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncReport {
    /// Revisions copied, oldest first.
    pub revisions: Vec<RevId>,
    /// The head both sides now share.
    pub head: Option<RevId>,
}

/// The revisions `to` needs from `from` to fast-forward, oldest first; an error when `to`'s
/// head is not an ancestor of `from`'s.
fn missing(
    from: &dyn ProjectStore,
    to_head: Option<RevId>,
    ahead_msg: &str,
) -> Result<Vec<Rev>, ProjectError> {
    let Some(from_head) = from.head()? else {
        return match to_head {
            None => Ok(Vec::new()),
            Some(_) => Err(ProjectError::Diverged(ahead_msg.to_string())),
        };
    };
    if Some(from_head) == to_head {
        return Ok(Vec::new());
    }
    let mut revs = from.history(RevRange {
        to: Some(from_head),
        stop_at: to_head,
        limit: None,
    })?;
    // The walk stopped at `to_head` only if it is an ancestor; otherwise it ran to the root.
    let reached = revs.last().map(|r| r.parent);
    if let Some(h) = to_head
        && reached != Some(Some(h))
    {
        return Err(ProjectError::Diverged(ahead_msg.to_string()));
    }
    revs.reverse();
    Ok(revs)
}

/// Make `dst`'s working files exactly `tree`'s, reading blobs from `src`.
fn set_working(
    dst: &mut dyn ProjectStore,
    src: &dyn ProjectStore,
    tree: &[(StorePath, Blake3)],
) -> Result<(), ProjectError> {
    for p in dst.list()? {
        if !tree.iter().any(|(w, _)| *w == p) {
            dst.delete(&p)?;
        }
    }
    for (p, h) in tree {
        let same = dst.read(p).map(|b| Blake3::of(&b) == *h).unwrap_or(false);
        if !same {
            let b: Bytes = src.blob_get(*h)?;
            dst.write(p, b)?;
        }
    }
    Ok(())
}

/// Replay `revs` (oldest first) from `src` onto `dst`.
fn replay(
    src: &dyn ProjectStore,
    dst: &mut dyn ProjectStore,
    revs: &[Rev],
) -> Result<Vec<RevId>, ProjectError> {
    let mut out = Vec::with_capacity(revs.len());
    for r in revs {
        let tree = src.tree(&r.id)?.entries;
        set_working(dst, src, &tree)?;
        let cmds = src.commands(&r.id)?;
        // The original time travels with the revision (metadata; the id does not include it).
        let id = dst.commit_at(&r.message, &cmds, r.at_ms)?;
        if id != r.id {
            return Err(ProjectError::Diverged(format!(
                "revision {} replayed as {id}: the two stores do not agree on history; nothing further was copied",
                r.id
            )));
        }
        out.push(id);
    }
    Ok(out)
}

/// Push `local`'s revisions to `remote` (a fast-forward of the remote): [`push_replay`]
/// then [`publish`].
pub fn push(
    local: &dyn ProjectStore,
    remote: &mut dyn ProjectStore,
) -> Result<SyncReport, ProjectError> {
    let copied = push_replay(local, remote)?;
    publish(remote, copied)
}

/// A push's first half: replay the revisions `remote` lacks from `local` onto it (for a
/// view of a Git remote, into its local mirror — nothing is sent yet). Reads `local`; the
/// editor core does this under its lock, and [`publish`] — the network — without it.
pub fn push_replay(
    local: &dyn ProjectStore,
    remote: &mut dyn ProjectStore,
) -> Result<Vec<RevId>, ProjectError> {
    if local.head()?.is_none() {
        return Err(ProjectError::Diverged(
            "nothing to push: save the project first".into(),
        ));
    }
    let revs = missing(
        local,
        remote.head()?,
        "the remote has revisions this project does not: pull first",
    )?;
    replay(local, remote, &revs)
}

/// A push's second half: make the `copied` revisions visible where `remote` lives (a view
/// of a Git remote sends them; an in-place store has nothing to do).
pub fn publish(
    remote: &mut dyn ProjectStore,
    copied: Vec<RevId>,
) -> Result<SyncReport, ProjectError> {
    if !copied.is_empty() {
        remote.publish()?;
    }
    Ok(SyncReport {
        revisions: copied,
        head: remote.head()?,
    })
}

/// Pull `remote`'s revisions into `local` (a fast-forward of the project). The caller makes
/// sure no unsaved work is in `local`'s working files (they become the pulled head's).
pub fn pull(
    remote: &dyn ProjectStore,
    local: &mut dyn ProjectStore,
) -> Result<SyncReport, ProjectError> {
    let revs = missing(
        remote,
        local.head()?,
        "this project has revisions the remote does not: push them first",
    )?;
    let copied = replay(remote, local, &revs)?;
    Ok(SyncReport {
        revisions: copied,
        head: local.head()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_store::MemoryStore;

    fn write(s: &mut dyn ProjectStore, p: &str, t: &str) {
        let p = StorePath::new(p).unwrap_or_else(|e| panic!("{e}"));
        s.write(&p, Bytes::from(t.to_string()))
            .unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn push_and_pull_fast_forward_with_identical_ids_and_refuse_divergence() {
        let mut a = MemoryStore::new("ada");
        let mut remote = MemoryStore::new("server");
        let mut b = MemoryStore::new("bob");
        write(&mut a, "scene.ron", "one");
        let r1 = a.commit("first", &[]).unwrap_or_else(|e| panic!("{e}"));
        write(&mut a, "scene.ron", "two");
        write(&mut a, "extra.ron", "x");
        let r2 = a.commit("second", &[]).unwrap_or_else(|e| panic!("{e}"));
        let rep = push(&a, &mut remote).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(rep.revisions, vec![r1, r2]);
        assert_eq!(remote.head().ok().flatten(), Some(r2));
        // Up to date: nothing moves.
        assert!(
            push(&a, &mut remote)
                .unwrap_or_else(|e| panic!("{e}"))
                .revisions
                .is_empty()
        );
        let rep = pull(&remote, &mut b).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(rep.head, Some(r2));
        let p = StorePath::new("extra.ron").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(b.read(&p).ok().as_deref(), Some(&b"x"[..]));
        // b moves on and pushes; a is now behind: its push is refused, its pull works.
        write(&mut b, "scene.ron", "three");
        let r3 = b.commit("third", &[]).unwrap_or_else(|e| panic!("{e}"));
        push(&b, &mut remote).unwrap_or_else(|e| panic!("{e}"));
        write(&mut a, "scene.ron", "diverge");
        let ra = a.commit("a's own", &[]).unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(
            push(&a, &mut remote),
            Err(ProjectError::Diverged(_))
        ));
        assert!(matches!(
            pull(&remote, &mut a),
            Err(ProjectError::Diverged(_))
        ));
        assert_eq!(a.head().ok().flatten(), Some(ra), "nothing was copied");
        let mut c = MemoryStore::new("cy");
        pull(&remote, &mut c).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.head().ok().flatten(), Some(r3));
        assert!(matches!(
            push(&MemoryStore::new("empty"), &mut remote),
            Err(ProjectError::Diverged(_))
        ));
    }
}

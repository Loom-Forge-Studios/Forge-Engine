//! Moving Git objects and refs between a local repository and a remote one.
//!
//! [`Transport`] is what a remote view needs: the remote's refs, a fetch, and a push that
//! moves refs only from the values it names (compare-and-swap: a remote that moved since it
//! was read is never overwritten). Two implementations:
//!
//! * [`LocalTransport`] — a repository on a path (a bare repository on a NAS share):
//!   objects are copied between the two object databases; no process is spawned and no
//!   `git` binary is needed.
//! * [`super::smart::SmartHttp`] — Git's smart HTTP protocol (GitHub, GitLab, Gitea,
//!   Forgejo, any `git http-backend`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use forge_store::StoreError;
use gix::ObjectId;

use super::repo::{RefSet, Repo};

/// A remote's refs.
pub(crate) type Refs = BTreeMap<String, ObjectId>;

/// One ref move a push asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RefUpdate {
    pub name: &'static str,
    /// What the remote must hold now (`None`: the ref must not exist).
    pub old: Option<ObjectId>,
    pub new: ObjectId,
}

/// See the module docs.
pub(crate) trait Transport: Send + Sync {
    /// The remote, for messages (never with credentials).
    fn name(&self) -> &str;
    /// The remote's refs.
    fn refs(&mut self) -> Result<Refs, StoreError>;
    /// Bring every object reachable from `wants` into `into`; `haves` are commits `into`
    /// already has (the remote may skip what they reach).
    fn fetch(
        &mut self,
        into: &Repo,
        wants: &[ObjectId],
        haves: &[ObjectId],
    ) -> Result<(), StoreError>;
    /// Send `objects` from `from`, then move the refs, in order; a ref that does not hold its
    /// `old` value fails the push with `RemoteMoved`.
    fn push(
        &mut self,
        from: &Repo,
        updates: &[RefUpdate],
        objects: &[ObjectId],
    ) -> Result<(), StoreError>;
}

/// Every tree and blob reachable from `tree` into `out`, not descending into what `stop`
/// holds or `out` already has.
fn tree_objects(
    repo: &Repo,
    tree: ObjectId,
    stop: &HashSet<ObjectId>,
    out: &mut HashSet<ObjectId>,
    order: &mut Vec<ObjectId>,
) -> Result<(), StoreError> {
    let mut stack = vec![tree];
    while let Some(t) = stack.pop() {
        if stop.contains(&t) || !out.insert(t) {
            continue;
        }
        order.push(t);
        for e in repo.tree(&t)? {
            if e.is_tree {
                stack.push(e.id);
            } else if !stop.contains(&e.id) && out.insert(e.id) {
                order.push(e.id);
            }
        }
    }
    Ok(())
}

/// The objects reachable from `tips` in `repo` that a repository holding `known` (commits
/// `repo` also has) lacks: the commits after the known ones, and their trees and blobs minus
/// what the known commits' trees hold. Commits come first, newest first.
pub(crate) fn missing_objects(
    repo: &Repo,
    tips: &[ObjectId],
    known: &[ObjectId],
) -> Result<Vec<ObjectId>, StoreError> {
    let known: HashSet<ObjectId> = known.iter().copied().filter(|k| repo.has(k)).collect();
    let mut stop = HashSet::new();
    let mut scratch = Vec::new();
    for k in &known {
        let c = repo.commit(k)?;
        tree_objects(repo, c.tree, &HashSet::new(), &mut stop, &mut scratch)?;
    }
    let mut seen = HashSet::new();
    let mut commits = Vec::new();
    let mut objects = Vec::new();
    for tip in tips {
        let mut cur = Some(*tip);
        while let Some(c) = cur {
            if known.contains(&c) || !seen.insert(c) {
                break;
            }
            commits.push(c);
            let info = repo.commit(&c)?;
            tree_objects(repo, info.tree, &stop, &mut seen, &mut objects)?;
            // Forge histories are linear (fast-forward only): the first parent is the history.
            cur = info.parents.first().copied();
        }
    }
    commits.extend(objects);
    Ok(commits)
}

/// A Git repository on a path: a bare repository on a share, or another project's.
pub(crate) struct LocalTransport {
    path: PathBuf,
    name: String,
    repo: Option<Repo>,
}

impl LocalTransport {
    pub(crate) fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            name: format!("git-file:{}", path.display()),
            repo: None,
        }
    }

    /// The remote repository; created (bare) on first use when the folder is empty or
    /// missing, so a share can be a remote without anyone running `git init --bare`.
    fn repo(&mut self) -> Result<&Repo, StoreError> {
        if self.repo.is_none() {
            let exists = self.path.join("HEAD").is_file() || self.path.join(".git").is_dir();
            let empty = std::fs::read_dir(&self.path).map_or(true, |mut d| d.next().is_none());
            let r = if exists {
                Repo::open(&self.path)?
            } else if empty {
                Repo::open_or_init_bare(&self.path)?
            } else {
                return Err(StoreError::Remote {
                    remote: self.name.clone(),
                    why: "the folder holds files but no Git repository".into(),
                });
            };
            self.repo = Some(r);
        }
        self.repo.as_ref().ok_or_else(|| StoreError::Remote {
            remote: self.name.clone(),
            why: "the repository did not open".into(),
        })
    }
}

fn copy(from: &Repo, to: &Repo, objects: &[ObjectId]) -> Result<(), StoreError> {
    for id in objects {
        if to.has(id) {
            continue;
        }
        let (kind, data) = from.read(id)?;
        to.write(kind, &data)?;
    }
    Ok(())
}

impl Transport for LocalTransport {
    fn name(&self) -> &str {
        &self.name
    }

    fn refs(&mut self) -> Result<Refs, StoreError> {
        let r = self.repo()?;
        let mut out = Refs::new();
        for name in [super::layout::MAIN_REF, super::layout::BLOBS_REF] {
            if let Some(id) = r.ref_get(name)? {
                out.insert(name.to_string(), id);
            }
        }
        Ok(out)
    }

    fn fetch(
        &mut self,
        into: &Repo,
        wants: &[ObjectId],
        haves: &[ObjectId],
    ) -> Result<(), StoreError> {
        let src = self.repo()?;
        let objects = missing_objects(src, wants, haves)?;
        copy(src, into, &objects)
    }

    fn push(
        &mut self,
        from: &Repo,
        updates: &[RefUpdate],
        objects: &[ObjectId],
    ) -> Result<(), StoreError> {
        let name = self.name.clone();
        let dst = self.repo()?;
        copy(from, dst, objects)?;
        for u in updates {
            if dst.ref_set(u.name, u.new, u.old, false)? == RefSet::Moved {
                return Err(StoreError::RemoteMoved { remote: name });
            }
        }
        Ok(())
    }
}

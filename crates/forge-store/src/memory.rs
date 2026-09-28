//! [`MemoryStore`] — the in-memory backend (D-4: labelled as such; its contents vanish when
//! it is dropped). It is a complete peer of `LocalFs`, not a mock: the parity guard holds it
//! to the same content hashes, and it serves headless tools, tests and throwaway sandboxes.

use std::collections::{BTreeMap, HashMap};

use bytes::Bytes;
use forge_cmd::{Clock, CommandEnvelope, SystemClock};

use crate::store::{CaseIndex, RevRecord, build_commit, decode_log, walk_history};
use crate::{Blake3, Lock, ProjectStore, Rev, RevId, RevRange, Stamp, StoreError, StorePath, Tree};

/// The in-memory project store.
pub struct MemoryStore {
    identity: String,
    files: BTreeMap<StorePath, (Bytes, Blake3)>,
    case: CaseIndex,
    blobs: HashMap<Blake3, Bytes>,
    revs: HashMap<RevId, RevRecord>,
    head: Option<RevId>,
    locks: BTreeMap<StorePath, String>,
    clock: Box<dyn Clock>,
}

impl MemoryStore {
    /// An empty store acting as `identity`, with the system clock.
    #[must_use]
    pub fn new(identity: &str) -> Self {
        Self::with_clock(identity, Box::new(SystemClock))
    }

    /// An empty store with an injected clock (commit times).
    #[must_use]
    pub fn with_clock(identity: &str, clock: Box<dyn Clock>) -> Self {
        Self {
            identity: identity.to_string(),
            files: BTreeMap::new(),
            case: CaseIndex::default(),
            blobs: HashMap::new(),
            revs: HashMap::new(),
            head: None,
            locks: BTreeMap::new(),
            clock,
        }
    }

    fn check_lock(&self, path: &StorePath) -> Result<(), StoreError> {
        match self.locks.get(path) {
            Some(owner) if *owner != self.identity => Err(StoreError::Locked {
                path: path.to_string(),
                owner: owner.clone(),
            }),
            _ => Ok(()),
        }
    }

    fn put(&mut self, b: Bytes) -> Blake3 {
        let h = Blake3::of(&b);
        self.blobs.entry(h).or_insert(b);
        h
    }

    fn record(&self, id: &RevId) -> Result<RevRecord, StoreError> {
        self.revs
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("revision {id}")))
    }
}

impl ProjectStore for MemoryStore {
    fn backend(&self) -> &'static str {
        "memory"
    }

    fn identity(&self) -> &str {
        &self.identity
    }

    fn read(&self, path: &StorePath) -> Result<Bytes, StoreError> {
        self.files
            .get(path)
            .map(|(b, _)| b.clone())
            .ok_or_else(|| StoreError::NotFound(path.to_string()))
    }

    fn write(&mut self, path: &StorePath, b: Bytes) -> Result<(), StoreError> {
        self.check_lock(path)?;
        if !self.files.contains_key(path) {
            if let Some(existing) = self.case.collision(path) {
                return Err(StoreError::CaseCollision {
                    path: path.to_string(),
                    existing,
                });
            }
            self.case.insert(path);
        }
        let h = Blake3::of(&b);
        self.files.insert(path.clone(), (b, h));
        Ok(())
    }

    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError> {
        self.check_lock(path)?;
        if self.files.remove(path).is_none() {
            return Err(StoreError::NotFound(path.to_string()));
        }
        self.case.remove(path);
        Ok(())
    }

    fn list(&self) -> Result<Vec<StorePath>, StoreError> {
        Ok(self.files.keys().cloned().collect())
    }

    fn blob_get(&self, h: Blake3) -> Result<Bytes, StoreError> {
        let b = self
            .blobs
            .get(&h)
            .ok_or_else(|| StoreError::NotFound(format!("blob {h}")))?;
        if Blake3::of(b) != h {
            return Err(StoreError::Corrupt {
                what: format!("blob {h}"),
                why: "its bytes hash differently".into(),
            });
        }
        Ok(b.clone())
    }

    fn blob_put(&mut self, b: Bytes) -> Result<Blake3, StoreError> {
        Ok(self.put(b))
    }

    fn blob_has(&self, h: Blake3) -> Result<bool, StoreError> {
        Ok(self.blobs.contains_key(&h))
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
        let snapshot: Vec<(StorePath, Bytes, Blake3)> = self
            .files
            .iter()
            .map(|(p, (b, h))| (p.clone(), b.clone(), *h))
            .collect();
        let mut files = Vec::with_capacity(snapshot.len());
        for (p, b, h) in snapshot {
            self.blobs.entry(h).or_insert(b);
            files.push((p, h));
        }
        let rec = build_commit(files, self.head, msg, envelopes, at, &mut |b| {
            Ok(self.put(b))
        })?;
        let id = rec.id();
        self.revs.insert(id, rec);
        self.head = Some(id);
        Ok(id)
    }

    fn head(&self) -> Result<Option<RevId>, StoreError> {
        Ok(self.head)
    }

    fn history(&self, range: RevRange) -> Result<Vec<Rev>, StoreError> {
        walk_history(&range, self.head, &|id| self.record(id))
    }

    fn tree(&self, rev: &RevId) -> Result<Tree, StoreError> {
        let rec = self.record(rev)?;
        let h = rec.to_rev(*rev)?.tree;
        Tree::decode(&self.blob_get(h)?)
    }

    fn commands(&self, rev: &RevId) -> Result<Vec<CommandEnvelope>, StoreError> {
        let rec = self.record(rev)?;
        let h = rec.to_rev(*rev)?.log;
        decode_log(&self.blob_get(h)?)
    }

    fn lock(&mut self, path: &StorePath) -> Result<Lock, StoreError> {
        self.check_lock(path)?;
        self.locks.insert(path.clone(), self.identity.clone());
        Ok(Lock {
            path: path.clone(),
            owner: self.identity.clone(),
        })
    }

    fn unlock(&mut self, lock: &Lock) -> Result<(), StoreError> {
        match self.locks.get(&lock.path) {
            None => Err(StoreError::NotFound(format!("a lock on {}", lock.path))),
            Some(owner) if *owner != self.identity => Err(StoreError::NotLockOwner {
                path: lock.path.to_string(),
                owner: owner.clone(),
                by: self.identity.clone(),
            }),
            Some(_) => {
                self.locks.remove(&lock.path);
                Ok(())
            }
        }
    }

    fn locks(&self) -> Result<Vec<Lock>, StoreError> {
        Ok(self
            .locks
            .iter()
            .map(|(p, o)| Lock {
                path: p.clone(),
                owner: o.clone(),
            })
            .collect())
    }

    fn stamps(&self) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        Ok(self
            .files
            .iter()
            .map(|(p, (_, h))| (p.clone(), Stamp::of_content(h)))
            .collect())
    }

    fn stamps_of(&self, paths: &[StorePath]) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        let mut out: Vec<(StorePath, Stamp)> = paths
            .iter()
            .filter_map(|p| {
                self.files
                    .get(p)
                    .map(|(_, h)| (p.clone(), Stamp::of_content(h)))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.dedup_by(|a, b| a.0 == b.0);
        Ok(out)
    }

    fn size(&self, path: &StorePath) -> Result<Option<u64>, StoreError> {
        Ok(self.files.get(path).map(|(b, _)| b.len() as u64))
    }
}

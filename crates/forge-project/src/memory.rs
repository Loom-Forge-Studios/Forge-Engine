//! **In-memory named stores (D-4, labelled).** `memory:<name>` opens the same
//! [`MemoryStore`] every time within this process — a project or a remote that lives only
//! as long as the process does. It is how the lifecycle flows are tested end to end
//! without a disk and how a remote behaves before the Git backend exists (WP-16, M2-15);
//! it is never presented as saved: the status line says "in memory".
//!
//! (`forge-store`'s own `memory:` backend opens a fresh store on every call; this registry
//! is what gives a name its identity.)

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use forge_cmd::CommandEnvelope;
use forge_store::{
    Blake3, Bytes, Lock, MemoryStore, ProjectStore, Rev, RevId, RevRange, Stamp, StoreError,
    StorePath, Tree,
};

type Shared = Arc<Mutex<MemoryStore>>;

fn registry() -> &'static Mutex<BTreeMap<String, Shared>> {
    static R: OnceLock<Mutex<BTreeMap<String, Shared>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while a store is held cannot leave a MemoryStore half-written (every method
    // completes its maps before returning), so a poisoned lock is still consistent.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Open (creating on first use) the in-memory store named `name`.
#[must_use]
pub fn open_named(name: &str, identity: &str) -> Box<dyn ProjectStore> {
    let shared = guard(registry())
        .entry(name.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(MemoryStore::new(identity))))
        .clone();
    Box::new(NamedStore {
        shared,
        identity: identity.to_string(),
    })
}

/// Whether a named in-memory store exists (a test's "the remote was created").
#[must_use]
pub fn exists(name: &str) -> bool {
    guard(registry()).contains_key(name)
}

/// A handle on a named store: every call goes to the one shared [`MemoryStore`].
struct NamedStore {
    shared: Shared,
    /// Who opened this handle (commit author for the status line; lock ownership is the
    /// shared store's, fixed when the name was first opened).
    identity: String,
}

impl ProjectStore for NamedStore {
    fn backend(&self) -> &'static str {
        "memory"
    }
    fn identity(&self) -> &str {
        &self.identity
    }
    fn read(&self, path: &StorePath) -> Result<Bytes, StoreError> {
        guard(&self.shared).read(path)
    }
    fn write(&mut self, path: &StorePath, b: Bytes) -> Result<(), StoreError> {
        guard(&self.shared).write(path, b)
    }
    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError> {
        guard(&self.shared).delete(path)
    }
    fn list(&self) -> Result<Vec<StorePath>, StoreError> {
        guard(&self.shared).list()
    }
    fn blob_get(&self, h: Blake3) -> Result<Bytes, StoreError> {
        guard(&self.shared).blob_get(h)
    }
    fn blob_put(&mut self, b: Bytes) -> Result<Blake3, StoreError> {
        guard(&self.shared).blob_put(b)
    }
    fn blob_has(&self, h: Blake3) -> Result<bool, StoreError> {
        guard(&self.shared).blob_has(h)
    }
    fn commit(&mut self, msg: &str, envelopes: &[CommandEnvelope]) -> Result<RevId, StoreError> {
        guard(&self.shared).commit(msg, envelopes)
    }
    fn commit_at(
        &mut self,
        msg: &str,
        envelopes: &[CommandEnvelope],
        at_ms: u64,
    ) -> Result<RevId, StoreError> {
        guard(&self.shared).commit_at(msg, envelopes, at_ms)
    }
    fn head(&self) -> Result<Option<RevId>, StoreError> {
        guard(&self.shared).head()
    }
    fn history(&self, range: RevRange) -> Result<Vec<Rev>, StoreError> {
        guard(&self.shared).history(range)
    }
    fn tree(&self, rev: &RevId) -> Result<Tree, StoreError> {
        guard(&self.shared).tree(rev)
    }
    fn commands(&self, rev: &RevId) -> Result<Vec<CommandEnvelope>, StoreError> {
        guard(&self.shared).commands(rev)
    }
    fn lock(&mut self, path: &StorePath) -> Result<Lock, StoreError> {
        guard(&self.shared).lock(path)
    }
    fn unlock(&mut self, lock: &Lock) -> Result<(), StoreError> {
        guard(&self.shared).unlock(lock)
    }
    fn locks(&self) -> Result<Vec<Lock>, StoreError> {
        guard(&self.shared).locks()
    }
    fn stamps(&self) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        guard(&self.shared).stamps()
    }
    fn size(&self, path: &StorePath) -> Result<Option<u64>, StoreError> {
        guard(&self.shared).size(path)
    }
    fn stamps_of(&self, paths: &[StorePath]) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        guard(&self.shared).stamps_of(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_store_for_the_whole_process() {
        let name = "forge-project-memory-test";
        let p = StorePath::new("a.txt").unwrap_or_else(|e| panic!("{e}"));
        let mut a = open_named(name, "ada");
        a.write(&p, Bytes::from_static(b"hi"))
            .unwrap_or_else(|e| panic!("{e}"));
        let rev = a.commit("one", &[]).unwrap_or_else(|e| panic!("{e}"));
        let b = open_named(name, "bob");
        assert_eq!(b.head().ok().flatten(), Some(rev));
        assert_eq!(b.read(&p).ok().as_deref(), Some(&b"hi"[..]));
        assert!(exists(name));
        assert_eq!(b.backend(), "memory");
    }
}

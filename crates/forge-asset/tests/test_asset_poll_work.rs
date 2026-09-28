//! Hot reload's poll: what it re-stamps after its own writes, and how much it reads when
//! sources vanish (M2-6, owner rules 1 and 2).
//!
//! * A poll that writes (it moves a sidecar after a source was renamed without it) takes the
//!   stamps of exactly the files it wrote as the new baseline, so the next poll is quiet —
//!   and an edit the user made to another file while that poll ran is **not** swallowed:
//!   the next poll sees it. A second whole-project walk at the end of the poll would pass
//!   the first check and fail the second; no re-stamp at all fails the first.
//! * When sources vanish, the new files are compared by size before anything is read, and
//!   each candidate is read and hashed at most once per poll, however many vanished sources
//!   consider it (a branch switch vanishes hundreds and adds thousands).
//!
//! Positive control (W2): `control_a_store_that_restamps_nothing_makes_the_next_poll_busy`
//! runs the first scenario over a store whose `stamps_of` answers nothing — the re-stamp
//! step with no effect — and the next poll is not quiet.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use common::{open, p};
use forge_asset::fixture;
use forge_asset::{AssetEvent, AssetServer, Settings};
use forge_cmd::CommandEnvelope;
use forge_store::{
    Blake3, Bytes, Lock, MemoryStore, ProjectStore, Rev, RevId, RevRange, Stamp, StoreError,
    StorePath, Tree,
};

/// A memory store the test can reach behind the server's back: it counts reads per path,
/// can edit a file the moment the server writes a sidecar (an edit "during the poll"), and
/// can be told to re-stamp nothing (the control).
#[derive(Clone)]
struct Probed {
    inner: Arc<Mutex<MemoryStore>>,
    identity: String,
    reads: Arc<Mutex<BTreeMap<StorePath, usize>>>,
    on_sidecar_write: Arc<Mutex<Option<(StorePath, Bytes)>>>,
    restamp_nothing: bool,
}

impl Probed {
    fn new(restamp_nothing: bool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MemoryStore::new("ada"))),
            identity: "ada".into(),
            reads: Arc::default(),
            on_sidecar_write: Arc::default(),
            restamp_nothing,
        }
    }
    fn s(&self) -> std::sync::MutexGuard<'_, MemoryStore> {
        self.inner.lock().expect("store")
    }
    /// A change made outside the asset system (a text editor, a VCS checkout).
    fn outside_write(&self, path: &str, b: Bytes) {
        self.s().write(&p(path), b).expect("write");
    }
    fn outside_delete(&self, path: &str) {
        self.s().delete(&p(path)).expect("delete");
    }
    fn outside_read(&self, path: &str) -> Bytes {
        self.s().read(&p(path)).expect("read")
    }
    fn reads_of(&self, path: &str) -> usize {
        self.reads
            .lock()
            .expect("reads")
            .get(&p(path))
            .copied()
            .unwrap_or(0)
    }
    fn reset_reads(&self) {
        self.reads.lock().expect("reads").clear();
    }
}

impl ProjectStore for Probed {
    fn backend(&self) -> &'static str {
        "probed-memory"
    }
    fn identity(&self) -> &str {
        &self.identity
    }
    fn read(&self, path: &StorePath) -> Result<Bytes, StoreError> {
        *self
            .reads
            .lock()
            .expect("reads")
            .entry(path.clone())
            .or_default() += 1;
        self.s().read(path)
    }
    fn write(&mut self, path: &StorePath, b: Bytes) -> Result<(), StoreError> {
        self.s().write(path, b)?;
        if path.as_str().ends_with(".meta.ron")
            && let Some((q, edit)) = self.on_sidecar_write.lock().expect("hook").take()
        {
            self.s().write(&q, edit)?;
        }
        Ok(())
    }
    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError> {
        self.s().delete(path)
    }
    fn list(&self) -> Result<Vec<StorePath>, StoreError> {
        self.s().list()
    }
    fn blob_get(&self, h: Blake3) -> Result<Bytes, StoreError> {
        self.s().blob_get(h)
    }
    fn blob_put(&mut self, b: Bytes) -> Result<Blake3, StoreError> {
        self.s().blob_put(b)
    }
    fn blob_has(&self, h: Blake3) -> Result<bool, StoreError> {
        self.s().blob_has(h)
    }
    fn commit(&mut self, msg: &str, envelopes: &[CommandEnvelope]) -> Result<RevId, StoreError> {
        self.s().commit(msg, envelopes)
    }
    fn head(&self) -> Result<Option<RevId>, StoreError> {
        self.s().head()
    }
    fn history(&self, range: RevRange) -> Result<Vec<Rev>, StoreError> {
        self.s().history(range)
    }
    fn tree(&self, rev: &RevId) -> Result<Tree, StoreError> {
        self.s().tree(rev)
    }
    fn commands(&self, rev: &RevId) -> Result<Vec<CommandEnvelope>, StoreError> {
        self.s().commands(rev)
    }
    fn lock(&mut self, path: &StorePath) -> Result<Lock, StoreError> {
        self.s().lock(path)
    }
    fn unlock(&mut self, lock: &Lock) -> Result<(), StoreError> {
        self.s().unlock(lock)
    }
    fn locks(&self) -> Result<Vec<Lock>, StoreError> {
        self.s().locks()
    }
    fn stamps(&self) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        self.s().stamps()
    }
    fn stamps_of(&self, paths: &[StorePath]) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        if self.restamp_nothing {
            return Ok(Vec::new());
        }
        self.s().stamps_of(paths)
    }
    fn size(&self, path: &StorePath) -> Result<Option<u64>, StoreError> {
        self.s().size(path)
    }
}

fn png(seed: u8) -> Bytes {
    fixture::checker_png(8 + u32::from(seed % 8), [seed, 20, 30, 255], [0, 0, 0, 255])
}

fn import(a: &mut AssetServer, path: &str) -> forge_asset::AssetId {
    let mut ev = Vec::new();
    a.import(&p(path), Settings::new(), &mut ev)
        .unwrap_or_else(|e| panic!("import {path}: {e}"))
}

/// Import `tex/a.png`, rename it outside the editor (its sidecar stays behind), and poll:
/// the poll re-attaches the id by content and moves the sidecar — its own writes.
fn rename_without_sidecar(store: &Probed) -> AssetServer {
    store.outside_write("tex/a.png", png(1));
    store.outside_write("tex/c.png", png(2));
    let mut a = open(Box::new(store.clone()));
    let id = import(&mut a, "tex/a.png");
    import(&mut a, "tex/c.png");
    // (This poll takes the import's sidecars into its baseline.)
    let ev = a.poll().expect("polls");
    assert!(ev.is_empty(), "nothing changed since the import: {ev:?}");

    let bytes = store.outside_read("tex/a.png");
    store.outside_delete("tex/a.png");
    store.outside_write("tex/b.png", bytes);
    let ev = a.poll().expect("polls");
    assert!(
        ev.contains(&AssetEvent::Renamed {
            id,
            from: p("tex/a.png"),
            to: p("tex/b.png"),
        }),
        "the rename is recognised by content: {ev:?}"
    );
    assert!(a.vfs().exists(&p("tex/b.png.meta.ron")).expect("exists"));
    assert!(!a.vfs().exists(&p("tex/a.png.meta.ron")).expect("exists"));
    a
}

#[test]
fn a_poll_that_writes_sidecars_leaves_the_next_poll_quiet() {
    let store = Probed::new(false);
    let mut a = rename_without_sidecar(&store);
    let ev = a.poll().expect("polls");
    assert!(ev.is_empty(), "the poll's own writes came back: {ev:?}");
    assert_eq!(
        a.last_poll_changes(),
        0,
        "the next poll must be quiet: the files the last poll wrote are its baseline"
    );
}

#[test]
fn an_edit_made_while_a_poll_runs_is_seen_by_the_next_poll() {
    let store = Probed::new(false);
    store.outside_write("tex/a.png", png(1));
    store.outside_write("tex/c.png", png(2));
    let mut a = open(Box::new(store.clone()));
    import(&mut a, "tex/a.png");
    let c_id = import(&mut a, "tex/c.png");
    assert!(a.poll().expect("polls").is_empty());
    // Armed after the imports (which write sidecars too): fires when the rename poll moves
    // a.png's sidecar, i.e. after that poll took its stamps and before it re-stamps.
    *store.on_sidecar_write.lock().expect("hook") = Some((p("tex/c.png"), png(3)));
    let bytes = store.outside_read("tex/a.png");
    store.outside_delete("tex/a.png");
    store.outside_write("tex/b.png", bytes);
    a.poll().expect("polls");
    assert!(
        store.on_sidecar_write.lock().expect("hook").is_none(),
        "fixture: the edit happened during the poll"
    );
    let ev = a.poll().expect("polls");
    assert!(
        ev.contains(&AssetEvent::Reimported {
            id: c_id,
            path: p("tex/c.png"),
        }),
        "the edit to tex/c.png made during the last poll was swallowed: {ev:?}"
    );
    assert_eq!(a.last_poll_changes(), 1, "exactly the edited file");
}

#[test]
fn control_a_store_that_restamps_nothing_makes_the_next_poll_busy() {
    let store = Probed::new(true);
    let mut a = rename_without_sidecar(&store);
    a.poll().expect("polls");
    assert!(
        a.last_poll_changes() > 0,
        "without the re-stamp the poll's own sidecar writes must show up as changes"
    );
}

#[test]
fn a_branch_switch_reads_each_new_file_at_most_once_and_only_on_a_size_match() {
    const VANISH: u8 = 12;
    let store = Probed::new(false);
    for i in 0..VANISH {
        store.outside_write(&format!("old/t{i}.png"), png(i));
    }
    let mut a = open(Box::new(store.clone()));
    for i in 0..VANISH {
        import(&mut a, &format!("old/t{i}.png"));
    }
    assert!(a.poll().expect("polls").is_empty());
    let sizes: Vec<usize> = (0..VANISH)
        .map(|i| store.outside_read(&format!("old/t{i}.png")).len())
        .collect();

    // The switch: every source goes (sidecars stay: they are the vanished sources' records),
    // one comes back under a new name, and many unrelated files arrive — some the same size
    // as a vanished source with other bytes, most of another size.
    let moved = store.outside_read("old/t0.png");
    for i in 0..VANISH {
        store.outside_delete(&format!("old/t{i}.png"));
    }
    store.outside_write("new/moved.png", moved);
    let mut same_size = Vec::new();
    let mut other_size = Vec::new();
    for (i, n) in sizes.iter().enumerate() {
        let path = format!("new/same{i}.dat");
        store.outside_write(&path, Bytes::from(vec![0xA5; *n]));
        same_size.push(path);
    }
    let max = sizes.iter().copied().max().unwrap_or(0);
    for i in 0..200 {
        let path = format!("new/other{i:03}.dat");
        store.outside_write(&path, Bytes::from(vec![7; max + 1 + i]));
        other_size.push(path);
    }

    store.reset_reads();
    let ev = a.poll().expect("polls");
    assert!(
        ev.iter().any(|e| matches!(
            e,
            AssetEvent::Renamed { to, .. } if *to == p("new/moved.png")
        )),
        "the renamed source is still found: {ev:?}"
    );
    for f in &other_size {
        assert_eq!(
            store.reads_of(f),
            0,
            "{f} cannot be a renamed source (its size matches none) but was read"
        );
    }
    for f in &same_size {
        let n = store.reads_of(f);
        assert!(
            n <= 1,
            "{f} was read {n} times in one poll ({VANISH} sources vanished)"
        );
    }
    assert!(
        same_size.iter().any(|f| store.reads_of(f) == 1),
        "fixture: a same-size candidate was compared by content"
    );
}

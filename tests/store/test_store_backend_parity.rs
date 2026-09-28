//! I17 — `test_store_backend_parity` (Ch.33.1): the same project, driven through every
//! backend by the same script, produces identical content hashes — working files, revision
//! ids, trees, command logs, locks — and round-trips from any backend to any other through
//! the `ProjectStore` trait alone with its revision ids intact. "A backend that cannot do
//! that is not a backend."
//!
//! Backends: `LocalFs` (a real folder), `MemoryStore` and Git (M2-15: a project folder whose
//! history is a Git repository, and a view of a Git remote), each opened through the
//! `StoreBackend` extension point after the first-party store plugin loaded through the
//! ordinary plugin loader (no backend is special-cased). S3 and SQL join this list when they
//! exist (gate row `C-store-s3-sql`).
//!
//! Positive control (W2): `positive_control_a_broken_backend_fails_parity` runs the same
//! script on backends with one classic defect each (CRLF translation on write, a commit that
//! drops the command log, an unsorted listing, a lock that does not hold) — every one must be
//! caught.

use std::path::PathBuf;

use forge_cmd::{
    Bus, Change, CommandEnvelope, CommandSink, EditorCommand, FixedClock, Issuer, Value,
};
use forge_plugin::{Extensions, Grants, loader};
use forge_store::parity::{copy_project, differences, snapshot};
use forge_store::{
    Blake3, Bytes, LocalFs, Lock, MemoryStore, ProjectStore, Rev, RevId, RevRange, StoreBackend,
    StoreError, StorePath, Tree, open_store,
};

fn p(s: &str) -> StorePath {
    StorePath::new(s).expect("valid path")
}

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("store-parity")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Real envelopes from a real bus: a human builds a scene, an automation session tweaks it.
fn envelopes() -> Vec<CommandEnvelope> {
    let mut bus = Bus::with_clock(Box::new(FixedClock(0)));
    let ada = Issuer::Human { user: "ada".into() };
    let auto = Issuer::Automation {
        session: "sess-4".into(),
        tool: "apply".into(),
    };
    let mut out = Vec::new();
    let e = bus.envelope(
        ada.clone(),
        EditorCommand::Spawn {
            name: "World".into(),
            parent: None,
        },
    );
    out.push(e.clone());
    let world = match bus.apply(e).expect("spawn").diff.changes.first() {
        Some(Change::Created { entity, .. }) => *entity,
        other => panic!("{other:?}"),
    };
    for (who, x) in [(ada, 1.0), (auto.clone(), 2.5), (auto, -0.0)] {
        let e = bus.envelope(
            who,
            EditorCommand::SetProperty {
                entity: world,
                path: "light.intensity".into(),
                value: Value::Float(x),
            },
        );
        out.push(e.clone());
        bus.apply(e).expect("edit");
    }
    out
}

/// The script every backend runs: text and binary files (with CR/LF and NUL bytes, where
/// broken backends go wrong), two commits carrying command logs, a lock, a delete, and an
/// uncommitted working change.
fn script(s: &mut dyn ProjectStore) -> Result<Vec<RevId>, StoreError> {
    let envs = envelopes();
    let rock: &[u8] = b"PNG\r\n\x1a\n\0\xffrock\r\n";
    s.write(
        &p("scenes/main.ron"),
        Bytes::from_static(b"Scene(\n  lights: 1,\n)\n"),
    )?;
    s.write(&p("assets/rock.png"), Bytes::from_static(rock))?;
    s.write(
        &p("project.ron"),
        Bytes::from_static(b"Project(preset: ThreeD)\r\n"),
    )?;
    let rock_h = s.blob_put(Bytes::from_static(rock))?;
    assert_eq!(
        rock_h,
        Blake3::of(rock),
        "blob_put returns the content address"
    );
    let r1 = s.commit("human:ada built the world", &envs[..2])?;
    s.lock(&p("assets/rock.png"))?;
    s.write(
        &p("scenes/main.ron"),
        Bytes::from_static(b"Scene(\n  lights: 2,\n)\n"),
    )?;
    s.write(&p("levels/one.ron"), Bytes::from_static(b"Level(1)"))?;
    s.delete(&p("project.ron"))?;
    let r2 = s.commit("automation:sess-4 tuned the light", &envs[2..])?;
    s.delete(&p("levels/one.ron"))?;
    s.write(&p("notes.txt"), Bytes::from_static(b"uncommitted\n"))?;
    Ok(vec![r1, r2])
}

fn host() -> Extensions {
    let mut x = Extensions::new();
    x.define::<StoreBackend>().expect("define");
    // Remote views mirror into this run's own cache (never the user's).
    let git = forge_store_backends::git::GitOptions {
        cache_dir: tmp("git-cache"),
        ..forge_store_backends::git::GitOptions::new()
    };
    let stores = forge_store_backends::StoreBackends::with_git(git).expect("first-party manifest");
    loader::load(&mut x, &[&stores], &[], &Grants::new()).expect("loads");
    x
}

#[test]
fn every_backend_holds_the_same_project_bit_for_bit() {
    let x = host();
    let dir = tmp("local");
    let url = format!("file:{}", dir.display());
    let mut local = open_store(&x, &url, "ada").expect("file backend");
    let mut memory = open_store(&x, "memory:parity", "ada").expect("memory backend");
    // Git (M2-15): a project folder whose history is a Git repository, and a view of a Git
    // remote (a bare repository on a path) that commits into its mirror and publishes.
    let gdir = tmp("git");
    let mut git = open_store(&x, &format!("git:{}", gdir.display()), "ada").expect("git backend");
    let bare = tmp("git-remote").join("orbits.git");
    let remote_url = format!("git-file:{}", bare.display());
    let mut remote = open_store(&x, &remote_url, "ada").expect("git remote backend");
    assert_eq!(
        (
            local.backend(),
            memory.backend(),
            git.backend(),
            remote.backend()
        ),
        ("local-fs", "memory", "git", "git")
    );

    let ids_local = script(local.as_mut()).expect("local runs the script");
    let ids_memory = script(memory.as_mut()).expect("memory runs the script");
    let ids_git = script(git.as_mut()).expect("git runs the script");
    let ids_remote = script(remote.as_mut()).expect("a git remote view runs the script");
    assert_eq!(
        ids_local, ids_memory,
        "revision ids are content, not backend"
    );
    assert_eq!(ids_local, ids_git);
    assert_eq!(ids_local, ids_remote);

    let a = snapshot(local.as_ref()).expect("snapshot");
    for (name, s) in [("memory", &memory), ("git", &git), ("git remote", &remote)] {
        let b = snapshot(s.as_ref()).expect("snapshot");
        let diff = differences(&a, &b);
        assert!(
            diff.is_empty(),
            "I17 parity broken ({name}):\n{}",
            diff.join("\n")
        );
    }
    // Published, the remote holds the same history for whoever opens it next.
    remote.publish().expect("publish");
    let reopened = open_store(&x, &remote_url, "bob").expect("reopen the remote");
    assert_eq!(snapshot(reopened.as_ref()).expect("snapshot").revs, a.revs);

    // Non-vacuity: the snapshot really covers files, revisions, trees, logs and locks.
    assert_eq!(a.files.len(), 3, "{:?}", a.files);
    assert_eq!(a.revs.len(), 2);
    assert_eq!(a.revs[0].parent, Some(a.revs[1].id));
    assert_eq!(a.revs[1].tree.len(), 3);
    assert_eq!(a.revs[0].tree.len(), 3);
    assert_eq!((a.revs[1].commands, a.revs[0].commands), (2, 2));
    assert_eq!(a.revs[0].issuers, ["automation:sess-4"]);
    assert_eq!(a.locks.len(), 1);
    // The command log is the real history: envelopes come back exactly.
    let log = local.commands(&ids_local[1]).expect("log");
    assert_eq!(log, envelopes()[2..].to_vec());
}

#[test]
fn a_project_round_trips_through_every_backend_with_its_revision_ids() {
    let mut original = MemoryStore::with_clock("ada", Box::new(FixedClock(1_000)));
    let ids = script(&mut original).expect("script");
    let mut want = snapshot(&original).expect("snapshot");
    want.locks.clear(); // locks are live state of a store, not project content

    // memory -> LocalFs -> Git -> LocalFs, each hop through the trait only, each with a
    // different clock (commit times are metadata; ids must not depend on them).
    let d1 = tmp("hop1");
    let d2 = tmp("hop2");
    let dg = tmp("hop-git");
    let mut hop1 = LocalFs::open_with_clock(&d1, "bob", Box::new(FixedClock(2_000))).expect("open");
    let mut hop2 = forge_store_backends::git::GitStore::open_folder_with_clock(
        &dg,
        "carol",
        Box::new(FixedClock(3_000)),
    )
    .expect("open");
    let mut hop3 = LocalFs::open_with_clock(&d2, "dan", Box::new(FixedClock(4_000))).expect("open");
    assert_eq!(copy_project(&original, &mut hop1).expect("copy"), ids);
    assert_eq!(copy_project(&hop1, &mut hop2).expect("copy"), ids);
    assert_eq!(copy_project(&hop2, &mut hop3).expect("copy"), ids);
    for (name, s) in [
        ("hop1", &hop1 as &dyn ProjectStore),
        ("hop2", &hop2),
        ("hop3", &hop3),
    ] {
        let got = snapshot(s).expect("snapshot");
        let diff = differences(&want, &got);
        assert!(diff.is_empty(), "{name}:\n{}", diff.join("\n"));
    }
    let times: Vec<u64> = hop3
        .history(RevRange::all())
        .expect("history")
        .iter()
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(times, [4_000, 4_000], "metadata is the destination's own");
}

// ---- positive control ------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// No fault: the wrapper itself must pass.
    None,
    /// Text-mode writes: CRLF becomes LF (the classic Windows/Linux backend bug).
    CrlfOnWrite,
    /// `commit` records the tree but drops the command log.
    DropsCommandLog,
    /// `list` returns files in reverse order.
    UnsortedList,
    /// `lock` succeeds but holds nothing.
    LockDoesNotHold,
}

struct Broken {
    inner: MemoryStore,
    fault: Fault,
}

impl ProjectStore for Broken {
    fn backend(&self) -> &'static str {
        "broken"
    }
    fn identity(&self) -> &str {
        self.inner.identity()
    }
    fn read(&self, path: &StorePath) -> Result<Bytes, StoreError> {
        self.inner.read(path)
    }
    fn write(&mut self, path: &StorePath, b: Bytes) -> Result<(), StoreError> {
        if self.fault == Fault::CrlfOnWrite {
            let mut v = Vec::with_capacity(b.len());
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
                    i += 1;
                    continue;
                }
                v.push(b[i]);
                i += 1;
            }
            return self.inner.write(path, Bytes::from(v));
        }
        self.inner.write(path, b)
    }
    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError> {
        self.inner.delete(path)
    }
    fn list(&self) -> Result<Vec<StorePath>, StoreError> {
        let mut l = self.inner.list()?;
        if self.fault == Fault::UnsortedList {
            l.reverse();
        }
        Ok(l)
    }
    fn blob_get(&self, h: Blake3) -> Result<Bytes, StoreError> {
        self.inner.blob_get(h)
    }
    fn blob_put(&mut self, b: Bytes) -> Result<Blake3, StoreError> {
        self.inner.blob_put(b)
    }
    fn blob_has(&self, h: Blake3) -> Result<bool, StoreError> {
        self.inner.blob_has(h)
    }
    fn commit(&mut self, msg: &str, envelopes: &[CommandEnvelope]) -> Result<RevId, StoreError> {
        if self.fault == Fault::DropsCommandLog {
            return self.inner.commit(msg, &[]);
        }
        self.inner.commit(msg, envelopes)
    }
    fn head(&self) -> Result<Option<RevId>, StoreError> {
        self.inner.head()
    }
    fn history(&self, range: RevRange) -> Result<Vec<Rev>, StoreError> {
        self.inner.history(range)
    }
    fn tree(&self, rev: &RevId) -> Result<Tree, StoreError> {
        self.inner.tree(rev)
    }
    fn commands(&self, rev: &RevId) -> Result<Vec<CommandEnvelope>, StoreError> {
        self.inner.commands(rev)
    }
    fn lock(&mut self, path: &StorePath) -> Result<Lock, StoreError> {
        if self.fault == Fault::LockDoesNotHold {
            return Ok(Lock {
                path: path.clone(),
                owner: self.inner.identity().to_string(),
            });
        }
        self.inner.lock(path)
    }
    fn unlock(&mut self, lock: &Lock) -> Result<(), StoreError> {
        self.inner.unlock(lock)
    }
    fn locks(&self) -> Result<Vec<Lock>, StoreError> {
        self.inner.locks()
    }
}

#[test]
fn positive_control_a_broken_backend_fails_parity() {
    let mut reference = MemoryStore::new("ada");
    let ref_ids = script(&mut reference).expect("script");
    let want = snapshot(&reference).expect("snapshot");
    for fault in [
        Fault::CrlfOnWrite,
        Fault::DropsCommandLog,
        Fault::UnsortedList,
        Fault::LockDoesNotHold,
    ] {
        let mut b = Broken {
            inner: MemoryStore::new("ada"),
            fault,
        };
        let ids = script(&mut b).expect("the broken backend still runs the script");
        let got = snapshot(&b).expect("snapshot");
        let diff = differences(&want, &got);
        assert!(
            !diff.is_empty(),
            "{fault:?} was not caught by the parity check"
        );
        if fault == Fault::DropsCommandLog {
            assert_ne!(ids, ref_ids, "a dropped log changes the revision ids");
        }
    }
    // The wrapper without a fault passes: the faults, not the wrapper, trip the check.
    let mut clean = Broken {
        inner: MemoryStore::new("ada"),
        fault: Fault::None,
    };
    assert_eq!(script(&mut clean).expect("script"), ref_ids);
    let diff = differences(&want, &snapshot(&clean).expect("snapshot"));
    assert!(diff.is_empty(), "{}", diff.join("; "));
}

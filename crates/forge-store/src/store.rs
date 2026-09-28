//! The [`ProjectStore`] trait (I17) and the revision model every backend shares.
//!
//! A commit snapshots the working files into the blob store (content-addressed, so an
//! unchanged file costs nothing), records the tree (path -> content address) as a blob,
//! records the command envelopes (Ch.33.4: the real history) as a blob, and writes a
//! revision record naming the three addresses. The [`RevId`] is the BLAKE3 of the record's
//! **content** (parent, tree, log, message) — never of a timestamp or a backend detail — so
//! the same project committed through any backend has the same revision ids
//! (`test_store_backend_parity`).

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use bytes::Bytes;
use forge_cmd::CommandEnvelope;
use serde::{Deserialize, Serialize};

use crate::{Blake3, StoreError, StorePath};

/// A revision's id: the BLAKE3 of its content.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RevId(pub Blake3);

impl fmt::Display for RevId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for RevId {
    type Err = StoreError;
    fn from_str(s: &str) -> Result<Self, StoreError> {
        s.parse().map(Self)
    }
}

/// One revision, as the history panel lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rev {
    /// Its id.
    pub id: RevId,
    /// The revision before it (`None`: the first).
    pub parent: Option<RevId>,
    /// The address of its tree (see [`Tree`]).
    pub tree: Blake3,
    /// The address of its command log.
    pub log: Blake3,
    /// The commit message.
    pub message: String,
    /// When, per the store's clock. Metadata: not part of the id.
    pub at_ms: u64,
    /// The distinct issuers of its commands (`human:ada`, `automation:sess-4`), first-seen order
    /// — so history reads "automation:sess-4 placed 37 trees", not "scene.ron changed".
    pub issuers: Vec<String>,
    /// How many commands it carries.
    pub commands: usize,
}

/// Which revisions [`ProjectStore::history`] returns: walking parents from `to` (default: the
/// head), newest first, stopping before `stop_at`, at most `limit`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RevRange {
    /// Start here instead of at the head.
    pub to: Option<RevId>,
    /// Stop before this revision (exclusive).
    pub stop_at: Option<RevId>,
    /// At most this many.
    pub limit: Option<usize>,
}

impl RevRange {
    /// Everything reachable from the head.
    #[must_use]
    pub fn all() -> Self {
        Self::default()
    }

    /// The newest `n`.
    #[must_use]
    pub fn last(n: usize) -> Self {
        Self {
            limit: Some(n),
            ..Self::default()
        }
    }
}

/// An exclusive lock on one path (Ch.33.3: Perforce's value, for un-mergeable binaries).
/// While held, only its owner can write or delete the path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Lock {
    /// The path.
    pub path: StorePath,
    /// Who holds it (the store's identity when it was taken).
    pub owner: String,
}

/// A revision's files: path -> content address, sorted by path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    /// The entries.
    pub entries: Vec<(StorePath, Blake3)>,
}

impl Tree {
    /// The canonical text (`<hex> <path>\n` per entry, sorted) whose BLAKE3 is the tree's
    /// address. Identical bytes on every backend and platform.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.entries.len() * 80);
        for (p, h) in &self.entries {
            out.extend_from_slice(h.to_hex().as_bytes());
            out.push(b' ');
            out.extend_from_slice(p.as_str().as_bytes());
            out.push(b'\n');
        }
        out
    }

    /// Parse [`Tree::encode`]'s text.
    pub fn decode(bytes: &[u8]) -> Result<Self, StoreError> {
        let bad = |why: String| StoreError::Corrupt {
            what: "a tree".into(),
            why,
        };
        let text = std::str::from_utf8(bytes).map_err(|e| bad(e.to_string()))?;
        let mut entries = Vec::new();
        for line in text.lines() {
            let (h, p) = line
                .split_once(' ')
                .ok_or_else(|| bad(format!("line {line:?}")))?;
            let h: Blake3 = h.parse().map_err(|_| bad(format!("hash {h:?}")))?;
            entries.push((StorePath::new(p)?, h));
        }
        Ok(Self { entries })
    }
}

/// Project storage (I17). The local filesystem is one implementation, not a privileged one;
/// the editor and tools see only this trait.
pub trait ProjectStore: Send + Sync {
    /// The backend's name (`local-fs`, `memory`).
    fn backend(&self) -> &'static str;
    /// Who this store acts as (lock owner, commit author).
    fn identity(&self) -> &str;

    /// A working file's bytes.
    fn read(&self, path: &StorePath) -> Result<Bytes, StoreError>;
    /// Write a working file (atomically: a reader sees the old bytes or the new, never half).
    /// Refused if another identity holds its lock, or it collides by case with another file.
    fn write(&mut self, path: &StorePath, b: Bytes) -> Result<(), StoreError>;
    /// Delete a working file. Refused if another identity holds its lock.
    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError>;
    /// Every working file, sorted.
    fn list(&self) -> Result<Vec<StorePath>, StoreError>;

    /// A blob by address. The bytes are verified against the address.
    fn blob_get(&self, h: Blake3) -> Result<Bytes, StoreError>;
    /// Store a blob; returns its address (storing the same bytes twice stores them once).
    fn blob_put(&mut self, b: Bytes) -> Result<Blake3, StoreError>;
    /// Whether a blob is stored.
    fn blob_has(&self, h: Blake3) -> Result<bool, StoreError>;

    /// Commit the working files with the command envelopes that produced them.
    fn commit(&mut self, msg: &str, envelopes: &[CommandEnvelope]) -> Result<RevId, StoreError>;
    /// [`ProjectStore::commit`], recording `at_ms` as the commit time instead of the store's
    /// clock (additive, WP-16). Push, pull and clone replay revisions with it, so a copied
    /// revision keeps when it was made; the id is the same either way (time is metadata).
    /// The default ignores `at_ms`.
    fn commit_at(
        &mut self,
        msg: &str,
        envelopes: &[CommandEnvelope],
        at_ms: u64,
    ) -> Result<RevId, StoreError> {
        let _ = at_ms;
        self.commit(msg, envelopes)
    }
    /// Make committed revisions visible where the store lives (additive, WP-16). A store
    /// that commits in place (`LocalFs`, `MemoryStore`, a Git project folder) has nothing to
    /// do; a view of a remote (a Git remote) sends its new revisions, and changes nothing on
    /// the remote when the remote moved since it was read. A push calls it after replaying.
    fn publish(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
    /// The newest revision, if any.
    fn head(&self) -> Result<Option<RevId>, StoreError>;
    /// Revisions, newest first.
    fn history(&self, range: RevRange) -> Result<Vec<Rev>, StoreError>;
    /// A revision's files.
    fn tree(&self, rev: &RevId) -> Result<Tree, StoreError>;
    /// A revision's command envelopes.
    fn commands(&self, rev: &RevId) -> Result<Vec<CommandEnvelope>, StoreError>;

    /// Lock `path` for this store's identity (idempotent for the holder).
    fn lock(&mut self, path: &StorePath) -> Result<Lock, StoreError>;
    /// Release a lock this identity holds.
    fn unlock(&mut self, lock: &Lock) -> Result<(), StoreError>;
    /// Every held lock, sorted by path.
    fn locks(&self) -> Result<Vec<Lock>, StoreError>;

    /// Every working file with a cheap change [`Stamp`], sorted by path (additive, WP-12).
    ///
    /// Equal stamps for a path mean "unchanged since the last call": the asset system's hot
    /// reload polls this instead of reading every file. The default reads and hashes each
    /// file, which is correct for any backend; a backend overrides it with something cheaper
    /// (`LocalFs`: size and modification time; `MemoryStore`: the stored content hash).
    fn stamps(&self) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        let mut out = Vec::new();
        for p in self.list()? {
            let b = self.read(&p)?;
            out.push((p, Stamp::of_content(&Blake3::of(&b))));
        }
        Ok(out)
    }

    /// A working file's size in bytes, `None` if it is not a working file (additive, WP-12).
    ///
    /// Hot reload compares sizes before it reads anything when it looks for where a vanished
    /// source went. The default reads the file; a backend overrides it with its metadata.
    fn size(&self, path: &StorePath) -> Result<Option<u64>, StoreError> {
        match self.read(path) {
            Ok(b) => Ok(Some(b.len() as u64)),
            Err(StoreError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The stamps of just `paths` — the same stamps [`ProjectStore::stamps`] would give
    /// them — sorted by path; a path that is not a working file is absent (additive, WP-12).
    ///
    /// Hot reload re-stamps the few files it wrote itself during a poll with this, instead
    /// of walking the whole project a second time (which would also swallow any edit a user
    /// made meanwhile). The default filters `stamps()`; a backend overrides it with
    /// something that does not look at every file.
    fn stamps_of(&self, paths: &[StorePath]) -> Result<Vec<(StorePath, Stamp)>, StoreError> {
        let want: std::collections::BTreeSet<&StorePath> = paths.iter().collect();
        Ok(self
            .stamps()?
            .into_iter()
            .filter(|(p, _)| want.contains(p))
            .collect())
    }
}

/// A working file's change token (see [`ProjectStore::stamps`]). Opaque: compare, never
/// interpret. Stamps belong to one store instance and are not portable between backends.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Stamp(pub [u8; 16]);

impl Stamp {
    /// A stamp derived from a content address (exact: it changes iff the content does).
    #[must_use]
    pub fn of_content(h: &Blake3) -> Self {
        let mut s = [0u8; 16];
        s.copy_from_slice(&h.0[..16]);
        Self(s)
    }

    /// A stamp derived from backend metadata (size, modification time, ...).
    #[must_use]
    pub fn of_metadata(parts: &[u64]) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(b"forge-stamp-v1");
        for p in parts {
            h.update(&p.to_le_bytes());
        }
        let mut s = [0u8; 16];
        s.copy_from_slice(&h.finalize().as_bytes()[..16]);
        Self(s)
    }
}

// ---- the shared revision model -----------------------------------------------------------

/// The revision record every backend keeps (RON). Identical for every backend: a backend
/// plugin stores it however it likes, but builds it with [`build_commit`] and names it by
/// [`RevRecord::id`], so the same project has the same revision ids everywhere (I17).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevRecord {
    /// The parent revision's id (hex); `None` for the first.
    pub parent: Option<String>,
    /// The tree blob's address (hex): [`Tree::encode`]'s text.
    pub tree: String,
    /// The command log blob's address (hex): [`encode_log`]'s text.
    pub log: String,
    /// The commit message.
    pub message: String,
    /// When (metadata: not part of the id).
    pub at_ms: u64,
    /// The distinct issuers of its commands, first-seen order.
    pub issuers: Vec<String>,
    /// How many commands it carries.
    pub commands: usize,
}

impl RevRecord {
    /// The id: BLAKE3 over the content fields in a fixed text form (no timestamp).
    #[must_use]
    pub fn id(&self) -> RevId {
        let mut h = blake3::Hasher::new();
        h.update(b"forge-rev-v1\n");
        h.update(self.parent.as_deref().unwrap_or("-").as_bytes());
        h.update(b"\n");
        h.update(self.tree.as_bytes());
        h.update(b"\n");
        h.update(self.log.as_bytes());
        h.update(b"\n");
        h.update(self.message.as_bytes());
        RevId(Blake3(*h.finalize().as_bytes()))
    }

    /// The [`Rev`] this record describes under `id`.
    pub fn to_rev(&self, id: RevId) -> Result<Rev, StoreError> {
        let parse = |s: &str| {
            s.parse::<Blake3>().map_err(|_| StoreError::Corrupt {
                what: format!("revision {id}"),
                why: format!("bad address {s:?}"),
            })
        };
        Ok(Rev {
            id,
            parent: self.parent.as_deref().map(parse).transpose()?.map(RevId),
            tree: parse(&self.tree)?,
            log: parse(&self.log)?,
            message: self.message.clone(),
            at_ms: self.at_ms,
            issuers: self.issuers.clone(),
            commands: self.commands,
        })
    }

    /// The record's RON text.
    pub fn encode(&self) -> Result<String, StoreError> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).map_err(|e| {
            StoreError::Corrupt {
                what: "a revision record".into(),
                why: e.to_string(),
            }
        })
    }

    /// Parse [`RevRecord::encode`]'s text, checking it is the record `id` names.
    pub fn decode(text: &str, id: &RevId) -> Result<Self, StoreError> {
        let rec: Self = ron::from_str(text).map_err(|e| StoreError::Corrupt {
            what: format!("revision {id}"),
            why: e.to_string(),
        })?;
        if rec.id() != *id {
            return Err(StoreError::Corrupt {
                what: format!("revision {id}"),
                why: format!("its content hashes to {}", rec.id()),
            });
        }
        Ok(rec)
    }
}

/// Encode the command log (JSON lines: one envelope per line; the wire format of Ch.34).
pub fn encode_log(envelopes: &[CommandEnvelope]) -> Result<Vec<u8>, StoreError> {
    let mut out = Vec::new();
    for e in envelopes {
        serde_json::to_writer(&mut out, e).map_err(|x| StoreError::Corrupt {
            what: "a command envelope".into(),
            why: x.to_string(),
        })?;
        out.push(b'\n');
    }
    Ok(out)
}

/// Parse [`encode_log`]'s text.
pub fn decode_log(bytes: &[u8]) -> Result<Vec<CommandEnvelope>, StoreError> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| {
            serde_json::from_slice(l).map_err(|x| StoreError::Corrupt {
                what: "a command log".into(),
                why: x.to_string(),
            })
        })
        .collect()
}

/// Build a commit's record: `files` (their bytes already stored as blobs) sorted into the
/// tree, the tree and the log stored through `blob_put`, the issuers collected.
pub fn build_commit(
    files: Vec<(StorePath, Blake3)>,
    parent: Option<RevId>,
    msg: &str,
    envelopes: &[CommandEnvelope],
    at_ms: u64,
    blob_put: &mut dyn FnMut(Bytes) -> Result<Blake3, StoreError>,
) -> Result<RevRecord, StoreError> {
    let mut entries = files;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let tree = Tree { entries };
    let tree_h = blob_put(Bytes::from(tree.encode()))?;
    let log_h = blob_put(Bytes::from(encode_log(envelopes)?))?;
    let mut seen = BTreeSet::new();
    let issuers = envelopes
        .iter()
        .map(|e| e.issuer.tag())
        .filter(|t| seen.insert(t.clone()))
        .collect();
    Ok(RevRecord {
        parent: parent.map(|p| p.to_string()),
        tree: tree_h.to_hex(),
        log: log_h.to_hex(),
        message: msg.to_string(),
        at_ms,
        issuers,
        commands: envelopes.len(),
    })
}

/// Walk parents for [`ProjectStore::history`], given a record lookup.
pub fn walk_history(
    range: &RevRange,
    head: Option<RevId>,
    get: &dyn Fn(&RevId) -> Result<RevRecord, StoreError>,
) -> Result<Vec<Rev>, StoreError> {
    let mut out = Vec::new();
    let mut cur = range.to.or(head);
    let limit = range.limit.unwrap_or(usize::MAX);
    while let Some(id) = cur {
        if Some(id) == range.stop_at || out.len() >= limit {
            break;
        }
        let rec = get(&id)?;
        let rev = rec.to_rev(id)?;
        cur = rev.parent;
        out.push(rev);
    }
    Ok(out)
}

/// Every path and directory prefix, case-folded, with the spelling in use: an `O(depth)`
/// check that a new path does not differ from an existing file or directory only by case.
#[derive(Clone, Debug, Default)]
pub struct CaseIndex {
    map: std::collections::HashMap<String, (String, usize)>,
}

fn prefixes(path: &StorePath) -> impl Iterator<Item = &str> {
    let s = path.as_str();
    s.match_indices('/')
        .map(move |(i, _)| &s[..i])
        .chain(std::iter::once(s))
}

impl CaseIndex {
    /// The existing spelling `path` (or one of its directories) collides with.
    #[must_use]
    pub fn collision(&self, path: &StorePath) -> Option<String> {
        prefixes(path).find_map(|pre| {
            self.map
                .get(&pre.to_lowercase())
                .filter(|(actual, _)| actual != pre)
                .map(|(actual, _)| actual.clone())
        })
    }

    /// Record `path` and its directories.
    pub fn insert(&mut self, path: &StorePath) {
        for pre in prefixes(path) {
            let e = self
                .map
                .entry(pre.to_lowercase())
                .or_insert_with(|| (pre.to_string(), 0));
            e.1 += 1;
        }
    }

    /// Forget `path`; a directory goes with its last file.
    pub fn remove(&mut self, path: &StorePath) {
        for pre in prefixes(path) {
            let key = pre.to_lowercase();
            if let Some(e) = self.map.get_mut(&key) {
                e.1 -= 1;
                if e.1 == 0 {
                    self.map.remove(&key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> StorePath {
        StorePath::new(s).expect("valid")
    }

    #[test]
    fn trees_round_trip_and_hash_canonically() {
        let t = Tree {
            entries: vec![
                (p("a.ron"), Blake3::of(b"a")),
                (p("b/c.png"), Blake3::of(b"c")),
            ],
        };
        assert_eq!(Tree::decode(&t.encode()), Ok(t.clone()));
        assert!(Tree::decode(b"nothex a.ron\n").is_err());
    }

    #[test]
    fn a_rev_id_covers_content_not_time() {
        let rec = RevRecord {
            parent: None,
            tree: Blake3::of(b"t").to_hex(),
            log: Blake3::of(b"l").to_hex(),
            message: "m".into(),
            at_ms: 1,
            issuers: vec![],
            commands: 0,
        };
        let later = RevRecord {
            at_ms: 99,
            ..rec.clone()
        };
        assert_eq!(rec.id(), later.id());
        let other = RevRecord {
            message: "n".into(),
            ..rec.clone()
        };
        assert_ne!(rec.id(), other.id());
        let text = rec.encode().expect("encodes");
        assert_eq!(RevRecord::decode(&text, &rec.id()), Ok(rec.clone()));
        assert_eq!(
            RevRecord::decode(&text, &other.id()).map_err(|e| e.code().as_str()),
            Err("STORE-0004"),
            "a record under the wrong id is corrupt"
        );
    }

    #[test]
    fn case_collisions_are_found_on_files_and_directories() {
        let mut ix = CaseIndex::default();
        ix.insert(&p("Tex/rock.png"));
        ix.insert(&p("scene.ron"));
        assert_eq!(ix.collision(&p("SCENE.ron")).as_deref(), Some("scene.ron"));
        assert_eq!(ix.collision(&p("tex/new.png")).as_deref(), Some("Tex"));
        assert_eq!(ix.collision(&p("Tex/new.png")), None);
        assert_eq!(ix.collision(&p("scene.ron")), None, "itself");
        ix.remove(&p("Tex/rock.png"));
        assert_eq!(
            ix.collision(&p("tex/new.png")),
            None,
            "the directory is gone"
        );
    }
}

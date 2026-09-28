//! The blob index: a content address (BLAKE3, what Forge names a blob by) to the Git
//! object holding the bytes (SHA-1, what Git names it by), kept beside the repository in
//! `forge-index` as append-only lines — so a lookup is one hash-map probe, not a walk.
//!
//! It is a cache: every entry of a committed blob can be rebuilt from the history
//! ([`BlobIndex::index_revision`], [`BlobIndex::index_blobs_commit`]); a blob only handed to
//! `blob_put` (an import artefact, derivable) exists only here and in the object database.
//! A torn last line (a crash while appending) is skipped when read.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;

use forge_store::{Blake3, StoreError, Tree};
use gix::ObjectId;

use super::layout;
use super::repo::Repo;

/// What the index knows of a blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub oid: ObjectId,
    /// Whether its bytes are text ([`layout::is_text`]); `None`: not looked at yet (a blob a
    /// fetch brought on the blobs chain; decided, once, when a commit needs to know).
    pub text: Option<bool>,
    /// Reachable from a ref a push sends (a revision's tree, or the blobs chain). A blob that
    /// is not stays local: it is never pushed.
    pub shared: bool,
}

/// See the module docs.
pub(crate) struct BlobIndex {
    file: PathBuf,
    map: HashMap<Blake3, Entry>,
    /// Commits already indexed (main and blobs chain).
    seen: HashSet<ObjectId>,
    pending: String,
}

fn io(op: &'static str, p: &std::path::Path, e: &std::io::Error) -> StoreError {
    StoreError::Io {
        op,
        path: p.display().to_string(),
        why: e.to_string(),
    }
}

impl BlobIndex {
    /// Load the index beside `repo` (empty when there is none yet).
    pub(crate) fn load(repo: &Repo) -> Result<Self, StoreError> {
        let file = repo.git_dir().join("forge-index");
        let mut ix = Self {
            file,
            map: HashMap::new(),
            seen: HashSet::new(),
            pending: String::new(),
        };
        let text = match std::fs::read(&ix.file) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io("read", &ix.file, &e)),
        };
        for line in text.lines() {
            let mut f = line.split(' ');
            match (f.next(), f.next(), f.next(), f.next()) {
                (Some("b"), Some(h), Some(oid), Some(flags)) => {
                    if let (Ok(h), Ok(oid)) =
                        (h.parse::<Blake3>(), ObjectId::from_hex(oid.as_bytes()))
                    {
                        let e = Entry {
                            oid,
                            text: if flags.contains('t') {
                                Some(true)
                            } else if flags.contains('b') {
                                Some(false)
                            } else {
                                None
                            },
                            shared: flags.contains('s'),
                        };
                        let merged = match ix.map.get(&h) {
                            Some(old) => Entry {
                                oid: old.oid,
                                text: old.text.or(e.text),
                                shared: old.shared || e.shared,
                            },
                            None => e,
                        };
                        ix.map.insert(h, merged);
                    }
                }
                (Some("c"), Some(oid), None, None) => {
                    if let Ok(oid) = ObjectId::from_hex(oid.as_bytes()) {
                        ix.seen.insert(oid);
                    }
                }
                _ => {} // a torn or unknown line
            }
        }
        Ok(ix)
    }

    pub(crate) fn get(&self, h: &Blake3) -> Option<Entry> {
        self.map.get(h).copied()
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    /// Record a blob (a no-op when nothing new is learnt).
    pub(crate) fn add(&mut self, h: Blake3, e: Entry) {
        // The same address is the same bytes, hence the same Git object: only what is known
        // about it can grow.
        let merged = match self.map.get(&h) {
            Some(old) => Entry {
                oid: old.oid,
                text: old.text.or(e.text),
                shared: old.shared || e.shared,
            },
            None => e,
        };
        if self.map.get(&h) == Some(&merged) {
            return;
        }
        self.map.insert(h, merged);
        let flags = format!(
            "{}{}",
            match merged.text {
                Some(true) => "t",
                Some(false) => "b",
                None => "?",
            },
            if merged.shared { "s" } else { "-" }
        );
        self.pending
            .push_str(&format!("b {} {} {flags}\n", h.to_hex(), merged.oid));
    }

    /// Mark a known blob shared.
    pub(crate) fn share(&mut self, h: &Blake3) {
        if let Some(e) = self.map.get(h).copied()
            && !e.shared
        {
            self.add(*h, Entry { shared: true, ..e });
        }
    }

    pub(crate) fn seen(&self, commit: &ObjectId) -> bool {
        self.seen.contains(commit)
    }

    fn saw(&mut self, commit: ObjectId) {
        if self.seen.insert(commit) {
            self.pending.push_str(&format!("c {commit}\n"));
        }
    }

    /// Append what was learnt to the file.
    pub(crate) fn flush(&mut self) -> Result<(), StoreError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
            .map_err(|e| io("open", &self.file, &e))?;
        f.write_all(self.pending.as_bytes())
            .and_then(|()| f.sync_data())
            .map_err(|e| io("append to", &self.file, &e))?;
        self.pending.clear();
        Ok(())
    }

    /// Index one revision commit someone else made (a fetch): its metadata blobs and its text
    /// files, by the addresses its Forge tree gives them. Nothing is trusted: every read
    /// verifies the bytes against the address.
    pub(crate) fn index_revision(
        &mut self,
        repo: &Repo,
        commit: &ObjectId,
    ) -> Result<(), StoreError> {
        if self.seen(commit) {
            return Ok(());
        }
        let c = repo.commit(commit)?;
        let meta = layout::meta_of(repo, &c.tree)?.ok_or_else(|| StoreError::Corrupt {
            what: format!("git commit {commit}"),
            why: "it was not made by Forge (no .forge/ metadata)".into(),
        })?;
        let mut tree_text = Vec::new();
        for (i, oid) in meta.iter().enumerate() {
            let (_, bytes) = repo.read(oid)?;
            let h = Blake3::of(&bytes);
            self.add(
                h,
                Entry {
                    oid: *oid,
                    text: Some(layout::is_text(&bytes)),
                    shared: true,
                },
            );
            if i == 1 {
                tree_text = bytes;
            }
        }
        let forge_tree = Tree::decode(&tree_text)?;
        let files = layout::text_files(repo, &c.tree)?;
        for (p, h) in &forge_tree.entries {
            if let Some(oid) = files.get(p.as_str()) {
                self.add(
                    *h,
                    Entry {
                        oid: *oid,
                        text: Some(true),
                        shared: true,
                    },
                );
            }
        }
        self.saw(*commit);
        Ok(())
    }

    /// Index one commit of the blobs chain.
    pub(crate) fn index_blobs_commit(
        &mut self,
        repo: &Repo,
        commit: &ObjectId,
    ) -> Result<(), StoreError> {
        if self.seen(commit) {
            return Ok(());
        }
        let c = repo.commit(commit)?;
        for e in repo.tree(&c.tree)? {
            if let Some(h) = layout::blob_name(&e.name)
                && !e.is_tree
            {
                self.add(
                    h,
                    Entry {
                        oid: e.id,
                        text: None,
                        shared: true,
                    },
                );
            }
        }
        self.saw(*commit);
        Ok(())
    }

    /// Record a commit this store made itself.
    pub(crate) fn made(&mut self, commit: ObjectId) {
        self.saw(commit);
    }
}

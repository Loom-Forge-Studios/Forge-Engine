//! `forge-store` — project storage (Ch.33).
//!
//! **I17: [`ProjectStore`] is a trait; the local filesystem is one implementation of it.**
//! The editor, tools and the collaboration layer see only the trait. Backends are plugins
//! on the `StoreBackend` extension point ([`backend`]); the first-party ones (the plugin
//! `plugins/forge-store-backends`) are [`LocalFs`] (the default: a project is an ordinary
//! folder) and [`MemoryStore`] (in-memory, labelled as such). Git, S3 and SQL backends are later
//! peers of the same trait (`Unbuilt` until their milestones).
//!
//! * **Content addressing (Ch.33.2):** blobs are named by their [`Blake3`] hash, stored once,
//!   and verified on every read.
//! * **The command log is the history (Ch.33.4):** [`ProjectStore::commit`] takes the
//!   `CommandEnvelope`s that produced the change, so a revision reads "automation:sess-4 …",
//!   and its [`RevId`] is the hash of its content, identical on every backend.
//! * **Locks (Ch.33.3):** an exclusive lock per path; only its owner can write it.
//! * **Portable paths:** a [`StorePath`] means the same file on Windows and Linux, and a
//!   path differing from an existing one only by case is refused everywhere.
//!
//! ```
//! use bytes::Bytes;
//! use forge_store::{MemoryStore, ProjectStore, RevRange, StorePath};
//!
//! let mut store = MemoryStore::new("ada");
//! let scene = StorePath::new("scenes/main.ron")?;
//! store.write(&scene, Bytes::from_static(b"Scene(entities: [])"))?;
//! let rev = store.commit("first scene", &[])?;
//! assert_eq!(store.history(RevRange::all())?[0].id, rev);
//! assert_eq!(store.tree(&rev)?.entries[0].0, scene);
//! # Ok::<(), forge_store::StoreError>(())
//! ```

#![forbid(unsafe_code)]

pub mod backend;
mod error;
mod local;
mod memory;
pub mod parity;
mod path;
mod store;

pub use backend::{BackendDescriptor, OpenFn, StoreBackend, open_store};
pub use error::StoreError;
pub use local::LocalFs;
pub use memory::MemoryStore;
pub use path::{Blake3, StorePath};
pub use store::{Lock, ProjectStore, Rev, RevId, RevRange, Stamp, Tree};

/// The backend kit (additive, WP-16): the revision model every backend shares, public so a
/// backend plugin (`plugins/forge-store-backends`' Git store, a third party's S3 store)
/// builds revisions exactly as `LocalFs` and `MemoryStore` do: the same record, the same
/// tree and log encodings, the same ids (I17), with no private path into this crate (I16).
pub mod kit {
    pub use crate::store::{
        CaseIndex, RevRecord, build_commit, decode_log, encode_log, walk_history,
    };
}

// Re-exported so backends and callers name one buffer type.
pub use bytes::Bytes;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_store_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in StoreError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-store |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }
}

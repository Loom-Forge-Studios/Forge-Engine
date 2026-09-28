//! The licence the editor reads (Ch.38 §38.2, E-57, I21; DoD M5-25; gate `C-licence-backend`):
//! **the signed entitlement file, verified offline by `forge-licence`**, behind the editor's
//! [`EntitlementSource`] seam — the backend the labelled in-memory stand-in
//! (`MemoryEntitlement`, D-4) held the place of.
//!
//! * The file is where `forge-licence` looks for it (`FORGE_ENTITLEMENT`, else
//!   `entitlement.json` in the per-user config directory: [`forge_licence::entitlement_path`]),
//!   and its Ed25519 signature is checked against the public key embedded in the build. Nothing
//!   here touches the network ([`EntitlementSource::network_calls`] stays 0).
//! * No file, or a file that does not verify (forged, tampered, another signer's), is **no
//!   entitlement**: the licence panel says so and collaboration is off (Ch.38 rule 1), and
//!   everything else — opening, editing, saving, building, shipping — works (E-57: degrade,
//!   never lock). A rejected file's reason is kept for the panel ([`SignedFileEntitlement::rejected`]).
//! * Renewal is a new file: [`EntitlementSource::renew`] re-reads it from disk (activation writes
//!   it), and observers hear of a change.
//!
//! The editor maps `forge-licence`'s entitlement onto its own model field for field; the four
//! Ch.38 §38.2 rules are then evaluated locally by `forge_editor::collab::licence::evaluate`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use forge_editor::collab::licence::{Entitlement, EntitlementSource, SeatKind, Tier};
use forge_licence::crypto::PublicKey;
use forge_project::collab::Observer;

/// Why a present entitlement file was not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// It does not verify against the embedded key (forged, tampered, another signer's) or is
    /// not a signed entitlement.
    Unverified(forge_licence::VerifyError),
    /// It could not be read (permissions, I/O).
    Unreadable(std::io::ErrorKind),
}

/// What the last read of the file found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Loaded {
    entitlement: Option<Entitlement>,
    rejected: Option<Rejected>,
}

/// The signed entitlement file, verified offline (see the module docs).
pub struct SignedFileEntitlement {
    path: Option<PathBuf>,
    key: PublicKey,
    loaded: Mutex<Loaded>,
    generation: AtomicU64,
    observers: Mutex<Vec<Observer>>,
}

impl std::fmt::Debug for SignedFileEntitlement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedFileEntitlement")
            .field("path", &self.path)
            .field("loaded", &*self.lock())
            .finish_non_exhaustive()
    }
}

/// `forge-licence`'s entitlement in the editor's model (the same fields, Ch.38 §38.2).
#[must_use]
pub fn to_editor(e: &forge_licence::Entitlement) -> Entitlement {
    Entitlement {
        tier: match e.tier {
            forge_licence::Tier::Individual => Tier::Individual,
            forge_licence::Tier::Team { over_100k } => Tier::Team { over_100k },
        },
        seat: match e.seat {
            forge_licence::SeatKind::Purchaser => SeatKind::Purchaser,
            forge_licence::SeatKind::Included => SeatKind::Included,
            forge_licence::SeatKind::Additional => SeatKind::Additional,
        },
        bound: e.bound.clone(),
        expires_ms: e.expires_ms,
        fallback: e.fallback.clone(),
    }
}

/// Read and verify the file at `path` against `key`.
fn load(path: Option<&Path>, key: &PublicKey) -> Loaded {
    let Some(path) = path else {
        return Loaded::default();
    };
    match std::fs::read(path) {
        Ok(bytes) => match forge_licence::verify_signed(&bytes, key) {
            Ok(e) => Loaded {
                entitlement: Some(to_editor(&e)),
                rejected: None,
            },
            Err(e) => Loaded {
                entitlement: None,
                rejected: Some(Rejected::Unverified(e)),
            },
        },
        // No file: an un-activated machine, not an error.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::default(),
        Err(e) => Loaded {
            entitlement: None,
            rejected: Some(Rejected::Unreadable(e.kind())),
        },
    }
}

impl SignedFileEntitlement {
    /// The entitlement file `forge-licence` names for this machine (`config_dir`: the per-user
    /// config directory), verified against the key embedded in this build.
    #[must_use]
    pub fn open(config_dir: Option<&Path>) -> Self {
        Self::at(
            forge_licence::entitlement_path(config_dir),
            forge_licence::EMBEDDED_PUBLIC_KEY,
        )
    }

    /// The file at `path` (`None`: no file can exist), verified against `key` (tests; a server
    /// configured with its own key).
    #[must_use]
    pub fn at(path: Option<PathBuf>, key: PublicKey) -> Self {
        let loaded = load(path.as_deref(), &key);
        Self {
            path,
            key,
            loaded: Mutex::new(loaded),
            generation: AtomicU64::new(0),
            observers: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Loaded> {
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The file read: `None` when no file can exist here (no config directory, no override).
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Why a present file was not used, if one was not.
    #[must_use]
    pub fn rejected(&self) -> Option<Rejected> {
        self.lock().rejected
    }

    /// Read the file again (activation wrote a new one); true when what it holds changed, and
    /// then the generation moves and observers are told.
    pub fn reload(&self) -> bool {
        let fresh = load(self.path.as_deref(), &self.key);
        let changed = {
            let mut l = self.lock();
            let changed = *l != fresh;
            *l = fresh;
            changed
        };
        if changed {
            self.generation.fetch_add(1, Ordering::AcqRel);
            let list: Vec<Observer> = self
                .observers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            for f in list {
                f();
            }
        }
        changed
    }
}

impl EntitlementSource for SignedFileEntitlement {
    fn backend(&self) -> &'static str {
        forge_ui::tr!("forge-licence: the signed entitlement file, verified offline")
    }
    fn entitlement(&self) -> Option<Entitlement> {
        self.lock().entitlement.clone()
    }
    fn renew(&self) -> String {
        // Offline: a renewal is a new signed file (activation writes it); read it again.
        let changed = self.reload();
        match (self.entitlement(), self.rejected(), changed) {
            (_, Some(_), _) => forge_ui::tr!(
                "The entitlement file on this machine could not be verified: activate your licence again."
            )
            .to_string(),
            (Some(e), None, _) => match e.expires_ms {
                Some(ms) => forge_ui::trf!(
                    "Read the entitlement file again: valid until {date}.",
                    date = forge_editor::collab::licence::date(ms)
                ),
                None => forge_ui::tr!("Read the entitlement file again: a perpetual licence.")
                    .to_string(),
            },
            (None, None, _) => forge_ui::tr!(
                "There is no entitlement file on this machine: activate a licence to renew."
            )
            .to_string(),
        }
    }
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    fn network_calls(&self) -> u64 {
        0
    }
    fn observe(&self, f: Observer) {
        self.observers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(f);
    }
}

//! The `StoreBackend` extension point (Ch.32.2 seed list, Ch.33.1): backends are plugins.
//!
//! `LocalFs` and `MemoryStore` are registered by the first-party plugin `forge.store`
//! (`plugins/forge-store-backends`) through the ordinary loader — no backend is special-cased
//! (I17) and none registers privately (I16); a third-party plugin can add `s3`, replace
//! `file`, or chain it (a logging or caching store).

use std::sync::Arc;

use forge_plugin::{ExtensionPoint, Extensions};

use crate::{ProjectStore, StoreError};

/// Opens a store at a location (the part of the URL after `scheme:`) for an identity.
pub type OpenFn =
    Arc<dyn Fn(&str, &str) -> Result<Box<dyn ProjectStore>, StoreError> + Send + Sync>;

/// A store backend, registered under its URL scheme (`file`, `memory`).
#[derive(Clone)]
pub struct BackendDescriptor {
    /// Shown in the new-project dialog (Ch.33.5).
    pub label: String,
    /// Opens a store.
    pub open: OpenFn,
}

/// The `StoreBackend` point (`forge.store.backend`).
pub struct StoreBackend;

impl ExtensionPoint for StoreBackend {
    type Item = BackendDescriptor;
    const ID: &'static str = "forge.store.backend";
    const NAME: &'static str = "StoreBackend";
}

/// Open the store a URL names (`file:C:/Projects/demo`, `memory:scratch`) through whatever
/// backend is registered for its scheme.
pub fn open_store(
    ext: &Extensions,
    url: &str,
    identity: &str,
) -> Result<Box<dyn ProjectStore>, StoreError> {
    let (scheme, loc) = url
        .split_once(':')
        .ok_or_else(|| StoreError::UnknownBackend(url.to_string()))?;
    let backend = ext
        .registry::<StoreBackend>()
        .and_then(|r| r.get(scheme))
        .ok_or_else(|| StoreError::UnknownBackend(scheme.to_string()))?;
    (backend.open)(loc, identity)
}

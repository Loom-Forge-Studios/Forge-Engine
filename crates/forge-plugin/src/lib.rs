//! `forge-plugin` — the kernel boundary: extension points, plugins, capabilities (Ch.32).
//!
//! **"Any feature is modifiable"** has one honest implementation (I16): the engine is a
//! small kernel plus plugins, and first-party subsystems register through the same
//! extension points, with the same operations, as any third-party plugin.
//!
//! * An [`ExtensionPoint`] names a kind of thing (`EditorPanel`, `Command`, `StoreBackend`);
//!   its [`Registry`] is ordered, keyed and **replaceable**: [`Registry::add`],
//!   [`Registry::replace`], [`Registry::remove`], and [`Registry::chain`] (wrap the built-in
//!   instead of discarding it — the middleware case, which keeps plugins composable).
//! * [`Extensions`] holds one registry per defined point.
//! * A plugin declares everything it does in its [`Manifest`] (`plugin.ron`). The
//!   [`loader`] finds every conflict from the manifests alone — two plugins replacing one
//!   item is a load error naming both, never a silent last-wins — then applies every
//!   plugin's operations in a fixed order.
//! * [`Capability`] is the one grant model shared by plugins, automation sessions and remote
//!   sessions (E-27). Nothing is granted by default.
//!
//! ```
//! use forge_plugin::{ExtensionPoint, ItemId, Order, PluginId, Registry};
//!
//! struct Greeter;
//! impl ExtensionPoint for Greeter {
//!     type Item = Box<dyn Fn() -> String>;
//!     const ID: &'static str = "example.greeter";
//!     const NAME: &'static str = "Greeter";
//! }
//!
//! let core = PluginId::new("forge.core")?;
//! let fancy = PluginId::new("com.example.fancy")?;
//! let mut greeters = Registry::<Greeter>::new();
//! let hello = greeters.add(core, "hello", Box::new(|| "hello".to_string()), Order::Last)?;
//!
//! // Wrap the built-in rather than replace it: the original still runs, inside.
//! greeters.chain(&fancy, &hello, |inner| Box::new(move || format!("** {} **", inner())))?;
//! assert_eq!(greeters.get("hello").map(|g| g()), Some("** hello **".to_string()));
//! # Ok::<(), forge_plugin::PluginError>(())
//! ```

#![forbid(unsafe_code)]

mod capability;
pub mod conformance;
mod error;
mod extensions;
mod id;
pub mod loader;
mod manifest;
pub mod points;
mod registry;

pub use capability::{
    Capability, CommandClass, DESTRUCTIVE_TARGETS, DefaultPolicy, FsScope, GpuUse, Grants, NetUse,
    Principal, PrincipalKind, SharedGrants,
};
pub use error::PluginError;
pub use extensions::{Extensions, PointInfo};
pub use id::{ItemId, PluginId, check_key};
pub use loader::{HostedPlugin, InstallCx, KERNEL_VERSION, LoadReport, SourcePlugin};
pub use manifest::{ItemRef, Manifest, PluginKind};
pub use registry::{ExtensionPoint, Order, Provenance, Registry, Removed, Replaced};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_plugin_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in PluginError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-plugin |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }

    #[test]
    fn the_kernel_version_parses() {
        assert!(semver::Version::parse(KERNEL_VERSION).is_ok());
    }
}

//! `forge-wasm` — the WASM component plugin host (Ch.32.3, M2-13).
//!
//! A WASM plugin is a directory: `plugin.ron` (the Ch.32.4 manifest, `kind: Wasm`) and
//! `plugin.wasm` (a WebAssembly component; `plugin.wat` text also loads). It is dropped in,
//! never compiled against the engine, sandboxed, and capability-scoped:
//!
//! * **The world** ([`WIT`]): the plugin exports one function, `call(point, key, input)`,
//!   that serves every item its manifest declares, and imports only `forge:plugin`
//!   interfaces. There is no WASI: no files, clock, sockets or environment unless an
//!   interface grants it (`WASM-0003`).
//! * **Capabilities, twice.** An interface that needs a capability links only if the
//!   manifest requests it (`WASM-0002`), and every call through it is checked against the
//!   **shared** grant table ([`forge_plugin::SharedGrants`], E-27) at that moment: a revoke
//!   stops the plugin's next call, and denials are recorded ([`HostEvent::Denied`]).
//! * **One loader.** A [`WasmPlugin`] is a [`forge_plugin::HostedPlugin`]: it installs
//!   through [`forge_plugin::loader::load_hosted`] alongside source plugins — same manifest
//!   checks, same conflict detection (two plugins replacing one item is a load error naming
//!   both), same install order. Items reach consumers through the ordinary registries.
//! * **Adapters** turn guest items into point items ([`WasmHost::adapt`]); `Command` and
//!   `Preset` are built in (see [`builtin`] for their encodings); `Importer` is
//!   [`importer::adapt_importers`] (WP-21). A WASM command *plans* and
//!   the bus applies, so it is undoable and provenance-tagged like any command (I7, I8).
//! * **Bounded.** Every call runs under a fuel budget and a memory cap ([`Limits`]); a
//!   trap discards the instance and the next call starts fresh (`WASM-0005`).
//! * **Hot reload** ([`PluginWatcher`]): changed code swaps in place under the installed
//!   items; a broken save keeps the old code running; a changed manifest asks for a reload
//!   through the loader.
//!
//! ```
//! use forge_plugin::points::{Command, install_commands};
//! use forge_plugin::{Capability, CommandClass, Extensions, Manifest, Principal, SharedGrants, loader};
//! use forge_cmd::{Bus, CommandSink, EditorCommand, Issuer, Value};
//! use forge_wasm::{WasmHost, wat};
//!
//! let manifest = Manifest::parse(r#"Plugin(id: "com.example.flag", version: "0.1.0",
//!     engine: "^0.1", kind: Wasm, provides: [Command("example.flag")])"#)?;
//! let code = wat::constant(br#"{"ops":[{"op":"set_setting","key":"flag","value":{"Bool":true}}]}"#);
//!
//! let grants = SharedGrants::new();
//! let host = WasmHost::new(grants.clone())?;
//! let plugin = host.load(manifest, code.as_bytes())?;
//!
//! let mut ext = Extensions::new();
//! ext.define::<Command>()?;
//! loader::load_hosted(&mut ext, &[], &[&plugin], &[], &grants.snapshot())?;
//! let mut bus = Bus::new();
//! install_commands(ext.registry::<Command>().expect("defined"), &mut bus)?;
//!
//! // A human grants the plugin ordinary commands; then its command plans and the bus applies.
//! grants.grant(Principal::Plugin(plugin.id().clone()), Capability::Command(CommandClass::Ordinary));
//! let run = EditorCommand::Invoke { target: "example.flag".into(), args: "{}".into() };
//! let e = bus.envelope(Issuer::Human { user: "ada".into() }, run);
//! bus.apply(e).map_err(|r| r.error)?;
//! assert_eq!(bus.project().setting("flag"), Some(&Value::Bool(true)));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

mod adapter;
pub mod builtin;
mod error;
mod files;
mod host;
pub mod importer;
mod plugin;
pub mod wat;
mod watch;

pub use adapter::{MakeFn, WrapFn};
pub use error::WasmError;
pub use files::PluginFiles;
pub use host::{
    EXPORT, HOST_INTERFACE, HostEvent, INTERFACES, Limits, PROJECT_READ_INTERFACE, ReadFn, WIT,
    WasmHost, code_file,
};
pub use plugin::{GuestItem, WasmPlugin};
#[doc(hidden)]
pub use watch::WatchFaults;
pub use watch::{PluginWatcher, ReloadEvent};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wasm_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in WasmError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-wasm |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }

    #[test]
    fn the_wit_names_every_linked_interface() {
        for (name, _) in INTERFACES {
            let iface = name
                .trim_start_matches("forge:plugin/")
                .trim_end_matches("@0.1.0");
            assert!(WIT.contains(&format!("interface {iface} {{")), "{name}");
        }
        assert!(WIT.contains("export call: func(point: string, key: string, input: list<u8>) -> result<list<u8>, string>;"));
    }
}

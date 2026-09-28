//! `WasmError` — the `WASM-*` codes (`docs/error-codes.md`).

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};
use forge_plugin::PluginError;

/// Every way hosting a WASM plugin can fail. Each names the plugin, so a user with twenty
/// plugins installed knows which one to disable.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WasmError {
    /// `WASM-0001`: the plugin's code is not a valid WebAssembly component (or WAT text).
    Compile {
        /// The plugin.
        plugin: String,
        /// The compiler's message.
        why: String,
    },
    /// `WASM-0002`: the component imports a host interface whose capability its manifest
    /// does not request. Capabilities are requested in the manifest, where the plugin
    /// manager shows them, or not at all.
    UndeclaredImport {
        /// The plugin.
        plugin: String,
        /// The imported interface.
        import: String,
        /// The capability it needs.
        capability: String,
    },
    /// `WASM-0003`: the component imports something this host does not provide (WASI, a
    /// socket, a clock): the sandbox has no ambient authority to link it to.
    UnknownImport {
        /// The plugin.
        plugin: String,
        /// The import's name.
        import: String,
    },
    /// `WASM-0004`: the component does not export `call` with the `forge:plugin` signature.
    MissingExport {
        /// The plugin.
        plugin: String,
        /// Why.
        why: String,
    },
    /// `WASM-0005`: the guest trapped (a bug, its fuel budget or its memory limit). The host
    /// re-instantiates it for the next call; nothing it did in the failed call is kept.
    Trap {
        /// The plugin.
        plugin: String,
        /// The trap.
        why: String,
    },
    /// `WASM-0006`: the guest returned an error for a call.
    Guest {
        /// The plugin.
        plugin: String,
        /// The item (`Point("key")`).
        item: String,
        /// The guest's message.
        why: String,
    },
    /// `WASM-0007`: a WASM plugin declares an operation on an extension point this host has
    /// no adapter for (or the adapter does not support that operation, e.g. `chains`).
    NoAdapter {
        /// The plugin.
        plugin: String,
        /// The point's manifest name.
        point: String,
        /// The operation (`provides`, `replaces`, `removes`, `chains`).
        op: &'static str,
    },
    /// `WASM-0008`: a plugin file could not be read.
    Io {
        /// The path.
        path: String,
        /// The OS error.
        why: String,
    },
    /// `WASM-0009`: a manifest given to the WASM host says `kind: Source`.
    NotWasm(String),
    /// `WASM-0010`: the guest's output for an item does not decode as that point expects.
    BadOutput {
        /// The plugin.
        plugin: String,
        /// The item.
        item: String,
        /// Why.
        why: String,
    },
    /// `WASM-0011`: a hot reload found the plugin's manifest changed (what it provides,
    /// replaces, removes, chains or requests). Code swaps in place; declarations do not — the
    /// host reloads the plugin set through the loader so conflicts are checked again.
    ManifestChanged {
        /// The plugin.
        plugin: String,
    },
    /// `WASM-0012`: a plugin-layer error (the `PLUGIN-*` code follows).
    Plugin(PluginError),
}

impl WasmError {
    /// The stable error code (`docs/error-codes.md`).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Compile { .. } => error_code!("WASM-0001"),
            Self::UndeclaredImport { .. } => error_code!("WASM-0002"),
            Self::UnknownImport { .. } => error_code!("WASM-0003"),
            Self::MissingExport { .. } => error_code!("WASM-0004"),
            Self::Trap { .. } => error_code!("WASM-0005"),
            Self::Guest { .. } => error_code!("WASM-0006"),
            Self::NoAdapter { .. } => error_code!("WASM-0007"),
            Self::Io { .. } => error_code!("WASM-0008"),
            Self::NotWasm(_) => error_code!("WASM-0009"),
            Self::BadOutput { .. } => error_code!("WASM-0010"),
            Self::ManifestChanged { .. } => error_code!("WASM-0011"),
            Self::Plugin(_) => error_code!("WASM-0012"),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<WasmError> {
        let s = || "x".to_string();
        vec![
            Self::Compile {
                plugin: s(),
                why: s(),
            },
            Self::UndeclaredImport {
                plugin: s(),
                import: s(),
                capability: s(),
            },
            Self::UnknownImport {
                plugin: s(),
                import: s(),
            },
            Self::MissingExport {
                plugin: s(),
                why: s(),
            },
            Self::Trap {
                plugin: s(),
                why: s(),
            },
            Self::Guest {
                plugin: s(),
                item: s(),
                why: s(),
            },
            Self::NoAdapter {
                plugin: s(),
                point: s(),
                op: "provides",
            },
            Self::Io {
                path: s(),
                why: s(),
            },
            Self::NotWasm(s()),
            Self::BadOutput {
                plugin: s(),
                item: s(),
                why: s(),
            },
            Self::ManifestChanged { plugin: s() },
            Self::Plugin(PluginError::BadPluginId(s())),
        ]
    }

    /// As a plugin-layer error, for the loader (`PLUGIN-0016` carrying this one).
    #[must_use]
    pub fn into_plugin_error(self, plugin: &str) -> PluginError {
        match self {
            Self::Plugin(e) => e,
            other => PluginError::Hosted {
                plugin: plugin.to_string(),
                why: other.to_string(),
            },
        }
    }
}

impl From<PluginError> for WasmError {
    fn from(e: PluginError) -> Self {
        Self::Plugin(e)
    }
}

impl fmt::Display for WasmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::Compile { plugin, why } => {
                write!(f, "`{plugin}` is not a valid WebAssembly component: {why}")
            }
            Self::UndeclaredImport {
                plugin,
                import,
                capability,
            } => write!(
                f,
                "`{plugin}` imports {import}, which needs {capability}, and its manifest does not request {capability}"
            ),
            Self::UnknownImport { plugin, import } => write!(
                f,
                "`{plugin}` imports {import}, which the Forge plugin host does not provide (plugins get only the forge:plugin interfaces)"
            ),
            Self::MissingExport { plugin, why } => write!(
                f,
                "`{plugin}` does not export `call: func(point: string, key: string, input: list<u8>) -> result<list<u8>, string>`: {why}"
            ),
            Self::Trap { plugin, why } => write!(f, "`{plugin}` trapped: {why}"),
            Self::Guest { plugin, item, why } => {
                write!(f, "`{plugin}` returned an error for {item}: {why}")
            }
            Self::NoAdapter { plugin, point, op } => write!(
                f,
                "`{plugin}` {op} a {point} item, and this host cannot run {point} items from WASM"
            ),
            Self::Io { path, why } => write!(f, "{path}: {why}"),
            Self::NotWasm(p) => write!(
                f,
                "`{p}` says `kind: Source`; the WASM host runs only `kind: Wasm` plugins"
            ),
            Self::BadOutput { plugin, item, why } => {
                write!(
                    f,
                    "`{plugin}` returned output for {item} that does not decode: {why}"
                )
            }
            Self::ManifestChanged { plugin } => write!(
                f,
                "`{plugin}`'s manifest changed; reload the plugin set so its declarations are checked again"
            ),
            Self::Plugin(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WasmError {}

impl CodedError for WasmError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}

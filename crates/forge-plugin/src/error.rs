//! `PluginError` — the `PLUGIN-*` codes (`docs/error-codes.md`).

use std::fmt;

use forge_core::{CodedError, ErrorCode, error_code};

/// Every way plugin loading or a registry operation can fail. Converts into
/// `forge_core::Error` with `?`. Conflicts always name **both** parties: a load never
/// resolves a conflict silently by load order (Ch.32.4).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginError {
    /// `PLUGIN-0001`: a plugin id is not lowercase dot-separated segments (`com.example.rivers`).
    BadPluginId(String),
    /// `PLUGIN-0002`: an item key is empty, too long, or has a character outside
    /// `A-Z a-z 0-9 _ - . :`.
    BadItemKey(String),
    /// `PLUGIN-0003`: two providers claim the same item.
    DuplicateItem {
        /// The item (`Point("key")`).
        item: String,
        /// The plugin that provided it first.
        first: String,
        /// The plugin that tried to provide it again.
        second: String,
    },
    /// `PLUGIN-0004`: no item has this key at this extension point (never added, or removed).
    UnknownItem(String),
    /// `PLUGIN-0005`: an item id names a different extension point than the registry's.
    WrongPoint {
        /// The item.
        item: String,
        /// The registry's point id.
        expected: &'static str,
    },
    /// `PLUGIN-0006`: two plugins modify the same item incompatibly (both replace it, or one
    /// replaces and the other removes it). A load error, never a silent last-wins.
    Conflict {
        /// The item.
        item: String,
        /// The first plugin and what it does (`replaces` / `removes`).
        first: String,
        /// The first plugin's operation.
        first_op: &'static str,
        /// The second plugin.
        second: String,
        /// The second plugin's operation.
        second_op: &'static str,
    },
    /// `PLUGIN-0007`: a manifest does not parse or is invalid.
    Manifest {
        /// The plugin id, when it could be read.
        plugin: Option<String>,
        /// Why.
        why: String,
    },
    /// `PLUGIN-0008`: the plugin requires a kernel version this engine is not.
    EngineMismatch {
        /// The plugin.
        plugin: String,
        /// Its `engine` requirement.
        requires: String,
        /// This kernel's version.
        kernel: String,
    },
    /// `PLUGIN-0009`: two loaded plugins share an id.
    DuplicatePlugin(String),
    /// `PLUGIN-0010`: a manifest or an install names an extension point this engine does not
    /// define.
    UnknownPoint {
        /// The plugin.
        plugin: String,
        /// The point's manifest name or id.
        point: String,
    },
    /// `PLUGIN-0011`: a plugin's install did something its manifest does not declare (every
    /// `provides` / `replaces` / `removes` / `chains` is declared, so conflicts are known
    /// before any plugin code runs).
    Undeclared {
        /// The plugin.
        plugin: String,
        /// The operation.
        op: &'static str,
        /// The item.
        item: String,
    },
    /// `PLUGIN-0012`: two different item types claim one extension point id.
    PointIdTaken(&'static str),
    /// `PLUGIN-0013`: plugin code panicked during install or inside a `chain` wrapper. The
    /// loader caught it; a wrapper's target item is gone (it was moved into the wrapper).
    Panicked {
        /// The plugin.
        plugin: String,
        /// What it was doing.
        during: String,
        /// The panic message.
        message: String,
    },
    /// `PLUGIN-0014`: a WASM plugin was asked to install and the load was given no sandbox
    /// host to run it (`loader::load`; `forge-wasm` hosts WASM plugins through
    /// `loader::load_hosted`). Its manifest still parses and takes part in conflict checks.
    WasmHostUnbuilt(String),
    /// `PLUGIN-0015`: a principal used a capability it was not granted.
    CapabilityDenied {
        /// Who (`plugin:com.example.rivers`, `automation:<session>`, `remote:<session>`).
        principal: String,
        /// The capability (`Net(Outbound)`).
        capability: String,
    },
    /// `PLUGIN-0016`: a hosted (sandboxed WASM) plugin could not install: its host refused
    /// it. The host's own error follows (a `WASM-*` code).
    Hosted {
        /// The plugin.
        plugin: String,
        /// The host's error.
        why: String,
    },
}

impl PluginError {
    /// The stable error code (`docs/error-codes.md`).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::BadPluginId(_) => error_code!("PLUGIN-0001"),
            Self::BadItemKey(_) => error_code!("PLUGIN-0002"),
            Self::DuplicateItem { .. } => error_code!("PLUGIN-0003"),
            Self::UnknownItem(_) => error_code!("PLUGIN-0004"),
            Self::WrongPoint { .. } => error_code!("PLUGIN-0005"),
            Self::Conflict { .. } => error_code!("PLUGIN-0006"),
            Self::Manifest { .. } => error_code!("PLUGIN-0007"),
            Self::EngineMismatch { .. } => error_code!("PLUGIN-0008"),
            Self::DuplicatePlugin(_) => error_code!("PLUGIN-0009"),
            Self::UnknownPoint { .. } => error_code!("PLUGIN-0010"),
            Self::Undeclared { .. } => error_code!("PLUGIN-0011"),
            Self::PointIdTaken(_) => error_code!("PLUGIN-0012"),
            Self::Panicked { .. } => error_code!("PLUGIN-0013"),
            Self::WasmHostUnbuilt(_) => error_code!("PLUGIN-0014"),
            Self::CapabilityDenied { .. } => error_code!("PLUGIN-0015"),
            Self::Hosted { .. } => error_code!("PLUGIN-0016"),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<PluginError> {
        let s = || "x".to_string();
        vec![
            Self::BadPluginId(s()),
            Self::BadItemKey(s()),
            Self::DuplicateItem {
                item: s(),
                first: s(),
                second: s(),
            },
            Self::UnknownItem(s()),
            Self::WrongPoint {
                item: s(),
                expected: "p",
            },
            Self::Conflict {
                item: s(),
                first: s(),
                first_op: "replaces",
                second: s(),
                second_op: "replaces",
            },
            Self::Manifest {
                plugin: None,
                why: s(),
            },
            Self::EngineMismatch {
                plugin: s(),
                requires: s(),
                kernel: s(),
            },
            Self::DuplicatePlugin(s()),
            Self::UnknownPoint {
                plugin: s(),
                point: s(),
            },
            Self::Undeclared {
                plugin: s(),
                op: "replace",
                item: s(),
            },
            Self::PointIdTaken("p"),
            Self::Panicked {
                plugin: s(),
                during: s(),
                message: s(),
            },
            Self::WasmHostUnbuilt(s()),
            Self::CapabilityDenied {
                principal: s(),
                capability: s(),
            },
            Self::Hosted {
                plugin: s(),
                why: s(),
            },
        ]
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::BadPluginId(id) => write!(
                f,
                "{id:?} is not a plugin id (lowercase dot-separated segments, e.g. com.example.rivers)"
            ),
            Self::BadItemKey(k) => {
                write!(f, "{k:?} is not an item key (1-128 of A-Z a-z 0-9 _ - . :)")
            }
            Self::DuplicateItem {
                item,
                first,
                second,
            } => write!(
                f,
                "{item} is provided by both `{first}` and `{second}`; one of them must provide it under another key or replace it"
            ),
            Self::UnknownItem(item) => write!(f, "{item} does not exist"),
            Self::WrongPoint { item, expected } => {
                write!(f, "{item} is not an item of extension point {expected}")
            }
            Self::Conflict {
                item,
                first,
                first_op,
                second,
                second_op,
            } => write!(
                f,
                "conflicting plugins: `{first}` {first_op} {item} and `{second}` {second_op} it; disable one of them"
            ),
            Self::Manifest { plugin, why } => match plugin {
                Some(p) => write!(f, "manifest of `{p}` is invalid: {why}"),
                None => write!(f, "manifest is invalid: {why}"),
            },
            Self::EngineMismatch {
                plugin,
                requires,
                kernel,
            } => write!(
                f,
                "`{plugin}` requires engine {requires}, this kernel is {kernel}"
            ),
            Self::DuplicatePlugin(p) => write!(f, "two plugins are both `{p}`"),
            Self::UnknownPoint { plugin, point } => write!(
                f,
                "`{plugin}` names extension point {point:?}, which this engine does not define"
            ),
            Self::Undeclared { plugin, op, item } => write!(
                f,
                "`{plugin}` tried to {op} {item} without declaring it in its manifest"
            ),
            Self::PointIdTaken(id) => write!(
                f,
                "extension point id {id:?} is already defined with a different item type"
            ),
            Self::Panicked {
                plugin,
                during,
                message,
            } => write!(f, "`{plugin}` panicked during {during}: {message}"),
            Self::WasmHostUnbuilt(p) => write!(
                f,
                "`{p}` is a WASM plugin and this load was given no WASM host to run it (load it through forge-wasm)"
            ),
            Self::CapabilityDenied {
                principal,
                capability,
            } => write!(f, "{principal} has not been granted {capability}"),
            Self::Hosted { plugin, why } => {
                write!(f, "hosted plugin `{plugin}` could not install: {why}")
            }
        }
    }
}

impl std::error::Error for PluginError {}

impl CodedError for PluginError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}

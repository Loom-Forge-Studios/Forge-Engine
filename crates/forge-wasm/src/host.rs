//! The host: one wasmtime engine, one linker holding every `forge:plugin` interface, the
//! shared grant table, and the per-call limits.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use forge_plugin::{
    Capability, ExtensionPoint, FsScope, Manifest, PluginId, PluginKind, Principal, SharedGrants,
};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, StoreContextMut, StoreLimits};

use crate::adapter::{Adapters, PointAdapter, Typed};
use crate::files::PluginFiles;
use crate::plugin::{GuestItem, Slot, WasmPlugin};
use crate::{WasmError, builtin};

/// The always-linked interface (`log`).
pub const HOST_INTERFACE: &str = "forge:plugin/host@0.1.0";
/// `read` a project file; needs `Fs(ProjectRead)`.
pub const PROJECT_READ_INTERFACE: &str = "forge:plugin/project-read@0.1.0";
/// The one export every plugin has.
pub const EXPORT: &str = "call";
/// The world, as WIT (`crates/forge-wasm/wit/forge-plugin.wit`).
pub const WIT: &str = include_str!("../wit/forge-plugin.wit");

/// Every interface the host links, and the capability a plugin needs to import it. Nothing
/// else is linkable: no WASI, no clocks, no sockets (WASM-0003).
pub const INTERFACES: &[(&str, Option<Capability>)] = &[
    (HOST_INTERFACE, None),
    (
        PROJECT_READ_INTERFACE,
        Some(Capability::Fs(FsScope::ProjectRead)),
    ),
];

/// Reads a project file by project path (the host wires it to its `ProjectStore`, I17).
pub type ReadFn = Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>;

/// What one guest call may use. A plugin that exceeds either traps (`WASM-0005`), and the
/// editor carries on: a runaway or greedy plugin cannot hang or exhaust it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// WebAssembly fuel per call (about one unit per instruction).
    pub fuel_per_call: u64,
    /// Linear memory, bytes, per instance.
    pub memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel_per_call: 2_000_000_000,
            memory_bytes: 256 << 20,
        }
    }
}

/// Something the host observed, for the plugin manager and the audit trail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// The plugin logged a line.
    Log {
        /// The plugin.
        plugin: PluginId,
        /// The line (truncated to 4 KiB).
        line: String,
    },
    /// A call used a capability the plugin does not hold right now. The guest got an error.
    Denied {
        /// The plugin.
        plugin: PluginId,
        /// The capability.
        capability: Capability,
        /// What it tried.
        what: String,
    },
    /// A call trapped; the instance was discarded.
    Trapped {
        /// The plugin.
        plugin: PluginId,
        /// The trap.
        why: String,
    },
    /// New code replaced the plugin's code in place (hot reload).
    Reloaded {
        /// The plugin.
        plugin: PluginId,
        /// The new generation.
        generation: u64,
    },
}

const MAX_EVENTS: usize = 1024;
const MAX_LINE: usize = 4096;

/// A bounded event log shared by the host and its guests.
#[derive(Clone, Default)]
pub(crate) struct EventLog(Arc<Mutex<VecDeque<HostEvent>>>);

impl EventLog {
    pub(crate) fn push(&self, e: HostEvent) {
        if let Ok(mut q) = self.0.lock() {
            if q.len() == MAX_EVENTS {
                q.pop_front();
            }
            q.push_back(e);
        }
    }

    fn drain(&self) -> Vec<HostEvent> {
        self.0
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }
}

/// A guest store's data: who it is, and the host services it may reach.
pub(crate) struct GuestState {
    pub(crate) plugin: PluginId,
    pub(crate) who: Principal,
    pub(crate) grants: SharedGrants,
    pub(crate) reader: Arc<RwLock<Option<ReadFn>>>,
    pub(crate) events: EventLog,
    pub(crate) limits: StoreLimits,
}

impl GuestState {
    /// Like [`GuestState::allowed`], but `what` is built by a closure that runs only
    /// on the denial path, so allowed calls never allocate the string.
    fn allowed_lazy<F: FnOnce() -> String>(
        &self,
        cap: Capability,
        what_builder: F,
    ) -> Result<(), String> {
        match self.grants.check(&self.who, cap) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.events.push(HostEvent::Denied {
                    plugin: self.plugin.clone(),
                    capability: cap,
                    what: what_builder(),
                });
                Err(e.to_string())
            }
        }
    }
}

/// What the plugins of one host share.
pub(crate) struct HostInner {
    pub(crate) engine: Engine,
    pub(crate) linker: Linker<GuestState>,
    pub(crate) grants: SharedGrants,
    pub(crate) reader: Arc<RwLock<Option<ReadFn>>>,
    pub(crate) events: EventLog,
    pub(crate) limits: Limits,
}

/// The WASM component plugin host (Ch.32.3). Plugins it loads are [`HostedPlugin`]s: they
/// install through [`forge_plugin::loader::load_hosted`], the same loader, manifest checks,
/// conflict checks and install order as every source plugin.
///
/// Capabilities are scoped twice: an import is linkable only if the manifest requests its
/// capability (checked at load, `WASM-0002`), and each call through it is checked against
/// the shared grant table at that moment (a denial returns an error to the guest).
///
/// [`HostedPlugin`]: forge_plugin::HostedPlugin
pub struct WasmHost {
    inner: Arc<HostInner>,
    adapters: Adapters,
}

impl std::fmt::Debug for WasmHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmHost")
            .field("limits", &self.inner.limits)
            .field("points", &self.adapters.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

fn wasm_err(plugin: &str) -> impl Fn(wasmtime::Error) -> WasmError + '_ {
    move |e| WasmError::Compile {
        plugin: plugin.to_string(),
        why: format!("{e:#}"),
    }
}

impl WasmHost {
    /// A host over `grants` (shared with every other holder of the table), with the default
    /// [`Limits`] and the built-in adapters (`Command`, `Preset`).
    pub fn new(grants: SharedGrants) -> Result<Self, WasmError> {
        Self::with_limits(grants, Limits::default())
    }

    /// A host with explicit limits.
    pub fn with_limits(grants: SharedGrants, limits: Limits) -> Result<Self, WasmError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(wasm_err("forge-wasm"))?;
        let mut linker = Linker::<GuestState>::new(&engine);
        link(&mut linker).map_err(wasm_err("forge-wasm"))?;
        let mut host = Self {
            inner: Arc::new(HostInner {
                engine,
                linker,
                grants,
                reader: Arc::new(RwLock::new(None)),
                events: EventLog::default(),
                limits,
            }),
            adapters: BTreeMap::new(),
        };
        builtin::install(&mut host);
        Ok(host)
    }

    /// The grant table this host checks.
    #[must_use]
    pub fn grants(&self) -> &SharedGrants {
        &self.inner.grants
    }

    /// The limits every call runs under.
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.inner.limits
    }

    /// Where `project-read` reads from (the open project's store), or `None` (no project:
    /// reads fail). Takes effect for every loaded plugin at its next call.
    pub fn set_project_reader(&self, reader: Option<ReadFn>) {
        if let Ok(mut r) = self.inner.reader.write() {
            *r = reader;
        }
    }

    /// Let WASM plugins provide and replace items of point `P`: `make` turns a guest item
    /// (a handle that calls the plugin's `call` export) into the point's item. Register
    /// adapters before loading the plugins that use them.
    pub fn adapt<P: ExtensionPoint>(
        &mut self,
        make: impl Fn(GuestItem) -> Result<P::Item, WasmError> + Send + Sync + 'static,
    ) {
        self.adapters.insert(
            P::NAME.to_string(),
            Arc::new(Typed::<P>::new(Arc::new(make), None)),
        );
    }

    /// [`WasmHost::adapt`], and let WASM plugins `chain` items of `P`: `wrap` builds the
    /// wrapper from the guest item and the wrapped item.
    pub fn adapt_chain<P: ExtensionPoint>(
        &mut self,
        make: impl Fn(GuestItem) -> Result<P::Item, WasmError> + Send + Sync + 'static,
        wrap: impl Fn(GuestItem, P::Item) -> P::Item + Send + Sync + 'static,
    ) {
        self.adapters.insert(
            P::NAME.to_string(),
            Arc::new(Typed::<P>::new(Arc::new(make), Some(Arc::new(wrap)))),
        );
    }

    /// The points WASM plugins can reach through this host.
    pub fn adapted_points(&self) -> impl Iterator<Item = &str> {
        self.adapters.keys().map(String::as_str)
    }

    /// Compile `code` (a component, binary or WAT text) for `manifest` and instantiate it.
    /// Checked before any guest code runs: the manifest is `kind: Wasm`, every import is a
    /// host interface (`WASM-0003`) whose capability the manifest requests (`WASM-0002`), and
    /// `call` is exported with the world's signature (`WASM-0004`).
    pub fn load(&self, manifest: Manifest, code: &[u8]) -> Result<WasmPlugin, WasmError> {
        if manifest.kind != PluginKind::Wasm {
            return Err(WasmError::NotWasm(manifest.id.to_string()));
        }
        let slot = Slot::new(&self.inner, &manifest, code)?;
        let adapters: BTreeMap<String, Arc<dyn PointAdapter>> = self.adapters.clone();
        Ok(WasmPlugin::new(
            manifest,
            Arc::new(slot),
            Arc::new(adapters),
            Arc::clone(&self.inner),
        ))
    }

    /// Load a plugin directory: `plugin.ron` and `plugin.wasm` (or `plugin.wat`), each read
    /// once ([`PluginFiles`]).
    pub fn load_dir(&self, dir: &Path) -> Result<WasmPlugin, WasmError> {
        self.load_files(&PluginFiles::read(dir)?)
    }

    /// Load a plugin from files already read (WP-36): the manifest is parsed from, and the
    /// code compiled from, exactly these bytes — what a host vetted is what runs. The plugin
    /// remembers them as the hot-reload watcher's starting point.
    pub fn load_files(&self, files: &PluginFiles) -> Result<WasmPlugin, WasmError> {
        let manifest = files.manifest()?;
        let Some((_, code)) = files.code() else {
            return Err(WasmError::Io {
                path: manifest.id.to_string(),
                why: "no plugin.wasm or plugin.wat".into(),
            });
        };
        let p = self.load(manifest, code)?;
        Ok(p.with_baseline(files.baseline()))
    }

    /// Every event since the last drain (oldest first; the log keeps the last 1024).
    pub fn drain_events(&self) -> Vec<HostEvent> {
        self.inner.events.drain()
    }
}

/// The code file of a plugin directory: `plugin.wasm`, else `plugin.wat`.
#[must_use]
pub fn code_file(dir: &Path) -> Option<PathBuf> {
    ["plugin.wasm", "plugin.wat"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

pub(crate) fn read_file(p: &Path) -> Result<Vec<u8>, WasmError> {
    std::fs::read(p).map_err(|e| WasmError::Io {
        path: p.display().to_string(),
        why: e.to_string(),
    })
}

/// Every import is a host interface the manifest has requested the capability for.
pub(crate) fn check_imports(
    engine: &Engine,
    component: &Component,
    manifest: &Manifest,
) -> Result<(), WasmError> {
    for (name, _) in component.component_type().imports(engine) {
        let Some((_, cap)) = INTERFACES.iter().find(|(n, _)| *n == name) else {
            return Err(WasmError::UnknownImport {
                plugin: manifest.id.to_string(),
                import: name.to_string(),
            });
        };
        if let Some(cap) = cap
            && !manifest.capabilities.contains(cap)
        {
            return Err(WasmError::UndeclaredImport {
                plugin: manifest.id.to_string(),
                import: name.to_string(),
                capability: cap.to_string(),
            });
        }
    }
    Ok(())
}

fn link(linker: &mut Linker<GuestState>) -> wasmtime::Result<()> {
    linker.instance(HOST_INTERFACE)?.func_wrap(
        "log",
        |cx: StoreContextMut<'_, GuestState>, (msg,): (String,)| {
            let s = cx.data();
            let mut line = msg;
            if line.len() > MAX_LINE {
                let mut cut = MAX_LINE;
                while !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                line.truncate(cut);
            }
            s.events.push(HostEvent::Log {
                plugin: s.plugin.clone(),
                line,
            });
            Ok(())
        },
    )?;
    linker.instance(PROJECT_READ_INTERFACE)?.func_wrap(
        "read",
        |cx: StoreContextMut<'_, GuestState>, (path,): (String,)| {
            let s = cx.data();
            let out = s
                .allowed_lazy(Capability::Fs(FsScope::ProjectRead), || {
                    format!("read {path}")
                })
                .and_then(|()| {
                    let reader = s
                        .reader
                        .read()
                        .map_err(|_| "the host is poisoned".to_string())?;
                    match reader.as_ref() {
                        Some(r) => r(&path),
                        None => Err("no project is open".to_string()),
                    }
                });
            Ok((out,))
        },
    )?;
    Ok(())
}

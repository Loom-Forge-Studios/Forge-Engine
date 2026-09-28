//! A loaded WASM plugin: its manifest, its live instance (swappable in place: hot reload),
//! and the items it installs through the ordinary loader.

use std::sync::{Arc, Mutex};

use forge_plugin::{
    Capability, HostedPlugin, InstallCx, ItemRef, Manifest, PluginError, PluginId, Principal,
    SharedGrants,
};
use wasmtime::component::{Component, Func, InstancePre};
use wasmtime::{Store, StoreLimitsBuilder};

use crate::WasmError;
use crate::adapter::Adapters;
use crate::files::Baseline;
use crate::host::{EXPORT, GuestState, HostEvent, HostInner, check_imports};

type CallSig<'a> = (&'a str, &'a str, &'a [u8]);
type CallRet = (Result<Vec<u8>, String>,);

/// One instantiation of the plugin's code.
struct Live {
    pre: InstancePre<GuestState>,
    /// `None` after a trap: the next call instantiates afresh from `pre`.
    running: Option<(Store<GuestState>, Func)>,
    generation: u64,
    /// FNV-1a of the code bytes: what an adapter versions the plugin's output by (a WASM
    /// importer's version follows its code across hot reloads and restarts).
    code_hash: u64,
}

/// FNV-1a, 64-bit: stable across builds and platforms (unlike `DefaultHasher`), so a value
/// derived from it may be persisted.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The plugin's code, behind a lock so the items it installed are `Send + Sync` and so a
/// hot reload can swap the code under them.
pub(crate) struct Slot {
    plugin: PluginId,
    host: Arc<HostInner>,
    live: Mutex<Live>,
}

impl Slot {
    pub(crate) fn new(
        host: &Arc<HostInner>,
        manifest: &Manifest,
        code: &[u8],
    ) -> Result<Self, WasmError> {
        let pre = compile(host, manifest, code)?;
        let running = instantiate(host, &manifest.id, &pre)?;
        Ok(Self {
            plugin: manifest.id.clone(),
            host: Arc::clone(host),
            live: Mutex::new(Live {
                pre,
                running: Some(running),
                generation: 1,
                code_hash: fnv1a(code),
            }),
        })
    }

    fn call(&self, point: &str, key: &str, input: &[u8]) -> Result<Vec<u8>, WasmError> {
        let trap = |why: String| WasmError::Trap {
            plugin: self.plugin.to_string(),
            why,
        };
        let mut live = self
            .live
            .lock()
            .map_err(|_| trap("an earlier call poisoned the plugin's lock".into()))?;
        let (mut store, func) = match live.running.take() {
            Some(r) => r,
            None => instantiate(&self.host, &self.plugin, &live.pre)?,
        };
        store
            .set_fuel(self.host.limits.fuel_per_call)
            .map_err(|e| trap(format!("{e:#}")))?;
        let result = func
            .typed::<CallSig<'_>, CallRet>(&store)
            .and_then(|f| f.call(&mut store, (point, key, input)));
        match result {
            Ok((out,)) => {
                live.running = Some((store, func));
                out.map_err(|why| WasmError::Guest {
                    plugin: self.plugin.to_string(),
                    item: ItemRef::new(point, key).to_string(),
                    why,
                })
            }
            Err(e) => {
                // A trapped component instance may not be entered again: drop it; the next
                // call starts from a fresh instance of the same code.
                let why = format!("{e:#}");
                self.host.events.push(HostEvent::Trapped {
                    plugin: self.plugin.clone(),
                    why: why.clone(),
                });
                Err(trap(why))
            }
        }
    }

    fn swap(&self, manifest: &Manifest, code: &[u8]) -> Result<u64, WasmError> {
        // Compile and instantiate outside the lock: calls keep running on the old code
        // until the new code is proven loadable.
        let pre = compile(&self.host, manifest, code)?;
        let running = instantiate(&self.host, &manifest.id, &pre)?;
        let mut live = self.live.lock().map_err(|_| WasmError::Trap {
            plugin: self.plugin.to_string(),
            why: "an earlier call poisoned the plugin's lock".into(),
        })?;
        live.pre = pre;
        live.running = Some(running);
        live.generation += 1;
        live.code_hash = fnv1a(code);
        let generation = live.generation;
        drop(live);
        self.host.events.push(HostEvent::Reloaded {
            plugin: self.plugin.clone(),
            generation,
        });
        Ok(generation)
    }

    fn generation(&self) -> u64 {
        self.live.lock().map_or(0, |l| l.generation)
    }

    fn code_hash(&self) -> u64 {
        self.live.lock().map_or(0, |l| l.code_hash)
    }
}

fn compile(
    host: &HostInner,
    manifest: &Manifest,
    code: &[u8],
) -> Result<InstancePre<GuestState>, WasmError> {
    let component = Component::new(&host.engine, code).map_err(|e| WasmError::Compile {
        plugin: manifest.id.to_string(),
        why: format!("{e:#}"),
    })?;
    check_imports(&host.engine, &component, manifest)?;
    host.linker
        .instantiate_pre(&component)
        .map_err(|e| WasmError::Compile {
            plugin: manifest.id.to_string(),
            why: format!("{e:#}"),
        })
}

fn instantiate(
    host: &HostInner,
    plugin: &PluginId,
    pre: &InstancePre<GuestState>,
) -> Result<(Store<GuestState>, Func), WasmError> {
    let state = GuestState {
        plugin: plugin.clone(),
        who: Principal::Plugin(plugin.clone()),
        grants: host.grants.clone(),
        reader: Arc::clone(&host.reader),
        events: host.events.clone(),
        limits: StoreLimitsBuilder::new()
            .memory_size(host.limits.memory_bytes)
            .instances(16)
            .memories(4)
            .tables(16)
            .build(),
    };
    let mut store = Store::new(&host.engine, state);
    store.limiter(|s| &mut s.limits);
    let trap = |e: wasmtime::Error| WasmError::Trap {
        plugin: plugin.to_string(),
        why: format!("{e:#}"),
    };
    store.set_fuel(host.limits.fuel_per_call).map_err(trap)?;
    let instance = pre.instantiate(&mut store).map_err(trap)?;
    let func = instance
        .get_func(&mut store, EXPORT)
        .ok_or_else(|| WasmError::MissingExport {
            plugin: plugin.to_string(),
            why: "no export named `call`".into(),
        })?;
    func.typed::<CallSig<'_>, CallRet>(&store)
        .map_err(|e| WasmError::MissingExport {
            plugin: plugin.to_string(),
            why: format!("{e:#}"),
        })?;
    Ok((store, func))
}

/// One item a WASM plugin serves: calling it calls the plugin's `call` export with this
/// item's point and key. Adapters wrap it into the point's item type. Cloning is cheap, and
/// every clone follows the plugin's code across hot reloads.
#[derive(Clone)]
pub struct GuestItem {
    slot: Arc<Slot>,
    point: String,
    key: String,
    grants: SharedGrants,
}

impl std::fmt::Debug for GuestItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestItem")
            .field("plugin", &self.slot.plugin)
            .field("point", &self.point)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl GuestItem {
    /// The plugin serving it.
    #[must_use]
    pub fn plugin(&self) -> &PluginId {
        &self.slot.plugin
    }

    /// The point's manifest name.
    #[must_use]
    pub fn point(&self) -> &str {
        &self.point
    }

    /// The item's key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// `Point("key")`.
    #[must_use]
    pub fn name(&self) -> String {
        ItemRef::new(&self.point, &self.key).to_string()
    }

    /// Call the plugin for this item. Serialised per plugin; bounded by the host's limits.
    pub fn call(&self, input: &[u8]) -> Result<Vec<u8>, WasmError> {
        self.slot.call(&self.point, &self.key, input)
    }

    /// Check `cap` against the shared grant table **now** (adapters gate the effects the
    /// guest asks for, e.g. a destructive command, with this). A denial is recorded.
    pub fn check(&self, cap: Capability) -> Result<(), PluginError> {
        let who = Principal::Plugin(self.slot.plugin.clone());
        self.grants.check(&who, cap).inspect_err(|_| {
            self.slot.host.events.push(HostEvent::Denied {
                plugin: self.slot.plugin.clone(),
                capability: cap,
                what: self.name(),
            });
        })
    }

    /// How many times the plugin's code has been loaded (1, then +1 per hot reload).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.slot.generation()
    }

    /// A stable hash (FNV-1a) of the code serving it now: it changes with a hot reload and
    /// is the same for the same bytes in every run.
    #[must_use]
    pub fn code_hash(&self) -> u64 {
        self.slot.code_hash()
    }
}

/// A loaded WASM plugin. Give it to [`forge_plugin::loader::load_hosted`] to install it;
/// [`WasmPlugin::reload`] swaps its code in place. Cloning shares the plugin.
#[derive(Clone)]
pub struct WasmPlugin {
    manifest: Manifest,
    slot: Arc<Slot>,
    adapters: Arc<Adapters>,
    host: Arc<HostInner>,
    /// The files it was compiled from, when it was loaded from files (WP-36): the hot-reload
    /// watcher's starting point.
    baseline: Option<Arc<Baseline>>,
}

impl std::fmt::Debug for WasmPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPlugin")
            .field("id", &self.manifest.id)
            .field("generation", &self.generation())
            .finish_non_exhaustive()
    }
}

impl WasmPlugin {
    pub(crate) fn new(
        manifest: Manifest,
        slot: Arc<Slot>,
        adapters: Arc<Adapters>,
        host: Arc<HostInner>,
    ) -> Self {
        Self {
            manifest,
            slot,
            adapters,
            host,
            baseline: None,
        }
    }

    /// It was compiled from `b` (see [`crate::PluginFiles`]).
    pub(crate) fn with_baseline(mut self, b: Baseline) -> Self {
        self.baseline = Some(Arc::new(b));
        self
    }

    /// The files it was compiled from, when it was loaded from files.
    pub(crate) fn baseline(&self) -> Option<&Baseline> {
        self.baseline.as_deref()
    }

    /// Its id.
    #[must_use]
    pub fn id(&self) -> &PluginId {
        &self.manifest.id
    }

    /// A handle on one of its items (whether or not the manifest declares it: the loader's
    /// install is what checks declarations; tools and benchmarks call items directly).
    #[must_use]
    pub fn item(&self, point: &str, key: &str) -> GuestItem {
        GuestItem {
            slot: Arc::clone(&self.slot),
            point: point.to_string(),
            key: key.to_string(),
            grants: self.host.grants.clone(),
        }
    }

    /// Hot reload: compile `code`, check it against the **unchanged** manifest, and swap it
    /// in under every installed item. On any error the old code keeps running. Returns the
    /// new generation.
    pub fn reload(&self, code: &[u8]) -> Result<u64, WasmError> {
        self.slot.swap(&self.manifest, code)
    }

    /// How many times its code has been loaded.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.slot.generation()
    }
}

impl HostedPlugin for WasmPlugin {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        let m = &self.manifest;
        let who = m.id.to_string();
        let ops: [(&'static str, &Vec<ItemRef>); 4] = [
            ("provides", &m.provides),
            ("replaces", &m.replaces),
            ("removes", &m.removes),
            ("chains", &m.chains),
        ];
        for (op, items) in ops {
            for r in items {
                let adapter = self.adapters.get(&r.point).ok_or_else(|| {
                    WasmError::NoAdapter {
                        plugin: who.clone(),
                        point: r.point.clone(),
                        op,
                    }
                    .into_plugin_error(&who)
                })?;
                let item = self.item(&r.point, &r.key);
                let done = match op {
                    "provides" => adapter.add(cx, &r.key, item),
                    "replaces" => adapter.replace(cx, &r.key, item),
                    "removes" => adapter.remove(cx, &r.key),
                    _ => adapter.chain(cx, &r.key, item),
                };
                done.map_err(|e| e.into_plugin_error(&who))?;
            }
        }
        Ok(())
    }
}

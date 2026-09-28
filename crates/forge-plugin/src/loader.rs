//! The source-plugin loader: manifests first, conflicts from manifests alone, then every
//! plugin's operations applied in a fixed order.
//!
//! 1. Every manifest is checked: unique ids, the `engine` requirement against
//!    [`KERNEL_VERSION`], every named point defined.
//! 2. Conflicts are found **from the manifests**, WASM ones included: two providers of one
//!    item (`PLUGIN-0003`), two replacers, or a replacer and a remover (`PLUGIN-0006`) — each
//!    error names both plugins. No plugin code has run yet.
//! 3. Each plugin's `install` (source, or hosted by a sandbox host: [`load_hosted`]) queues its
//!    operations; one it did not declare is refused
//!    (`PLUGIN-0011`). A panicking install is caught (`PLUGIN-0013`).
//! 4. Queued operations apply phase by phase — every `add`, then every `replace`, `remove`,
//!    `chain` — and within a phase in plugin-id order. The result does not depend on the
//!    order plugins were discovered in, and a replace always finds the item it targets.
//!
//! On an error the host may hold part of the load; a load error is fatal for that plugin
//! set, and the caller reports it and starts from a fresh [`Extensions`].

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{self, AssertUnwindSafe};

use crate::manifest::Op;
use crate::registry::panic_message;
use crate::{
    Capability, ExtensionPoint, Extensions, Grants, ItemId, ItemRef, Manifest, Order, PluginError,
    PluginId, PluginKind, Principal,
};

/// The kernel's version, which manifests' `engine` requirements are checked against.
pub const KERNEL_VERSION: &str = "0.1.0";

/// A plugin compiled into the engine (Ch.32.3: native, full trust, reaches every point).
/// First-party subsystems are source plugins too, loaded through exactly this path (I16).
pub trait SourcePlugin {
    /// Its manifest (usually `Manifest::parse(include_str!("plugin.ron"))`).
    fn manifest(&self) -> &Manifest;
    /// Queue its operations. Everything it does must be declared in its manifest.
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError>;
}

/// A plugin whose code runs inside a sandbox host (a WASM component, Ch.32.3; the host is
/// `forge-wasm`). Its manifest says `kind: Wasm`; it installs through the same
/// [`InstallCx`], checked against the same manifest, in the same fixed order as a source
/// plugin — the host only turns its guest exports into point items.
pub trait HostedPlugin {
    /// Its manifest (`kind: Wasm`).
    fn manifest(&self) -> &Manifest;
    /// Queue its operations (each item calls into the sandbox).
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError>;
}

type Apply = Box<dyn FnOnce(&mut Extensions) -> Result<(), PluginError>>;

struct Queued {
    op: Op,
    plugin: PluginId,
    seq: usize,
    apply: Apply,
}

/// What a plugin's `install` gets: typed operations, checked against its manifest and
/// queued for the loader's fixed apply order.
pub struct InstallCx {
    manifest: Manifest,
    names: BTreeMap<&'static str, &'static str>,
    /// Items more than one plugin removes (they agree; the second removal is a no-op).
    shared_removals: BTreeSet<ItemRef>,
    ops: Vec<Queued>,
}

impl InstallCx {
    fn check<P: ExtensionPoint>(&self, op: Op, key: &str) -> Result<ItemId, PluginError> {
        let id = ItemId::of::<P>(key)?;
        let r = ItemRef::new(P::NAME, key);
        if !self.manifest.declares(op, &r) {
            return Err(PluginError::Undeclared {
                plugin: self.manifest.id.to_string(),
                op: op.verb(),
                item: r.to_string(),
            });
        }
        if self.names.get(P::NAME) != Some(&P::ID) {
            return Err(PluginError::UnknownPoint {
                plugin: self.manifest.id.to_string(),
                point: P::NAME.to_string(),
            });
        }
        Ok(id)
    }

    fn push(&mut self, op: Op, apply: Apply) {
        let seq = self.ops.len();
        self.ops.push(Queued {
            op,
            plugin: self.manifest.id.clone(),
            seq,
            apply,
        });
    }

    /// The plugin being installed.
    #[must_use]
    pub fn plugin(&self) -> &PluginId {
        &self.manifest.id
    }

    /// Add `item` to point `P` under `key` (declared in `provides`).
    pub fn add<P: ExtensionPoint>(
        &mut self,
        key: &str,
        item: P::Item,
        order: Order,
    ) -> Result<(), PluginError> {
        self.check::<P>(Op::Add, key)?;
        let owner = self.manifest.id.clone();
        let key = key.to_string();
        self.push(
            Op::Add,
            Box::new(move |x: &mut Extensions| {
                let who = owner.to_string();
                x.require_mut::<P>(&who)?
                    .add(owner, &key, item, order)
                    .map(|_| ())
            }),
        );
        Ok(())
    }

    /// Replace `key` at point `P` with `item` (declared in `replaces`).
    pub fn replace<P: ExtensionPoint>(
        &mut self,
        key: &str,
        item: P::Item,
    ) -> Result<(), PluginError> {
        let id = self.check::<P>(Op::Replace, key)?;
        let by = self.manifest.id.clone();
        self.push(
            Op::Replace,
            Box::new(move |x: &mut Extensions| {
                x.require_mut::<P>(by.as_str())?
                    .replace(&by, &id, item)
                    .map(|_| ())
            }),
        );
        Ok(())
    }

    /// Remove `key` from point `P` (declared in `removes`).
    pub fn remove<P: ExtensionPoint>(&mut self, key: &str) -> Result<(), PluginError> {
        let id = self.check::<P>(Op::Remove, key)?;
        let by = self.manifest.id.clone();
        let shared = self.shared_removals.contains(&ItemRef::new(P::NAME, key));
        self.push(
            Op::Remove,
            Box::new(move |x: &mut Extensions| {
                let reg = x.require_mut::<P>(by.as_str())?;
                if shared && reg.get(id.key()).is_none() {
                    return Ok(()); // another plugin already removed it; they agree
                }
                reg.remove(&by, &id).map(|_| ())
            }),
        );
        Ok(())
    }

    /// Wrap `key` at point `P` with `wrap` (declared in `chains`).
    pub fn chain<P: ExtensionPoint>(
        &mut self,
        key: &str,
        wrap: impl FnOnce(P::Item) -> P::Item + 'static,
    ) -> Result<(), PluginError> {
        let id = self.check::<P>(Op::Chain, key)?;
        let by = self.manifest.id.clone();
        self.push(
            Op::Chain,
            Box::new(move |x: &mut Extensions| {
                x.require_mut::<P>(by.as_str())?.chain(&by, &id, wrap)
            }),
        );
        Ok(())
    }
}

/// One plugin's capabilities, as the plugin manager shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityReport {
    /// The plugin.
    pub plugin: PluginId,
    /// What its manifest requests.
    pub requested: Vec<Capability>,
    /// What of that a human has granted.
    pub granted: Vec<Capability>,
}

/// The outcome of a successful load.
#[derive(Debug, Default)]
pub struct LoadReport {
    /// Plugins installed (source and hosted), in id order.
    pub installed: Vec<PluginId>,
    /// Plugins whose manifests took part in the checks but that were not installed, with why
    /// (a WASM plugin loaded with no sandbox host: `PLUGIN-0014`).
    pub not_installed: Vec<(PluginId, PluginError)>,
    /// Requested vs granted capabilities, per plugin.
    pub capabilities: Vec<CapabilityReport>,
}

/// Load `sources` (compiled in) alongside the manifests of `wasm` plugins into `ext`,
/// against [`KERNEL_VERSION`].
pub fn load(
    ext: &mut Extensions,
    sources: &[&dyn SourcePlugin],
    wasm: &[Manifest],
    grants: &Grants,
) -> Result<LoadReport, PluginError> {
    let kernel = semver::Version::parse(KERNEL_VERSION).map_err(|e| PluginError::Manifest {
        plugin: None,
        why: format!("kernel version: {e}"),
    })?;
    load_against(ext, sources, wasm, grants, &kernel)
}

/// [`load`] against an explicit kernel version (tests, tools).
pub fn load_against(
    ext: &mut Extensions,
    sources: &[&dyn SourcePlugin],
    wasm: &[Manifest],
    grants: &Grants,
    kernel: &semver::Version,
) -> Result<LoadReport, PluginError> {
    load_inner(ext, sources, &[], wasm, grants, kernel)
}

/// Load `sources` and sandbox-`hosted` plugins (a WASM host's plugins) together, through
/// exactly the path [`load`] takes: one manifest check, one conflict check across both
/// kinds, one install order. `wasm` lists manifests of WASM plugins no host was given
/// (reported `PLUGIN-0014`).
pub fn load_hosted(
    ext: &mut Extensions,
    sources: &[&dyn SourcePlugin],
    hosted: &[&dyn HostedPlugin],
    wasm: &[Manifest],
    grants: &Grants,
) -> Result<LoadReport, PluginError> {
    let kernel = semver::Version::parse(KERNEL_VERSION).map_err(|e| PluginError::Manifest {
        plugin: None,
        why: format!("kernel version: {e}"),
    })?;
    load_inner(ext, sources, hosted, wasm, grants, &kernel)
}

#[derive(Clone, Copy)]
enum Installer<'a> {
    Source(&'a dyn SourcePlugin),
    Hosted(&'a dyn HostedPlugin),
}

impl Installer<'_> {
    fn manifest(&self) -> &Manifest {
        match self {
            Self::Source(s) => s.manifest(),
            Self::Hosted(h) => h.manifest(),
        }
    }

    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        match self {
            Self::Source(s) => s.install(cx),
            Self::Hosted(h) => h.install(cx),
        }
    }
}

fn load_inner(
    ext: &mut Extensions,
    sources: &[&dyn SourcePlugin],
    hosted: &[&dyn HostedPlugin],
    wasm: &[Manifest],
    grants: &Grants,
    kernel: &semver::Version,
) -> Result<LoadReport, PluginError> {
    // 1. Manifests, in id order.
    let mut all: Vec<&Manifest> = sources
        .iter()
        .map(|s| s.manifest())
        .chain(hosted.iter().map(|h| h.manifest()))
        .chain(wasm)
        .collect();
    all.sort_by(|a, b| a.id.cmp(&b.id));
    for w in all.windows(2) {
        if w[0].id == w[1].id {
            return Err(PluginError::DuplicatePlugin(w[0].id.to_string()));
        }
    }
    let names: BTreeMap<&'static str, &'static str> =
        ext.points().map(|p| (p.name, p.id)).collect();
    for m in &all {
        m.validate()?;
        if !m.engine.matches(kernel) {
            return Err(PluginError::EngineMismatch {
                plugin: m.id.to_string(),
                requires: m.engine.to_string(),
                kernel: kernel.to_string(),
            });
        }
        for r in m
            .provides
            .iter()
            .chain(&m.replaces)
            .chain(&m.removes)
            .chain(&m.chains)
        {
            if !names.contains_key(r.point.as_str()) {
                return Err(PluginError::UnknownPoint {
                    plugin: m.id.to_string(),
                    point: r.point.clone(),
                });
            }
        }
    }
    for s in sources {
        if s.manifest().kind != PluginKind::Source {
            return Err(PluginError::Manifest {
                plugin: Some(s.manifest().id.to_string()),
                why: "compiled in as a source plugin, but its manifest says `kind: Wasm`".into(),
            });
        }
    }
    for h in hosted {
        if h.manifest().kind != PluginKind::Wasm {
            return Err(PluginError::Manifest {
                plugin: Some(h.manifest().id.to_string()),
                why: "given to a sandbox host, but its manifest says `kind: Source`".into(),
            });
        }
    }

    // 2. Conflicts, from the manifests alone.
    check_conflicts(&all)?;
    let mut removers: BTreeMap<&ItemRef, usize> = BTreeMap::new();
    for m in &all {
        for r in &m.removes {
            *removers.entry(r).or_default() += 1;
        }
    }
    let shared_removals: BTreeSet<ItemRef> = removers
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(r, _)| r.clone())
        .collect();

    // 3. Queue each plugin's operations, in id order (source and hosted alike).
    let mut sorted: Vec<Installer<'_>> = sources
        .iter()
        .map(|s| Installer::Source(*s))
        .chain(hosted.iter().map(|h| Installer::Hosted(*h)))
        .collect();
    sorted.sort_by(|a, b| a.manifest().id.cmp(&b.manifest().id));
    let mut queue = Vec::new();
    for s in &sorted {
        let mut cx = InstallCx {
            manifest: s.manifest().clone(),
            names: names.clone(),
            shared_removals: shared_removals.clone(),
            ops: Vec::new(),
        };
        match panic::catch_unwind(AssertUnwindSafe(|| s.install(&mut cx))) {
            Ok(Ok(())) => queue.extend(cx.ops),
            Ok(Err(e)) => return Err(e),
            Err(payload) => {
                return Err(PluginError::Panicked {
                    plugin: s.manifest().id.to_string(),
                    during: "install".into(),
                    message: panic_message(payload.as_ref()),
                });
            }
        }
    }

    // 4. Apply: by phase, then plugin id, then queue order.
    queue.sort_by(|a, b| (a.op, &a.plugin, a.seq).cmp(&(b.op, &b.plugin, b.seq)));
    for q in queue {
        (q.apply)(ext)?;
    }

    let installed: Vec<PluginId> = sorted.iter().map(|s| s.manifest().id.clone()).collect();
    let not_installed = wasm
        .iter()
        .map(|m| (m.id.clone(), PluginError::WasmHostUnbuilt(m.id.to_string())))
        .collect();
    let capabilities = all
        .iter()
        .map(|m| {
            let who = Principal::Plugin(m.id.clone());
            CapabilityReport {
                plugin: m.id.clone(),
                requested: m.capabilities.clone(),
                granted: m
                    .capabilities
                    .iter()
                    .copied()
                    .filter(|c| grants.has(&who, *c))
                    .collect(),
            }
        })
        .collect();
    Ok(LoadReport {
        installed,
        not_installed,
        capabilities,
    })
}

/// Two providers of one item, two replacers of one item, or a replacer and a remover of one
/// item are load errors naming both plugins (manifests in id order, so the report is stable).
fn check_conflicts(all: &[&Manifest]) -> Result<(), PluginError> {
    match conflicts(all).into_iter().next() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// **Every** conflict among `all` (the plugin manager lists them all, each naming both
/// plugins; the loader refuses the set on the first): two providers of one item
/// (`PLUGIN-0003`), two replacers, or a replacer and a remover (`PLUGIN-0006`), or a chain
/// of a removed item. Manifests are taken in id order, so the list is stable.
#[must_use]
pub fn conflicts(all: &[&Manifest]) -> Vec<PluginError> {
    let mut sorted: Vec<&Manifest> = all.to_vec();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let all = &sorted[..];
    let mut out = Vec::new();
    let mut providers: BTreeMap<&ItemRef, &PluginId> = BTreeMap::new();
    let mut modifiers: BTreeMap<&ItemRef, (&PluginId, &'static str)> = BTreeMap::new();
    for m in all {
        for r in &m.provides {
            if let Some(first) = providers.get(r) {
                out.push(PluginError::DuplicateItem {
                    item: r.to_string(),
                    first: first.to_string(),
                    second: m.id.to_string(),
                });
            } else {
                providers.insert(r, &m.id);
            }
        }
    }
    for m in all {
        let ops = m
            .replaces
            .iter()
            .map(|r| (r, "replaces"))
            .chain(m.removes.iter().map(|r| (r, "removes")));
        for (r, op) in ops {
            if let Some(&(first, first_op)) = modifiers.get(r) {
                // Two plugins removing the same item agree; anything else conflicts.
                if !(first_op == "removes" && op == "removes") {
                    out.push(PluginError::Conflict {
                        item: r.to_string(),
                        first: first.to_string(),
                        first_op,
                        second: m.id.to_string(),
                        second_op: op,
                    });
                }
            } else {
                modifiers.insert(r, (&m.id, op));
            }
        }
    }
    for m in all {
        for r in &m.chains {
            if let Some(&(first, "removes")) = modifiers.get(r) {
                out.push(PluginError::Conflict {
                    item: r.to_string(),
                    first: first.to_string(),
                    first_op: "removes",
                    second: m.id.to_string(),
                    second_op: "chains",
                });
            }
        }
    }
    out
}

//! I16 — `test_no_privileged_plugin` (Ch.32.1, M2-13, M2-14): **no first-party subsystem uses
//! a capability a plugin cannot use.** The engine is a kernel (`crates/*`) plus plugins
//! (`plugins/*`), and a first-party plugin reaches the kernel exactly as a third-party one
//! would.
//!
//! **Static half — no back door in the source.** Every `plugins/*` crate is parsed (its
//! non-test code), every path it names into a kernel crate — `use` trees, expression and type
//! paths, paths through local `use` aliases, macro-free code — is resolved against the kernel
//! crate's **public surface** (built by walking the kernel crate's modules from its root:
//! `pub` items in `pub` modules, `pub use` re-exports and globs, enum variants, inherent
//! methods), and it is a violation to reach:
//!
//! * an item that exists but is not public (`pub(crate)`, private, in a private module);
//! * a `#[doc(hidden)]` item or an item under a hidden module (test kits, internals);
//! * kernel source by inclusion: `#[path = ...]` into `crates/`, or `include!` /
//!   `include_str!` / `include_bytes!` of a file under `crates/`;
//! * a privileged kernel feature (`mutate-*`, `internal*`, `private*`, `test*`) in the
//!   plugin's `Cargo.toml`;
//! * a registration identity of its own (`PluginId::new`) or a mutable registry
//!   (`registry_mut`, `Registry::new`) — a plugin changes registries only through the
//!   loader's `InstallCx`, where its manifest declares everything it does.
//!
//! Every `plugins/*` crate must implement `SourcePlugin` (it is a real plugin), and no kernel
//! code branches on a plugin being first-party (`is_first_party()` outside tests).
//!
//! **Runtime half — the public loader only.** Every first-party plugin (the three subsystems
//! M2-14 moved — importers, presets, store backends — plus the asset types and the panel
//! sets) loads through `forge_plugin::loader::load`; every item it registers is owned by it;
//! and **the same plugin under a third-party id installs an identical registry**, so the
//! loader grants `forge.*` nothing.
//!
//! Positive controls (W2):
//! * `positive_control_each_back_door_is_caught` — a real plugin's source with one back door
//!   injected at a time (a `#[doc(hidden)]` kernel item, a `pub(crate)` kernel item, a
//!   private module, `#[path]` into kernel source, `include!` of kernel source, a minted
//!   `PluginId`, a privileged feature): the scan must go red on each, naming it;
//! * `positive_control_a_plugin_privileged_by_its_id_is_caught` — a plugin that installs
//!   more when its id is `forge.*` fails the third-party equivalence.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use forge_plugin::{
    Extensions, Grants, InstallCx, Manifest, PluginError, PluginId, SourcePlugin, loader,
};
use syn::visit::Visit;

// ---- the kernel's public surface -----------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Reach {
    /// On the public surface.
    Public,
    /// Exists, but is not public (why).
    Private(String),
    /// Public but `#[doc(hidden)]` (why).
    Hidden(String),
    /// Not found (macro-generated, or a surface this index does not model).
    Unresolved,
}

#[derive(Clone, Debug, Default)]
struct Item {
    public: bool,
    hidden: bool,
    variants: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
struct Reexport {
    public: bool,
    hidden: bool,
    /// Absolute target: `[crate, seg, ...]` (a kernel crate) or `[]` (external).
    target: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Module {
    path: Vec<String>,
    items: BTreeMap<String, Item>,
    mods: BTreeMap<String, (bool, bool, Module)>,
    reexports: BTreeMap<String, Reexport>,
    globs: Vec<Reexport>,
}

#[derive(Clone, Debug, Default)]
struct KernelCrate {
    root: Module,
    /// `(type name, method)` -> (public, hidden) for inherent methods.
    methods: BTreeMap<(String, String), (bool, bool)>,
}

fn is_public(v: &syn::Visibility) -> bool {
    matches!(v, syn::Visibility::Public(_))
}

fn is_hidden(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("doc") && a.parse_args::<syn::Ident>().is_ok_and(|i| i == "hidden")
    })
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.parse_args::<syn::Meta>().is_ok_and(|m| match m {
                syn::Meta::Path(p) => p.is_ident("test"),
                _ => false,
            })
    })
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|a| {
        if !a.path().is_ident("path") {
            return None;
        }
        match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        }
    })
}

/// The file a `mod name;` declared in `file` lives in.
fn mod_file(file: &Path, name: &str, attr: Option<&str>) -> Option<PathBuf> {
    let dir = file.parent()?;
    if let Some(p) = attr {
        return Some(dir.join(p));
    }
    let stem = file.file_stem()?.to_string_lossy();
    let base = if matches!(stem.as_ref(), "lib" | "main" | "mod") {
        dir.to_path_buf()
    } else {
        dir.join(stem.as_ref())
    };
    [
        base.join(format!("{name}.rs")),
        base.join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Flatten a use tree into `(local name, full path)`; a glob is `("*", path)`.
fn flatten(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<(String, Vec<String>)>) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if let Some(last) = prefix.last() {
                    out.push((last.clone(), prefix.clone()));
                }
            } else {
                let mut full = prefix.clone();
                full.push(name.clone());
                out.push((name, full));
            }
        }
        syn::UseTree::Rename(r) => {
            let mut full = prefix.clone();
            let name = r.ident.to_string();
            if name != "self" {
                full.push(name);
            }
            out.push((r.rename.to_string(), full));
        }
        syn::UseTree::Glob(_) => out.push(("*".into(), prefix.clone())),
        syn::UseTree::Group(g) => {
            for t in &g.items {
                flatten(t, prefix, out);
            }
        }
    }
}

struct Indexer<'a> {
    krate: &'a str,
    kernel: &'a BTreeSet<String>,
    methods: BTreeMap<(String, String), (bool, bool)>,
}

impl Indexer<'_> {
    /// Make a `use` path in module `m` absolute.
    fn absolute(&self, m: &Module, raw: &[String]) -> Vec<String> {
        let Some(first) = raw.first() else {
            return Vec::new();
        };
        match first.as_str() {
            "crate" => {
                let mut v = vec![self.krate.to_string()];
                v.extend_from_slice(&raw[1..]);
                v
            }
            "self" => {
                let mut v = m.path.clone();
                v.extend_from_slice(&raw[1..]);
                v
            }
            "super" => {
                let mut v = m.path.clone();
                let mut rest = raw;
                while rest.first().map(String::as_str) == Some("super") {
                    v.pop();
                    rest = &rest[1..];
                }
                v.extend_from_slice(rest);
                v
            }
            k if self.kernel.contains(k) => raw.to_vec(),
            _ => {
                // Uniform paths: a name in this module (declared anywhere in it).
                let mut v = m.path.clone();
                v.extend_from_slice(raw);
                v
            }
        }
    }

    fn module(&mut self, items: &[syn::Item], file: &Path, path: Vec<String>) -> Module {
        let mut m = Module {
            path,
            ..Module::default()
        };
        let mut uses = Vec::new();
        for it in items {
            let attrs: &[syn::Attribute] = match it {
                syn::Item::Const(i) => &i.attrs,
                syn::Item::Enum(i) => &i.attrs,
                syn::Item::Fn(i) => &i.attrs,
                syn::Item::Macro(i) => &i.attrs,
                syn::Item::Mod(i) => &i.attrs,
                syn::Item::Static(i) => &i.attrs,
                syn::Item::Struct(i) => &i.attrs,
                syn::Item::Trait(i) => &i.attrs,
                syn::Item::Type(i) => &i.attrs,
                syn::Item::Union(i) => &i.attrs,
                syn::Item::Use(i) => &i.attrs,
                syn::Item::Impl(i) => &i.attrs,
                _ => &[],
            };
            if is_cfg_test(attrs) {
                continue;
            }
            let hidden = is_hidden(attrs);
            let mut add = |name: String, public: bool, variants: BTreeSet<String>| {
                m.items.insert(
                    name,
                    Item {
                        public,
                        hidden,
                        variants,
                    },
                );
            };
            match it {
                syn::Item::Const(i) => add(i.ident.to_string(), is_public(&i.vis), BTreeSet::new()),
                syn::Item::Static(i) => {
                    add(i.ident.to_string(), is_public(&i.vis), BTreeSet::new())
                }
                syn::Item::Fn(i) => {
                    add(i.sig.ident.to_string(), is_public(&i.vis), BTreeSet::new())
                }
                syn::Item::Struct(i) => {
                    add(i.ident.to_string(), is_public(&i.vis), BTreeSet::new());
                }
                syn::Item::Union(i) => add(i.ident.to_string(), is_public(&i.vis), BTreeSet::new()),
                syn::Item::Trait(i) => add(i.ident.to_string(), is_public(&i.vis), BTreeSet::new()),
                syn::Item::Type(i) => add(i.ident.to_string(), is_public(&i.vis), BTreeSet::new()),
                syn::Item::Enum(i) => add(
                    i.ident.to_string(),
                    is_public(&i.vis),
                    i.variants.iter().map(|v| v.ident.to_string()).collect(),
                ),
                // A W2 fault-switch set (`forge_trace::control_switches! { pub struct X {..} }`,
                // ADR 0046) declares the struct inside the macro: read it as the struct it is,
                // so a plugin naming it is resolved (and a hidden one is flagged) as before.
                syn::Item::Macro(i)
                    if i.mac
                        .path
                        .segments
                        .last()
                        .is_some_and(|s| s.ident == "control_switches") =>
                {
                    if let Ok(s) = i.mac.parse_body::<syn::ItemStruct>() {
                        m.items.insert(
                            s.ident.to_string(),
                            Item {
                                public: is_public(&s.vis),
                                hidden: hidden || is_hidden(&s.attrs),
                                variants: BTreeSet::new(),
                            },
                        );
                    }
                }
                syn::Item::Macro(i) => {
                    if let Some(id) = &i.ident {
                        let exported = i.attrs.iter().any(|a| a.path().is_ident("macro_export"));
                        add(id.to_string(), exported, BTreeSet::new());
                    }
                }
                syn::Item::Impl(i) if i.trait_.is_none() => {
                    if let syn::Type::Path(tp) = &*i.self_ty
                        && let Some(last) = tp.path.segments.last()
                    {
                        for ii in &i.items {
                            if let syn::ImplItem::Fn(f) = ii {
                                self.methods.insert(
                                    (last.ident.to_string(), f.sig.ident.to_string()),
                                    (is_public(&f.vis), is_hidden(&f.attrs) || hidden),
                                );
                            }
                        }
                    }
                }
                syn::Item::Use(u) => uses.push(u.clone()),
                syn::Item::Mod(md) => {
                    let name = md.ident.to_string();
                    let mut child_path = m.path.clone();
                    child_path.push(name.clone());
                    let child = if let Some((_, inner)) = &md.content {
                        self.module(inner, file, child_path)
                    } else if let Some(f) = mod_file(file, &name, path_attr(&md.attrs).as_deref()) {
                        match std::fs::read_to_string(&f)
                            .ok()
                            .and_then(|t| syn::parse_file(&t).ok())
                        {
                            Some(parsed) => self.module(&parsed.items, &f, child_path),
                            None => Module {
                                path: child_path,
                                ..Module::default()
                            },
                        }
                    } else {
                        Module {
                            path: child_path,
                            ..Module::default()
                        }
                    };
                    m.mods.insert(name, (is_public(&md.vis), hidden, child));
                }
                _ => {}
            }
        }
        for u in uses {
            let mut flat = Vec::new();
            flatten(&u.tree, &mut Vec::new(), &mut flat);
            let public = is_public(&u.vis);
            let hidden = is_hidden(&u.attrs);
            for (name, raw) in flat {
                let target = self.absolute(&m, &raw);
                let r = Reexport {
                    public,
                    hidden,
                    target,
                };
                if name == "*" {
                    m.globs.push(r);
                } else if name != "_" {
                    m.reexports.entry(name).or_insert(r);
                }
            }
        }
        m
    }
}

struct Kernel {
    crates: BTreeMap<String, KernelCrate>,
}

impl Kernel {
    fn load(root: &Path, names: &BTreeSet<String>) -> Self {
        let mut crates = BTreeMap::new();
        for (name, dir) in kernel_dirs(root) {
            let lib = dir.join("src").join("lib.rs");
            let Some(parsed) = std::fs::read_to_string(&lib)
                .ok()
                .and_then(|t| syn::parse_file(&t).ok())
            else {
                continue;
            };
            let mut ix = Indexer {
                krate: &name,
                kernel: names,
                methods: BTreeMap::new(),
            };
            let root = ix.module(&parsed.items, &lib, vec![name.clone()]);
            crates.insert(
                name.clone(),
                KernelCrate {
                    root,
                    methods: ix.methods,
                },
            );
        }
        Self { crates }
    }

    fn resolve(&self, segs: &[String], depth: usize) -> Reach {
        self.resolve_from(segs, depth, 1)
    }

    /// Resolve `segs`; visibility is checked only from segment `strict` on (a `pub use`
    /// target may sit in a private module: that is how a public surface is built).
    fn resolve_from(&self, segs: &[String], depth: usize, strict: usize) -> Reach {
        if depth > 16 || segs.is_empty() {
            return Reach::Unresolved;
        }
        let Some(k) = self.crates.get(&segs[0]) else {
            return Reach::Unresolved;
        };
        let mut m = &k.root;
        let mut hidden_by: Option<String> = None;
        let mut i = 1;
        while i < segs.len() {
            let s = &segs[i];
            let here = segs[..=i].join("::");
            if let Some((public, hidden, child)) = m.mods.get(s) {
                if !public && i >= strict {
                    return Reach::Private(format!("module {here} is not public"));
                }
                if *hidden && hidden_by.is_none() {
                    hidden_by = Some(format!("module {here} is #[doc(hidden)]"));
                }
                m = child;
                i += 1;
                continue;
            }
            if let Some(item) = m.items.get(s) {
                if !item.public && i >= strict {
                    return Reach::Private(format!("{here} is not public"));
                }
                if item.hidden {
                    return Reach::Hidden(format!("{here} is #[doc(hidden)]"));
                }
                if let Some(next) = segs.get(i + 1)
                    && !item.variants.contains(next)
                    && let Some((public, hidden)) = k.methods.get(&(s.clone(), next.clone()))
                {
                    let what = format!("{here}::{next}");
                    if !public && i + 1 >= strict {
                        return Reach::Private(format!("{what} is not public"));
                    }
                    if *hidden {
                        return Reach::Hidden(format!("{what} is #[doc(hidden)]"));
                    }
                }
                return hidden_by.map_or(Reach::Public, Reach::Hidden);
            }
            if let Some(r) = m.reexports.get(s) {
                if !r.public && i >= strict {
                    return Reach::Private(format!("{here} is a private import"));
                }
                if r.hidden {
                    return Reach::Hidden(format!("{here} is a #[doc(hidden)] re-export"));
                }
                if r.target.is_empty() || !self.crates.contains_key(&r.target[0]) {
                    return hidden_by.map_or(Reach::Public, Reach::Hidden); // external: its own surface
                }
                let mut t = r.target.clone();
                let from = if i + 1 >= strict {
                    t.len()
                } else {
                    t.len() + strict - i - 1
                };
                t.extend_from_slice(&segs[i + 1..]);
                let got = self.resolve_from(&t, depth + 1, from);
                return match (got, hidden_by) {
                    (Reach::Public, Some(h)) => Reach::Hidden(h),
                    (got, _) => got,
                };
            }
            for g in &m.globs {
                if (!g.public && i >= strict) || g.target.is_empty() {
                    continue;
                }
                let mut t = g.target.clone();
                let from = if i >= strict {
                    t.len()
                } else {
                    t.len() + strict - i
                };
                t.extend_from_slice(&segs[i..]);
                match self.resolve_from(&t, depth + 1, from) {
                    Reach::Unresolved => {}
                    got => return got,
                }
            }
            return Reach::Unresolved;
        }
        hidden_by.map_or(Reach::Public, Reach::Hidden)
    }

    /// Names of `#[doc(hidden)]` inherent methods (reachable by method-call syntax).
    fn hidden_methods(&self) -> BTreeSet<String> {
        let mut hidden = BTreeSet::new();
        let mut visible = BTreeSet::new();
        for k in self.crates.values() {
            for ((_, name), (public, h)) in &k.methods {
                if *public && *h {
                    hidden.insert(name.clone());
                } else if *public {
                    visible.insert(name.clone());
                }
            }
        }
        hidden.difference(&visible).cloned().collect()
    }
}

// ---- the workspace -------------------------------------------------------------------------

fn root() -> PathBuf {
    forge_tests::workspace_root()
}

fn package_name(toml: &str) -> Option<String> {
    let v: toml::Table = toml::from_str(toml).ok()?;
    Some(v.get("package")?.get("name")?.as_str()?.to_string())
}

fn crates_in(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.filter_map(Result::ok) {
        let d = e.path();
        if let Some(name) = std::fs::read_to_string(d.join("Cargo.toml"))
            .ok()
            .and_then(|t| package_name(&t))
        {
            out.push((name.replace('-', "_"), d));
        }
    }
    out.sort();
    out
}

fn kernel_dirs(root: &Path) -> Vec<(String, PathBuf)> {
    crates_in(&root.join("crates"))
}

fn plugin_dirs(root: &Path) -> Vec<(String, PathBuf)> {
    crates_in(&root.join("plugins"))
}

// ---- scanning a plugin --------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Violation {
    file: String,
    what: String,
    why: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} — {}", self.file, self.what, self.why)
    }
}

#[derive(Default)]
struct Scan {
    violations: Vec<Violation>,
    /// Kernel references resolved to the public surface.
    checked: usize,
    unresolved: BTreeSet<String>,
}

struct Visitor<'k> {
    kernel: &'k Kernel,
    hidden_methods: &'k BTreeSet<String>,
    names: &'k BTreeSet<String>,
    file: PathBuf,
    rel: String,
    kernel_src: PathBuf,
    aliases: BTreeMap<String, Vec<String>>,
    scan: &'k mut Scan,
}

impl Visitor<'_> {
    fn flag(&mut self, what: String, why: String) {
        self.scan.violations.push(Violation {
            file: self.rel.clone(),
            what,
            why,
        });
    }

    fn check_path(&mut self, segs: Vec<String>) {
        let Some(first) = segs.first() else { return };
        let full = if self.names.contains(first) {
            segs
        } else if let Some(a) = self.aliases.get(first) {
            let mut v = a.clone();
            v.extend_from_slice(&segs[1..]);
            v
        } else {
            return;
        };
        if !self.names.contains(&full[0]) {
            return;
        }
        let text = full.join("::");
        if full
            .last()
            .is_some_and(|l| l.ends_with("_for_tests") || l.ends_with("_for_control"))
        {
            self.flag(text.clone(), "a test-only kernel item".into());
        }
        match self.kernel.resolve(&full, 0) {
            Reach::Public => self.scan.checked += 1,
            Reach::Private(why) => self.flag(text, why),
            Reach::Hidden(why) => self.flag(text, why),
            Reach::Unresolved => {
                self.scan.unresolved.insert(text);
            }
        }
        if matches!(
            full.as_slice(),
            [k, .., t, f] if k == "forge_plugin" && ((t == "PluginId" && f == "new") || (t == "Registry" && f == "new"))
        ) {
            self.flag(
                full.join("::"),
                "a plugin registers only through the loader's InstallCx, never with an identity or registry of its own".into(),
            );
        }
    }

    fn inside_kernel(&self, p: &Path) -> bool {
        let norm = |p: &Path| -> PathBuf {
            let mut out = PathBuf::new();
            for c in p.components() {
                match c {
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => out.push(other),
                }
            }
            out
        };
        norm(p).starts_with(norm(&self.kernel_src))
    }
}

fn skip(attrs: &[syn::Attribute]) -> bool {
    is_cfg_test(attrs)
}

impl<'ast> Visit<'ast> for Visitor<'_> {
    fn visit_item(&mut self, i: &'ast syn::Item) {
        let attrs: &[syn::Attribute] = match i {
            syn::Item::Fn(x) => &x.attrs,
            syn::Item::Mod(x) => &x.attrs,
            syn::Item::Impl(x) => &x.attrs,
            syn::Item::Use(x) => &x.attrs,
            syn::Item::Struct(x) => &x.attrs,
            syn::Item::Enum(x) => &x.attrs,
            syn::Item::Const(x) => &x.attrs,
            syn::Item::Static(x) => &x.attrs,
            syn::Item::Trait(x) => &x.attrs,
            _ => &[],
        };
        if skip(attrs) {
            return;
        }
        syn::visit::visit_item(self, i);
    }

    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        if let Some(p) = path_attr(&m.attrs) {
            let target = self.file.parent().map(|d| d.join(&p)).unwrap_or_default();
            if self.inside_kernel(&target) {
                self.flag(
                    format!("#[path = {p:?}] mod {}", m.ident),
                    "includes kernel source into the plugin".into(),
                );
            }
        }
        syn::visit::visit_item_mod(self, m);
    }

    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        let mut flat = Vec::new();
        flatten(&u.tree, &mut Vec::new(), &mut flat);
        for (name, full) in flat {
            let full = match full.first().map(String::as_str) {
                Some(f) if !self.names.contains(f) => match self.aliases.get(f) {
                    Some(a) => {
                        let mut v = a.clone();
                        v.extend_from_slice(&full[1..]);
                        v
                    }
                    None => continue,
                },
                _ => full,
            };
            if name != "*" && name != "_" {
                self.aliases.insert(name, full.clone());
            }
            self.check_path(full);
        }
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        self.check_path(segs);
        syn::visit::visit_path(self, p);
    }

    fn visit_expr_method_call(&mut self, c: &'ast syn::ExprMethodCall) {
        let name = c.method.to_string();
        if self.hidden_methods.contains(&name)
            || name.ends_with("_for_tests")
            || name.ends_with("_for_control")
        {
            self.flag(
                format!(".{name}()"),
                "calls a #[doc(hidden)] kernel method".into(),
            );
        }
        if name == "registry_mut" {
            self.flag(
                ".registry_mut()".into(),
                "a plugin changes registries only through the loader's InstallCx".into(),
            );
        }
        syn::visit::visit_expr_method_call(self, c);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        let name = m
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        if matches!(name.as_str(), "include" | "include_str" | "include_bytes")
            && let Ok(lit) = m.parse_body::<syn::LitStr>()
        {
            let target = self
                .file
                .parent()
                .map(|d| d.join(lit.value()))
                .unwrap_or_default();
            if self.inside_kernel(&target) {
                self.flag(
                    format!("{name}!({:?})", lit.value()),
                    "includes kernel source into the plugin".into(),
                );
            }
        }
        // Paths inside macro bodies that parse as expressions (format args, vec!, assert!).
        if let Ok(args) = m.parse_body_with(
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
        ) {
            for e in &args {
                self.visit_expr(e);
            }
        }
        syn::visit::visit_macro(self, m);
    }
}

/// Scan one plugin's non-test sources (`files`: relative path -> text, the crate root first).
fn scan_sources(
    kernel: &Kernel,
    names: &BTreeSet<String>,
    root: &Path,
    plugin_dir: &Path,
    files: &BTreeMap<PathBuf, String>,
) -> Scan {
    let hidden_methods = kernel.hidden_methods();
    let mut scan = Scan::default();
    for (path, text) in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        let parsed = match syn::parse_file(text) {
            Ok(p) => p,
            Err(e) => {
                scan.violations.push(Violation {
                    file: rel,
                    what: "parse".into(),
                    why: e.to_string(),
                });
                continue;
            }
        };
        let mut v = Visitor {
            kernel,
            hidden_methods: &hidden_methods,
            names,
            file: path.clone(),
            rel,
            kernel_src: root.join("crates"),
            aliases: BTreeMap::new(),
            scan: &mut scan,
        };
        v.visit_file(&parsed);
    }
    let _ = plugin_dir;
    scan
}

/// Every `.rs` file of a plugin's library, reached from `src/lib.rs` through its `mod`s
/// (test modules are skipped when scanned).
fn plugin_files(dir: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(f: &Path, out: &mut BTreeMap<PathBuf, String>) {
        let Ok(text) = std::fs::read_to_string(f) else {
            return;
        };
        if let Ok(parsed) = syn::parse_file(&text) {
            for it in &parsed.items {
                if let syn::Item::Mod(m) = it
                    && m.content.is_none()
                    && !is_cfg_test(&m.attrs)
                    && let Some(child) =
                        mod_file(f, &m.ident.to_string(), path_attr(&m.attrs).as_deref())
                {
                    walk(&child, out);
                }
            }
        }
        out.insert(f.to_path_buf(), text);
    }
    let mut out = BTreeMap::new();
    walk(&dir.join("src").join("lib.rs"), &mut out);
    out
}

/// A plugin's `Cargo.toml`: no privileged kernel feature.
fn check_manifest(toml_text: &str, names: &BTreeSet<String>, rel: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    let Ok(v) = toml::from_str::<toml::Table>(toml_text) else {
        return vec![Violation {
            file: rel.into(),
            what: "Cargo.toml".into(),
            why: "does not parse".into(),
        }];
    };
    for table in ["dependencies", "build-dependencies"] {
        let Some(deps) = v.get(table).and_then(toml::Value::as_table) else {
            continue;
        };
        for (dep, spec) in deps {
            if !names.contains(&dep.replace('-', "_")) {
                continue;
            }
            let feats = spec
                .get("features")
                .and_then(toml::Value::as_array)
                .map(|a| a.iter().filter_map(|f| f.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            for f in feats {
                if ["mutate-", "internal", "private", "test"]
                    .iter()
                    .any(|p| f.starts_with(p))
                {
                    out.push(Violation {
                        file: rel.into(),
                        what: format!("{dep} feature {f:?}"),
                        why: "enables a privileged kernel feature".into(),
                    });
                }
            }
        }
    }
    out
}

struct World {
    root: PathBuf,
    names: BTreeSet<String>,
    kernel: Kernel,
}

fn world() -> World {
    let root = root();
    let names: BTreeSet<String> = kernel_dirs(&root).into_iter().map(|(n, _)| n).collect();
    let kernel = Kernel::load(&root, &names);
    World {
        root,
        names,
        kernel,
    }
}

fn scan_plugin(w: &World, dir: &Path, files: &BTreeMap<PathBuf, String>, toml: &str) -> Scan {
    let mut s = scan_sources(&w.kernel, &w.names, &w.root, dir, files);
    let rel = dir
        .strip_prefix(&w.root)
        .unwrap_or(dir)
        .join("Cargo.toml")
        .display()
        .to_string()
        .replace('\\', "/");
    s.violations.extend(check_manifest(toml, &w.names, &rel));
    s
}

// ---- the static half ------------------------------------------------------------------------

/// Reasoned exceptions: `(file prefix, what the violation names, why it grants nothing)`. An
/// entry that matches nothing fails, so the list cannot rot.
const ALLOW: &[(&str, &str, &str)] = &[(
    "plugins/forge-panels-",
    "forge_editor::services::PanelFaults",
    "W2 fault-injection switches the panel guards' positive controls set; a fault only degrades a panel (never grants it anything), and the type is `pub`, so a third-party panel can read it exactly as a first-party one does",
)];

fn allowed(v: &Violation) -> Option<usize> {
    ALLOW
        .iter()
        .position(|(file, what, _)| v.file.starts_with(file) && v.what == *what)
}

#[test]
fn no_first_party_plugin_reaches_past_the_kernels_public_surface() {
    let w = world();
    let plugins = plugin_dirs(&w.root);
    assert!(
        plugins.len() >= 3,
        "M2-14: at least three first-party plugins"
    );
    let mut all = Vec::new();
    let mut checked = 0;
    let mut unresolved = BTreeSet::new();
    for (name, dir) in &plugins {
        let files = plugin_files(dir);
        assert!(!files.is_empty(), "{name}: no sources");
        let toml = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
        let s = scan_plugin(&w, dir, &files, &toml);
        println!(
            "{name}: {} kernel references on the public surface, {} unresolved",
            s.checked,
            s.unresolved.len()
        );
        checked += s.checked;
        unresolved.extend(s.unresolved);
        all.extend(s.violations);
        // A plugin crate is a real plugin: it implements SourcePlugin.
        let is_plugin = files.values().any(|t| {
            t.contains("impl SourcePlugin for") || t.contains("impl forge_plugin::SourcePlugin for")
        });
        assert!(
            is_plugin,
            "{name} is in plugins/ but implements no SourcePlugin"
        );
    }
    let mut used = vec![false; ALLOW.len()];
    let bad: Vec<&Violation> = all
        .iter()
        .filter(|v| match allowed(v) {
            Some(i) => {
                used[i] = true;
                false
            }
            None => true,
        })
        .collect();
    assert!(
        bad.is_empty(),
        "I16: first-party plugins use capabilities a third-party plugin cannot:\n{}",
        bad.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    for (i, u) in used.iter().enumerate() {
        assert!(
            *u,
            "allow-list entry {:?} matches nothing: remove it",
            ALLOW[i].1
        );
        assert!(
            ALLOW[i].2.len() > 20,
            "allow-list entry {:?} needs a reason",
            ALLOW[i].1
        );
    }
    // Non-vacuous: the scan really resolved the plugins' use of the kernel.
    assert!(
        checked >= 300,
        "only {checked} kernel references were resolved"
    );
    println!("unresolved (macro-generated or unmodelled; not failures): {unresolved:?}");
}

#[test]
fn no_kernel_code_branches_on_a_plugin_being_first_party() {
    let w = world();
    let mut hits = Vec::new();
    for (_, dir) in kernel_dirs(&w.root) {
        for (path, text) in plugin_files(&dir) {
            let Ok(parsed) = syn::parse_file(&text) else {
                continue;
            };
            struct Find<'a>(&'a mut Vec<String>, String);
            impl<'ast> Visit<'ast> for Find<'_> {
                fn visit_item(&mut self, i: &'ast syn::Item) {
                    if let syn::Item::Mod(m) = i
                        && is_cfg_test(&m.attrs)
                    {
                        return;
                    }
                    syn::visit::visit_item(self, i);
                }
                fn visit_expr_method_call(&mut self, c: &'ast syn::ExprMethodCall) {
                    if c.method == "is_first_party" {
                        self.0.push(self.1.clone());
                    }
                    syn::visit::visit_expr_method_call(self, c);
                }
            }
            let mut f = Find(&mut hits, path.display().to_string());
            f.visit_file(&parsed);
        }
    }
    assert!(
        hits.is_empty(),
        "kernel code branches on first-party identity (I16): {hits:?}"
    );
}

// ---- positive controls: each back door, injected into a real plugin --------------------------

fn back_doors() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "a #[doc(hidden)] kernel item",
            "fn _door() { let _ = forge_plugin::PluginError::all_variants_for_tests(); }",
            "all_variants_for_tests",
        ),
        (
            "a pub(crate) kernel item",
            "fn _door() { let _ = forge_plugin::registry::panic_message; }",
            "forge_plugin::registry",
        ),
        (
            "a private kernel module through an alias",
            "use forge_store::local as door; fn _door() { let _ = door::LocalFs::open; }",
            "forge_store::local",
        ),
        (
            "kernel source by #[path]",
            "#[path = \"../../../crates/forge-store/src/memory.rs\"] mod door;",
            "#[path",
        ),
        (
            "kernel source by include!",
            "mod door { include!(\"../../../crates/forge-plugin/src/id.rs\"); }",
            "include!",
        ),
        (
            "a minted identity",
            "fn _door() { let _ = forge_plugin::PluginId::new(\"forge.store\"); }",
            "PluginId::new",
        ),
        (
            "a mutable registry outside install",
            "fn _door(x: &mut forge_plugin::Extensions) { let _ = x.registry_mut::<forge_store::StoreBackend>(); }",
            "registry_mut",
        ),
    ]
}

#[test]
fn positive_control_each_back_door_is_caught() {
    let w = world();
    let dir = w.root.join("plugins").join("forge-store-backends");
    let files = plugin_files(&dir);
    let toml = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
    let clean = scan_plugin(&w, &dir, &files, &toml);
    assert!(
        clean.violations.is_empty(),
        "the real plugin is clean: {:?}",
        clean.violations
    );
    let lib = dir.join("src").join("lib.rs");
    for (what, door, expect) in back_doors() {
        let mut mutant = files.clone();
        let text = mutant.get(&lib).cloned().unwrap_or_default();
        mutant.insert(lib.clone(), format!("{text}\n{door}\n"));
        let s = scan_plugin(&w, &dir, &mutant, &toml);
        assert!(
            s.violations.iter().any(|v| v.to_string().contains(expect)),
            "{what}: the scan missed `{door}` (found {:?})",
            s.violations
        );
    }
    // A privileged kernel feature in the plugin's Cargo.toml.
    let mutant_toml = format!(
        "{toml}\n[dependencies.forge-num]\npath = \"../../crates/forge-num\"\nfeatures = [\"mutate-det\"]\n"
    );
    let s = scan_plugin(&w, &dir, &files, &mutant_toml);
    assert!(
        s.violations.iter().any(|v| v.what.contains("mutate-det")),
        "a privileged feature was missed: {:?}",
        s.violations
    );
}

#[test]
fn negative_control_a_test_module_and_a_same_named_local_are_not_references() {
    let w = world();
    let dir = w.root.join("plugins").join("forge-store-backends");
    let mut files = plugin_files(&dir);
    let lib = dir.join("src").join("lib.rs");
    let text = files.get(&lib).cloned().unwrap_or_default();
    files.insert(
        lib,
        format!(
            "{text}\n#[cfg(test)]\nmod tests {{ fn _kit() {{ let _ = forge_plugin::PluginError::all_variants_for_tests(); }} }}\n\
             mod registry {{ pub fn panic_message() {{}} }}\nfn _local() {{ registry::panic_message(); }}\n"
        ),
    );
    let toml = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
    let s = scan_plugin(&w, &dir, &files, &toml);
    assert!(s.violations.is_empty(), "{:?}", s.violations);
}

// ---- the runtime half: the public loader only, and no privilege by id -----------------------

/// A plugin loaded under another id with the same declarations.
struct Renamed<'a> {
    inner: &'a dyn SourcePlugin,
    manifest: Manifest,
}

impl SourcePlugin for Renamed<'_> {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        self.inner.install(cx)
    }
}

fn renamed<'a>(p: &'a dyn SourcePlugin, id: &str) -> Renamed<'a> {
    let mut manifest = p.manifest().clone();
    manifest.id = PluginId::new(id).unwrap_or_else(|e| panic!("{e}"));
    Renamed { inner: p, manifest }
}

/// What a load put in every registry: point -> (key -> owner).
fn contents(x: &Extensions, report_ids: &[PluginId]) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (point, k, prov) in x.inventory() {
        let owner = prov.owner.to_string();
        assert!(
            report_ids.contains(&prov.owner),
            "{}({k:?}) is owned by {owner}, which the loader did not install",
            point.name
        );
        out.entry(point.name.to_string())
            .or_default()
            .insert(k, owner);
    }
    out
}

type Fresh = fn() -> Extensions;

fn asset_points() -> Extensions {
    let mut x = Extensions::new();
    forge_asset::AssetServer::define_points(&mut x).unwrap_or_else(|e| panic!("{e}"));
    x
}

fn preset_point() -> Extensions {
    let mut x = Extensions::new();
    x.define::<forge_plugin::points::Preset>()
        .unwrap_or_else(|e| panic!("{e}"));
    x
}

fn store_point() -> Extensions {
    let mut x = Extensions::new();
    x.define::<forge_store::StoreBackend>()
        .unwrap_or_else(|e| panic!("{e}"));
    x
}

fn editor_points() -> Extensions {
    forge_editor::editor_extensions().unwrap_or_else(|e| panic!("{e}"))
}

/// Load `p` through the public loader, then the same plugin as a third party; the two
/// registries must be identical but for the owner's name.
fn equivalent_under_a_third_party_id(p: &dyn SourcePlugin, fresh: Fresh) -> Result<usize, String> {
    let mut a = fresh();
    let ra = loader::load(&mut a, &[p], &[], &Grants::new()).map_err(|e| e.to_string())?;
    let third = format!(
        "com.thirdparty.{}",
        p.manifest().id.as_str().replace('.', "-")
    );
    let clone = renamed(p, &third);
    let mut b = fresh();
    let rb = loader::load(&mut b, &[&clone], &[], &Grants::new()).map_err(|e| e.to_string())?;
    let ca = contents(&a, &ra.installed);
    let cb = contents(&b, &rb.installed);
    let strip =
        |c: &BTreeMap<String, BTreeMap<String, String>>| -> BTreeMap<String, BTreeSet<String>> {
            c.iter()
                .map(|(pt, m)| (pt.clone(), m.keys().cloned().collect()))
                .collect()
        };
    if strip(&ca) != strip(&cb) {
        return Err(format!(
            "{} installs {:?} as itself but {:?} as {third}",
            p.manifest().id,
            strip(&ca),
            strip(&cb)
        ));
    }
    let n: usize = ca.values().map(BTreeMap::len).sum();
    for m in ca.values() {
        for owner in m.values() {
            if owner != p.manifest().id.as_str() {
                return Err(format!(
                    "an item is owned by {owner}, not {}",
                    p.manifest().id
                ));
            }
        }
    }
    Ok(n)
}

#[test]
fn first_party_plugins_load_through_the_public_loader_and_gain_nothing_by_their_id() {
    let importers = forge_importers::Importers::new().unwrap_or_else(|e| panic!("{e}"));
    let presets = forge_presets::BuiltinPresets::new().unwrap_or_else(|e| panic!("{e}"));
    let stores = forge_store_backends::StoreBackends::new().unwrap_or_else(|e| panic!("{e}"));
    let types = forge_asset::FirstPartyAssets::new().unwrap_or_else(|e| panic!("{e}"));
    let core = forge_panels_core::PanelsCore::new().unwrap_or_else(|e| panic!("{e}"));
    let scene = forge_panels_scene::PanelsScene::new().unwrap_or_else(|e| panic!("{e}"));
    let assets = forge_panels_assets::PanelsAssets::new().unwrap_or_else(|e| panic!("{e}"));
    let domain = forge_panels_domain::PanelsDomain::new().unwrap_or_else(|e| panic!("{e}"));
    let project = forge_panels_project::PanelsProject::new().unwrap_or_else(|e| panic!("{e}"));
    let authoring =
        forge_panels_authoring::PanelsAuthoring::new().unwrap_or_else(|e| panic!("{e}"));
    let cases: [(&dyn SourcePlugin, Fresh, usize); 10] = [
        // The three subsystems M2-14 moved into plugins/, and the kernel's asset types.
        (&importers, asset_points, 4),
        (
            &presets,
            preset_point,
            forge_editor::presets::BUILTIN_PRESETS.len(),
        ),
        (&stores, store_point, 2),
        (&types, asset_points, 4),
        // The first-party panel sets (WP-U3..U13).
        (&core, editor_points, 1),
        (&scene, editor_points, 1),
        (&assets, editor_points, 1),
        (&domain, editor_points, 1),
        (&project, editor_points, 1),
        (&authoring, editor_points, 1),
    ];
    for (p, fresh, at_least) in cases {
        let n = equivalent_under_a_third_party_id(p, fresh).unwrap_or_else(|e| panic!("I16: {e}"));
        assert!(
            n >= at_least,
            "{} installed {n} items through the loader, expected at least {at_least}",
            p.manifest().id
        );
    }
}

/// A plugin that installs one more item when it is first-party.
struct Sly(Manifest);

impl SourcePlugin for Sly {
    fn manifest(&self) -> &Manifest {
        &self.0
    }
    fn install(&self, cx: &mut InstallCx) -> Result<(), PluginError> {
        use forge_plugin::points::{Preset, PresetDescriptor, PresetKind};
        let d = || PresetDescriptor {
            label: "x".into(),
            kind: PresetKind::TwoD,
            defaults: BTreeMap::new(),
            files: BTreeMap::new(),
        };
        cx.add::<Preset>("sly.a", d(), forge_plugin::Order::Last)?;
        if cx.plugin().as_str().starts_with("forge.") {
            cx.add::<Preset>("sly.b", d(), forge_plugin::Order::Last)?;
        }
        Ok(())
    }
}

#[test]
fn positive_control_a_plugin_privileged_by_its_id_is_caught() {
    let m = Manifest::source("forge.sly", "0.1.0", "^0.1")
        .unwrap_or_else(|e| panic!("{e}"))
        .provides("Preset", "sly.a")
        .provides("Preset", "sly.b");
    let e = equivalent_under_a_third_party_id(&Sly(m), preset_point)
        .expect_err("a plugin privileged by its id must fail the equivalence");
    assert!(e.contains("sly.b"), "{e}");
}

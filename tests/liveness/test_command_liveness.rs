//! I7 — `test_command_liveness` (Ch.7.3, Ch.21.18, W3): **every mutation of project state
//! is a command on the bus; the UI has no privileged path.**
//!
//! `forge-cmd` makes the in-memory project unreachable by type (`Project`'s mutators are
//! crate-private; only the `Bus` holds one). What types cannot stop is an editor item that
//! names some *other* project-state writer: the ECS `World`, or the filesystem directly
//! (the project store is I17's trait, reached through commands). This guard walks the AST of
//! every item in `forge-editor`, in the `forge-editor` binary (`tools/forge-editor-bin`) and
//! in every panel plugin (`plugins/forge-panels-*`) and every crate that declares itself a
//! client of the core in its manifest (`[package.metadata.forge] editor-client = true`: a
//! crate hosting sessions over the core), and resolves every path it names
//! **through the import graph**
//! — `use` aliases, glob imports, `pub use` re-export chains inside the crate and across
//! workspace crates, `crate::`/`self::`/`super::`, fn-local `use`, paths inside macro
//! arguments and closures — to its canonical definition, and fails on any reference to a
//! mutator that is not on the reasoned allow-list (`tests/liveness/i7_allow.txt`: the
//! session-state and user-config writers of Ch.21.18). A name match is never a reference: a
//! local `struct World` or a method called `write` is not flagged.
//!
//! Positive controls (W2):
//! * `positive_control_each_bypass_spelling_is_caught` — ten spellings of a direct write, each
//!   in its own fixture module, each must be flagged with the right mutator;
//! * `positive_control_mutant_handler_in_forge_editor_fails` — the mutation build the plan
//!   names: a UI handler that writes state directly is injected into forge-editor's real
//!   sources (into the scaffold fixture while forge-editor does not exist yet), and the guard
//!   must go from green to red on it.
//!
//! * `positive_control_mutant_panel_writing_the_store_fails` — the plan's §21.23 mutant: a
//!   panel handler in `forge-panels-core` that writes through `ProjectStore` directly.
//! * `positive_control_a_declared_client_with_a_privileged_path_fails` — a crate declared a
//!   client in its manifest is checked (and one undeclared is not): its bus-holding and
//!   file-writing mutant is caught.
//!
//! **The split (Ch.7.1, Ch.34 §34.2).** A second rule keeps the UI a client: outside
//! `forge_editor::core` no editor item may name the bus itself (`forge_cmd::Bus`,
//! `CommandSink`) — the UI reaches the core only through the `BusClient` and the `Applied`
//! stream, which is what makes the split editor a transport swap — and `forge_editor::core`
//! may not name `forge_ui` (the core is headless). Positive controls:
//! `positive_control_a_panel_holding_the_bus_fails` and
//! `positive_control_a_core_naming_the_ui_fails`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::visit::Visit;

/// Canonical paths of project-state writers. A reference to one of these, or to anything
/// under it (`World::spawn`), from forge-editor is a violation unless allow-listed.
const MUTATORS: &[&str] = &[
    // The ECS world: live project/game state. The editor never holds one (Ch.21.18).
    "forge_core::world::World",
    // Direct filesystem writes: project files change through the store (I17) via commands.
    "std::fs::write",
    "std::fs::remove_file",
    "std::fs::remove_dir",
    "std::fs::remove_dir_all",
    "std::fs::rename",
    "std::fs::copy",
    "std::fs::create_dir",
    "std::fs::create_dir_all",
    "std::fs::hard_link",
    "std::fs::set_permissions",
    "std::fs::File::create",
    "std::fs::File::create_new",
    "std::fs::File::options",
    "std::fs::OpenOptions",
    // The project store (I17): project files change through commands; the store is the
    // core's, never a panel's. Naming the trait (a `&mut dyn ProjectStore` parameter, UFCS
    // `ProjectStore::write`) or a concrete store is the privileged path.
    "forge_store::store::ProjectStore",
    "forge_store::local::LocalFs",
    "forge_store::memory::MemoryStore",
    // The project's store host (WP-U7, E-36): it creates, saves, pushes and pulls through
    // the store. Only the headless core may hold it; a panel naming it is a privileged path.
    "forge_project::host::ProjectHost",
    // The team server and the identity database, write side (WP-U10, Ch.37): publishing,
    // claims, the review queue and team management change the team's state; only the core
    // performs them, for a command it checked. Panels get the read-only view traits.
    "forge_project::collab::CollabBackend",
    "forge_project::collab::IdentityBackend",
];

/// The mutators that live in the workspace (checked to name a real item: a guard whose
/// target does not exist reports green forever).
const WORKSPACE_MUTATORS: &[&str] = &[
    "forge_core::world::World",
    "forge_store::store::ProjectStore",
    "forge_store::local::LocalFs",
    "forge_store::memory::MemoryStore",
    "forge_project::host::ProjectHost",
    "forge_project::collab::CollabBackend",
    "forge_project::collab::IdentityBackend",
];

const EDITOR: &str = "forge_editor";

/// A crate declares itself a client of the editor core — a crate that hosts sessions over the
/// core, such as a protocol server, is one more client like the UI, with no privileged path
/// — with this in its `Cargo.toml`; the guard then checks it like the panels.
const CLIENT_METADATA: &str = "[package.metadata.forge]";

/// Whether a `Cargo.toml` declares its crate a client of the editor core
/// (`[package.metadata.forge]` with `editor-client = true`).
fn declares_client(toml: &str) -> bool {
    let mut in_table = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_table = l == CLIENT_METADATA;
            continue;
        }
        if in_table
            && let Some((k, v)) = l.split_once('=')
            && k.trim() == "editor-client"
        {
            return v.trim() == "true";
        }
    }
    false
}

/// Every crate the guard checks: the editor library, the binary, every panel plugin, and
/// every crate that declares itself a client of the core ([`declares_client`]).
fn editor_crates(ws: &Workspace) -> Vec<String> {
    ws.crates
        .iter()
        .filter(|(k, c)| {
            *k == EDITOR || *k == "forge_editor_bin" || k.starts_with("forge_panels_") || c.client
        })
        .map(|(k, _)| k.clone())
        .collect()
}

/// Named only by `forge_editor::core` (the split rule).
const CORE_ONLY: &[&str] = &["forge_cmd::bus::Bus", "forge_cmd::bus::CommandSink"];
/// The core may not name the UI layer.
const UI_ROOT: &str = "forge_ui";

// ==== source loading ========================================================================

trait Source {
    fn read(&self, rel: &str) -> Option<String>;
}

struct Disk(PathBuf);

impl Source for Disk {
    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(rel)).ok()
    }
}

struct Mem(BTreeMap<String, String>);

impl Source for Mem {
    fn read(&self, rel: &str) -> Option<String> {
        self.0.get(rel).cloned()
    }
}

type ModPath = Vec<String>;

#[derive(Default)]
struct Module {
    file: String,
    /// Non-module items defined here.
    defs: BTreeSet<String>,
    /// Child modules.
    mods: BTreeSet<String>,
    /// `use` aliases: name -> path as written.
    uses: BTreeMap<String, Vec<String>>,
    /// Glob imports, as written.
    globs: Vec<Vec<String>>,
    /// The items, for visiting.
    items: Vec<syn::Item>,
}

struct Crate {
    modules: BTreeMap<ModPath, Module>,
    /// Declared a client of the editor core in its manifest ([`declares_client`]).
    client: bool,
}

#[derive(Default)]
struct Workspace {
    crates: BTreeMap<String, Crate>,
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.parse_args::<syn::Meta>()
                .map(|m| m.path().is_ident("test"))
                .unwrap_or(false)
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

/// Flatten a use tree into (alias, path) pairs and glob paths.
fn flatten_use(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    uses: &mut Vec<(String, Vec<String>)>,
    globs: &mut Vec<Vec<String>>,
) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten_use(&p.tree, prefix, uses, globs);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if let Some(last) = prefix.last() {
                    uses.push((last.clone(), prefix.clone()));
                }
            } else {
                let mut p = prefix.clone();
                p.push(name.clone());
                uses.push((name, p));
            }
        }
        syn::UseTree::Rename(r) => {
            let alias = r.rename.to_string();
            if alias == "_" {
                return;
            }
            let mut p = prefix.clone();
            if r.ident != "self" {
                p.push(r.ident.to_string());
            }
            uses.push((alias, p));
        }
        syn::UseTree::Glob(_) => globs.push(prefix.clone()),
        syn::UseTree::Group(g) => {
            for t in &g.items {
                flatten_use(t, prefix, uses, globs);
            }
        }
    }
}

fn use_prefix(u: &syn::ItemUse) -> Vec<String> {
    if u.leading_colon.is_some() {
        vec!["::".to_string()]
    } else {
        Vec::new()
    }
}

fn load_crate(src: &dyn Source, root_file: &str) -> Result<Crate, String> {
    let mut krate = Crate {
        modules: BTreeMap::new(),
        client: false,
    };
    let text = src
        .read(root_file)
        .ok_or_else(|| format!("cannot read {root_file}"))?;
    let file = syn::parse_file(&text).map_err(|e| format!("{root_file}: {e}"))?;
    let dir = parent_dir(root_file);
    load_module(src, &mut krate, Vec::new(), root_file, &dir, file.items)?;
    Ok(krate)
}

fn parent_dir(file: &str) -> String {
    Path::new(file)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn load_module(
    src: &dyn Source,
    krate: &mut Crate,
    path: ModPath,
    file: &str,
    child_dir: &str,
    items: Vec<syn::Item>,
) -> Result<(), String> {
    let mut m = Module {
        file: file.to_string(),
        ..Module::default()
    };
    let mut children: Vec<(String, String, String, Vec<syn::Item>)> = Vec::new();
    let mut kept = Vec::new();
    for item in items {
        let attrs: &[syn::Attribute] = match &item {
            syn::Item::Mod(x) => &x.attrs,
            syn::Item::Fn(x) => &x.attrs,
            syn::Item::Impl(x) => &x.attrs,
            syn::Item::Use(x) => &x.attrs,
            syn::Item::Struct(x) => &x.attrs,
            syn::Item::Enum(x) => &x.attrs,
            _ => &[],
        };
        if is_cfg_test(attrs) {
            continue;
        }
        match &item {
            syn::Item::Mod(md) => {
                let name = md.ident.to_string();
                m.mods.insert(name.clone());
                if let Some((_, inner)) = &md.content {
                    children.push((
                        name.clone(),
                        file.to_string(),
                        join(child_dir, &name),
                        inner.clone(),
                    ));
                } else {
                    let (rel, dir) = if let Some(p) = path_attr(&md.attrs) {
                        let rel = join(&parent_dir(file), &p);
                        let dir = parent_dir(&rel);
                        (rel, join(&dir, &name))
                    } else {
                        let flat = join(child_dir, &format!("{name}.rs"));
                        if src.read(&flat).is_some() {
                            (flat, join(child_dir, &name))
                        } else {
                            (
                                join(child_dir, &format!("{name}/mod.rs")),
                                join(child_dir, &name),
                            )
                        }
                    };
                    let text = src
                        .read(&rel)
                        .ok_or_else(|| format!("{file}: `mod {name};` but {rel} is missing"))?;
                    let parsed = syn::parse_file(&text).map_err(|e| format!("{rel}: {e}"))?;
                    children.push((name.clone(), rel, dir, parsed.items));
                }
                continue;
            }
            syn::Item::Use(u) => {
                let mut uses = Vec::new();
                let mut prefix = use_prefix(u);
                flatten_use(&u.tree, &mut prefix, &mut uses, &mut m.globs);
                m.uses.extend(uses);
                continue;
            }
            syn::Item::ExternCrate(ec) => {
                let name = ec.ident.to_string();
                let alias = ec
                    .rename
                    .as_ref()
                    .map_or(name.clone(), |(_, r)| r.to_string());
                m.uses.insert(alias, vec![name]);
                continue;
            }
            syn::Item::Fn(x) => {
                m.defs.insert(x.sig.ident.to_string());
            }
            syn::Item::Struct(x) => {
                m.defs.insert(x.ident.to_string());
            }
            syn::Item::Enum(x) => {
                m.defs.insert(x.ident.to_string());
            }
            syn::Item::Trait(x) => {
                m.defs.insert(x.ident.to_string());
            }
            syn::Item::Type(x) => {
                m.defs.insert(x.ident.to_string());
            }
            syn::Item::Const(x) => {
                m.defs.insert(x.ident.to_string());
            }
            syn::Item::Static(x) => {
                m.defs.insert(x.ident.to_string());
            }
            syn::Item::Union(x) => {
                m.defs.insert(x.ident.to_string());
            }
            syn::Item::TraitAlias(x) => {
                m.defs.insert(x.ident.to_string());
            }
            syn::Item::Macro(x) => {
                if let Some(i) = &x.ident {
                    m.defs.insert(i.to_string());
                }
            }
            _ => {}
        }
        kept.push(item);
    }
    m.items = kept;
    krate.modules.insert(path.clone(), m);
    for (name, f, dir, items) in children {
        let mut p = path.clone();
        p.push(name);
        load_module(src, krate, p, &f, &dir, items)?;
    }
    Ok(())
}

/// The package name from a `Cargo.toml` (`[package] name = "..."`).
fn package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_package = l == "[package]";
            continue;
        }
        if in_package
            && let Some(rest) = l.strip_prefix("name")
            && let Some(v) = rest.trim_start().strip_prefix('=')
        {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn workspace_root() -> PathBuf {
    forge_tests::workspace_root()
}

/// Every workspace crate under `crates/*`, `tools/*`, `plugins/*` (D-6), by crate ident.
fn load_workspace(root: &Path) -> Result<Workspace, String> {
    let mut ws = Workspace::default();
    for group in ["crates", "tools", "plugins", "samples"] {
        let Ok(rd) = std::fs::read_dir(root.join(group)) else {
            continue;
        };
        for entry in rd.flatten() {
            let dir = entry.path();
            let Ok(toml) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
                continue;
            };
            let Some(name) = package_name(&toml) else {
                continue;
            };
            let src = Disk(dir.clone());
            let root_file = if dir.join("src/lib.rs").exists() {
                "src/lib.rs"
            } else if dir.join("src/main.rs").exists() {
                "src/main.rs"
            } else {
                continue;
            };
            let mut krate = load_crate(&src, root_file).map_err(|e| format!("{name}: {e}"))?;
            krate.client = declares_client(&toml);
            ws.crates.insert(name.replace('-', "_"), krate);
        }
    }
    Ok(ws)
}

// ==== resolution ============================================================================

const MAX_DEPTH: usize = 32;

#[derive(Default)]
struct Locals {
    uses: BTreeMap<String, Vec<String>>,
    globs: Vec<Vec<String>>,
}

struct Resolver<'w> {
    ws: &'w Workspace,
}

fn canon(krate: &str, mp: &[String], rest: &[String]) -> String {
    let mut parts = vec![krate.to_string()];
    parts.extend(mp.iter().cloned());
    parts.extend(rest.iter().cloned());
    parts.join("::")
}

fn cat(a: &[String], b: &[String]) -> Vec<String> {
    a.iter().chain(b.iter()).cloned().collect()
}

impl Resolver<'_> {
    fn module(&self, krate: &str, mp: &[String]) -> Option<&Module> {
        self.ws.crates.get(krate)?.modules.get(mp)
    }

    fn is_std(name: &str) -> bool {
        matches!(name, "std" | "core" | "alloc")
    }

    /// A path whose first segment is a crate name.
    fn resolve_extern(&self, segs: &[String], depth: usize) -> Option<String> {
        let first = segs.first()?;
        if self.ws.crates.contains_key(first.as_str()) {
            return self.resolve_in_module(first, &[], &segs[1..], depth + 1);
        }
        if Self::is_std(first) {
            return Some(cat(&["std".to_string()], &segs[1..]).join("::"));
        }
        None
    }

    /// Split a canonical module path back into (crate, module path) if it is a workspace
    /// module.
    fn as_workspace_module(&self, canonical: &str) -> Option<(String, ModPath)> {
        let mut parts = canonical.split("::").map(str::to_string);
        let krate = parts.next()?;
        let mp: ModPath = parts.collect();
        self.module(&krate, &mp)?;
        Some((krate, mp))
    }

    /// Resolve `segs` as written in module `mp` of `krate`, with fn-local `use`s on top.
    fn resolve(
        &self,
        krate: &str,
        mp: &[String],
        locals: &Locals,
        segs: &[String],
    ) -> Option<String> {
        let first = segs.first()?;
        if let Some(p) = locals.uses.get(first) {
            return self.resolve_scoped(krate, mp, &cat(p, &segs[1..]), 1);
        }
        if let Some(r) = self.resolve_scoped(krate, mp, segs, 0) {
            return Some(r);
        }
        self.try_globs(krate, mp, &locals.globs, segs, 1)
    }

    fn resolve_scoped(
        &self,
        krate: &str,
        mp: &[String],
        segs: &[String],
        depth: usize,
    ) -> Option<String> {
        if depth > MAX_DEPTH || segs.is_empty() {
            return None;
        }
        match segs[0].as_str() {
            "::" => return self.resolve_extern(&segs[1..], depth),
            "crate" => return self.resolve_in_module(krate, &[], &segs[1..], depth),
            "self" => return self.resolve_in_module(krate, mp, &segs[1..], depth),
            "super" => {
                let mut up = mp.to_vec();
                let mut i = 0;
                while segs.get(i).map(String::as_str) == Some("super") {
                    up.pop()?;
                    i += 1;
                }
                return self.resolve_in_module(krate, &up, &segs[i..], depth);
            }
            "Self" => return None,
            _ => {}
        }
        let m = self.module(krate, mp)?;
        let first = &segs[0];
        if m.defs.contains(first) || m.mods.contains(first) {
            return self.resolve_in_module(krate, mp, segs, depth);
        }
        if let Some(p) = m.uses.get(first) {
            return self.resolve_scoped(krate, mp, &cat(p, &segs[1..]), depth + 1);
        }
        // Glob imports shadow the extern prelude; workspace globs are checked strictly.
        if let Some(r) = self.try_workspace_globs(krate, mp, &m.globs, segs, depth) {
            return Some(r);
        }
        if let Some(r) = self.resolve_extern(segs, depth) {
            return Some(r);
        }
        self.try_std_globs(krate, mp, &m.globs, segs, depth)
    }

    fn try_globs(
        &self,
        krate: &str,
        mp: &[String],
        globs: &[Vec<String>],
        segs: &[String],
        depth: usize,
    ) -> Option<String> {
        self.try_workspace_globs(krate, mp, globs, segs, depth)
            .or_else(|| self.try_std_globs(krate, mp, globs, segs, depth))
    }

    fn try_workspace_globs(
        &self,
        krate: &str,
        mp: &[String],
        globs: &[Vec<String>],
        segs: &[String],
        depth: usize,
    ) -> Option<String> {
        for g in globs {
            let Some(target) = self.resolve_scoped(krate, mp, g, depth + 1) else {
                continue;
            };
            if let Some((k, tmp)) = self.as_workspace_module(&target)
                && let Some(r) = self.resolve_in_module_strict(&k, &tmp, segs, depth + 1)
            {
                return Some(r);
            }
        }
        None
    }

    fn try_std_globs(
        &self,
        krate: &str,
        mp: &[String],
        globs: &[Vec<String>],
        segs: &[String],
        depth: usize,
    ) -> Option<String> {
        for g in globs {
            let Some(target) = self.resolve_scoped(krate, mp, g, depth + 1) else {
                continue;
            };
            if target.starts_with("std::") || target == "std" {
                return Some(format!("{target}::{}", segs.join("::")));
            }
        }
        None
    }

    /// Like `resolve_in_module`, but the first segment must be declared in the module (not
    /// merely reachable through the module's own globs of the extern prelude).
    fn resolve_in_module_strict(
        &self,
        krate: &str,
        mp: &[String],
        segs: &[String],
        depth: usize,
    ) -> Option<String> {
        let m = self.module(krate, mp)?;
        let first = segs.first()?;
        if m.defs.contains(first) || m.mods.contains(first) || m.uses.contains_key(first) {
            return self.resolve_in_module(krate, mp, segs, depth);
        }
        self.try_workspace_globs(krate, mp, &m.globs, segs, depth + 1)
    }

    fn resolve_in_module(
        &self,
        krate: &str,
        mp: &[String],
        segs: &[String],
        depth: usize,
    ) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut cur = mp.to_vec();
        for (i, seg) in segs.iter().enumerate() {
            let m = self.module(krate, &cur)?;
            if m.mods.contains(seg) {
                cur.push(seg.clone());
                continue;
            }
            if m.defs.contains(seg) {
                return Some(canon(krate, &cur, &segs[i..]));
            }
            if let Some(p) = m.uses.get(seg) {
                return self.resolve_scoped(krate, &cur, &cat(p, &segs[i + 1..]), depth + 1);
            }
            return self.try_workspace_globs(krate, &cur, &m.globs, &segs[i..], depth + 1);
        }
        Some(canon(krate, &cur, &[]))
    }
}

// ==== the check =============================================================================

/// Collects every path an item names (signature, body, closures, macro arguments) and the
/// fn-local `use`s in scope.
#[derive(Default)]
struct Collector {
    paths: Vec<Vec<String>>,
    locals: Locals,
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_path(&mut self, p: &'ast syn::Path) {
        let mut segs: Vec<String> = Vec::new();
        if p.leading_colon.is_some() {
            segs.push("::".into());
        }
        segs.extend(p.segments.iter().map(|s| s.ident.to_string()));
        self.paths.push(segs);
        syn::visit::visit_path(self, p);
    }

    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        let mut uses = Vec::new();
        let mut prefix = use_prefix(u);
        flatten_use(&u.tree, &mut prefix, &mut uses, &mut self.locals.globs);
        self.locals.uses.extend(uses);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        syn::visit::visit_macro(self, m);
        let parser = Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
        if let Ok(args) = parser.parse2(m.tokens.clone()) {
            for e in &args {
                self.visit_expr(e);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Violation {
    /// The editor item (`forge_editor::panels::on_click`).
    item: String,
    /// The mutator it reaches.
    mutator: String,
    /// The path as written.
    written: String,
    /// The file.
    file: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}) names `{}`, which resolves to project-state writer `{}` — send a forge-cmd \
             command instead (I7)",
            self.item, self.file, self.written, self.mutator
        )
    }
}

fn mutator_of(canonical: &str) -> Option<&'static str> {
    MUTATORS
        .iter()
        .copied()
        .find(|m| canonical == *m || canonical.starts_with(&format!("{m}::")))
}

fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map_or_else(|| "?".into(), |s| s.ident.to_string()),
        _ => "?".into(),
    }
}

/// Visits one unit's syntax into a collector.
type UnitVisit<'a> = Box<dyn Fn(&mut Collector) + 'a>;

/// Every (unit name, syntax node) pair to check in one module.
fn units(m: &Module) -> Vec<(String, UnitVisit<'_>)> {
    let mut out: Vec<(String, UnitVisit<'_>)> = Vec::new();
    for item in &m.items {
        match item {
            syn::Item::Impl(imp) => {
                let ty = type_name(&imp.self_ty);
                out.push((
                    format!("impl {ty}"),
                    Box::new(move |c: &mut Collector| {
                        c.visit_type(&imp.self_ty);
                        if let Some((_, tr, _)) = &imp.trait_ {
                            c.visit_path(tr);
                        }
                        c.visit_generics(&imp.generics);
                    }),
                ));
                for ii in &imp.items {
                    let name = match ii {
                        syn::ImplItem::Fn(f) => f.sig.ident.to_string(),
                        syn::ImplItem::Const(k) => k.ident.to_string(),
                        syn::ImplItem::Type(t) => t.ident.to_string(),
                        _ => "?".into(),
                    };
                    out.push((
                        format!("{ty}::{name}"),
                        Box::new(move |c: &mut Collector| c.visit_impl_item(ii)),
                    ));
                }
            }
            other => {
                let name = match other {
                    syn::Item::Fn(f) => f.sig.ident.to_string(),
                    syn::Item::Struct(s) => s.ident.to_string(),
                    syn::Item::Enum(e) => e.ident.to_string(),
                    syn::Item::Trait(t) => t.ident.to_string(),
                    syn::Item::Type(t) => t.ident.to_string(),
                    syn::Item::Const(k) => k.ident.to_string(),
                    syn::Item::Static(s) => s.ident.to_string(),
                    syn::Item::Union(u) => u.ident.to_string(),
                    syn::Item::Macro(mm) => mm
                        .ident
                        .as_ref()
                        .map_or_else(|| "macro".into(), ToString::to_string),
                    _ => "item".into(),
                };
                out.push((name, Box::new(move |c: &mut Collector| c.visit_item(other))));
            }
        }
    }
    out
}

/// All references from `krate` to a mutator.
fn check(ws: &Workspace, krate: &str) -> Vec<Violation> {
    let r = Resolver { ws };
    let mut out = BTreeSet::new();
    let Some(c) = ws.crates.get(krate) else {
        return Vec::new();
    };
    for (mp, m) in &c.modules {
        for (name, visit) in units(m) {
            let mut col = Collector::default();
            visit(&mut col);
            let item = canon(krate, mp, std::slice::from_ref(&name));
            for p in &col.paths {
                if let Some(canonical) = r.resolve(krate, mp, &col.locals, p)
                    && let Some(mutator) = mutator_of(&canonical)
                {
                    out.insert(Violation {
                        item: item.clone(),
                        mutator: mutator.to_string(),
                        written: p.join("::"),
                        file: m.file.clone(),
                    });
                }
            }
        }
    }
    out.into_iter().collect()
}

/// The split rule (see the module docs): the bus only in `forge_editor::core`, no UI there.
fn check_split(ws: &Workspace, krate: &str) -> Vec<Violation> {
    let r = Resolver { ws };
    let mut out = BTreeSet::new();
    let Some(c) = ws.crates.get(krate) else {
        return Vec::new();
    };
    for (mp, m) in &c.modules {
        let in_core = krate == EDITOR && mp.first().map(String::as_str) == Some("core");
        for (name, visit) in units(m) {
            let mut col = Collector::default();
            visit(&mut col);
            let item = canon(krate, mp, std::slice::from_ref(&name));
            for p in &col.paths {
                let Some(canonical) = r.resolve(krate, mp, &col.locals, p) else {
                    continue;
                };
                let hit = if in_core {
                    (canonical == UI_ROOT || canonical.starts_with(&format!("{UI_ROOT}::")))
                        .then(|| format!("{UI_ROOT} (the core is headless)"))
                } else {
                    CORE_ONLY
                        .iter()
                        .find(|b| canonical == **b || canonical.starts_with(&format!("{b}::")))
                        .map(|b| format!("{b} (only forge_editor::core holds the bus)"))
                };
                if let Some(mutator) = hit {
                    out.insert(Violation {
                        item: item.clone(),
                        mutator,
                        written: p.join("::"),
                        file: m.file.clone(),
                    });
                }
            }
        }
    }
    out.into_iter().collect()
}

// ==== the allow-list ========================================================================

#[derive(Debug)]
struct Allow {
    item: String,
    mutator: String,
}

fn allow_list() -> Vec<Allow> {
    let path = workspace_root().join("tests/liveness/i7_allow.txt");
    let text = std::fs::read_to_string(&path).expect("tests/liveness/i7_allow.txt exists");
    parse_allow(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `item | mutator | reason` per line; `#` comments. Every field is required: an allow-list
/// entry without a reason is refused (W3: the list is explicit, never implied).
fn parse_allow(text: &str) -> Result<Vec<Allow>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = l.split('|').map(str::trim).collect();
        if f.len() != 3 || f.iter().any(|x| x.is_empty()) {
            return Err(format!(
                "line {}: expected `item | mutator | reason`",
                n + 1
            ));
        }
        if mutator_of(f[1]) != Some(f[1]) {
            return Err(format!(
                "line {}: `{}` is not a listed mutator",
                n + 1,
                f[1]
            ));
        }
        out.push(Allow {
            item: f[0].to_string(),
            mutator: f[1].to_string(),
        });
    }
    Ok(out)
}

/// Violations not covered by the allow-list, and allow-list entries that cover nothing.
fn apply_allow(v: Vec<Violation>, allow: &[Allow]) -> (Vec<Violation>, Vec<String>) {
    let covered = |x: &Violation| {
        allow
            .iter()
            .any(|a| a.item == x.item && a.mutator == x.mutator)
    };
    let stale = allow
        .iter()
        .filter(|a| !v.iter().any(|x| x.item == a.item && x.mutator == a.mutator))
        .map(|a| format!("{} | {}", a.item, a.mutator))
        .collect();
    (v.into_iter().filter(|x| !covered(x)).collect(), stale)
}

// ==== fixtures ==============================================================================

/// A small editor crate that reaches project state only through the bus. It deliberately
/// contains name-alikes that are **not** references: a local `struct World`, a method named
/// `write`, a read-only fs call, a string mentioning `forge_core::World`.
fn clean_editor() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "src/lib.rs".into(),
        r#"
//! Scaffold editor (fixture).
pub mod panels;
pub mod prelude {
    pub use forge_cmd::{Bus, CommandSink, EditorCommand, Issuer};
}
/// Not the ECS world: a local type that happens to share the name.
pub struct World { pub zoom: f64 }
pub fn zoom(w: &mut World) { w.zoom += 1.0; }
pub fn note() -> &'static str { "never call forge_core::World directly" }
"#
        .into(),
    );
    m.insert(
        "src/panels.rs".into(),
        r#"
use crate::prelude::*;
use std::io::Write;

pub fn on_rename(bus: &mut Bus, entity: forge_cmd::EntityKey) {
    let e = bus.envelope(Issuer::Human { user: "u".into() }, EditorCommand::Rename { entity, name: "n".into() });
    let _ = bus.apply(e);
}

pub fn on_log(buf: &mut Vec<u8>) {
    let _ = buf.write(b"x");
    let _ = std::fs::read_to_string("config.ron");
}

pub struct Inspector { pub selected: Option<forge_cmd::EntityKey>, pub id: forge_core::EntityId }
impl Inspector {
    pub fn select(&mut self, e: forge_cmd::EntityKey) { self.selected = Some(e); }
}
"#
        .into(),
    );
    m
}

/// Add `mod <name>;` with `body` to an editor source map.
fn with_module(
    mut src: BTreeMap<String, String>,
    root: &str,
    name: &str,
    body: &str,
) -> BTreeMap<String, String> {
    let lib = src.get(root).cloned().unwrap_or_default();
    src.insert(root.into(), format!("{lib}\npub mod {name};\n"));
    src.insert(format!("src/{name}.rs"), body.into());
    src
}

/// The real workspace plus an editor crate from `editor` sources (replacing any real one),
/// plus `forge_fake`, a crate that re-exports the ECS world under another name.
fn workspace_with(editor: &BTreeMap<String, String>, root: &str) -> Workspace {
    let mut ws = load_workspace(&workspace_root()).expect("workspace parses");
    let krate = load_crate(&Mem(editor.clone()), root).expect("fixture editor parses");
    ws.crates.insert(EDITOR.into(), krate);
    let mut fake = BTreeMap::new();
    fake.insert(
        "src/lib.rs".to_string(),
        "pub mod scene { pub use forge_core::World as Handle; }\npub use scene::*;\n".to_string(),
    );
    ws.crates.insert(
        "forge_fake".into(),
        load_crate(&Mem(fake), "src/lib.rs").expect("fake"),
    );
    ws
}

/// Ten spellings of a UI handler writing project state directly, and the mutator each must
/// resolve to.
fn bypass_cases() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "alias",
            "use forge_core::World as Scene;\npub fn on_click(s: &mut Scene) { let _ = s; }\n",
            "forge_core::world::World",
        ),
        (
            "crate_alias",
            "use forge_core as fc;\npub fn on_click(s: &mut fc::World) { let _ = s; }\n",
            "forge_core::world::World",
        ),
        (
            "glob",
            "use forge_core::*;\npub fn on_click(s: &mut World) { let _ = s; }\n",
            "forge_core::world::World",
        ),
        (
            "reexport_chain",
            "mod inner { pub use forge_core::World as Stage; }\npub use inner::*;\n\
             pub fn on_click(s: &mut self::Stage) { let _ = s; }\n",
            "forge_core::world::World",
        ),
        (
            "cross_crate_reexport",
            "pub fn on_click(h: &mut forge_fake::Handle) { let _ = h; }\n",
            "forge_core::world::World",
        ),
        (
            "fs_module_alias",
            "use std::fs;\npub fn on_save() { let _ = fs::write(\"scene.ron\", b\"x\"); }\n",
            "std::fs::write",
        ),
        (
            "macro_and_closure",
            "pub fn on_delete() { let f = || { println!(\"{:?}\", ::std::fs::remove_file(\"a\")); }; f(); }\n",
            "std::fs::remove_file",
        ),
        (
            "std_glob",
            "use std::fs::*;\npub fn on_new() { let _ = File::create(\"x\"); }\n",
            "std::fs::File::create",
        ),
        (
            "struct_field",
            "pub struct Panel { pub world: forge_core::World }\n",
            "forge_core::world::World",
        ),
        (
            "fn_local_use_and_super",
            "pub mod deep { pub fn on_drag() { use super::super::prelude::Bus as _B; \
             use forge_core::World as W; let _x: Option<&mut W> = None; } }\n",
            "forge_core::world::World",
        ),
    ]
}

// ==== tests =================================================================================

#[test]
fn forge_editor_reaches_project_state_only_through_the_bus() {
    let allow = allow_list();
    let root = workspace_root();
    let ws = load_workspace(&root).expect("workspace parses");
    assert!(
        ws.crates.contains_key("forge_cmd") && ws.crates.contains_key("forge_core"),
        "the loader must see the crates it resolves against"
    );
    if !ws.crates.contains_key(EDITOR) {
        eprintln!(
            "UNBUILT: crates/forge-editor does not exist yet (WP-U4); the guard and its positive \
             controls run, and the real check runs on the crate the moment it exists"
        );
        assert!(
            allow.is_empty(),
            "allow-list entries without a forge-editor to check them against: {allow:?}"
        );
        return;
    }
    let crates = editor_crates(&ws);
    assert!(
        crates.iter().any(|c| c == "forge_editor_bin")
            && crates.iter().any(|c| c.starts_with("forge_panels_")),
        "the binary and the panel plugins are checked too: {crates:?}"
    );
    let all: Vec<Violation> = crates.iter().flat_map(|c| check(&ws, c)).collect();
    let (v, stale) = apply_allow(all, &allow);
    assert!(
        v.is_empty(),
        "I7 violated:\n{}",
        v.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        stale.is_empty(),
        "stale allow-list entries (remove them): {stale:?}"
    );
}

#[test]
fn the_ui_reaches_the_core_only_through_the_bus_client() {
    let ws = load_workspace(&workspace_root()).expect("workspace parses");
    let v: Vec<Violation> = editor_crates(&ws)
        .iter()
        .flat_map(|c| check_split(&ws, c))
        .collect();
    assert!(
        v.is_empty(),
        "the split is broken:\n{}",
        v.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    // Non-vacuous: the core module exists and does name the bus.
    let core = ws
        .crates
        .get(EDITOR)
        .and_then(|c| c.modules.get(&vec!["core".to_string()]))
        .expect("forge_editor::core exists");
    assert!(
        core.uses
            .values()
            .any(|p| p.last().is_some_and(|s| s == "Bus"))
    );
}

/// The real sources of a workspace crate directory, with `mutant` added as a module.
fn real_with_mutant(dir: &str, name: &str, body: &str) -> (BTreeMap<String, String>, &'static str) {
    let root = workspace_root().join(dir);
    let mut map = BTreeMap::new();
    collect_rs(&root, &root.join("src"), &mut map);
    let root_file = if map.contains_key("src/lib.rs") {
        "src/lib.rs"
    } else {
        "src/main.rs"
    };
    (with_module(map, root_file, name, body), root_file)
}

fn workspace_with_crate(name: &str, src: &BTreeMap<String, String>, root: &str) -> Workspace {
    let mut ws = load_workspace(&workspace_root()).expect("workspace parses");
    let krate = load_crate(&Mem(src.clone()), root).expect("mutant parses");
    ws.crates.insert(name.into(), krate);
    ws
}

#[test]
fn positive_control_mutant_panel_writing_the_store_fails() {
    let (src, root) = real_with_mutant(
        "plugins/forge-panels-core",
        "i7_store_mutant",
        "use forge_store::ProjectStore;\n\
         /// A panel handler that saves the scene itself instead of sending a command.\n\
         pub fn on_save(store: &mut dyn ProjectStore) { let _ = store; }\n",
    );
    let ws = workspace_with_crate("forge_panels_core", &src, root);
    let baseline = workspace_with_crate(
        "forge_panels_core",
        &real_with_mutant("plugins/forge-panels-core", "empty", "").0,
        root,
    );
    let (b, _) = apply_allow(check(&baseline, "forge_panels_core"), &allow_list());
    assert!(b.is_empty(), "the baseline is green: {b:#?}");
    let (v, _) = apply_allow(check(&ws, "forge_panels_core"), &allow_list());
    assert_eq!(
        v.iter()
            .map(|x| (x.item.as_str(), x.mutator.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            "forge_panels_core::i7_store_mutant::on_save",
            "forge_store::store::ProjectStore"
        )],
        "the store-writing panel handler must be the one violation"
    );
}

/// WP-U7: the project's store host is the core's alone. A panel that saves through it
/// (instead of sending `forge.project.save`) is a privileged path.
#[test]
fn positive_control_a_panel_holding_the_project_host_fails() {
    let (src, root) = real_with_mutant(
        "plugins/forge-panels-core",
        "i7_host_mutant",
        "use forge_project::host::ProjectHost;\n\
         /// A panel handler that saves through the core's store host itself.\n\
         pub fn on_save(host: &mut ProjectHost) { let _ = host; }\n",
    );
    let ws = workspace_with_crate("forge_panels_core", &src, root);
    let (v, _) = apply_allow(check(&ws, "forge_panels_core"), &allow_list());
    assert_eq!(
        v.iter()
            .map(|x| (x.item.as_str(), x.mutator.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            "forge_panels_core::i7_host_mutant::on_save",
            "forge_project::host::ProjectHost"
        )],
        "the panel holding the project host must be the one violation"
    );
}

#[test]
fn positive_control_a_panel_holding_the_bus_fails() {
    let (src, root) = real_with_mutant(
        "plugins/forge-panels-core",
        "split_mutant",
        "/// A panel that applies commands on the bus itself: a privileged path.\n\
         pub fn on_click(bus: &mut forge_cmd::Bus) { let _ = bus; }\n",
    );
    let ws = workspace_with_crate("forge_panels_core", &src, root);
    let v = check_split(&ws, "forge_panels_core");
    assert_eq!(
        v.iter().map(|x| x.item.as_str()).collect::<Vec<_>>(),
        vec!["forge_panels_core::split_mutant::on_click"],
        "{v:#?}"
    );
    assert!(v[0].mutator.starts_with("forge_cmd::bus::Bus"));
}

/// A crate that hosts sessions over the core (a protocol server a plugin brings) declares
/// itself a client in its manifest and is then checked like the editor: one that holds the
/// bus (a privileged path), or writes the project's files itself, must fail the guard. The
/// declaration is what selects it: the same mutant crate undeclared is not in the checked
/// set, declared it is, and its violations are found.
#[test]
fn positive_control_a_declared_client_with_a_privileged_path_fails() {
    const NAME: &str = "forge_seed";
    assert!(declares_client(
        "[package]\nname = \"x\"\n\n[package.metadata.forge]\neditor-client = true\n"
    ));
    assert!(!declares_client(
        "[package]\nname = \"x\"\n\n[package.metadata.forge]\neditor-client = false\n"
    ));
    assert!(!declares_client(
        "[package]\nname = \"x\"\n\n[dependencies]\neditor-client = true\n"
    ));
    let (src, root) = real_with_mutant(
        "crates/forge-seed",
        "i7_client_mutant",
        "/// A session host that applies its client's commands on the bus directly.\n\
         pub fn apply_fast(bus: &mut forge_cmd::Bus) { let _ = bus; }\n\
         /// A session host that saves the scene itself.\n\
         pub fn save(path: &str) { let _ = std::fs::write(path, b\"scene\"); }\n",
    );
    let mut ws = workspace_with_crate(NAME, &src, root);
    assert!(
        !editor_crates(&ws).iter().any(|c| c == NAME),
        "an undeclared kernel crate is not an editor client"
    );
    if let Some(k) = ws.crates.get_mut(NAME) {
        k.client = true;
    }
    assert!(editor_crates(&ws).iter().any(|c| c == NAME));
    let split = check_split(&ws, NAME);
    assert_eq!(
        split.iter().map(|x| x.item.as_str()).collect::<Vec<_>>(),
        vec!["forge_seed::i7_client_mutant::apply_fast"],
        "{split:#?}"
    );
    let writes = check(&ws, NAME);
    assert_eq!(
        writes
            .iter()
            .map(|x| (x.item.as_str(), x.mutator.as_str()))
            .collect::<Vec<_>>(),
        vec![("forge_seed::i7_client_mutant::save", "std::fs::write")],
        "{writes:#?}"
    );
    // The same sources without the mutant module are clean.
    let (clean, root) = real_with_mutant("crates/forge-seed", "empty", "");
    let mut ws = workspace_with_crate(NAME, &clean, root);
    if let Some(k) = ws.crates.get_mut(NAME) {
        k.client = true;
    }
    assert!(check_split(&ws, NAME).is_empty() && check(&ws, NAME).is_empty());
}

#[test]
fn positive_control_a_core_naming_the_ui_fails() {
    let root = workspace_root().join("crates/forge-editor");
    let mut map = BTreeMap::new();
    collect_rs(&root, &root.join("src"), &mut map);
    let core = map.get("src/core.rs").cloned().expect("core.rs");
    map.insert(
        "src/core.rs".into(),
        format!(
            "{core}\n/// The core drawing a window: not headless any more.\n\
             pub fn draw(ui: &mut forge_ui::Ui) {{ let _ = ui; }}\n"
        ),
    );
    let ws = workspace_with_crate(EDITOR, &map, "src/lib.rs");
    let v = check_split(&ws, EDITOR);
    assert_eq!(
        v.iter().map(|x| x.item.as_str()).collect::<Vec<_>>(),
        vec!["forge_editor::core::draw"],
        "{v:#?}"
    );
}

#[test]
fn every_workspace_mutator_names_a_real_item() {
    let ws = load_workspace(&workspace_root()).expect("workspace parses");
    let r = Resolver { ws: &ws };
    for m in WORKSPACE_MUTATORS {
        let segs: Vec<String> = m.split("::").map(str::to_string).collect();
        let (krate, rest) = segs.split_first().expect("non-empty");
        let (name, mp) = rest.split_last().expect("has an item");
        let module = r
            .module(krate, mp)
            .unwrap_or_else(|| panic!("{m}: no module"));
        assert!(
            module.defs.contains(name),
            "{m} is not defined: the guard would be vacuous"
        );
    }
    // And the public spelling resolves to it through the re-export.
    let got = r.resolve(
        "forge_core",
        &[],
        &Locals::default(),
        &["World".to_string()],
    );
    assert_eq!(got.as_deref(), Some("forge_core::world::World"));
    assert!(MUTATORS.contains(&"forge_core::world::World"));
}

#[test]
fn negative_control_a_name_match_is_not_a_reference() {
    let ws = workspace_with(&clean_editor(), "src/lib.rs");
    let v = check(&ws, EDITOR);
    assert!(v.is_empty(), "false positives: {v:#?}");
}

#[test]
fn positive_control_each_bypass_spelling_is_caught() {
    for (name, body, mutator) in bypass_cases() {
        let src = with_module(clean_editor(), "src/lib.rs", name, body);
        let ws = workspace_with(&src, "src/lib.rs");
        let v = check(&ws, EDITOR);
        assert!(
            v.iter()
                .any(|x| x.mutator == mutator && x.item.contains(name)),
            "bypass `{name}` was not caught as {mutator}: {v:#?}"
        );
        assert!(
            v.iter().all(|x| x.item.contains(name)),
            "the clean part of the fixture was flagged in case `{name}`: {v:#?}"
        );
    }
}

#[test]
fn positive_control_mutant_handler_in_forge_editor_fails() {
    // The plan's mutation build: a UI handler that writes state directly. Injected into the
    // real forge-editor sources when the crate exists; into the scaffold fixture until then.
    let root = workspace_root();
    let editor_dir = root.join("crates/forge-editor");
    let (sources, root_file) = if editor_dir.join("Cargo.toml").exists() {
        let mut map = BTreeMap::new();
        collect_rs(&editor_dir, &editor_dir.join("src"), &mut map);
        let root_file = if map.contains_key("src/lib.rs") {
            "src/lib.rs"
        } else {
            "src/main.rs"
        };
        (map, root_file)
    } else {
        (clean_editor(), "src/lib.rs")
    };
    let allow = allow_list();

    let baseline = workspace_with(&sources, root_file);
    let (v, _) = apply_allow(check(&baseline, EDITOR), &allow);
    assert!(
        v.is_empty(),
        "the baseline must be green for the control to mean anything: {v:#?}"
    );

    let mutant = with_module(
        sources,
        root_file,
        "i7_mutant",
        "use forge_core::World as Scene;\n\
         /// A button handler that edits the scene without the bus.\n\
         pub fn on_click(scene: &mut Scene) { let _ = scene; }\n",
    );
    let ws = workspace_with(&mutant, root_file);
    let (v, _) = apply_allow(check(&ws, EDITOR), &allow);
    assert_eq!(
        v.iter()
            .map(|x| (x.item.as_str(), x.mutator.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            "forge_editor::i7_mutant::on_click",
            "forge_core::world::World"
        )],
        "the injected direct-write handler must be the one violation"
    );
}

#[test]
fn allow_list_entries_need_a_reason_and_a_real_mutator() {
    assert!(parse_allow("a | std::fs::write | user config writer (Ch.21.18)").is_ok());
    assert!(parse_allow("a | std::fs::write |").is_err());
    assert!(parse_allow("a | std::fs::read | x").is_err());
    let v = vec![Violation {
        item: "forge_editor::config::save".into(),
        mutator: "std::fs::write".into(),
        written: "fs::write".into(),
        file: "src/config.rs".into(),
    }];
    let allow = parse_allow("forge_editor::config::save | std::fs::write | user config (Ch.21.18)")
        .expect("ok");
    let (left, stale) = apply_allow(v.clone(), &allow);
    assert!(left.is_empty() && stale.is_empty());
    let other = parse_allow("forge_editor::other | std::fs::write | reason").expect("ok");
    let (left, stale) = apply_allow(v, &other);
    assert_eq!(left.len(), 1);
    assert_eq!(stale.len(), 1);
}

fn collect_rs(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_rs(base, &p, out);
        } else if p.extension().is_some_and(|x| x == "rs")
            && let (Ok(rel), Ok(text)) = (p.strip_prefix(base), std::fs::read_to_string(&p))
        {
            out.insert(rel.to_string_lossy().replace('\\', "/"), text);
        }
    }
}

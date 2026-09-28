//! I1 — **no global position type exists.** Every position is `(FrameId, DVec3)`, spelled
//! `forge_frames::FramePos` (Ch.2.1–2.3).
//!
//! `test_no_bare_position` is an AST lint (syn) over every engine source file
//! (`crates/*/src`, `tools/*/src`, `plugins/*/src`, test modules excluded):
//!
//! * no public function, trait method or inherent method takes a bare position-shaped
//!   parameter, or returns a bare position from a position-shaped function name;
//! * no public field of a public struct or enum variant is a bare position-shaped field;
//! * no type named like a global position (`WorldPos`, `GlobalPosition`, ...) is defined.
//!
//! "Bare" means `DVec3` (or a local alias of it), `[f64; 3]` or `(f64, f64, f64)`, anywhere
//! inside the type (`Option<DVec3>`, `&[DVec3]`). "Position-shaped" is the name: a `_`-token
//! from [`SHAPED`]. The allow-list is `tests/liveness/i1_allow.txt`, plus an `i1_allow.txt` a
//! crate may carry at its root for its own items (so a crate that is not in every edition's
//! tree keeps its entries with it): short, every entry with a reason, an entry that matches
//! nothing fails (so it cannot rot), and a crate's file names only that crate's items.
//!
//! **Positive controls (W2):** synthetic sources exercising every rule must be flagged, and
//! a real engine file with one bad function appended must be flagged.

// A test harness: its helpers panic on a broken fixture by design (the no-unwrap rule of
// Ch.1.2 governs engine code; clippy exempts only `#[test]` bodies, not their helpers).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use forge_tests::workspace_root;
use quote::ToTokens;
use syn::visit::Visit;

const ALLOW_FILE: &str = "tests/liveness/i1_allow.txt";
const SCANNED_ROOTS: [&str; 4] = ["crates", "tools", "plugins", "samples"];

/// Name tokens that make a parameter, field or function position-shaped.
const SHAPED: &[&str] = &[
    "pos",
    "position",
    "positions",
    "point",
    "points",
    "location",
    "locations",
    "loc",
    "origin",
    "center",
    "centre",
    "coord",
    "coords",
    "coordinate",
    "coordinates",
    "world",
    "global",
    "translation",
    "place",
    // A frame-local position: the `local` half of a `FramePos` handed out on its own (WP-09:
    // `camera_local() -> DVec3` slipped past the list before this token was added).
    "local",
];

/// Type names that would be a global position type (compared case-insensitively).
const GLOBAL_TYPES: &[&str] = &[
    "worldpos",
    "worldposition",
    "globalpos",
    "globalposition",
    "absolutepos",
    "absoluteposition",
];

fn shaped(name: &str) -> bool {
    let n = name.trim_start_matches("r#").to_ascii_lowercase();
    n.split('_').any(|t| SHAPED.contains(&t))
}

/// Is `ty` (or anything inside it) a bare position type?
fn bare(ty: &syn::Type, aliases: &BTreeSet<String>) -> bool {
    use syn::{GenericArgument, PathArguments, Type};
    let is_f64 = |t: &Type| matches!(t, Type::Path(p) if p.path.is_ident("f64"));
    match ty {
        Type::Path(p) => p.path.segments.last().is_some_and(|seg| {
            let id = seg.ident.to_string();
            id == "DVec3"
                // The 2D pipeline's positions are FramePos2 (Ch.35 §35.3, WP-U15).
                || id == "DVec2"
                || aliases.contains(&id)
                || match &seg.arguments {
                    PathArguments::AngleBracketed(a) => a.args.iter().any(|g| match g {
                        GenericArgument::Type(t) => bare(t, aliases),
                        _ => false,
                    }),
                    _ => false,
                }
        }),
        Type::Array(a) => {
            let three = matches!(&a.len, syn::Expr::Lit(l)
                if matches!(&l.lit, syn::Lit::Int(i) if i.base10_digits() == "3"));
            (three && is_f64(&a.elem)) || bare(&a.elem, aliases)
        }
        Type::Tuple(t) => {
            (t.elems.len() == 3 && t.elems.iter().all(is_f64))
                || t.elems.iter().any(|e| bare(e, aliases))
        }
        Type::Reference(r) => bare(&r.elem, aliases),
        Type::Slice(s) => bare(&s.elem, aliases),
        Type::Paren(p) => bare(&p.elem, aliases),
        Type::Group(g) => bare(&g.elem, aliases),
        Type::Ptr(p) => bare(&p.elem, aliases),
        _ => false,
    }
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().is_ident("cfg") && a.meta.to_token_stream().to_string().contains("test"))
}

fn is_pub(v: &syn::Visibility) -> bool {
    matches!(v, syn::Visibility::Public(_))
}

/// Local names bound to a bare position type: `type P = DVec3;`, `use x::DVec3 as P;`.
fn collect_aliases(files: &[(String, syn::File)]) -> BTreeSet<String> {
    struct V<'a>(&'a mut BTreeSet<String>);
    impl<'ast> Visit<'ast> for V<'_> {
        fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
            if bare(&i.ty, &BTreeSet::new()) {
                self.0.insert(i.ident.to_string());
            }
        }
        fn visit_use_rename(&mut self, r: &'ast syn::UseRename) {
            if r.ident == "DVec3" || r.ident == "DVec2" {
                self.0.insert(r.rename.to_string());
            }
        }
    }
    let mut out = BTreeSet::new();
    // Aliases of aliases: iterate to a fixed point (bounded by the alias count).
    loop {
        let before = out.len();
        let mut found = out.clone();
        for (_, f) in files {
            V(&mut found).visit_file(f);
            for item in &f.items {
                if let syn::Item::Type(t) = item
                    && bare(&t.ty, &out)
                {
                    found.insert(t.ident.to_string());
                }
            }
        }
        out = found;
        if out.len() == before {
            return out;
        }
    }
}

/// One lint finding: `id` is what the allow-list names, `what` explains it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Violation {
    id: String,
    what: String,
}

struct Lint<'a> {
    file: &'a str,
    aliases: &'a BTreeSet<String>,
    out: Vec<Violation>,
}

impl Lint<'_> {
    fn push(&mut self, item: String, what: String) {
        self.out.push(Violation {
            id: format!("{}::{item}", self.file),
            what,
        });
    }

    fn sig(&mut self, owner: Option<String>, sig: &syn::Signature) {
        let name = sig.ident.to_string();
        let q = owner.map_or(name.clone(), |o| format!("{o}::{name}"));
        for arg in &sig.inputs {
            let syn::FnArg::Typed(pt) = arg else { continue };
            let syn::Pat::Ident(pi) = &*pt.pat else {
                continue;
            };
            let pname = pi.ident.to_string();
            if shaped(&pname) && bare(&pt.ty, self.aliases) {
                self.push(
                    format!("{q}({pname})"),
                    format!(
                        "parameter `{pname}: {}` is a bare position — take a FramePos",
                        pt.ty.to_token_stream()
                    ),
                );
            }
        }
        if let syn::ReturnType::Type(_, ty) = &sig.output
            && shaped(&name)
            && bare(ty, self.aliases)
        {
            self.push(
                format!("{q}->"),
                format!(
                    "`{name}` returns a bare position `{}` — return a FramePos",
                    ty.to_token_stream()
                ),
            );
        }
    }

    fn fields(&mut self, owner: &str, fields: &syn::Fields, force_pub: bool) {
        for f in fields {
            let Some(id) = &f.ident else { continue };
            let n = id.to_string();
            if (force_pub || is_pub(&f.vis)) && shaped(&n) && bare(&f.ty, self.aliases) {
                self.push(
                    format!("{owner}.{n}"),
                    format!(
                        "public field `{n}: {}` is a bare position — use a FramePos",
                        f.ty.to_token_stream()
                    ),
                );
            }
        }
    }

    fn type_name(&mut self, ident: &syn::Ident) {
        let n = ident.to_string();
        if GLOBAL_TYPES.contains(&n.to_ascii_lowercase().as_str()) {
            self.push(
                format!("type {n}"),
                format!("`{n}` is a global position type — I1 says none exists; use FramePos"),
            );
        }
    }
}

impl<'ast> Visit<'ast> for Lint<'_> {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        if !is_cfg_test(&m.attrs) {
            syn::visit::visit_item_mod(self, m);
        }
    }
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if is_pub(&f.vis) && !is_cfg_test(&f.attrs) {
            self.sig(None, &f.sig);
        }
    }
    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if i.trait_.is_some() || is_cfg_test(&i.attrs) {
            return; // trait impls are governed by the trait's declaration
        }
        let owner = i.self_ty.to_token_stream().to_string().replace(' ', "");
        for item in &i.items {
            if let syn::ImplItem::Fn(f) = item
                && is_pub(&f.vis)
            {
                self.sig(Some(owner.clone()), &f.sig);
            }
        }
    }
    fn visit_item_trait(&mut self, t: &'ast syn::ItemTrait) {
        self.type_name(&t.ident);
        if !is_pub(&t.vis) {
            return;
        }
        for item in &t.items {
            if let syn::TraitItem::Fn(f) = item {
                self.sig(Some(t.ident.to_string()), &f.sig);
            }
        }
    }
    fn visit_item_struct(&mut self, s: &'ast syn::ItemStruct) {
        self.type_name(&s.ident);
        if is_pub(&s.vis) {
            self.fields(&s.ident.to_string(), &s.fields, false);
        }
    }
    fn visit_item_enum(&mut self, e: &'ast syn::ItemEnum) {
        self.type_name(&e.ident);
        if is_pub(&e.vis) {
            for v in &e.variants {
                self.fields(&format!("{}::{}", e.ident, v.ident), &v.fields, true);
            }
        }
    }
    fn visit_item_union(&mut self, u: &'ast syn::ItemUnion) {
        self.type_name(&u.ident);
    }
    fn visit_item_type(&mut self, t: &'ast syn::ItemType) {
        self.type_name(&t.ident);
    }
}

fn parse(rel: &str, src: &str) -> Result<syn::File, String> {
    syn::parse_file(src).map_err(|e| format!("{rel}: does not parse: {e}"))
}

/// Lint parsed files against each other's aliases.
fn lint(files: &[(String, syn::File)]) -> Vec<Violation> {
    let aliases = collect_aliases(files);
    let mut out = Vec::new();
    for (rel, f) in files {
        let mut l = Lint {
            file: rel,
            aliases: &aliases,
            out: Vec::new(),
        };
        l.visit_file(f);
        out.extend(l.out);
    }
    out.sort();
    out
}

/// Every engine source file, as `(workspace-relative path, source)`.
fn engine_sources(root: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut paths = Vec::new();
    for top in SCANNED_ROOTS {
        let Ok(rd) = std::fs::read_dir(root.join(top)) else {
            continue;
        };
        for krate in rd.flatten() {
            walk(&krate.path().join("src"), &mut paths);
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{rel}: {e}"));
            (rel, src)
        })
        .collect()
}

/// Parse the allow-list: `id  # reason` per line; `#` lines and blanks are comments.
fn parse_allow(text: &str) -> Result<BTreeMap<String, String>, String> {
    parse_allow_file(ALLOW_FILE, text)
}

/// [`parse_allow`] for the allow-list file `file`.
fn parse_allow_file(file: &str, text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (id, reason) = line
            .split_once('#')
            .map(|(a, b)| (a.trim(), b.trim()))
            .unwrap_or((line, ""));
        if reason.is_empty() {
            return Err(format!("{file}:{}: `{id}` has no reason", n + 1));
        }
        if out.insert(id.to_string(), reason.to_string()).is_some() {
            return Err(format!("{file}:{}: `{id}` listed twice", n + 1));
        }
    }
    Ok(out)
}

/// Violations not on the allow-list, and allow-list entries that match nothing.
fn judge(found: &[Violation], allow: &BTreeMap<String, String>) -> Result<(), String> {
    let hit: BTreeSet<&str> = found.iter().map(|v| v.id.as_str()).collect();
    let mut errs: Vec<String> = found
        .iter()
        .filter(|v| !allow.contains_key(&v.id))
        .map(|v| format!("{}: {}", v.id, v.what))
        .collect();
    errs.extend(
        allow
            .keys()
            .filter(|k| !hit.contains(k.as_str()))
            .map(|k| format!("{ALLOW_FILE}: stale entry `{k}` matches nothing — remove it")),
    );
    if errs.is_empty() {
        Ok(())
    } else {
        Err(format!("I1 violations:\n  {}", errs.join("\n  ")))
    }
}

/// Every allow-list: the repository's ([`ALLOW_FILE`]) and each crate's own `i1_allow.txt`
/// (whose entries must name that crate's files), merged.
fn allow_lists(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let text =
        std::fs::read_to_string(root.join(ALLOW_FILE)).map_err(|e| format!("{ALLOW_FILE}: {e}"))?;
    let mut all = parse_allow(&text)?;
    for top in SCANNED_ROOTS {
        let Ok(rd) = std::fs::read_dir(root.join(top)) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let Ok(text) = std::fs::read_to_string(dir.join("i1_allow.txt")) else {
                continue;
            };
            let krate = format!(
                "{top}/{}",
                dir.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            );
            let file = format!("{krate}/i1_allow.txt");
            for (id, reason) in parse_allow_file(&file, &text)? {
                if !id.starts_with(&format!("{krate}/")) {
                    return Err(format!("{file}: `{id}` is not an item of {krate}"));
                }
                if all.insert(id.clone(), reason).is_some() {
                    return Err(format!(
                        "{file}: `{id}` is also listed in another allow-list"
                    ));
                }
            }
        }
    }
    Ok(all)
}

fn workspace_files() -> Vec<(String, syn::File)> {
    engine_sources(&workspace_root())
        .into_iter()
        .map(|(rel, src)| {
            let f = parse(&rel, &src).unwrap_or_else(|e| panic!("{e}"));
            (rel, f)
        })
        .collect()
}

#[test]
fn test_no_bare_position() {
    let files = workspace_files();
    let names: Vec<&str> = files.iter().map(|(r, _)| r.as_str()).collect();
    assert!(
        names.contains(&"crates/forge-frames/src/lib.rs")
            && names.contains(&"crates/forge-num/src/linalg.rs"),
        "the scan did not reach the engine crates (vacuous): {names:?}"
    );
    let allow = allow_lists(&workspace_root()).unwrap_or_else(|e| panic!("{e}"));
    if let Err(e) = judge(&lint(&files), &allow) {
        panic!("{e}");
    }
}

// ---- positive controls --------------------------------------------------------------------

fn lint_src(src: &str) -> Vec<String> {
    let files = vec![("x.rs".to_string(), parse("x.rs", src).unwrap())];
    lint(&files).into_iter().map(|v| v.id).collect()
}

#[test]
fn positive_control_every_rule_flags_its_violation() {
    let ids = lint_src(
        r#"
        use forge_num::DVec3;
        use forge_num::DVec3 as Pt;
        type Where = [f64; 3];
        type Where2 = Where;
        pub fn teleport(pos: DVec3) {}
        pub fn spawn_at(spawn_point: &Option<DVec3>, v: DVec3) {}
        pub fn aim(target_position: Pt) {}
        pub fn place(world_coords: (f64, f64, f64)) {}
        pub fn drop_at(location: Where2) {}
        pub fn origin() -> DVec3 { DVec3::ZERO }
        pub struct View;
        impl View { pub fn camera_local(&self) -> DVec3 { DVec3::ZERO } }
        pub fn fit(camera_local: DVec3) {}
        pub struct Ship { pub position: DVec3, velocity: DVec3, pub centre: [f64; 3] }
        pub enum Marker { At { point: DVec3 } }
        pub trait Mover { fn move_to(&self, pos: DVec3); fn world_position(&self) -> DVec3; }
        pub struct Body;
        impl Body { pub fn set_position(&mut self, position: DVec3) {} }
        pub struct WorldPos;
        type GlobalPosition = u8;
        "#,
    );
    let want = [
        "x.rs::teleport(pos)",
        "x.rs::spawn_at(spawn_point)",
        "x.rs::aim(target_position)",
        "x.rs::place(world_coords)",
        "x.rs::drop_at(location)",
        "x.rs::origin->",
        "x.rs::View::camera_local->",
        "x.rs::fit(camera_local)",
        "x.rs::Ship.position",
        "x.rs::Ship.centre",
        "x.rs::Marker::At.point",
        "x.rs::Mover::move_to(pos)",
        "x.rs::Mover::world_position->",
        "x.rs::Body::set_position(position)",
        "x.rs::type WorldPos",
        "x.rs::type GlobalPosition",
    ];
    for w in want {
        assert!(
            ids.iter().any(|i| i == w),
            "not flagged: {w}\nflagged: {ids:#?}"
        );
    }
    assert_eq!(ids.len(), want.len(), "unexpected extra findings: {ids:#?}");
}

#[test]
fn the_lint_leaves_legal_signatures_alone() {
    let ids = lint_src(
        r#"
        use forge_num::DVec3;
        pub fn rebase(pos: FramePos, vel: FrameVel) -> FramePos { pos }
        pub fn rotate(v: DVec3, axis: DVec3) -> DVec3 { v }
        pub(crate) fn helper(pos: DVec3) {}
        fn private(position: DVec3) {}
        pub struct Pod { position: DVec3, pub velocity: DVec3 }
        impl Mover for Pod { fn move_to(&self, pos: DVec3) {} }
        #[cfg(test)]
        mod tests { pub fn fixture(pos: DVec3) {} }
        pub fn count_points(n: usize) -> usize { n }
        "#,
    );
    assert!(ids.is_empty(), "false positives: {ids:#?}");
}

#[test]
fn positive_control_a_real_engine_file_with_a_bad_function_is_flagged() {
    let root = workspace_root();
    let rel = "crates/forge-frames/src/resolve.rs";
    let mut src = std::fs::read_to_string(root.join(rel)).unwrap();
    assert!(
        lint_src(&src).is_empty(),
        "{rel} is not clean to begin with"
    );
    src.push_str("\npub fn teleport(frames: &WorldFrame, world_position: DVec3) {}\n");
    let files = vec![(rel.to_string(), parse(rel, &src).unwrap())];
    let ids: Vec<String> = lint(&files).into_iter().map(|v| v.id).collect();
    assert_eq!(ids, [format!("{rel}::teleport(world_position)")]);
}

#[test]
fn positive_control_the_allow_list_needs_reasons_and_cannot_rot() {
    assert!(
        parse_allow("crates/a.rs::f(pos)\n").is_err(),
        "an entry without a reason"
    );
    assert!(
        parse_allow("crates/a.rs::f(pos)  #   \n").is_err(),
        "an empty reason"
    );
    assert!(
        parse_allow("a::f(pos) # r\na::f(pos) # r\n").is_err(),
        "a duplicate"
    );
    let allow = parse_allow("x.rs::gone(pos)  # was removed\n").unwrap();
    let e = judge(&[], &allow).unwrap_err();
    assert!(e.contains("stale entry"), "{e}");
    let found = vec![Violation {
        id: "x.rs::f(pos)".into(),
        what: "bare".into(),
    }];
    assert!(
        judge(&found, &BTreeMap::new()).is_err(),
        "an unlisted violation passed"
    );
    let allow = parse_allow("x.rs::f(pos)  # reason\n").unwrap();
    assert_eq!(judge(&found, &allow), Ok(()));
}

/// W2 for the 2D extension (WP-U15, Ch.35 §35.3): a bare `DVec2` in a position-shaped
/// signature is flagged like a bare `DVec3`; a `FramePos2` and a non-position `DVec2` are not.
#[test]
fn positive_control_a_bare_2d_position_is_flagged() {
    let ids = lint_src(
        r#"
        use forge_num::DVec2;
        use forge_num::DVec2 as P2;
        pub fn warp(pos: DVec2) {}
        pub fn spawn(spawn_point: P2) {}
        pub struct Coin { pub position: DVec2, pub velocity: DVec2 }
        pub fn ok(at: forge_frames::FramePos2, dir: DVec2) {}
        "#,
    );
    for w in [
        "x.rs::warp(pos)",
        "x.rs::spawn(spawn_point)",
        "x.rs::Coin.position",
    ] {
        assert!(ids.iter().any(|i| i == w), "{w} not flagged: {ids:?}");
    }
    assert_eq!(
        ids.len(),
        3,
        "velocity, a FramePos2 and a direction are fine: {ids:?}"
    );
}

/// W2 for the per-crate allow-lists: a crate's `i1_allow.txt` is merged, but it may name
/// only that crate's items, and not repeat the repository's entries.
#[test]
fn positive_control_a_crate_allow_list_names_only_its_own_items() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("i1-allow-lists");
    let _ = std::fs::remove_dir_all(&root);
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write(ALLOW_FILE, "crates/a/src/x.rs::f(pos)  # the repo's\n");
    write(
        "crates/b/i1_allow.txt",
        "crates/b/src/y.rs::g(pos)  # b's own\n",
    );
    let all = allow_lists(&root).expect("merged");
    assert!(all.contains_key("crates/a/src/x.rs::f(pos)"));
    assert!(all.contains_key("crates/b/src/y.rs::g(pos)"));
    write(
        "crates/b/i1_allow.txt",
        "crates/a/src/z.rs::h(pos)  # a's, in b's file\n",
    );
    let e = allow_lists(&root).expect_err("another crate's item");
    assert!(e.contains("is not an item of crates/b"), "{e}");
    write(
        "crates/a/i1_allow.txt",
        "crates/a/src/x.rs::f(pos)  # listed twice\n",
    );
    write("crates/b/i1_allow.txt", "");
    let e = allow_lists(&root).expect_err("a duplicate across files");
    assert!(e.contains("also listed"), "{e}");
}

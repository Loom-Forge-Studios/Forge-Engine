//! I21 — `test_no_runtime_phone_home` (Ch.38.1, Ch.38.6): **the software never phones home
//! to function, and nothing whatsoever of that kind is in a customer's shipped game.**
//!
//! The guard walks `forge-runtime`'s full transitive dependency graph **as cargo resolves
//! it** (`cargo metadata`, host platform, features unified — a superset of any real build,
//! so it can over-report but never under-report) and fails on:
//!
//! 1. `forge-licence` (or any licensing crate) anywhere in the closure;
//! 2. a network-capable crate that is not an allow-listed transport
//!    (`tests/licence/net_allow.txt`, each entry with its reason): a known network/telemetry
//!    crate by name, a crate whose enabled features turn on sockets (`tokio/net`, the Win32
//!    networking APIs), a registry crate whose source names a socket type, or a local crate
//!    whose code reaches `std::net`'s sockets — resolved through `use` trees, renames and
//!    aliases, not by a name match (W3);
//! 3. `forge-num` built with `mutate-det` (WP-01 follow-up): the determinism positive control
//!    can never ship. `forge-runtime` also refuses to *compile* with it (a constant
//!    assertion), which `positive_control_mutate_det_runtime_does_not_compile` proves.
//!
//! Positive controls (W2): every rule is shown failing on a synthetic graph, and — the
//! plan's "mutation build adds the dependency" — a scratch workspace that is the real
//! `forge-runtime` plus a `forge-licence` dependency (and, separately, a local telemetry
//! crate that opens a `TcpStream`) is resolved by real cargo and must fail, while the same
//! scratch workspace without the addition must pass.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use forge_tests::{DepGraph, workspace_root};
use syn::visit::Visit;

const RUNTIME: &str = "forge-runtime";

/// Licensing crates: never in the runtime closure, whatever an allow-list says.
const LICENSING: &[&str] = &["forge-licence", "forge-license", "forge-entitlement"];

/// Crates whose purpose is network I/O or telemetry. Conservative on purpose: a false alarm
/// costs one allow-list line with a reason; a miss ships a phone-home in someone's game.
const NET_CRATES: &[&str] = &[
    "reqwest",
    "hyper",
    "hyper-util",
    "h2",
    "h3",
    "ureq",
    "curl",
    "curl-sys",
    "isahc",
    "surf",
    "attohttpc",
    "minreq",
    "mio",
    "socket2",
    "async-net",
    "async-std",
    "smol",
    "tungstenite",
    "tokio-tungstenite",
    "websocket",
    "quinn",
    "quinn-proto",
    "rustls",
    "native-tls",
    "openssl",
    "openssl-sys",
    "hickory-resolver",
    "trust-dns-resolver",
    "tonic",
    "axum",
    "warp",
    "actix-web",
    "zmq",
    "nng",
    "libp2p",
    "ssh2",
    "lettre",
    "sentry",
    "opentelemetry",
    "opentelemetry-otlp",
    "tracing-opentelemetry",
    "posthog-rs",
    "segment",
    "steamworks",
    "matchbox_socket",
    "laminar",
    "renet",
];

/// `(crate, features)`: the crate is harmless unless one of these features is enabled.
const NET_FEATURES: &[(&str, &[&str])] = &[
    ("tokio", &["net"]),
    (
        "windows-sys",
        &[
            "Win32_Networking",
            "Win32_Networking_WinSock",
            "Win32_Networking_WinHttp",
            "Win32_Networking_WinInet",
        ],
    ),
    (
        "windows",
        &[
            "Win32_Networking",
            "Win32_Networking_WinSock",
            "Win32_Networking_WinHttp",
            "Win32_Networking_WinInet",
        ],
    ),
];

/// `std::net` items that are plain data (addresses, parse errors), not I/O.
const STD_NET_DATA: &[&str] = &[
    "IpAddr",
    "Ipv4Addr",
    "Ipv6Addr",
    "SocketAddr",
    "SocketAddrV4",
    "SocketAddrV6",
    "AddrParseError",
    "Ipv6MulticastScope",
];

/// Socket types, searched for in registry crates' sources.
const SOCKET_WORDS: &[&str] = &["TcpStream", "TcpListener", "UdpSocket"];

// ---- the check ----------------------------------------------------------------------------

/// Every I21 violation in `root`'s closure (empty = clean). `Err` if `root` is not in the
/// graph: a guard about a crate that does not exist must fail, not pass (W2).
fn violations(g: &DepGraph, root: &str, allow: &BTreeSet<String>) -> Result<Vec<String>, String> {
    let mut closure = g.closure(root)?;
    closure.insert(root.to_string());
    let mut out = Vec::new();
    for pkg in &closure {
        if LICENSING.contains(&pkg.as_str()) {
            out.push(format!(
                "{pkg}: licensing code in the shipped runtime (I21 forbids it absolutely)"
            ));
            continue;
        }
        if pkg == "forge-num" && g.features(pkg).contains("mutate-det") {
            out.push("forge-num: built with `mutate-det`, the determinism positive control".into());
        }
        if allow.contains(pkg) {
            continue;
        }
        if NET_CRATES.contains(&pkg.as_str()) {
            out.push(format!(
                "{pkg}: a network/telemetry crate, not allow-listed"
            ));
            continue;
        }
        let feats = g.features(pkg);
        for (name, bad) in NET_FEATURES {
            if pkg == name {
                for f in bad.iter().filter(|f| feats.contains(**f)) {
                    out.push(format!("{pkg}: feature `{f}` enables networking"));
                }
            }
        }
        for d in g.dirs(pkg) {
            let hits = if d.local {
                local_std_net_uses(&d.dir)
            } else {
                registry_socket_words(&d.dir)
            };
            for h in hits {
                out.push(format!("{pkg}: {h}"));
            }
        }
    }
    Ok(out)
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // Only what is compiled into the library/binary: skip tests, benches, examples.
            if !matches!(name, "tests" | "benches" | "examples" | "target" | ".git") {
                rust_files(&p, out);
            }
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// A registry crate's compiled sources naming a socket type **in code**: an identifier
/// token, so a doc comment's example (`serde_json::from_reader`'s docs show a `TcpStream`) is
/// not a finding, while any code that names the type is. A file that does not tokenize falls
/// back to the plain word match (over-reporting, never under-reporting).
fn registry_socket_words(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(&dir.join("src"), &mut files);
    let mut hits = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for w in SOCKET_WORDS {
            if code_names(&text, w) {
                hits.push(format!("{} names `{w}`", f.display()));
            }
        }
    }
    hits
}

/// Does `src` name `w` as an identifier in code (comments and doc comments excluded)?
fn code_names(src: &str, w: &str) -> bool {
    fn walk(ts: proc_macro2::TokenStream, w: &str) -> bool {
        ts.into_iter().any(|t| match t {
            proc_macro2::TokenTree::Ident(i) => i == w,
            proc_macro2::TokenTree::Group(g) => walk(g.stream(), w),
            _ => false,
        })
    }
    match src.parse::<proc_macro2::TokenStream>() {
        Ok(ts) => walk(ts, w),
        Err(_) => contains_word(src, w),
    }
}

fn contains_word(text: &str, w: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(w).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + w.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// A local crate's code reaching `std::net` I/O, resolved through `use` trees.
fn local_std_net_uses(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(&dir.join("src"), &mut files);
    let mut hits = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for h in std_net_uses(&text) {
            hits.push(format!("{}: {h}", f.display()));
        }
    }
    hits
}

/// `std::net` socket paths in one source file. Unparseable source is itself a finding: a
/// guard that cannot read the code cannot vouch for it.
fn std_net_uses(src: &str) -> Vec<String> {
    let file = match syn::parse_file(src) {
        Ok(f) => f,
        Err(e) => return vec![format!("cannot parse ({e}); I21 cannot vouch for it")],
    };
    let mut v = NetPaths::default();
    v.visit_file(&file);
    let std_aliases: BTreeSet<String> = std::iter::once("std".to_string())
        .chain(v.std_aliases.iter().cloned())
        .collect();
    let mut net_aliases: BTreeSet<String> = BTreeSet::new();
    let mut hits = BTreeSet::new();
    // `use` imports first: they can introduce aliases of `std` or of `std::net`.
    for (path, alias) in &v.uses {
        if let Some(rest) = strip_std_net(path, &std_aliases) {
            if rest.is_empty() {
                net_aliases.insert(alias.clone().unwrap_or_else(|| "net".into()));
            }
            if is_io(&rest) {
                hits.insert(format!("uses `std::net::{}`", show(&rest)));
            }
        }
    }
    for path in &v.paths {
        let resolved = match path.first() {
            Some(first) if net_aliases.contains(first) => {
                let mut p = vec!["std".to_string(), "net".to_string()];
                p.extend(path[1..].iter().cloned());
                p
            }
            _ => path.clone(),
        };
        if let Some(rest) = strip_std_net(&resolved, &std_aliases)
            && is_io(&rest)
        {
            hits.insert(format!("reaches `std::net::{}`", show(&rest)));
        }
    }
    hits.into_iter().collect()
}

fn show(rest: &[String]) -> String {
    if rest.is_empty() {
        "*".into()
    } else {
        rest.join("::")
    }
}

/// `Some(rest)` if `path` is `std::net::rest` (with `std` possibly an alias).
fn strip_std_net(path: &[String], std_aliases: &BTreeSet<String>) -> Option<Vec<String>> {
    let (a, b) = (path.first()?, path.get(1)?);
    (std_aliases.contains(a) && b == "net").then(|| path[2..].to_vec())
}

/// Whether the `std::net` item named by `rest` is I/O (anything but the plain data types).
/// A glob (`*`) is I/O. The bare module (`rest` empty) is only an alias: its uses decide.
fn is_io(rest: &[String]) -> bool {
    match rest.first() {
        None => false,
        Some(first) => first == "*" || !STD_NET_DATA.contains(&first.as_str()),
    }
}

#[derive(Default)]
struct NetPaths {
    /// Flattened `use` paths with their rename, if any.
    uses: Vec<(Vec<String>, Option<String>)>,
    /// Every other path in the file.
    paths: Vec<Vec<String>>,
    /// Names `std` is imported under (`use std as s;`, `extern crate std as s;`).
    std_aliases: Vec<String>,
}

fn flatten(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut NetPaths) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let mut p = prefix.clone();
            let name = n.ident.to_string();
            if name != "self" {
                p.push(name);
            }
            out.uses.push((p, None));
        }
        syn::UseTree::Rename(r) => {
            let mut p = prefix.clone();
            let name = r.ident.to_string();
            if name != "self" {
                p.push(name);
            }
            if p.len() == 1 && p[0] == "std" {
                out.std_aliases.push(r.rename.to_string());
            }
            out.uses.push((p, Some(r.rename.to_string())));
        }
        syn::UseTree::Glob(_) => {
            let mut p = prefix.clone();
            p.push("*".into());
            out.uses.push((p, None));
        }
        syn::UseTree::Group(g) => {
            for t in &g.items {
                flatten(t, prefix, out);
            }
        }
    }
}

impl<'ast> Visit<'ast> for NetPaths {
    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        flatten(&u.tree, &mut Vec::new(), self);
    }
    fn visit_item_extern_crate(&mut self, e: &'ast syn::ItemExternCrate) {
        if e.ident == "std"
            && let Some((_, rename)) = &e.rename
        {
            self.std_aliases.push(rename.to_string());
        }
    }
    fn visit_path(&mut self, p: &'ast syn::Path) {
        self.paths
            .push(p.segments.iter().map(|s| s.ident.to_string()).collect());
        syn::visit::visit_path(self, p);
    }
}

/// `tests/licence/net_allow.txt`: `<crate> — <reason>` per line; `#` comments. An entry may be
/// scoped to one host OS, `<crate> [linux] — <reason>`: the resolved graph is the host
/// platform's (cargo metadata), so a transport only one platform links (the X11 and D-Bus
/// clients on Linux, WP-30) is excused — and checked for staleness — only there.
fn allow_list() -> Result<BTreeSet<String>, String> {
    let path = workspace_root().join("tests/licence/net_allow.txt");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_allow_list(&text, std::env::consts::OS)
}

/// The host OS names an entry may be scoped to (`std::env::consts::OS` spellings).
const ALLOW_OSES: &[&str] = &["windows", "linux", "macos"];

/// The entries of `text` that apply on host `os`.
fn parse_allow_list(text: &str, os: &str) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, reason) = line
            .split_once(" — ")
            .ok_or_else(|| format!("net_allow.txt:{}: `<crate> — <reason>` expected", i + 1))?;
        let (name, scope) = match name.trim().split_once(" [") {
            Some((n, rest)) => {
                let s = rest.strip_suffix(']').ok_or_else(|| {
                    format!(
                        "net_allow.txt:{}: `<crate> [os] — <reason>` expected",
                        i + 1
                    )
                })?;
                if !ALLOW_OSES.contains(&s) {
                    return Err(format!(
                        "net_allow.txt:{}: unknown OS `{s}` (one of {ALLOW_OSES:?})",
                        i + 1
                    ));
                }
                (n.trim(), Some(s))
            }
            None => (name.trim(), None),
        };
        if reason.trim().len() < 10 {
            return Err(format!(
                "net_allow.txt:{}: `{name}` needs a real reason",
                i + 1
            ));
        }
        if LICENSING.contains(&name) {
            return Err(format!(
                "net_allow.txt:{}: a licensing crate can never be allow-listed",
                i + 1
            ));
        }
        if scope.is_none_or(|s| s == os) {
            out.insert(name.to_string());
        }
    }
    Ok(out)
}

// ---- the guard ------------------------------------------------------------------------------

/// The real runtime graph: cargo metadata's resolve for edges and sources, and the features
/// cargo enables when building forge-runtime alone (the workspace-unified features would
/// include whatever the editor switches on, which the shipped runtime never links).
fn runtime_graph(root: &Path, extra: &[&str]) -> DepGraph {
    DepGraph::resolve_with(root, extra)
        .and_then(|g| g.with_features_of(root, RUNTIME, extra))
        .expect("resolve forge-runtime")
}

#[test]
fn forge_runtime_links_no_licensing_or_network_crate() {
    let g = runtime_graph(&workspace_root(), &["--locked"]);
    let allow = allow_list().expect("allow-list");
    let v = violations(&g, RUNTIME, &allow).expect("forge-runtime is in the graph");
    assert!(v.is_empty(), "I21 violated:\n{}", v.join("\n"));

    // Non-vacuity: the walk really goes deep (forge-core pulls bevy_ecs and its closure),
    // and the local crates it scanned are the runtime's real ones.
    let c = g.closure(RUNTIME).expect("closure");
    for must in ["forge-core", "forge-num", "forge-frames", "bevy_ecs"] {
        assert!(c.contains(must), "closure lacks {must}: {c:?}");
    }
    assert!(c.len() > 20, "suspiciously small closure: {c:?}");
    let scanned_local = c
        .iter()
        .filter(|p| g.dirs(p).iter().any(|d| d.local))
        .count();
    assert!(scanned_local >= 4, "local crates scanned: {scanned_local}");
    // Every allow-list entry must name a crate actually in the closure (no stale excuses).
    for a in &allow {
        assert!(
            c.contains(a),
            "net_allow.txt lists `{a}`, which forge-runtime does not link"
        );
    }
}

#[test]
fn mutate_det_never_ships_in_forge_runtime() {
    let root = workspace_root();
    let g = runtime_graph(&root, &["--locked"]);
    assert!(g.closure(RUNTIME).expect("closure").contains("forge-num"));
    assert!(!g.features("forge-num").contains("mutate-det"));
    // Positive control: the same resolution with the feature switched on is caught.
    let mutated = DepGraph::resolve(&root)
        .and_then(|g| {
            g.with_features_of(
                &root,
                RUNTIME,
                &["--locked", "--features", "forge-num/mutate-det"],
            )
        })
        .expect("resolve mutated");
    let v = violations(&mutated, RUNTIME, &BTreeSet::new()).expect("runtime");
    assert!(v.iter().any(|l| l.contains("mutate-det")), "{v:?}");
}

// ---- positive controls ----------------------------------------------------------------------

/// Synthetic `cargo metadata`: `(name, source dir or none, local)`, edges, features.
fn meta(
    pkgs: &[(&str, Option<&Path>, bool)],
    edges: &[(&str, &str)],
    feats: &[(&str, &[&str])],
) -> serde_json::Value {
    let packages: Vec<_> = pkgs
        .iter()
        .map(|(n, dir, local)| {
            let manifest = dir
                .map(|d| d.join("Cargo.toml").display().to_string())
                .unwrap_or_default();
            serde_json::json!({
                "id": n, "name": n,
                "manifest_path": manifest,
                "source": if *local { serde_json::Value::Null } else { "registry+x".into() },
            })
        })
        .collect();
    let nodes: Vec<_> = pkgs
        .iter()
        .map(|(n, _, _)| {
            let deps: Vec<_> = edges
                .iter()
                .filter(|(a, _)| a == n)
                .map(|(_, b)| serde_json::json!({"pkg": b, "dep_kinds": [{"kind": null}]}))
                .collect();
            let f: Vec<&str> = feats
                .iter()
                .filter(|(p, _)| p == n)
                .flat_map(|(_, f)| f.iter().copied())
                .collect();
            serde_json::json!({"id": n, "deps": deps, "features": f})
        })
        .collect();
    serde_json::json!({"packages": packages, "resolve": {"nodes": nodes}})
}

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("phone-home")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).expect("scratch dir");
    d
}

fn crate_with_source(name: &str, src: &str) -> PathBuf {
    let d = scratch(name);
    std::fs::write(d.join("src/lib.rs"), src).expect("write");
    d
}

fn run(m: &serde_json::Value, allow: &[&str]) -> Vec<String> {
    let g = DepGraph::from_metadata(m).expect("synthetic metadata");
    let allow = allow.iter().map(|s| s.to_string()).collect();
    violations(&g, RUNTIME, &allow).expect("runtime present")
}

type Pkgs<'a> = Vec<(&'a str, Option<&'a Path>, bool)>;
type Edges<'a> = Vec<(&'a str, &'a str)>;

/// A runtime (with clean source) on forge-core, plus `extra` packages and edges.
fn with_runtime<'a>(
    runtime_src: &'a Path,
    extra: &[(&'a str, Option<&'a Path>, bool)],
    e: &[(&'a str, &'a str)],
) -> (Pkgs<'a>, Edges<'a>) {
    let mut p = vec![
        (RUNTIME, Some(runtime_src), true),
        ("forge-core", None, true),
    ];
    p.extend_from_slice(extra);
    let mut edges = vec![(RUNTIME, "forge-core")];
    edges.extend_from_slice(e);
    (p, edges)
}

#[test]
fn positive_control_every_phone_home_path_is_caught() {
    let clean_src = crate_with_source(
        "clean",
        "use std::net::{IpAddr, Ipv4Addr};\npub fn lo() -> IpAddr { IpAddr::V4(Ipv4Addr::LOCALHOST) }\n",
    );

    // The faithful model passes: address types from std::net are data, not I/O.
    let (p, e) = with_runtime(&clean_src, &[], &[]);
    assert_eq!(run(&meta(&p, &e, &[]), &[]), Vec::<String>::new());

    let expect_caught = |what: &str, m: serde_json::Value, needle: &str| {
        let v = run(&m, &[]);
        assert!(
            v.iter().any(|l| l.contains(needle)),
            "{what}: not caught (want `{needle}`): {v:?}"
        );
    };

    // 1. forge-licence, directly and transitively.
    let (p, e) = with_runtime(
        &clean_src,
        &[("forge-licence", None, true)],
        &[(RUNTIME, "forge-licence")],
    );
    expect_caught("licence direct", meta(&p, &e, &[]), "forge-licence");
    let (p, e) = with_runtime(
        &clean_src,
        &[("forge-licence", None, true)],
        &[("forge-core", "forge-licence")],
    );
    expect_caught("licence transitive", meta(&p, &e, &[]), "forge-licence");
    // An allow-list cannot excuse licensing code.
    let v = run(&meta(&p, &e, &[]), &["forge-licence"]);
    assert!(v.iter().any(|l| l.contains("forge-licence")), "{v:?}");

    // 2a. A network crate three levels down.
    let (p, e) = with_runtime(
        &clean_src,
        &[("bevy_thing", None, false), ("reqwest", None, false)],
        &[("forge-core", "bevy_thing"), ("bevy_thing", "reqwest")],
    );
    expect_caught("net crate transitive", meta(&p, &e, &[]), "reqwest");
    // ...which an explicit transport allow-list entry does excuse.
    assert_eq!(run(&meta(&p, &e, &[]), &["reqwest"]), Vec::<String>::new());

    // 2b. tokio is fine without `net`, caught with it; Win32 networking features too.
    let (p, e) = with_runtime(
        &clean_src,
        &[("tokio", None, false)],
        &[("forge-core", "tokio")],
    );
    assert_eq!(
        run(&meta(&p, &e, &[("tokio", &["rt", "sync"])]), &[]),
        Vec::<String>::new()
    );
    expect_caught(
        "tokio/net",
        meta(&p, &e, &[("tokio", &["rt", "net"])]),
        "feature `net`",
    );
    let (p, e) = with_runtime(
        &clean_src,
        &[("windows-sys", None, false)],
        &[("forge-core", "windows-sys")],
    );
    expect_caught(
        "winsock",
        meta(&p, &e, &[("windows-sys", &["Win32_Networking_WinSock"])]),
        "Win32_Networking_WinSock",
    );

    // 2c. A registry crate with an innocent name whose source opens sockets.
    let sneaky = crate_with_source(
        "sneaky-registry",
        "pub fn ping() { let _ = std::net::TcpStream::connect(\"example.com:80\"); }\n",
    );
    let (p, e) = with_runtime(
        &clean_src,
        &[("innocent-metrics", Some(sneaky.as_path()), false)],
        &[("forge-core", "innocent-metrics")],
    );
    expect_caught("registry socket", meta(&p, &e, &[]), "TcpStream");

    // 2d. Local crates reaching std::net sockets, in every spelling the resolver normalises.
    let spellings: [(&str, &str, &str); 6] = [
        (
            "grouped",
            "use std::{io, net::{TcpStream as T}};\npub fn f(_: Option<T>) { let _ = io::empty(); }\n",
            "TcpStream",
        ),
        (
            "absolute-expr",
            "pub fn f() { let _ = ::std::net::UdpSocket::bind(\"0.0.0.0:0\"); }\n",
            "UdpSocket",
        ),
        (
            "std-alias",
            "use std as s;\npub fn f() { let _ = s::net::TcpListener::bind(\"0.0.0.0:0\"); }\n",
            "TcpListener",
        ),
        (
            "module-alias",
            "use std::net as n;\npub fn f() { let _ = n::TcpStream::connect(\"a:1\"); }\n",
            "TcpStream",
        ),
        (
            "glob",
            "use std::net::*;\npub fn f() -> Option<TcpStream> { None }\n",
            "std::net::*",
        ),
        (
            "extern-alias",
            "extern crate std as q;\npub fn f() { let _ = q::net::UdpSocket::bind(\"0.0.0.0:0\"); }\n",
            "UdpSocket",
        ),
    ];
    for (name, src, needle) in spellings {
        let dir = crate_with_source(name, src);
        let (p, e) = with_runtime(
            &clean_src,
            &[("forge-telemetry", Some(dir.as_path()), true)],
            &[("forge-core", "forge-telemetry")],
        );
        expect_caught(name, meta(&p, &e, &[]), needle);
    }
    // Unparseable local source cannot be vouched for.
    let broken = crate_with_source("broken", "pub fn f( {\n");
    let (p, e) = with_runtime(
        &clean_src,
        &[("forge-broken", Some(broken.as_path()), true)],
        &[("forge-core", "forge-broken")],
    );
    expect_caught("unparseable", meta(&p, &e, &[]), "cannot parse");

    // 3. mutate-det in the runtime graph.
    let (p, e) = with_runtime(
        &clean_src,
        &[("forge-num", None, true)],
        &[("forge-core", "forge-num")],
    );
    expect_caught(
        "mutate-det",
        meta(&p, &e, &[("forge-num", &["mutate-det"])]),
        "mutate-det",
    );

    // And asking about a runtime that is not in the graph is an error, never a pass.
    let g = DepGraph::from_metadata(&meta(&[("other", None, true)], &[], &[])).expect("meta");
    assert!(violations(&g, RUNTIME, &BTreeSet::new()).is_err());
}

/// A scratch workspace that is the real `forge-runtime` manifest (its path dependencies
/// pointing at the real crates) plus an optional extra dependency, resolved by real cargo.
fn mutant_workspace(name: &str, extra: Option<(&str, &str)>) -> DepGraph {
    let root = workspace_root();
    let ws = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("phone-home-mutant")
        .join(name);
    let _ = std::fs::remove_dir_all(&ws);
    let rt = ws.join("forge-runtime");
    std::fs::create_dir_all(rt.join("src")).expect("mkdir");
    let crates = root.join("crates").display().to_string().replace('\\', "/");
    let real = std::fs::read_to_string(root.join("crates/forge-runtime/Cargo.toml"))
        .expect("forge-runtime manifest");
    // Normalise first: a CRLF checkout (core.autocrlf) would hide `[dependencies]\n`.
    let mut manifest = real
        .replace("\r\n", "\n")
        .replace("path = \"../", &format!("path = \"{crates}/"));
    let mut members = vec!["\"forge-runtime\"".to_string()];
    if let Some((dep, src)) = extra {
        let d = ws.join(dep);
        std::fs::create_dir_all(d.join("src")).expect("mkdir");
        std::fs::write(
            d.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{dep}\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n"
            ),
        )
        .expect("write");
        std::fs::write(d.join("src/lib.rs"), src).expect("write");
        manifest = manifest.replace(
            "[dependencies]\n",
            &format!("[dependencies]\n{dep} = {{ path = \"../{dep}\" }}\n"),
        );
        members.push(format!("\"{dep}\""));
    }
    // Keep the real workspace tables (package, lints, dependencies); swap the members.
    let root_toml = std::fs::read_to_string(root.join("Cargo.toml")).expect("root manifest");
    let root_toml = root_toml
        .lines()
        .map(|l| {
            if l.starts_with("members") {
                format!("members = [{}]", members.join(", "))
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(ws.join("Cargo.toml"), root_toml).expect("write");
    std::fs::write(rt.join("Cargo.toml"), manifest.replace("\r\n", "\n")).expect("write");
    std::fs::write(rt.join("src/lib.rs"), "").expect("write");
    std::fs::copy(root.join("Cargo.lock"), ws.join("Cargo.lock")).expect("lock");
    runtime_graph(&ws, &["--offline"])
}

#[test]
fn positive_control_mutation_build_adding_forge_licence_fails() {
    let allow = allow_list().expect("allow-list");
    // Control of the control: the scratch copy of the real runtime passes, so what fails
    // below is the added dependency and nothing about the scratch setup.
    let base = mutant_workspace("base", None);
    let v = violations(&base, RUNTIME, &allow).expect("runtime");
    assert!(v.is_empty(), "scratch runtime should be clean: {v:?}");
    assert!(base.closure(RUNTIME).expect("closure").contains("bevy_ecs"));

    let licence = mutant_workspace(
        "licence",
        Some(("forge-licence", "pub fn activated() -> bool { true }\n")),
    );
    let v = violations(&licence, RUNTIME, &allow).expect("runtime");
    assert!(
        v.iter().any(|l| l.starts_with("forge-licence:")),
        "a runtime linking forge-licence must fail I21: {v:?}"
    );

    let telemetry = mutant_workspace(
        "telemetry",
        Some((
            "forge-usage-stats",
            "use std::io::Write;\nuse std::net::TcpStream;\npub fn report() { if let Ok(mut s) = TcpStream::connect(\"stats.invalid:443\") { let _ = s.write_all(b\"hi\"); } }\n",
        )),
    );
    let v = violations(&telemetry, RUNTIME, &allow).expect("runtime");
    assert!(
        v.iter()
            .any(|l| l.starts_with("forge-usage-stats:") && l.contains("TcpStream")),
        "a runtime linking a crate that opens sockets must fail I21: {v:?}"
    );
}

#[test]
fn positive_control_mutate_det_runtime_does_not_compile() {
    let root = workspace_root();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
        .join("mutate-det-runtime");
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "check",
            "--locked",
            "-p",
            RUNTIME,
            "--lib",
            "--features",
            "forge-num/mutate-det",
            "--message-format",
            "short",
        ])
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(&root)
        .output()
        .expect("run cargo check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "forge-runtime compiled with forge-num/mutate-det — the positive control can ship:\n{stderr}"
    );
    assert!(
        stderr.contains("must never ship in forge-runtime"),
        "the build failed, but not on the MUTATE_DET assertion:\n{stderr}"
    );
}

#[test]
fn std_net_resolution_unit_cases() {
    // Data types and unrelated `net` modules are not findings.
    assert!(std_net_uses("use std::net::SocketAddr; pub fn f(_: SocketAddr) {}").is_empty());
    assert!(std_net_uses("mod net { pub struct TcpStream; } use net::TcpStream;").is_empty());
    assert!(std_net_uses("pub fn f() { let _ = core::net::Ipv4Addr::LOCALHOST; }").is_empty());
    assert!(
        !std_net_uses("pub fn f() { let _ = std::net::TcpStream::connect(\"a:1\"); }").is_empty()
    );
    assert!(contains_word("x TcpStream y", "TcpStream"));
    assert!(!contains_word("MyTcpStreamish", "TcpStream"));
    // Registry sources: code names the socket type; a comment or a string does not.
    assert!(code_names(
        "use std::net::TcpStream; fn f(_: TcpStream) {}",
        "TcpStream"
    ));
    assert!(code_names(
        "fn f() { let _ = std::net::TcpListener::bind(\"a:1\"); }",
        "TcpListener"
    ));
    assert!(!code_names(
        "/// let s = TcpStream::connect(x);\n//! TcpStream\nfn f() {}",
        "TcpStream"
    ));
    assert!(!code_names(
        "/* TcpStream */ fn f() { let _ = \"TcpStream\"; }",
        "TcpStream"
    ));
}

/// OS-scoped allow-list entries (WP-30): an entry for another OS neither excuses a crate nor
/// is checked for staleness here; an entry for this OS, or unscoped, does both. Controls: an
/// unknown OS name, a malformed scope and a scoped licensing crate are refused, so a typo can
/// never turn an entry into a silent no-op or an excuse.
#[test]
fn allow_list_entries_scoped_to_another_os_do_not_apply() {
    let text = "# comment\n\
                x11rb [linux] — the X11 client winit and arboard use on Linux\n\
                winhttp-ish [windows] — a made-up Windows-only transport for this test\n\
                forge-net — the game transport, on every platform\n";
    let linux = parse_allow_list(text, "linux").expect("parses");
    let windows = parse_allow_list(text, "windows").expect("parses");
    assert_eq!(
        linux,
        ["forge-net", "x11rb"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
    assert_eq!(
        windows,
        ["forge-net", "winhttp-ish"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
    let e = parse_allow_list("x11rb [linx] — a typo in the scope, refused\n", "linux")
        .expect_err("an unknown OS must be refused");
    assert!(e.contains("unknown OS `linx`"), "{e}");
    let e = parse_allow_list("x11rb [linux — an unclosed scope, refused\n", "linux")
        .expect_err("a malformed scope must be refused");
    assert!(e.contains("[os]"), "{e}");
    let e = parse_allow_list("forge-licence [linux] — scoped or not, never\n", "linux")
        .expect_err("a licensing crate can never be allow-listed");
    assert!(e.contains("licensing"), "{e}");
}

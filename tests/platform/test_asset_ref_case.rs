//! M0-17 — the asset-reference case lint: **the Windows-works/Linux-breaks bug class, killed
//! at M0** (Risk S14, I18).
//!
//! NTFS resolves `Assets/Rock.png` to `assets/rock.png`; ext4 does not. Every reference that
//! only works case-insensitively ships a build that loads on the author's Windows machine and
//! fails on Linux (or on a case-sensitive CI runner, or a console filesystem). The same holds
//! for backslash separators. This lint makes both a test failure on *every* platform: it
//! resolves references by listing directories and comparing names exactly, never by asking
//! the filesystem (which on Windows would say "exists").
//!
//! What counts as a reference: every string literal in Rust source — including those inside
//! macros (`include_str!`, `include_bytes!`, `concat!(env!("CARGO_MANIFEST_DIR"), "/…")`,
//! `#[path]`, `Path::new`, `join`) — and every quoted string in TOML / RON / JSON / YAML data
//! files (manifest `path =` keys, theme and asset files, gate rows, DoD evidence). A string
//! is checked against the referencing file's directory, its crate root and the workspace
//! root; it is a violation when it names an existing file or directory **only** with
//! different letter case, or only with `\` separators. Strings that name nothing are not
//! references and are ignored. Additionally, no two entries of one directory may differ only
//! by case (they cannot coexist on Windows or macOS checkouts).
//!
//! `forge-store`'s `StorePath` refuses case collisions inside a project (STORE-0007); this
//! lint covers the engine's own tree. When `forge-asset` lands (WP-12) its importers route
//! asset references through the same resolution rule.
//!
//! Positive control (W2): `positive_control_every_case_bug_is_caught` builds a fixture tree
//! with each bug (a manifest `path`, `include_bytes!`, `concat!(env!(…))`, a RON asset
//! reference, a backslash path, a `..` path) beside its correct twin, and requires exactly the
//! broken ones flagged; `positive_control_a_real_reference_with_its_case_flipped_fails` takes
//! a real reference from this repository, flips the case of one letter, and requires a flag.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use forge_tests::workspace_root;
use proc_macro2::{TokenStream, TokenTree};

/// Directories never scanned: build output and VCS internals.
const SKIP_DIRS: &[&str] = &["target", ".git", "node_modules"];

/// Data-file extensions whose quoted strings are references. `gltf` is JSON whose `uri`s
/// name buffers and images (WP-12); import sidecars (`*.meta.ron`) are covered as RON.
/// forge-asset applies the same exact-name rule to project assets at import
/// (`crates/forge-asset/tests/test_asset_ref_exact.rs`).
const DATA_EXT: &[&str] = &["toml", "ron", "json", "yml", "yaml", "gltf"];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Resolution {
    Exact,
    /// Resolves only case-insensitively; the name as it is on disk.
    CaseOnly(String),
    Missing,
}

/// Directory listings, cached. Names come from `read_dir`, which reports the on-disk case on
/// every platform.
#[derive(Default)]
struct Fs {
    listings: RefCell<HashMap<PathBuf, Vec<String>>>,
}

impl Fs {
    fn names(&self, dir: &Path) -> Vec<String> {
        if let Some(n) = self.listings.borrow().get(dir) {
            return n.clone();
        }
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        self.listings
            .borrow_mut()
            .insert(dir.to_path_buf(), names.clone());
        names
    }

    /// Resolve `rel` (`/`-separated) under `base`, comparing each component exactly.
    fn resolve(&self, base: &Path, rel: &str) -> Resolution {
        let mut cur = base.to_path_buf();
        let mut actual: Vec<String> = Vec::new();
        let mut case_only = false;
        let mut depth = 0usize;
        for comp in rel.split('/') {
            match comp {
                "" | "." => continue,
                ".." => {
                    if !cur.pop() {
                        return Resolution::Missing;
                    }
                    actual.push("..".into());
                    depth = depth.saturating_sub(1);
                    continue;
                }
                _ => {}
            }
            let names = self.names(&cur);
            let found = if names.iter().any(|n| n == comp) {
                comp.to_string()
            } else if let Some(n) = names
                .iter()
                .find(|n| n.to_lowercase() == comp.to_lowercase())
            {
                case_only = true;
                n.clone()
            } else {
                return Resolution::Missing;
            };
            cur.push(&found);
            actual.push(found);
            depth += 1;
        }
        if depth == 0 {
            return Resolution::Missing; // `.`, `..`: a directory walk, not a reference
        }
        if case_only {
            Resolution::CaseOnly(actual.join("/"))
        } else {
            Resolution::Exact
        }
    }
}

/// Could `s` be a path? Cheap filter; the resolution decides.
fn candidate(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 260
        && (s.contains('/') || s.contains('\\') || s.contains('.'))
        && s.chars().any(char::is_alphabetic)
        && !s
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || ":{}*<>|?\"".contains(c))
}

struct Lint<'a> {
    root: &'a Path,
    fs: Fs,
}

impl<'a> Lint<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            fs: Fs::default(),
        }
    }

    /// The nearest ancestor of `file` (inside the root) with a `Cargo.toml`.
    fn crate_root(&self, file: &Path) -> Option<PathBuf> {
        let mut d = file.parent()?;
        loop {
            if self.fs.names(d).iter().any(|n| n == "Cargo.toml") {
                return Some(d.to_path_buf());
            }
            if d == self.root {
                return None;
            }
            d = d.parent()?;
        }
    }

    /// Check one string found in `file`.
    fn check(&self, file: &Path, s: &str, out: &mut Vec<String>) {
        if !candidate(s) {
            return;
        }
        let backslash = s.contains('\\');
        let norm = s.replace('\\', "/");
        let norm = norm.trim_start_matches('/');
        let mut bases: Vec<PathBuf> = Vec::new();
        if let Some(d) = file.parent() {
            bases.push(d.to_path_buf());
        }
        bases.extend(self.crate_root(file));
        bases.push(self.root.to_path_buf());
        let results: Vec<Resolution> = bases.iter().map(|b| self.fs.resolve(b, norm)).collect();
        let rel = file.strip_prefix(self.root).unwrap_or(file).display();
        if results.contains(&Resolution::Exact) {
            if backslash {
                out.push(format!(
                    "{rel}: \"{s}\" uses `\\` separators — resolves on Windows only; write `/`"
                ));
            }
            return;
        }
        if let Some(Resolution::CaseOnly(actual)) = results
            .iter()
            .find(|r| matches!(r, Resolution::CaseOnly(_)))
        {
            out.push(format!(
                "{rel}: \"{s}\" resolves only case-insensitively (on disk: \"{actual}\") — works on Windows, breaks on Linux"
            ));
        }
    }

    /// Every string in one file, by its kind.
    fn lint_file(&self, file: &Path, text: &str, out: &mut Vec<String>) {
        let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
        let strings = if ext == "rs" {
            match rust_strings(text) {
                Ok(s) => s,
                Err(e) => {
                    out.push(format!("{}: cannot tokenize ({e})", file.display()));
                    return;
                }
            }
        } else if DATA_EXT.contains(&ext) {
            quoted_strings(text)
        } else {
            return;
        };
        for s in strings {
            self.check(file, &s, out);
        }
    }

    /// The whole tree: every reference, and every case collision.
    fn lint_tree(&self) -> (Vec<String>, usize) {
        let mut out = Vec::new();
        let mut files = Vec::new();
        self.walk(self.root, &mut files, &mut out);
        for f in &files {
            if let Ok(text) = std::fs::read_to_string(f) {
                self.lint_file(f, &text, &mut out);
            }
        }
        (out, files.len())
    }

    fn walk(&self, dir: &Path, files: &mut Vec<PathBuf>, out: &mut Vec<String>) {
        let names = self.fs.names(dir);
        for c in collisions(&names) {
            let rel = dir.strip_prefix(self.root).unwrap_or(dir).display();
            out.push(format!(
                "{rel}/: {c} differ only by case — they cannot coexist on Windows or macOS"
            ));
        }
        for n in names {
            if SKIP_DIRS.contains(&n.as_str()) {
                continue;
            }
            let p = dir.join(&n);
            if p.is_dir() {
                self.walk(&p, files, out);
            } else {
                files.push(p);
            }
        }
    }
}

/// Groups of names in one directory that differ only by case.
fn collisions(names: &[String]) -> Vec<String> {
    let mut by_lower: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for n in names {
        by_lower.entry(n.to_lowercase()).or_default().push(n);
    }
    by_lower
        .into_values()
        .filter(|v| v.len() > 1)
        .map(|v| v.join(" / "))
        .collect()
}

/// Every string literal in Rust source, including inside macro invocations and doc
/// attributes.
fn rust_strings(src: &str) -> Result<Vec<String>, String> {
    let ts: TokenStream = src.parse().map_err(|e| format!("{e:?}"))?;
    let mut out = Vec::new();
    collect(ts, &mut out);
    Ok(out)
}

fn collect(ts: TokenStream, out: &mut Vec<String>) {
    for tt in ts {
        match tt {
            TokenTree::Group(g) => collect(g.stream(), out),
            TokenTree::Literal(l) => {
                if let Ok(s) = syn::parse_str::<syn::LitStr>(&l.to_string()) {
                    out.push(s.value());
                }
            }
            _ => {}
        }
    }
}

/// Quoted strings in a data file: `"…"` with backslash escapes, and TOML's `'…'` literals.
fn quoted_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' && c != '\'' {
            continue;
        }
        let quote = c;
        let mut s = String::new();
        let mut closed = false;
        while let Some(d) = chars.next() {
            if d == '\n' {
                break;
            }
            if quote == '"' && d == '\\' {
                match chars.next() {
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some(o) => {
                        s.push('\\');
                        s.push(o);
                    }
                    None => break,
                }
                continue;
            }
            if d == quote {
                closed = true;
                break;
            }
            s.push(d);
        }
        if closed {
            out.push(s);
        }
    }
    out
}

// ---- the guard ------------------------------------------------------------------------------

#[test]
fn no_reference_in_the_repository_depends_on_letter_case() {
    let root = workspace_root();
    let lint = Lint::new(&root);
    let (v, files) = lint.lint_tree();
    assert!(v.is_empty(), "M0-17 case lint:\n{}", v.join("\n"));
    // Non-vacuity: the walk saw the tree, and real references were resolved exactly.
    assert!(files > 200, "only {files} files scanned");
    let refs = real_references(&lint);
    assert!(
        refs.len() > 50,
        "only {} exact references found — the scan is not reading the repo",
        refs.len()
    );
}

/// `(referencing file, string)` for every string in the repo that resolves exactly.
fn real_references(lint: &Lint<'_>) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    let mut sink = Vec::new();
    lint.walk(lint.root, &mut files, &mut sink);
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        let strings = if ext == "rs" {
            rust_strings(&text).unwrap_or_default()
        } else if DATA_EXT.contains(&ext) {
            quoted_strings(&text)
        } else {
            continue;
        };
        for s in strings {
            if !candidate(&s) || s.contains('\\') {
                continue;
            }
            let mut v = Vec::new();
            lint.check(&f, &s, &mut v);
            let norm = s.trim_start_matches('/');
            let exact = f
                .parent()
                .map(|d| lint.fs.resolve(d, norm) == Resolution::Exact)
                .unwrap_or(false)
                || lint.fs.resolve(lint.root, norm) == Resolution::Exact;
            if v.is_empty() && exact {
                out.push((f.clone(), s));
            }
        }
    }
    out
}

// ---- positive controls ----------------------------------------------------------------------

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, text).expect("write");
}

#[test]
fn positive_control_every_case_bug_is_caught() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("case-lint-fixture");
    let _ = std::fs::remove_dir_all(&root);
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/demo\"]\n",
    );
    write(&root, "assets/rock.png", "png");
    write(&root, "crates/demo/themes/dark.ron", "Theme()");
    write(&root, "crates/demo/src/lib.rs", RUST_FIXTURE);
    write(&root, "crates/demo/src/extra.rs", "");
    write(
        &root,
        "crates/demo/Cargo.toml",
        "[package]\nname = \"demo\"\n[lib]\npath = \"src/Lib.rs\"\n[[bin]]\npath = \"src/lib.rs\"\n",
    );
    write(
        &root,
        "crates/demo/scene.ron",
        "Scene(ok: \"../../assets/rock.png\", bad: \"../../assets/ROCK.png\", root_bad: \"Assets/rock.png\")\n",
    );
    write(
        &root,
        "assets/ship.gltf",
        "{\"images\": [{\"uri\": \"rock.png\"}, {\"uri\": \"Rock.PNG\"}]}\n",
    );

    let lint = Lint::new(&root);
    let (v, _) = lint.lint_tree();
    let text = v.join("\n");
    let expect = [
        ("manifest path", "\"src/Lib.rs\""),
        ("include_bytes!", "\"../../assets/Rock.png\""),
        ("concat!(env!(..))", "\"/Themes/dark.ron\""),
        ("#[path]", "\"Extra.rs\""),
        ("RON asset ref", "\"../../assets/ROCK.png\""),
        ("root-relative", "\"Assets/rock.png\""),
        ("backslash", "\"themes\\dark.ron\""),
        ("glTF image uri", "\"Rock.PNG\""),
    ];
    for (what, needle) in expect {
        assert!(
            text.contains(needle),
            "{what} not caught ({needle}):\n{text}"
        );
    }
    // The correct twins are not flagged: exactly the eight bugs, nothing else.
    assert_eq!(v.len(), expect.len(), "unexpected findings:\n{text}");

    // Collisions (cannot be created on a case-insensitive filesystem, so checked directly).
    let names = ["Notes.txt", "NOTES.txt", "src", "lib.rs"].map(String::from);
    assert_eq!(collisions(&names), ["Notes.txt / NOTES.txt"]);
    assert!(collisions(&["a.rs".into(), "b.rs".into()]).is_empty());
}

const RUST_FIXTURE: &str = r#"
#[path = "Extra.rs"]
mod extra;
#[path = "extra.rs"]
mod extra_ok;
pub static ROCK: &[u8] = include_bytes!("../../assets/Rock.png");
pub static ROCK_OK: &[u8] = include_bytes!("../../assets/rock.png");
pub static DARK: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Themes/dark.ron"));
pub static DARK_OK: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/themes/dark.ron"));
pub fn theme() -> std::path::PathBuf { std::path::Path::new("themes\\dark.ron").to_path_buf() }
pub fn not_a_path() -> &'static str { "Hello.World" }
"#;

#[test]
fn positive_control_a_real_reference_with_its_case_flipped_fails() {
    let root = workspace_root();
    let lint = Lint::new(&root);
    let refs = real_references(&lint);
    // Prefer an include_str!/include_bytes! target or a manifest path: the classic carriers.
    let (file, s) = refs
        .iter()
        .find(|(f, s)| f.extension().is_some_and(|e| e == "rs") && s.ends_with(".ron"))
        .or_else(|| refs.first())
        .cloned()
        .expect("the repo has references");
    let last = s.rfind('/').map(|i| i + 1).unwrap_or(0);
    let pos = s[last..]
        .char_indices()
        .find(|(_, c)| c.is_ascii_lowercase())
        .map(|(i, _)| last + i)
        .expect("a lowercase letter to flip");
    let mut flipped = s.clone();
    flipped.replace_range(pos..=pos, &s[pos..=pos].to_ascii_uppercase());
    let mut v = Vec::new();
    lint.check(&file, &flipped, &mut v);
    assert_eq!(
        v.len(),
        1,
        "flipping `{s}` to `{flipped}` in {} must be flagged: {v:?}",
        file.display()
    );
    // And the original is clean.
    let mut ok = Vec::new();
    lint.check(&file, &s, &mut ok);
    assert!(ok.is_empty(), "{ok:?}");
}

#[test]
fn string_extraction_unit_cases() {
    assert_eq!(
        quoted_strings(r#"a = "x/y.ron" b = 'lit\raw' c = "esc\"q""#),
        ["x/y.ron", "lit\\raw", "esc\"q"]
    );
    let rs = rust_strings("/// doc\nfn f() { g!(\"a/b.rs\", r\"c/d.ron\"); }").expect("tokens");
    assert!(
        rs.contains(&"a/b.rs".to_string()) && rs.contains(&"c/d.ron".to_string()),
        "{rs:?}"
    );
    assert!(!candidate("https://example.com/a.png"));
    assert!(!candidate("hello world.txt"));
    assert!(candidate("assets/rock.png"));
}

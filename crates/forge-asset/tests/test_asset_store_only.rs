//! I17 for the asset system (gate row `C-asset-store-only`): forge-asset reaches storage only
//! through the `ProjectStore` trait. Its sources name no filesystem API (`std::fs`, `File`,
//! `OpenOptions`, `read_dir`, ...) and its manifest pulls in no filesystem crate (`notify`,
//! `walkdir`, ...). A project in memory, in git or in S3 then behaves exactly like one in a
//! folder, and the exact-case reference rule (M0-17) cannot be bypassed by an importer that
//! "just opens the file".
//!
//! The scan works on tokens with comments and string literals removed, so a doc comment
//! mentioning `std::fs` is not a violation and `use std::{io, fs}` is.
//!
//! Positive control (W2): `positive_control_every_bypass_spelling_is_caught` feeds synthetic
//! sources with each spelling (and their harmless look-alikes) through the same scanner.

use std::path::Path;

/// Identifiers that are filesystem access wherever they appear in forge-asset.
/// (`File` alone is not: `AssetSource::File` is a variant; `File::open` is caught below.)
const FORBIDDEN_IDENTS: &[&str] = &[
    "OpenOptions",
    "read_dir",
    "DirEntry",
    "create_dir_all",
    "remove_file",
    "canonicalize",
    "notify",
    "walkdir",
    "memmap2",
];

/// Crates that would give forge-asset its own window onto the disk.
const FORBIDDEN_CRATES: &[&str] = &["notify", "walkdir", "memmap2", "tempfile", "glob"];

#[derive(Debug, PartialEq)]
enum Tok {
    Ident(String),
    Punct(char),
}

/// Tokens of Rust source with comments, strings and char literals removed.
fn tokens(src: &str) -> Vec<Tok> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == 'r' && matches!(b.get(i + 1), Some('"') | Some('#')) && {
            // raw string r"..." / r#"..."#
            let mut j = i + 1;
            while b.get(j) == Some(&'#') {
                j += 1;
            }
            b.get(j) == Some(&'"')
        } {
            let mut hashes = 0;
            i += 1;
            while b[i] == '#' {
                hashes += 1;
                i += 1;
            }
            i += 1;
            while i < b.len() {
                if b[i] == '"' && (0..hashes).all(|k| b.get(i + 1 + k) == Some(&'#')) {
                    i += 1 + hashes;
                    break;
                }
                i += 1;
            }
        } else if c == '"' {
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
        } else if c == '\'' && b.get(i + 2) == Some(&'\'') {
            i += 3; // 'x'
        } else if c == '\'' && b.get(i + 1) == Some(&'\\') {
            while i < b.len() && !(b[i] == '\'' && b[i - 1] != '\\' && i > 0) {
                i += 1;
            }
            i += 1;
        } else if c.is_alphabetic() || c == '_' {
            let s = i;
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(b[s..i].iter().collect()));
        } else {
            if !c.is_whitespace() {
                out.push(Tok::Punct(c));
            }
            i += 1;
        }
    }
    out
}

/// Every filesystem access in `src`.
fn violations(src: &str) -> Vec<String> {
    let t = tokens(src);
    let id = |k: usize| match t.get(k) {
        Some(Tok::Ident(s)) => Some(s.as_str()),
        _ => None,
    };
    let is = |k: usize, c: char| t.get(k) == Some(&Tok::Punct(c));
    let mut v = Vec::new();
    for k in 0..t.len() {
        // std::fs
        if id(k) == Some("std") && is(k + 1, ':') && is(k + 2, ':') {
            if id(k + 3) == Some("fs") {
                v.push("std::fs".to_string());
            }
            // use std::{io, fs::read}
            if is(k + 3, '{') {
                let mut depth = 0;
                let mut j = k + 3;
                while j < t.len() {
                    if is(j, '{') {
                        depth += 1;
                    } else if is(j, '}') {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if depth == 1
                        && id(j) == Some("fs")
                        && (is(j - 1, '{') || is(j - 1, ','))
                    {
                        v.push("std::{.., fs}".to_string());
                    }
                    j += 1;
                }
            }
        }
        if let Some(s) = id(k)
            && FORBIDDEN_IDENTS.contains(&s)
        {
            v.push(s.to_string());
        }
        // File::open / File::create / File::options
        if id(k) == Some("File")
            && is(k + 1, ':')
            && is(k + 2, ':')
            && matches!(
                id(k + 3),
                Some("open" | "create" | "create_new" | "options")
            )
        {
            v.push("File::".to_string());
        }
    }
    v
}

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).expect("list").flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn forge_asset_sources_touch_no_filesystem() {
    let mut files = Vec::new();
    rust_files(&crate_dir().join("src"), &mut files);
    assert!(files.len() >= 10, "found the sources ({})", files.len());
    let mut all = Vec::new();
    for f in files {
        let src = std::fs::read_to_string(&f).expect("read");
        for v in violations(&src) {
            all.push(format!("{}: {v}", f.display()));
        }
    }
    assert!(
        all.is_empty(),
        "filesystem access outside ProjectStore (I17):\n{}",
        all.join("\n")
    );
}

#[test]
fn forge_asset_depends_on_no_filesystem_crate() {
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml")).expect("manifest");
    let deps: Vec<&str> = manifest
        .lines()
        .map(str::trim)
        .filter_map(|l| l.split_once('=').map(|(k, _)| k.trim()))
        .collect();
    for c in FORBIDDEN_CRATES {
        assert!(!deps.contains(c), "forge-asset depends on {c} (I17)");
    }
}

#[test]
fn positive_control_every_bypass_spelling_is_caught() {
    let bad = [
        "use std::fs;",
        "fn f() { let _ = std :: fs :: read(\"x\"); }",
        "use std::{io, fs};",
        "use std::{fs::read_to_string, io};",
        "use std::fs as disk;",
        "fn f() { let _ = File::open(\"x\"); }",
        "fn f() { let _ = OpenOptions::new(); }",
        "fn f(p: &Path) { for _ in p.read_dir() {} }",
        "fn f() { notify::recommended_watcher(); }",
    ];
    for src in bad {
        assert!(!violations(src).is_empty(), "not caught: {src}");
    }
    let fine = [
        "// std::fs is not used here",
        "/* use std::fs; */ fn f() {}",
        "fn f() -> &'static str { \"std::fs::read\" }",
        "fn f() -> &'static str { r#\"File::open\"# }",
        "struct S { fs_cache: u8, files: u8 } fn g(s: S) -> u8 { s.fs_cache }",
        "use std::{io, sync::Arc};",
        "enum Source { File(u8) } fn f(s: Source) { match s { Source::File(_) => {} } }",
        "fn f(c: char) -> bool { c == '\"' }",
    ];
    for src in fine {
        assert!(
            violations(src).is_empty(),
            "false alarm: {src} -> {:?}",
            violations(src)
        );
    }
    // And the manifest check sees a dependency line.
    assert!(
        "notify = \"8\""
            .split_once('=')
            .is_some_and(|(k, _)| FORBIDDEN_CRATES.contains(&k.trim()))
    );
}

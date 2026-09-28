//! `cargo xtask plan-coverage` — every directory in the repo is claimed by the master plan's
//! repo tree (master-plan.md, "Repo tree — coverage checklist").
//!
//! The tree is parsed straight out of the fenced block under that heading, so the plan text
//! *is* the data: there is no second list to drift.
//!
//! Claim rules:
//! * A directory listed in the tree with children listed under it is a **container**: it is
//!   claimed, but each of its sub-directories must be claimed too (`crates/`, `tests/`).
//! * A directory listed with no children is a **leaf**: it owns its whole subtree
//!   (`crates/forge-num/src/...`, `presets/2d/...`).
//! * Anything else is an orphan and fails, naming the path and what to do about it.
//!
//! Directories are enumerated with `git ls-files --cached --others --exclude-standard`, so
//! build output and anything `.gitignore`d never counts; with no git available it falls back
//! to a filesystem walk that skips `.git` and `target`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use crate::util::{Failure, XResult, read};

pub const PLAN_FILE: &str = "docs/plan/master-plan.md";
const TREE_HEADING: &str = "## Repo tree";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// Claimed directory path (relative, `/`-separated) → is it a leaf (owns its subtree)?
    pub dirs: BTreeMap<String, bool>,
}

impl Claims {
    /// Is `dir` claimed? Returns `Err(parent container)` naming where it should be listed.
    pub fn check(&self, dir: &str) -> Result<(), String> {
        if self.dirs.contains_key(dir) {
            return Ok(());
        }
        // Walk up: the nearest claimed ancestor decides.
        let mut cur = dir;
        while let Some((parent, _)) = cur.rsplit_once('/') {
            if let Some(&leaf) = self.dirs.get(parent) {
                return if leaf {
                    Ok(())
                } else {
                    Err(parent.to_string())
                };
            }
            cur = parent;
        }
        Err(String::new())
    }
}

/// Extract the fenced tree block under the "Repo tree" heading.
pub fn tree_block(plan: &str) -> XResult<&str> {
    let start = plan
        .find(TREE_HEADING)
        .ok_or_else(|| Failure::one(format!("{PLAN_FILE}: no \"{TREE_HEADING}\" heading")))?;
    let rest = &plan[start..];
    let open = rest
        .find("```")
        .ok_or_else(|| Failure::one(format!("{PLAN_FILE}: repo tree has no code fence")))?;
    let body = &rest[open + 3..];
    let body = body.split_once('\n').map_or("", |(_, b)| b);
    let close = body
        .find("```")
        .ok_or_else(|| Failure::one(format!("{PLAN_FILE}: repo tree fence never closes")))?;
    Ok(&body[..close])
}

/// Parse tree lines like `│   ├── forge-num/        Ch.3   ...`. The depth is the character
/// column of the name divided by four (the tree's indent unit). A line may list several
/// directories (`plugin/  store/  preset/   Ch.30`): all leading tokens that end in `/`.
pub fn parse_tree(block: &str) -> XResult<Claims> {
    let mut stack: Vec<String> = Vec::new(); // path segments by depth (depth 1 = index 0)
    let mut entries: Vec<(String, usize)> = Vec::new(); // (path, depth)
    let mut root_seen = false;
    for (lineno, line) in block.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let Some(col) = chars
            .iter()
            .position(|c| !matches!(c, '│' | '├' | '└' | '─' | ' ' | '\t'))
        else {
            continue;
        };
        let text: String = chars[col..].iter().collect();
        if !root_seen {
            // The first line is the repo root (`forge/`).
            root_seen = true;
            continue;
        }
        if col % 4 != 0 || col == 0 {
            return Err(Failure::one(format!(
                "{PLAN_FILE}: repo tree line {} is not on a 4-column indent: {line:?}",
                lineno + 1
            )));
        }
        let depth = col / 4;
        let dirs: Vec<&str> = text
            .split_whitespace()
            .take_while(|t| t.ends_with('/'))
            .map(|t| t.trim_end_matches('/'))
            .collect();
        stack.truncate(depth - 1);
        if stack.len() != depth - 1 {
            return Err(Failure::one(format!(
                "{PLAN_FILE}: repo tree line {} is indented deeper than its parent: {line:?}",
                lineno + 1
            )));
        }
        if dirs.is_empty() {
            // A file entry (`justfile`, `defects.md`). Files are not checked, but a file
            // still occupies its depth so siblings resolve correctly.
            continue;
        }
        for (i, d) in dirs.iter().enumerate() {
            let mut path = stack.clone();
            path.push((*d).to_string());
            entries.push((path.join("/"), depth));
            if i == dirs.len() - 1 {
                stack.push((*d).to_string());
            }
        }
    }
    let mut claims = Claims::default();
    for (path, _) in &entries {
        let has_child = entries.iter().any(|(p, _)| {
            p.len() > path.len() && p.starts_with(path.as_str()) && p.as_bytes()[path.len()] == b'/'
        });
        claims.dirs.insert(path.clone(), !has_child);
    }
    Ok(claims)
}

/// Every non-ignored file (relative, `/`-separated): `git ls-files`, or a filesystem walk
/// that skips `.git` and `target` where git is unavailable.
pub fn repo_files(root: &Path) -> Vec<String> {
    git_files(root).unwrap_or_else(|| walk_files(root))
}

/// All directories (relative, `/`-separated) that contain at least one non-ignored file.
pub fn repo_dirs(root: &Path) -> BTreeSet<String> {
    let files = repo_files(root);
    let mut dirs = BTreeSet::new();
    for f in files {
        let mut cur = f.as_str();
        while let Some((parent, _)) = cur.rsplit_once('/') {
            dirs.insert(parent.to_string());
            cur = parent;
        }
    }
    dirs
}

fn git_files(root: &Path) -> Option<Vec<String>> {
    let out = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    Some(
        s.split('\0')
            .filter(|f| !f.is_empty())
            // A file deleted in the working tree but still in the index is not there.
            .filter(|f| root.join(f).exists())
            .map(str::to_string)
            .collect(),
    )
}

fn walk_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if name != ".git" && name != "target" {
                    stack.push(p);
                }
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}

pub fn check(claims: &Claims, dirs: &BTreeSet<String>) -> XResult<()> {
    let errs: Vec<String> = dirs
        .iter()
        .filter_map(|d| {
            claims.check(d).err().map(|parent| {
                let under = if parent.is_empty() {
                    "at the top level".to_string()
                } else {
                    format!("under {parent}/")
                };
                format!(
                    "directory {d}/ is not claimed by any chapter: add it {under} in the repo tree of \
                     {PLAN_FILE} with the chapter that specifies it, or remove it"
                )
            })
        })
        .collect();
    Failure::many(errs)
}

pub fn run(root: &Path) -> XResult<String> {
    let plan = read(&root.join(PLAN_FILE))?;
    let claims = parse_tree(tree_block(&plan)?)?;
    let dirs = repo_dirs(root);
    check(&claims, &dirs)?;
    Ok(format!(
        "plan-coverage: {} directories, all claimed ({} claims in the repo tree)\n",
        dirs.len(),
        claims.dirs.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::workspace_root;

    const TREE: &str = "\
forge/
├── crates/
│   ├── forge-num/        Ch.3   deterministic math
│   └── forge-runtime/    Ch.28  the shipping runtime binary
├── tests/
│   ├── determinism/       Ch.3, Ch.30
│   ├── plugin/  store/  preset/   Ch.30
│   └── e2e/               Ch.30   serial leg
├── xtask/                 Ch.30   gate, dod
├── presets/               Ch.31  2d/ 3d/ — DATA, not code
├── docs/
│   ├── defects.md         W8 — the one allocator
│   └── adr/               Ch.30
└── justfile               Ch.30
";

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_containers_leaves_and_multi_entries() {
        let c = parse_tree(TREE).unwrap();
        assert_eq!(c.dirs.get("crates"), Some(&false));
        assert_eq!(c.dirs.get("crates/forge-num"), Some(&true));
        assert_eq!(c.dirs.get("tests/store"), Some(&true));
        assert_eq!(c.dirs.get("tests/preset"), Some(&true));
        assert_eq!(
            c.dirs.get("presets"),
            Some(&true),
            "tokens after Ch. are not dirs"
        );
        assert!(!c.dirs.contains_key("presets/2d") && !c.dirs.contains_key("2d"));
        assert_eq!(c.dirs.get("docs/adr"), Some(&true));
        assert_eq!(c.dirs.get("docs"), Some(&false));
    }

    #[test]
    fn claimed_dirs_and_leaf_subtrees_pass() {
        let c = parse_tree(TREE).unwrap();
        check(
            &c,
            &set(&[
                "crates",
                "crates/forge-num",
                "crates/forge-num/src",
                "crates/forge-num/src/det",
                "tests",
                "tests/e2e",
                "presets/2d",
                "xtask/src",
                "docs/adr",
            ]),
        )
        .unwrap();
    }

    #[test]
    fn positive_control_an_unclaimed_crate_fails() {
        let c = parse_tree(TREE).unwrap();
        let err = check(
            &c,
            &set(&["crates", "crates/forge-mystery", "crates/forge-mystery/src"]),
        )
        .unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("directory crates/forge-mystery/ is not claimed"),
            "{s}"
        );
        assert!(s.contains("under crates/"), "{s}");
        // The orphan's own subtree is reported too, so nothing inside it is silently accepted.
        assert!(s.contains("crates/forge-mystery/src/"), "{s}");
    }

    #[test]
    fn positive_control_an_unclaimed_top_level_dir_fails() {
        let c = parse_tree(TREE).unwrap();
        let err = check(&c, &set(&["scratch"])).unwrap_err();
        assert!(err.to_string().contains("at the top level"), "{err}");
    }

    #[test]
    fn positive_control_an_unclaimed_dir_inside_a_container_fails() {
        let c = parse_tree(TREE).unwrap();
        assert!(check(&c, &set(&["docs/plan"])).is_err());
        assert!(check(&c, &set(&["tests/fuzz"])).is_err());
    }

    #[test]
    fn misindented_tree_is_rejected() {
        assert!(parse_tree("forge/\n├── a/\n│     ├── b/\n").is_err());
    }

    #[test]
    fn the_real_plan_tree_parses_and_the_repo_is_covered() {
        let root = workspace_root();
        let plan = read(&root.join(PLAN_FILE)).unwrap();
        let claims = parse_tree(tree_block(&plan).unwrap()).unwrap();
        for must in [
            "crates/forge-num",
            "crates/forge-runtime",
            "tools/forge-server",
            "tests/licence",
            "tests/store",
            "xtask",
            "docs/adr",
            "docs/plan",
            ".github",
            ".cargo",
        ] {
            assert!(
                claims.dirs.contains_key(must),
                "{must} missing from parsed tree"
            );
        }
        run(&root).expect("the repo must be fully claimed by the plan tree");
    }
}

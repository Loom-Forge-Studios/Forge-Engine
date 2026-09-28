//! `cargo xtask timed-gates` — WP-40: every wall-clock gate of the workspace times through
//! `forge_trace::timed::run_timed_alone` and runs in nextest's serial `wall-clock` group.
//!
//! A timed body run through that helper runs in a process of its own, at the High priority
//! class, holding the machine-wide timed lock, and only once the machine is quiet (ADR 0042,
//! ADR 0057). A test that reads the clock and asserts on it *without* the helper measures
//! beside whatever else runs — another lane's compiler, a GPU test — and fails integrations
//! the way WP-36's, WP-U17's, WP-U18's and WP-39's did. So, for every Rust file under a
//! workspace member's `tests/` (and the repo-level `tests/` suites):
//!
//! 1. a file that reads the wall clock (`Instant::now`) either **times through the helper**
//!    (`run_timed_alone(`, `run_timed_alone_with(` or `run_timed(`) or carries a declaration
//!    `// timed-gates: exempt(<why this is not a wall-clock gate>)` — a timeout, a hang guard,
//!    a number printed for the record;
//! 2. a file that times through the helper is a **test binary** (not a shared module) and that
//!    binary is named (`binary(<name>)`) in a `.config/nextest.toml` override whose
//!    `test-group` is `wall-clock` and whose `threads-required` is `num-cpus`, and the group
//!    runs one test at a time (`max-threads = 1`): in a nextest run nothing else of the run
//!    shares the machine with it;
//! 3. every `binary(<name>)` in the group names a test binary that exists (a typo would drop a
//!    gate from the group without a sound).
//!
//! The check is per file: a file that times through the helper is trusted for all its tests.
//! Unit-test modules under `src/` are not scanned (none holds a wall-clock gate; their clock
//! reads are hang guards). The positive controls are this module's unit tests.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::util::{Failure, XResult, is_banned_reason};

/// The nextest group every timed binary runs in.
pub const GROUP: &str = "wall-clock";
/// The declaration a file that reads the clock without gating on it carries.
pub const EXEMPT: &str = "timed-gates: exempt(";

/// The calls that route a timed body through the helper (the perf gate's `measuring` wraps
/// `run_timed_alone`).
const ROUTES: [&str; 4] = [
    "run_timed_alone(",
    "run_timed_alone_with(",
    "run_timed(",
    "forge_perf_gate::measuring(",
];

/// The test binaries of the `wall-clock` group in a nextest config, or what is wrong with it.
pub fn wall_clock_group(nextest_toml: &str) -> Result<BTreeSet<String>, Vec<String>> {
    let table: toml::Table = nextest_toml
        .parse()
        .map_err(|e| vec![format!(".config/nextest.toml does not parse: {e}")])?;
    let mut errs = Vec::new();
    let max = table
        .get("test-groups")
        .and_then(|g| g.get(GROUP))
        .and_then(|g| g.get("max-threads"))
        .and_then(toml::Value::as_integer);
    if max != Some(1) {
        errs.push(format!(
            ".config/nextest.toml: [test-groups] {GROUP} must be {{ max-threads = 1 }} (found {max:?})"
        ));
    }
    let overrides = table
        .get("profile")
        .and_then(|p| p.get("default"))
        .and_then(|d| d.get("overrides"))
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut names = BTreeSet::new();
    for o in &overrides {
        if o.get("test-group").and_then(toml::Value::as_str) != Some(GROUP) {
            continue;
        }
        let filter = o.get("filter").and_then(toml::Value::as_str).unwrap_or("");
        if o.get("threads-required").and_then(toml::Value::as_str) != Some("num-cpus") {
            errs.push(format!(
                ".config/nextest.toml: the {GROUP} override `{filter}` must set \
                 threads-required = \"num-cpus\" (nothing else of the run beside a timed test)"
            ));
        }
        names.extend(binaries_in(filter));
    }
    if names.is_empty() {
        errs.push(format!(
            ".config/nextest.toml: no override puts any binary in the {GROUP} group"
        ));
    }
    if errs.is_empty() {
        Ok(names)
    } else {
        Err(errs)
    }
}

/// The `binary(<name>)` names in a nextest filter expression.
fn binaries_in(filter: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = filter;
    while let Some(i) = rest.find("binary(") {
        rest = &rest[i + "binary(".len()..];
        if let Some(j) = rest.find(')') {
            out.push(rest[..j].trim().to_string());
            rest = &rest[j..];
        }
    }
    out
}

/// The source with `//` comments removed (clock reads and calls in comments do not count).
fn code_only(src: &str) -> String {
    src.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The reasons of the exemptions a file declares (`// timed-gates: exempt(<why>)`).
fn exemptions(src: &str) -> Vec<String> {
    src.lines()
        .filter_map(|l| {
            let t = l.trim_start();
            if !t.starts_with("//") {
                return None;
            }
            let i = t.find(EXEMPT)?;
            let rest = &t[i + EXEMPT.len()..];
            Some(
                rest.rfind(')')
                    .map_or(rest, |j| &rest[..j])
                    .trim()
                    .to_string(),
            )
        })
        .collect()
}

/// Check one test file (`rel` for messages; `binary`: its test binary's name, `None` for a
/// module another file includes) against the group.
pub fn check_file(
    rel: &str,
    src: &str,
    binary: Option<&str>,
    group: &BTreeSet<String>,
) -> Vec<String> {
    let code = code_only(src);
    let reads_clock = code.contains("Instant::now");
    let routed = ROUTES.iter().any(|r| code.contains(r));
    let exempt = exemptions(src);
    let mut errs = Vec::new();
    for why in &exempt {
        if is_banned_reason(why) {
            errs.push(format!(
                "{rel}: `// {EXEMPT}{why})` needs a real reason (why this clock read is not a \
                 wall-clock gate), not {why:?}"
            ));
        }
    }
    if routed {
        match binary {
            None => errs.push(format!(
                "{rel}: times through run_timed_alone from a module, not a test binary: call it \
                 from the test file so its binary can be checked against the {GROUP} group"
            )),
            Some(b) if !group.contains(b) => errs.push(format!(
                "{rel}: test binary `{b}` times through run_timed_alone but is not in nextest's \
                 serial {GROUP} group: add `binary({b})` to the {GROUP} override in \
                 .config/nextest.toml (WP-40)"
            )),
            Some(_) => {}
        }
    } else if reads_clock && exempt.is_empty() {
        errs.push(format!(
            "{rel}: reads the wall clock (Instant::now) but neither times through \
             forge_trace::timed::run_timed_alone nor declares `// {EXEMPT}<why>)`: a wall-clock \
             gate must run alone on a quiet machine (WP-40, W5)"
        ));
    }
    errs
}

/// A workspace member: its directory and whether cargo discovers its tests automatically.
struct Member {
    dir: PathBuf,
    /// `(path relative to the member, name)` of its `[[test]]` targets.
    targets: Vec<(String, String)>,
    autotests: bool,
}

fn member(dir: &Path) -> Option<Member> {
    let src = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let t: toml::Table = src.parse().ok()?;
    let autotests = t
        .get("package")
        .and_then(|p| p.get("autotests"))
        .and_then(toml::Value::as_bool)
        != Some(false);
    let targets = t
        .get("test")
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let name = e.get("name")?.as_str()?.to_string();
                    let path = e
                        .get("path")
                        .and_then(toml::Value::as_str)
                        .map_or_else(|| format!("tests/{name}.rs"), str::to_string);
                    Some((path.replace('\\', "/"), name))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Member {
        dir: dir.to_path_buf(),
        targets,
        autotests,
    })
}

fn members(root: &Path) -> Vec<Member> {
    let mut out = Vec::new();
    for d in ["crates", "plugins", "tools", "samples"] {
        let Ok(rd) = std::fs::read_dir(root.join(d)) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        dirs.sort();
        out.extend(dirs.iter().filter_map(|p| member(p)));
    }
    for d in ["tests", "tests-premium", "xtask"] {
        out.extend(member(&root.join(d)));
    }
    out
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rs_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// The test binary a file of `m` is (`rel`: relative to the member, `/`-separated).
fn binary_of(m: &Member, rel: &str) -> Option<String> {
    if let Some((_, name)) = m.targets.iter().find(|(p, _)| p == rel) {
        return Some(name.clone());
    }
    if !m.autotests {
        return None;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        ["tests", f] => f.strip_suffix(".rs").map(str::to_string),
        ["tests", d, "main.rs"] => Some((*d).to_string()),
        _ => None,
    }
}

/// Every violation in the repository at `root`.
pub fn check_all(root: &Path) -> Vec<String> {
    let nextest = match std::fs::read_to_string(root.join(".config").join("nextest.toml")) {
        Ok(s) => s,
        Err(e) => return vec![format!(".config/nextest.toml: {e}")],
    };
    let group = match wall_clock_group(&nextest) {
        Ok(g) => g,
        Err(errs) => return errs,
    };
    let mut errs = Vec::new();
    let mut binaries = BTreeSet::new();
    for m in members(root) {
        // The repo-level suites live in tests/<suite>/ and tests-premium/<suite>/ (autotests
        // off, [[test]] paths).
        let scan = if m.dir == root.join("tests") || m.dir == root.join("tests-premium") {
            m.dir.clone()
        } else {
            m.dir.join("tests")
        };
        let mut files = Vec::new();
        rs_files(&scan, &mut files);
        for f in files {
            let Ok(rel) = f.strip_prefix(&m.dir) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let binary = binary_of(&m, &rel);
            if let Some(b) = &binary {
                binaries.insert(b.clone());
            }
            let Ok(src) = crate::util::read(&f) else {
                continue;
            };
            let shown = f.strip_prefix(root).map_or_else(
                |_| f.display().to_string(),
                |p| p.to_string_lossy().replace('\\', "/"),
            );
            errs.extend(check_file(&shown, &src, binary.as_deref(), &group));
        }
        // [[test]] targets outside tests/ (none today) still count as binaries.
        binaries.extend(m.targets.iter().map(|(_, n)| n.clone()));
    }
    errs.extend(stale_names(&group, &binaries));
    errs
}

/// The group's names that are no test binary of the workspace.
fn stale_names(group: &BTreeSet<String>, binaries: &BTreeSet<String>) -> Vec<String> {
    group
        .difference(binaries)
        .map(|b| {
            format!(
                ".config/nextest.toml: the {GROUP} group names binary({b}), which is no test \
                 binary of the workspace (a typo drops a gate from the group)"
            )
        })
        .collect()
}

/// `cargo xtask timed-gates`.
pub fn run(root: &Path) -> XResult<String> {
    let errs = check_all(root);
    if errs.is_empty() {
        Ok(
            "timed-gates: every wall-clock test file times through run_timed_alone in the \
             serial wall-clock group, or declares why it is not a gate\n"
                .to_string(),
        )
    } else {
        Err(Failure { messages: errs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::workspace_root;

    const NEXTEST: &str = r#"
[test-groups]
wall-clock = { max-threads = 1 }

[[profile.default.overrides]]
filter = 'binary(test_gate) | binary(test_other)'
test-group = 'wall-clock'
threads-required = "num-cpus"
"#;

    fn group() -> BTreeSet<String> {
        wall_clock_group(NEXTEST).expect("a valid group")
    }

    const TIMES_ALONE: &str = "use std::time::Instant;\n#[test]\nfn gate() {\n    \
        let r = forge_trace::timed::run_timed_alone(|| {\n        let t = Instant::now();\n        \
        work();\n        let ms = t.elapsed().as_secs_f64() * 1e3;\n        \
        if ms <= 2.0 { Ok(format!(\"{ms}\")) } else { Err(format!(\"{ms}\")) }\n    });\n    \
        r.unwrap();\n}\n";

    /// Positive control (W2): a file that times without the helper — the shape of the
    /// pre-WP-40 solve-budget tests — is flagged.
    const TIMES_BESIDE_EVERYTHING: &str = "use std::time::Instant;\n#[test]\nfn gate() {\n    \
        let t = Instant::now();\n    work();\n    let ms = t.elapsed().as_secs_f64() * 1e3;\n    \
        assert!(ms <= 2.0, \"{ms} ms\");\n}\n";

    #[test]
    fn a_gate_timed_alone_in_the_group_passes() {
        assert!(check_file("t.rs", TIMES_ALONE, Some("test_gate"), &group()).is_empty());
    }

    #[test]
    fn positive_control_a_file_that_times_without_the_helper_is_flagged() {
        let errs = check_file("t.rs", TIMES_BESIDE_EVERYTHING, Some("test_gate"), &group());
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("neither times through"), "{errs:?}");
    }

    #[test]
    fn positive_control_a_timed_binary_outside_the_group_is_flagged() {
        let errs = check_file("t.rs", TIMES_ALONE, Some("test_elsewhere"), &group());
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].contains("not in nextest's serial wall-clock group"),
            "{errs:?}"
        );
        let module = check_file("t.rs", TIMES_ALONE, None, &group());
        assert!(module[0].contains("from a module"), "{module:?}");
    }

    #[test]
    fn an_exemption_needs_a_reason_and_comments_do_not_count() {
        let timeout = format!(
            "// {EXEMPT}a hang guard: the loop gives up after 30 s)\n{TIMES_BESIDE_EVERYTHING}"
        );
        assert!(check_file("t.rs", &timeout, Some("x"), &group()).is_empty());
        let lazy = format!("// {EXEMPT}todo)\n{TIMES_BESIDE_EVERYTHING}");
        let errs = check_file("t.rs", &lazy, Some("x"), &group());
        assert!(
            errs.iter().any(|e| e.contains("needs a real reason")),
            "{errs:?}"
        );
        // A clock read in a comment is not a clock read; a run_timed_alone in one is no route.
        let doc = "//! times with Instant::now() through run_timed_alone(..)\nfn f() {}\n";
        assert!(check_file("t.rs", doc, Some("x"), &group()).is_empty());
    }

    #[test]
    fn the_group_must_run_alone_and_serially() {
        let loose = NEXTEST.replace("threads-required = \"num-cpus\"", "");
        assert!(wall_clock_group(&loose).is_err());
        let parallel = NEXTEST.replace("max-threads = 1", "max-threads = 4");
        assert!(wall_clock_group(&parallel).is_err());
        assert_eq!(
            group().into_iter().collect::<Vec<_>>(),
            ["test_gate", "test_other"]
        );
    }

    #[test]
    fn positive_control_a_misspelt_group_entry_is_flagged() {
        let binaries: BTreeSet<String> = ["test_gate".to_string()].into();
        let errs = stale_names(&group(), &binaries);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("binary(test_other)"), "{errs:?}");
    }

    #[test]
    fn the_committed_repository_holds_the_guard() {
        let errs = check_all(&workspace_root());
        assert!(errs.is_empty(), "{}", errs.join("\n"));
    }
}

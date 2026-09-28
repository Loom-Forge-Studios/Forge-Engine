//! `cargo xtask export-public <out-dir> [--docs-only | --no-test]` — write the public edition
//! (ADR 0059).
//!
//! The export is one-way: the private repository is the source, and the public tree is
//! produced from it, never edited. What it writes:
//!
//! * every repository file that is not premium ([`Manifest::excluded`]), or with
//!   `--docs-only` only the manifest's `docs_only` paths;
//! * markdown scrubbed ([`scrub`]): premium sections, headings, rows and sentences cut,
//!   citations of cut things dropped, links to anything not exported rewritten;
//! * `docs/plan/dod-status.ron` without the rows milestones.md no longer has (and the other
//!   way round: a status row that names something premium takes its milestone row with it),
//!   `tests/gates.ron` without the premium rows, and the workspace `Cargo.toml` without the
//!   premium crates;
//! * the manifest's public files in place of the private README and CONTRIBUTING;
//! * for a code export, a fresh `Cargo.lock` (`cargo generate-lockfile --offline`).
//!
//! **Refusal.** A code export refuses while `premium_debt.ron` is not empty, naming the debt:
//! the base still needs premium code, so the public tree would not build. `--docs-only`
//! works regardless.
//!
//! **Leak scan.** Every export re-reads what it wrote and fails if any file names a premium
//! identifier (any file; the premium DoD ids, gate rows and error codes are identifiers too,
//! and a range citation such as `M7-35..M7-39` names every id it spans), a doc term (prose:
//! markdown and the text files under docs/) or a code term (every other file), or a markdown
//! link dangles. When the debt is zero, a code export then builds the exported tree offline
//! (`cargo build --workspace --all-targets`) and runs its tests (`cargo nextest run` when
//! cargo-nextest is installed, as `just verify` does, else `cargo test`). `--no-test`
//! stops after the build, for a caller that runs the tests itself (in parts: the full suite
//! outlasts a ten-minute command, WP-44).
//!
//! **The out dir** must be empty (a `.git` directory is kept), or hold a previous export
//! (it has the [`MARKER`] file); it is cleared before writing and may not be inside the
//! repository.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::boundary::{self, Debt, GATES_FILE, MILESTONES_FILE, STATUS_FILE};
use super::scan::Matcher;
use super::scrub::{self, Dangling, Rules, Stats};
use super::{DEBT_FILE, MANIFEST_FILE, Manifest};
use crate::util::{Failure, XResult, read};

/// Written into every export; a directory holding it may be cleared by the next export.
pub const MARKER: &str = ".forge-public-export";

pub const PLAN_FILE: &str = "docs/plan/master-plan.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub out: PathBuf,
    pub docs_only: bool,
    /// Build and test the exported tree after a code export (always on from the command
    /// line; unit tests of the export's mechanics turn it off).
    pub build: bool,
    /// Build it, but leave its tests to the caller (`--no-test`).
    pub no_test: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub files: Vec<(String, u64)>,
    pub stats: Stats,
    pub links_rewritten: usize,
    pub dod_dropped: Vec<String>,
    pub chapters_cut: Vec<u32>,
    pub built: bool,
    /// Its tests ran and passed (not with `--no-test`).
    pub tested: bool,
}

impl Report {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|(_, b)| *b).sum()
    }
}

/// The refusal message for a code export with debt outstanding.
pub fn refusal(debt: &Debt) -> String {
    let mut s = format!(
        "export-public: refusing to export code: {DEBT_FILE} is not empty (size {}, by work package {:?}). The base still depends on premium code, so the public tree would not build. Use --docs-only, or burn the debt down (WP-42/43/44):\n",
        debt.size(),
        debt.by_wp()
    );
    for d in &debt.deps {
        s.push_str(&format!("  {} dep: {} -> {}\n", d.wp, d.krate, d.on));
    }
    for l in &debt.lodged {
        s.push_str(&format!("  {} lodged: {}\n", l.wp, l.path));
    }
    for n in &debt.names {
        s.push_str(&format!(
            "  {} names: {} [{}]\n",
            n.wp,
            n.path,
            n.identifiers.join(", ")
        ));
    }
    s
}

/// The scrub's matcher for markdown: the hard identifiers but the DoD ids (the scrub treats
/// those as citations, see [`scrub::Dangling`]) and the doc terms.
pub fn doc_matcher(m: &Manifest) -> Matcher {
    Matcher::new(m.names_without_dod(), m.doc_terms.clone())
}

/// The leak scan's matcher for prose: every hard identifier (the DoD ids, gate rows and error
/// codes included, and a range citation spanning one) and the doc terms.
pub fn leak_matcher(m: &Manifest) -> Matcher {
    Matcher::new(m.hard_identifiers(), m.doc_terms.clone())
}

/// ADR numbers whose file is not exported.
fn excluded_adrs(m: &Manifest, files: &[String]) -> BTreeSet<u32> {
    files
        .iter()
        .filter(|f| m.excluded(f))
        .filter_map(|f| f.strip_prefix("docs/adr/"))
        .filter_map(|n| n.get(..4)?.parse().ok())
        .collect()
}

fn headings(text: &str) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let mut fence = false;
    for l in text.lines() {
        if l.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if !fence && l.starts_with('#') {
            let h = l.trim_start_matches('#');
            if h.starts_with(' ') {
                *out.entry(h.trim().to_string()).or_default() += 1;
            }
        }
    }
    out
}

/// The headings of `file` that the export cuts (marked, or naming a premium thing).
pub fn cut_headings_of(
    m: &Manifest,
    file: &str,
    src: &str,
    matcher: &Matcher,
) -> Result<Vec<String>, String> {
    let rw = rewrites_for(m, file);
    let (text, _) = scrub::apply_rewrites(src, &rw);
    let before = headings(&text);
    let (cut1, _) = scrub::cut_marked(&text, &sections_for(m, file))?;
    let (cut2, _) = scrub::cut_headings(&cut1, matcher);
    let after = headings(&cut2);
    Ok(before
        .into_iter()
        .filter(|(h, n)| after.get(h).copied().unwrap_or(0) < *n)
        .map(|(h, _)| h)
        .collect())
}

/// Cut master-plan chapters, and cut `(chapter, section)` pairs.
pub type PlanCuts = (BTreeSet<u32>, BTreeSet<(u32, u32)>);

/// The master-plan chapters and sections the export cuts (marked or by heading).
pub fn plan_cuts(m: &Manifest, plan: &str, matcher: &Matcher) -> Result<PlanCuts, String> {
    let mut chapters = BTreeSet::new();
    let mut sections = BTreeSet::new();
    for h in cut_headings_of(m, PLAN_FILE, plan, matcher)? {
        match scrub::heading_number(&h) {
            Some((c, None)) => {
                chapters.insert(c);
            }
            Some((c, Some(s))) => {
                sections.insert((c, s));
            }
            None => {}
        }
    }
    Ok((chapters, sections))
}

/// The milestones (`M3`, ...) whose whole section the export cuts from milestones.md.
pub fn milestone_cuts(m: &Manifest, ms: &str, matcher: &Matcher) -> Result<Vec<String>, String> {
    Ok(cut_headings_of(m, MILESTONES_FILE, ms, matcher)?
        .into_iter()
        .filter_map(|h| {
            let id: String = h.split_whitespace().next()?.to_string();
            let digits = id.strip_prefix('M')?;
            (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(id)
        })
        .collect())
}

fn rewrites_for<'a>(m: &'a Manifest, file: &str) -> Vec<(&'a str, &'a str)> {
    m.rewrites
        .iter()
        .filter(|r| r.file == file)
        .map(|r| (r.from.as_str(), r.to.as_str()))
        .collect()
}

fn sections_for(m: &Manifest, file: &str) -> BTreeSet<String> {
    m.doc_sections
        .iter()
        .filter(|s| s.file == file)
        .map(|s| s.id.clone())
        .collect()
}

/// The exported texts of the markdown files and `dod-status.ron`, with the DoD ids dropped.
pub struct Docs {
    pub texts: BTreeMap<String, String>,
    pub stats: Stats,
    pub links_rewritten: usize,
    pub dod_dropped: Vec<String>,
    pub chapters_cut: Vec<u32>,
}

/// Scrub every exported markdown file and the DoD status file (see the module docs).
pub fn scrub_docs(root: &Path, m: &Manifest, files: &[String], all: &[String]) -> XResult<Docs> {
    let matcher = doc_matcher(m);
    let mut dangling = Dangling {
        adrs: excluded_adrs(m, all),
        files: m.withheld.iter().map(|w| w.path.clone()).collect(),
        ..Dangling::default()
    };
    if let Ok(plan) = read(&root.join(PLAN_FILE)) {
        let (c, s) =
            plan_cuts(m, &plan, &matcher).map_err(|e| Failure::one(format!("{PLAN_FILE}: {e}")))?;
        dangling.chapters = c;
        dangling.sections = s;
    }
    let md: Vec<&String> = files.iter().filter(|f| f.ends_with(".md")).collect();
    let mut sources = BTreeMap::new();
    for f in &md {
        sources.insert((*f).clone(), read(&root.join(f))?);
    }
    let status_src = if files.iter().any(|f| f == STATUS_FILE) {
        Some(read(&root.join(STATUS_FILE))?)
    } else {
        None
    };
    let source_ids: BTreeSet<String> = sources
        .get(MILESTONES_FILE)
        .map(|t| crate::dod::milestone_ids(t).into_iter().collect())
        .unwrap_or_default();
    let mut dropped: BTreeSet<String> = m.dod_list().into_iter().collect();
    // A milestone cut whole (`M3`) is cited like a DoD id: a citation of it dangles.
    if let Some(ms) = sources.get(MILESTONES_FILE) {
        dropped.extend(
            milestone_cuts(m, ms, &matcher)
                .map_err(|e| Failure::one(format!("{MILESTONES_FILE}: {e}")))?,
        );
    }
    // Milestones and DoD status drop each other's rows: iterate to a fixed point.
    let (texts, stats, status_text) = loop {
        dangling.dod = dropped.clone();
        let rules = Rules {
            matcher: &matcher,
            dangling: &dangling,
        };
        let mut texts = BTreeMap::new();
        let mut stats = Stats::default();
        for (f, src) in &sources {
            let (t, s) =
                scrub::scrub_markdown(src, &rewrites_for(m, f), &sections_for(m, f), &rules)
                    .map_err(|e| Failure::one(format!("{f}: {e}")))?;
            stats.add(&s);
            texts.insert(f.clone(), t);
        }
        let mut grew = false;
        if let Some(ms) = texts.get(MILESTONES_FILE) {
            let kept: BTreeSet<String> = crate::dod::milestone_ids(ms).into_iter().collect();
            for id in source_ids.difference(&kept) {
                grew |= dropped.insert(id.clone());
            }
        }
        let mut status_text = None;
        if let Some(src) = &status_src {
            let (t, ids) = scrub::map_ron_rows(src, &|id, row| {
                if dropped.contains(id) {
                    None
                } else {
                    scrub::scrub_status_row(row, &rules)
                }
            });
            for id in ids {
                grew |= dropped.insert(id);
            }
            status_text = Some(t);
        }
        if !grew {
            break (texts, stats, status_text);
        }
    };
    // Links, once every file's final text is known.
    let exported: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let exists = |p: &str| {
        exported.contains(p)
            || exported
                .iter()
                .any(|f| f.starts_with(&format!("{}/", p.trim_end_matches('/'))))
    };
    let anchor_sets: BTreeMap<String, BTreeSet<String>> = texts
        .iter()
        .map(|(f, t)| (f.clone(), scrub::anchors(t)))
        .collect();
    let anchors_of = |p: &str| anchor_sets.get(p).cloned();
    let mut links_rewritten = 0;
    let mut out = BTreeMap::new();
    for (f, t) in texts {
        let (fixed, n) = scrub::fix_links(&f, &t, &exists, &anchors_of);
        links_rewritten += n;
        out.insert(f, fixed);
    }
    // A document the scrub empties is premium as a whole: exporting an empty file would still
    // publish its name. It is listed as a private path, or restated for the base edition.
    let emptied: Vec<String> = out
        .iter()
        .filter(|(f, t)| f.ends_with(".md") && t.trim().is_empty())
        .map(|(f, _)| {
            format!(
                "{f}: the scrub leaves nothing of it — list it as a private path in {MANIFEST_FILE}, or restate it for the base edition"
            )
        })
        .collect();
    if !emptied.is_empty() {
        return Err(Failure { messages: emptied });
    }
    if let Some(t) = status_text {
        out.insert(STATUS_FILE.to_string(), t);
    }
    let dod_dropped = dropped
        .into_iter()
        .filter(|d| source_ids.contains(d))
        .collect();
    Ok(Docs {
        texts: out,
        stats,
        links_rewritten,
        dod_dropped,
        chapters_cut: dangling.chapters.into_iter().collect(),
    })
}

/// Every leak in the tree at `dir`: a file whose path or text names a premium identifier
/// (a premium DoD id, gate row or error code included, or a range citation spanning one),
/// a prose file naming a doc term, or a markdown link that does not resolve.
pub fn leak_scan(dir: &Path, m: &Manifest) -> Vec<String> {
    let doc = leak_matcher(m);
    let hard = m.code_matcher();
    let files = walk(dir);
    let set: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let mut texts = BTreeMap::new();
    let mut leaks = Vec::new();
    for f in &files {
        if let Some(h) = hard.all_hits(f).into_iter().next() {
            leaks.push(format!("{f}: its path names `{h}`"));
        }
        let Ok(raw) = std::fs::read(dir.join(f)) else {
            continue;
        };
        let Ok(text) = String::from_utf8(raw) else {
            continue;
        };
        let md = f.ends_with(".md");
        // Prose: markdown anywhere, and every text file under docs/ (the DoD status rows).
        let prose = md || f.starts_with("docs/");
        for (n, line) in text.lines().enumerate() {
            let hit = if prose {
                doc.first_hit(line)
            } else {
                hard.first_hit(line)
            };
            if let Some(h) = hit {
                leaks.push(format!("{f}:{}: names `{h}`", n + 1));
            }
        }
        if md {
            texts.insert(f.clone(), text);
        }
    }
    let anchor_sets: BTreeMap<&String, BTreeSet<String>> =
        texts.iter().map(|(f, t)| (f, scrub::anchors(t))).collect();
    let exists = |p: &str| {
        set.contains(p)
            || set
                .iter()
                .any(|f| f.starts_with(&format!("{}/", p.trim_end_matches('/'))))
    };
    let anchors_of = |p: &str| anchor_sets.get(&p.to_string()).cloned();
    for (f, t) in &texts {
        for (target, why) in scrub::dangling_links(f, t, &exists, &anchors_of) {
            leaks.push(format!("{f}: link ({target}) dangles: {why}"));
        }
    }
    leaks
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if name != ".git" && name != "target" {
                    stack.push(p);
                }
            } else if let Ok(rel) = p.strip_prefix(dir) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

/// Make `out` ready: it may not be inside `root`; it must be empty (bar `.git`) or a previous
/// export; everything but `.git` is removed.
fn prepare_out(root: &Path, out: &Path) -> XResult<()> {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let (root_a, out_a) = (abs(root), abs(out));
    if out_a.starts_with(&root_a) || root_a.starts_with(&out_a) {
        return Err(Failure::one(format!(
            "export-public: the out dir {} overlaps the repository {}",
            out_a.display(),
            root_a.display()
        )));
    }
    if out.exists() {
        let entries: Vec<_> = std::fs::read_dir(out)
            .map_err(|e| Failure::one(format!("cannot read {}: {e}", out.display())))?
            .flatten()
            .filter(|e| e.file_name() != ".git")
            .collect();
        if !entries.is_empty() && !out.join(MARKER).is_file() {
            return Err(Failure::one(format!(
                "export-public: {} is not empty and holds no previous export ({MARKER}); refusing to clear it",
                out.display()
            )));
        }
        for e in entries {
            let p = e.path();
            let r = if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_file(&p)
            };
            r.map_err(|err| Failure::one(format!("cannot clear {}: {err}", p.display())))?;
        }
    }
    std::fs::create_dir_all(out)
        .map_err(|e| Failure::one(format!("cannot create {}: {e}", out.display())))
}

fn write(out: &Path, rel: &str, bytes: &[u8]) -> XResult<u64> {
    let p = out.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Failure::one(format!("cannot create {}: {e}", parent.display())))?;
    }
    std::fs::write(&p, bytes)
        .map_err(|e| Failure::one(format!("cannot write {}: {e}", p.display())))?;
    Ok(bytes.len() as u64)
}

fn cargo(out: &Path, args: &[&str], target: &Path) -> XResult<()> {
    let bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(bin)
        .args(args)
        .current_dir(out)
        .env("CARGO_TARGET_DIR", target)
        .status()
        .map_err(|e| Failure::one(format!("cannot run cargo {args:?}: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Failure::one(format!(
            "export-public: `cargo {}` failed in the exported tree {}",
            args.join(" "),
            out.display()
        )))
    }
}

/// Write the public tree (see the module docs).
pub fn export(root: &Path, m: &Manifest, debt: &Debt, opts: &Options) -> XResult<Report> {
    if !opts.docs_only && !debt.is_empty() {
        return Err(Failure::one(refusal(debt)));
    }
    let all = crate::plan_coverage::repo_files(root);
    let in_docs_only = |f: &str| {
        m.docs_only.iter().any(|d| {
            let d = d.trim_end_matches('/');
            f == d || f.starts_with(&format!("{d}/"))
        })
    };
    let mut files: Vec<String> = all
        .iter()
        .filter(|f| !m.excluded(f))
        .filter(|f| !opts.docs_only || in_docs_only(f))
        .cloned()
        .collect();
    for p in &m.public_files {
        if !opts.docs_only || in_docs_only(&p.to) {
            files.push(p.to.clone());
        }
    }
    files.sort();
    files.dedup();
    // Scrub against the tree as exported: public files are read from their private source.
    let docs = {
        let staged: Vec<String> = files
            .iter()
            .filter(|f| !m.public_files.iter().any(|p| &p.to == *f))
            .cloned()
            .collect();
        let mut d = scrub_docs(root, m, &staged, &all)?;
        // Links in the other files may point at the public README/CONTRIBUTING.
        for p in &m.public_files {
            if files.contains(&p.to) {
                d.texts.insert(p.to.clone(), read(&root.join(&p.from))?);
            }
        }
        d
    };
    prepare_out(root, &opts.out)?;
    let mut report = Report {
        stats: docs.stats.clone(),
        links_rewritten: docs.links_rewritten,
        dod_dropped: docs.dod_dropped.clone(),
        chapters_cut: docs.chapters_cut.clone(),
        ..Report::default()
    };
    for f in &files {
        let bytes: Vec<u8> = if let Some(t) = docs.texts.get(f) {
            t.clone().into_bytes()
        } else if f == GATES_FILE || f == "Cargo.toml" || f == boundary::NEXTEST_FILE {
            let raw = std::fs::read(root.join(f))
                .map_err(|e| Failure::one(format!("cannot read {f}: {e}")))?;
            boundary::export_text(m, f, &raw)
                .unwrap_or_default()
                .into_bytes()
        } else {
            std::fs::read(root.join(f))
                .map_err(|e| Failure::one(format!("cannot read {f}: {e}")))?
        };
        let n = write(&opts.out, f, &bytes)?;
        report.files.push((f.clone(), n));
    }
    let marker = format!(
        "The public edition of Forge Engine, exported one-way from the source repository by\n`cargo xtask export-public{}`. Do not edit it here: changes are made in the source and\nexported again.\n",
        if opts.docs_only { " --docs-only" } else { "" }
    );
    write(&opts.out, MARKER, marker.as_bytes())?;
    // The exported tree builds in its own target dir: beside the out dir, or where
    // FORGE_EXPORT_TARGET_DIR says (a lane keeps it with its other targets).
    let target = std::env::var_os("FORGE_EXPORT_TARGET_DIR")
        .map_or_else(|| opts.out.with_extension("target"), PathBuf::from);
    if !opts.docs_only {
        cargo(&opts.out, &["generate-lockfile", "--offline"], &target)?;
        let lock = std::fs::metadata(opts.out.join("Cargo.lock")).map_or(0, |md| md.len());
        report.files.push(("Cargo.lock".into(), lock));
        // NOTICES follows the exported dependency graph (Ch.38.5), not the private one's (a
        // tree without one, a test fixture, has nothing to regenerate).
        if opts.build && opts.out.join("NOTICES").is_file() {
            crate::licence_audit::run(&opts.out, crate::licence_audit::Mode::Write)?;
            if let Some(f) = report.files.iter_mut().find(|(f, _)| f == "NOTICES") {
                f.1 = std::fs::metadata(opts.out.join("NOTICES")).map_or(0, |md| md.len());
            }
        }
    }
    let leaks = leak_scan(&opts.out, m);
    if !leaks.is_empty() {
        return Err(Failure {
            messages: std::iter::once(format!(
                "export-public: the leak scan found {} premium names or dangling links in {}",
                leaks.len(),
                opts.out.display()
            ))
            .chain(leaks)
            .collect(),
        });
    }
    if !opts.docs_only && opts.build {
        cargo(
            &opts.out,
            &[
                "build",
                "--workspace",
                "--all-targets",
                "--offline",
                "--locked",
            ],
            &target,
        )?;
        report.built = true;
        if !opts.no_test {
            let nextest = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
                .args(["nextest", "--version"])
                .output()
                .is_ok_and(|o| o.status.success());
            if nextest {
                cargo(
                    &opts.out,
                    &[
                        "nextest",
                        "run",
                        "--workspace",
                        "--offline",
                        "--locked",
                        "--no-fail-fast",
                    ],
                    &target,
                )?;
            } else {
                cargo(
                    &opts.out,
                    &["test", "--workspace", "--offline", "--locked"],
                    &target,
                )?;
            }
            report.tested = true;
        }
    }
    report.files.sort();
    Ok(report)
}

/// `cargo xtask export-public <out-dir> [--docs-only | --no-test]`.
pub fn run(root: &Path, args: &[String]) -> XResult<String> {
    let docs_only = args.iter().any(|a| a == "--docs-only");
    let no_test = args.iter().any(|a| a == "--no-test");
    let outs: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if let Some(bad) = args
        .iter()
        .find(|a| a.starts_with("--") && *a != "--docs-only" && *a != "--no-test")
    {
        return Err(Failure::one(format!("export-public: unknown flag `{bad}`")));
    }
    let [out] = outs.as_slice() else {
        return Err(Failure::one(
            "usage: cargo xtask export-public <out-dir> [--docs-only | --no-test]",
        ));
    };
    let Some(m) = Manifest::load(root)? else {
        return Err(Failure::one(format!(
            "export-public: no {MANIFEST_FILE}: this is already the public edition"
        )));
    };
    let debt = boundary::load_debt(root)?.unwrap_or_default();
    let report = export(
        root,
        &m,
        &debt,
        &Options {
            out: PathBuf::from(out),
            docs_only,
            build: true,
            no_test,
        },
    )?;
    Ok(format!(
        "export-public: wrote {} files, {} bytes, to {}{}\n  cut: {} marked sections, {} headings with their sections, {} sentences/rows/lines; {} links rewritten\n  master-plan chapters cut whole: {:?}\n  DoD rows not exported ({}): {}\n  leak scan: clean (no premium identifier, no doc term, no dangling link){}\n",
        report.files.len(),
        report.bytes(),
        out,
        if docs_only { " (docs only)" } else { "" },
        report.stats.sections,
        report.stats.headings,
        report.stats.units,
        report.links_rewritten,
        report.chapters_cut,
        report.dod_dropped.len(),
        report.dod_dropped.join(" "),
        match (report.built, report.tested) {
            (true, true) =>
                "\n  the exported tree builds and its tests pass (offline, in the out dir)",
            (true, false) =>
                "\n  the exported tree builds (offline, every target); its tests were left to the caller (--no-test)",
            _ => "",
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-xtask-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn put(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap_or(dir)).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(p, text).unwrap_or_else(|e| panic!("{e}"));
    }

    fn manifest() -> Manifest {
        Manifest::parse(
            r#"(
                crates: [(name: "forge-secret", path: "crates/forge-secret", wp: "WP-44", what: "premium")],
                identifiers: [(text: "HiddenTree", wp: "WP-43")],
                doc_terms: ["hush"],
                public_files: [(from: "README.public.md", to: "README.md")],
                docs_only: ["docs", "README.md"],
            )"#,
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    /// A tiny private repository: a base crate, a premium crate, docs with premium parts.
    fn fixture(name: &str) -> PathBuf {
        let root = temp(name);
        put(
            &root,
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n\n# why the secret is fast\n[profile.dev.package.forge-secret]\nopt-level = 2\n",
        );
        put(
            &root,
            "crates/forge-base/Cargo.toml",
            "[package]\nname = \"forge-base\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n",
        );
        put(
            &root,
            "crates/forge-base/src/lib.rs",
            "//! The base.\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn base_works() {\n        assert_eq!(1 + 1, 2);\n    }\n}\n",
        );
        put(
            &root,
            "crates/forge-secret/Cargo.toml",
            "[package]\nname = \"forge-secret\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        );
        put(
            &root,
            "crates/forge-secret/src/lib.rs",
            "pub struct HiddenTree;\n",
        );
        put(
            &root,
            "README.md",
            "# Private\n\nThe forge-secret edition.\n",
        );
        put(
            &root,
            "README.public.md",
            "# Public\n\nSee [the plan](docs/plan.md).\n",
        );
        put(
            &root,
            "docs/plan.md",
            "# Plan\n\n## Base\n\nBase text. [Base](../crates/forge-base/src/lib.rs) link.\n\nA hushed aside.\n\n## The HiddenTree\n\nGone.\n",
        );
        put(&root, "premium.ron", "()\n");
        root
    }

    #[test]
    fn positive_control_a_planted_identifier_is_caught_by_the_leak_scan() {
        let m = manifest();
        let dir = temp("leak");
        put(&dir, "docs/a.md", "# A\n\nClean prose.\n");
        put(&dir, "src/lib.rs", "pub fn f() {}\n");
        assert!(leak_scan(&dir, &m).is_empty(), "{:?}", leak_scan(&dir, &m));
        for (rel, text, want) in [
            (
                "src/lib.rs",
                "use forge_secret::X;\n",
                "names `forge_secret`",
            ),
            ("src/lib.rs", "// a HiddenTree\n", "names `HiddenTree`"),
            ("docs/a.md", "# A\n\nThe hushed truth.\n", "names `hush`"),
            ("docs/a.md", "# A\n\n[x](gone.md)\n", "dangles"),
            ("docs/a.md", "# A\n\nThe SeedHush type.\n", "names `hush`"),
            (
                "docs/plan/status.ron",
                "(evidence: [\"has_no_hush.rs\"])\n",
                "names `hush`",
            ),
            ("crates/forge-secret/x.txt", "clean\n", "path names"),
        ] {
            let d2 = temp("leak2");
            put(&d2, "docs/a.md", "# A\n\nClean prose.\n");
            put(&d2, "src/lib.rs", "pub fn f() {}\n");
            put(&d2, rel, text);
            let leaks = leak_scan(&d2, &m);
            assert!(
                leaks.iter().any(|l| l.contains(want)),
                "{rel} {text:?}: {leaks:?}"
            );
            let _ = std::fs::remove_dir_all(&d2);
        }
        // Doc terms are prose-only: code may use the word.
        put(&dir, "src/lib.rs", "// hush\n");
        assert!(leak_scan(&dir, &m).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn code_export_refuses_while_debt_remains_and_names_it() {
        let root = fixture("refuse");
        let debt = Debt::parse(
            r#"(deps: [(krate: "forge-base", on: "forge-secret", wp: "WP-42")], names: [(path: "crates/forge-base/src/lib.rs", wp: "WP-43", identifiers: ["HiddenTree"])])"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let out = temp("refuse-out");
        let opts = Options {
            out: out.clone(),
            docs_only: false,
            build: false,
            no_test: false,
        };
        let err = export(&root, &manifest(), &debt, &opts)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("refusing to export code"), "{err}");
        assert!(err.contains("forge-base -> forge-secret") && err.contains("[HiddenTree]"));
        assert!(!out.exists(), "a refused export writes nothing");
        // --docs-only works with the same debt.
        let docs = Options {
            docs_only: true,
            ..opts
        };
        let r = export(&root, &manifest(), &debt, &docs).unwrap_or_else(|e| panic!("{e}"));
        let names: Vec<&str> = r.files.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(names, vec!["README.md", "docs/plan.md"]);
        let plan = std::fs::read_to_string(out.join("docs/plan.md")).unwrap_or_default();
        assert!(
            plan.contains("Base text. Base link.") && !plan.contains("aside"),
            "{plan}"
        );
        assert!(!plan.contains("hush") && !plan.contains("Hidden") && !plan.contains("Gone"));
        let readme = std::fs::read_to_string(out.join("README.md")).unwrap_or_default();
        assert!(readme.starts_with("# Public"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn the_out_dir_must_be_empty_or_a_previous_export() {
        let root = fixture("outdir");
        let out = temp("outdir-out");
        put(&out, "precious.txt", "keep me");
        let opts = Options {
            out: out.clone(),
            docs_only: true,
            build: false,
            no_test: false,
        };
        let err = export(&root, &manifest(), &Debt::default(), &opts)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("refusing to clear it"), "{err}");
        assert!(out.join("precious.txt").is_file());
        std::fs::remove_file(out.join("precious.txt")).unwrap_or_else(|e| panic!("{e}"));
        put(&out, ".git/HEAD", "ref");
        export(&root, &manifest(), &Debt::default(), &opts).unwrap_or_else(|e| panic!("{e}"));
        // A second export over the first clears it and keeps .git.
        put(&out, "stale.md", "old");
        export(&root, &manifest(), &Debt::default(), &opts).unwrap_or_else(|e| panic!("{e}"));
        assert!(!out.join("stale.md").exists() && out.join(".git/HEAD").is_file());
        let inside = Options {
            out: root.join("public"),
            ..opts
        };
        assert!(export(&root, &manifest(), &Debt::default(), &inside).is_err());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_debt_free_code_export_builds_and_tests_offline() {
        let root = fixture("code");
        let out = temp("code-out");
        let r = export(
            &root,
            &manifest(),
            &Debt::default(),
            &Options {
                out: out.clone(),
                docs_only: false,
                build: true,
                no_test: false,
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(r.built);
        assert!(out.join("Cargo.lock").is_file() && !out.join("crates/forge-secret").exists());
        assert!(!out.join("premium.ron").exists() && !out.join("README.public.md").exists());
        let ws = std::fs::read_to_string(out.join("Cargo.toml")).unwrap_or_default();
        assert!(!ws.contains("secret"), "{ws}");
        let lock = std::fs::read_to_string(out.join("Cargo.lock")).unwrap_or_default();
        assert!(lock.contains("forge-base") && !lock.contains("forge-secret"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_dir_all(out.with_extension("target"));
    }

    #[test]
    fn the_docs_only_export_of_this_repository_is_leak_free() {
        let root = crate::util::workspace_root();
        let Some(m) = Manifest::load(&root).unwrap_or_else(|e| panic!("{e}")) else {
            return; // the public edition: nothing to export
        };
        let out = temp("self-docs");
        let debt = boundary::load_debt(&root)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_default();
        let r = export(
            &root,
            &m,
            &debt,
            &Options {
                out: out.clone(),
                docs_only: true,
                build: false,
                no_test: false,
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(r.files.iter().any(|(f, _)| f == PLAN_FILE));
        // Positive control on the real export: plant one identifier and the scan sees it.
        let first = m.hard_identifiers().into_iter().next().unwrap_or_default();
        let plan = out.join(PLAN_FILE);
        let mut text = std::fs::read_to_string(&plan).unwrap_or_default();
        text.push_str(&format!("\nPlanted: {first}.\n"));
        std::fs::write(&plan, text).unwrap_or_else(|e| panic!("{e}"));
        let leaks = leak_scan(&out, &m);
        assert!(
            leaks.len() == 1 && leaks[0].contains(&first),
            "the planted identifier is the only leak: {leaks:?}"
        );
        let _ = std::fs::remove_dir_all(&out);
    }
}

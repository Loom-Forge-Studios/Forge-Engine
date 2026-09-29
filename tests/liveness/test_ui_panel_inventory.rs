//! `test_ui_panel_inventory`: gate row `C-ui-panel-inventory`, DoD M2-18, Ch.21 §21.21 and
//! §21.23.
//!
//! Ch.21 lists every editor panel the plan implies. This guard proves that list is
//! complete and that every reference in it resolves. If it does not, a panel can be
//! planned, forgotten and never built, and nobody finds out until a user looks for it.
//! This is the liveness question (Ch.30 §30.2) asked of the plan instead of the code:
//! *does this panel have an owner, a chapter and an acceptance test?*
//!
//! It checks:
//!
//! 1. Every row of the §21.21 inventory:
//!    * names at least one chapter, and each chapter exists in the master plan's
//!      document map;
//!    * names a work package defined in the §21.24 table, and that WP's DoD column
//!      claims the panel's DoD id (so the two tables cannot drift), or names
//!      `UNSCHEDULED`, and then the §21.24 gap table claims the DoD id and no WP does;
//!    * for a scheduled panel, quotes a non-empty backlog clause that **names the panel**:
//!      of every panel in the inventory, this one's name matches the clause best (most
//!      shared name words, ties broken by the earliest match; see [`names_panel`]). A
//!      clause that is in the WP's scope but describes another feature does not make the
//!      WP's implementer build the panel, so it does not count;
//!    * names a UI DoD id (`M2-18` or later, or a parity row M7/M9/M10 for a parity panel)
//!      that exists in `milestones.md` and has a row
//!      in `dod-status.ron`.
//! 2. Panel ids and DoD ids are unique within the inventory.
//! 3. Every panel the WP-U0 scope names is present ([`REQUIRED_PANELS`]).
//! 4. Every UI DoD id in `milestones.md` is cited somewhere in Ch.21. An id nobody cites
//!    is an orphan acceptance test.
//! 5. Every D-5 budget row in §21.22 names a `C-ui-*` gate row that exists in
//!    `tests/gates.ron`, and a DoD id that exists.
//!
//! 6. When the orchestrator backlog is available (`FORGE_BACKLOG` names `backlog.json`),
//!    every WP in the §21.24 table exists in it, and every scheduled panel's backlog clause
//!    appears verbatim in its WP's backlog title or scope. With item 1's naming rule, this
//!    is what stops Ch.21 from giving a panel to a WP whose implementer, working from the
//!    backlog, would skip it. `backlog.json` is outside the repository and `just verify`
//!    and CI do not set `FORGE_BACKLOG`, so this half has its own gate row,
//!    `C-ui-panel-backlog`, which is `Awaiting` in `tests/gates.ron` and shows as AWAITING
//!    in `just gate`. It is never reported as passed (W9). `just backlog-check <path>`
//!    runs it. The positive controls always run it, against a backlog built from the
//!    committed plan and then mutated.
//!
//! Every check is a pure function of the documents' text. The positive controls feed
//! it mutated copies of the committed documents and assert that it fails for the named
//! reason (W2). A parser that finds no table fails; it never passes vacuously.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

const PLAN: &str = "docs/plan/master-plan.md";
const MILESTONES: &str = "docs/plan/milestones.md";
const DOD_STATUS: &str = "docs/plan/dod-status.ron";
const GATES: &str = "tests/gates.ron";
/// Environment variable naming the orchestrator's `backlog.json` (it lives outside the
/// repository, so the path is never hard-coded).
const BACKLOG_ENV: &str = "FORGE_BACKLOG";
/// The WP cell of a panel no backlog work package names yet (§21.24 gap table).
const UNSCHEDULED: &str = "UNSCHEDULED";

/// The first DoD number of the UI table appended to M2 by WP-U0.
const FIRST_UI_DOD: u32 = 18;

/// The panels the WP-U0 scope names ("the complete editor panel inventory drawn from every
/// chapter"), as their `EditorPanel` extension-point ids. Some scope items map to more
/// than one panel ("preset/new-project", "team/collab",
/// "sequencer/animation").
const REQUIRED_PANELS: &[(&str, &str)] = &[
    ("hierarchy", "forge.hierarchy"),
    ("inspector", "forge.inspector"),
    ("viewport", "forge.viewport"),
    ("asset browser", "forge.assets"),
    ("console", "forge.console"),
    ("profiler", "forge.profiler"),
    ("graph editor", "forge.graph"),
    ("new project", "forge.launcher"),
    ("preset", "forge.presets"),
    ("plugin manager", "forge.plugins"),
    ("audit", "forge.audit_log"),
    ("team", "forge.team"),
    ("collab", "forge.sandbox"),
    ("conflicts", "forge.conflicts"),
    ("publish queue", "forge.publish_queue"),
    ("licence", "forge.licence"),
    ("remote connect", "forge.remote"),
    ("sequencer", "forge.sequencer"),
    ("animation", "forge.anim_graph"),
    ("localisation", "forge.localisation"),
    ("settings", "forge.settings"),
    ("keybindings", "forge.keybindings"),
    ("command palette", "forge.command_palette"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Panel {
    name: String,
    id: String,
    chapters: Vec<String>,
    wp: String,
    dod: String,
    backlog_clause: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Budget {
    test: String,
    gate_row: String,
    dod: String,
}

#[derive(Debug, Default)]
struct Summary {
    panels: usize,
    wps: usize,
    budgets: usize,
    ui_dod_ids: usize,
    /// Whether the backlog cross-check (item 6) ran.
    backlog_checked: bool,
}

// ---- the documents -------------------------------------------------------------------

struct Docs {
    plan: String,
    milestones: String,
    dod_status: String,
    gates: String,
    /// `backlog.json`, when available.
    backlog: Option<String>,
}

impl Docs {
    fn committed() -> Self {
        let root = forge_tests::workspace_root();
        let read = |p: &str| {
            std::fs::read_to_string(root.join(p))
                .unwrap_or_else(|e| panic!("cannot read {p}: {e}"))
                .replace("\r\n", "\n")
        };
        Self {
            plan: read(PLAN),
            milestones: read(MILESTONES),
            dod_status: read(DOD_STATUS),
            gates: read(GATES),
            backlog: std::env::var_os(BACKLOG_ENV).map(|p| {
                std::fs::read_to_string(&p)
                    .unwrap_or_else(|e| panic!("{BACKLOG_ENV}={}: {e}", p.to_string_lossy()))
            }),
        }
    }
}

/// Work package id → its backlog title and scope, joined.
fn backlog_scopes(json: &str) -> Result<BTreeMap<String, String>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("backlog: parse error: {e}"))?;
    let items: Vec<&serde_json::Value> = match &v {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(o) => o.values().collect(),
        _ => return Err("backlog: neither an array nor an object of work packages".into()),
    };
    let mut out = BTreeMap::new();
    for it in items {
        let field = |k: &str| it.get(k).and_then(|x| x.as_str()).unwrap_or_default();
        if !field("id").is_empty() {
            out.insert(
                field("id").to_string(),
                format!("{} {}", field("title"), field("scope")),
            );
        }
    }
    if out.is_empty() {
        return Err("backlog: no work packages with an id".into());
    }
    Ok(out)
}

/// Words too generic to identify a panel: they name a kind of UI, not a feature
/// ("Graph editor" is identified by "graph", "Licence status" by "licence").
const GENERIC_WORDS: &[&str] = &[
    "panel", "editor", "window", "view", "dialog", "status", "tool", "store", "with", "from",
    "that", "into", "over", "your",
];

/// The identifying words of `text`, in order: split on anything that is not a letter or
/// digit, lower-cased, a plural `s` dropped from words longer than four letters, words
/// shorter than four letters and [`GENERIC_WORDS`] removed.
fn name_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|w| {
            let w = w.to_lowercase();
            match w.strip_suffix('s') {
                Some(stem) if w.chars().count() > 4 => stem.to_string(),
                _ => w,
            }
        })
        .filter(|w| w.chars().count() >= 4 && !GENERIC_WORDS.contains(&w.as_str()))
        .collect()
}

/// How well `clause` names the panel called `name`: the number of the name's identifying
/// words the clause contains, and the position of the first one (earlier is better).
/// `None` if it contains none.
fn name_match(name: &str, clause: &[String]) -> Option<(usize, usize)> {
    let terms: BTreeSet<String> = name_words(name).into_iter().collect();
    let hits: Vec<usize> = terms
        .iter()
        .filter_map(|t| clause.iter().position(|w| w == t))
        .collect();
    Some((hits.len(), *hits.iter().min()?))
}

/// `Ok` if `clause` names the panel `name` better than, or as well as, it names any
/// other panel in `all_names`; otherwise the reason. Better = more shared identifying
/// words; equal counts are broken by the earlier first match.
fn names_panel(name: &str, clause: &str, all_names: &[&str]) -> Result<(), String> {
    let words = name_words(clause);
    let Some(own) = name_match(name, &words) else {
        return Err("shares no identifying word with the panel's name".to_string());
    };
    let better = |(c, p): (usize, usize)| c > own.0 || (c == own.0 && p < own.1);
    match all_names
        .iter()
        .filter(|n| **n != name)
        .find(|n| name_match(n, &words).is_some_and(better))
    {
        Some(other) => Err(format!("names panel {other:?} better than this one")),
        None => Ok(()),
    }
}

/// A backlog clause cell: `"words"` → `words`.
fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim().to_string()
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Deserialize)]
struct DodStatus {
    items: Vec<IdOnly>,
}

#[derive(Deserialize)]
struct Gates {
    rows: Vec<IdOnly>,
}

// ---- parsing -------------------------------------------------------------------------

/// Lines of a section that starts with a line beginning `start` and ends before the next
/// top-level (`# `) heading outside a code fence.
fn section<'a>(doc: &'a str, start: &str) -> Option<Vec<&'a str>> {
    let mut lines = doc.lines();
    lines.by_ref().find(|l| l.starts_with(start))?;
    let mut out = Vec::new();
    let mut fenced = false;
    for l in lines {
        if l.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if !fenced && l.starts_with("# ") {
            break;
        }
        out.push(l);
    }
    Some(out)
}

fn cells(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    let inner = t.strip_prefix('|')?.strip_suffix('|')?;
    Some(inner.split('|').map(|c| c.trim().to_string()).collect())
}

/// The body rows of the first table in `lines` whose header row starts with `header`
/// (leading cells, compared exactly after trimming).
fn table(lines: &[&str], header: &[&str]) -> Option<Vec<Vec<String>>> {
    let pos = lines.iter().position(|l| {
        cells(l)
            .is_some_and(|c| c.len() >= header.len() && c.iter().zip(header).all(|(a, b)| a == b))
    })?;
    let mut rows = Vec::new();
    // Skip the header and the `|---|` separator.
    for l in lines.iter().skip(pos + 2) {
        match cells(l) {
            Some(c) => rows.push(c),
            None => break,
        }
    }
    Some(rows)
}

fn strip_ticks(s: &str) -> String {
    s.trim().trim_matches('`').trim().to_string()
}

/// `M2-19..M2-23`, `M2-24, M2-25`, `M2-18` → every id, ranges expanded.
fn dod_ids_in(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some((m, n, len)) = dod_at(text, i) {
            let mut end = i + len;
            let mut hi = n;
            if text[end..].starts_with("..")
                && let Some((m2, n2, len2)) = dod_at(text, end + 2)
                && m2 == m
                && n2 >= n
            {
                hi = n2;
                end += 2 + len2;
            }
            for k in n..=hi {
                out.insert(format!("M{m}-{k}"));
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// A DoD id `M<digits>-<digits>` starting at byte `i` (and not preceded by an
/// alphanumeric), as (milestone, number, byte length).
fn dod_at(text: &str, i: usize) -> Option<(u32, u32, usize)> {
    let b = text.as_bytes();
    if b.get(i) != Some(&b'M') || (i > 0 && b[i - 1].is_ascii_alphanumeric()) {
        return None;
    }
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let m_len = digits(i + 1);
    if m_len == 0 || b.get(i + 1 + m_len) != Some(&b'-') {
        return None;
    }
    let n_start = i + 2 + m_len;
    let n_len = digits(n_start);
    if n_len == 0 {
        return None;
    }
    let m = text[i + 1..i + 1 + m_len].parse().ok()?;
    let n = text[n_start..n_start + n_len].parse().ok()?;
    Some((m, n, 2 + m_len + n_len))
}

fn is_ui_dod(id: &str) -> bool {
    id.strip_prefix("M2-")
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| n >= FIRST_UI_DOD)
}

/// A parity row (E-67: M7, M9, M10) — the DoD of a panel a parity work package adds (the
/// input debugger, WP-65). Such rows are not UI DoD ids (rule 4 does not ask Ch.21 to cite
/// them), but a panel may name one.
fn is_parity_dod(id: &str) -> bool {
    ["M7-", "M9-", "M10-"]
        .iter()
        .any(|p| id.strip_prefix(p).is_some_and(|n| n.parse::<u32>().is_ok()))
}

/// Chapter numbers in the document map (first cell a number, possibly bold).
fn document_map_chapters(plan: &str) -> BTreeSet<String> {
    let Some(lines) = section_h2(plan, "## Document map") else {
        return BTreeSet::new();
    };
    table(&lines, &["Ch", "Title"])
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            let c = r.first()?.trim_matches('*').trim().to_string();
            c.bytes().all(|b| b.is_ascii_digit()).then_some(c)
        })
        .collect()
}

/// A `## ` section: from its heading to the next `## ` or `# ` heading.
fn section_h2<'a>(doc: &'a str, start: &str) -> Option<Vec<&'a str>> {
    let mut lines = doc.lines();
    lines.by_ref().find(|l| l.starts_with(start))?;
    Some(
        lines
            .take_while(|l| !(l.starts_with("## ") || l.starts_with("# ")))
            .collect(),
    )
}

/// DoD ids defined in milestones.md (table rows whose first cell is an id).
fn milestone_ids(md: &str) -> BTreeSet<String> {
    md.lines()
        .filter_map(|l| cells(l)?.into_iter().next())
        .filter(|c| dod_at(c, 0).is_some_and(|(_, _, len)| len == c.len()))
        .collect()
}

// ---- the check -----------------------------------------------------------------------

fn check(d: &Docs) -> Result<Summary, Vec<String>> {
    let mut errs = Vec::new();

    let Some(ch21) = section(&d.plan, "# Chapter 21 ") else {
        return Err(vec![format!("{PLAN}: no \"# Chapter 21 \" heading")]);
    };
    let ch21_text = ch21.join("\n");

    let chapters = document_map_chapters(&d.plan);
    if chapters.is_empty() {
        errs.push(format!("{PLAN}: document map has no chapter rows"));
    }
    let milestone_ids = milestone_ids(&d.milestones);
    let status_ids: BTreeSet<String> = match ron::from_str::<DodStatus>(&d.dod_status) {
        Ok(f) => f.items.into_iter().map(|i| i.id).collect(),
        Err(e) => {
            errs.push(format!("{DOD_STATUS}: parse error: {e}"));
            BTreeSet::new()
        }
    };
    let gate_ids: BTreeSet<String> = match ron::from_str::<Gates>(&d.gates) {
        Ok(f) => f.rows.into_iter().map(|r| r.id).collect(),
        Err(e) => {
            errs.push(format!("{GATES}: parse error: {e}"));
            BTreeSet::new()
        }
    };

    // §21.24 — work packages and the DoD ids each claims.
    let wps: BTreeMap<String, BTreeSet<String>> = table(&ch21, &["WP", "Scope", "DoD"])
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.len() >= 3)
        .map(|r| (r[0].clone(), dod_ids_in(&r[2])))
        .collect();
    if wps.is_empty() {
        errs.push("Ch.21 §21.24: no work-package table (| WP | Scope | DoD |)".to_string());
    }

    // §21.21 — the inventory.
    let panels: Vec<Panel> = table(&ch21, &["Panel", "Id", "Chapters", "WP", "DoD"])
        .unwrap_or_default()
        .into_iter()
        .map(|r| Panel {
            name: r[0].clone(),
            id: strip_ticks(&r[1]),
            chapters: r[2]
                .split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect(),
            wp: r[3].clone(),
            dod: r[4].clone(),
            backlog_clause: r.get(7).map(|c| unquote(c)).unwrap_or_default(),
        })
        .collect();

    // §21.24 — the gap table: DoD ids of panels no backlog WP names yet.
    let gap_dod: BTreeSet<String> = table(&ch21, &["Gap", "Panels", "DoD"])
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.len() >= 3)
        .flat_map(|r| dod_ids_in(&r[2]))
        .collect();
    let claimed_by_wps: BTreeSet<&String> = wps.values().flatten().collect();
    for id in &gap_dod {
        if claimed_by_wps.contains(id) {
            errs.push(format!(
                "{id} is in the §21.24 gap table and also claimed by a work package"
            ));
        }
    }
    if panels.is_empty() {
        errs.push(
            "Ch.21 §21.21: no panel inventory table (| Panel | Id | Chapters | WP | DoD |)"
                .to_string(),
        );
    }

    let all_names: Vec<&str> = panels.iter().map(|p| p.name.as_str()).collect();
    let mut seen_ids = BTreeSet::new();
    let mut seen_dod = BTreeSet::new();
    for p in &panels {
        let who = format!("panel {:?} ({})", p.name, p.id);
        if !seen_ids.insert(p.id.clone()) {
            errs.push(format!("{who}: duplicate panel id"));
        }
        if !seen_dod.insert(p.dod.clone()) {
            errs.push(format!("{who}: DoD {} is claimed by another panel", p.dod));
        }
        if p.chapters.is_empty() {
            errs.push(format!("{who}: names no chapter"));
        }
        for c in &p.chapters {
            match c.strip_prefix("Ch.") {
                Some(n) if chapters.contains(n) => {}
                _ => errs.push(format!(
                    "{who}: chapter {c} is not a chapter in the document map"
                )),
            }
        }
        if p.wp == UNSCHEDULED {
            if !gap_dod.contains(&p.dod) {
                errs.push(format!(
                    "{who}: is UNSCHEDULED but {} is not in the §21.24 gap table",
                    p.dod
                ));
            }
        } else if p.backlog_clause.is_empty() || p.backlog_clause.starts_with('—') {
            errs.push(format!(
                "{who}: scheduled on {} but quotes no backlog clause",
                p.wp
            ));
        } else if let Err(why) = names_panel(&p.name, &p.backlog_clause, &all_names) {
            errs.push(format!(
                "{who}: backlog clause {:?} does not name the panel ({why}), so {}'s \
                 implementer would not build it",
                p.backlog_clause, p.wp
            ));
        }
        if gap_dod.contains(&p.dod) && p.wp != UNSCHEDULED {
            errs.push(format!(
                "{who}: {} is in the §21.24 gap table but the panel names {}",
                p.dod, p.wp
            ));
        }
        match wps.get(&p.wp) {
            None if p.wp == UNSCHEDULED => {}
            None => errs.push(format!(
                "{who}: work package {} is not defined in the §21.24 table",
                p.wp
            )),
            Some(claimed) if !claimed.contains(&p.dod) => errs.push(format!(
                "{who}: {} does not claim {} in the §21.24 table",
                p.wp, p.dod
            )),
            Some(_) => {}
        }
        if !is_ui_dod(&p.dod) && !is_parity_dod(&p.dod) {
            errs.push(format!(
                "{who}: DoD {} is not a UI DoD id (M2-{FIRST_UI_DOD} or later) or a parity row",
                p.dod
            ));
        }
        if !milestone_ids.contains(&p.dod) {
            errs.push(format!("{who}: DoD {} is not an id in {MILESTONES}", p.dod));
        }
        if !status_ids.contains(&p.dod) {
            errs.push(format!("{who}: DoD {} has no row in {DOD_STATUS}", p.dod));
        }
    }

    for (scope, id) in REQUIRED_PANELS {
        if !seen_ids.contains(*id) {
            errs.push(format!(
                "required panel {id} (WP-U0 scope: \"{scope}\") is missing from the §21.21 inventory"
            ));
        }
    }

    // Every UI DoD id in milestones.md is cited by Ch.21.
    let cited = dod_ids_in(&ch21_text);
    let ui_ids: Vec<&String> = milestone_ids.iter().filter(|i| is_ui_dod(i)).collect();
    if ui_ids.is_empty() {
        errs.push(format!(
            "{MILESTONES}: no UI DoD ids (M2-{FIRST_UI_DOD} or later)"
        ));
    }
    for id in &ui_ids {
        if !cited.contains(*id) {
            errs.push(format!(
                "UI DoD {id} in {MILESTONES} is cited nowhere in Ch.21 (an orphan acceptance test)"
            ));
        }
    }

    // §21.22 — D-5 budgets.
    let budgets: Vec<Budget> = table(&ch21, &["Test", "Budget"])
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.len() >= 5)
        .map(|r| Budget {
            test: r[0]
                .split_whitespace()
                .next()
                .map(strip_ticks)
                .unwrap_or_default(),
            gate_row: strip_ticks(&r[3]),
            dod: r[4].clone(),
        })
        .collect();
    if budgets.is_empty() {
        errs.push("Ch.21 §21.22: no budget table (| Test | Budget | ...)".to_string());
    }
    for b in &budgets {
        if !b.gate_row.starts_with("C-ui-") {
            errs.push(format!(
                "budget {}: gate row {:?} is not a C-ui-* row",
                b.test, b.gate_row
            ));
        } else if !gate_ids.contains(&b.gate_row) {
            errs.push(format!(
                "budget {}: gate row {} does not exist in {GATES}",
                b.test, b.gate_row
            ));
        }
        if !milestone_ids.contains(&b.dod) {
            errs.push(format!(
                "budget {}: DoD {} is not an id in {MILESTONES}",
                b.test, b.dod
            ));
        }
    }

    // The orchestrator backlog (item 6).
    let backlog_checked = match d.backlog.as_deref().map(backlog_scopes) {
        None => false,
        Some(Err(e)) => {
            errs.push(e);
            true
        }
        Some(Ok(scopes)) => {
            for wp in wps.keys() {
                if !scopes.contains_key(wp) {
                    errs.push(format!(
                        "work package {wp} is in the §21.24 table but not in the backlog"
                    ));
                }
            }
            for p in panels.iter().filter(|p| p.wp != UNSCHEDULED) {
                if let Some(scope) = scopes.get(&p.wp)
                    && !p.backlog_clause.is_empty()
                    && !scope.contains(&p.backlog_clause)
                {
                    errs.push(format!(
                        "panel {:?} ({}): backlog clause {:?} is not in {}'s backlog scope",
                        p.name, p.id, p.backlog_clause, p.wp
                    ));
                }
            }
            true
        }
    };

    if errs.is_empty() {
        Ok(Summary {
            panels: panels.len(),
            wps: wps.len(),
            budgets: budgets.len(),
            ui_dod_ids: ui_ids.len(),
            backlog_checked,
        })
    } else {
        Err(errs)
    }
}

// ---- the guard -----------------------------------------------------------------------

#[test]
fn committed_plan_panel_inventory_is_complete_and_resolves() {
    let s = check(&Docs::committed()).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    // Non-vacuous: the tables were actually found and are the size the plan says.
    assert!(
        s.panels >= REQUIRED_PANELS.len(),
        "only {} panels parsed",
        s.panels
    );
    assert!(s.wps >= 13, "only {} work packages parsed", s.wps);
    assert!(s.budgets >= 11, "only {} D-5 budgets parsed", s.budgets);
    assert!(s.ui_dod_ids >= 40, "only {} UI DoD ids", s.ui_dod_ids);
    if s.backlog_checked {
        println!("backlog cross-check: ran against {BACKLOG_ENV}");
    } else {
        // Not a pass: gate row C-ui-panel-backlog is Awaiting in tests/gates.ron and
        // `just gate` shows it. This line is for a run with --nocapture.
        println!(
            "backlog cross-check: AWAITING({BACKLOG_ENV} unset; gate row C-ui-panel-backlog; run `just backlog-check <path>`)"
        );
    }
}

#[test]
fn dod_ranges_expand() {
    let ids = dod_ids_in("M2-19..M2-21, M2-30 and XM2-40 and M3-1");
    let want: BTreeSet<String> = ["M2-19", "M2-20", "M2-21", "M2-30", "M3-1"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(ids, want);
}

// ---- positive controls (W2) ----------------------------------------------------------

/// Apply `f` to a copy of the committed docs and return the errors `check` reports.
/// Panics if the mutation did not change anything (a control that mutates nothing proves
/// nothing) or if `check` still passes.
fn errors_after(f: impl FnOnce(&mut Docs)) -> String {
    let original = Docs::committed();
    let mut d = Docs::committed();
    f(&mut d);
    assert!(
        d.plan != original.plan
            || d.milestones != original.milestones
            || d.dod_status != original.dod_status
            || d.gates != original.gates
            || d.backlog != original.backlog,
        "the mutation changed nothing, so this control would be vacuous"
    );
    match check(&d) {
        Ok(_) => panic!("check passed on a broken plan"),
        Err(e) => e.join("\n"),
    }
}

fn replace_once(s: &mut String, from: &str, to: &str) {
    assert!(s.contains(from), "control anchor {from:?} not found");
    *s = s.replacen(from, to, 1);
}

#[test]
fn positive_control_unknown_chapter_wp_or_dod_fails() {
    let e = errors_after(|d| {
        replace_once(
            &mut d.plan,
            "| `forge.undo_history` | Ch.7 |",
            "| `forge.undo_history` | Ch.99 |",
        )
    });
    assert!(
        e.contains("chapter Ch.99 is not a chapter in the document map"),
        "{e}"
    );

    let e = errors_after(|d| replace_once(&mut d.plan, "| WP-U6 | M2-32 |", "| WP-U99 | M2-32 |"));
    assert!(
        e.contains("work package WP-U99 is not defined in the §21.24 table"),
        "{e}"
    );

    let e = errors_after(|d| replace_once(&mut d.plan, "| WP-U6 | M2-32 |", "| WP-U6 | M2-99 |"));
    assert!(e.contains("DoD M2-99 is not an id in"), "{e}");
    assert!(e.contains("DoD M2-99 has no row in"), "{e}");
}

#[test]
fn positive_control_panel_wp_mismatch_fails() {
    // The viewport claims WP-U7, whose §21.24 row does not claim M2-32.
    let e = errors_after(|d| replace_once(&mut d.plan, "| WP-U6 | M2-32 |", "| WP-U7 | M2-32 |"));
    assert!(
        e.contains("WP-U7 does not claim M2-32 in the §21.24 table"),
        "{e}"
    );
}

#[test]
fn positive_control_missing_required_panel_fails() {
    let e = errors_after(|d| {
        d.plan = d
            .plan
            .lines()
            .filter(|l| !l.starts_with("| Licence status | `forge.licence` |"))
            .collect::<Vec<_>>()
            .join("\n");
    });
    assert!(
        e.contains("required panel forge.licence (WP-U0 scope: \"licence\") is missing"),
        "{e}"
    );
}

#[test]
fn positive_control_duplicate_panel_or_dod_fails() {
    let e = errors_after(|d| {
        replace_once(
            &mut d.plan,
            "| Console | `forge.console` |",
            "| Console | `forge.hierarchy` |",
        )
    });
    assert!(e.contains("duplicate panel id"), "{e}");
}

#[test]
fn positive_control_orphan_ui_dod_fails() {
    let e = errors_after(|d| {
        replace_once(
            &mut d.milestones,
            "| M2-70 |",
            "| M2-71 | an orphan row nobody cites |\n| M2-70 |",
        )
    });
    assert!(
        e.contains("UI DoD M2-71 in docs/plan/milestones.md is cited nowhere in Ch.21"),
        "{e}"
    );
}

#[test]
fn positive_control_budget_without_gate_row_fails() {
    let e = errors_after(|d| {
        d.gates = d
            .gates
            .lines()
            .filter(|l| !l.contains("(id: \"C-ui-glyph-atlas\""))
            .collect::<Vec<_>>()
            .join("\n");
    });
    assert!(
        e.contains("budget ui_single_glyph_atlas: gate row C-ui-glyph-atlas does not exist"),
        "{e}"
    );
}

#[test]
fn positive_control_missing_tables_fail_rather_than_pass_vacuously() {
    let e = errors_after(|d| {
        replace_once(
            &mut d.plan,
            "| Panel | Id | Chapters | WP | DoD |",
            "| Pane | Id | Chapters | WP | DoD |",
        );
        replace_once(&mut d.plan, "| Test | Budget |", "| Tests | Budget |");
    });
    assert!(e.contains("no panel inventory table"), "{e}");
    assert!(e.contains("no budget table"), "{e}");
    // With no inventory, every required panel is reported missing too.
    assert!(e.contains("required panel forge.hierarchy"), "{e}");
}

#[test]
fn positive_control_unscheduled_panel_outside_gap_table_fails() {
    // Unschedule the undo history without adding a gap row: nothing tracks the panel.
    let e = errors_after(|d| {
        replace_once(
            &mut d.plan,
            UNDO_ROW_TAIL,
            "| UNSCHEDULED | M2-41 | `forge-panels-core` | the bus's transaction log | — (gap, §21.24) |",
        );
        replace_once(
            &mut d.plan,
            "| M2-28, M2-41..M2-44 |",
            "| M2-28, M2-42..M2-44 |",
        );
    });
    assert!(
        e.contains("is UNSCHEDULED but M2-41 is not in the §21.24 gap table"),
        "{e}"
    );

    // A gap row for a panel a WP already claims: claimed twice.
    let e = errors_after(|d| {
        replace_once(
            &mut d.plan,
            "| Gap | Panels | DoD | Proposed backlog change |\n|---|---|---|---|\n",
            "| Gap | Panels | DoD | Proposed backlog change |\n|---|---|---|---|\n| G-1 | undo history | M2-41 | extend WP-U4 |\n",
        );
    });
    assert!(
        e.contains("M2-41 is in the §21.24 gap table and also claimed by a work package"),
        "{e}"
    );

    // A scheduled panel with no backlog clause to show its WP's backlog scope names it.
    let e = errors_after(|d| {
        replace_once(
            &mut d.plan,
            UNDO_ROW_TAIL,
            "| WP-U4 | M2-41 | `forge-panels-core` | the bus's transaction log | — |",
        );
    });
    assert!(e.contains("quotes no backlog clause"), "{e}");
}

/// A backlog that agrees with the committed plan: each §21.24 WP, with the backlog
/// clauses of its panels as its scope.
fn backlog_from_plan(plan: &str) -> String {
    let ch21 = section(plan, "# Chapter 21 ").expect("Chapter 21");
    let mut scopes: BTreeMap<String, String> = table(&ch21, &["WP", "Scope", "DoD"])
        .expect("§21.24 table")
        .into_iter()
        .map(|r| (r[0].clone(), String::new()))
        .collect();
    for r in table(&ch21, &["Panel", "Id", "Chapters", "WP", "DoD"]).expect("inventory") {
        if let (Some(scope), Some(clause)) = (scopes.get_mut(&r[3]), r.get(7)) {
            scope.push_str(&unquote(clause));
            scope.push_str("; ");
        }
    }
    let items: Vec<serde_json::Value> = scopes
        .into_iter()
        .map(|(id, scope)| serde_json::json!({ "id": id, "title": "", "scope": scope }))
        .collect();
    serde_json::Value::Array(items).to_string()
}

#[test]
fn backlog_built_from_the_plan_passes() {
    // The controls' baseline: the cross-check accepts an agreeing backlog, so the failures
    // in the next test are caused by their mutations and nothing else.
    let mut d = Docs::committed();
    d.backlog = Some(backlog_from_plan(&d.plan));
    let s = check(&d).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    assert!(s.backlog_checked);
}

#[test]
fn positive_control_backlog_disagreement_fails() {
    // A panel whose WP's backlog scope does not name it (how "revision history" on
    // WP-U7 went unnoticed): the backlog no longer contains the viewport's clause.
    let e = errors_after(|d| {
        let b = backlog_from_plan(&d.plan);
        d.backlog = Some(b.replace("Viewport panel hosting a render surface", "something else"));
    });
    assert!(
        e.contains(
            "backlog clause \"Viewport panel hosting a render surface\" is not in WP-U6's backlog scope"
        ),
        "{e}"
    );

    // A WP in §21.24 that the backlog does not define (how "WP-U13" went unnoticed).
    let e = errors_after(|d| {
        let b = backlog_from_plan(&d.plan);
        d.backlog = Some(b.replace("\"WP-U12\"", "\"WP-U12-renamed\""));
    });
    assert!(
        e.contains("work package WP-U12 is in the §21.24 table but not in the backlog"),
        "{e}"
    );
}

#[test]
fn positive_control_clause_that_does_not_name_the_panel_fails() {
    // The hole the verifier found: a panel quoting a clause that IS in its WP's backlog
    // scope but describes another feature. Every table agrees and the backlog contains the
    // clause, yet the WP's implementer would never build the panel from it.
    let e = errors_after(|d| {
        replace_once(
            &mut d.plan,
            UNDO_ROW_TAIL,
            "| WP-U4 | M2-41 | `forge-panels-core` | the bus's transaction log | \"transform gizmos\" |",
        );
        // A backlog in which WP-U4's scope contains "transform gizmos", so the backlog half
        // passes and only the naming rule can catch it.
        d.backlog = Some(backlog_from_plan(&d.plan));
    });
    assert!(
        e.contains(
            "panel \"Undo history\" (forge.undo_history): backlog clause \"transform gizmos\" does not name the panel"
        ),
        "{e}"
    );
    assert!(
        !e.contains("not in WP-U4's backlog scope"),
        "the backlog half must pass here, so the naming rule alone is under test: {e}"
    );

    // A clause that names a panel, but a different one: the hierarchy handed to WP-U10
    // with the presence clause, which mentions "hierarchy" only as where badges appear.
    let e = errors_after(|d| {
        replace_once(
            &mut d.plan,
            "| WP-U5 | M2-35 | `forge-panels-scene` | `ProjectMirror` | \"Scene hierarchy panel\" |",
            "| WP-U10 | M2-35 | `forge-panels-scene` | `ProjectMirror` | \"presence indicators in hierarchy/inspector/viewport\" |",
        );
        unclaim_hierarchy(&mut d.plan);
        replace_once(
            &mut d.plan,
            "| M2-57..M2-62, M2-69 |",
            "| M2-35, M2-57..M2-62, M2-69 |",
        );
        d.backlog = Some(backlog_from_plan(&d.plan));
    });
    assert!(
        e.contains("does not name the panel (names panel \"Presence\" better than this one)"),
        "{e}"
    );
}

#[test]
fn every_committed_clause_names_its_panel_and_no_other() {
    // The naming rule's baseline on real data, spelled out so a change to the word rules
    // that breaks a committed row fails here with the row named.
    let d = Docs::committed();
    let ch21 = section(&d.plan, "# Chapter 21 ").expect("Chapter 21");
    let rows = table(&ch21, &["Panel", "Id", "Chapters", "WP", "DoD"]).expect("inventory");
    let names: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    let mut scheduled = 0;
    for r in rows.iter().filter(|r| r[3] != UNSCHEDULED) {
        let clause = unquote(&r[7]);
        names_panel(&r[0], &clause, &names)
            .unwrap_or_else(|why| panic!("{:?} / {clause:?}: {why}", r[0]));
        scheduled += 1;
    }
    assert!(scheduled >= 25, "only {scheduled} scheduled panels parsed");
    // And the rule is not vacuous: an unrelated clause names nothing.
    assert!(names_panel("Undo history", "transform gizmos", &names).is_err());
}

/// The committed undo-history row from its WP cell on (the controls' anchor).
const UNDO_ROW_TAIL: &str = "| WP-U4 | M2-41 | `forge-panels-core` | the bus's transaction log | \"undo/redo UI with history panel\" |";

/// Take M2-35 (the hierarchy) out of WP-U5's §21.24 claims, whichever way the row spells it:
/// the start of a range (the next id then starts it), or one id of a list.
fn unclaim_hierarchy(plan: &mut String) {
    let row = plan
        .lines()
        .find(|l| l.starts_with("| WP-U5 |"))
        .unwrap_or_else(|| panic!("control anchor: the WP-U5 row"))
        .to_string();
    let fixed = if row.contains("M2-35..") {
        row.replace("M2-35..", &format!("M2-{}..", 35 + 1))
    } else {
        assert!(row.contains("M2-35, "), "control anchor: M2-35 in {row}");
        row.replace("M2-35, ", "")
    };
    *plan = plan.replacen(&row, &fixed, 1);
}

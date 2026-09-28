//! `cargo xtask dod` — every definition-of-done item and its status (Ch.30.1).
//!
//! Statuses (milestones.md): `settled` · `blocked(reason)` · `superseded(by)` · `UNMET` ·
//! `unread`. **A milestone ships at 0 UNMET and 0 unread.**
//!
//! The status data lives in `docs/plan/dod-status.ron`; the *ids* live in
//! `docs/plan/milestones.md`. This command cross-checks the two, so a DoD id appended to
//! milestones.md without a status — or a status row for an id that does not exist, e.g.
//! after someone renumbered — fails loudly instead of disappearing.
//!
//! Part of an item can be superseded while the rest stands (E-33 drops the macOS leg from
//! M0-1 and M0-3 but not the rest of either). That is recorded as a `superseded_wording`
//! entry beside the item's own status, never by rewriting the milestone text.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use crate::util::{Failure, XResult, is_banned_reason, read};

pub const STATUS_FILE: &str = "docs/plan/dod-status.ron";
pub const MILESTONES_FILE: &str = "docs/plan/milestones.md";
pub const DECISIONS_FILE: &str = "docs/plan/decisions.md";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StatusFile {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub status: Status,
    #[serde(default)]
    pub superseded_wording: Vec<Wording>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub enum Status {
    /// Settled, with the repo paths that prove it. Each must exist.
    Settled {
        evidence: Vec<String>,
    },
    Blocked {
        reason: String,
    },
    Superseded {
        by: String,
    },
    Unmet,
    Unread,
}

/// Part of an item's wording that a later decision overrode.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Wording {
    pub by: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Only print this milestone (e.g. `M0`).
    pub milestone: Option<String>,
    /// Fail unless this milestone has 0 UNMET and 0 unread.
    pub ship: Option<String>,
}

pub fn parse(src: &str) -> XResult<StatusFile> {
    ron::from_str(src).map_err(|e| Failure::one(format!("{STATUS_FILE}: parse error: {e}")))
}

/// Every DoD id in milestones.md, in document order. A DoD row is a table row whose first
/// cell is `M<digit>-<digits>`.
pub fn milestone_ids(milestones_md: &str) -> Vec<String> {
    milestones_md
        .lines()
        .filter_map(|l| {
            let cell = l.trim().strip_prefix('|')?.split('|').next()?.trim();
            is_dod_id(cell).then(|| cell.to_string())
        })
        .collect()
}

fn is_dod_id(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('M') else {
        return false;
    };
    let Some((m, n)) = rest.split_once('-') else {
        return false;
    };
    !m.is_empty()
        && !n.is_empty()
        && m.bytes().all(|b| b.is_ascii_digit())
        && n.bytes().all(|b| b.is_ascii_digit())
}

/// Decision ids (`E-33`, `O-6`, ...) defined in decisions.md table rows.
pub fn decision_ids(decisions_md: &str) -> BTreeSet<String> {
    decisions_md
        .lines()
        .filter_map(|l| {
            let cell = l.trim().strip_prefix('|')?.split('|').next()?.trim();
            let (p, n) = cell.split_once('-')?;
            (p.len() == 1
                && p.bytes().all(|b| b.is_ascii_uppercase())
                && !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit()))
            .then(|| cell.to_string())
        })
        .collect()
}

fn valid_supersessor(by: &str, decisions: &BTreeSet<String>, ids: &BTreeSet<&str>) -> bool {
    decisions.contains(by) || ids.contains(by)
}

pub fn validate(
    file: &StatusFile,
    milestone_ids: &[String],
    decisions: &BTreeSet<String>,
    root: &Path,
) -> XResult<()> {
    let mut errs = Vec::new();
    let expected: BTreeSet<&str> = milestone_ids.iter().map(String::as_str).collect();
    let mut seen = BTreeSet::new();
    for item in &file.items {
        if !seen.insert(item.id.as_str()) {
            errs.push(format!("{STATUS_FILE}: duplicate item {}", item.id));
        }
        if !expected.contains(item.id.as_str()) {
            errs.push(format!(
                "{STATUS_FILE}: {} is not a DoD id in {MILESTONES_FILE} (ids are identities: \
                 never renumber, append new ones)",
                item.id
            ));
        }
        match &item.status {
            Status::Settled { evidence } => {
                if evidence.is_empty() {
                    errs.push(format!(
                        "{STATUS_FILE}: {} is settled with no evidence — a DoD item is settled by \
                         a file, not a claim",
                        item.id
                    ));
                }
                for e in evidence {
                    if !root.join(e).exists() {
                        errs.push(format!(
                            "{STATUS_FILE}: {} cites evidence {e}, which does not exist",
                            item.id
                        ));
                    }
                }
            }
            Status::Blocked { reason } => {
                if is_banned_reason(reason) {
                    errs.push(format!(
                        "{STATUS_FILE}: {} is blocked with {reason:?}, which is not a reason (W9)",
                        item.id
                    ));
                }
            }
            Status::Superseded { by } => {
                if !valid_supersessor(by, decisions, &expected) {
                    errs.push(format!(
                        "{STATUS_FILE}: {} is superseded by {by}, which is neither a decision in \
                         {DECISIONS_FILE} nor a DoD id",
                        item.id
                    ));
                }
            }
            Status::Unmet | Status::Unread => {}
        }
        for w in &item.superseded_wording {
            if !valid_supersessor(&w.by, decisions, &expected) {
                errs.push(format!(
                    "{STATUS_FILE}: {} has wording superseded by {}, which is neither a decision \
                     in {DECISIONS_FILE} nor a DoD id",
                    item.id, w.by
                ));
            }
            if w.text.trim().is_empty() {
                errs.push(format!(
                    "{STATUS_FILE}: {} has a superseded-wording entry with no text",
                    item.id
                ));
            }
        }
    }
    for id in milestone_ids {
        if !seen.contains(id.as_str()) {
            errs.push(format!(
                "{STATUS_FILE}: DoD id {id} from {MILESTONES_FILE} has no status row — add one \
                 (a new item starts as UNMET or unread, never as silence)"
            ));
        }
    }
    Failure::many(errs)
}

fn milestone_of(id: &str) -> &str {
    id.split_once('-').map_or(id, |(m, _)| m)
}

fn label(s: &Status) -> (&'static str, String) {
    match s {
        Status::Settled { evidence } => ("settled", format!("evidence: {}", evidence.join(", "))),
        Status::Blocked { reason } => ("blocked", format!("reason: {reason}")),
        Status::Superseded { by } => ("superseded", format!("by: {by}")),
        Status::Unmet => ("UNMET", String::new()),
        Status::Unread => ("unread", String::new()),
    }
}

/// Render the table in milestones.md order, with a per-milestone summary.
pub fn render(file: &StatusFile, order: &[String], opts: &Options) -> String {
    let by_id: BTreeMap<&str, &Item> = file.items.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut out = String::new();
    let mut summary: BTreeMap<&str, [usize; 5]> = BTreeMap::new();
    let mut current = "";
    for id in order {
        let Some(item) = by_id.get(id.as_str()) else {
            continue;
        };
        let m = milestone_of(id);
        if opts.milestone.as_deref().is_some_and(|f| f != m) {
            continue;
        }
        if m != current {
            if !current.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("{m}\n"));
            current = m;
        }
        let (state, detail) = label(&item.status);
        let slot = match item.status {
            Status::Settled { .. } => 0,
            Status::Blocked { .. } => 1,
            Status::Superseded { .. } => 2,
            Status::Unmet => 3,
            Status::Unread => 4,
        };
        summary.entry(m).or_default()[slot] += 1;
        out.push_str(format!("  {id:<7}{state:<12}{detail}").trim_end());
        out.push('\n');
        for w in &item.superseded_wording {
            out.push_str(&format!(
                "  {:<7}{:<12}wording superseded(by {}): {}\n",
                "", "", w.by, w.text
            ));
        }
    }
    out.push_str("\nsummary        settled  blocked  superseded  UNMET  unread\n");
    for (m, c) in &summary {
        let ships = if c[3] == 0 && c[4] == 0 {
            "ships"
        } else {
            "does not ship"
        };
        out.push_str(&format!(
            "  {m:<13}{:>7}  {:>7}  {:>10}  {:>5}  {:>6}   {ships}\n",
            c[0], c[1], c[2], c[3], c[4]
        ));
    }
    out
}

pub fn ship_check(file: &StatusFile, milestone: &str) -> XResult<()> {
    let open: Vec<String> = file
        .items
        .iter()
        .filter(|i| milestone_of(&i.id) == milestone)
        .filter(|i| matches!(i.status, Status::Unmet | Status::Unread))
        .map(|i| {
            format!(
                "{milestone} does not ship: {} is {}",
                i.id,
                label(&i.status).0
            )
        })
        .collect();
    Failure::many(open)
}

pub fn run(root: &Path, opts: &Options) -> XResult<String> {
    let file = parse(&read(&root.join(STATUS_FILE))?)?;
    let ids = milestone_ids(&read(&root.join(MILESTONES_FILE))?);
    let decisions = decision_ids(&read(&root.join(DECISIONS_FILE))?);
    validate(&file, &ids, &decisions, root)?;
    let out = render(&file, &ids, opts);
    if let Some(m) = &opts.ship {
        ship_check(&file, m)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::workspace_root;

    const MD: &str = "| id | Definition |\n|---|---|\n| M0-1 | a |\n| M0-2 | b |\n| M1-1 | c |\n";

    fn decisions() -> BTreeSet<String> {
        ["E-33".to_string()].into_iter().collect()
    }

    #[test]
    fn extracts_ids_in_order() {
        assert_eq!(milestone_ids(MD), vec!["M0-1", "M0-2", "M1-1"]);
    }

    #[test]
    fn committed_status_file_covers_every_milestone_id() {
        let root = workspace_root();
        let out = run(&root, &Options::default()).expect("dod-status.ron must validate");
        let ids = milestone_ids(&read(&root.join(MILESTONES_FILE)).unwrap());
        assert!(
            ids.len() > 100,
            "expected the full M0..M6 id list, got {}",
            ids.len()
        );
        for id in &ids {
            assert!(
                out.lines()
                    .any(|l| l.trim_start().starts_with(&format!("{id} "))),
                "{id} not printed"
            );
        }
        assert!(ids.contains(&"M0-1".to_string()) && ids.contains(&"M5-27".to_string()));
    }

    #[test]
    fn committed_status_records_e33_on_m0_1_and_m0_3() {
        let root = workspace_root();
        let f = parse(&read(&root.join(STATUS_FILE)).unwrap()).unwrap();
        for id in ["M0-1", "M0-3"] {
            let item = f.items.iter().find(|i| i.id == id).unwrap();
            assert!(
                item.superseded_wording.iter().any(|w| w.by == "E-33"),
                "{id} must record its macOS wording as superseded(by E-33)"
            );
        }
    }

    #[test]
    fn decision_ids_are_read_from_table_rows() {
        let d = decision_ids("| E-33 | x |\n| O-6 | y |\n| id | z |\n");
        assert!(d.contains("E-33") && d.contains("O-6") && !d.contains("id"));
    }

    // ---- positive controls ----

    #[test]
    fn positive_control_an_id_with_no_status_fails() {
        let f = parse("(items: [(id: \"M0-1\", status: Unmet), (id: \"M1-1\", status: Unread)])")
            .unwrap();
        let err = validate(&f, &milestone_ids(MD), &decisions(), &workspace_root()).unwrap_err();
        assert!(err.to_string().contains("DoD id M0-2"), "{err}");
    }

    #[test]
    fn positive_control_a_renumbered_id_fails() {
        let f = parse(
            "(items: [(id: \"M0-1\", status: Unmet), (id: \"M0-2\", status: Unmet), \
             (id: \"M1-1\", status: Unread), (id: \"M0-3\", status: Unmet)])",
        )
        .unwrap();
        let err = validate(&f, &milestone_ids(MD), &decisions(), &workspace_root()).unwrap_err();
        assert!(err.to_string().contains("M0-3 is not a DoD id"), "{err}");
    }

    #[test]
    fn positive_control_settled_without_existing_evidence_fails() {
        let f = parse(
            "(items: [(id: \"M0-1\", status: Settled(evidence: [\"no/such/file.rs\"])), \
             (id: \"M0-2\", status: Settled(evidence: [])), (id: \"M1-1\", status: Unread)])",
        )
        .unwrap();
        let err = validate(&f, &milestone_ids(MD), &decisions(), &workspace_root()).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("no/such/file.rs, which does not exist"), "{s}");
        assert!(s.contains("M0-2 is settled with no evidence"), "{s}");
    }

    #[test]
    fn positive_control_superseded_by_unknown_decision_fails() {
        let f = parse(
            "(items: [(id: \"M0-1\", status: Superseded(by: \"E-999\")), \
             (id: \"M0-2\", status: Unmet, superseded_wording: [(by: \"E-998\", text: \"x\")]), \
             (id: \"M1-1\", status: Unread)])",
        )
        .unwrap();
        let err = validate(&f, &milestone_ids(MD), &decisions(), &workspace_root()).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("superseded by E-999"), "{s}");
        assert!(s.contains("superseded by E-998"), "{s}");
    }

    #[test]
    fn positive_control_ship_fails_with_unmet_or_unread() {
        let f = parse("(items: [(id: \"M0-1\", status: Unmet), (id: \"M1-1\", status: Unread)])")
            .unwrap();
        assert!(ship_check(&f, "M0").is_err());
        assert!(ship_check(&f, "M1").is_err());
        let ok = parse(
            "(items: [(id: \"M0-1\", status: Blocked(reason: \"awaiting a Linux runner\"))])",
        )
        .unwrap();
        assert!(ship_check(&ok, "M0").is_ok());
    }

    #[test]
    fn render_prints_wording_supersession() {
        let f = parse(
            "(items: [(id: \"M0-1\", status: Unmet, superseded_wording: [(by: \"E-33\", text: \"macOS leg\")])])",
        )
        .unwrap();
        let out = render(&f, &["M0-1".to_string()], &Options::default());
        assert!(
            out.contains("wording superseded(by E-33): macOS leg"),
            "{out}"
        );
        assert!(out.contains("does not ship"), "{out}");
    }
}

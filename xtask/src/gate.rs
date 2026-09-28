//! `cargo xtask gate` — prints one row per invariant and per pinned contract (Ch.30.1).
//!
//! The rows live in `tests/gates.ron`. Three terminal states and nothing else
//! (milestones.md, "Gate rows — the format"):
//!
//! * `BOUND`  — a test file asserts it *and* has a non-vacuous positive control (W2). The
//!   row names both, and this command checks the file exists and defines the control fn,
//!   so a row cannot point at a test or control that was never written.
//! * `AWAITING(reason)` — blocked on hardware, an operator, or an input not on disk.
//! * `UNBUILT(reason)` — the subsystem does not exist yet, and the reason says why that is
//!   correct right now. "Nobody got to it" is not a reason (W9).

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use crate::util::{Failure, XResult, is_banned_reason, read};

pub const GATES_FILE: &str = "tests/gates.ron";

/// Every invariant id the master plan defines. All must have a row.
pub const INVARIANTS: std::ops::RangeInclusive<u32> = 1..=21;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GateFile {
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub state: State,
}

/// The three legal states. A fourth is unrepresentable: it fails to parse.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub enum State {
    Bound { file: String, control: String },
    Awaiting { reason: String },
    Unbuilt { reason: String },
}

pub fn parse(src: &str) -> XResult<GateFile> {
    ron::from_str(src).map_err(|e| Failure::one(format!("{GATES_FILE}: parse error: {e}")))
}

/// Validate every rule the gate format imposes. `root` is used to resolve BOUND files.
pub fn validate(file: &GateFile, root: &Path) -> XResult<()> {
    let mut errs = Vec::new();
    let mut seen = BTreeSet::new();
    for row in &file.rows {
        if !seen.insert(row.id.clone()) {
            errs.push(format!("{GATES_FILE}: duplicate row id {}", row.id));
        }
        if !is_row_id(&row.id) {
            errs.push(format!(
                "{GATES_FILE}: row id {:?} is not I<n>, S<n> or a pinned contract C-<name>",
                row.id
            ));
        }
        if row.name.trim().is_empty() {
            errs.push(format!("{GATES_FILE}: row {} has an empty name", row.id));
        }
        match &row.state {
            State::Bound { file, control } => {
                let path = root.join(file);
                match std::fs::read_to_string(&path) {
                    Err(_) => errs.push(format!(
                        "{GATES_FILE}: row {} is BOUND to {file}, which does not exist (W2: a guard \
                         naming a file that was never written reports green while guarding nothing)",
                        row.id
                    )),
                    Ok(src) => {
                        if control.trim().is_empty() {
                            errs.push(format!(
                                "{GATES_FILE}: row {} is BOUND without a positive control (W2)",
                                row.id
                            ));
                        } else if !defines_fn(&src, control) {
                            errs.push(format!(
                                "{GATES_FILE}: row {} names positive control `{control}`, but {file} \
                                 defines no `fn {control}` (W2)",
                                row.id
                            ));
                        }
                    }
                }
            }
            State::Awaiting { reason } | State::Unbuilt { reason } => {
                if is_banned_reason(reason) {
                    errs.push(format!(
                        "{GATES_FILE}: row {} has reason {reason:?}, which is not a reason (W9)",
                        row.id
                    ));
                }
            }
        }
    }
    for n in planned_invariants(root) {
        let id = format!("I{n}");
        if !seen.contains(&id) {
            errs.push(format!(
                "{GATES_FILE}: invariant {id} has no gate row — every invariant must be BOUND, \
                 AWAITING(reason) or UNBUILT(reason); silence is not a state (W9)"
            ));
        }
    }
    Failure::many(errs)
}

/// The invariants the master plan's table lists (its `| **I<n>** |` rows), or the whole
/// range when there is no plan to read. An edition whose plan leaves an invariant out (the
/// public export does not carry a premium one, ADR 0062) gates the ones it lists; every
/// listed invariant must have a row.
#[must_use]
pub fn planned_invariants(root: &Path) -> Vec<u32> {
    let listed: Vec<u32> = std::fs::read_to_string(root.join("docs/plan/master-plan.md"))
        .map(|t| {
            t.lines()
                .filter_map(|l| l.strip_prefix("| **I")?.split_once("** |")?.0.parse().ok())
                .filter(|n| INVARIANTS.contains(n))
                .collect()
        })
        .unwrap_or_default();
    if listed.is_empty() {
        INVARIANTS.collect()
    } else {
        listed
    }
}

fn is_row_id(id: &str) -> bool {
    let num = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = id.strip_prefix("C-") {
        return !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    }
    if let Some(rest) = id.strip_prefix('I') {
        return num(rest);
    }
    if let Some(rest) = id.strip_prefix('S') {
        return num(rest);
    }
    false
}

/// True if `src` contains a definition `fn <name>` (followed by `(` or `<`).
pub(crate) fn defines_fn(src: &str, name: &str) -> bool {
    let needle = format!("fn {name}");
    src.match_indices(&needle).any(|(i, _)| {
        let after = &src[i + needle.len()..];
        after.starts_with('(') || after.starts_with('<')
    })
}

/// Where a BOUND test that could not run on this machine records why (W9): one file per
/// row, `<row id>.txt`, holding the reason. The test deletes it again when it runs for real.
/// `just verify` runs the tests before `cargo xtask gate`, so the gate reports what the last
/// run actually observed instead of a static BOUND.
pub const OBSERVATIONS_DIR: &str = "target/gate-observations";

/// Runtime observations: row id -> the AWAITING reason a bound test recorded.
pub type Observations = std::collections::BTreeMap<String, String>;

/// Read the observations under `root` (none if the directory does not exist).
pub fn observations(root: &Path) -> Observations {
    let mut out = Observations::new();
    let Ok(dir) = std::fs::read_dir(root.join(OBSERVATIONS_DIR)) else {
        return out;
    };
    for e in dir.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "txt")
            && let (Some(id), Ok(reason)) = (
                p.file_stem().and_then(|s| s.to_str()).map(str::to_string),
                std::fs::read_to_string(&p),
            )
        {
            out.insert(id, reason.trim().to_string());
        }
    }
    out
}

/// Render the table. Pure so it can be tested.
pub fn render(file: &GateFile) -> String {
    render_observed(file, &Observations::new())
}

/// Render the table, reporting a BOUND row whose test recorded an observation as
/// `AWAITING(observed: reason)` — a bound guard that did not run is never shown green.
pub fn render_observed(file: &GateFile, observed: &Observations) -> String {
    let mut out = String::new();
    let (mut bound, mut awaiting, mut unbuilt) = (0, 0, 0);
    let idw = file.rows.iter().map(|r| r.id.len()).max().unwrap_or(0) + 2;
    let namew = file.rows.iter().map(|r| r.name.len()).max().unwrap_or(0) + 2;
    for row in &file.rows {
        let (state, detail) = match &row.state {
            State::Bound { file, control } => match observed.get(&row.id) {
                Some(reason) => {
                    awaiting += 1;
                    (
                        "AWAITING",
                        format!(
                            "reason: observed at the last test run: {reason} (bound to {file}, \
                             positive control: {control})"
                        ),
                    )
                }
                None => {
                    bound += 1;
                    ("BOUND", format!("{file}  (positive control: {control})"))
                }
            },
            State::Awaiting { reason } => {
                awaiting += 1;
                ("AWAITING", format!("reason: {reason}"))
            }
            State::Unbuilt { reason } => {
                unbuilt += 1;
                ("UNBUILT", format!("reason: {reason}"))
            }
        };
        out.push_str(&format!(
            "{:<idw$}{:<namew$}{:<10}{}\n",
            row.id, row.name, state, detail
        ));
    }
    out.push_str(&format!(
        "\n{} rows: {bound} BOUND, {awaiting} AWAITING, {unbuilt} UNBUILT\n",
        file.rows.len()
    ));
    out
}

pub fn run(root: &Path) -> XResult<String> {
    let file = parse(&read(&root.join(GATES_FILE))?)?;
    validate(&file, root)?;
    Ok(render_observed(&file, &observations(root)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::workspace_root;

    fn row(n: u32) -> String {
        format!(
            "(id: \"I{n}\", name: \"inv-{n}\", state: Unbuilt(reason: \"subsystem for I{n} lands later in M0\"))"
        )
    }

    fn all_unbuilt() -> String {
        let rows: Vec<String> = INVARIANTS.map(row).collect();
        format!("(rows: [{}])", rows.join(",\n"))
    }

    #[test]
    fn the_committed_gate_file_is_valid_and_lists_every_invariant() {
        let out = run(&workspace_root()).expect("committed tests/gates.ron must validate");
        let planned = planned_invariants(&workspace_root());
        // The source plan lists every invariant; the public edition's, the ones it carries.
        if workspace_root().join("premium.ron").is_file() {
            assert_eq!(planned, INVARIANTS.collect::<Vec<_>>());
        } else {
            assert!(
                planned.len() >= 15,
                "the plan lists its invariants: {planned:?}"
            );
        }
        for n in planned {
            assert!(
                out.lines().any(|l| l.starts_with(&format!("I{n} "))),
                "I{n} missing from gate output"
            );
        }
        for line in out.lines().take_while(|l| !l.is_empty()) {
            let state = line.split_whitespace().nth(2).unwrap_or("");
            assert!(
                matches!(state, "BOUND" | "AWAITING" | "UNBUILT"),
                "illegal state in {line:?}"
            );
        }
    }

    #[test]
    fn a_complete_unbuilt_file_passes() {
        let f = parse(&all_unbuilt()).unwrap();
        validate(&f, &workspace_root()).unwrap();
    }

    // ---- positive controls: each broken property must make the gate fail ----

    #[test]
    fn positive_control_missing_invariant_fails() {
        let src = all_unbuilt().replace(&format!("{},", row(7)), "");
        let f = parse(&src).unwrap();
        assert_eq!(f.rows.len(), 20);
        let err = validate(&f, &workspace_root()).unwrap_err();
        assert!(
            err.to_string().contains("invariant I7 has no gate row"),
            "{err}"
        );
    }

    #[test]
    fn positive_control_nobody_got_to_it_is_not_a_reason() {
        let src = all_unbuilt().replace("subsystem for I3 lands later in M0", "nobody got to it");
        let err = validate(&parse(&src).unwrap(), &workspace_root()).unwrap_err();
        assert!(err.to_string().contains("not a reason"), "{err}");
    }

    #[test]
    fn positive_control_bound_to_nonexistent_file_fails() {
        let src = all_unbuilt().replace(
            "state: Unbuilt(reason: \"subsystem for I1 lands later in M0\")",
            "state: Bound(file: \"tests/liveness/never_written.rs\", control: \"x\")",
        );
        let err = validate(&parse(&src).unwrap(), &workspace_root()).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn positive_control_bound_with_undefined_control_fails() {
        let src = all_unbuilt().replace(
            "state: Unbuilt(reason: \"subsystem for I12 lands later in M0\")",
            "state: Bound(file: \"xtask/src/gate_parity.rs\", control: \"control_that_does_not_exist\")",
        );
        let err = validate(&parse(&src).unwrap(), &workspace_root()).unwrap_err();
        assert!(
            err.to_string()
                .contains("defines no `fn control_that_does_not_exist`"),
            "{err}"
        );
    }

    #[test]
    fn positive_control_a_fourth_state_does_not_parse() {
        let src = all_unbuilt().replace(
            "Unbuilt(reason: \"subsystem for I2 lands later in M0\")",
            "Pending(reason: \"subsystem for I2 lands later in M0\")",
        );
        assert!(
            parse(&src).is_err(),
            "a fourth state must be unrepresentable"
        );
    }

    #[test]
    fn positive_control_duplicate_row_fails() {
        let src = all_unbuilt().replace(
            "(rows: [",
            "(rows: [(id: \"I1\", name: \"dup\", state: Unbuilt(reason: \"a duplicate row for testing\")),",
        );
        let err = validate(&parse(&src).unwrap(), &workspace_root()).unwrap_err();
        assert!(err.to_string().contains("duplicate row id I1"), "{err}");
    }

    #[test]
    fn a_bound_row_whose_test_could_not_run_reports_awaiting() {
        let src = all_unbuilt().replace(
            "state: Unbuilt(reason: \"subsystem for I12 lands later in M0\")",
            "state: Bound(file: \"xtask/src/gate_parity.rs\", control: \"positive_control_mismatched_lists_fail\")",
        );
        let f = parse(&src).unwrap();
        let clean = render_observed(&f, &Observations::new());
        assert!(
            clean
                .lines()
                .any(|l| l.starts_with("I12 ") && l.contains("BOUND"))
        );
        let mut obs = Observations::new();
        obs.insert("I12".into(), "no GPU adapter".into());
        let out = render_observed(&f, &obs);
        let line = out
            .lines()
            .find(|l| l.starts_with("I12 "))
            .expect("I12 row");
        assert!(
            line.contains("AWAITING")
                && line.contains("no GPU adapter")
                && !line.contains(" BOUND "),
            "{line}"
        );
        assert!(out.contains("0 BOUND, 1 AWAITING"), "{out}");
    }

    #[test]
    fn observations_are_read_from_the_observations_dir() {
        let dir = std::env::temp_dir().join(format!("forge-gate-obs-{}", std::process::id()));
        let obs = dir.join(OBSERVATIONS_DIR);
        std::fs::create_dir_all(&obs).unwrap();
        std::fs::write(obs.join("C-ui-pixel-goldens.txt"), "no adapter\n").unwrap();
        let got = observations(&dir);
        assert_eq!(
            got.get("C-ui-pixel-goldens").map(String::as_str),
            Some("no adapter")
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert!(observations(&dir).is_empty());
    }

    #[test]
    fn defines_fn_needs_a_real_definition() {
        assert!(defines_fn("fn foo() {}", "foo"));
        assert!(defines_fn("pub fn foo<T>() {}", "foo"));
        assert!(!defines_fn("fn foobar() {}", "foo"));
        assert!(!defines_fn("// calls foo()", "foo"));
    }
}

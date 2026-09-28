//! `cargo xtask gate-parity` — invariant I12: the local gate is a superset of the remote gate.
//!
//! W4: `just verify` is CI's command list, in CI's order. This implementation is stricter
//! than "superset": the two lists must be **identical and in the same order**, because the
//! owner's rule is that a green `just verify` predicts a green push, and any extra local
//! step would be a step CI never runs.
//!
//! What is compared:
//! * the command lines of the `verify:` recipe in `justfile`, and
//! * the `run:` lines of every CI job between the markers
//!   `# >>> just verify` and `# <<< just verify` in `.github/workflows/ci.yml`.
//!
//! Inside the marked region CI may not hide anything: a multi-line `run: |` block, a step
//! `if:` condition, or `continue-on-error` is rejected, because each makes a step that runs
//! (or fails) differently from its local twin. The CI matrix must cover exactly the co-primary
//! platforms (I18, D-2): windows and ubuntu, no macOS.

use std::path::Path;

use crate::util::{Failure, XResult, read};

pub const JUSTFILE: &str = "justfile";
pub const CI_FILE: &str = ".github/workflows/ci.yml";
pub const BEGIN: &str = "# >>> just verify";
pub const END: &str = "# <<< just verify";

/// The command lines of a just recipe, with just's line prefixes (`@`, `-`) stripped and
/// whitespace normalised. Comment lines are skipped.
pub fn recipe_commands(justfile: &str, recipe: &str) -> XResult<Vec<String>> {
    let mut lines = justfile.lines();
    let header = format!("{recipe}:");
    let mut found = false;
    for l in lines.by_ref() {
        let t = l.trim_start_matches('@');
        if !l.starts_with([' ', '\t']) && (t == header || t.starts_with(&format!("{header} "))) {
            found = true;
            break;
        }
    }
    if !found {
        return Err(Failure::one(format!("{JUSTFILE}: no `{recipe}` recipe")));
    }
    let mut cmds = Vec::new();
    for l in lines {
        if l.trim().is_empty() {
            // just allows blank lines inside a recipe only if followed by indented lines;
            // treat a blank line as continuing and let the next non-indented line end it.
            continue;
        }
        if !l.starts_with([' ', '\t']) {
            break;
        }
        let t = l.trim();
        if t.starts_with('#') {
            continue;
        }
        let t = t.trim_start_matches(['@', '-']).trim();
        if t.ends_with('\\') {
            return Err(Failure::one(format!(
                "{JUSTFILE}: `{recipe}` uses a line continuation; keep one command per line so \
                 gate-parity compares like with like"
            )));
        }
        cmds.push(normalise(t));
    }
    if cmds.is_empty() {
        return Err(Failure::one(format!(
            "{JUSTFILE}: `{recipe}` has no commands"
        )));
    }
    Ok(cmds)
}

/// One job's command list from the CI workflow's marked region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiJob {
    pub name: String,
    pub commands: Vec<String>,
}

/// Extract the verify command list of every job that has a marked region.
pub fn ci_commands(yaml: &str) -> XResult<Vec<CiJob>> {
    let mut jobs = Vec::new();
    let mut errs = Vec::new();
    let mut current_job = String::from("<unknown>");
    let mut in_jobs = false;
    let mut region: Option<Vec<String>> = None;
    for (i, raw) in yaml.lines().enumerate() {
        let lineno = i + 1;
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if in_jobs && indent == 2 && trimmed.ends_with(':') && !trimmed.starts_with('#') {
            current_job = trimmed.trim_end_matches(':').to_string();
        }
        if trimmed == BEGIN {
            if region.is_some() {
                errs.push(format!("{CI_FILE}:{lineno}: nested `{BEGIN}` marker"));
            }
            region = Some(Vec::new());
            continue;
        }
        if trimmed == END {
            match region.take() {
                Some(commands) => jobs.push(CiJob {
                    name: current_job.clone(),
                    commands,
                }),
                None => errs.push(format!("{CI_FILE}:{lineno}: `{END}` without `{BEGIN}`")),
            }
            continue;
        }
        let Some(cmds) = region.as_mut() else {
            continue;
        };
        let body = trimmed.trim_start_matches("- ");
        if let Some(cmd) = body.strip_prefix("run:") {
            let cmd = cmd.trim();
            if cmd.starts_with('|') || cmd.starts_with('>') || cmd.is_empty() {
                errs.push(format!(
                    "{CI_FILE}:{lineno}: multi-line `run:` inside the verify region; one command \
                     per step so it can be compared with `just verify`"
                ));
            } else {
                cmds.push(normalise(cmd));
            }
        } else if body.starts_with("if:") {
            errs.push(format!(
                "{CI_FILE}:{lineno}: a step `if:` inside the verify region makes a CI step that \
                 does not always run; `just verify` always runs it"
            ));
        } else if body.starts_with("continue-on-error:") {
            errs.push(format!(
                "{CI_FILE}:{lineno}: `continue-on-error` inside the verify region makes a failing \
                 step non-gating"
            ));
        } else if body.starts_with("uses:") {
            errs.push(format!(
                "{CI_FILE}:{lineno}: an action (`uses:`) inside the verify region has no local \
                 twin; put setup steps before `{BEGIN}`"
            ));
        }
    }
    if region.is_some() {
        errs.push(format!("{CI_FILE}: `{BEGIN}` never closed"));
    }
    if jobs.is_empty() {
        errs.push(format!(
            "{CI_FILE}: no job has a `{BEGIN}` … `{END}` region — CI's gate list is unreadable"
        ));
    }
    Failure::many(errs)?;
    Ok(jobs)
}

/// The OS list of the CI matrix (`os: [a, b]`).
pub fn ci_matrix_os(yaml: &str) -> Vec<String> {
    yaml.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("os:"))
        .map(|rest| {
            rest.trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn normalise(cmd: &str) -> String {
    cmd.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The comparison itself. Pure, so the positive controls can feed it mismatched lists.
pub fn compare(local: &[String], remote: &CiJob) -> XResult<()> {
    if local == remote.commands.as_slice() {
        return Ok(());
    }
    let mut msg = format!(
        "I12 violated: `just verify` and CI job `{}` differ (W4: verify must be CI's list in \
         CI's order)\n",
        remote.name
    );
    let n = local.len().max(remote.commands.len());
    for i in 0..n {
        let l = local.get(i).map_or("<missing>", String::as_str);
        let r = remote.commands.get(i).map_or("<missing>", String::as_str);
        let mark = if l == r { "  " } else { "!=" };
        msg.push_str(&format!(
            "   {mark} {:>2}  just: {l}\n            ci:   {r}\n",
            i + 1
        ));
    }
    Err(Failure::one(msg))
}

pub fn check_matrix(os: &[String]) -> XResult<()> {
    let mut errs = Vec::new();
    for need in ["windows-latest", "ubuntu-latest"] {
        if !os.iter().any(|o| o == need) {
            errs.push(format!(
                "{CI_FILE}: matrix is missing {need} — Windows and Linux are co-primary (I18)"
            ));
        }
    }
    for o in os {
        if o.starts_with("macos") {
            errs.push(format!(
                "{CI_FILE}: matrix includes {o}; macOS is out of scope (E-33, D-2)"
            ));
        }
    }
    Failure::many(errs)
}

pub fn check_sources(justfile: &str, yaml: &str) -> XResult<usize> {
    let local = recipe_commands(justfile, "verify")?;
    let jobs = ci_commands(yaml)?;
    check_matrix(&ci_matrix_os(yaml))?;
    let mut errs = Vec::new();
    for job in &jobs {
        if let Err(e) = compare(&local, job) {
            errs.extend(e.messages);
        }
    }
    Failure::many(errs)?;
    Ok(local.len())
}

pub fn run(root: &Path) -> XResult<String> {
    let justfile = read(&root.join(JUSTFILE))?;
    let yaml = read(&root.join(CI_FILE))?;
    let n = check_sources(&justfile, &yaml)?;
    Ok(format!(
        "gate-parity: `just verify` == {CI_FILE} ({n} commands, same order) on {}\n",
        ci_matrix_os(&yaml).join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::workspace_root;

    const JUST: &str = "\
# a comment
check:
    cargo test

# CI's list
verify:
    cargo fmt --all -- --check
    @cargo   clippy -- -D warnings
    cargo test

gate:
    cargo xtask gate
";

    fn ci(steps: &str) -> String {
        format!(
            "name: ci\non: [push]\njobs:\n  verify:\n    strategy:\n      matrix:\n        os: [windows-latest, ubuntu-latest]\n    runs-on: ${{{{ matrix.os }}}}\n    steps:\n      - uses: actions/checkout@v4\n      {BEGIN}\n{steps}      {END}\n"
        )
    }

    const GOOD_STEPS: &str = "\
      - name: fmt
        run: cargo fmt --all -- --check
      - name: clippy
        run: cargo clippy -- -D warnings
      - run: cargo test
";

    #[test]
    fn recipe_parsing_strips_prefixes_and_stops_at_next_recipe() {
        let c = recipe_commands(JUST, "verify").unwrap();
        assert_eq!(
            c,
            vec![
                "cargo fmt --all -- --check",
                "cargo clippy -- -D warnings",
                "cargo test"
            ]
        );
    }

    #[test]
    fn matching_lists_pass() {
        assert_eq!(check_sources(JUST, &ci(GOOD_STEPS)).unwrap(), 3);
    }

    #[test]
    fn the_committed_justfile_and_ci_workflow_agree() {
        run(&workspace_root()).expect("just verify must equal CI's list (I12)");
    }

    // ---- positive controls (W2): every way the lists can drift must fail ----

    #[test]
    fn positive_control_mismatched_lists_fail() {
        // CI runs a step `just verify` does not: the exact W4 failure (a formatter check that
        // ran remotely and not locally).
        let extra = format!("{GOOD_STEPS}      - run: cargo deny check\n");
        let err = check_sources(JUST, &ci(&extra)).unwrap_err();
        assert!(err.to_string().contains("I12 violated"), "{err}");
        assert!(err.to_string().contains("cargo deny check"), "{err}");

        // Local runs a step CI does not.
        let missing = "      - run: cargo fmt --all -- --check\n      - run: cargo test\n";
        assert!(check_sources(JUST, &ci(missing)).is_err());

        // Same commands, different order.
        let reordered = "      - run: cargo test\n      - run: cargo fmt --all -- --check\n      - run: cargo clippy -- -D warnings\n";
        assert!(check_sources(JUST, &ci(reordered)).is_err());

        // A changed flag.
        let changed = GOOD_STEPS.replace("-D warnings", "-W warnings");
        assert!(check_sources(JUST, &ci(&changed)).is_err());
    }

    #[test]
    fn positive_control_hidden_ci_behaviour_fails() {
        let cond = GOOD_STEPS.replace(
            "      - run: cargo test\n",
            "      - run: cargo test\n        if: runner.os == 'Linux'\n",
        );
        assert!(ci_commands(&ci(&cond)).is_err(), "if: must be rejected");
        let coe = GOOD_STEPS.replace(
            "      - run: cargo test\n",
            "      - run: cargo test\n        continue-on-error: true\n",
        );
        assert!(
            ci_commands(&ci(&coe)).is_err(),
            "continue-on-error must be rejected"
        );
        let multi = "      - run: |\n          cargo test\n";
        assert!(ci_commands(&ci(multi)).is_err(), "run: | must be rejected");
        let unmarked = ci(GOOD_STEPS).replace(BEGIN, "").replace(END, "");
        assert!(
            ci_commands(&unmarked).is_err(),
            "a CI file with no region must fail"
        );
    }

    #[test]
    fn positive_control_matrix_must_be_windows_and_linux_only() {
        let mac = ci(GOOD_STEPS).replace(
            "[windows-latest, ubuntu-latest]",
            "[windows-latest, ubuntu-latest, macos-latest]",
        );
        assert!(check_sources(JUST, &mac).is_err());
        let no_linux =
            ci(GOOD_STEPS).replace("[windows-latest, ubuntu-latest]", "[windows-latest]");
        assert!(check_sources(JUST, &no_linux).is_err());
    }

    #[test]
    fn positive_control_missing_verify_recipe_fails() {
        assert!(recipe_commands("check:\n    cargo test\n", "verify").is_err());
    }
}

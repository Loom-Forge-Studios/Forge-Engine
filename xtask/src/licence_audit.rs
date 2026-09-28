//! `cargo xtask licence-audit` — invariant I13: permissive dependencies only, and a generated,
//! fresh `NOTICES` file (Ch.38.5, Appendix A.5).
//!
//! Three layers, each catching what the one before cannot:
//!
//! 1. **The allow-list itself is audited.** `deny.toml`'s `[licenses] allow` (and every
//!    per-crate exception) and `about.toml`'s `accepted` may contain only licences from
//!    [`PERMISSIVE`]. Widening the list to admit a copyleft licence fails here, before
//!    cargo-deny ever runs — otherwise the guard could be defeated by editing its own input.
//! 2. **`cargo deny check licenses bans sources`** checks the resolved graph against that
//!    list: no copyleft of any strength at any depth, no unknown registries or git sources.
//! 3. **`cargo about generate`** renders `NOTICES` from the resolved graph; `--check` (the
//!    default, used by `just verify`) fails if the committed file is stale, `--write`
//!    regenerates it (`just notices`).
//!
//! Security advisories (`cargo deny check advisories`) need a network fetch of the advisory
//! database, so they run in `just audit`, not in the gate (ADR 0002).

use std::path::Path;
use std::process::Command;

use crate::util::{Failure, XResult, read};

pub const DENY_FILE: &str = "deny.toml";
pub const ABOUT_FILE: &str = "about.toml";
pub const ABOUT_TEMPLATE: &str = "about.hbs";
pub const NOTICES_FILE: &str = "NOTICES";

/// The permissive family the plan admits (A.5: MIT, Apache-2.0, BSD, Zlib, ISC; Ch.38 adds
/// the Unicode data licences). Nothing here imposes obligations beyond preserving notices.
pub const PERMISSIVE: &[&str] = &[
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "0BSD",
    "Zlib",
    "ISC",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    // D-8 / ADR 0006: Boost Software License 1.0: permissive, no copyleft, no binary
    // attribution requirement.
    "BSL-1.0",
];

/// Licence families that are copyleft of some strength. Named explicitly so the error says
/// *why* a licence is refused, not just that it is unknown.
const COPYLEFT_MARKERS: &[&str] = &[
    "GPL",
    "LGPL",
    "AGPL",
    "MPL",
    "EPL",
    "CDDL",
    "EUPL",
    "OSL",
    "CECILL",
    "SSPL",
    "CC-BY-SA",
    "RPL",
    "APSL",
    "SLEEPYCAT",
];

fn classify(licence: &str) -> Option<String> {
    let l = licence.trim();
    if PERMISSIVE.contains(&l) {
        return None;
    }
    let upper = l.to_ascii_uppercase();
    if COPYLEFT_MARKERS.iter().any(|m| upper.contains(m)) {
        Some(format!(
            "`{l}` is copyleft — no copyleft of any strength is permitted (I13, Appendix A.5)"
        ))
    } else {
        Some(format!(
            "`{l}` is not in the permissive allow-list; adding a licence family is a plan \
             amendment (Appendix A.5), not a config edit"
        ))
    }
}

/// Audit `deny.toml`: the allow-list and every exception must be permissive, copyleft must
/// be denied, and private (workspace) crates must be the only ones exempt.
pub fn audit_deny(src: &str) -> XResult<()> {
    let doc: toml::Value =
        toml::from_str(src).map_err(|e| Failure::one(format!("{DENY_FILE}: parse error: {e}")))?;
    let mut errs = Vec::new();
    let lic = doc.get("licenses");
    let allow: Vec<&str> = lic
        .and_then(|l| l.get("allow"))
        .and_then(toml::Value::as_array)
        .map(|a| a.iter().filter_map(toml::Value::as_str).collect())
        .unwrap_or_default();
    if allow.is_empty() {
        errs.push(format!("{DENY_FILE}: [licenses] allow is empty or missing"));
    }
    for l in &allow {
        if let Some(why) = classify(l) {
            errs.push(format!("{DENY_FILE}: [licenses] allow: {why}"));
        }
    }
    if let Some(exc) = lic
        .and_then(|l| l.get("exceptions"))
        .and_then(toml::Value::as_array)
    {
        for e in exc {
            let name = e
                .get("crate")
                .or_else(|| e.get("name"))
                .and_then(toml::Value::as_str)
                .unwrap_or("?");
            for l in e
                .get("allow")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
            {
                if let Some(why) = classify(l) {
                    errs.push(format!("{DENY_FILE}: exception for {name}: {why}"));
                }
            }
        }
    }
    if let Some(c) = lic
        .and_then(|l| l.get("confidence-threshold"))
        .and_then(toml::Value::as_float)
        && c < 0.9
    {
        errs.push(format!(
            "{DENY_FILE}: confidence-threshold {c} is below 0.9 — a fuzzy match can mislabel a \
             copyleft text as permissive"
        ));
    }
    Failure::many(errs)
}

/// Audit `about.toml`: `accepted` must be permissive only.
pub fn audit_about(src: &str) -> XResult<()> {
    let doc: toml::Value =
        toml::from_str(src).map_err(|e| Failure::one(format!("{ABOUT_FILE}: parse error: {e}")))?;
    let accepted: Vec<&str> = doc
        .get("accepted")
        .and_then(toml::Value::as_array)
        .map(|a| a.iter().filter_map(toml::Value::as_str).collect())
        .unwrap_or_default();
    let mut errs = Vec::new();
    if accepted.is_empty() {
        errs.push(format!("{ABOUT_FILE}: `accepted` is empty or missing"));
    }
    for l in accepted {
        if let Some(why) = classify(l) {
            errs.push(format!("{ABOUT_FILE}: accepted: {why}"));
        }
    }
    Failure::many(errs)
}

/// Line-ending-insensitive comparison of a generated NOTICES with the committed one.
pub fn notices_fresh(generated: &str, committed: &str) -> bool {
    let norm = |s: &str| s.replace("\r\n", "\n").trim_end().to_string();
    norm(generated) == norm(committed)
}

fn run_tool(root: &Path, program: &str, args: &[&str]) -> XResult<std::process::Output> {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| {
            Failure::one(format!(
                "cannot run `{program} {}`: {e} — install it with `cargo install --locked {}`",
                args.join(" "),
                if args.first() == Some(&"deny") {
                    "cargo-deny"
                } else {
                    "cargo-about"
                }
            ))
        })
}

/// Run cargo-deny's licence, ban and source checks on the workspace at `root`.
pub fn cargo_deny(root: &Path) -> XResult<()> {
    let out = run_tool(
        root,
        "cargo",
        &["deny", "--locked", "check", "licenses", "bans", "sources"],
    )?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Failure::one(format!(
            "cargo deny check failed (I13):\n{}",
            String::from_utf8_lossy(&out.stderr)
        )))
    }
}

/// Render NOTICES with cargo-about.
///
/// The output goes through `--output-file`, never captured stdout: cargo-about refuses to
/// write redirected stdout when any ancestor process is PowerShell (it guards against
/// PowerShell's re-encoding), so capturing stdout made `just verify` fail whenever it was
/// launched from a PowerShell terminal — the default shell on Windows.
pub fn generate_notices(root: &Path) -> XResult<String> {
    let out_file = std::env::temp_dir().join(format!(
        "forge-notices-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let out_arg = out_file.to_string_lossy().into_owned();
    let result = (|| {
        let out = run_tool(
            root,
            "cargo",
            &[
                "about",
                "generate",
                "--frozen",
                "--fail",
                "--threshold",
                "0.93",
                "--output-file",
                &out_arg,
                ABOUT_TEMPLATE,
            ],
        )?;
        if !out.status.success() {
            return Err(Failure::one(format!(
                "cargo about generate failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        let bytes = std::fs::read(&out_file).map_err(|e| {
            Failure::one(format!(
                "cargo about wrote no output file {}: {e}",
                out_file.display()
            ))
        })?;
        String::from_utf8(bytes)
            .map(|s| s.replace("\r\n", "\n"))
            .map_err(|e| Failure::one(format!("cargo about produced non-UTF-8 output: {e}")))
    })();
    // Best effort: the file lives in the OS temp dir, so a leftover is harmless.
    let _ = std::fs::remove_file(&out_file);
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Check,
    Write,
}

pub fn run(root: &Path, mode: Mode) -> XResult<String> {
    audit_deny(&read(&root.join(DENY_FILE))?)?;
    audit_about(&read(&root.join(ABOUT_FILE))?)?;
    cargo_deny(root)?;
    let generated = generate_notices(root)?;
    let path = root.join(NOTICES_FILE);
    match mode {
        Mode::Write => {
            std::fs::write(&path, &generated)
                .map_err(|e| Failure::one(format!("cannot write {NOTICES_FILE}: {e}")))?;
            Ok(format!(
                "licence-audit: cargo-deny green; {NOTICES_FILE} regenerated\n"
            ))
        }
        Mode::Check => {
            let committed = std::fs::read_to_string(&path).unwrap_or_default();
            if notices_fresh(&generated, &committed) {
                Ok(format!(
                    "licence-audit: allow-lists permissive, cargo-deny green, {NOTICES_FILE} fresh\n"
                ))
            } else {
                Err(Failure::one(format!(
                    "{NOTICES_FILE} is stale: it does not match the resolved dependency graph \
                     (Ch.38.5 — a distribution without current notices violates its \
                     dependencies' licences). Run `just notices` and commit the result."
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::workspace_root;

    #[test]
    fn committed_allow_lists_are_permissive() {
        let root = workspace_root();
        audit_deny(&read(&root.join(DENY_FILE)).unwrap()).unwrap();
        audit_about(&read(&root.join(ABOUT_FILE)).unwrap()).unwrap();
    }

    #[test]
    fn notices_comparison_ignores_line_endings_only() {
        assert!(notices_fresh("a\nb\n", "a\r\nb\r\n"));
        assert!(!notices_fresh("a\nb\n", "a\nc\n"));
    }

    // ---- positive controls ----

    #[test]
    fn positive_control_copyleft_in_allow_list_fails() {
        for bad in ["GPL-3.0-only", "LGPL-2.1-or-later", "AGPL-3.0", "MPL-2.0"] {
            let src = format!("[licenses]\nallow = [\"MIT\", \"{bad}\"]\n");
            let err = audit_deny(&src).unwrap_err();
            assert!(err.to_string().contains("copyleft"), "{bad}: {err}");
        }
        let src = "[licenses]\nallow = [\"MIT\"]\n[[licenses.exceptions]]\ncrate = \"x\"\nallow = [\"GPL-2.0\"]\n";
        assert!(audit_deny(src).is_err(), "a copyleft exception must fail");
        assert!(audit_about("accepted = [\"MIT\", \"EPL-2.0\"]\n").is_err());
    }

    #[test]
    fn positive_control_unknown_licence_and_low_confidence_fail() {
        assert!(audit_deny("[licenses]\nallow = [\"WTFPL\"]\n").is_err());
        assert!(audit_deny("[licenses]\nallow = [\"MIT\"]\nconfidence-threshold = 0.5\n").is_err());
        assert!(audit_deny("[licenses]\nallow = []\n").is_err());
    }

    #[test]
    fn positive_control_stale_notices_fails() {
        assert!(!notices_fresh(
            "crate-a 1.0 MIT\ncrate-b 2.0 MIT\n",
            "crate-a 1.0 MIT\n"
        ));
    }

    /// End-to-end positive control for the resolved-graph check: a throwaway workspace whose
    /// only dependency is a GPL-3.0 path crate must be REFUSED by cargo-deny running with the
    /// committed `deny.toml`, while the same workspace with an MIT crate passes. This proves
    /// the real tool, with the real config, rejects copyleft — not just that the config reads
    /// well. Fully offline: path dependencies only.
    #[test]
    fn positive_control_gpl_crate_fails_cargo_deny() {
        let deny = read(&workspace_root().join(DENY_FILE)).unwrap();
        let run_case = |licence: &str| -> XResult<()> {
            let dir = std::env::temp_dir().join(format!(
                "forge-deny-control-{}-{}",
                std::process::id(),
                licence.replace(['.', '-'], "_")
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("app/src")).unwrap();
            std::fs::create_dir_all(dir.join("dep/src")).unwrap();
            std::fs::write(
                dir.join("Cargo.toml"),
                "[workspace]\nresolver = \"3\"\nmembers = [\"app\"]\n",
            )
            .unwrap();
            std::fs::write(
                dir.join("app/Cargo.toml"),
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\
                 [dependencies]\ndep = { path = \"../dep\" }\n",
            )
            .unwrap();
            std::fs::write(dir.join("app/src/lib.rs"), "").unwrap();
            std::fs::write(
                dir.join("dep/Cargo.toml"),
                format!(
                    "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"{licence}\"\n"
                ),
            )
            .unwrap();
            std::fs::write(dir.join("dep/src/lib.rs"), "").unwrap();
            std::fs::write(dir.join("deny.toml"), &deny).unwrap();
            let lock = Command::new("cargo")
                .args(["generate-lockfile", "--offline"])
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                lock.status.success(),
                "{}",
                String::from_utf8_lossy(&lock.stderr)
            );
            let r = cargo_deny(&dir);
            let _ = std::fs::remove_dir_all(&dir);
            r
        };
        run_case("MIT").expect("an MIT dependency must pass cargo-deny (control of the control)");
        let err = run_case("GPL-3.0-only").expect_err("a GPL dependency must fail cargo-deny");
        assert!(err.to_string().contains("GPL-3.0"), "{err}");
    }
}

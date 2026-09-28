//! Open core (ADR 0059, E-66): the private repository is the source, and the public
//! repository is an export of it with the premium parts removed.
//!
//! * [`Manifest`] is `premium.ron` at the repository root: what is premium (crates, paths,
//!   cargo features, doc sections, DoD ids, gate rows) and the identifiers that must never
//!   appear in a public file.
//! * [`boundary`] is `cargo xtask premium-boundary`: no base crate depends on a premium
//!   crate and no base file names a premium identifier, except the known violations listed
//!   in `premium_debt.ron`, a list that can only shrink.
//! * [`export`] is `cargo xtask export-public <out-dir> [--docs-only]`: writes the public
//!   tree, cuts premium sections out of the docs ([`scrub`]), and leak-scans what it wrote.
//!
//! Nothing in this module names a premium identifier itself: every name comes from
//! `premium.ron`, so the tooling is exported with the base like any other xtask command.

pub mod boundary;
pub mod export;
pub mod protections;
pub mod scan;
pub mod scrub;

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use crate::util::{Failure, XResult, read};

pub const MANIFEST_FILE: &str = "premium.ron";
pub const DEBT_FILE: &str = "premium_debt.ron";

/// The work packages that move premium code across the boundary. Every premium item and
/// every debt entry names one of them.
pub const MOVING_WPS: [&str; 3] = ["WP-42", "WP-43", "WP-44"];

/// The owner of a premium path that no work package moves because it simply stays in the
/// private repository (a premium ADR, an evidence note, the open-core records themselves).
pub const PRIVATE: &str = "private";

/// `premium.ron`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// Premium workspace members: never exported, and no base member may depend on one.
    pub crates: Vec<PremiumCrate>,
    /// Names reserved for premium crates that do not exist yet: identifiers already.
    #[serde(default)]
    pub reserved_crates: Vec<String>,
    /// Premium files and directories outside the premium crates (a module lodged in a base
    /// crate, a preset, a premium ADR). Never exported; a base file naming one is a leak.
    #[serde(default)]
    pub paths: Vec<PremiumPath>,
    /// Files the export leaves out that are not premium names: records of the full build a
    /// base test writes (the base edition regenerates its own). Never exported; a base file
    /// may name one.
    #[serde(default)]
    pub withheld: Vec<Withheld>,
    /// Premium cargo features: `(crate, feature)`. A base member never declares one.
    #[serde(default)]
    pub features: Vec<PremiumFeature>,
    /// Identifiers that must never appear in any exported file (case-sensitive, whole word).
    /// The crate names (both spellings) and every premium path are added to these.
    #[serde(default)]
    pub identifiers: Vec<Identifier>,
    /// Words that must never appear in exported prose (case-insensitive, word prefix). They
    /// are for the docs only: a sentence, table row or list item holding one is cut.
    #[serde(default)]
    pub doc_terms: Vec<String>,
    /// Words that must never appear in a base file that is not prose (code, manifests, data;
    /// case-insensitive, word prefix, matched like the doc terms). Premium code in a base
    /// crate is caught by these even when it names no listed identifier (WP-44): a file that
    /// talks about a premium thing is premium, whatever its identifiers are called.
    #[serde(default)]
    pub code_terms: Vec<String>,
    /// Exact, case-sensitive spellings of base names that hold a code term word but are not
    /// the premium thing: the seed algebra's root, `SeedPath::universe()`, and its path text
    /// `universe/...` (base, ADR 0062). They are blanked out before the code terms are
    /// matched; the hard identifiers still see them.
    #[serde(default)]
    pub code_term_exempt: Vec<String>,
    /// Test binaries of premium crates that the nextest configuration names (its serial
    /// groups): the export drops their `binary(..)` clauses from `.config/nextest.toml`.
    #[serde(default)]
    pub nextest_binaries: Vec<String>,
    /// Sections cut from the docs: `<!-- premium:begin <id> -->` ... `<!-- premium:end <id> -->`.
    #[serde(default)]
    pub doc_sections: Vec<DocSection>,
    /// Premium DoD ids, grouped by the work package that owns them: their rows leave
    /// milestones.md and dod-status.ron, and a citation of one (or of a range spanning one,
    /// `M7-35..M7-39`) is a premium name.
    #[serde(default)]
    pub dod_ids: Vec<IdGroup>,
    /// Premium gate rows, grouped by work package: their rows leave tests/gates.ron, and
    /// the id itself is a premium name everywhere else.
    #[serde(default)]
    pub gate_rows: Vec<IdGroup>,
    /// Premium error codes (`ERR-0006`), grouped by work package: their rows leave
    /// docs/error-codes.md, and the code is a premium name everywhere else.
    #[serde(default)]
    pub error_codes: Vec<IdGroup>,
    /// Exact text replacements applied to a doc before it is scrubbed (a base heading or
    /// sentence that names a premium thing in passing, restated for the base edition).
    #[serde(default)]
    pub rewrites: Vec<Rewrite>,
    /// Files written into the export in place of the private ones (README, CONTRIBUTING).
    #[serde(default)]
    pub public_files: Vec<PublicFile>,
    /// What a `--docs-only` export contains: top-level paths (files or directories).
    #[serde(default)]
    pub docs_only: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PremiumCrate {
    pub name: String,
    pub path: String,
    pub wp: String,
    pub what: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PremiumPath {
    pub path: String,
    pub wp: String,
    pub why: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Withheld {
    pub path: String,
    pub why: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PremiumFeature {
    pub krate: String,
    pub feature: String,
    pub why: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Identifier {
    pub text: String,
    pub wp: String,
}

/// Ids owned by one work package (`(wp: "WP-43", ids: ["C-a", "C-b"])`).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct IdGroup {
    pub wp: String,
    pub ids: Vec<String>,
}

fn flatten(groups: &[IdGroup]) -> Vec<String> {
    groups.iter().flat_map(|g| g.ids.iter().cloned()).collect()
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DocSection {
    pub file: String,
    pub id: String,
    pub why: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Rewrite {
    pub file: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PublicFile {
    pub from: String,
    pub to: String,
}

impl Manifest {
    pub fn parse(src: &str) -> XResult<Self> {
        ron::from_str(src).map_err(|e| Failure::one(format!("{MANIFEST_FILE}: parse error: {e}")))
    }

    pub fn load(root: &Path) -> XResult<Option<Self>> {
        let p = root.join(MANIFEST_FILE);
        if !p.is_file() {
            return Ok(None);
        }
        Self::parse(&read(&p)?).map(Some)
    }

    /// Every premium crate name, both spellings (`a-b` and `a_b`), reserved names included.
    pub fn crate_names(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for n in self
            .crates
            .iter()
            .map(|c| c.name.as_str())
            .chain(self.reserved_crates.iter().map(String::as_str))
        {
            out.insert(n.to_string());
            out.insert(n.replace('-', "_"));
        }
        out
    }

    /// Is `name` (a package name) premium?
    pub fn is_premium_crate(&self, name: &str) -> bool {
        self.crates.iter().any(|c| c.name == name) || self.reserved_crates.iter().any(|c| c == name)
    }

    /// Every premium DoD id.
    pub fn dod_list(&self) -> Vec<String> {
        flatten(&self.dod_ids)
    }

    /// Every premium gate row id.
    pub fn gate_list(&self) -> Vec<String> {
        flatten(&self.gate_rows)
    }

    /// Every premium error code.
    pub fn error_code_list(&self) -> Vec<String> {
        flatten(&self.error_codes)
    }

    /// The work package that owns a premium DoD id, gate row or error code.
    pub fn id_wp(&self, id: &str) -> Option<&str> {
        self.dod_ids
            .iter()
            .chain(&self.gate_rows)
            .chain(&self.error_codes)
            .find(|g| g.ids.iter().any(|i| i == id))
            .map(|g| g.wp.as_str())
    }

    /// The hard identifiers, less the DoD ids: the crate names (both spellings), the listed
    /// identifiers, the premium paths, the premium gate rows and error codes. The doc scrub
    /// matches these; DoD ids it handles as citations (a heading keeps its text without one).
    pub fn names_without_dod(&self) -> Vec<String> {
        let mut set: BTreeSet<String> = self.crate_names();
        set.extend(self.identifiers.iter().map(|i| i.text.clone()));
        set.extend(self.paths.iter().map(|p| p.path.clone()));
        set.extend(self.gate_list());
        set.extend(self.error_code_list());
        set.into_iter().collect()
    }

    /// The matcher for a base file that is not prose: the hard identifiers and the code
    /// terms.
    #[must_use]
    pub fn code_matcher(&self) -> scan::Matcher {
        scan::Matcher::new(self.hard_identifiers(), self.code_terms.clone())
            .exempting(self.code_term_exempt.clone())
    }

    /// The hard identifiers: never in any exported file, in any file type. Everything in
    /// [`Self::names_without_dod`] and the premium DoD ids.
    pub fn hard_identifiers(&self) -> Vec<String> {
        let mut set: BTreeSet<String> = self.names_without_dod().into_iter().collect();
        set.extend(self.dod_list());
        set.into_iter().collect()
    }

    /// Paths that never leave the private repository: premium crates, premium paths, the
    /// manifest and debt files, and the private files the public ones replace.
    pub fn excluded(&self, rel: &str) -> bool {
        let under = |p: &str| {
            let p = p.trim_end_matches('/');
            rel == p || rel.starts_with(&format!("{p}/"))
        };
        self.crates.iter().any(|c| under(&c.path))
            || self.paths.iter().any(|p| under(&p.path))
            || self.withheld.iter().any(|w| under(&w.path))
            || rel == MANIFEST_FILE
            || rel == DEBT_FILE
            || self
                .public_files
                .iter()
                .any(|f| rel == f.from || rel == f.to)
    }

    /// Structural checks on the manifest itself (the parts that need no other file).
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let wp_ok = |wp: &str| MOVING_WPS.contains(&wp);
        let mut names = BTreeSet::new();
        for c in &self.crates {
            if !names.insert(c.name.clone()) {
                errs.push(format!("{MANIFEST_FILE}: crate {} is listed twice", c.name));
            }
            if !wp_ok(&c.wp) {
                errs.push(format!(
                    "{MANIFEST_FILE}: crate {} names {}, not one of {MOVING_WPS:?}",
                    c.name, c.wp
                ));
            }
        }
        for p in &self.paths {
            if !wp_ok(&p.wp) && p.wp != PRIVATE {
                errs.push(format!(
                    "{MANIFEST_FILE}: path {} names {}, not one of {MOVING_WPS:?} or \"{PRIVATE}\"",
                    p.path, p.wp
                ));
            }
        }
        for i in &self.identifiers {
            if !wp_ok(&i.wp) {
                errs.push(format!(
                    "{MANIFEST_FILE}: identifier {:?} names {}, not one of {MOVING_WPS:?}",
                    i.text, i.wp
                ));
            }
            if i.text.trim().len() < 3 {
                errs.push(format!(
                    "{MANIFEST_FILE}: identifier {:?} is too short to scan for without false hits",
                    i.text
                ));
            }
        }
        for (what, groups) in [
            ("dod_ids", &self.dod_ids),
            ("gate_rows", &self.gate_rows),
            ("error_codes", &self.error_codes),
        ] {
            let mut seen = BTreeSet::new();
            for g in groups {
                if !wp_ok(&g.wp) {
                    errs.push(format!(
                        "{MANIFEST_FILE}: a {what} group names {}, not one of {MOVING_WPS:?}",
                        g.wp
                    ));
                }
                for id in &g.ids {
                    if !seen.insert(id) {
                        errs.push(format!("{MANIFEST_FILE}: {what} lists {id} twice"));
                    }
                }
            }
        }
        for t in &self.doc_terms {
            if t.trim().len() < 4 || t.to_lowercase() != *t {
                errs.push(format!(
                    "{MANIFEST_FILE}: doc term {t:?} must be lowercase and at least 4 characters"
                ));
            }
        }
        for t in &self.code_terms {
            if t.trim().len() < 4 || t.to_lowercase() != *t {
                errs.push(format!(
                    "{MANIFEST_FILE}: code term {t:?} must be lowercase and at least 4 characters"
                ));
            }
        }
        let mut ids = BTreeSet::new();
        for s in &self.doc_sections {
            if !ids.insert(s.id.clone()) {
                errs.push(format!(
                    "{MANIFEST_FILE}: doc section {} is listed twice",
                    s.id
                ));
            }
            if !s
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                errs.push(format!(
                    "{MANIFEST_FILE}: doc section id {:?} must be lowercase ascii, digits and '-'",
                    s.id
                ));
            }
        }
        errs
    }
}

/// The begin/end markers of a premium doc section.
pub fn section_markers(id: &str) -> (String, String) {
    (
        format!("<!-- premium:begin {id} -->"),
        format!("<!-- premium:end {id} -->"),
    )
}

/// Every `premium:begin` marker id in `text`, in order.
pub fn marker_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("<!-- premium:begin ")?
                .strip_suffix("-->")
                .map(|s| s.trim().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest::parse(
            r#"(
                crates: [(name: "forge-secret", path: "crates/forge-secret", wp: "WP-44", what: "a premium crate")],
                reserved_crates: ["forge-later"],
                paths: [(path: "presets/hidden", wp: "WP-44", why: "a premium preset")],
                withheld: [(path: "docs/record.md", why: "a record of the full build")],
                identifiers: [(text: "SecretTree", wp: "WP-43")],
                doc_terms: ["hush"],
                public_files: [(from: "README.public.md", to: "README.md")],
            )"#,
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn identifiers_cover_both_crate_spellings_paths_and_reserved_names() {
        let ids = manifest().hard_identifiers();
        for want in [
            "forge-secret",
            "forge_secret",
            "forge-later",
            "forge_later",
            "SecretTree",
            "presets/hidden",
        ] {
            assert!(ids.iter().any(|i| i == want), "{want} missing from {ids:?}");
        }
    }

    #[test]
    fn exclusion_covers_crates_paths_and_the_private_files() {
        let m = manifest();
        assert!(m.excluded("crates/forge-secret/src/lib.rs"));
        assert!(m.excluded("crates/forge-secret"));
        assert!(!m.excluded("crates/forge-secretive/src/lib.rs"));
        assert!(m.excluded("presets/hidden/workspace.ron"));
        assert!(m.excluded("docs/record.md"));
        assert!(!m.hard_identifiers().iter().any(|i| i == "docs/record.md"));
        assert!(m.excluded("premium.ron") && m.excluded("premium_debt.ron"));
        assert!(m.excluded("README.md") && m.excluded("README.public.md"));
        assert!(!m.excluded("crates/forge-num/src/lib.rs"));
    }

    #[test]
    fn positive_control_a_bad_manifest_is_rejected() {
        assert!(manifest().validate().is_empty());
        let mut m = manifest();
        m.crates[0].wp = "WP-99".into();
        m.identifiers[0].text = "ab".into();
        m.doc_terms.push("Upper".into());
        assert_eq!(m.validate().len(), 3, "{:?}", m.validate());
    }

    #[test]
    fn markers_are_found() {
        let (b, e) = section_markers("ch-9");
        let text = format!("x\n{b}\nsecret\n{e}\ny\n");
        assert_eq!(marker_ids(&text), vec!["ch-9".to_string()]);
    }
}

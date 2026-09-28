//! `cargo xtask allocators` — W8 and Ch.1.2: defect numbers and error codes each have exactly
//! one allocator, and an allocator never hands out the same number twice.
//!
//! * `docs/defects.md` allocates `D-NNNN`. Ids are unique and strictly increasing in
//!   document order (append-only, so a reused or back-filled number is visible).
//! * `docs/error-codes.md` allocates `<PREFIX>-NNNN`, one prefix per crate. Every prefix
//!   must be registered in the prefix table; codes are unique and strictly increasing within
//!   a prefix. Codes are never reused, so a retired code stays in the table marked retired.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::util::{Failure, XResult, read};

pub const DEFECTS_FILE: &str = "docs/defects.md";
pub const ERROR_CODES_FILE: &str = "docs/error-codes.md";

/// First cells of every markdown table row.
fn first_cells(md: &str) -> impl Iterator<Item = (usize, &str)> {
    md.lines().enumerate().filter_map(|(i, l)| {
        let cell = l.trim().strip_prefix('|')?.split('|').next()?.trim();
        Some((i + 1, cell.trim_matches('`')))
    })
}

fn split_code(s: &str) -> Option<(&str, u32)> {
    let (p, n) = s.rsplit_once('-')?;
    if p.is_empty() || n.len() != 4 || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !p
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return None;
    }
    n.parse().ok().map(|v| (p, v))
}

pub fn check_defects(md: &str) -> XResult<usize> {
    let mut errs = Vec::new();
    let mut last = 0u32;
    let mut seen = BTreeSet::new();
    let mut count = 0;
    for (line, cell) in first_cells(md) {
        let Some(("D", n)) = split_code(cell) else {
            continue;
        };
        count += 1;
        if !seen.insert(n) {
            errs.push(format!(
                "{DEFECTS_FILE}:{line}: D-{n:04} allocated twice (W8)"
            ));
        } else if n <= last {
            errs.push(format!(
                "{DEFECTS_FILE}:{line}: D-{n:04} follows D-{last:04} — the allocator is append-only"
            ));
        }
        last = last.max(n);
    }
    Failure::many(errs)?;
    Ok(count)
}

/// The registered prefixes: rows of the table under the `## Prefixes` heading, whose first
/// cell is the prefix and second cell the crate, in document order. A `Vec`, not a map: a
/// prefix claimed twice must stay visible so `check_error_codes` can reject it (a map would
/// silently keep the last claim and hand one prefix to two crates).
fn prefixes(md: &str) -> Vec<(String, String)> {
    let Some(start) = md.find("## Prefixes") else {
        return Vec::new();
    };
    let section = &md[start..];
    let end = section[3..].find("\n## ").map_or(section.len(), |e| e + 3);
    section[..end]
        .lines()
        .filter_map(|l| {
            let mut cells = l.trim().strip_prefix('|')?.split('|').map(str::trim);
            let p = cells.next()?.trim_matches('`');
            let c = cells.next()?.trim_matches('`');
            (!p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
            .then(|| (p.to_string(), c.to_string()))
        })
        .collect()
}

pub fn check_error_codes(md: &str) -> XResult<usize> {
    let table = prefixes(md);
    let mut errs = Vec::new();
    if table.is_empty() {
        errs.push(format!("{ERROR_CODES_FILE}: no `## Prefixes` table"));
    }
    let mut crates = BTreeSet::new();
    let mut registered: BTreeMap<String, String> = BTreeMap::new();
    for (p, c) in &table {
        if !crates.insert(c.clone()) {
            errs.push(format!(
                "{ERROR_CODES_FILE}: crate {c} has two prefixes (second: {p}) — one per crate"
            ));
        }
        if let Some(first) = registered.get(p) {
            errs.push(format!(
                "{ERROR_CODES_FILE}: prefix {p} is claimed by two crates ({first} and {c}) — \
                 one crate per prefix, or two crates allocate the same codes"
            ));
        } else {
            registered.insert(p.clone(), c.clone());
        }
    }
    let mut last: BTreeMap<&str, u32> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut count = 0;
    for (line, cell) in first_cells(md) {
        let Some((p, n)) = split_code(cell) else {
            continue;
        };
        count += 1;
        if !registered.contains_key(p) {
            errs.push(format!(
                "{ERROR_CODES_FILE}:{line}: {cell} uses unregistered prefix {p} — register it in \
                 the Prefixes table first"
            ));
        }
        if !seen.insert(cell.to_string()) {
            errs.push(format!(
                "{ERROR_CODES_FILE}:{line}: {cell} allocated twice — codes are never reused (Ch.1.2)"
            ));
        } else if last.get(p).is_some_and(|&l| n <= l) {
            errs.push(format!(
                "{ERROR_CODES_FILE}:{line}: {cell} is out of order — the allocator is append-only"
            ));
        }
        let e = last.entry(p).or_insert(0);
        *e = (*e).max(n);
    }
    Failure::many(errs)?;
    Ok(count)
}

pub fn run(root: &Path) -> XResult<String> {
    let d = check_defects(&read(&root.join(DEFECTS_FILE))?)?;
    let e = check_error_codes(&read(&root.join(ERROR_CODES_FILE))?)?;
    Ok(format!(
        "allocators: {d} defects, {e} error codes — no duplicates, append-only (W8)\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::workspace_root;

    const CODES: &str = "\
## Prefixes
| Prefix | Crate |
|---|---|
| CORE | forge-core |
| CMD | forge-cmd |

## Codes
| Code | Meaning |
|---|---|
| CORE-0001 | a |
| CORE-0002 | b |
| CMD-0001 | c |
";

    #[test]
    fn committed_allocators_are_valid() {
        run(&workspace_root()).unwrap();
    }

    #[test]
    fn a_valid_code_table_passes() {
        assert_eq!(check_error_codes(CODES).unwrap(), 3);
        assert_eq!(
            check_defects("| Id | x |\n|---|---|\n| D-0001 | a |\n| D-0002 | b |\n").unwrap(),
            2
        );
    }

    // ---- positive controls ----

    #[test]
    fn positive_control_duplicate_defect_fails() {
        let md = "| D-0001 | a |\n| D-0002 | b |\n| D-0002 | c |\n";
        assert!(
            check_defects(md)
                .unwrap_err()
                .to_string()
                .contains("allocated twice")
        );
        let md = "| D-0003 | a |\n| D-0002 | b |\n";
        assert!(
            check_defects(md)
                .unwrap_err()
                .to_string()
                .contains("append-only")
        );
    }

    #[test]
    fn positive_control_duplicate_or_unregistered_code_fails() {
        let dup = format!("{CODES}| CORE-0002 | again |\n");
        assert!(
            check_error_codes(&dup)
                .unwrap_err()
                .to_string()
                .contains("allocated twice")
        );
        let unreg = format!("{CODES}| GPU-0001 | x |\n");
        assert!(
            check_error_codes(&unreg)
                .unwrap_err()
                .to_string()
                .contains("unregistered prefix GPU")
        );
        let two = CODES.replace("| CMD | forge-cmd |", "| CMD | forge-core |");
        assert!(
            check_error_codes(&two)
                .unwrap_err()
                .to_string()
                .contains("two prefixes")
        );
    }

    #[test]
    fn positive_control_prefix_claimed_by_two_crates_fails() {
        let two = CODES.replace(
            "| CMD | forge-cmd |",
            "| CMD | forge-cmd |\n| CORE | forge-seed |",
        );
        let err = check_error_codes(&two).unwrap_err().to_string();
        assert!(
            err.contains("prefix CORE is claimed by two crates (forge-core and forge-seed)"),
            "{err}"
        );
        // Not vacuous: the same table without the second claim passes.
        assert!(check_error_codes(CODES).is_ok());
    }
}

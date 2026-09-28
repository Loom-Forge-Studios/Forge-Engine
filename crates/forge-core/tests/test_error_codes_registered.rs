//! Every code forge-core can put in an `Error` is allocated in `docs/error-codes.md` under the
//! crate that owns it (Ch.1.2, W8) — so a log line always resolves to a meaning.

use std::collections::BTreeMap;

use forge_core::{CodedError, CoreError, ErrorCode};
use forge_frames::{FrameError, FrameId};
use forge_seed::SeedPathError;

/// code -> crate, from the `## Codes` table.
fn registry() -> BTreeMap<String, String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/error-codes.md");
    let md = std::fs::read_to_string(path).expect("docs/error-codes.md");
    md.lines()
        .filter_map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            // | code | crate | variant | meaning |
            (cells.len() >= 5 && looks_like_code(cells[1]))
                .then(|| (cells[1].to_string(), cells[2].to_string()))
        })
        .collect()
}

/// Same shape rule as `ErrorCode::parse`, for a non-static table cell.
fn looks_like_code(s: &str) -> bool {
    let Some((p, n)) = s.rsplit_once('-') else {
        return false;
    };
    !p.is_empty()
        && p.bytes().all(|b| b.is_ascii_uppercase())
        && n.len() == 4
        && n.bytes().all(|b| b.is_ascii_digit())
}

fn emitted() -> Vec<(ErrorCode, &'static str)> {
    let mut v: Vec<(ErrorCode, &'static str)> = CoreError::all_variants_for_tests()
        .iter()
        .map(|e| (e.code(), "forge-core"))
        .collect();
    let f = FrameId(0);
    for e in [
        FrameError::UnknownFrame(f),
        FrameError::Disconnected { from: f, to: f },
    ] {
        v.push((e.error_code(), "forge-frames"));
    }
    for e in [
        SeedPathError::MissingRoot,
        SeedPathError::BadSegment(String::new()),
    ] {
        v.push((e.error_code(), "forge-seed"));
    }
    v.push((ErrorCode::new("CORE-0013"), "forge-core"));
    v
}

fn unregistered(reg: &BTreeMap<String, String>, codes: &[(ErrorCode, &str)]) -> Vec<String> {
    codes
        .iter()
        .filter(|(c, krate)| reg.get(c.as_str()).map(String::as_str) != Some(*krate))
        .map(|(c, krate)| format!("{c} ({krate})"))
        .collect()
}

#[test]
fn every_emitted_code_is_registered_to_its_crate() {
    let reg = registry();
    let missing = unregistered(&reg, &emitted());
    assert!(
        missing.is_empty(),
        "not in docs/error-codes.md: {missing:?}"
    );
}

/// Positive control (W2): an unallocated code, and a real code claimed by the wrong crate,
/// are both reported.
#[test]
fn positive_control_an_unregistered_code_is_reported() {
    let reg = registry();
    let bad = [
        (ErrorCode::new("CORE-9999"), "forge-core"),
        (ErrorCode::new("CORE-0001"), "forge-frames"),
    ];
    assert_eq!(unregistered(&reg, &bad).len(), 2);
}

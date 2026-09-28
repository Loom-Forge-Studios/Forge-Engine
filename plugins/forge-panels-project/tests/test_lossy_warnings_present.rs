//! `test_lossy_warnings_present` (Ch.21 §21.23, Ch.31 §31.4; gate row
//! `C-lossy-warnings-present`; DoD M2-48): every lossy promotion arrow **shows a
//! confirmation naming the loss** before anything changes — the project's state hash is
//! identical until the user confirms — and every lossless arrow is one click with no
//! confirmation.
//!
//! The lossy arrow, on a project that has something to lose: 3D → 2D (the 3D template's
//! mesh and light). Lossless: 2D → 3D. (A plugin's preset and its arrows are guarded where
//! the plugin is, with the same switcher.)
//!
//! Positive control (W2): `positive_control_a_path_without_its_warning_fails` — the
//! preset switcher sends a lossy change without asking
//! (`PanelFaults::lossy_warning_skipped`): the guard names every path that lost data
//! without a warning.

mod common;

use forge_cmd::Value;
use forge_editor::presets::Family;
use forge_editor::project::{ProjectOp, Template};
use forge_editor::services::PanelFaults;
use forge_editor::testing::Rig;

const P: &str = "forge.presets";

fn fresh(template: Template, fault: bool, tag: &str) -> Rig {
    let dir = common::tmp(&format!("lossy-{tag}"));
    let mut rig = common::rig_in(
        &[P],
        &dir,
        PanelFaults {
            lossy_warning_skipped: fault,
            ..PanelFaults::default()
        },
        |_| {},
    );
    rig.shell.emitter().emit(
        ProjectOp::Create {
            location: format!("memory:lossy-{tag}-{fault}-{}", std::process::id()),
            name: "Lossy".into(),
            template,
            discard_unsaved: false,
        }
        .command(),
    );
    rig.settle();
    rig
}

fn family(rig: &Rig) -> String {
    match common::setting(rig, "project.preset") {
        Some(Value::Text(t)) => t,
        _ => String::new(),
    }
}

/// Pick `to` in the preset switcher and press Switch. Returns the confirmation text, if
/// one was shown.
fn switch(rig: &mut Rig, to: Family) -> Option<String> {
    let list = common::part(rig, P, &["content", "presets"]);
    let i = Family::ALL.iter().position(|f| *f == to).unwrap_or(0) as u64;
    common::select_row(rig, list, i);
    let promote = common::part(rig, P, &["content", "detail", "detail_bar", "promote"]);
    common::click(rig, promote);
    let confirm = common::part(rig, P, &["content", "confirm"]);
    common::visible(rig, confirm).then(|| {
        let t = common::part(rig, P, &["content", "confirm", "confirm_text"]);
        common::label(rig, t)
    })
}

/// One lossy arrow: a confirmation naming `names` appears and nothing changes until it is
/// accepted; then the switch happens. Failures are pushed to `bad`.
fn lossy(rig: &mut Rig, to: Family, names: &[&str], path: &str, bad: &mut Vec<String>) {
    let before = rig.state_hash();
    let text = switch(rig, to);
    match &text {
        None => bad.push(format!("{path}: no confirmation shown")),
        Some(t) => {
            for n in names {
                if !t.contains(n) {
                    bad.push(format!("{path}: the warning does not name {n}: {t:?}"));
                }
            }
        }
    }
    if rig.state_hash() != before {
        bad.push(format!(
            "{path}: the project changed before the user confirmed"
        ));
        return;
    }
    let yes = common::part(
        rig,
        P,
        &["content", "confirm", "confirm_bar", "confirm_yes"],
    );
    common::click(rig, yes);
    if family(rig) != to.dir() {
        bad.push(format!(
            "{path}: confirming did not switch ({})",
            family(rig)
        ));
    }
}

fn lossless(rig: &mut Rig, to: Family, path: &str, bad: &mut Vec<String>) {
    if let Some(t) = switch(rig, to) {
        bad.push(format!("{path}: a lossless switch asked: {t:?}"));
    }
    if family(rig) != to.dir() {
        bad.push(format!(
            "{path}: one click did not switch ({})",
            family(rig)
        ));
    }
}

fn run(fault: bool) -> Vec<String> {
    let mut bad = Vec::new();
    // 3D -> 2D (lossy), then 2D -> 3D (lossless: additive).
    let mut rig = fresh(Template::ThreeD, fault, "3d");
    lossy(
        &mut rig,
        Family::TwoD,
        &["Ground", "Sun"],
        "3D->2D",
        &mut bad,
    );
    lossless(&mut rig, Family::ThreeD, "2D->3D", &mut bad);
    bad
}

#[test]
fn test_lossy_warnings_present() {
    let bad = run(false);
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn positive_control_a_path_without_its_warning_fails() {
    let bad = run(true);
    println!("positive control: {bad:#?}");
    assert!(
        bad.iter()
            .any(|b| b.starts_with("3D->2D") && b.contains("no confirmation")),
        "3D->2D lost data without a warning and the guard did not see it: {bad:#?}"
    );
}

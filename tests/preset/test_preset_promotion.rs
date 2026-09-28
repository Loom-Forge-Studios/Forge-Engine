//! Ch.31 §31.4 — `test_preset_promotion` (DoD M2-12, I15's "every project is promotable to
//! every preset"): each promotion arrow, applied to a real project by the core's
//! `forge.project.promote` command with the presets' real data (their `workspace.ron`
//! defaults, their new-scene templates), then walked back:
//!
//! * **no loss where none is expected** — 2D → 3D → 2D with 2D content only (a user's own
//!   value included): the project after the round trip is the project before, every entity,
//!   property and setting;
//! * **a lossy arrow says so and changes nothing until confirmed** — 3D → 2D with 3D-only
//!   data: refused without `accept_loss`, naming the loss, the project untouched; confirmed,
//!   it is one undoable step;
//! * **every project is promotable to every preset** — every (from, to) pair succeeds.
//!
//! (A plugin's presets and their arrows are guarded with the plugin.)
//!
//! Positive controls (W2): `positive_control_a_promotion_that_loses_data_is_caught` — a
//! promotion that also drops one property fails the round trip; and
//! `positive_control_a_lossy_switch_without_confirmation_is_caught` — a client that always
//! confirms (the warning path removed) fails the "changes nothing until confirmed" check.

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::{EditorCore, LocalBus, SharedCore};
use forge_editor::presets::Family;
use forge_editor::project::promote::promote_command;
use forge_editor::project::{ProjectOp, Template};
use forge_project::format::ProjectDoc;
use std::sync::atomic::{AtomicU64, Ordering};

type Promote<'a> = &'a dyn Fn(&mut LocalBus, Family, bool) -> Result<(), String>;

struct Rig {
    core: SharedCore,
    client: LocalBus,
}

fn rig(template: Template) -> Rig {
    static N: AtomicU64 = AtomicU64::new(0);
    let core = EditorCore::new();
    let mut client = EditorCore::connect(&core, Issuer::Human { user: "ada".into() });
    client.apply(
        ProjectOp::Create {
            location: format!(
                "memory:promotion-{}-{}",
                template.id(),
                N.fetch_add(1, Ordering::Relaxed)
            ),
            name: "Promotion".into(),
            template,
            discard_unsaved: false,
        }
        .command(),
        None,
    );
    let _ = client.pump();
    assert!(
        EditorCore::project_status(&core).open.is_some(),
        "{:?}",
        EditorCore::project_status(&core).outcomes
    );
    Rig { core, client }
}

fn doc(r: &Rig) -> ProjectDoc {
    EditorCore::read(&r.core, ProjectDoc::from_project)
}

fn apply(r: &mut Rig, cmd: EditorCommand) {
    r.client.apply(cmd, None);
    let p = r.client.pump();
    assert!(
        p.refused.is_empty(),
        "{:?}",
        p.refused.first().map(|x| &x.rejection)
    );
}

/// The real client: the promote command, `accept_loss` as asked.
fn real(c: &mut LocalBus, to: Family, accept: bool) -> Result<(), String> {
    c.apply(promote_command(to, accept), None);
    match c.pump().refused.first() {
        Some(r) => Err(r.rejection.error.to_string()),
        None => Ok(()),
    }
}

fn family_of(r: &Rig) -> String {
    match EditorCore::read(&r.core, |p| {
        p.setting(forge_project::PRESET_SETTING).cloned()
    }) {
        Some(Value::Text(t)) => t,
        other => format!("{other:?}"),
    }
}

/// Promote along `path` (no confirmation: every step must be lossless), then check the
/// project is exactly what it was. `Err`: what differs.
fn round_trip(r: &mut Rig, path: &[Family], promote: Promote<'_>) -> Result<(), String> {
    let before = doc(r);
    for to in path {
        promote(&mut r.client, *to, false).map_err(|e| format!("{to:?}: {e}"))?;
        if family_of(r) != to.dir() {
            return Err(format!(
                "promoted to {to:?} but the project says {}",
                family_of(r)
            ));
        }
    }
    let after = doc(r);
    if after != before {
        let mut diff = Vec::new();
        for (k, v) in &before.settings {
            if after.settings.get(k) != Some(v) {
                diff.push(format!("setting {k}: {v:?} -> {:?}", after.settings.get(k)));
            }
        }
        for k in after.settings.keys() {
            if !before.settings.contains_key(k) {
                diff.push(format!("setting {k} appeared"));
            }
        }
        for e in &before.entities {
            match after.entities.iter().find(|a| a.id == e.id) {
                None => diff.push(format!("entity {} is gone", e.name)),
                Some(a) if a != e => {
                    diff.push(format!("entity {} changed: {e:?} -> {a:?}", e.name))
                }
                Some(_) => {}
            }
        }
        return Err(diff.join("; "));
    }
    Ok(())
}

/// A lossy step: refused without confirmation, naming `loss`, the project unchanged; then
/// confirmed: applied as one undoable step that undo reverts exactly.
fn lossy(r: &mut Rig, to: Family, loss: &str, promote: Promote<'_>) -> Result<(), String> {
    let before = doc(r);
    match promote(&mut r.client, to, false) {
        Ok(()) => return Err(format!("{to:?} applied without confirmation")),
        Err(e) if !e.contains(loss) => {
            return Err(format!("the refusal does not name {loss:?}: {e}"));
        }
        Err(_) => {}
    }
    if doc(r) != before {
        return Err("a refused switch changed the project".into());
    }
    promote(&mut r.client, to, true)?;
    let after = doc(r);
    if after == before {
        return Err("the confirmed switch changed nothing".into());
    }
    let t = r.client.undo_target().ok_or("nothing to undo")?;
    r.client.undo(t);
    let _ = r.client.pump();
    if doc(r) != before {
        return Err("undo did not restore the project: the switch was not one step".into());
    }
    Ok(())
}

#[test]
fn test_preset_promotion() {
    use Family::*;
    // 2D → 3D → 2D with 2D content only: the 2D content lives on as a 2D layer and comes back
    // unchanged; a user's own value survives (the grid differs from every preset's default).
    let mut r = rig(Template::TwoD);
    apply(
        &mut r,
        EditorCommand::Spawn {
            name: "Tree".into(),
            parent: None,
        },
    );
    apply(
        &mut r,
        EditorCommand::SetSetting {
            key: "editor.grid_size".into(),
            value: Some(Value::Float(0.5)),
        },
    );
    round_trip(&mut r, &[ThreeD, TwoD], &real).unwrap_or_else(|e| panic!("2D ⇄ 3D: {e}"));
    // The 2D content is really kept as a 2D layer while in 3D.
    real(&mut r.client, ThreeD, false).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        doc(&r).settings.get("render.layers_2d"),
        Some(&Value::Bool(true))
    );

    // Lossy arrows: they say so, and change nothing until confirmed.
    let mut r = rig(Template::ThreeD);
    lossy(&mut r, TwoD, "3D-only data", &real).unwrap_or_else(|e| panic!("3D → 2D: {e}"));

    // Every project is promotable to every preset.
    for from in Template::ALL {
        for to in Family::ALL {
            if Some(to.dir()) == from.preset() {
                continue;
            }
            let mut r = rig(from.clone());
            real(&mut r.client, to, true).unwrap_or_else(|e| panic!("{from:?} → {to:?}: {e}"));
            assert_eq!(family_of(&r), to.dir(), "{from:?} → {to:?}");
        }
    }
}

#[test]
fn positive_control_a_promotion_that_loses_data_is_caught() {
    use Family::*;
    // A promotion that also drops the player's scale (a rule losing data it should not).
    let leaky = |c: &mut LocalBus, to: Family, accept: bool| -> Result<(), String> {
        real(c, to, accept)?;
        let (p, _) = c.snapshot();
        if let Some((k, _)) = p.entities().find(|(_, e)| e.name() == "Player") {
            c.apply(
                EditorCommand::RemoveProperty {
                    entity: k,
                    path: "transform.scale".into(),
                },
                None,
            );
            let _ = c.pump();
        }
        Ok(())
    };
    let scaled = |r: &mut Rig| {
        let player = EditorCore::read(&r.core, |p| {
            p.entities()
                .find(|(_, e)| e.name() == "Player")
                .map(|(k, _)| k)
        })
        .unwrap_or_else(|| panic!("the 2D template has a player"));
        apply(
            r,
            EditorCommand::SetProperty {
                entity: player,
                path: "transform.scale".into(),
                value: Value::Float(2.0),
            },
        );
    };
    let mut r = rig(Template::TwoD);
    scaled(&mut r);
    let e = round_trip(&mut r, &[ThreeD, TwoD], &leaky)
        .err()
        .unwrap_or_default();
    assert!(
        e.contains("entity Player changed"),
        "a lossy round trip passed: {e:?}"
    );
    // The same trip through the real rules passes (the control is the leak, not the trip).
    let mut r = rig(Template::TwoD);
    scaled(&mut r);
    assert_eq!(round_trip(&mut r, &[ThreeD, TwoD], &real), Ok(()));
}

#[test]
fn positive_control_a_lossy_switch_without_confirmation_is_caught() {
    // A client that always confirms (the warning path removed).
    let always = |c: &mut LocalBus, to: Family, _: bool| real(c, to, true);
    let mut r = rig(Template::ThreeD);
    let e = lossy(&mut r, Family::TwoD, "3D-only data", &always)
        .err()
        .unwrap_or_default();
    assert!(e.contains("applied without confirmation"), "{e:?}");
}

//! The split-editor parity script (Ch.34 §34.5): one command sequence, and how to drive it
//! through any `BusClient`. Shared by `test_split_editor_parity` (a host in the test's
//! process) and forge-cli's `test_headless_remote_host` (the `forge --headless --remote-host`
//! process), so both prove parity on the same commands.
#![allow(dead_code)]

use forge_cmd::{EditorCommand, EntityKey, Value};
use forge_editor::client::{BusClient, TxnInfo};

pub enum Step {
    Do(EditorCommand),
    Drag {
        label: &'static str,
        frames: Vec<EditorCommand>,
        commit: bool,
    },
    Undo,
    Redo,
}

fn spawn(name: &str, parent: Option<u64>) -> Step {
    Step::Do(EditorCommand::Spawn {
        name: name.into(),
        parent: parent.map(EntityKey),
    })
}

fn prop(e: u64, path: &str, v: Value) -> EditorCommand {
    EditorCommand::SetProperty {
        entity: EntityKey(e),
        path: path.into(),
        value: v,
    }
}

fn setting(k: &str, v: Option<Value>) -> Step {
    Step::Do(EditorCommand::SetSetting {
        key: k.into(),
        value: v,
    })
}

pub fn script() -> Vec<Step> {
    let mut s = vec![
        spawn("Cube", None),
        spawn("Light", None),
        spawn("W\u{e8}ird \u{f1}ame \u{2713}", Some(0)),
        Step::Do(prop(0, "transform.position", Value::Vec3([0.1, 0.2, 0.3]))),
        Step::Do(prop(0, "mass", Value::Float(1.0 / 3.0))),
        Step::Do(prop(
            1,
            "light.power",
            Value::Float(f64::MIN_POSITIVE / 8.0),
        )),
        Step::Do(prop(1, "light.zero", Value::Float(-0.0))),
        Step::Do(prop(2, "tiny", Value::Float(1e-300))),
        Step::Do(prop(2, "count", Value::Int(i64::MIN))),
        Step::Do(prop(2, "target", Value::Entity(EntityKey(1)))),
        Step::Do(prop(2, "note", Value::Text("line\nnext \u{1f680}".into()))),
        setting("editor.grid", Some(Value::Float(0.1))),
        setting("editor.snap", Some(Value::Bool(true))),
        Step::Do(EditorCommand::Rename {
            entity: EntityKey(1),
            name: "Key Light".into(),
        }),
        Step::Do(EditorCommand::Reparent {
            entity: EntityKey(1),
            parent: Some(EntityKey(0)),
        }),
    ];
    // A 40-frame drag, committed: one undo entry.
    s.push(Step::Drag {
        label: "Drag scale",
        frames: (0..40)
            .map(|i| prop(0, "scale", Value::Float(1.0 + f64::from(i) * 0.037)))
            .collect(),
        commit: true,
    });
    // A drag cancelled (Esc): it leaves nothing.
    s.push(Step::Drag {
        label: "Drag power",
        frames: (0..12)
            .map(|i| prop(1, "light.power", Value::Float(500.0 + f64::from(i) * 0.1)))
            .collect(),
        commit: false,
    });
    s.extend([
        // Refused on both sides: no entity 999.
        Step::Do(EditorCommand::Rename {
            entity: EntityKey(999),
            name: "ghost".into(),
        }),
        Step::Do(EditorCommand::RemoveProperty {
            entity: EntityKey(2),
            path: "tiny".into(),
        }),
        setting("editor.snap", None),
        Step::Undo,
        Step::Undo,
        Step::Redo,
        spawn("Temp", None),
        Step::Do(EditorCommand::Despawn {
            entity: EntityKey(3),
        }),
        Step::Undo,
        Step::Do(prop(0, "mass", Value::Float(0.1 + 0.2))),
    ]);
    s
}

/// The undo history as `(label, state)` rows.
pub fn rows(h: Vec<TxnInfo>) -> Vec<(String, String)> {
    h.into_iter()
        .map(|i| (i.label, i.state.to_string()))
        .collect()
}

/// A pump's refusals as `what: CODE` lines.
pub fn codes(p: forge_editor::client::Pumped) -> Vec<String> {
    p.refused
        .into_iter()
        .map(|r| format!("{}: {}", r.what, r.rejection.code()))
        .collect()
}

/// Drive `script` through `c`; `settle` makes every answer so far visible to `c.pump()`.
pub fn drive(
    c: &mut dyn BusClient,
    settle: &mut dyn FnMut(&mut dyn BusClient) -> Vec<String>,
) -> Vec<String> {
    let mut refused = Vec::new();
    for step in script() {
        match step {
            Step::Do(cmd) => {
                c.apply(cmd, None);
            }
            Step::Drag {
                label,
                frames,
                commit,
            } => {
                let t = c.begin(label);
                for f in frames {
                    c.apply(f, Some(t));
                }
                if commit {
                    c.commit(t);
                } else {
                    c.cancel(t);
                }
            }
            Step::Undo => {
                if let Some(t) = c.undo_target() {
                    c.undo(t);
                }
            }
            Step::Redo => {
                if let Some(t) = c.redo_target() {
                    c.redo(t);
                }
            }
        }
        refused.extend(settle(c));
    }
    refused
}

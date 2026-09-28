//! `forge-scene` — scene composition (Ch.28 §28.1–28.8, DoD M5-9): **nesting and
//! inheritance, Godot's model**.
//!
//! * A **scene** is a reusable subtree kept in the project's scene library. An **instance**
//!   places a scene inside another scene or the world; a **derived** scene is a scene whose
//!   root is an instance of its base, so it holds only what it overrides and adds. Nesting
//!   and inheritance are one mechanism, to any depth.
//! * Instances refer to their scene by a **stable id**; every node inside a scene has a
//!   **uid** in it, which override records key on ([`model`]).
//! * An instance's **overrides are reflected property diffs** against its base
//!   ([`model::OverrideRecord`], [`model::overrides`]): an edit on an instance is the
//!   override, Revert to base sets it back. There is no separate override bookkeeping.
//! * **A base change reaches every instance and derived scene except where overridden**, in
//!   the same diff as the edit: [`SceneDeriver`] is the bus's deriver, so this holds for
//!   every command from every issuer (a gizmo drag, a script, an automation session's
//!   `SetProperty`), and one undo step takes the edit and its propagation back together.
//! * **Cycles are refused with a readable error** naming the loop (`SCENE-0002`).
//! * Every change is a command: the `forge.scene.*` handlers ([`plan`]) plan diffs like any
//!   other, so the editor and every other issuer use the same path (I7).
//! * **The files store references, not copies** ([`store`]): an instance is its root and
//!   the override records it needs; loading derives the rest.
//!
//! ```
//! use forge_cmd::{Bus, CommandSink, EditorCommand, Issuer, Value};
//!
//! let mut bus = Bus::new();
//! forge_scene::install(&mut bus, &[]);
//! let invoke = |target: &str, args: serde_json::Value| EditorCommand::Invoke {
//!     target: target.into(),
//!     args: args.to_string(),
//! };
//! let run = |bus: &mut Bus, cmd| {
//!     let e = bus.envelope(Issuer::Test, cmd);
//!     bus.apply(e).expect("applies")
//! };
//! run(&mut bus, invoke("forge.scene.new", serde_json::json!({"name": "Tree", "id": "tree"})));
//! let tree = forge_scene::model::scene_root(bus.project(), "tree").expect("the scene");
//! run(&mut bus, EditorCommand::SetProperty { entity: tree, path: "height".into(), value: Value::Float(4.0) });
//! run(&mut bus, invoke("forge.scene.instance", serde_json::json!({"scene": "tree"})));
//! let inst = forge_scene::model::dependents(bus.project(), tree)[0];
//! assert_eq!(bus.project().entity(inst).and_then(|e| e.property("height")), Some(&Value::Float(4.0)));
//! // Editing the scene reaches the instance.
//! run(&mut bus, EditorCommand::SetProperty { entity: tree, path: "height".into(), value: Value::Float(6.0) });
//! assert_eq!(bus.project().entity(inst).and_then(|e| e.property("height")), Some(&Value::Float(6.0)));
//! ```

#![forbid(unsafe_code)]

mod derive;
mod error;
pub mod model;
pub mod place;
pub mod plan;
mod read;
pub mod store;

use std::sync::Arc;

use forge_cmd::{Bus, CmdError};

pub use derive::SceneDeriver;
pub use error::SceneError;
pub use read::SceneRead;

forge_trace::control_switches! {
    /// W2 fault switches for the scene guards' positive controls (ADR 0046). A production
    /// build carries none: every accessor is `false`.
    #[derive(Clone, Debug, Default)]
    pub struct SceneFaults {
        /// Propagation overwrites a mirror's value even where the mirror overrides it.
        pub ignore_overrides: bool,
        /// Moving an instance into a scene skips the cycle check.
        pub skip_cycle_check: bool,
        /// The deriver does nothing (base edits stay in the base).
        pub no_propagation: bool,
        /// The files keep every mirrored node whole (copies instead of references).
        pub store_copies: bool,
    }
}

/// Register the scene commands on `bus` and install the [`SceneDeriver`], leaving the
/// `Invoke` targets in `exempt` (whole-document loads) alone. A target the bus already
/// has keeps its handler.
pub fn install(bus: &mut Bus, exempt: &[&str]) {
    for (target, policy, plan) in plan::handlers() {
        let _ = bus.register_handler(target, policy, plan);
    }
    bus.set_deriver(Some(Arc::new(SceneDeriver::new(exempt))));
}

/// W2 positive controls only: [`install`] with fault switches on.
#[cfg(any(test, feature = "controls"))]
#[doc(hidden)]
pub fn install_with_faults(bus: &mut Bus, exempt: &[&str], faults: SceneFaults) {
    for (target, policy, plan) in plan::handlers() {
        let _ = bus.register_handler(target, policy, plan);
    }
    bus.set_deriver(Some(Arc::new(SceneDeriver::with_faults(exempt, faults))));
}

/// A scene refusal carried in a bus rejection, recovered from its text (for a caller that
/// wants the `SCENE-*` code).
#[must_use]
pub fn scene_code(e: &CmdError) -> Option<String> {
    let text = e.to_string();
    let at = text.find("SCENE-")?;
    text.get(at..at + 10).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in SceneError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-scene |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(
                e.to_string().starts_with(code.as_str()),
                "Display leads with the code"
            );
        }
    }

    #[test]
    fn meta_paths_and_uids() {
        assert!(model::is_meta("scene"));
        assert!(model::is_meta("scene.base"));
        assert!(!model::is_meta("scenery.x"));
        let a = model::mix_uid(1, 2);
        assert_ne!(a, model::mix_uid(2, 1));
        assert_eq!(a, model::mix_uid(1, 2), "deterministic");
        assert!(
            (1 << 62..1 << 63).contains(&a),
            "its own range, fits an i64"
        );
        assert!(plan::id_is_well_formed("pine_tree_2"));
        assert!(!plan::id_is_well_formed("Pine Tree"));
    }
}

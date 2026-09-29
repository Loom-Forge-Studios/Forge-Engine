//! `forge-input` — the input runtime (Ch.28 §28.9–§28.18, DoD M7-12). Base: both editions.
//!
//! The editor authors the game's input (the input-map panel, M2-68: contexts, actions,
//! default bindings, modifiers, triggers); this crate plays it against real devices:
//!
//! | Part | Module |
//! |---|---|
//! | The control vocabulary shared with the editor | [`control`] |
//! | The authored map and its compiled form | [`map`] |
//! | Deadzones and response curves | [`modifier`] |
//! | Hold, tap, multi-tap, pulse, chord and combo triggers | [`trigger`] |
//! | Devices, players, contexts, the per-frame update, rebinding | [`runtime`] |
//! | Rebinding persistence | [`persist`] |
//! | Touch gestures | [`touch`] |
//! | On-screen touch controls (a virtual gamepad) | [`onscreen`] |
//! | Gyro aiming | [`motion`] |
//! | Rumble | [`haptics`] |
//! | Platform-aware glyphs | [`glyph`] |
//! | The input debugger's snapshot | [`debug`] |
//! | winit (keyboard, mouse, touch) and gilrs (gamepads) | [`backend`] (features) |
//!
//! Budgets (owner rule 2) are in `budgets.ron` next to this crate's manifest ([`BUDGETS`],
//! declared into a tracer by [`declare_budgets`]); `tests/test_input_budgets.rs` measures
//! them. The premium edition layers nothing here: input is the same in both editions.

#![forbid(unsafe_code)]

pub mod backend;
pub mod control;
pub mod debug;
pub mod error;
pub mod faults;
pub mod glyph;
pub mod haptics;
pub mod map;
pub mod modifier;
pub mod motion;
pub mod onscreen;
pub mod persist;
pub mod runtime;
pub mod touch;
pub mod trigger;

pub use control::{ActionKind, Binding, Control, ControlRef, Device, Shape};
pub use debug::DebugSnapshot;
pub use error::InputError;
pub use faults::InputFaults;
pub use glyph::{Glyph, GlyphContext, KeyLabels, PadFamily, Platform};
pub use haptics::{MotorCommand, Rumble};
pub use map::{
    ActionDef, ActionHandle, BindingDef, CompiledMap, ContextDef, ContextHandle, InputMapDef,
};
pub use modifier::{Curve, DeadzoneKind, Modifier};
pub use motion::{GyroSettings, GyroSpace, MotionSample, MotionSource};
pub use onscreen::{OnScreenControl, OnScreenControls, OnScreenKind};
pub use persist::{FileOverrideStore, MemoryOverrideStore, OverrideStore};
pub use runtime::{
    ActionEvent, ActionEventKind, ActionState, BindingOverrides, ConflictPolicy, DeviceDesc,
    DeviceFilter, DeviceId, InputRuntime, InputStats, JoinPolicy, Phase, PlayerEvent, PlayerId,
    RawEvent, RebindRequest, RebindState,
};
pub use touch::{GestureConfig, TouchInput, TouchPhase};
pub use trigger::Trigger;

/// The crate's budget rows (`budgets.ron`).
pub const BUDGETS_RON: &str = include_str!("../budgets.ron");

/// One budget row.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct BudgetRow {
    pub name: String,
    /// `"ms"`, `"ns"`, or `""` for a count.
    pub unit: String,
    pub allowance: f64,
    /// The measured median it was set from (same unit), or 0 for an exact count.
    pub baseline: f64,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
struct BudgetFile {
    rows: Vec<BudgetRow>,
}

/// The budget rows (see [`BUDGETS_RON`]).
pub fn budgets() -> Result<Vec<BudgetRow>, String> {
    ron::from_str::<BudgetFile>(BUDGETS_RON)
        .map(|f| f.rows)
        .map_err(|e| e.to_string())
}

/// A budget row's allowance by name.
pub fn budget(name: &str) -> Option<f64> {
    budgets()
        .ok()?
        .into_iter()
        .find(|r| r.name == name)
        .map(|r| r.allowance)
}

/// Declare the time budgets into a tracer (the profiler shows `input.update` against its
/// allowance, measured by the zone of that name).
pub fn declare_budgets(tracer: &forge_trace::Tracer) -> usize {
    let Ok(rows) = budgets() else { return 0 };
    let mut n = 0;
    for r in rows {
        match r.unit.as_str() {
            "ms" => tracer.declare_budget(&r.name, r.allowance, "ms"),
            "ns" => tracer.declare_budget(&r.name, r.allowance * 1e-6, "ms"),
            _ => tracer.declare_budget(&r.name, r.allowance, ""),
        }
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    #[test]
    fn budgets_parse() {
        let rows = super::budgets().unwrap_or_else(|e| panic!("{e}"));
        assert!(rows.iter().any(|r| r.name == "input.update"));
    }
}

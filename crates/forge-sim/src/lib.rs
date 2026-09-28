//! `forge-sim` — the play core (M2-5; Ch.21 §21.21 "Play controls", Ch.34 §34.4, Ch.37 §37.5).
//!
//! * [`edit`] — the edit world as a snapshot ([`EditSnapshot`]), from the core's project or a
//!   client's mirror, with a content hash.
//! * [`world`] — the simulation world ([`SimWorld`]): forked from the edit world onto
//!   `forge_core`'s regionized scheduler (one region per frame), stepped at a fixed 60 Hz with
//!   deterministic `f64` arithmetic, changed only by [`SimInput`]s at step boundaries, and
//!   hashed to the bit.
//! * [`session`] — the play session ([`PlaySession`]): Play / Pause / Step / Stop, the
//!   editor clock turned into fixed steps, the control log, and the recorder.
//! * [`record`] — the replay file ([`Recording`], JSON Lines) and [`replay`], which re-runs
//!   a session and requires every event and hash to match.
//!
//! The editor runs a [`PlaySession`] behind its `PlayBackend` trait, forked from the mirror;
//! `forge --headless` drives the same session forked from the core's project. Neither can
//! change the project: a session is handed a snapshot, never a command sink.
//!
//! Every step is a `sim.step` zone on the session's tracer (`forge-trace`), with a named
//! budget of [`session::SIM_STEP_BUDGET_MS`] per frame.

#![forbid(unsafe_code)]

pub mod edit;
mod error;
pub mod record;
pub mod session;
pub mod world;

pub use edit::{EditEntity, EditSnapshot};
pub use error::SimError;
pub use record::{FORMAT_VERSION, RecEvent, RecHeader, Recording, ReplayReport, replay};
pub use session::{
    MAX_CATCH_UP, PlayCommand, PlayLogEntry, PlaySession, PlayState, step_of_tick, tick_of_step,
};
pub use world::{InputAction, SIM_DT, SIM_RATE_HZ, SimFaults, SimInput, SimTransform, SimWorld};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sim_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in SimError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-sim |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }
}

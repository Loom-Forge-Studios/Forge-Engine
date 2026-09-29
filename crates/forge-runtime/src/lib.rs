//! `forge-runtime` — the shipping runtime (Ch.28): the part of Forge a customer's game
//! contains.
//!
//! **I21: the software never phones home to function** (Ch.38.1). This crate links no
//! licensing crate (`forge-licence` is not in its dependency graph and cannot be) and no
//! network crate that is not an explicitly allow-listed transport. That is a structural
//! guarantee, checked on the *resolved* dependency graph by
//! `tests/licence/test_no_runtime_phone_home.rs`, not a promise.
//!
//! **The determinism positive control can never ship.** `forge-num`'s `mutate-det` feature
//! perturbs the noise basis by one ulp so the I2 gate can prove it fails (Ch.3.4). A runtime
//! built with it would generate different worlds from every other build; the constant
//! assertion below makes such a build a compile error, so it cannot reach a customer.
//!
//! What is here: identity, the engine credit (Ch.38.7, E-64 — inert static text), the spine
//! crates a game needs, and **game UI on `forge-ui`** (§21.20, M2-29): [`game_ui`] is the
//! sample main menu, settings screen, HUD, pause menu and credits, with gamepad navigation
//! and localisation, linking no editor crate (`test_game_ui_links_no_editor`). The game
//! loop and the gameplay framework arrive with their milestones.

#![forbid(unsafe_code)]

pub mod game_ui;
pub mod gpu_mode;
pub mod input_ui;

/// `mutate-det` is a test-only positive control (Ch.3.4). A runtime built with it does not
/// compile — this is the assertion the I21 guard's `MUTATE_DET` control builds against.
const _: () = assert!(
    !forge_num::MUTATE_DET,
    "RUNTIME: forge-num's `mutate-det` positive control is enabled; it must never ship in forge-runtime"
);

/// Whether this build carries `forge-num`'s `mutate-det` perturbation. Always `false`: the
/// constant assertion above refuses to compile the crate otherwise.
pub const MUTATE_DET_SHIPPED: bool = forge_num::MUTATE_DET;

/// The runtime's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The engine credit every product containing `forge-runtime` carries (Ch.38.7, E-64).
/// Static text: no network, no entitlement, nothing executed (`test_attribution_is_inert`).
pub const ENGINE_CREDIT: &str = forge_ui::game::ENGINE_CREDIT;

/// What this runtime is, as its binary prints it.
#[must_use]
pub fn describe() -> String {
    format!("forge-runtime {VERSION} ({ENGINE_CREDIT})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_carries_the_credit() {
        assert!(describe().contains(ENGINE_CREDIT));
    }
}

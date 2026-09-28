//! Shared by the forge-gpu guards: a GPU guard never passes silently for want of an adapter.
//! Locally that is AWAITING(no adapter) (hardware); under `CI=true` it is a failure (W9),
//! because CI is expected to provide at least a software rasteriser.

#![allow(dead_code)]

use forge_gpu::{AdapterPool, PoolOptions};

/// What "no adapter" means here: `Ok(reason)` = AWAITING locally, `Err(msg)` = fail on CI.
pub fn no_adapter_outcome(error: &str, on_ci: bool) -> Result<String, String> {
    if on_ci {
        Err(format!(
            "CI=true and no GPU adapter ({error}): GPU guards must not pass silently (W9)"
        ))
    } else {
        Ok(format!("no GPU adapter on this machine: {error}"))
    }
}

pub fn on_ci() -> bool {
    std::env::var("CI").is_ok_and(|v| v == "true" || v == "1")
}

/// A pool, or `None` after printing AWAITING (locally); panics under `CI=true`.
pub fn pool(opts: &PoolOptions) -> Option<AdapterPool> {
    match AdapterPool::new(opts) {
        Ok(p) => Some(p),
        Err(e) => match no_adapter_outcome(&e.to_string(), on_ci()) {
            Ok(reason) => {
                println!("AWAITING({reason})");
                None
            }
            Err(loud) => panic!("{loud}"),
        },
    }
}

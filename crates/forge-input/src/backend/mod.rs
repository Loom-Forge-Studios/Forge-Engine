//! Device backends: each turns a platform's events into the runtime's raw events. They are
//! features, so a headless build links no windowing or gamepad crate.

#[cfg(feature = "gilrs")]
pub mod gilrs;
#[cfg(feature = "winit")]
pub mod winit;

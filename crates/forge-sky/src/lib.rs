//! `forge-sky` — the sky's physics in `f64` (Ch.11).
//!
//! * [`atmosphere`] — an atmosphere **derived from a body's composition** (DoD M1-8): gases,
//!   pressure, temperature, gravity, aerosols and ozone become the coefficients of Bruneton's
//!   precomputed scattering model, which `forge-render` tabulates on the GPU; plus an `f64`
//!   reference integrator the tables are checked against.
//!
//! Nothing here names `f32`: `forge-render` narrows at its last mile.

#![forbid(unsafe_code)]

pub mod atmosphere;
mod error;

pub use atmosphere::{
    Aerosols, AtmosphereBody, AtmosphereParams, Composition, DensityLayer, DensityProfile, Gas,
    ReferenceModel, WAVELENGTHS,
};
pub use error::SkyError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sky_error_code_is_registered() {
        let md = include_str!("../../../docs/error-codes.md");
        for e in SkyError::all_variants_for_tests() {
            let code = e.code();
            let row = format!("| {code} | forge-sky |");
            assert!(
                md.contains(&row),
                "{code} is not allocated in docs/error-codes.md"
            );
            assert!(e.to_string().starts_with(code.as_str()));
        }
    }
}

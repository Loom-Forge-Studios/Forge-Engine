//! `test_atmosphere` — the GPU atmosphere tables reproduce the model they were built from
//! (DoD M1-8; Ch.11 §11.2).
//!
//! The renderer's own sky functions (`shaders/atmosphere.wgsl`, the ones `sky.wgsl` and
//! `mesh.wgsl` call) are evaluated on the GPU through [`GpuAtmosphere::probe`] and held
//! against `forge_sky::ReferenceModel`, which integrates the same composition-derived model
//! directly in `f64` with no tables:
//!
//! * **Transmittance** toward the sun from the ground to orbit, zenith to below the horizon:
//!   within 1 % (relative) + 1e-3.
//! * **Single scattering** (tables built with one order): sky radiance for views from the
//!   zenith to the horizon, sun high and low, from the ground and from 30 km (outside the
//!   aureole, nu > 0.95, where Mie's forward peak is sharper than a table holds): within 3 %.
//! * **Multiple scattering** (four orders) only adds light, and not more than doubles it.
//! * **Composition matters**: Mars' thin CO2-and-dust air gives a darker, redder sky than
//!   Earth's from the same code.
//!
//! Positive control (W2): `positive_control_a_perturbed_composition_disagrees` — tables built
//! from the same body with 30 % more Rayleigh scattering (a composition change) fail the
//! single-scattering comparison against the unperturbed reference, so the tolerance can fail.

mod common;

use forge_frames::DVec3;
use forge_render::{AtmosphereSettings, CentreOffset, GpuAtmosphere, ProbeQuery, ProbeResult};
use forge_sky::{AtmosphereBody, AtmosphereParams, ReferenceModel};

const ROW: &str = "C-atmosphere-matches-reference";
/// Single-scattering agreement (the worst case measured on the RTX 3080 is 1.6 %).
const TOLERANCE: f64 = 0.03;

fn single_order() -> AtmosphereSettings {
    AtmosphereSettings {
        orders: 1,
        ..AtmosphereSettings::default()
    }
}

/// `(camera, view, sun)` geometries: ground and 30 km, views from zenith to the horizon, sun
/// high and near the horizon, azimuths toward and away from the sun.
fn sky_rays(p: &AtmosphereParams) -> Vec<(DVec3, DVec3, DVec3)> {
    let mut out = Vec::new();
    for alt in [2.0, 30_000.0] {
        let cam = DVec3::new(0.0, 0.0, p.bottom_radius + alt);
        for sun_elev in [60.0f64, 12.0, 3.0] {
            let se = sun_elev.to_radians();
            let sun = DVec3::new(se.cos(), 0.0, se.sin());
            for view_elev in [90.0f64, 45.0, 15.0, 5.0] {
                for az in [0.0f64, 90.0, 180.0] {
                    let (ve, a) = (view_elev.to_radians(), az.to_radians());
                    let view = DVec3::new(ve.cos() * a.cos(), ve.cos() * a.sin(), ve.sin());
                    // Keep away from the sun's aureole, where Mie's forward peak is sharper
                    // than any table can hold.
                    if view.dot(sun) > 0.95 {
                        continue;
                    }
                    out.push((cam, view, sun));
                }
            }
        }
    }
    out
}

/// Worst relative error of GPU single scattering (tables of `built`) against the f64
/// reference of `truth`, over the green and blue channels.
fn single_scattering_error(
    dev: &forge_gpu::GpuDevice,
    built: &AtmosphereParams,
    truth: &AtmosphereParams,
) -> (f64, String) {
    let a = GpuAtmosphere::new(dev, built, &single_order()).expect("atmosphere");
    let rays = sky_rays(truth);
    let q: Vec<ProbeQuery> = rays
        .iter()
        .map(|&(camera, view, sun)| ProbeQuery::Sky {
            camera: CentreOffset(camera),
            view,
            sun,
        })
        .collect();
    let got = a.probe(dev, &q).expect("probe");
    let m = ReferenceModel {
        p: *truth,
        steps: 600,
    };
    let mut worst = (0.0, String::new());
    for ((cam, view, sun), g) in rays.iter().zip(&got) {
        let r = cam.length();
        let mu = cam.dot(*view) / r;
        let mu_s = cam.dot(*sun) / r;
        let nu = view.dot(*sun);
        let want = m.single_scattering(r, mu, mu_s, nu, 400);
        for k in 1..3 {
            let e = (g.radiance[k] - want[k]).abs() / want[k].max(1e-6);
            if e > worst.0 {
                worst = (
                    e,
                    format!(
                        "alt {:.0} m, view.z {:.3}, sun.z {:.3}, nu {nu:.3}: gpu {:?} vs f64 {want:?}",
                        r - truth.bottom_radius,
                        view.z,
                        sun.z,
                        g.radiance
                    ),
                );
            }
        }
    }
    worst
}

#[test]
fn transmittance_matches_the_f64_reference() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let p = AtmosphereBody::earth().derive().expect("earth");
    let a = GpuAtmosphere::new(dev, &p, &single_order()).expect("atmosphere");
    let m = ReferenceModel::new(p);
    let mut q = Vec::new();
    let mut want = Vec::new();
    for alt in [0.0, 1_000.0, 10_000.0, 40_000.0] {
        let at = DVec3::new(0.0, 0.0, p.bottom_radius + alt);
        for elev in [90.0f64, 30.0, 10.0, 3.0, 1.0, 0.2] {
            let e = elev.to_radians();
            let sun = DVec3::new(e.cos(), 0.0, e.sin());
            q.push(ProbeQuery::Sun {
                at: CentreOffset(at),
                sun,
            });
            want.push(m.transmittance_to_sun(at.length(), sun.z));
        }
    }
    let got = a.probe(dev, &q).expect("probe");
    for ((g, w), qq) in got.iter().zip(&want).zip(&q) {
        for k in 0..3 {
            let err = (g.transmittance[k] - w[k]).abs();
            assert!(
                err <= 0.01 * w[k] + 1e-3,
                "{qq:?} channel {k}: gpu {:?} vs f64 {w:?}",
                g.transmittance
            );
        }
    }
    assert!(dev.take_uncaptured_errors().is_empty());
}

#[test]
fn single_scattering_matches_the_f64_reference() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let p = AtmosphereBody::earth().derive().expect("earth");
    let (worst, at) = single_scattering_error(dev, &p, &p);
    println!(
        "single scattering: worst relative error {:.2} % ({at})",
        worst * 100.0
    );
    assert!(worst < TOLERANCE, "worst {:.2} % at {at}", worst * 100.0);
}

#[test]
fn positive_control_a_perturbed_composition_disagrees() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let p = AtmosphereBody::earth().derive().expect("earth");
    let mut wrong = p;
    wrong.rayleigh_scattering = wrong.rayleigh_scattering.map(|b| b * 1.3);
    let (worst, _) = single_scattering_error(dev, &wrong, &p);
    assert!(
        worst >= TOLERANCE,
        "tables of a 30 % denser atmosphere must fail the comparison ({:.2} %)",
        worst * 100.0
    );
}

fn zenith(dev: &forge_gpu::GpuDevice, p: &AtmosphereParams, orders: u32) -> ProbeResult {
    let a = GpuAtmosphere::new(
        dev,
        p,
        &AtmosphereSettings {
            orders,
            ..AtmosphereSettings::default()
        },
    )
    .expect("atmosphere");
    let cam = DVec3::new(0.0, 0.0, p.bottom_radius + 2.0);
    let s = 45f64.to_radians();
    let sun = DVec3::new(s.cos(), 0.0, s.sin());
    a.probe(
        dev,
        &[ProbeQuery::Sky {
            camera: CentreOffset(cam),
            view: DVec3::Z,
            sun,
        }],
    )
    .expect("probe")[0]
}

#[test]
fn multiple_scattering_adds_light_and_composition_changes_the_sky() {
    let Some(pool) = common::pool(ROW) else {
        return;
    };
    let dev = pool.primary();
    let earth = AtmosphereBody::earth().derive().expect("earth");
    let one = zenith(dev, &earth, 1);
    let four = zenith(dev, &earth, 4);
    println!(
        "earth zenith, 1 order {:?}, 4 orders {:?}",
        one.radiance, four.radiance
    );
    for k in 0..3 {
        let r = four.radiance[k] / one.radiance[k];
        assert!(r > 1.02 && r < 2.0, "channel {k}: multiple / single = {r}");
    }
    // Earth's noon sky is blue.
    assert!(four.radiance[2] > 1.5 * four.radiance[0]);
    // Mars' dust (blue-absorbing) makes a butterscotch sky; its thin CO2 alone is nearly
    // black: the same code, a different composition.
    let mars_body = AtmosphereBody::mars();
    let mars = zenith(dev, &mars_body.derive().expect("mars"), 4);
    println!("mars zenith {:?}", mars.radiance);
    assert!(
        mars.radiance[0] > 1.3 * mars.radiance[2] && four.radiance[0] < 0.5 * four.radiance[2],
        "dusty Mars is red where Earth is blue: {:?} vs {:?}",
        mars.radiance,
        four.radiance
    );
    let clear_mars = AtmosphereBody {
        aerosols: forge_sky::Aerosols::none(),
        ..mars_body
    };
    let clear = clear_mars.derive().expect("clear mars");
    let thin = zenith(dev, &clear, 4);
    println!("dust-free mars zenith {:?}", thin.radiance);
    assert!(
        thin.radiance[2] < four.radiance[2] * 0.05,
        "610 Pa of CO2 scatters two orders of magnitude less than Earth's air: {:?}",
        thin.radiance
    );
    assert!(dev.take_uncaptured_errors().is_empty());
}

//! Reference scenes: the determinism corpus (`tests/test_phys_determinism.rs`) and the budget
//! scene the perf gate steps (`phys.step.*` in `tests/perf/budgets.ron`). Each is a pure
//! function of its arguments: the same call builds the same world, body for body.

use forge_frames::{DQuat, DVec3, FrameId, FramePos};

use crate::types::{
    AxisMotion, BodyDesc, BodyKind, ColliderDesc, JointDesc, JointKind, Layers, Material, Shape,
    ZoneDesc, ZoneEffect, ZoneShape,
};
use crate::types::{BodyId, BodyState};
use crate::{PhysError, PhysicsSettings, PhysicsWorld};

/// The frame the reference scenes are built in.
pub const FRAME: FrameId = FrameId(3);

fn at(x: f64, y: f64, z: f64) -> FramePos {
    FramePos::new(FRAME, DVec3::new(x, y, z))
}

/// SplitMix64: the scenes' only source of variety, a pure function of the index.
fn mix(i: u64) -> u64 {
    let mut z = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform in [0, 1) from integer bits.
fn unit(i: u64) -> f64 {
    (mix(i) >> 11) as f64 / (1u64 << 53) as f64
}

fn shape_of(i: u64) -> Shape {
    let s = 0.2 + 0.2 * unit(i * 7 + 1);
    match mix(i) % 5 {
        0 => Shape::Sphere { radius: s },
        1 => Shape::Cuboid {
            half_extents: DVec3::new(s, 0.8 * s, 1.1 * s),
        },
        2 => Shape::Capsule {
            half_height: s,
            radius: 0.6 * s,
        },
        3 => Shape::Cylinder {
            half_height: 0.7 * s,
            radius: s,
        },
        _ => Shape::ConvexHull {
            vertices: (0..10)
                .map(|k| {
                    let j = i * 31 + k;
                    DVec3::new(unit(j * 3), unit(j * 3 + 1), unit(j * 3 + 2)) * (2.0 * s)
                        - DVec3::splat(s)
                })
                .collect(),
        },
    }
}

/// A static ground: a height field of gentle waves (the level) under a 40 m floor box.
fn level(w: &mut PhysicsWorld) -> Result<(), PhysError> {
    let g = w.add_body(&BodyDesc::fixed(at(0.0, -0.5, 0.0)))?;
    w.add_collider(
        g,
        &ColliderDesc::new(Shape::Cuboid {
            half_extents: DVec3::new(40.0, 0.5, 40.0),
        }),
    )?;
    let (rows, cols) = (17u32, 17u32);
    let heights = (0..rows)
        .flat_map(|r| {
            (0..cols).map(move |c| {
                let (x, z) = (f64::from(c) / 16.0, f64::from(r) / 16.0);
                0.5 * (x * (1.0 - x) + z * (1.0 - z))
            })
        })
        .collect();
    let hf = w.add_body(&BodyDesc::fixed(at(20.0, 0.0, 20.0)))?;
    w.add_collider(
        hf,
        &ColliderDesc::new(Shape::HeightField {
            rows,
            cols,
            heights,
            size: DVec3::new(16.0, 2.0, 16.0),
        }),
    )?;
    Ok(())
}

/// The determinism corpus on `backend`: ~120 mixed bodies (every dynamic collider kind,
/// materials, layers, a trigger) dropped onto a floor and a height field, a chain of joints
/// (fixed, hinge, slider, spring, cone-twist and a 6DOF both backends build), a kinematic
/// platform, a gravity zone and a damping zone. `nudge` perturbs the first body's start by
/// that many ulps (the determinism guard's positive control). Returns the world and the
/// platform, which [`drive`] moves.
pub fn corpus(backend: &str, nudge: u64) -> Result<(PhysicsWorld, BodyId), PhysError> {
    let mut w = PhysicsWorld::first_party(backend, FRAME, PhysicsSettings::default())?;
    level(&mut w)?;
    for i in 0..120u64 {
        let (gx, gz) = ((i % 10) as f64, (i / 10 % 4) as f64);
        let y = 1.0 + (i / 40) as f64 * 1.3 + 0.1 * unit(i * 5 + 3);
        let mut x = -6.0 + gx * 1.3;
        if i == 0 {
            x = f64::from_bits(x.to_bits() + nudge);
        }
        let mut b = BodyDesc::dynamic(at(x, y, -2.0 + gz * 1.3));
        b.rotation = DQuat::from_axis_angle_checked(DVec3::new(0.3, 1.0, 0.2), unit(i * 11) * 3.0)
            .unwrap_or(DQuat::IDENTITY);
        b.angular_velocity = DVec3::new(0.0, unit(i * 13) - 0.5, 0.0);
        let id = w.add_body(&b)?;
        let mut c = ColliderDesc::new(shape_of(i));
        c.material = Material {
            friction: 0.3 + 0.6 * unit(i * 17),
            restitution: 0.3 * unit(i * 19),
            ..Material::default()
        };
        if i % 23 == 0 {
            c.layers = Layers {
                memberships: 2,
                filters: 1,
            };
        }
        w.add_collider(id, &c)?;
    }
    // A trigger over part of the pile.
    let t = w.add_body(&BodyDesc::fixed(at(0.0, 1.0, 0.0)))?;
    w.add_collider(
        t,
        &ColliderDesc::sensor(Shape::Cuboid {
            half_extents: DVec3::new(2.0, 1.0, 2.0),
        }),
    )?;
    // A hanging chain: one link per joint kind, from a static hook.
    let hook = w.add_body(&BodyDesc::fixed(at(10.0, 8.0, 0.0)))?;
    let kinds = [
        JointKind::Fixed,
        JointKind::Hinge {
            limits: Some((-0.8, 0.8)),
        },
        JointKind::ConeTwist {
            swing: 0.5,
            twist: (-0.3, 0.3),
        },
        JointKind::Spring {
            rest_length: 0.8,
            stiffness: 4000.0,
            damping: 40.0,
        },
        JointKind::Slider {
            limits: Some((-0.3, 0.3)),
        },
        JointKind::SixDof {
            axes: [
                AxisMotion::Locked,
                AxisMotion::Locked,
                AxisMotion::Locked,
                AxisMotion::Limited {
                    min: -0.4,
                    max: 0.4,
                },
                AxisMotion::Locked,
                AxisMotion::Locked,
            ],
        },
    ];
    let mut prev = hook;
    for (k, kind) in kinds.iter().enumerate() {
        let y = 7.0 - k as f64;
        let mut b = BodyDesc::dynamic(at(10.0 + 0.3 * k as f64, y, 0.0));
        b.linear_velocity = DVec3::new(1.5, 0.0, 0.5);
        let link = w.add_body(&b)?;
        w.add_collider(
            link,
            &ColliderDesc::new(Shape::Cuboid {
                half_extents: DVec3::new(0.2, 0.3, 0.2),
            }),
        )?;
        let mut j = JointDesc::new(prev, link, *kind);
        j.anchor_a = if prev == hook {
            DVec3::ZERO
        } else {
            DVec3::new(0.0, -0.5, 0.0)
        };
        j.anchor_b = DVec3::new(0.0, 0.5, 0.0);
        j.axis = DVec3::new(0.0, 0.0, 1.0);
        w.add_joint(&j)?;
        prev = link;
    }
    // A kinematic platform (driven by the caller's targets or at rest) and the zones.
    let p = w.add_body(&BodyDesc::new(BodyKind::Kinematic, at(-10.0, 0.5, 5.0)))?;
    w.add_collider(
        p,
        &ColliderDesc::new(Shape::Cuboid {
            half_extents: DVec3::new(2.0, 0.2, 2.0),
        }),
    )?;
    w.add_zone(&ZoneDesc::new(
        at(-6.0, 3.0, 2.0),
        ZoneShape::Sphere { radius: 2.5 },
        ZoneEffect::Gravity {
            acceleration: DVec3::new(0.0, 4.0, 0.0),
        },
    ))?;
    w.add_zone(&ZoneDesc::new(
        at(2.0, 1.0, 1.0),
        ZoneShape::Box {
            half_extents: DVec3::new(2.0, 2.0, 2.0),
        },
        ZoneEffect::Damping {
            linear: 3.0,
            angular: 3.0,
        },
    ))?;
    Ok((w, p))
}

/// Step `i` of the corpus's script, then the step: the platform circles (a kinematic
/// target every step), step 30 kicks a body, step 90 teleports one, step 150 removes one.
pub fn drive(w: &mut PhysicsWorld, platform: BodyId, i: u64) -> Result<(), PhysError> {
    let a = i as f64 * 0.05;
    let (s, c) = (forge_num::det::sin(a), forge_num::det::cos(a));
    w.set_kinematic_target(
        platform,
        at(-10.0 + 2.0 * c, 0.5, 5.0 + 2.0 * s),
        DQuat::from_axis_angle(DVec3::Y, a),
    )?;
    match i {
        30 => w.apply_impulse(
            BodyId(12),
            DVec3::new(40.0, 80.0, 0.0),
            DVec3::new(0.0, 5.0, 0.0),
        )?,
        90 => w.set_state(
            BodyId(20),
            &BodyState {
                position: at(3.0, 6.0, -1.0),
                rotation: DQuat::IDENTITY,
                linear_velocity: DVec3::new(0.0, -2.0, 1.0),
                angular_velocity: DVec3::new(1.0, 0.0, 0.0),
            },
        )?,
        150 => w.remove_body(BodyId(33))?,
        _ => {}
    }
    w.step()
}

/// The budget scene (`phys.step.<backend>`): `n` dynamic bodies — spheres, boxes and
/// capsules in a loose 3D grid — falling onto a floor, so a measured step includes broad
/// phase, contact generation and solving for a busy pile. `settle` steps are run first.
pub fn pile(backend: &str, n: u32, settle: u32) -> Result<PhysicsWorld, PhysError> {
    let mut w = PhysicsWorld::first_party(backend, FRAME, PhysicsSettings::default())?;
    let g = w.add_body(&BodyDesc::fixed(at(0.0, -0.5, 0.0)))?;
    w.add_collider(
        g,
        &ColliderDesc::new(Shape::Cuboid {
            half_extents: DVec3::new(60.0, 0.5, 60.0),
        }),
    )?;
    let side = 20u32;
    for i in 0..n {
        let (gx, gz, gy) = (i % side, i / side % side, i / (side * side));
        let p = at(
            f64::from(gx) * 1.1 - 11.0,
            0.6 + f64::from(gy) * 1.1,
            f64::from(gz) * 1.1 - 11.0,
        );
        let id = w.add_body(&BodyDesc::dynamic(p))?;
        let shape = match i % 3 {
            0 => Shape::Sphere { radius: 0.45 },
            1 => Shape::Cuboid {
                half_extents: DVec3::splat(0.4),
            },
            _ => Shape::Capsule {
                half_height: 0.25,
                radius: 0.25,
            },
        };
        w.add_collider(id, &ColliderDesc::new(shape))?;
    }
    for _ in 0..settle {
        w.step()?;
    }
    Ok(w)
}

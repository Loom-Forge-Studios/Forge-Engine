//! avian3d's generic 6DOF joint (ADR 0066): an XPBD constraint on avian's own custom-constraint
//! API, for the axis combinations none of avian's stock joints express.
//!
//! It measures what rapier3d's generic joint measures, so a 6DOF joint means the same on both
//! backends: B's joint frame relative to A's, the linear axes as the anchor offset along A's
//! frame axes and the angular axes as the angle about each of A's frame axes
//! (`2 atan2(q_i, q_w)` of the relative rotation, `q_w >= 0`). A locked axis is held at zero, a
//! limited one inside its range, a free one is left alone. Each substep solves the three
//! angular rows one after another (each reads the rotation the previous one left), then one
//! positional correction for the linear rows, as avian's own prismatic joint does.

use avian3d::dynamics::joints::EntityConstraint;
use avian3d::dynamics::solver::solver_body::{SolverBody, SolverBodyInertia};
use avian3d::dynamics::solver::xpbd::{
    AngularConstraint, PositionConstraint, XpbdConstraint, XpbdConstraintSolverData,
    compute_lagrange_update,
};
use avian3d::math::{Quaternion, Scalar, Vector};
use avian3d::prelude::*;
use bevy_ecs::component::Component;
use bevy_ecs::entity::{Entity, EntityMapper, MapEntities};

use crate::types::AxisMotion;

/// The generic 6DOF joint between two bodies.
#[derive(Component, Clone, Copy, Debug)]
#[require(SixDofSolverData)]
pub(super) struct SixDofJoint {
    pub body1: Entity,
    pub body2: Entity,
    /// The joint frames in each body's local space (the basis's x is the joint axis).
    pub anchor1: Vector,
    pub anchor2: Vector,
    pub basis1: Quaternion,
    pub basis2: Quaternion,
    /// Linear x, y, z then angular x, y, z: the allowed range, `None` when free.
    pub ranges: [Option<(Scalar, Scalar)>; 6],
}

impl SixDofJoint {
    pub fn ranges(axes: &[AxisMotion; 6]) -> [Option<(Scalar, Scalar)>; 6] {
        axes.map(|m| match m {
            AxisMotion::Locked => Some((0.0, 0.0)),
            AxisMotion::Free => None,
            AxisMotion::Limited { min, max } => Some((min, max)),
        })
    }
}

/// What `prepare` reads once per step for every substep.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(super) struct SixDofSolverData {
    world_r1: Vector,
    world_r2: Vector,
    center_difference: Vector,
    /// Each joint frame's world rotation at the start of the step.
    frame1: Quaternion,
    frame2: Quaternion,
}

impl XpbdConstraintSolverData for SixDofSolverData {}

impl MapEntities for SixDofJoint {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        self.body1 = mapper.get_mapped(self.body1);
        self.body2 = mapper.get_mapped(self.body2);
    }
}

impl EntityConstraint<2> for SixDofJoint {
    fn entities(&self) -> [Entity; 2] {
        [self.body1, self.body2]
    }
}

impl AngularConstraint for SixDofJoint {}

impl PositionConstraint for SixDofJoint {}

impl XpbdConstraint<2> for SixDofJoint {
    type SolverData = SixDofSolverData;

    fn prepare(&mut self, bodies: [&RigidBodyQueryReadOnlyItem; 2], d: &mut SixDofSolverData) {
        let [b1, b2] = bodies;
        let (r1, r2) = (b1.rotation.0, b2.rotation.0);
        d.world_r1 = r1 * (self.anchor1 - b1.center_of_mass.0);
        d.world_r2 = r2 * (self.anchor2 - b2.center_of_mass.0);
        d.center_difference =
            (b2.position.0 - b1.position.0) + (r2 * b2.center_of_mass.0 - r1 * b1.center_of_mass.0);
        d.frame1 = r1 * self.basis1;
        d.frame2 = r2 * self.basis2;
    }

    fn solve(
        &mut self,
        bodies: [&mut SolverBody; 2],
        inertias: [&SolverBodyInertia; 2],
        d: &mut SixDofSolverData,
        dt: Scalar,
    ) {
        let [b1, b2] = bodies;
        let [i1, i2] = inertias;
        let (ii1, ii2) = (
            i1.effective_inv_angular_inertia(),
            i2.effective_inv_angular_inertia(),
        );
        for axis in 0..3 {
            let Some((lo, hi)) = self.ranges[3 + axis] else {
                continue;
            };
            let f1 = b1.delta_rotation.0 * d.frame1;
            let f2 = b2.delta_rotation.0 * d.frame2;
            let mut r = f1.inverse() * f2;
            if r.w < 0.0 {
                r = -r;
            }
            let angle = 2.0 * forge_num::det::atan2([r.x, r.y, r.z][axis], r.w);
            // The rotation from A's frame to B's that the row removes (avian's sign: body 1
            // turns along it, body 2 against it).
            let excess = angle - angle.clamp(lo, hi);
            if excess != 0.0 {
                let about = f1 * Vector::AXES[axis];
                self.align_orientation(b1, b2, ii1, ii2, excess * about, 0.0, 0.0, dt);
            }
        }
        let r1 = b1.delta_rotation.0 * d.world_r1;
        let r2 = b2.delta_rotation.0 * d.world_r2;
        let separation = (b2.delta_position - b1.delta_position) + (r2 - r1) + d.center_difference;
        let f1 = b1.delta_rotation.0 * d.frame1;
        let local = f1.inverse() * separation;
        let mut correction = Vector::ZERO;
        for axis in 0..3 {
            if let Some((lo, hi)) = self.ranges[axis] {
                correction[axis] = local[axis].clamp(lo, hi) - local[axis];
            }
        }
        let delta_x = f1 * correction;
        let magnitude = delta_x.length();
        if magnitude <= Scalar::EPSILON {
            return;
        }
        let dir = delta_x / magnitude;
        let w1 = PositionConstraint::compute_generalized_inverse_mass(
            self,
            i1.effective_inv_mass().max_element(),
            ii1,
            r1,
            dir,
        );
        let w2 = PositionConstraint::compute_generalized_inverse_mass(
            self,
            i2.effective_inv_mass().max_element(),
            ii2,
            r2,
            dir,
        );
        let delta_lagrange = compute_lagrange_update(0.0, magnitude, &[w1, w2], 0.0, dt);
        self.apply_positional_impulse(b1, b2, i1, i2, delta_lagrange * dir, r1, r2);
    }
}

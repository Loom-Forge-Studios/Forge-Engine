//! 2D particles (Ch.35 §35.2).
//!
//! An [`Emitter`] spawns particles at a rate and in bursts, each with a lifetime, speed and
//! direction drawn from ranges, then moves them under gravity and drag and fades their
//! size and colour over their life. The simulation is a fixed step on the CPU in `f64`
//! (structure of arrays), drawn as sprites through the batcher (one draw call for a whole
//! system on one texture).
//!
//! **Deterministic without a random-number *state***: particle `k`'s properties are the
//! pure function [`Stream::range`]`(seed, k * 8 + field)` of the system's seed and its
//! spawn index — the I3 discipline, so a replay, a rewind or another platform spawns the
//! same particles (I2 corpus row `2d/particles/*`).

use forge_frames::FramePos2;
use forge_num::{DVec2, det};

use crate::Error2d;
use crate::math::{Stream, lerp};
use crate::sprite::{Sprite, TextureId, UvRect};

/// How particles are made and how they change.
#[derive(Clone, Debug, PartialEq)]
pub struct Emitter {
    /// Particles per second.
    pub rate: f64,
    /// `(time since start, count)` bursts (seconds), in time order.
    pub bursts: Vec<(f64, u32)>,
    /// Lifetime range (seconds).
    pub life: (f64, f64),
    /// Speed range (units per second).
    pub speed: (f64, f64),
    /// Mean direction and the spread either side (radians).
    pub direction: f64,
    pub spread: f64,
    pub gravity: DVec2,
    /// Velocity damping per second.
    pub drag: f64,
    /// Size at birth and at death (world units).
    pub size: (f64, f64),
    /// Linear RGBA at birth and at death.
    pub color: ([f64; 4], [f64; 4]),
    /// At most this many alive (spawns beyond it are skipped, still counted).
    pub max: usize,
    /// Spawn inside a circle of this radius around the emitter.
    pub radius: f64,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            rate: 30.0,
            bursts: Vec::new(),
            life: (0.8, 1.2),
            speed: (1.0, 2.0),
            direction: core::f64::consts::FRAC_PI_2,
            spread: 0.4,
            gravity: DVec2::new(0.0, -2.0),
            drag: 0.5,
            size: (0.25, 0.05),
            color: ([1.0, 0.8, 0.3, 1.0], [1.0, 0.2, 0.0, 0.0]),
            max: 1000,
            radius: 0.0,
        }
    }
}

/// A running particle system.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticleSystem {
    pub emitter: Emitter,
    /// Where it emits from.
    pub at: FramePos2,
    pub emitting: bool,
    stream: Stream,
    time: f64,
    /// Particles spawned so far (the next spawn index).
    spawned: u64,
    /// From the rate alone (bursts not counted).
    rate_spawned: u64,
    next_burst: usize,
    // Structure of arrays.
    /// Positions in `at.frame` (absolute: a moving emitter leaves its particles behind).
    pos: Vec<DVec2>,
    vel: Vec<DVec2>,
    age: Vec<f64>,
    life: Vec<f64>,
}

impl ParticleSystem {
    /// A system emitting at `at`, its randomness keyed by `seed` (derive it from a
    /// `forge_seed::Seed`: `seed.child("particles", n).raw()`).
    pub fn new(emitter: Emitter, at: FramePos2, seed: u64) -> Result<Self, Error2d> {
        let e = &emitter;
        let ok = e.rate >= 0.0
            && e.rate.is_finite()
            && e.life.0 > 0.0
            && e.life.1 >= e.life.0
            && e.speed.1 >= e.speed.0
            && e.drag >= 0.0
            && e.gravity.is_finite()
            && e.max > 0;
        if !ok {
            return Err(Error2d::invalid(
                "emitter: rate, drag >= 0; life > 0; ranges ordered; max > 0",
            ));
        }
        Ok(Self {
            emitter,
            at,
            emitting: true,
            stream: Stream::new(seed),
            time: 0.0,
            spawned: 0,
            rate_spawned: 0,
            next_burst: 0,
            pos: Vec::new(),
            vel: Vec::new(),
            age: Vec::new(),
            life: Vec::new(),
        })
    }

    /// Particles alive.
    #[must_use]
    pub fn alive(&self) -> usize {
        self.age.len()
    }

    /// Particles spawned so far.
    #[must_use]
    pub fn spawned(&self) -> u64 {
        self.spawned
    }

    fn spawn(&mut self) {
        let k = self.spawned;
        self.spawned += 1;
        if self.age.len() >= self.emitter.max {
            return;
        }
        let e = &self.emitter;
        let s = self.stream;
        let base = k * 8;
        let angle = e.direction + s.range(base, -e.spread, e.spread);
        let speed = s.range(base + 1, e.speed.0, e.speed.1);
        let life = s.range(base + 2, e.life.0, e.life.1);
        let (c, sn) = (det::cos(angle), det::sin(angle));
        let r = e.radius * det::sqrt(s.unit(base + 3));
        let ra = s.range(base + 4, 0.0, core::f64::consts::TAU);
        let start = DVec2::new(r * det::cos(ra), r * det::sin(ra));
        self.pos.push(self.at.local + start);
        self.vel.push(DVec2::new(c * speed, sn * speed));
        self.age.push(0.0);
        self.life.push(life);
    }

    /// Spawn `n` particles now (a landing's puff of dust, a pickup's sparkle), from the
    /// emitter's current position; they take the next spawn indices, so a replay spawns the
    /// same ones.
    pub fn burst(&mut self, n: u32) {
        for _ in 0..n {
            self.spawn();
        }
    }

    /// Advance by `dt` seconds (a fixed step: call it with the simulation's step).
    pub fn step(&mut self, dt: f64) {
        if dt.is_nan() || dt <= 0.0 {
            return;
        }
        self.time += dt;
        if self.emitting {
            let due = (self.emitter.rate * self.time).floor() as u64;
            while self.rate_spawned < due {
                self.rate_spawned += 1;
                self.spawn();
            }
            while let Some(&(t, n)) = self.emitter.bursts.get(self.next_burst) {
                if t > self.time {
                    break;
                }
                self.next_burst += 1;
                for _ in 0..n {
                    self.spawn();
                }
            }
        }
        let g = self.emitter.gravity;
        let damp = 1.0 / (1.0 + dt * self.emitter.drag);
        // One compacting pass: order-preserving, so draw order is stable frame to frame.
        let mut w = 0;
        for i in 0..self.age.len() {
            let age = self.age[i] + dt;
            if age >= self.life[i] {
                continue;
            }
            let v = (self.vel[i] + g * dt) * damp;
            self.age[w] = age;
            self.life[w] = self.life[i];
            self.vel[w] = v;
            self.pos[w] = self.pos[i] + v * dt;
            w += 1;
        }
        self.age.truncate(w);
        self.life.truncate(w);
        self.vel.truncate(w);
        self.pos.truncate(w);
    }

    /// Sprites for the live particles.
    pub fn sprites(
        &self,
        texture: TextureId,
        uv: UvRect,
        layer: i32,
        order: i32,
        out: &mut Vec<Sprite>,
    ) {
        let e = &self.emitter;
        for i in 0..self.age.len() {
            let f = (self.age[i] / self.life[i]).clamp(0.0, 1.0);
            let size = lerp(e.size.0, e.size.1, f);
            let mut tint = [0.0; 4];
            for (k, t) in tint.iter_mut().enumerate() {
                *t = lerp(e.color.0[k], e.color.1[k], f);
            }
            out.push(Sprite {
                pos: FramePos2::new(self.at.frame, self.pos[i]),
                size: DVec2::new(size, size),
                pivot: DVec2::new(0.5, 0.5),
                angle: 0.0,
                uv,
                texture,
                tint,
                layer,
                order,
                flip_x: false,
                flip_y: false,
            });
        }
    }

    /// A bit-exact digest of the live particles (positions, velocities, ages).
    #[must_use]
    pub fn state_bits(&self) -> Vec<u64> {
        let mut v = Vec::with_capacity(self.age.len() * 5 + 1);
        v.push(self.spawned);
        for i in 0..self.age.len() {
            v.extend_from_slice(&self.pos[i].to_bits());
            v.extend_from_slice(&self.vel[i].to_bits());
            v.push(self.age[i].to_bits());
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_frames::FrameId;

    fn sys(seed: u64) -> ParticleSystem {
        let e = Emitter {
            rate: 100.0,
            bursts: vec![(0.5, 50)],
            ..Emitter::default()
        };
        ParticleSystem::new(e, FramePos2::new(FrameId(0), DVec2::ZERO), seed).expect("system")
    }

    #[test]
    fn rate_and_bursts_spawn_exact_counts() {
        let mut s = sys(1);
        for _ in 0..60 {
            s.step(1.0 / 60.0);
        }
        assert_eq!(s.spawned(), 100 + 50);
        assert!(s.alive() <= 150 && s.alive() > 50);
    }

    #[test]
    fn particles_die_at_the_end_of_their_life_and_fall() {
        let mut s = sys(2);
        s.emitting = false;
        s.emitter.bursts = vec![(0.0, 10)];
        s.emitter.rate = 0.0;
        s.emitting = true;
        s.step(1.0 / 60.0);
        s.emitting = false;
        let mut out = Vec::new();
        s.sprites(TextureId(0), UvRect::FULL, 0, 0, &mut out);
        assert_eq!(out.len(), 10);
        for _ in 0..90 {
            s.step(1.0 / 60.0);
        }
        assert_eq!(s.alive(), 0, "every life is at most 1.2 s");
    }

    #[test]
    fn a_seed_fixes_every_particle_and_another_seed_changes_them() {
        let run = |seed| {
            let mut s = sys(seed);
            for _ in 0..120 {
                s.step(1.0 / 60.0);
            }
            s.state_bits()
        };
        assert_eq!(run(7), run(7));
        assert_ne!(run(7), run(8));
    }
}

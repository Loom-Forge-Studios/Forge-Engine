//! The 2D sample game (Ch.35 §35.3, DoD M4-12) — **Spike S13's acceptance test**: a small
//! platformer with tilemaps, cutout animation, 2D lights and a gamepad, built only from
//! `forge-2d`'s public API, with menus, a HUD and credits on the game-UI runtime
//! (`forge-runtime`, WP-U11), playable in the editor (play-in-editor, [`pie`]) and shipped as
//! an exported build whose binary links no 3D-only crate
//! (`tests/preset/test_2d_tax_is_zero.rs`).
//!
//! * **Tilemaps**: the level is an autotiled blob-47 tile map (`forge_2d::scenes::level`);
//!   its solid cells are the physics terrain and the lights' shadow casters.
//! * **Cutout animation**: the hero is a `Skeleton2D` rig (hip, torso, head, two-bone legs)
//!   playing its walk clip in step with its speed, mirrored when it turns, feet planted by
//!   two-bone IK when it stands.
//! * **2D lights**: dusk ambient, a lantern that follows the hero and lamps along the level,
//!   all shadowed by the terrain; the bricks behind the start are normal-mapped.
//! * **A gamepad**: input arrives as `forge_ui::game::PadInput` from a
//!   `forge_ui::game::GamepadSource` (the platform HAL's seam, Ch.27): the left stick or
//!   the d-pad runs, South jumps, Start pauses. The window maps the keyboard onto the same
//!   inputs; a device backend is the HAL's (not built: `forge-play`, M5-8), and the tests
//!   drive the game with `ScriptedPad`.
//! * **Menus and credits** ([`session`], feature `app`): the runtime's main menu, settings,
//!   pause menu, HUD (coins as the score) and credits with the E-64 engine entry, every
//!   string a localisation key; a level-clear banner.
//! * Also: parallax hills, dust particles, coins to collect, a pixel-perfect 320x180 camera
//!   at a whole-number scale.
//!
//! The simulation is a fixed 60 Hz step in `f64` with deterministic trigonometry: a
//! recorded input sequence replays to the same bits on every platform (the I2 corpus row
//! `2d/sample-game/run`; `tests/test_sample_game.rs` pins the scripted run's fingerprint).
//!
//! Features: the core (this module) needs neither a GPU nor a window; `app` (default) adds
//! the menus, the render path and the binary; `pie` adds the editor's play backend and is
//! never part of an exported build.

#![forbid(unsafe_code)]

#[cfg(feature = "pie")]
pub mod pie;
#[cfg(feature = "app")]
pub mod session;
#[cfg(feature = "app")]
pub mod view;

use forge_2d::anim::FrameAnim;
use forge_2d::atlas::Atlas;
use forge_2d::camera::Camera2d;
use forge_2d::light::{Light2d, Occluder};
use forge_2d::math::Xform2;
use forge_2d::parallax::ParallaxLayer;
use forge_2d::particles::{Emitter, ParticleSystem};
use forge_2d::physics::{BodyDef, BodyKind, ColliderDef, PhysicsWorld2d, RigidBodyId, Shape};
use forge_2d::scenes::{self, SceneTextures};
use forge_2d::skeleton::{Clip, Part, Skeleton2d};
use forge_2d::sprite::{Frame2d, Sprite, UvRect};
use forge_2d::tilemap::Tilemap;
use forge_2d::{DVec2, Error2d, FrameId, FramePos2};
use forge_ui::game::{PadButton, PadInput};

/// The fixed step.
pub const DT: f64 = 1.0 / 60.0;
/// The reference resolution the game is drawn at (pixel art, 16 px a unit).
pub const REFERENCE: (u32, u32) = (320, 180);
const PPU: f64 = 16.0;
/// Where the level ends (world x).
pub const GOAL_X: f64 = 60.0;
const RUN_SPEED: f64 = 6.0;
/// Clears a 3-unit wall: apex `v^2 / 2g` = 13^2 / (2 x 24.5) = 3.45 units.
const JUMP_SPEED: f64 = 13.0;
const GRAVITY_SCALE: f64 = 2.5;
/// Stick values inside this are no input.
const DEAD_ZONE: f64 = 0.2;

/// The pad as the game reads it: the left stick's x and whether South is held (the d-pad
/// counts as a full stick deflection).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadState {
    pub x: f64,
    pub jump: bool,
    left: bool,
    right: bool,
    stick: f64,
}

impl PadState {
    /// Fold one pad input in.
    pub fn apply(&mut self, i: PadInput) {
        match i {
            PadInput::Stick { x, .. } => self.stick = f64::from(x),
            PadInput::Button { button, pressed } => match button {
                PadButton::South => self.jump = pressed,
                PadButton::DPadLeft => self.left = pressed,
                PadButton::DPadRight => self.right = pressed,
                _ => {}
            },
        }
        let dpad = f64::from(u8::from(self.right)) - f64::from(u8::from(self.left));
        self.x = if dpad != 0.0 { dpad } else { self.stick };
    }
}

/// A coin.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Coin {
    at: DVec2,
    taken: bool,
}

/// The game (see the module docs).
#[derive(Clone, Debug)]
pub struct Game {
    map: Tilemap,
    world: PhysicsWorld2d,
    hero: RigidBodyId,
    skeleton: Skeleton2d,
    parts: Vec<Part>,
    walk: Clip,
    /// Walk-cycle time (advances with the distance covered).
    walk_us: u64,
    facing: f64,
    grounded: bool,
    jump_held: bool,
    dust: ParticleSystem,
    coins: Vec<Coin>,
    lamps: Vec<DVec2>,
    occluders: Vec<Occluder>,
    pub steps: u64,
    pub coins_taken: u32,
    pub won: bool,
}

fn frame() -> FrameId {
    FrameId(0)
}

fn at(x: f64, y: f64) -> FramePos2 {
    FramePos2::new(frame(), DVec2::new(x, y))
}

impl Game {
    /// A new game at the start of the level.
    pub fn new() -> Result<Self, Error2d> {
        let map = scenes::level(forge_2d::sprite::TextureId(0))?;
        let mut world = scenes::level_physics(&map)?;
        // Walls at both ends of the level.
        let walls = world.add_body(&BodyDef::new(BodyKind::Static, at(0.0, 0.0)))?;
        for x in [-40.5, 80.5] {
            let mut c = ColliderDef::new(Shape::Box {
                half: DVec2::new(0.5, 20.0),
            });
            c.offset = DVec2::new(x, 10.0);
            world.add_collider(walls, c)?;
        }
        let hero = world.add_body(&BodyDef {
            lock_rotation: true,
            gravity_scale: GRAVITY_SCALE,
            ..BodyDef::new(BodyKind::Dynamic, at(-2.0, 0.0))
        })?;
        let mut c = ColliderDef::new(Shape::Capsule {
            half_height: 0.35,
            radius: 0.3,
        });
        c.friction = 0.0;
        world.add_collider(hero, c)?;
        let occluders = map
            .solid_boxes()
            .into_iter()
            .map(|b| Occluder::rect(frame(), b.min + map.origin.local, b.max + map.origin.local))
            .collect();
        let (skeleton, parts, walk) = scenes::cutout();
        let dust = ParticleSystem::new(
            Emitter {
                rate: 0.0,
                life: (0.3, 0.6),
                speed: (0.5, 1.5),
                spread: 1.2,
                gravity: DVec2::new(0.0, -3.0),
                size: (0.15, 0.03),
                color: ([0.8, 0.75, 0.6, 0.9], [0.8, 0.75, 0.6, 0.0]),
                max: 200,
                ..Emitter::default()
            },
            at(0.0, 0.0),
            forge_seed::Seed::root(0x5A_4D1E).child("dust", 0).raw(),
        )?;
        let coins = (0..12)
            .map(|i| Coin {
                at: DVec2::new(
                    1.0 + 4.5 * f64::from(i),
                    -2.3 + if i % 3 == 0 { 1.5 } else { 0.0 },
                ),
                taken: false,
            })
            .collect();
        let lamps = (0..6)
            .map(|i| DVec2::new(-6.0 + 12.0 * f64::from(i), -1.5))
            .collect();
        Ok(Self {
            map,
            world,
            hero,
            skeleton,
            parts,
            walk,
            walk_us: 0,
            facing: 1.0,
            grounded: false,
            jump_held: false,
            dust,
            coins,
            lamps,
            occluders,
            steps: 0,
            coins_taken: 0,
            won: false,
        })
    }

    /// The hero's position.
    pub fn hero(&self) -> Result<FramePos2, Error2d> {
        self.world.position(self.hero)
    }

    /// Is the hero standing on something (a contact whose normal points up)?
    #[must_use]
    pub fn grounded(&self) -> bool {
        self.grounded
    }

    /// One fixed step with the pad as it is now.
    pub fn step(&mut self, pad: &PadState) -> Result<(), Error2d> {
        let x = if pad.x.abs() < DEAD_ZONE {
            0.0
        } else {
            pad.x.clamp(-1.0, 1.0)
        };
        let v = self.world.linear_velocity(self.hero)?;
        // Run: steer toward the target speed (snappier on the ground than in the air).
        let target = x * RUN_SPEED;
        let accel = if self.grounded { 0.35 } else { 0.12 };
        let mut vx = v.x + (target - v.x) * accel;
        if x == 0.0 && self.grounded {
            vx *= 0.6;
        }
        let mut vy = v.y;
        let pressed = pad.jump && !self.jump_held;
        self.jump_held = pad.jump;
        if pressed && self.grounded {
            vy = JUMP_SPEED;
            self.burst_dust()?;
        }
        self.world
            .set_linear_velocity(self.hero, DVec2::new(vx, vy))?;
        self.world.step();
        let was = self.grounded;
        self.grounded = self
            .world
            .touches(self.hero)
            .iter()
            .any(|t| !t.sensor && t.normal.y > 0.7);
        if self.grounded && !was {
            self.burst_dust()?;
        }
        if x != 0.0 {
            self.facing = x.signum();
        }
        let p = self.world.position(self.hero)?.local;
        let speed = self.world.linear_velocity(self.hero)?.x.abs();
        // The walk cycle advances with distance: one clip per 1.6 units.
        let dt_us = (speed * DT / 1.6 * self.walk.duration_us as f64).round() as u64;
        self.walk_us = if self.grounded && speed > 0.2 {
            self.walk_us + dt_us
        } else {
            0
        };
        self.dust.at = FramePos2::new(frame(), p + DVec2::new(0.0, -0.65));
        self.dust.step(DT);
        for c in &mut self.coins {
            if !c.taken && (c.at - p).length() < 0.8 {
                c.taken = true;
                self.coins_taken += 1;
            }
        }
        if p.x >= GOAL_X {
            self.won = true;
        }
        self.steps += 1;
        Ok(())
    }

    fn burst_dust(&mut self) -> Result<(), Error2d> {
        let p = self.world.position(self.hero)?.local;
        self.dust.at = FramePos2::new(frame(), p + DVec2::new(0.0, -0.65));
        self.dust.burst(8);
        Ok(())
    }

    /// A bit-exact digest of the simulation (hero, contacts, particles, coins).
    #[must_use]
    pub fn state_bits(&self) -> Vec<u64> {
        let mut v = self.world.state_bits();
        v.extend(self.dust.state_bits());
        v.push(self.walk_us);
        v.push(u64::from(self.coins_taken));
        v.push(self.steps);
        v
    }

    /// The hero's rig posed for this moment, in world space.
    fn pose(&self) -> Result<Vec<Xform2>, Error2d> {
        let p = self.world.position(self.hero)?.local;
        let root = Xform2 {
            translation: p + DVec2::new(0.0, -0.05),
            rot: forge_2d::math::Rot2::IDENTITY,
            scale: DVec2::new(self.facing * 0.9, 0.9),
        };
        let mut pose = self.skeleton.sample(&self.walk, self.walk_us);
        if self.grounded && self.walk_us == 0 {
            // Standing: plant each foot straight below its hip with two-bone IK.
            for (thigh, shin, dx) in [(3usize, 4usize, -0.12), (5, 6, 0.12)] {
                let foot = p + DVec2::new(dx * self.facing, -0.62);
                self.skeleton.two_bone_ik(
                    &mut pose,
                    &root,
                    thigh,
                    shin,
                    foot,
                    self.facing < 0.0,
                )?;
            }
        }
        Ok(self.skeleton.world(&pose, &root))
    }

    /// Everything to draw this frame.
    pub fn frame(&self, t: &SceneTextures, atlas: &Atlas) -> Result<Frame2d, Error2d> {
        let p = self.world.position(self.hero)?.local;
        let cam_x = p.x.clamp(-30.0, GOAL_X + 5.0);
        let mut cam = Camera2d::pixel_perfect(at(cam_x, (p.y + 1.5).max(-1.0)), REFERENCE, PPU);
        cam.snap = true;
        let mut f = Frame2d::new(cam);
        f.clear = [0.18, 0.2, 0.35, 1.0];
        f.ambient = [0.24, 0.23, 0.34];
        let half = cam.half_extent(REFERENCE.0, REFERENCE.1);
        let c = cam.center().local;
        let view =
            forge_2d::math::Aabb2::new(c - half - DVec2::splat(1.0), c + half + DVec2::splat(1.0));
        let local_view = forge_2d::math::Aabb2::new(
            view.min - self.map.origin.local,
            view.max - self.map.origin.local,
        );
        let first_tile = f.sprites.len();
        self.map.sprites(&local_view, &mut f.sprites);
        // The map was built before the renderer held its tile set: draw with that texture.
        for s in &mut f.sprites[first_tile..] {
            s.texture = t.tiles;
        }
        // Parallax hills.
        f.parallax
            .insert(-10, ParallaxLayer::new(0.3, 0.5).repeating(4.0, 0.0));
        let r = atlas.regions[&scenes::ATLAS_HILLS];
        let mut hills = Sprite::new(at(0.0, -1.0), DVec2::new(4.0, 2.0), t.atlas).at_layer(-10, 0);
        hills.uv = UvRect::from_region(&r, atlas.opts.page_w, atlas.opts.page_h);
        f.sprites.push(hills);
        // Normal-mapped bricks behind the start.
        for i in 0..3 {
            f.sprites.push(
                Sprite::new(
                    at(-8.0 + 2.0 * f64::from(i), -3.0),
                    DVec2::new(2.0, 2.0),
                    t.bricks,
                )
                .at_layer(-1, 0),
            );
        }
        // Coins.
        let cr = atlas.regions[&scenes::ATLAS_COIN];
        for coin in self.coins.iter().filter(|c| !c.taken) {
            let mut s = Sprite::new(
                FramePos2::new(frame(), coin.at),
                DVec2::new(0.75, 0.75),
                t.atlas,
            )
            .at_layer(4, 0);
            s.uv = UvRect::from_region(&cr, atlas.opts.page_w, atlas.opts.page_h);
            f.sprites.push(s);
        }
        // The hero: cutout parts on the white texture, tinted.
        let world = self.pose()?;
        let mut parts = self.parts.clone();
        for part in &mut parts {
            part.texture = t.white;
        }
        let mut figure = Skeleton2d::sprites(&parts, &world, at(0.0, 0.0), 5, 0);
        let colors = [
            [0.85, 0.2, 0.25, 1.0],
            [0.95, 0.8, 0.65, 1.0],
            [0.2, 0.25, 0.6, 1.0],
            [0.15, 0.2, 0.5, 1.0],
            [0.2, 0.25, 0.6, 1.0],
            [0.15, 0.2, 0.5, 1.0],
        ];
        for (s, c) in figure.iter_mut().zip(colors) {
            s.tint = c;
        }
        f.sprites.extend(figure);
        self.dust
            .sprites(t.white, UvRect::FULL, 6, 0, &mut f.sprites);
        // Lights: the hero's lantern and the lamps.
        let mut lantern = Light2d::point(
            FramePos2::new(frame(), p + DVec2::new(0.3 * self.facing, 0.4)),
            7.0,
            1.6,
        );
        lantern.color = [1.0, 0.85, 0.55];
        lantern.height = 1.2;
        f.lights.push(lantern);
        for l in &self.lamps {
            let mut lamp = Light2d::point(FramePos2::new(frame(), *l), 6.0, 1.2);
            lamp.color = [0.55, 0.75, 1.0];
            f.lights.push(lamp);
        }
        f.occluders.clone_from(&self.occluders);
        Ok(f)
    }

    /// The walk animation of the hero's sheet (the atlas's 4-frame walk), for the HUD icon.
    pub fn hud_anim() -> Result<FrameAnim, Error2d> {
        scenes::hero_walk()
    }
}

/// A scripted run through the level: hold right, jump every 0.4 s. Returns the inputs per
/// step, as a pad would deliver them.
#[must_use]
pub fn demo_script(steps: u64) -> Vec<Vec<PadInput>> {
    (0..steps)
        .map(|i| {
            let mut v = Vec::new();
            if i == 0 {
                v.push(PadInput::Stick { x: 1.0, y: 0.0 });
            }
            match i % 24 {
                0 => v.push(PadInput::Button {
                    button: PadButton::South,
                    pressed: true,
                }),
                8 => v.push(PadInput::Button {
                    button: PadButton::South,
                    pressed: false,
                }),
                _ => {}
            }
            v
        })
        .collect()
}

/// How many fixed steps the scripted run lasts (15 s): long enough to clear the level.
pub const SCRIPT_STEPS: usize = 900;

/// A run's fingerprint: FNV-1a over [`Game::state_bits`] folded in every 30 steps — the
/// whole trajectory, not only where it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint(pub u64);

impl Default for Fingerprint {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fingerprint {
    /// Fold `game`'s state in if `step` (0-based, just taken) closes a 30-step window.
    pub fn observe(&mut self, step: u64, game: &Game) {
        if step % 30 == 29 {
            for w in game.state_bits() {
                for b in w.to_le_bytes() {
                    self.0 ^= u64::from(b);
                    self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
                }
            }
        }
    }
}

/// Play `script` (the pad inputs per step) for `steps` fixed steps from a new game, calling
/// `each(step, &game)` after every step. The same loop the menus' session, the editor's play
/// backend and the I2 corpus run, so all of them reach the same bits.
pub fn play_script(
    script: &[Vec<PadInput>],
    steps: usize,
    mut each: impl FnMut(u64, &Game),
) -> Result<Game, Error2d> {
    let mut game = Game::new()?;
    let mut pad = PadState::default();
    for i in 0..steps {
        if let Some(inputs) = script.get(i) {
            for x in inputs {
                pad.apply(*x);
            }
        }
        game.step(&pad)?;
        each(game.steps - 1, &game);
    }
    Ok(game)
}

/// The scripted run's pinned fingerprint ([`demo_script`] for [`SCRIPT_STEPS`] steps): the
/// same on windows-x86_64 and ubuntu-x86_64, whether the run is played by the core, through
/// the menus, in the editor or by the exported binary. A change here is a change to the 2D
/// solver, the game or the script: say which in the commit.
pub const SCRIPT_FINGERPRINT: Fingerprint = Fingerprint(0xb051_597b_3a74_23b3);

/// Where the scripted run clears the level (the step the goal is reached) and its
/// fingerprint there: what `forge-2d-game --play-script` prints, played through the menus,
/// in any build — the exported one included (`tests/preset/test_2d_tax_is_zero.rs`).
pub const SCRIPT_WIN: (u64, Fingerprint) = (695, Fingerprint(0xc366_f9b6_8e66_f6ac));

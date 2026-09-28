//! The 2D components the inspector edits (Ch.35 §35.2 "2D-specific inspector").
//!
//! Each is a `#[forge_api]` struct, so the inspector generates its rows — labels, units,
//! ranges, steps, tooltips from these doc comments — with no code per type, the API schema
//! (`schema://forge`) describes it, and the editor registers every one of them under every
//! preset (I15: a preset never gates a capability; the 2D preset only opens the panels that
//! use them). A 2D entity's properties are `<key>.<field>` like any component's.
//!
//! [`KEYS`] are the keys the editor registers them under.

use forge_reflect::forge_api;

/// How a 2D body moves.
#[forge_api]
#[derive(Clone, Debug, PartialEq)]
pub enum BodyKind2d {
    /// Never moves (terrain, tiles).
    Static,
    /// Moves by its velocity; nothing pushes it (moving platforms).
    Kinematic,
    /// Moves by forces, gravity and contacts.
    Dynamic,
}

/// A 2D collider's shape.
#[forge_api]
#[derive(Clone, Debug, PartialEq)]
pub enum Shape2d {
    /// A box.
    Box {
        /// Width.
        #[forge(units = "m", min = 0.001, step = 0.05)]
        width: f64,
        /// Height.
        #[forge(units = "m", min = 0.001, step = 0.05)]
        height: f64,
    },
    /// A circle.
    Circle {
        /// Radius.
        #[forge(units = "m", min = 0.001, step = 0.05)]
        radius: f64,
    },
    /// A capsule standing on end (a character).
    Capsule {
        /// Length of the straight part.
        #[forge(units = "m", min = 0.001, step = 0.05)]
        length: f64,
        /// Radius of the rounded ends.
        #[forge(units = "m", min = 0.001, step = 0.05)]
        cap_radius: f64,
    },
}

/// A sprite: a frame of a sprite sheet drawn at the entity.
#[forge_api(name = "Sprite 2D", category = "2D")]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Sprite2d {
    /// The sprite sheet (a `d2.sheet` id).
    pub sheet: String,
    /// The frame of the sheet to show (or the first frame of the animation).
    #[forge(range = 0..=4096, step = 1)]
    pub frame: i64,
    /// The animation playing (empty: none).
    pub animation: String,
    /// Draw layer: higher layers draw over lower ones.
    #[forge(range = -100..=100, step = 1)]
    pub layer: i64,
    /// Order within the layer.
    #[forge(range = -1000..=1000, step = 1)]
    pub order: i64,
    /// Mirror left-right.
    pub flip_x: bool,
    /// Mirror top-bottom.
    pub flip_y: bool,
}

/// A 2D rigid body (the 2D solver).
#[forge_api(name = "Body 2D", category = "2D")]
#[derive(Clone, Debug, PartialEq)]
pub struct Body2d {
    /// How it moves.
    pub kind: BodyKind2d,
    /// Multiplies gravity for this body.
    #[forge(range = -10.0..=10.0, step = 0.1)]
    pub gravity_scale: f64,
    /// Never rotates (a platformer character).
    pub lock_rotation: bool,
    /// Slows it down over time.
    #[forge(units = "1/s", min = 0.0, step = 0.05)]
    pub linear_damping: f64,
}

impl Default for Body2d {
    fn default() -> Self {
        Self {
            kind: BodyKind2d::Dynamic,
            gravity_scale: 1.0,
            lock_rotation: false,
            linear_damping: 0.0,
        }
    }
}

/// A 2D collider on the entity's body.
#[forge_api(name = "Collider 2D", category = "2D")]
#[derive(Clone, Debug, PartialEq)]
pub struct Collider2d {
    /// Its shape.
    pub shape: Shape2d,
    /// Mass per square metre (kg).
    #[forge(min = 0.0, step = 0.1)]
    pub density: f64,
    /// Grip against other surfaces.
    #[forge(range = 0.0..=2.0, step = 0.05, widget = "slider")]
    pub friction: f64,
    /// Bounciness.
    #[forge(range = 0.0..=1.0, step = 0.05, widget = "slider")]
    pub restitution: f64,
    /// Reports overlaps and never pushes.
    pub sensor: bool,
}

impl Default for Collider2d {
    fn default() -> Self {
        Self {
            shape: Shape2d::Box {
                width: 1.0,
                height: 1.0,
            },
            density: 1.0,
            friction: 0.6,
            restitution: 0.0,
            sensor: false,
        }
    }
}

/// A 2D light, with shadows from 2D occluders.
#[forge_api(name = "Light 2D", category = "2D")]
#[derive(Clone, Debug, PartialEq)]
pub struct Light2dComponent {
    /// Red.
    #[forge(range = 0.0..=1.0, step = 0.01, category = "Colour")]
    pub red: f64,
    /// Green.
    #[forge(range = 0.0..=1.0, step = 0.01, category = "Colour")]
    pub green: f64,
    /// Blue.
    #[forge(range = 0.0..=1.0, step = 0.01, category = "Colour")]
    pub blue: f64,
    /// Brightness.
    #[forge(range = 0.0..=20.0, step = 0.1, widget = "slider")]
    pub intensity: f64,
    /// Where it fades to nothing.
    #[forge(units = "m", min = 0.01, step = 0.1)]
    pub radius: f64,
    /// Height above the plane (for normal-mapped sprites).
    #[forge(units = "m", min = 0.0, step = 0.05)]
    pub height: f64,
    /// Full cone angle; 360 is a point light.
    #[forge(units = "deg", range = 1.0..=360.0, step = 1.0)]
    pub cone: f64,
    /// Blocked by occluders.
    pub shadows: bool,
}

impl Default for Light2dComponent {
    fn default() -> Self {
        Self {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            intensity: 1.0,
            radius: 5.0,
            height: 1.0,
            cone: 360.0,
            shadows: true,
        }
    }
}

/// The 2D camera: pixel-perfect with whole-number scaling, or smooth.
#[forge_api(name = "Camera 2D", category = "2D")]
#[derive(Clone, Debug, PartialEq)]
pub struct Camera2dComponent {
    /// Art pixels per world unit.
    #[forge(min = 1.0, step = 1.0)]
    pub pixels_per_unit: f64,
    /// Width in pixels the world is drawn at before scaling (0: the window's).
    #[forge(range = 0..=7680, step = 1)]
    pub reference_width: i64,
    /// Height the world is drawn at before scaling (0: the window's).
    #[forge(range = 0..=4320, step = 1)]
    pub reference_height: i64,
    /// Move by whole art pixels (no shimmer when scrolling).
    pub pixel_snap: bool,
}

impl Default for Camera2dComponent {
    fn default() -> Self {
        Self {
            pixels_per_unit: 16.0,
            reference_width: 320,
            reference_height: 180,
            pixel_snap: true,
        }
    }
}

/// A parallax layer: how fast this entity's layer moves with the camera.
#[forge_api(name = "Parallax 2D", category = "2D")]
#[derive(Clone, Debug, PartialEq)]
pub struct Parallax2d {
    /// The draw layer it applies to.
    #[forge(range = -100..=100, step = 1)]
    pub layer: i64,
    /// Horizontal speed relative to the camera (1: with the world, 0: fixed on screen).
    #[forge(range = -2.0..=2.0, step = 0.05)]
    pub factor_x: f64,
    /// Vertical speed relative to the camera.
    #[forge(range = -2.0..=2.0, step = 0.05)]
    pub factor_y: f64,
    /// Repeat the layer every this many units across (0: no repeat).
    #[forge(units = "m", min = 0.0, step = 0.5)]
    pub repeat_x: f64,
}

impl Default for Parallax2d {
    fn default() -> Self {
        Self {
            layer: -10,
            factor_x: 0.5,
            factor_y: 1.0,
            repeat_x: 0.0,
        }
    }
}

/// A 2D particle emitter.
#[forge_api(name = "Particles 2D", category = "2D")]
#[derive(Clone, Debug, PartialEq)]
pub struct Particles2d {
    /// Particles per second.
    #[forge(units = "1/s", min = 0.0, step = 1.0)]
    pub rate: f64,
    /// Shortest life.
    #[forge(units = "s", min = 0.01, step = 0.05)]
    pub life_min: f64,
    /// Longest life.
    #[forge(units = "s", min = 0.01, step = 0.05)]
    pub life_max: f64,
    /// Launch speed.
    #[forge(units = "m/s", min = 0.0, step = 0.1)]
    pub speed: f64,
    /// Launch direction.
    #[forge(units = "deg", range = -180.0..=180.0, step = 1.0, widget = "angle")]
    pub direction: f64,
    /// Spread either side of the direction.
    #[forge(units = "deg", range = 0.0..=180.0, step = 1.0)]
    pub spread: f64,
    /// Most particles alive at once.
    #[forge(range = 1..=100000, step = 1)]
    pub max: i64,
}

impl Default for Particles2d {
    fn default() -> Self {
        Self {
            rate: 30.0,
            life_min: 0.8,
            life_max: 1.2,
            speed: 1.5,
            direction: 90.0,
            spread: 25.0,
            max: 1000,
        }
    }
}

/// A tile map drawn at the entity.
#[forge_api(name = "Tilemap 2D", category = "2D")]
#[derive(Clone, Debug, PartialEq)]
pub struct Tilemap2d {
    /// The tile map (a `d2.tilemap` id).
    pub map: String,
    /// World size of one cell.
    #[forge(units = "m", min = 0.001, step = 0.05)]
    pub cell_size: f64,
    /// Solid tiles collide and cast shadows.
    pub collides: bool,
}

impl Default for Tilemap2d {
    fn default() -> Self {
        Self {
            map: String::new(),
            cell_size: 1.0,
            collides: true,
        }
    }
}

/// A cutout character: a `Skeleton2D` rig and the clip it plays.
#[forge_api(name = "Skeleton 2D", category = "2D")]
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton2dComponent {
    /// The rig (a `d2.rig` id).
    pub rig: String,
    /// The clip playing (empty: the rest pose).
    pub clip: String,
    /// Playback speed.
    #[forge(range = 0.0..=4.0, step = 0.05)]
    pub speed: f64,
}

impl Default for Skeleton2dComponent {
    fn default() -> Self {
        Self {
            rig: String::new(),
            clip: String::new(),
            speed: 1.0,
        }
    }
}

/// The component keys the editor registers the 2D components under.
pub const KEYS: [&str; 9] = [
    "sprite2d",
    "body2d",
    "collider2d",
    "light2d",
    "camera2d",
    "parallax2d",
    "particles2d",
    "tilemap2d",
    "skeleton2d",
];

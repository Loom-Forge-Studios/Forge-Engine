//! `test_viewport_camera_precision` (Ch.21 §21.21, DoD M2-32; Ch.2 §2.4): the editor camera
//! is usable **from 1 m to far from the world origin without precision jitter**, because it is
//! `f64` and camera-relative: a point is resolved into the camera's frame and the camera's
//! position is subtracted in `f64` before anything is projected.
//!
//! The check: put the camera `R` metres from the world origin (1 m to 1e7 m, the far end of a
//! large world's one depth pass), put an object a few metres in front of it, then
//! slide the camera sideways in 1 mm steps. A pinhole camera moves the object on screen by
//! exactly `focal * step / depth` pixels per step; every step must do that to within
//! 0.05 px, and the object must never jump. Orbiting a pivot ten metres away a thousand
//! times 5e6 m out keeps the pivot within a micrometre.
//!
//! **Positive control (W2):** the same check run through a projection that narrows both
//! positions to `f32` before subtracting (what a camera without the camera-relative rule
//! does) must fail 5e6 m out — an `f32` there is half a metre coarse, so the
//! object sits still and then jumps hundreds of pixels.

use forge_editor::viewport::camera::{EditorCamera, Projected, project_naive_f32_for_control};
use forge_frames::{DQuat, DVec3, FrameId, FramePos, Tick, WorldFrame};

const W: f64 = 1280.0;
const H: f64 = 720.0;
/// Radii the camera works at (metres from the world origin).
const RADII: [f64; 5] = [1.0, 1.0e3, 3.0e4, 5.0e6, 1.0e7];
const STEP: f64 = 0.001;
const STEPS: usize = 200;
const JITTER_PX: f64 = 0.05;

type Project = fn(&EditorCamera, &WorldFrame, FramePos) -> Option<Projected>;

fn camera_relative(cam: &EditorCamera, tree: &WorldFrame, p: FramePos) -> Option<Projected> {
    let off = cam.offset_of(tree, Tick(0), p)?;
    cam.project(off, W, H)
}

fn naive(cam: &EditorCamera, _tree: &WorldFrame, p: FramePos) -> Option<Projected> {
    project_naive_f32_for_control(cam, p, W, H)
}

fn tree() -> WorldFrame {
    WorldFrame
}

/// Slide the camera sideways at radius `r` and check every step's screen motion.
fn check(project: Project, r: f64) -> Result<f64, String> {
    let tree = tree();
    let f = FrameId(0);
    // The camera `r` metres out along a skewed direction, looking along -z.
    let dir = DVec3::new(0.6, 0.64, 0.48);
    let mut cam = EditorCamera::new(f);
    cam.pos = FramePos::new(f, dir * r);
    cam.orientation = DQuat::IDENTITY;
    let depth = 3.0;
    let target = FramePos::new(f, cam.pos.local + DVec3::new(0.25, -0.1, -depth));
    let focal = H / (2.0 * cam.tan_half_fov());
    let expected = focal * STEP / depth;
    let first = project(&cam, &tree, target).ok_or("target not in front")?;
    let mut last = first.x;
    let mut worst: f64 = 0.0;
    for i in 1..=STEPS {
        // Every position is formed from the start in f64 (no accumulated drift in the test).
        cam.pos = FramePos::new(f, dir * r + DVec3::new(STEP * i as f64, 0.0, 0.0));
        let p = project(&cam, &tree, target).ok_or("target left the view")?;
        let dx = last - p.x;
        worst = worst.max((dx - expected).abs());
        last = p.x;
    }
    if worst > JITTER_PX {
        return Err(format!(
            "at {r:e} m the object jitters: a {STEP} m step moved it by up to {worst:.3} px \
             more or less than the {expected:.3} px it should"
        ));
    }
    Ok(worst)
}

#[test]
fn test_viewport_camera_precision() {
    for r in RADII {
        let worst = check(camera_relative, r).unwrap_or_else(|e| panic!("{e}"));
        println!("radius {r:e} m: worst step error {worst:.5} px");
    }
}

#[test]
fn orbiting_far_from_the_origin_keeps_the_pivot_still() {
    let f = FrameId(0);
    let mut cam = EditorCamera::new(f);
    cam.pos = FramePos::new(f, DVec3::new(5.0e6, 0.0, 10.0));
    cam.look_at(FramePos::new(f, DVec3::new(5.0e6, 0.0, 0.0)));
    let pivot = cam.pivot();
    for i in 0..1000 {
        cam.orbit(1.0, if i % 2 == 0 { 0.5 } else { -0.5 });
    }
    let drift = (cam.pivot().local - pivot.local).length();
    assert!(drift < 1e-6, "pivot drifted {drift} m");
}

#[test]
fn positive_control_an_f32_camera_jitters_far_from_the_origin() {
    // Sanity: at 1 m the naive projection is still fine (f32 is fine near the origin)...
    check(naive, 1.0).expect("naive at 1 m");
    // ...and 5e6 m out it must fail the same check.
    let e = check(naive, 5.0e6).expect_err("an f32 camera must jitter at 5e6 m");
    assert!(e.contains("jitters"), "{e}");
}

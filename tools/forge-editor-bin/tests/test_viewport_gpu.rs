//! `test_viewport_gpu` (Ch.21 §21.13, §21.21; DoD M2-32): the viewport hosts the real
//! renderer. A viewport's frame request goes through the binary's `forge-render` host and
//! comes back as a texture in which the entity is drawn **where the editor's own
//! camera-relative projection puts it** — the overlay lines (bounds, gizmo) and the
//! rendered image agree — and a viewport that asks for nothing renders nothing.
//!
//! Positive control (W2): `positive_control_an_empty_scene_has_no_entity_pixels` renders the
//! same view with no drawables; the entity check must fail on it (so the check sees the
//! entity, not the ground or the sky).
//!
//! Needs a GPU adapter: without one it prints `AWAITING(no GPU adapter)` locally and fails
//! under `CI=true` (W9), like forge-render's guards.

use std::rc::Rc;

use forge_cmd::EntityKey;
use forge_editor::services::EditorServices;
use forge_editor::viewport::camera::EditorCamera;
use forge_editor::viewport::scene::Drawable;
use forge_editor::viewport::surface::SurfaceFrame;
use forge_editor_bin::viewport_gpu::ViewportRenderHost;
use forge_frames::{DQuat, DVec3, FrameId, FramePos, Tick};
use forge_gpu::{AdapterPool, GpuDevice, PoolOptions};

const W: u32 = 320;
const H: u32 = 200;

/// `<workspace>/target/gate-observations/C-viewport-forge-render.txt`: `cargo xtask gate`
/// shows the row AWAITING while it exists.
fn observation() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/gate-observations/C-viewport-forge-render.txt")
}

fn pool() -> Option<AdapterPool> {
    match AdapterPool::new(&PoolOptions {
        max_devices: Some(1),
        ..PoolOptions::default()
    }) {
        Ok(p) => {
            let _ = std::fs::remove_file(observation());
            Some(p)
        }
        Err(e) => {
            let ci = std::env::var("CI").is_ok_and(|v| v == "true" || v == "1");
            assert!(!ci, "CI=true and no GPU adapter: {e}");
            let reason = format!("no GPU adapter: {e}");
            println!("test_viewport_gpu: AWAITING({reason})");
            let f = observation();
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&f, &reason);
            None
        }
    }
}

fn cube(key: u64, x: f64) -> Drawable {
    Drawable {
        key: EntityKey(key),
        name: "Cube".into(),
        pos: FramePos::new(FrameId(0), DVec3::new(x, 0.5, 0.0)),
        rotation: DQuat::IDENTITY,
        scale: 1.0,
        seed_path: None,
        locked: false,
        layer: false,
    }
}

type Surfaces = Rc<std::cell::RefCell<forge_editor::viewport::surface::ViewportSurfaces>>;

/// One rendered frame: its RGBA8 pixels, the pixel the editor's projection puts the probe
/// at, the host, the surface id and the registry.
struct Rendered {
    img: Vec<u8>,
    at: (u32, u32),
    host: ViewportRenderHost,
    id: u32,
    surfaces: Surfaces,
}

/// Render one frame of `drawables` from the default editor camera.
fn render(dev: &GpuDevice, drawables: Vec<Drawable>, probe: FramePos) -> Rendered {
    let services = EditorServices::default();
    let mut host = ViewportRenderHost::new(&services, "forge-render (test)");
    let id = services.viewport_surfaces.borrow_mut().allocate();
    let camera = EditorCamera::new(FrameId(0));
    services.viewport_surfaces.borrow_mut().request(
        id,
        SurfaceFrame {
            camera,
            size_px: (W, H),
            drawables: Rc::new(drawables),
            selection: Vec::new(),
            tick: Tick(0),
            world: None,
        },
    );
    let done = host.render_dirty(dev);
    assert_eq!(done.len(), 1, "one dirty viewport rendered");
    assert_eq!(services.viewport_surfaces.borrow().error(), None);
    let tex = host.texture(id).expect("texture");
    let img = forge_gpu::transfer::read_texture(dev, tex).expect("readback");
    let off = probe.local - camera.pos.local;
    let p = camera
        .project(off, f64::from(W), f64::from(H))
        .expect("in front");
    Rendered {
        img,
        at: (p.x.round() as u32, p.y.round() as u32),
        host,
        id,
        surfaces: Rc::clone(&services.viewport_surfaces),
    }
}

fn px(img: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// The entity colour (a warm red-orange) at `(x, y)` and its 3x3 neighbourhood.
fn entity_at(img: &[u8], at: (u32, u32)) -> Result<(), String> {
    for dy in 0..3 {
        for dx in 0..3 {
            let (x, y) = (at.0 + dx - 1, at.1 + dy - 1);
            let [r, g, b, _] = px(img, x, y);
            if !(i32::from(r) > i32::from(g) + 25 && i32::from(r) > i32::from(b) + 40) {
                return Err(format!(
                    "pixel ({x}, {y}) is {:?}, not the entity's colour",
                    [r, g, b]
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn the_viewport_renders_the_entity_where_the_editor_projects_it() {
    let Some(pool) = pool() else {
        return;
    };
    let dev = pool.primary();
    let c = cube(1, 0.0);
    let Rendered {
        img,
        at,
        mut host,
        id,
        surfaces,
    } = render(dev, vec![c.clone()], c.pos);
    entity_at(&img, at).unwrap_or_else(|e| panic!("{e}"));
    // Two metres to the side the ground or the sky shows, not the entity.
    let side = FramePos::new(FrameId(0), DVec3::new(2.4, 0.5, 0.0));
    let cam = EditorCamera::new(FrameId(0));
    let q = cam
        .project(side.local - cam.pos.local, f64::from(W), f64::from(H))
        .expect("front");
    assert!(entity_at(&img, (q.x.round() as u32, q.y.round() as u32)).is_err());
    // Nothing changed: nothing is rendered again.
    let before = host.rendered;
    assert!(host.render_dirty(dev).is_empty());
    assert_eq!(host.rendered, before);
    assert_eq!(surfaces.borrow().presented_count(id), 1);
}

#[test]
fn positive_control_an_empty_scene_has_no_entity_pixels() {
    let Some(pool) = pool() else {
        return;
    };
    let dev = pool.primary();
    let c = cube(1, 0.0);
    let Rendered { img, at, .. } = render(dev, Vec::new(), c.pos);
    assert!(
        entity_at(&img, at).is_err(),
        "the check must not find an entity in an empty scene"
    );
}

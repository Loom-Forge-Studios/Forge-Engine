//! What the viewport shows, read from the mirror (Ch.21 §21.21): every entity with a
//! `Transform` as a [`Drawable`], kept up to date **incrementally** from the mirror's change
//! log (a property change re-reads one entity, never the scene), plus the line overlays —
//! grid, frame axes, bounding boxes — projected camera-relative.
//!
//! The overlay lines are also the **placeholder renderer**: without a GPU scene renderer
//! attached (headless tests, a machine with no adapter) the viewport still shows every
//! entity as its bounding box over the grid. With `forge-render` attached (the editor
//! binary) the same lines are drawn over the rendered image.

use std::collections::BTreeMap;

use forge_cmd::{EntityKey, Value};
use forge_frames::{DQuat, DVec3, FrameId, FramePos, FrameResolver, Tick};
use forge_ui::widgets::euler_to_quat;

use crate::inspect::GEN_SEED_PATH;
use crate::mirror::{MirrorChange, ProjectMirror};
use crate::viewport::camera::EditorCamera;

/// Transform property paths (the built-in `Transform` component, `crate::inspect`).
pub const P_FRAME: &str = "transform.position.frame";
pub const P_LOCAL: &str = "transform.position.local";
pub const P_YAW: &str = "transform.yaw";
pub const P_PITCH: &str = "transform.pitch";
pub const P_ROLL: &str = "transform.roll";
pub const P_SCALE: &str = "transform.scale";
/// The hierarchy's visibility and lock badges (WP-U5).
pub const P_HIDDEN: &str = "editor.hidden";
pub const P_LOCKED: &str = "editor.locked";

/// Half the edge of the unit cube every entity is shown as (and picked by) until it has a
/// mesh: `forge_render::MeshData::cube()` is one metre on a side.
pub const UNIT_HALF: f64 = 0.5;

/// One entity as the viewport shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawable {
    pub key: EntityKey,
    pub name: String,
    /// Where it is (I1: a frame and an offset).
    pub pos: FramePos,
    /// Its axes → the axes of `pos.frame` (from yaw, pitch, roll).
    pub rotation: DQuat,
    /// Uniform scale.
    pub scale: f64,
    /// Generated content's seed path (`gen.seed_path`), shown under the cursor.
    pub seed_path: Option<String>,
    pub locked: bool,
    /// Shown by a viewport layer ([`crate::viewport::layer::P_LAYER`]): not drawn as a box
    /// and not picked (the layer's own tools edit it).
    pub layer: bool,
}

impl Drawable {
    /// Half extent of its bounding box (metres, along its own axes).
    pub fn half_extent(&self) -> f64 {
        UNIT_HALF * self.scale.abs()
    }
    /// Radius of its bounding sphere.
    pub fn radius(&self) -> f64 {
        self.half_extent() * 3f64.sqrt()
    }
}

fn float(p: &BTreeMap<String, Value>, k: &str, d: f64) -> f64 {
    match p.get(k) {
        Some(Value::Float(v)) => *v,
        Some(Value::Int(v)) => *v as f64,
        _ => d,
    }
}

/// The drawable for an entity, if it has a transform and is not hidden.
pub fn drawable(mirror: &ProjectMirror, key: EntityKey) -> Option<Drawable> {
    let e = mirror.entity(key)?;
    let p = &e.properties;
    if matches!(p.get(P_HIDDEN), Some(Value::Bool(true))) {
        return None;
    }
    let local = match p.get(P_LOCAL)? {
        Value::Vec3(v) => DVec3::new(v[0], v[1], v[2]),
        _ => return None,
    };
    let frame = match p.get(P_FRAME) {
        Some(Value::Int(f)) => u32::try_from(*f).ok()?,
        _ => 0,
    };
    Some(Drawable {
        key,
        name: e.name.clone(),
        pos: FramePos::new(FrameId(frame), local),
        rotation: euler_to_quat(
            float(p, P_YAW, 0.0),
            float(p, P_PITCH, 0.0),
            float(p, P_ROLL, 0.0),
        ),
        scale: float(p, P_SCALE, 1.0),
        seed_path: match p.get(GEN_SEED_PATH) {
            Some(Value::Text(s)) => Some(s.clone()),
            _ => None,
        },
        locked: matches!(p.get(P_LOCKED), Some(Value::Bool(true))),
        layer: p.contains_key(crate::viewport::layer::P_LAYER),
    })
}

/// The viewport's scene (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct ViewportScene {
    items: BTreeMap<EntityKey, Drawable>,
    /// The mirror change-log position this scene has applied up to (`None`: rebuild).
    seen: Option<u64>,
    revision: u64,
    /// For a play-in-editor view ([`ViewportScene::with_overrides`]): the play core's
    /// revision it shows. `None` for the edit world.
    play_revision: Option<u64>,
    /// Entities re-read by the last sync (the incremental guard's counter).
    pub last_reads: usize,
    /// The scene shown in isolation (see [`ViewportScene::set_isolation`]).
    isolate: Option<EntityKey>,
}

/// Does the viewport show `k`? The world never shows the scene library (its scenes are
/// templates; their instances are in the world); a scene opened in isolation shows only
/// itself (WP-U20).
pub fn shown(mirror: &ProjectMirror, k: EntityKey, isolate: Option<EntityKey>) -> bool {
    // A walk up the entity's own ancestors, allocating nothing: the cost of a drawable's
    // refresh stays O(depth), never O(project).
    let mut at = k;
    for _ in 0..4096 {
        if Some(at) == isolate {
            return true;
        }
        match mirror.entity(at).and_then(|e| e.parent) {
            Some(p) => at = p,
            None => break,
        }
    }
    match isolate {
        Some(_) => false,
        None => !mirror.entity(at).is_some_and(|e| {
            e.properties.get(forge_scene::model::LIBRARY) == Some(&Value::Bool(true))
        }),
    }
}

/// What a scene shows, as a cache key: the edit world's revision and, for a play view, the
/// simulation's revision. Two scenes with equal versions show the same drawables.
pub type SceneVersion = (u64, Option<u64>);

impl ViewportScene {
    pub fn new() -> Self {
        Self::default()
    }

    /// The edit world's revision: bumped whenever a drawable of the edit world changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// What this scene shows (see [`SceneVersion`]): it changes when the edit world changed
    /// or, for a play view, when the simulation stepped. A cache of what a scene shows keys
    /// on this, not on [`ViewportScene::revision`] alone.
    pub fn version(&self) -> SceneVersion {
        (self.revision, self.play_revision)
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn get(&self, key: EntityKey) -> Option<&Drawable> {
        self.items.get(&key)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Drawable> {
        self.items.values()
    }

    /// Bring the scene up to date with the mirror: only the entities the change log names
    /// are re-read; a gap in the log (or the first call) rebuilds. Returns whether anything
    /// changed.
    pub fn sync(&mut self, mirror: &ProjectMirror) -> bool {
        self.last_reads = 0;
        let now = mirror.change_seq();
        // A move into or out of the scene library (or of the scene open in isolation) changes
        // what a whole subtree shows: that rebuilds. Other moves change nothing drawn.
        let isolate = self.isolate;
        let composes = |before: &Option<EntityKey>, after: &Option<EntityKey>| {
            isolate.is_some()
                || [before, after]
                    .into_iter()
                    .flatten()
                    .any(|p| !shown(mirror, *p, None))
        };
        let touched: Option<Vec<EntityKey>> = match self.seen {
            Some(s) if s == now => return false,
            Some(s) => mirror.changes_since(s).and_then(|it| {
                let mut v: Vec<EntityKey> = Vec::new();
                for c in it {
                    match c {
                        MirrorChange::Created(e)
                        | MirrorChange::Renamed(e)
                        | MirrorChange::Property(e, _) => v.push(*e),
                        MirrorChange::Removed { entity, .. } => v.push(*entity),
                        MirrorChange::Reparented { before, after, .. }
                            if composes(before, after) =>
                        {
                            return None;
                        }
                        MirrorChange::Reparented { .. } => {}
                    }
                }
                v.sort_unstable();
                v.dedup();
                Some(v)
            }),
            None => None,
        };
        self.seen = Some(now);
        let mut changed = false;
        match touched {
            Some(keys) => {
                for k in keys {
                    self.last_reads += 1;
                    changed |= self.refresh(mirror, k);
                }
            }
            None => {
                let isolate = self.isolate;
                let fresh: BTreeMap<EntityKey, Drawable> = mirror
                    .entities()
                    .filter(|(k, _)| shown(mirror, **k, isolate))
                    .filter_map(|(k, _)| drawable(mirror, *k).map(|d| (*k, d)))
                    .collect();
                self.last_reads = mirror.len();
                changed = fresh != self.items;
                self.items = fresh;
            }
        }
        if changed {
            self.revision += 1;
        }
        changed
    }

    /// Show only the scene rooted at `root` ("Open base scene"), or (`None`) the world
    /// without the scene library. A change rebuilds at the next sync.
    pub fn set_isolation(&mut self, root: Option<EntityKey>) {
        if self.isolate != root {
            self.isolate = root;
            self.seen = None;
        }
    }

    /// The scene shown in isolation, if any.
    pub fn isolation(&self) -> Option<EntityKey> {
        self.isolate
    }

    fn refresh(&mut self, mirror: &ProjectMirror, k: EntityKey) -> bool {
        let d = if shown(mirror, k, self.isolate) {
            drawable(mirror, k)
        } else {
            None
        };
        match d {
            Some(d) => {
                if self.items.get(&k) == Some(&d) {
                    false
                } else {
                    self.items.insert(k, d);
                    true
                }
            }
            None => self.items.remove(&k).is_some(),
        }
    }

    /// A drawable with its transform replaced (the play-in-editor sandbox shows simulated
    /// transforms without touching the project). `play_revision` is the play core's
    /// revision the transforms come from; it is part of the view's
    /// [`ViewportScene::version`], so every simulation step is a new version.
    pub fn with_overrides(
        &self,
        over: &BTreeMap<EntityKey, (FramePos, DQuat, f64)>,
        play_revision: u64,
    ) -> Self {
        let mut s = self.clone();
        for (k, (p, r, sc)) in over {
            if let Some(d) = s.items.get_mut(k) {
                d.pos = *p;
                d.rotation = *r;
                d.scale = *sc;
            }
        }
        s.play_revision = Some(play_revision);
        s
    }
}

// ---- picking ------------------------------------------------------------------------------

/// A pick: the nearest entity along a ray, and the distance to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub key: EntityKey,
    pub distance: f64,
}

/// Ray against an oriented box, all camera-relative: `rel` the box centre's offset from
/// the camera, `rot` the box axes → camera-frame axes, `half` its half extent, `dir` the unit
/// ray direction from the camera. The entry distance, if hit.
pub fn ray_box(rel: DVec3, rot: DQuat, half: f64, dir: DVec3) -> Option<f64> {
    // Into the box's axes: origin at -center.
    let o = rot.inverse_rotate(-rel);
    let d = rot.inverse_rotate(dir);
    let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
    for (oi, di) in [(o.x, d.x), (o.y, d.y), (o.z, d.z)] {
        if di.abs() < 1e-300 {
            if oi.abs() > half {
                return None;
            }
            continue;
        }
        let (a, b) = ((-half - oi) / di, (half - oi) / di);
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    if t1 < 0.0 {
        return None;
    }
    Some(t0.max(0.0))
}

/// The nearest drawable under the ray through pixel `(x, y)` of a `w x h` viewport. Locked
/// entities are skipped when `skip_locked`.
pub fn pick(
    scene: &ViewportScene,
    cam: &EditorCamera,
    tree: &dyn FrameResolver,
    t: Tick,
    px: (f64, f64),
    size: (f64, f64),
    skip_locked: bool,
) -> Option<Pick> {
    let dir = cam.ray_dir(px.0, px.1, size.0, size.1);
    let mut best: Option<Pick> = None;
    for d in scene.iter() {
        if (skip_locked && d.locked) || d.layer {
            continue;
        }
        let Some(off) = cam.offset_of(tree, t, d.pos) else {
            continue;
        };
        // Cheap reject by the bounding sphere first.
        let along = off.dot(dir);
        let r = d.radius();
        if along < -r || (off - dir * along).length_squared() > r * r {
            continue;
        }
        let Some(rot) = cam.rotation_from(tree, t, d.pos.frame) else {
            continue;
        };
        if let Some(hit) = ray_box(off, rot * d.rotation, d.half_extent(), dir)
            && best.is_none_or(|b| hit < b.distance)
        {
            best = Some(Pick {
                key: d.key,
                distance: hit,
            });
        }
    }
    best
}

// ---- line overlays ------------------------------------------------------------------------

/// What a line is, so the widget can draw it with a theme colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LineStyle {
    GridMinor,
    GridMajor,
    /// A frame axis (0 = x, 1 = y, 2 = z).
    Axis(u8),
    Bounds,
    BoundsSelected,
    /// A gizmo handle on axis 0..=2 (3: the uniform / view handle); `hot`: hovered or dragged.
    Gizmo {
        axis: u8,
        hot: bool,
    },
    /// A tool's footprint under the pointer (a brush's, say).
    Cursor,
    /// A measurement.
    Measure,
    /// What a viewport layer draws ([`crate::viewport::layer`]).
    Layer,
    /// The physics debug drawing during Play ([`crate::viewport::physics`]).
    Physics(forge_phys::DebugKind),
}

/// A 2D segment in viewport pixels (already camera-relative and projected; the only
/// narrowing to `f32` is here, of pixel coordinates).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: (f32, f32),
    pub b: (f32, f32),
    pub style: LineStyle,
}

/// Collects projected segments for one viewport.
#[derive(Debug)]
pub struct LineSink<'a> {
    pub cam: &'a EditorCamera,
    pub w: f64,
    pub h: f64,
    pub out: &'a mut Vec<Segment>,
}

impl LineSink<'_> {
    /// A segment given as two camera-relative offsets (camera-frame axes).
    pub fn seg(&mut self, a: DVec3, b: DVec3, style: LineStyle) {
        if let Some((p, q)) = self.cam.project_segment(a, b, self.w, self.h) {
            // Pixel coordinates far outside the view are clamped so a long clipped line
            // never loses precision when narrowed.
            let c = |v: f64, m: f64| v.clamp(-4.0 * m, 5.0 * m) as f32;
            self.out.push(Segment {
                a: (c(p.x, self.w), c(p.y, self.h)),
                b: (c(q.x, self.w), c(q.y, self.h)),
                style,
            });
        }
    }

    /// The twelve edges of an oriented box.
    pub fn boxed(&mut self, rel: DVec3, rot: DQuat, half: DVec3, style: LineStyle) {
        let corner = |i: u8| {
            let s = |bit: u8, v: f64| if i & bit != 0 { v } else { -v };
            rel + rot.rotate(DVec3::new(s(1, half.x), s(2, half.y), s(4, half.z)))
        };
        for (i, j) in [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            self.seg(corner(i), corner(j), style);
        }
    }

    /// A circle of `radius` about `center` in the plane normal to `axis` (camera-frame axes).
    pub fn circle(&mut self, rel: DVec3, axis: DVec3, radius: f64, style: LineStyle) {
        let Some(n) = axis.try_normalize() else {
            return;
        };
        let u = n
            .cross(DVec3::Y)
            .try_normalize()
            .or_else(|| n.cross(DVec3::X).try_normalize())
            .unwrap_or(DVec3::X);
        let v = n.cross(u);
        const N: usize = 48;
        let at = |i: usize| {
            let a = std::f64::consts::TAU * i as f64 / N as f64;
            rel + (u * a.cos() + v * a.sin()) * radius
        };
        for i in 0..N {
            self.seg(at(i), at(i + 1), style);
        }
    }
}

/// Grid spacing for a camera `height` metres above the grid plane: a power of ten, so the
/// grid reads the same from 1 m to orbit.
pub fn grid_step(height: f64) -> f64 {
    let h = height.abs().max(0.1);
    10f64.powf((h / 2.0).log10().floor()).max(0.01)
}

/// The grid on the camera frame's `y = 0` plane around the camera (its spacing a power of ten
/// for the camera's height, never finer than `min_step`, the project's grid size), and the
/// frame's axes.
/// Every coordinate is formed relative to the camera in `f64`, so the grid is exact at any
/// distance from the frame's origin.
pub fn grid_and_axes(sink: &mut LineSink<'_>, min_step: f64) {
    const HALF_LINES: i64 = 20;
    let cam = sink.cam.pos.local;
    let height = cam.y;
    let step = grid_step(height).max(min_step);
    let extent = step * HALF_LINES as f64;
    // Grid lines at multiples of `step` near the camera's foot.
    let bx = (cam.x / step).round();
    let bz = (cam.z / step).round();
    for i in -HALF_LINES..=HALF_LINES {
        let gx = (bx + i as f64) * step;
        let gz = (bz + i as f64) * step;
        let major_x = ((bx as i64 + i) % 10) == 0;
        let major_z = ((bz as i64 + i) % 10) == 0;
        // Offsets from the camera: (grid coordinate - camera coordinate), in f64.
        let (x, z0, z1) = (
            gx - cam.x,
            bz * step - extent - cam.z,
            bz * step + extent - cam.z,
        );
        if gx != 0.0 {
            sink.seg(
                DVec3::new(x, -height, z0),
                DVec3::new(x, -height, z1),
                if major_x {
                    LineStyle::GridMajor
                } else {
                    LineStyle::GridMinor
                },
            );
        }
        let (z, x0, x1) = (
            gz - cam.z,
            bx * step - extent - cam.x,
            bx * step + extent - cam.x,
        );
        if gz != 0.0 {
            sink.seg(
                DVec3::new(x0, -height, z),
                DVec3::new(x1, -height, z),
                if major_z {
                    LineStyle::GridMajor
                } else {
                    LineStyle::GridMinor
                },
            );
        }
    }
    // The frame's axes through its origin, as long as the grid is wide.
    let o = -cam;
    let len = extent.max(1.0);
    sink.seg(o - DVec3::X * len, o + DVec3::X * len, LineStyle::Axis(0));
    sink.seg(o, o + DVec3::Y * len, LineStyle::Axis(1));
    sink.seg(o - DVec3::Z * len, o + DVec3::Z * len, LineStyle::Axis(2));
}

/// Bounding boxes of the drawables in front of the camera, nearest first, at most `cap`
/// (selected ones always, highlighted). Returns how many were drawn.
pub fn bounds(
    sink: &mut LineSink<'_>,
    scene: &ViewportScene,
    tree: &dyn FrameResolver,
    t: Tick,
    selected: &[EntityKey],
    cap: usize,
) -> usize {
    let mut visible: Vec<(bool, f64, DVec3, DQuat, &Drawable)> = Vec::new();
    for d in scene.iter().filter(|d| !d.layer) {
        let Some(off) = sink.cam.offset_of(tree, t, d.pos) else {
            continue;
        };
        // Cull what is behind the camera by its bounding sphere.
        let v = sink.cam.orientation.inverse_rotate(off);
        if -v.z < -d.radius() {
            continue;
        }
        let Some(rot) = sink.cam.rotation_from(tree, t, d.pos.frame) else {
            continue;
        };
        visible.push((
            !selected.contains(&d.key),
            off.length_squared(),
            off,
            rot,
            d,
        ));
    }
    // Selected first, then nearest.
    visible.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let n = visible.len().min(cap);
    for (unselected, _, off, rot, d) in visible.into_iter().take(cap) {
        let style = if unselected {
            LineStyle::Bounds
        } else {
            LineStyle::BoundsSelected
        };
        sink.boxed(off, rot * d.rotation, DVec3::splat(d.half_extent()), style);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_steps_are_powers_of_ten_that_follow_height() {
        assert_eq!(grid_step(1.0), 0.1);
        assert_eq!(grid_step(5.0), 1.0);
        assert_eq!(grid_step(50.0), 10.0);
        assert_eq!(grid_step(6.4e6), 1.0e6);
    }

    #[test]
    fn ray_hits_an_axis_aligned_box_at_its_face() {
        let hit = ray_box(
            DVec3::new(0.0, 0.0, -5.0),
            DQuat::IDENTITY,
            0.5,
            DVec3::new(0.0, 0.0, -1.0),
        );
        assert!((hit.expect("hit") - 4.5).abs() < 1e-12);
        let miss = ray_box(
            DVec3::new(2.0, 0.0, -5.0),
            DQuat::IDENTITY,
            0.5,
            DVec3::new(0.0, 0.0, -1.0),
        );
        assert!(miss.is_none());
        // Rotated 45° about y: the corner faces the ray.
        let q = DQuat::from_axis_angle(DVec3::Y, std::f64::consts::FRAC_PI_4);
        let hit = ray_box(
            DVec3::new(0.0, 0.0, -5.0),
            q,
            0.5,
            DVec3::new(0.0, 0.0, -1.0),
        );
        assert!((hit.expect("hit") - (5.0 - 0.5 * 2f64.sqrt())).abs() < 1e-9);
        // Behind the camera: no hit.
        assert!(
            ray_box(
                DVec3::new(0.0, 0.0, 5.0),
                DQuat::IDENTITY,
                0.5,
                DVec3::new(0.0, 0.0, -1.0)
            )
            .is_none()
        );
    }
}

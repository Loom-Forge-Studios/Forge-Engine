//! What the renderer draws: meshes, metal-rough materials, placed instances and lights. Every
//! position is a [`FramePos`] and every value is `f64`; narrowing happens in
//! [`last_mile`](crate::last_mile) only.

use forge_frames::{DQuat, DVec3, FrameId, FramePos};
use forge_num::det;

/// A vertex in object space (`local` is the mesh's own coordinates, not a place).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Object-space coordinates.
    pub local: DVec3,
    /// Unit normal, object space.
    pub normal: DVec3,
    /// Texture coordinates.
    pub uv: [f64; 2],
}

/// An indexed triangle mesh (counter-clockwise front faces).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    /// Vertices.
    pub vertices: Vec<Vertex>,
    /// Triangle list indices.
    pub indices: Vec<u32>,
}

impl MeshData {
    /// The bounding sphere (object-space centre offset and radius): the centre of the
    /// axis-aligned box and the farthest vertex from it.
    #[must_use]
    pub fn bounds(&self) -> (DVec3, f64) {
        vertex_bounds(&self.vertices)
    }

    /// A unit quad in the `xy` plane (`-0.5..0.5`), facing `+z`.
    #[must_use]
    pub fn quad() -> Self {
        let n = DVec3::Z;
        let v = |x: f64, y: f64| Vertex {
            local: DVec3::new(x, y, 0.0),
            normal: n,
            uv: [x + 0.5, 0.5 - y],
        };
        Self {
            vertices: vec![v(-0.5, -0.5), v(0.5, -0.5), v(0.5, 0.5), v(-0.5, 0.5)],
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    /// A unit cube (`-0.5..0.5`), 24 vertices with flat normals.
    #[must_use]
    pub fn cube() -> Self {
        let mut m = Self::default();
        let faces = [
            (DVec3::X, DVec3::Y, DVec3::Z),
            (-DVec3::X, DVec3::Y, -DVec3::Z),
            (DVec3::Y, DVec3::Z, DVec3::X),
            (-DVec3::Y, DVec3::Z, -DVec3::X),
            (DVec3::Z, DVec3::X, DVec3::Y),
            (-DVec3::Z, DVec3::X, -DVec3::Y),
        ];
        for (n, u, v) in faces {
            // u x v == n keeps the winding counter-clockwise seen from outside.
            let (u, v) = if u.cross(v).dot(n) > 0.0 {
                (u, v)
            } else {
                (v, u)
            };
            let base = m.vertices.len() as u32;
            for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                m.vertices.push(Vertex {
                    local: n * 0.5 + u * a + v * b,
                    normal: n,
                    uv: [a + 0.5, 0.5 - b],
                });
            }
            m.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        m
    }

    /// A UV sphere of radius 0.5 with `segments` around and `rings` from pole to pole.
    #[must_use]
    pub fn uv_sphere(segments: u32, rings: u32) -> Self {
        let (segments, rings) = (segments.max(3), rings.max(2));
        let mut m = Self::default();
        for r in 0..=rings {
            let theta = core::f64::consts::PI * f64::from(r) / f64::from(rings);
            let (st, ct) = (det::sin(theta), det::cos(theta));
            for s in 0..=segments {
                let phi = core::f64::consts::TAU * f64::from(s) / f64::from(segments);
                let n = DVec3::new(st * det::cos(phi), ct, -st * det::sin(phi));
                m.vertices.push(Vertex {
                    local: n * 0.5,
                    normal: n,
                    uv: [
                        f64::from(s) / f64::from(segments),
                        f64::from(r) / f64::from(rings),
                    ],
                });
            }
        }
        let row = segments + 1;
        for r in 0..rings {
            for s in 0..segments {
                let (a, b) = (r * row + s, (r + 1) * row + s);
                m.indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        m
    }
}

/// A metal-rough material (glTF 2.0's model). Colours are linear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// Base colour (linear RGB) and alpha.
    pub base_color: [f64; 4],
    /// 0 = dielectric, 1 = metal.
    pub metallic: f64,
    /// Perceptual roughness, 0..1 (clamped to 0.045 in the shader to keep GGX finite).
    pub roughness: f64,
    /// Dielectric specular reflectance, 0..1 (0.5 = 4 % at normal incidence, Filament's
    /// convention: `f0 = 0.16 reflectance^2`).
    pub reflectance: f64,
    /// Emitted radiance, linear RGB (in the same units as light contributions, before
    /// exposure).
    pub emissive: [f64; 3],
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            reflectance: 0.5,
            emissive: [0.0; 3],
        }
    }
}

impl Material {
    /// A purely emissive, black material (unlit colour — for tests and markers).
    #[must_use]
    pub fn unlit(rgb: [f64; 3]) -> Self {
        Self {
            base_color: [0.0, 0.0, 0.0, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            reflectance: 0.0,
            emissive: rgb,
        }
    }
}

/// A mesh held by a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshId(pub u32);

/// A pool of equal-sized meshes a renderer draws together (WP-U21,
/// [`crate::Renderer::add_mesh_pool`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshPoolId(pub u32);

/// A material held by a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MaterialId(pub u32);

/// A mesh placed in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    /// Where its object-space origin is.
    pub pos: FramePos,
    /// Object axes -> the axes of `pos.frame`.
    pub orientation: DQuat,
    /// Per-axis scale (non-zero).
    pub scale: DVec3,
    /// The mesh.
    pub mesh: MeshId,
    /// The material.
    pub material: MaterialId,
    /// Whether it casts sun shadows.
    pub casts_shadows: bool,
}

impl Instance {
    /// An unrotated, unscaled instance that casts shadows.
    #[must_use]
    pub fn new(pos: FramePos, mesh: MeshId, material: MaterialId) -> Self {
        Self {
            pos,
            orientation: DQuat::IDENTITY,
            scale: DVec3::splat(1.0),
            mesh,
            material,
            casts_shadows: true,
        }
    }

    /// With a uniform scale.
    #[must_use]
    pub fn scaled(mut self, s: f64) -> Self {
        self.scale = DVec3::splat(s);
        self
    }

    /// With per-axis scale.
    #[must_use]
    pub fn scaled3(mut self, s: DVec3) -> Self {
        self.scale = s;
        self
    }

    /// With an orientation.
    #[must_use]
    pub fn oriented(mut self, q: DQuat) -> Self {
        self.orientation = q;
        self
    }
}

/// The sun (or any light at infinity).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectionalLight {
    /// The frame whose axes `toward_light` is given in.
    pub frame: FrameId,
    /// Unit vector from the scene toward the light.
    pub toward_light: DVec3,
    /// Linear RGB colour (multiplies `illuminance`).
    pub color: [f64; 3],
    /// Illuminance at normal incidence, lux (the sun at noon: ~100,000).
    pub illuminance: f64,
    /// Whether it casts shadows (cascades, [`CascadeSettings`](crate::CascadeSettings)).
    pub casts_shadows: bool,
}

/// A point light (spot when `spot` is set).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PunctualLight {
    /// Where it is.
    pub pos: FramePos,
    /// Linear RGB colour (multiplies `intensity`).
    pub color: [f64; 3],
    /// Luminous intensity, candela.
    pub intensity: f64,
    /// Influence radius, metres: contribution is windowed smoothly to exactly 0 here, which
    /// is what makes clustered culling exact.
    pub range: f64,
    /// Spot cone, if any.
    pub spot: Option<Spot>,
}

/// A spot light's cone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    /// The axis of the cone, unit, in the axes of the light's `pos.frame`.
    pub direction: DVec3,
    /// Full intensity inside this half-angle, radians.
    pub inner_angle: f64,
    /// Zero outside this half-angle, radians.
    pub outer_angle: f64,
}

/// Where the frame's atmosphere is: its ground sphere's centre. The tables are the renderer's
/// ([`crate::Renderer::set_atmosphere`]); without them, or without a sun, nothing is added.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneAtmosphere {
    /// The centre of the ground sphere whose air it is.
    pub centre: FramePos,
}

/// Points of light at infinity (a starry backdrop), drawn in the sky pass: the list installed
/// with [`crate::Renderer::set_sky_points`] (uploaded once), whose directions are in the axes
/// of `frame` — so turning the frame turns the whole backdrop without a new upload.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyPoints {
    /// The frame whose axes the points' directions are given in.
    pub frame: FrameId,
    /// Multiplies every point's illuminance (1 = as given).
    pub brightness: f64,
}

/// One point of light at infinity: where, how bright, what colour, and how spread out (an
/// extended source is drawn as a Gaussian).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyPoint {
    /// Unit direction, in the axes of the [`SkyPoints::frame`].
    pub dir: DVec3,
    /// Illuminance it delivers, lux (before extinction and exposure).
    pub lux: f64,
    /// Colour, unit luminance (linear RGB).
    pub color: [f64; 3],
    /// The Gaussian's angular standard deviation, radians (`0`: a point source).
    pub sigma: f64,
}

/// How a [`SkySprite`] shines, as seen from the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SkyShine {
    /// Self-luminous (a sun's disc): the illuminance it delivers at the camera, linear RGB,
    /// lux.
    Emitter {
        /// Illuminance at the camera per channel, lux.
        illuminance: [f64; 3],
    },
    /// Lit by a light (a moon): the illuminance it reflects to the camera (linear RGB, lux),
    /// where its light is (the disc's lit side faces it; `None`: unlit, the whole disc
    /// evenly), and the disc's normalisation — its surface brightness over its
    /// disc-integrated brightness, so a resolved crescent keeps the illuminance it delivers.
    Lit {
        /// Illuminance at the camera per channel, lux.
        illuminance: [f64; 3],
        /// Where its light comes from.
        light: Option<FramePos>,
        /// Surface-brightness normalisation of the lit disc.
        norm: f64,
    },
}

/// Something in the sky drawn as a sprite (Ch.10): a point-spread function while it is
/// smaller than a pixel — its light conserved wherever it falls, so it never shimmers — and a
/// disc once resolved, shaded as a sphere lit from its light when it is [`SkyShine::Lit`].
/// Its direction and angular size are worked out from the camera in `f64` at the last mile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkySprite {
    /// Its centre.
    pub pos: FramePos,
    /// Its radius, metres.
    pub radius: f64,
    /// How it shines.
    pub shine: SkyShine,
}

/// Everything drawn in one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Placed meshes.
    pub instances: Vec<Instance>,
    /// Point and spot lights (clustered).
    pub lights: Vec<PunctualLight>,
    /// The sun.
    pub sun: Option<DirectionalLight>,
    /// Uniform ambient radiance, linear RGB (a stand-in until GI stage 1, Ch.10 §10.6).
    pub ambient: [f64; 3],
    /// Exposure as EV100 (sunny day ~15). Scene values are multiplied by
    /// `1 / (1.2 * 2^ev100)` before tone mapping.
    pub ev100: f64,
    /// Radiance of the background where nothing is drawn, linear RGB (under an atmosphere the
    /// sky replaces it).
    pub background: [f64; 3],
    /// Sprites in the sky (a sun disc, a moon).
    pub sky_sprites: Vec<SkySprite>,
    /// The atmosphere the camera looks through, if any.
    pub atmosphere: Option<SceneAtmosphere>,
    /// The points of light at infinity, if any.
    pub sky_points: Option<SkyPoints>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            instances: Vec::new(),
            lights: Vec::new(),
            sun: None,
            ambient: [0.0; 3],
            ev100: 0.0,
            background: [0.0; 3],
            sky_sprites: Vec::new(),
            atmosphere: None,
            sky_points: None,
        }
    }
}

impl Scene {
    /// The exposure multiplier `1 / (1.2 * 2^ev100)`.
    #[must_use]
    pub fn exposure(&self) -> f64 {
        1.0 / (1.2 * det::pow(2.0, self.ev100))
    }
}

/// The bounding sphere of `vertices` (object-space centre offset and radius): the centre of
/// the axis-aligned box and the farthest vertex from it.
pub(crate) fn vertex_bounds(vertices: &[Vertex]) -> (DVec3, f64) {
    let Some(first) = vertices.first() else {
        return (DVec3::ZERO, 0.0);
    };
    let (mut lo, mut hi) = (first.local, first.local);
    for v in vertices {
        lo = DVec3::new(
            lo.x.min(v.local.x),
            lo.y.min(v.local.y),
            lo.z.min(v.local.z),
        );
        hi = DVec3::new(
            hi.x.max(v.local.x),
            hi.y.max(v.local.y),
            hi.z.max(v.local.z),
        );
    }
    let c = (lo + hi) * 0.5;
    let r = vertices
        .iter()
        .map(|v| (v.local - c).length())
        .fold(0.0, f64::max);
    (c, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_winding_faces_outward() {
        for m in [
            MeshData::cube(),
            MeshData::uv_sphere(16, 8),
            MeshData::quad(),
        ] {
            for t in m.indices.as_chunks::<3>().0 {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize]);
                let n = (b.local - a.local).cross(c.local - a.local);
                if n.length() < 1e-12 {
                    continue; // degenerate pole triangle
                }
                assert!(n.dot(a.normal + b.normal + c.normal) > 0.0);
            }
        }
    }

    #[test]
    fn bounds_and_exposure() {
        let (c, r) = MeshData::cube().bounds();
        assert!(c.length() < 1e-12 && (r - 0.75f64.sqrt()).abs() < 1e-12);
        let s = Scene {
            ev100: 0.0,
            ..Scene::default()
        };
        assert!((s.exposure() - 1.0 / 1.2).abs() < 1e-12);
    }
}

// forge-render — shared declarations (prepended to mesh.wgsl and sky.wgsl).
//
// Every position here is CAMERA-RELATIVE f32 in view axes (+x right, +y up, -z forward):
// the CPU resolved it in f64 and narrowed it at the last mile (last_mile.rs, Ch.2.4).
// Nothing on the GPU ever sees a frame-local or world coordinate.

const PI: f32 = 3.14159265358979;

struct Frame {
    // width, height, cluster tile width, cluster tile height (pixels)
    screen: vec4<f32>,
    // cluster tiles x, y, depth slices z, unused
    cluster_grid: vec4<u32>,
    // nearest clustered depth, farthest, slice scale (slices / log2(far/near)), light count
    cluster_depth: vec4<f32>,
    // unit vector toward the sun (view axes), w = 1 when there is a sun
    sun_dir: vec4<f32>,
    // sun colour * illuminance (lux), w = 1 when cascades are bound
    sun_color: vec4<f32>,
    // ambient radiance, w = exposure
    ambient: vec4<f32>,
    // far view depth of each cascade
    cascade_splits: vec4<f32>,
    // world size of one texel of each cascade (normal-offset bias)
    cascade_texel: vec4<f32>,
    // view space -> cascade clip (x, y in -1..1, depth 1 nearest the light)
    cascade: array<mat4x4<f32>, 4>,
    // cascade count; 1 = isotropic Rayleigh phase in the sky pass (the clear-sky golden's W2
    // control only); unused x2
    flags: vec4<u32>,
    // camera minus the atmosphere's ground sphere centre, km, view axes; w = 1 with an atmosphere
    atmo_camera: vec4<f32>,
    // solid angle of one pixel (sr), tan(half vertical fov), aspect, sky point count
    sky: vec4<f32>,
    // the sky points' axes in view axes (columns); pts_x.w = their brightness,
    // pts_y.w = 1 in the hard-disc sprite control, pts_z.w = extra sky evaluations (the
    // perf gate's control)
    pts_x: vec4<f32>,
    pts_y: vec4<f32>,
    pts_z: vec4<f32>,
};

struct PassU {
    // view space -> clip (a shell's reversed-Z projection, or a cascade's matrix)
    proj: mat4x4<f32>,
    // near, far, reversed (1/0), unused
    params: vec4<f32>,
};

struct Instance {
    // object space -> view space (camera-relative translation)
    model_view: mat4x4<f32>,
    n0: vec4<f32>,
    n1: vec4<f32>,
    n2: vec4<f32>,
    // material index, unused x3
    material: vec4<u32>,
};

struct Material {
    base_color: vec4<f32>,
    // emissive rgb, metallic
    emissive_metal: vec4<f32>,
    // perceptual roughness, reflectance, unused x2
    params: vec4<f32>,
};

struct Light {
    // view-space position (camera-relative), range
    pos_range: vec4<f32>,
    // colour * intensity (cd), kind (0 point, 1 spot)
    color_kind: vec4<f32>,
    // spot axis (view axes), angle scale
    dir_scale: vec4<f32>,
    // angle offset, unused x3
    spot: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;
@group(0) @binding(2) var<storage, read> draw_index: array<u32>;
@group(0) @binding(3) var<storage, read> materials: array<Material>;
@group(0) @binding(4) var<storage, read> lights: array<Light>;
@group(0) @binding(5) var<storage, read> clusters: array<vec2<u32>>;
@group(0) @binding(6) var<storage, read> light_index: array<u32>;
@group(1) @binding(0) var<uniform> pass_u: PassU;

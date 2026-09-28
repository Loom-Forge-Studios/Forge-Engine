// ---- sprites and meshes -> G-buffer ------------------------------------------------------

@group(1) @binding(0) var color_tex: texture_2d<f32>;
@group(1) @binding(1) var normal_tex: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;

struct SpriteIn {
    @location(0) center: vec2<f32>,
    @location(1) ax: vec2<f32>,
    @location(2) ay: vec2<f32>,
    @location(3) pivot: vec2<f32>,
    @location(4) uv: vec4<f32>,
    @location(5) tint: vec4<f32>,
    @location(6) flags: u32,
};

struct GIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
    // World-space directions of the image's x and y axes (y up), for the normal map.
    @location(2) basis: vec4<f32>,
};

struct GOut {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
};

@vertex
fn vs_sprite(@builtin(vertex_index) vi: u32, i: SpriteIn) -> GIn {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[vi];
    let l = c - i.pivot;
    let p = i.center + i.ax * l.x + i.ay * l.y;
    var u = mix(i.uv.x, i.uv.z, c.x);
    var v = mix(i.uv.w, i.uv.y, c.y);
    var sx = 1.0;
    var sy = 1.0;
    if ((i.flags & 1u) != 0u) {
        u = mix(i.uv.z, i.uv.x, c.x);
        sx = -1.0;
    }
    if ((i.flags & 2u) != 0u) {
        v = mix(i.uv.y, i.uv.w, c.y);
        sy = -1.0;
    }
    var o: GIn;
    o.pos = to_clip(p);
    o.uv = vec2<f32>(u, v);
    o.tint = i.tint;
    let bx = normalize(vec2<f32>(i.ax.x, -i.ax.y) + vec2<f32>(1e-20, 0.0)) * sx;
    let by = normalize(vec2<f32>(i.ay.x, -i.ay.y) + vec2<f32>(0.0, 1e-20)) * sy;
    o.basis = vec4<f32>(bx, by);
    return o;
}

@fragment
fn fs_gbuffer(i: GIn) -> GOut {
    let c = textureSample(color_tex, samp, i.uv) * i.tint;
    let t = textureSample(normal_tex, samp, i.uv).xyz * 2.0 - 1.0;
    let n = normalize(vec3<f32>(i.basis.xy * t.x + i.basis.zw * t.y, max(t.z, 0.0)) + vec3<f32>(0.0, 0.0, 1e-6));
    var o: GOut;
    o.albedo = c;
    o.normal = vec4<f32>(n * 0.5 + 0.5, c.a);
    return o;
}

struct MeshIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_mesh(i: MeshIn) -> GIn {
    var o: GIn;
    o.pos = to_clip(i.pos);
    o.uv = i.uv;
    o.tint = i.color;
    o.basis = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    return o;
}


// forge-render — clustered forward+ with PBR metal-rough (Ch.10 §10.2-10.5).
//
// BRDF: Lambert diffuse + Cook-Torrance specular with the GGX distribution, the
// height-correlated Smith visibility term and Schlick's Fresnel (Filament's formulation;
// test_pbr checks this shader against an f64 reference). Directional light with cascaded
// shadows; punctual lights from the fragment's cluster only.

@group(2) @binding(0) var shadow_maps: texture_depth_2d_array;
@group(2) @binding(1) var shadow_sampler: sampler_comparison;

// The atmosphere (Ch.11 §11.2): Bruneton tables; a placeholder when the frame has none
// (frame.atmo_camera.w == 0 and nothing is sampled).
@group(3) @binding(0) var<uniform> atm: AtmosphereU;
@group(3) @binding(1) var t_transmittance: texture_2d<f32>;
@group(3) @binding(2) var t_scattering: texture_3d<f32>;
@group(3) @binding(3) var t_irradiance: texture_2d<f32>;
@group(3) @binding(4) var lut_sampler: sampler;
@group(3) @binding(5) var t_single_mie: texture_3d<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) material: u32,
};

@vertex
fn vs_main(
    @location(0) p: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @builtin(instance_index) ii: u32,
) -> VsOut {
    let inst = instances[draw_index[ii]];
    let v = inst.model_view * vec4<f32>(p, 1.0);
    let nm = mat3x3<f32>(inst.n0.xyz, inst.n1.xyz, inst.n2.xyz);
    var out: VsOut;
    out.clip = pass_u.proj * v;
    out.view_pos = v.xyz;
    out.normal = nm * n;
    out.uv = uv;
    out.material = inst.material.x;
    return out;
}

@vertex
fn vs_shadow(@location(0) p: vec3<f32>, @builtin(instance_index) ii: u32) -> @builtin(position) vec4<f32> {
    let inst = instances[draw_index[ii]];
    return pass_u.proj * (inst.model_view * vec4<f32>(p, 1.0));
}

// Mesh pools (WP-U21): every mesh of a pool lives in one arena, 8 floats a vertex (position,
// normal, uv — the vertex buffer's layout); an instance's `material.y` is its mesh's first
// vertex there, so one instanced draw covers every mesh of a pool drawn with one index list.
@group(0) @binding(7) var<storage, read> arena: array<f32>;

fn pulled_p(k: u32) -> vec3<f32> {
    return vec3<f32>(arena[k], arena[k + 1u], arena[k + 2u]);
}

@vertex
fn vs_pulled(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let inst = instances[draw_index[ii]];
    let k = (inst.material.y + vi) * 8u;
    let v = inst.model_view * vec4<f32>(pulled_p(k), 1.0);
    let n = vec3<f32>(arena[k + 3u], arena[k + 4u], arena[k + 5u]);
    let nm = mat3x3<f32>(inst.n0.xyz, inst.n1.xyz, inst.n2.xyz);
    var out: VsOut;
    out.clip = pass_u.proj * v;
    out.view_pos = v.xyz;
    out.normal = nm * n;
    out.uv = vec2<f32>(arena[k + 6u], arena[k + 7u]);
    out.material = inst.material.x;
    return out;
}

@vertex
fn vs_shadow_pulled(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> @builtin(position) vec4<f32> {
    let inst = instances[draw_index[ii]];
    let k = (inst.material.y + vi) * 8u;
    return pass_u.proj * (inst.model_view * vec4<f32>(pulled_p(k), 1.0));
}

fn d_ggx(nh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let f = (nh * a2 - nh) * nh + 1.0;
    return a2 / (PI * f * f);
}

fn v_smith(nv: f32, nl: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = nl * sqrt(nv * nv * (1.0 - a2) + a2);
    let gl = nv * sqrt(nl * nl * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-7);
}

fn f_schlick(f0: vec3<f32>, vh: f32) -> vec3<f32> {
    let f = pow(1.0 - vh, 5.0);
    return f0 + (vec3<f32>(1.0) - f0) * f;
}

// Reflected radiance per unit incident illuminance, times N.L.
fn brdf_nl(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, diffuse: vec3<f32>, f0: vec3<f32>, a: f32) -> vec3<f32> {
    let nl = clamp(dot(n, l), 0.0, 1.0);
    if nl <= 0.0 {
        return vec3<f32>(0.0);
    }
    let h = normalize(v + l);
    let nv = max(dot(n, v), 1e-4);
    let nh = clamp(dot(n, h), 0.0, 1.0);
    let vh = clamp(dot(v, h), 0.0, 1.0);
    let spec = f_schlick(f0, vh) * (d_ggx(nh, a) * v_smith(nv, nl, a));
    return (diffuse / PI + spec) * nl;
}

// `dpx`, `dpy`: the screen derivatives of the view-space position (taken in uniform control
// flow by the caller). They give the receiver's plane in each cascade, so every PCF tap
// compares against the receiver's own depth at that tap (receiver-plane depth bias): a flat
// surface lit at a slant never shadows itself (one depth compared across a 3x3 kernel did —
// rings of acne on open ground under a 40-degree sun, found by test_atmosphere_goldens).
fn sun_shadow(p: vec3<f32>, n: vec3<f32>, l: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>) -> f32 {
    let count = frame.flags.x;
    if frame.sun_color.w < 0.5 || count == 0u {
        return 1.0;
    }
    let d = -p.z;
    var c = count;
    for (var i = 0u; i < count; i++) {
        if d <= frame.cascade_splits[i] {
            c = i;
            break;
        }
    }
    if c >= count {
        return 1.0;
    }
    // Normal offset: push the lookup off the surface by a texel or two (more at grazing
    // angles), so a lit surface never shadows itself.
    let texel = frame.cascade_texel[c];
    let nl = clamp(dot(n, l), 0.0, 1.0);
    let offset = n * texel * (0.6 + 1.4 * (1.0 - nl));
    let lc = frame.cascade[c] * vec4<f32>(p + offset, 1.0);
    let uv = vec2<f32>(lc.x * 0.5 + 0.5, 0.5 - lc.y * 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || lc.z > 1.0 {
        return 1.0;
    }
    // The receiver's depth slope in shadow-map uv: solve the screen-space Jacobian.
    let m = frame.cascade[c];
    let ex = (m * vec4<f32>(dpx, 0.0)).xyz;
    let ey = (m * vec4<f32>(dpy, 0.0)).xyz;
    let du = vec2<f32>(ex.x * 0.5, ey.x * 0.5);
    let dv = vec2<f32>(-ex.y * 0.5, -ey.y * 0.5);
    let det_j = du.x * dv.y - du.y * dv.x;
    var slope = vec2<f32>(0.0);
    if abs(det_j) > 1e-20 {
        // [du.x dv.x; du.y dv.y]^-1 applied to (dz/dx, dz/dy).
        slope = vec2<f32>(dv.y * ex.z - dv.x * ey.z, -du.y * ex.z + du.x * ey.z) / det_j;
    }
    let ts = 1.0 / vec2<f32>(textureDimensions(shadow_maps));
    // A receiver tilted more than this per texel is a silhouette, not a plane.
    let depth_per_m = length(vec3<f32>(m[0].z, m[1].z, m[2].z));
    let max_step = 4.0 * texel * depth_per_m;
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<f32>(f32(x), f32(y)) * ts;
            let dz = clamp(dot(slope, o), -max_step, max_step);
            sum += textureSampleCompareLevel(shadow_maps, shadow_sampler, uv + o, c, lc.z + dz);
        }
    }
    return sum / 9.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let m = materials[in.material];
    let n = normalize(in.normal);
    let v = normalize(-in.view_pos);
    let metal = m.emissive_metal.w;
    let rough = clamp(m.params.x, 0.045, 1.0);
    let a = rough * rough;
    let base = m.base_color.rgb;
    let diffuse = base * (1.0 - metal);
    let r = m.params.y;
    let f0 = mix(vec3<f32>(0.16 * r * r), base, metal);

    // Ambient: a uniform stand-in until GI stage 1 (Ch.10 §10.6).
    var color = frame.ambient.rgb * (diffuse + f0);

    // With an atmosphere: where the fragment is, in km from the ground sphere's centre, and how far
    // it is from the camera (from the camera-relative position, never by subtracting two
    // world-scale coordinates).
    let atmo = frame.atmo_camera.w > 0.5;
    let dpx = dpdx(in.view_pos);
    let dpy = dpdy(in.view_pos);
    let dist = length(in.view_pos);
    let view_dir = in.view_pos / max(dist, 1e-6);
    let p_km = frame.atmo_camera.xyz + in.view_pos * 0.001;

    if frame.sun_dir.w > 0.5 {
        let l = frame.sun_dir.xyz;
        var sun = frame.sun_color.rgb;
        if atmo {
            // The sunlight that reaches this point through the air, and the sky's light.
            let r = length(p_km);
            sun *= transmittance_to_sun(t_transmittance, r, dot(p_km, l) / r);
            let e_sky = sky_irradiance(t_irradiance, p_km, n, l) * frame.sun_color.rgb;
            color += e_sky * diffuse / PI;
        }
        color += brdf_nl(n, v, l, diffuse, f0, a) * sun * sun_shadow(in.view_pos, n, l, dpx, dpy);
    }

    // Punctual lights from this fragment's cluster.
    let d = -in.view_pos.z;
    let zn = frame.cluster_depth.x;
    let zf = frame.cluster_depth.y;
    if d >= zn && d < zf {
        let g = frame.cluster_grid;
        let tx = min(u32(in.clip.x / frame.screen.z), g.x - 1u);
        let ty = min(u32(in.clip.y / frame.screen.w), g.y - 1u);
        let sl = min(u32(max(floor(log2(d / zn) * frame.cluster_depth.z), 0.0)), g.z - 1u);
        let cl = clusters[(sl * g.y + ty) * g.x + tx];
        for (var k = 0u; k < cl.y; k++) {
            let li = lights[light_index[cl.x + k]];
            let to_l = li.pos_range.xyz - in.view_pos;
            let dist2 = max(dot(to_l, to_l), 1e-8);
            let l = to_l * inverseSqrt(dist2);
            let range = li.pos_range.w;
            // Inverse square, windowed smoothly to exactly zero at the range.
            let x = dist2 / (range * range);
            let win = clamp(1.0 - x * x, 0.0, 1.0);
            var att = win * win / dist2;
            if li.color_kind.w > 0.5 {
                let cd = dot(-l, li.dir_scale.xyz);
                let s = clamp(cd * li.dir_scale.w + li.spot.x, 0.0, 1.0);
                att *= s * s;
            }
            color += brdf_nl(n, v, l, diffuse, f0, a) * li.color_kind.rgb * att;
        }
    }

    color += m.emissive_metal.rgb;
    if atmo && frame.sun_dir.w > 0.5 {
        // Aerial perspective: attenuated by the air between, plus the light it scatters in.
        let s = sky_radiance_along(
            t_transmittance, t_scattering, t_single_mie, frame.atmo_camera.xyz, view_dir, dist * 0.001,
            frame.sun_dir.xyz,
        );
        color = color * s.transmittance + s.radiance * frame.sun_color.rgb;
    }
    return vec4<f32>(color * frame.ambient.w, 1.0);
}

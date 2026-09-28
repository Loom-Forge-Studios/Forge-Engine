// forge-render — the atmosphere's table precomputation (Bruneton 2017's passes as compute
// shaders). Appended to atmosphere.wgsl. Every pass reads the tables it needs as sampled
// textures and writes one or two storage textures; accumulations ping-pong between two
// tables (a storage texture is never read and written in one pass).
//
//   transmittance        T(r, mu)                        -> transmittance
//   direct_irradiance    E_direct(r, mu_s)               -> delta_irradiance
//   single_scattering    S1 Rayleigh, S1 Mie             -> delta_rayleigh, delta_mie, scattering
//   for order 2..N:
//     scattering_density J(order) from order-1 and E     -> delta_density
//     indirect_irradiance E(order-1), E_total += E        -> delta_irradiance, irradiance'
//     multiple_scattering S(order) from J, S_total += S  -> delta_multiple, scattering'

@group(0) @binding(0) var<uniform> atm: AtmosphereU;
@group(0) @binding(1) var lut_sampler: sampler;

// Read tables (group 1): whichever the pass needs; unused bindings get placeholders.
@group(1) @binding(0) var t_transmittance: texture_2d<f32>;
@group(1) @binding(1) var t_rayleigh: texture_3d<f32>;
@group(1) @binding(2) var t_mie: texture_3d<f32>;
@group(1) @binding(3) var t_multiple: texture_3d<f32>;
@group(1) @binding(4) var t_irradiance: texture_2d<f32>;
@group(1) @binding(5) var t_density: texture_3d<f32>;
@group(1) @binding(6) var t_scattering: texture_3d<f32>;

// Written tables (group 2).
@group(2) @binding(0) var o_2d_a: texture_storage_2d<rgba16float, write>;
@group(2) @binding(1) var o_2d_b: texture_storage_2d<rgba16float, write>;
@group(2) @binding(2) var o_3d_a: texture_storage_3d<rgba16float, write>;
@group(2) @binding(3) var o_3d_b: texture_storage_3d<rgba16float, write>;
@group(2) @binding(4) var o_3d_c: texture_storage_3d<rgba16float, write>;

const TRANSMITTANCE_STEPS: i32 = 500;
const SCATTERING_STEPS: i32 = 50;
const DENSITY_STEPS: i32 = 16;
const IRRADIANCE_STEPS: i32 = 32;

fn optical_length(profile: u32, r: f32, mu: f32) -> f32 {
    let dx = distance_to_top(r, mu) / f32(TRANSMITTANCE_STEPS);
    var sum = 0.0;
    for (var i = 0; i <= TRANSMITTANCE_STEPS; i++) {
        let d_i = f32(i) * dx;
        let r_i = sqrt(d_i * d_i + 2.0 * r * mu * d_i + r * r);
        let y = profile_density(profile, r_i - a_bottom());
        var w = 1.0;
        if i == 0 || i == TRANSMITTANCE_STEPS {
            w = 0.5;
        }
        sum += y * w * dx;
    }
    return sum;
}

@compute @workgroup_size(8, 8, 1)
fn cs_transmittance(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(u32(atm.sizes0.x), u32(atm.sizes0.y));
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let rm = transmittance_r_mu(uv);
    let tau = atm.rayleigh.rgb * optical_length(0u, rm.x, rm.y)
        + atm.mie_extinction.rgb * optical_length(1u, rm.x, rm.y)
        + atm.absorption.rgb * optical_length(2u, rm.x, rm.y);
    textureStore(o_2d_a, vec2<i32>(id.xy), vec4<f32>(tau, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn cs_direct_irradiance(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(u32(atm.sizes0.z), u32(atm.sizes0.w));
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let rm = irradiance_r_mu_s(uv);
    let alpha = atm.rayleigh.w;
    let mu_s = rm.y;
    // The sun's disc partly below the horizon: the average cosine over it.
    var f = mu_s;
    if mu_s < -alpha {
        f = 0.0;
    } else if mu_s <= alpha {
        f = (mu_s + alpha) * (mu_s + alpha) / (4.0 * alpha);
    }
    let e = transmittance_to_top(t_transmittance, rm.x, mu_s) * f;
    textureStore(o_2d_a, vec2<i32>(id.xy), vec4<f32>(e, 1.0));
}

// Sample `i` of `n` along a ray of length `d`: (distance, quadrature weight). Samples crowd
// toward the densest air — the start of a ray that leaves the atmosphere (the viewer's own
// altitude), the end of one that meets the ground — by t = d s^2 (or its mirror), with the
// trapezoid rule in s. Uniform steps of a 600 km horizon ray are 12 km apart, ten times the
// haze's scale height: test_atmosphere measured 16 % errors that way.
fn ray_sample(i: i32, n: i32, d: f32, ground: bool) -> vec2<f32> {
    let s = f32(i) / f32(n);
    var t: f32;
    var dt: f32;
    if ground {
        t = d * (1.0 - (1.0 - s) * (1.0 - s));
        dt = 2.0 * d * (1.0 - s);
    } else {
        t = d * s * s;
        dt = 2.0 * d * s;
    }
    var w = 1.0;
    if i == 0 || i == n {
        w = 0.5;
    }
    return vec2<f32>(t, w * dt / f32(n));
}

fn scattering_size() -> vec3<u32> {
    return vec3<u32>(u32(atm.sizes1.w * atm.sizes1.z), u32(atm.sizes1.y), u32(atm.sizes1.x));
}

@compute @workgroup_size(4, 4, 4)
fn cs_single_scattering(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = scattering_size();
    if any(id >= size) {
        return;
    }
    let p = scattering_params_at(id);
    let len = distance_to_nearest(p.r, p.mu, p.ground);
    var ray = vec3<f32>(0.0);
    var mie = vec3<f32>(0.0);
    for (var i = 0; i <= SCATTERING_STEPS; i++) {
        let tw = ray_sample(i, SCATTERING_STEPS, len, p.ground);
        let d = tw.x;
        let r_d = clamp_radius(sqrt(d * d + 2.0 * p.r * p.mu * d + p.r * p.r));
        let mu_s_d = clamp_cosine((p.r * p.mu_s + d * p.nu) / r_d);
        let t = transmittance_segment(t_transmittance, p.r, p.mu, d, p.ground)
            * transmittance_to_sun(t_transmittance, r_d, mu_s_d);
        ray += t * profile_density(0u, r_d - a_bottom()) * tw.y;
        mie += t * profile_density(1u, r_d - a_bottom()) * tw.y;
    }
    ray *= atm.rayleigh.rgb;
    mie *= atm.mie_scattering.rgb;
    let c = vec3<i32>(id);
    textureStore(o_3d_a, c, vec4<f32>(ray, 1.0));
    textureStore(o_3d_b, c, vec4<f32>(mie, 1.0));
    textureStore(o_3d_c, c, vec4<f32>(ray, 1.0));
}

// Scattered radiance of order `order` arriving at (r, mu) — single scattering with its phase
// functions for order 1, the previous multiple-scattering table after.
fn scattering_of_order(r: f32, mu: f32, mu_s: f32, nu: f32, ground: bool, order: u32) -> vec3<f32> {
    if order == 1u {
        let ray = sample_scattering(t_rayleigh, r, mu, mu_s, nu, ground).rgb;
        let mie = sample_scattering(t_mie, r, mu, mu_s, nu, ground).rgb;
        return ray * rayleigh_phase(nu) + mie * mie_phase(nu);
    }
    return sample_scattering(t_multiple, r, mu, mu_s, nu, ground).rgb;
}

@compute @workgroup_size(4, 4, 4)
fn cs_scattering_density(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = scattering_size();
    if any(id >= size) {
        return;
    }
    let order = atm.misc.x;
    let p = scattering_params_at(id);
    let zenith = vec3<f32>(0.0, 0.0, 1.0);
    let omega = vec3<f32>(safe_sqrt(1.0 - p.mu * p.mu), 0.0, p.mu);
    var sx = 0.0;
    if omega.x != 0.0 {
        sx = (p.nu - p.mu * p.mu_s) / omega.x;
    }
    let sy = safe_sqrt(1.0 - sx * sx - p.mu_s * p.mu_s);
    let omega_s = vec3<f32>(sx, sy, p.mu_s);
    let dphi = API / f32(DENSITY_STEPS);
    let dtheta = API / f32(DENSITY_STEPS);
    let h = p.r - a_bottom();
    let ray_d = atm.rayleigh.rgb * profile_density(0u, h);
    let mie_d = atm.mie_scattering.rgb * profile_density(1u, h);
    var sum = vec3<f32>(0.0);
    for (var l = 0; l < DENSITY_STEPS; l++) {
        let theta = (f32(l) + 0.5) * dtheta;
        let ct = cos(theta);
        let st = sin(theta);
        let ground_i = ray_hits_ground(p.r, ct);
        var dist_ground = 0.0;
        var t_ground = vec3<f32>(0.0);
        var albedo = vec3<f32>(0.0);
        if ground_i {
            dist_ground = distance_to_bottom(p.r, ct);
            t_ground = transmittance_segment(t_transmittance, p.r, ct, dist_ground, true);
            albedo = atm.ground_albedo.rgb;
        }
        for (var m = 0; m < 2 * DENSITY_STEPS; m++) {
            let phi = (f32(m) + 0.5) * dphi;
            let omega_i = vec3<f32>(cos(phi) * st, sin(phi) * st, ct);
            let domega = dtheta * dphi * st;
            let nu1 = dot(omega_s, omega_i);
            var incident = scattering_of_order(p.r, omega_i.z, p.mu_s, nu1, ground_i, order - 1u);
            let n_ground = normalize(zenith * p.r + omega_i * dist_ground);
            let e_ground = sample_irradiance(t_irradiance, a_bottom(), dot(n_ground, omega_s));
            incident += t_ground * albedo * (1.0 / API) * e_ground;
            let nu2 = dot(omega, omega_i);
            sum += incident * (ray_d * rayleigh_phase(nu2) + mie_d * mie_phase(nu2)) * domega;
        }
    }
    textureStore(o_3d_a, vec3<i32>(id), vec4<f32>(sum, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn cs_indirect_irradiance(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(u32(atm.sizes0.z), u32(atm.sizes0.w));
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    // This pass integrates order `misc.x - 1`.
    let order = atm.misc.x - 1u;
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let rm = irradiance_r_mu_s(uv);
    let r = rm.x;
    let mu_s = rm.y;
    let dphi = API / f32(IRRADIANCE_STEPS);
    let dtheta = API / f32(IRRADIANCE_STEPS);
    let omega_s = vec3<f32>(safe_sqrt(1.0 - mu_s * mu_s), 0.0, mu_s);
    var sum = vec3<f32>(0.0);
    for (var j = 0; j < IRRADIANCE_STEPS / 2; j++) {
        let theta = (f32(j) + 0.5) * dtheta;
        for (var i = 0; i < 2 * IRRADIANCE_STEPS; i++) {
            let phi = (f32(i) + 0.5) * dphi;
            let omega = vec3<f32>(cos(phi) * sin(theta), sin(phi) * sin(theta), cos(theta));
            let domega = dtheta * dphi * sin(theta);
            let nu = dot(omega, omega_s);
            sum += scattering_of_order(r, omega.z, mu_s, nu, false, order) * omega.z * domega;
        }
    }
    let prev = textureLoad(t_irradiance, vec2<i32>(id.xy), 0).rgb;
    textureStore(o_2d_a, vec2<i32>(id.xy), vec4<f32>(sum, 1.0));
    textureStore(o_2d_b, vec2<i32>(id.xy), vec4<f32>(prev + sum, 1.0));
}

@compute @workgroup_size(4, 4, 4)
fn cs_multiple_scattering(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = scattering_size();
    if any(id >= size) {
        return;
    }
    let p = scattering_params_at(id);
    let len = distance_to_nearest(p.r, p.mu, p.ground);
    var sum = vec3<f32>(0.0);
    for (var i = 0; i <= SCATTERING_STEPS; i++) {
        let tw = ray_sample(i, SCATTERING_STEPS, len, p.ground);
        let d = tw.x;
        let r_i = clamp_radius(sqrt(d * d + 2.0 * p.r * p.mu * d + p.r * p.r));
        let mu_i = clamp_cosine((p.r * p.mu + d) / r_i);
        let mu_s_i = clamp_cosine((p.r * p.mu_s + d * p.nu) / r_i);
        let j = sample_scattering(t_density, r_i, mu_i, mu_s_i, p.nu, p.ground).rgb;
        sum += j * transmittance_segment(t_transmittance, p.r, p.mu, d, p.ground) * tw.y;
    }
    let c = vec3<i32>(id);
    let prev = textureLoad(t_scattering, c, 0);
    textureStore(o_3d_a, c, vec4<f32>(sum, 1.0));
    textureStore(o_3d_b, c, prev + vec4<f32>(sum / rayleigh_phase(p.nu), 0.0));
}

// ---- probe: the rendering functions evaluated at given rays (for the f64 reference guard) ----

struct ProbeIn {
    // camera (km from the centre), mode (0 sky, 1 to point, 2 transmittance to sun, 3 sky irradiance)
    camera: vec4<f32>,
    // view direction or normal (unit), unused
    dir: vec4<f32>,
    // sun direction (unit)
    sun: vec4<f32>,
    // point (km from the centre) for mode 1
    point: vec4<f32>,
};

struct ProbeOut {
    radiance: vec4<f32>,
    transmittance: vec4<f32>,
};

@group(3) @binding(0) var<storage, read> probe_in: array<ProbeIn>;
@group(3) @binding(1) var<storage, read_write> probe_out: array<ProbeOut>;

@compute @workgroup_size(64, 1, 1)
fn cs_probe(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&probe_in) {
        return;
    }
    let q = probe_in[id.x];
    let mode = u32(q.camera.w);
    var out = ProbeOut(vec4<f32>(0.0), vec4<f32>(0.0));
    if mode == 0u {
        let s = sky_radiance(t_transmittance, t_scattering, t_mie, q.camera.xyz, q.dir.xyz, q.sun.xyz);
        out = ProbeOut(vec4<f32>(s.radiance, 0.0), vec4<f32>(s.transmittance, 0.0));
    } else if mode == 1u {
        let s = sky_radiance_to_point(t_transmittance, t_scattering, t_mie, q.camera.xyz, q.point.xyz, q.sun.xyz);
        out = ProbeOut(vec4<f32>(s.radiance, 0.0), vec4<f32>(s.transmittance, 0.0));
    } else if mode == 2u {
        let r = length(q.camera.xyz);
        let t = transmittance_to_sun(t_transmittance, r, dot(q.camera.xyz, q.sun.xyz) / r);
        out = ProbeOut(vec4<f32>(0.0), vec4<f32>(t, 0.0));
    } else {
        let e = sky_irradiance(t_irradiance, q.camera.xyz, q.dir.xyz, q.sun.xyz);
        out = ProbeOut(vec4<f32>(e, 0.0), vec4<f32>(0.0));
    }
    probe_out[id.x] = out;
}

// forge-render — Bruneton's precomputed atmospheric scattering (Bruneton & Neyret 2008, and
// Bruneton's 2017 reference implementation, re-derived here in WGSL): the functions shared by
// the table precomputation (atmosphere_precompute.wgsl) and the renderer (sky.wgsl, mesh.wgsl).
//
// Lengths are KILOMETRES, coefficients per km; everything is per unit solar irradiance (the
// renderer multiplies by the sun's illuminance). The includer declares
//   var<uniform> atm: AtmosphereU;   var lut_sampler: sampler;
// at its own group/binding; tables are passed as arguments so the precompute passes can read
// whichever intermediate table they need.
//
// Parameterisation (Bruneton 2017): transmittance T(r, mu) in a 2D table; scattering
// S(r, mu, mu_s, nu) in a 3D table of NU slices side by side; ground irradiance E(r, mu_s) in
// a 2D table. The scattering table holds Rayleigh plus every multiple order (divided by the
// Rayleigh phase function); single Mie scattering has a table of its own (all three channels:
// Bruneton's red-only extrapolation is wrong by ~18 % at a low sun under wavelength-dependent
// haze, measured by test_atmosphere).

const API: f32 = 3.14159265358979;

struct AtmosphereU {
    // bottom radius, top radius (km), Mie g, mu_s_min
    radii: vec4<f32>,
    // Rayleigh scattering (1/km); w = sun angular radius (rad)
    rayleigh: vec4<f32>,
    mie_scattering: vec4<f32>,
    mie_extinction: vec4<f32>,
    absorption: vec4<f32>,
    ground_albedo: vec4<f32>,
    // Density layers (Rayleigh lower/upper, Mie lower/upper, absorption lower/upper), two
    // vec4 each: (width km, exp_term, exp_scale 1/km, linear_term 1/km), (constant, -, -, -).
    layers: array<vec4<f32>, 12>,
    // transmittance width, height; irradiance width, height
    sizes0: vec4<f32>,
    // scattering R, MU, MU_S, NU
    sizes1: vec4<f32>,
    // scattering order (precompute), unused x3
    misc: vec4<u32>,
};

fn a_bottom() -> f32 { return atm.radii.x; }
fn a_top() -> f32 { return atm.radii.y; }

fn clamp_cosine(mu: f32) -> f32 { return clamp(mu, -1.0, 1.0); }
fn clamp_distance(d: f32) -> f32 { return max(d, 0.0); }
fn clamp_radius(r: f32) -> f32 { return clamp(r, a_bottom(), a_top()); }
fn safe_sqrt(a: f32) -> f32 { return sqrt(max(a, 0.0)); }

fn layer_density(i: u32, h: f32) -> f32 {
    let a = atm.layers[2u * i];
    let b = atm.layers[2u * i + 1u];
    return clamp(a.y * exp(a.z * h) + a.w * h + b.x, 0.0, 1.0);
}

// Profile 0 = Rayleigh, 1 = Mie, 2 = absorption.
fn profile_density(p: u32, h: f32) -> f32 {
    if h < atm.layers[4u * p].x {
        return layer_density(2u * p, h);
    }
    return layer_density(2u * p + 1u, h);
}

fn distance_to_top(r: f32, mu: f32) -> f32 {
    let disc = r * r * (mu * mu - 1.0) + a_top() * a_top();
    return clamp_distance(-r * mu + safe_sqrt(disc));
}

fn distance_to_bottom(r: f32, mu: f32) -> f32 {
    let disc = r * r * (mu * mu - 1.0) + a_bottom() * a_bottom();
    return clamp_distance(-r * mu - safe_sqrt(disc));
}

fn ray_hits_ground(r: f32, mu: f32) -> bool {
    return mu < 0.0 && r * r * (mu * mu - 1.0) + a_bottom() * a_bottom() >= 0.0;
}

fn distance_to_nearest(r: f32, mu: f32, ground: bool) -> f32 {
    if ground {
        return distance_to_bottom(r, mu);
    }
    return distance_to_top(r, mu);
}

fn coord_from_unit(x: f32, size: f32) -> f32 { return 0.5 / size + x * (1.0 - 1.0 / size); }
fn unit_from_coord(u: f32, size: f32) -> f32 { return (u - 0.5 / size) / (1.0 - 1.0 / size); }

// W2 control of the clear-sky golden only (set by the sky pass from frame.flags.y): an isotropic
// phase in place of Rayleigh's. False everywhere else, including the table precomputation.
var<private> rayleigh_isotropic: bool = false;

fn rayleigh_phase(nu: f32) -> f32 {
    if rayleigh_isotropic {
        return 1.0 / (4.0 * API);
    }
    return 3.0 / (16.0 * API) * (1.0 + nu * nu);
}

fn mie_phase(nu: f32) -> f32 {
    let g = atm.radii.z;
    let k = 3.0 / (8.0 * API) * (1.0 - g * g) / (2.0 + g * g);
    return k * (1.0 + nu * nu) / pow(1.0 + g * g - 2.0 * g * nu, 1.5);
}

// ---- transmittance ----------------------------------------------------------------------

fn transmittance_uv(r: f32, mu: f32) -> vec2<f32> {
    let h = sqrt(a_top() * a_top() - a_bottom() * a_bottom());
    let rho = safe_sqrt(r * r - a_bottom() * a_bottom());
    let d = distance_to_top(r, mu);
    let d_min = a_top() - r;
    let d_max = rho + h;
    let x_mu = (d - d_min) / (d_max - d_min);
    let x_r = rho / h;
    return vec2<f32>(coord_from_unit(x_mu, atm.sizes0.x), coord_from_unit(x_r, atm.sizes0.y));
}

// (r, mu) of a transmittance texel (uv in 0..1).
fn transmittance_r_mu(uv: vec2<f32>) -> vec2<f32> {
    let x_mu = unit_from_coord(uv.x, atm.sizes0.x);
    let x_r = unit_from_coord(uv.y, atm.sizes0.y);
    let h = sqrt(a_top() * a_top() - a_bottom() * a_bottom());
    let rho = h * x_r;
    let r = sqrt(rho * rho + a_bottom() * a_bottom());
    let d_min = a_top() - r;
    let d_max = rho + h;
    let d = d_min + x_mu * (d_max - d_min);
    var mu = 1.0;
    if d != 0.0 {
        mu = clamp_cosine((h * h - rho * rho - d * d) / (2.0 * r * d));
    }
    return vec2<f32>(r, mu);
}

// The transmittance table holds OPTICAL DEPTH to the top, not its exponential: a grazing
// ray's blue transmittance (e^-11 and less) is subnormal in Rgba16Float, and Bruneton's ratio
// of two such values is noise (a bright line along the horizon on the software rasteriser).
// Depth is well conditioned in half floats, interpolates better (log-space), and a segment's
// transmittance is exp of a difference — no division.
fn optical_depth_to_top(tt: texture_2d<f32>, r: f32, mu: f32) -> vec3<f32> {
    return textureSampleLevel(tt, lut_sampler, transmittance_uv(r, mu), 0.0).rgb;
}

fn transmittance_to_top(tt: texture_2d<f32>, r: f32, mu: f32) -> vec3<f32> {
    return exp(-optical_depth_to_top(tt, r, mu));
}

// Transmittance over a segment of length d from (r, mu), via two table lookups.
fn transmittance_segment(tt: texture_2d<f32>, r: f32, mu: f32, d: f32, ground: bool) -> vec3<f32> {
    let r_d = clamp_radius(sqrt(d * d + 2.0 * r * mu * d + r * r));
    let mu_d = clamp_cosine((r * mu + d) / r_d);
    var tau: vec3<f32>;
    if ground {
        tau = optical_depth_to_top(tt, r_d, -mu_d) - optical_depth_to_top(tt, r, -mu);
    } else {
        tau = optical_depth_to_top(tt, r, mu) - optical_depth_to_top(tt, r_d, mu_d);
    }
    return exp(-max(tau, vec3<f32>(0.0)));
}

// Toward the sun: the fraction of its disc above the horizon, smoothed.
fn transmittance_to_sun(tt: texture_2d<f32>, r: f32, mu_s: f32) -> vec3<f32> {
    let sin_h = a_bottom() / r;
    let cos_h = -safe_sqrt(1.0 - sin_h * sin_h);
    let a = sin_h * atm.rayleigh.w;
    return transmittance_to_top(tt, r, mu_s) * smoothstep(-a, a, mu_s - cos_h);
}

// ---- scattering table -------------------------------------------------------------------

fn scattering_uvwz(r: f32, mu: f32, mu_s: f32, nu: f32, ground: bool) -> vec4<f32> {
    let bottom = a_bottom();
    let top = a_top();
    let h = sqrt(top * top - bottom * bottom);
    let rho = safe_sqrt(r * r - bottom * bottom);
    let u_r = coord_from_unit(rho / h, atm.sizes1.x);
    let r_mu = r * mu;
    let disc = r_mu * r_mu - r * r + bottom * bottom;
    let half_mu = atm.sizes1.y * 0.5;
    var u_mu: f32;
    if ground {
        let d = -r_mu - safe_sqrt(disc);
        let d_min = r - bottom;
        let d_max = rho;
        var x = 0.0;
        if d_max != d_min {
            x = (d - d_min) / (d_max - d_min);
        }
        u_mu = 0.5 - 0.5 * coord_from_unit(x, half_mu);
    } else {
        let d = -r_mu + safe_sqrt(disc + h * h);
        let d_min = top - r;
        let d_max = rho + h;
        u_mu = 0.5 + 0.5 * coord_from_unit((d - d_min) / (d_max - d_min), half_mu);
    }
    let d = distance_to_top(bottom, mu_s);
    let d_min = top - bottom;
    let d_max = h;
    let a = (d - d_min) / (d_max - d_min);
    let d_big = distance_to_top(bottom, atm.radii.w);
    let a_big = (d_big - d_min) / (d_max - d_min);
    let u_mu_s = coord_from_unit(max(1.0 - a / a_big, 0.0) / (1.0 + a), atm.sizes1.z);
    let u_nu = (nu + 1.0) * 0.5;
    return vec4<f32>(u_nu, u_mu_s, u_mu, u_r);
}

struct ScatterParams {
    r: f32,
    mu: f32,
    mu_s: f32,
    nu: f32,
    ground: bool,
};

fn scattering_params(uvwz: vec4<f32>) -> ScatterParams {
    let bottom = a_bottom();
    let top = a_top();
    let h = sqrt(top * top - bottom * bottom);
    let rho = h * unit_from_coord(uvwz.w, atm.sizes1.x);
    let r = sqrt(rho * rho + bottom * bottom);
    let half_mu = atm.sizes1.y * 0.5;
    var mu: f32;
    var ground: bool;
    if uvwz.z < 0.5 {
        let d_min = r - bottom;
        let d_max = rho;
        let d = d_min + (d_max - d_min) * unit_from_coord(1.0 - 2.0 * uvwz.z, half_mu);
        mu = -1.0;
        if d != 0.0 {
            mu = clamp_cosine(-(rho * rho + d * d) / (2.0 * r * d));
        }
        ground = true;
    } else {
        let d_min = top - r;
        let d_max = rho + h;
        let d = d_min + (d_max - d_min) * unit_from_coord(2.0 * uvwz.z - 1.0, half_mu);
        mu = 1.0;
        if d != 0.0 {
            mu = clamp_cosine((h * h - rho * rho - d * d) / (2.0 * r * d));
        }
        ground = false;
    }
    let x_mu_s = unit_from_coord(uvwz.y, atm.sizes1.z);
    let d_min = top - bottom;
    let d_max = h;
    let d_big = distance_to_top(bottom, atm.radii.w);
    let a_big = (d_big - d_min) / (d_max - d_min);
    let a = (a_big - x_mu_s * a_big) / (1.0 + x_mu_s * a_big);
    let d = d_min + min(a, a_big) * (d_max - d_min);
    var mu_s = 1.0;
    if d != 0.0 {
        mu_s = clamp_cosine((h * h - d * d) / (2.0 * bottom * d));
    }
    let nu = clamp_cosine(uvwz.x * 2.0 - 1.0);
    return ScatterParams(r, mu, mu_s, nu, ground);
}

// Parameters of a 3D-texel (integer coordinates) of the scattering table.
fn scattering_params_at(texel: vec3<u32>) -> ScatterParams {
    let mu_s_size = atm.sizes1.z;
    let fx = f32(texel.x) + 0.5;
    let frag_nu = floor(fx / mu_s_size);
    let frag_mu_s = fx - frag_nu * mu_s_size;
    let size = vec4<f32>(atm.sizes1.w - 1.0, mu_s_size, atm.sizes1.y, atm.sizes1.x);
    let uvwz = vec4<f32>(frag_nu, frag_mu_s, f32(texel.y) + 0.5, f32(texel.z) + 0.5) / size;
    var p = scattering_params(uvwz);
    // Keep nu consistent with mu and mu_s.
    let s = safe_sqrt((1.0 - p.mu * p.mu) * (1.0 - p.mu_s * p.mu_s));
    p.nu = clamp(p.nu, p.mu * p.mu_s - s, p.mu * p.mu_s + s);
    return p;
}

fn sample_scattering(st: texture_3d<f32>, r: f32, mu: f32, mu_s: f32, nu: f32, ground: bool) -> vec4<f32> {
    let uvwz = scattering_uvwz(r, mu, mu_s, nu, ground);
    let nu_size = atm.sizes1.w;
    let tx = uvwz.x * (nu_size - 1.0);
    let t0 = floor(tx);
    let l = tx - t0;
    let uvw0 = vec3<f32>((t0 + uvwz.y) / nu_size, uvwz.z, uvwz.w);
    let uvw1 = vec3<f32>((t0 + 1.0 + uvwz.y) / nu_size, uvwz.z, uvwz.w);
    return mix(
        textureSampleLevel(st, lut_sampler, uvw0, 0.0),
        textureSampleLevel(st, lut_sampler, uvw1, 0.0),
        l,
    );
}

// ---- irradiance table -------------------------------------------------------------------

fn irradiance_uv(r: f32, mu_s: f32) -> vec2<f32> {
    let x_r = (r - a_bottom()) / (a_top() - a_bottom());
    let x_mu_s = mu_s * 0.5 + 0.5;
    return vec2<f32>(coord_from_unit(x_mu_s, atm.sizes0.z), coord_from_unit(x_r, atm.sizes0.w));
}

fn irradiance_r_mu_s(uv: vec2<f32>) -> vec2<f32> {
    let x_mu_s = unit_from_coord(uv.x, atm.sizes0.z);
    let x_r = unit_from_coord(uv.y, atm.sizes0.w);
    return vec2<f32>(a_bottom() + x_r * (a_top() - a_bottom()), clamp_cosine(2.0 * x_mu_s - 1.0));
}

fn sample_irradiance(it: texture_2d<f32>, r: f32, mu_s: f32) -> vec3<f32> {
    return textureSampleLevel(it, lut_sampler, irradiance_uv(r, mu_s), 0.0).rgb;
}

// ---- rendering --------------------------------------------------------------------------


struct SkyResult {
    radiance: vec3<f32>,
    transmittance: vec3<f32>,
};

// Radiance of the sky seen from `camera` (km, from the ground sphere's centre) along the unit `view`,
// the sun along the unit `sun`; transmittance to the top (0 into the ground).
fn sky_radiance(
    tt: texture_2d<f32>, st: texture_3d<f32>, mt: texture_3d<f32>,
    camera_in: vec3<f32>, view: vec3<f32>, sun: vec3<f32>,
) -> SkyResult {
    var camera = camera_in;
    var r = length(camera);
    var rmu = dot(camera, view);
    let top = a_top();
    let dtt = -rmu - safe_sqrt(rmu * rmu - r * r + top * top);
    if dtt > 0.0 {
        camera = camera + view * dtt;
        r = top;
        rmu += dtt;
    } else if r > top {
        return SkyResult(vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let mu = rmu / r;
    let mu_s = dot(camera, sun) / r;
    let nu = dot(view, sun);
    let ground = ray_hits_ground(r, mu);
    var trans = vec3<f32>(0.0);
    if !ground {
        trans = transmittance_to_top(tt, r, mu);
    }
    let c = sample_scattering(st, r, mu, mu_s, nu, ground);
    let m = sample_scattering(mt, r, mu, mu_s, nu, ground).rgb;
    let rad = c.rgb * rayleigh_phase(nu) + m * mie_phase(nu);
    return SkyResult(rad, trans);
}

// Radiance scattered toward `camera` between it and `point` (both km from the centre), and
// the transmittance between them (aerial perspective).
fn sky_radiance_to_point(
    tt: texture_2d<f32>, st: texture_3d<f32>, mt: texture_3d<f32>,
    camera_in: vec3<f32>, point: vec3<f32>, sun: vec3<f32>,
) -> SkyResult {
    let d = length(point - camera_in);
    return sky_radiance_along(tt, st, mt, camera_in, (point - camera_in) / max(d, 1e-9), d, sun);
}

// The same for the point `d_in` km along the unit `view` from `camera_in`: the renderer knows
// a fragment's camera-relative offset exactly, which the difference of two world-scale f32
// positions would not give it.
fn sky_radiance_along(
    tt: texture_2d<f32>, st: texture_3d<f32>, mt: texture_3d<f32>,
    camera_in: vec3<f32>, view: vec3<f32>, d_in: f32, sun: vec3<f32>,
) -> SkyResult {
    var camera = camera_in;
    var d = d_in;
    var r = length(camera);
    var rmu = dot(camera, view);
    let top = a_top();
    let dtt = -rmu - safe_sqrt(rmu * rmu - r * r + top * top);
    if dtt > 0.0 {
        if d <= dtt {
            // The point is outside the atmosphere, in front of it.
            return SkyResult(vec3<f32>(0.0), vec3<f32>(1.0));
        }
        camera = camera + view * dtt;
        r = top;
        rmu += dtt;
        d -= dtt;
    }
    let mu = rmu / r;
    let mu_s = dot(camera, sun) / r;
    let nu = dot(view, sun);
    let ground = ray_hits_ground(r, mu);
    let trans = transmittance_segment(tt, r, mu, d, ground);
    let c0 = sample_scattering(st, r, mu, mu_s, nu, ground);
    let r_p = clamp_radius(sqrt(d * d + 2.0 * r * mu * d + r * r));
    let mu_p = (r * mu + d) / r_p;
    let mu_s_p = (r * mu_s + d * nu) / r_p;
    let c1 = sample_scattering(st, r_p, mu_p, mu_s_p, nu, ground);
    let ray = max(c0.rgb - trans * c1.rgb, vec3<f32>(0.0));
    let m0 = sample_scattering(mt, r, mu, mu_s, nu, ground).rgb;
    let m1 = sample_scattering(mt, r_p, mu_p, mu_s_p, nu, ground).rgb;
    var mie = max(m0 - trans * m1, vec3<f32>(0.0));
    // Bruneton's guard against artefacts when the sun is below the horizon.
    mie = mie * smoothstep(0.0, 0.01, mu_s);
    return SkyResult(ray * rayleigh_phase(nu) + mie * mie_phase(nu), trans);
}

// Sky irradiance on a surface at `point` with unit `normal` (the sun's direct part is the
// caller's: transmittance_to_sun times the cosine).
fn sky_irradiance(it: texture_2d<f32>, point: vec3<f32>, normal: vec3<f32>, sun: vec3<f32>) -> vec3<f32> {
    let r = length(point);
    let mu_s = dot(point, sun) / r;
    return sample_irradiance(it, r, mu_s) * (1.0 + dot(normal, point) / r) * 0.5;
}

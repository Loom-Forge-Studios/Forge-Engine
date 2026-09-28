// forge-render — the Sky shell (drawn first, no depth): the atmosphere's sky radiance over
// the whole target (Bruneton tables), then the sky points (lights at infinity, point sources
// or Gaussians), then the sky sprites — every one a flux-conserving sprite, attenuated by the
// atmosphere along its line of sight. Appended to common.wgsl and atmosphere.wgsl.
//
// Photometry: a sprite carries its illuminance at the camera (lux, times exposure). Sub-pixel
// sources are drawn as a Gaussian point-spread function whose integral over the image is
// exactly that flux, sampled at pixel centres — so a point crossing the screen by sub-pixel
// steps keeps its brightness (no shimmer). Resolved discs spread the flux over their solid
// angle; a lit disc is shaded as a Lambertian sphere lit from its light and normalised by the
// sprite's own factor, so it shows its phase and still integrates to the flux it carries.

struct Sprite {
    // unit direction (view axes), true angular radius in pixels
    dir_px: vec4<f32>,
    // flux (lux x exposure, rgb) — or, for kind 0, radiance; w = kind
    // (0 legacy disc, 1 emitter, 2 lit)
    flux: vec4<f32>,
    // direction from the sprite toward its light (view axes); w = the lit disc's normalisation
    light: vec4<f32>,
    pad: vec4<f32>,
};

struct SkyPointRec {
    // unit direction (the points' axes), illuminance (lux)
    dir_lux: vec4<f32>,
    // colour (unit luminance), Gaussian sigma in radians (0 = point source)
    color_sigma: vec4<f32>,
};

@group(2) @binding(0) var<storage, read> sprites: array<Sprite>;
@group(2) @binding(1) var<storage, read> points: array<SkyPointRec>;

@group(3) @binding(0) var<uniform> atm: AtmosphereU;
@group(3) @binding(1) var t_transmittance: texture_2d<f32>;
@group(3) @binding(2) var t_scattering: texture_3d<f32>;
@group(3) @binding(3) var t_irradiance: texture_2d<f32>;
@group(3) @binding(4) var lut_sampler: sampler;
@group(3) @binding(5) var t_single_mie: texture_3d<f32>;

// Point-spread function of a sub-pixel source, pixels.
const PSF_SIGMA: f32 = 0.7;
// The quad reaches this many sigmas (the mass beyond 4 sigma is 3e-4).
const PSF_REACH: f32 = 4.0;

fn exposure() -> f32 {
    return frame.ambient.w;
}

// Transmittance from the camera along the unit view direction `dir` out of the atmosphere
// (1 without an atmosphere, 0 into the ground).
fn view_transmittance(dir: vec3<f32>) -> vec3<f32> {
    if frame.atmo_camera.w < 0.5 {
        return vec3<f32>(1.0);
    }
    let cam = frame.atmo_camera.xyz;
    var r = length(cam);
    var rmu = dot(cam, dir);
    let top = a_top();
    if r > top {
        let disc = rmu * rmu - r * r + top * top;
        let dtt = -rmu - safe_sqrt(disc);
        if disc < 0.0 || dtt < 0.0 {
            return vec3<f32>(1.0);
        }
        r = top;
        rmu += dtt;
    }
    let mu = rmu / r;
    if ray_hits_ground(r, mu) {
        return vec3<f32>(0.0);
    }
    return transmittance_to_top(t_transmittance, r, mu);
}

// ---- the sky background -----------------------------------------------------------------

struct BgOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_sky_bg(@builtin(vertex_index) vi: u32) -> BgOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: BgOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_sky_bg(in: BgOut) -> @location(0) vec4<f32> {
    let ndc = vec2<f32>(
        in.clip.x / frame.screen.x * 2.0 - 1.0,
        1.0 - in.clip.y / frame.screen.y * 2.0,
    );
    let tan_half = frame.sky.y;
    let dir = normalize(vec3<f32>(ndc.x * tan_half * frame.sky.z, ndc.y * tan_half, -1.0));
    rayleigh_isotropic = frame.flags.y == 1u;
    let s = sky_radiance(t_transmittance, t_scattering, t_single_mie, frame.atmo_camera.xyz, dir, frame.sun_dir.xyz);
    var rad = s.radiance;
    // W2 positive control of the perf gate (frame.pts_z.w > 0 only there): evaluate the sky
    // that many more times, so the shell.sky pass regresses.
    let repeats = u32(frame.pts_z.w);
    for (var i = 0u; i < repeats; i++) {
        // i + 1: a zero offset on the first repeat let a compiler (WARP) merge it with the
        // evaluation above, so one repeat cost nothing there.
        let d2 = normalize(dir + vec3<f32>(f32(i + 1u) * 1e-7));
        rad += sky_radiance(t_transmittance, t_scattering, t_single_mie, frame.atmo_camera.xyz, d2, frame.sun_dir.xyz).radiance;
    }
    rad /= f32(repeats + 1u);
    var l = vec3<f32>(0.0);
    if frame.sun_dir.w > 0.5 {
        l = rad * frame.sun_color.rgb;
    }
    return vec4<f32>(l * exposure(), 1.0);
}

// ---- point sources and discs --------------------------------------------------------------

struct SpriteOut {
    @builtin(position) clip: vec4<f32>,
    // offset from the source's centre, pixels (y up)
    @location(0) q: vec2<f32>,
    @location(1) @interpolate(flat) flux_kind: vec4<f32>,
    // true radius (px), sigma (px), phase normalisation, unused
    @location(2) @interpolate(flat) size: vec4<f32>,
    @location(3) @interpolate(flat) dir: vec3<f32>,
    @location(4) @interpolate(flat) light: vec3<f32>,
};

fn corner(vi: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    return corners[vi];
}

// A screen-aligned quad of half-size `half` pixels around the direction `dir`.
fn quad(dir: vec3<f32>, half: f32, vi: u32) -> SpriteOut {
    var out: SpriteOut;
    let c = pass_u.proj * vec4<f32>(dir, 1.0);
    let k = corner(vi);
    if c.w <= 0.0 {
        // Behind the camera: a degenerate quad outside the clip volume.
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    } else {
        let ndc = c.xy / c.w;
        out.clip = vec4<f32>(ndc + k * half * 2.0 / frame.screen.xy, 0.0, 1.0);
    }
    out.q = k * half;
    out.dir = dir;
    return out;
}

fn gaussian(r2: f32, sigma: f32) -> f32 {
    return exp(-r2 / (2.0 * sigma * sigma)) / (2.0 * PI * sigma * sigma);
}

// The weight of a sub-pixel source at squared offset `r2` (pixels): the flux-conserving
// Gaussian PSF — or, in the W2 positive control (frame.pts_y.w = 1), a hard 0.75 px disc of the
// same total flux, which covers 1, 2 or 4 pixel centres depending on where the source falls
// and so flickers as it moves.
fn point_weight(r2: f32, sigma: f32) -> f32 {
    if frame.pts_y.w > 0.5 {
        if r2 > 0.5625 {
            return 0.0;
        }
        return 1.0 / (PI * 0.5625);
    }
    return gaussian(r2, sigma);
}

@vertex
fn vs_sprite(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> SpriteOut {
    let s = sprites[ii];
    let radius = s.dir_px.w;
    let kind = s.flux.w;
    var half: f32;
    if kind < 0.5 {
        half = max(radius, 0.75);
    } else if radius < 1.0 {
        half = PSF_REACH * PSF_SIGMA;
    } else {
        half = radius + 1.0;
    }
    var out = quad(s.dir_px.xyz, half, vi);
    out.flux_kind = s.flux;
    out.size = vec4<f32>(radius, PSF_SIGMA, s.light.w, 0.0);
    out.light = s.light.xyz;
    return out;
}

@fragment
fn fs_sprite(in: SpriteOut) -> @location(0) vec4<f32> {
    let kind = in.flux_kind.w;
    let radius = in.size.x;
    let r2 = dot(in.q, in.q);
    if kind < 0.5 {
        // Legacy Sky-shell instance: a flat disc of at least 0.75 px, radiance given.
        if r2 > max(radius, 0.75) * max(radius, 0.75) {
            discard;
        }
        return vec4<f32>(in.flux_kind.rgb, 1.0);
    }
    let omega_px = frame.sky.x;
    var weight: f32;
    if radius < 1.0 {
        weight = point_weight(r2, in.size.y);
    } else {
        let r = sqrt(r2);
        let cover = clamp(radius - r + 0.5, 0.0, 1.0);
        if cover <= 0.0 {
            discard;
        }
        weight = cover / (PI * radius * radius);
        if kind > 1.5 {
            // A Lambertian sphere lit from its light, normalised by the sprite's factor.
            let z_s = -in.dir;
            let x_s = normalize(vec3<f32>(1.0, 0.0, 0.0) - z_s * z_s.x);
            let y_s = cross(z_s, x_s);
            let u = min(r / radius, 1.0);
            let nq = in.q / radius;
            let n = x_s * nq.x + y_s * nq.y + z_s * sqrt(max(1.0 - u * u, 0.0));
            weight *= max(dot(n, in.light), 0.0) * in.size.z;
        }
    }
    let l = in.flux_kind.rgb / omega_px * weight * view_transmittance(in.dir);
    return vec4<f32>(l, 1.0);
}

@vertex
fn vs_point(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> SpriteOut {
    let st = points[ii];
    let g = st.dir_lux.xyz;
    let dir = frame.pts_x.xyz * g.x + frame.pts_y.xyz * g.y + frame.pts_z.xyz * g.z;
    // Angular sigma to pixels: one pixel subtends sqrt(omega_px) radians at the centre.
    let sigma = max(PSF_SIGMA, st.color_sigma.w / sqrt(frame.sky.x));
    var out = quad(dir, PSF_REACH * sigma, vi);
    out.flux_kind = vec4<f32>(st.color_sigma.rgb * (st.dir_lux.w * frame.pts_x.w * exposure()), 1.0);
    out.size = vec4<f32>(0.0, sigma, 1.0, 0.0);
    out.light = vec3<f32>(0.0);
    return out;
}

@fragment
fn fs_point(in: SpriteOut) -> @location(0) vec4<f32> {
    let w = point_weight(dot(in.q, in.q), in.size.y);
    return vec4<f32>(in.flux_kind.rgb / frame.sky.x * w * view_transmittance(in.dir), 1.0);
}

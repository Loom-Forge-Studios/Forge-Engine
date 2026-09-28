// forge-render — HDR -> display: ACES filmic fit (Narkowicz 2015), then sRGB. The sRGB
// transfer is done by the target format when it is an *Srgb format, else here.

@group(0) @binding(0) var hdr: texture_2d<f32>;
// x = 1: encode sRGB in the shader (non-sRGB target); y = 1: write exposed linear radiance
// unchanged (radiometric tests, float target)
@group(0) @binding(1) var<uniform> tm: vec4<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_tonemap(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if tm.y > 0.5 {
        return vec4<f32>(textureLoad(hdr, vec2<i32>(pos.xy), 0).rgb, 1.0);
    }
    let c = aces(max(textureLoad(hdr, vec2<i32>(pos.xy), 0).rgb, vec3<f32>(0.0)));
    if tm.x > 0.5 {
        return vec4<f32>(srgb(c), 1.0);
    }
    return vec4<f32>(c, 1.0);
}

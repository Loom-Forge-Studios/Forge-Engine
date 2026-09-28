// ---- composite and upscale ----------------------------------------------------------------

@group(1) @binding(0) var galbedo: texture_2d<f32>;
@group(1) @binding(1) var glight: texture_2d<f32>;
@group(1) @binding(2) var scene: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vi << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vi & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(pos.xy);
    let a = textureLoad(galbedo, px, 0);
    let l = textureLoad(glight, px, 0).rgb;
    let lit = a.rgb * (frame.ambient.rgb + l);
    return vec4<f32>(mix(frame.clear.rgb, lit, a.a), 1.0);
}

@fragment
fn fs_upscale(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let s = frame.viewport.z;
    let q = (pos.xy - frame.viewport.xy) / s;
    if (q.x < 0.0 || q.y < 0.0 || q.x >= frame.size.x || q.y >= frame.size.y) {
        return vec4<f32>(frame.clear.rgb, 1.0);
    }
    return vec4<f32>(textureLoad(scene, vec2<i32>(floor(q)), 0).rgb, 1.0);
}

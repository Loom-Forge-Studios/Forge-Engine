// ---- lights -------------------------------------------------------------------------------

@group(1) @binding(0) var gnormal: texture_2d<f32>;

struct LightIn {
    @location(0) pos: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) color_radius: vec4<f32>,
    @location(3) spot: vec4<f32>,
    @location(4) extra: vec4<f32>,
};

struct LOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) center: vec2<f32>,
    @location(1) color_radius: vec4<f32>,
    @location(2) spot: vec4<f32>,
    @location(3) extra: vec4<f32>,
};

@vertex
fn vs_light(i: LightIn) -> LOut {
    var o: LOut;
    o.pos = to_clip(i.pos);
    o.center = i.center;
    o.color_radius = i.color_radius;
    o.spot = i.spot;
    o.extra = i.extra;
    return o;
}

fn shade(p: vec2<f32>, i: LOut, n: vec3<f32>) -> vec3<f32> {
    // Pixel space is y-down; the normal buffer is world (y-up).
    let d = i.center - p;
    let dw = vec2<f32>(d.x, -d.y);
    let dist = length(dw);
    let r = i.color_radius.w;
    let f = clamp(1.0 - dist / r, 0.0, 1.0);
    let atten = f * f;
    let l = normalize(vec3<f32>(dw, i.extra.x));
    let ndl = max(dot(n, l), 0.0);
    var cone = 1.0;
    if (i.extra.y > 0.5) {
        let to_p = normalize(-dw + vec2<f32>(1e-20, 0.0));
        let c = dot(to_p, i.spot.xy);
        cone = smoothstep(i.spot.w, i.spot.z, c);
    }
    return i.color_radius.xyz * atten * ndl * cone;
}

@fragment
fn fs_light(i: LOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(i.pos.xy);
    let g = textureLoad(gnormal, px, 0);
    var n = vec3<f32>(0.0, 0.0, 1.0);
    if (g.a > 0.0) {
        n = normalize(g.xyz * 2.0 - 1.0);
    }
    var acc = shade(i.pos.xy, i, n);
    // W2 perf control: evaluate the light again `repeats` times at jittered points.
    let reps = frame.knobs.x;
    if (reps > 0u) {
        var extra = vec3<f32>(0.0);
        for (var k = 0u; k < reps; k = k + 1u) {
            let j = vec2<f32>(f32(k) * 1e-4, f32(k) * -1e-4);
            extra = extra + shade(i.pos.xy + j, i, n);
        }
        acc = mix(acc, extra / f32(reps), 1e-6);
    }
    return vec4<f32>(acc, 1.0);
}


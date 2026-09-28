// forge-2d's render path (Ch.35): sprites and meshes into an albedo + normal G-buffer at
// the reference size, 2D lights (shadowed fans) into a light buffer, a composite, and a
// whole-number upscale into the output. Every position arrives in render-target pixels,
// narrowed from f64 in last_mile.rs.

struct Frame {
    size: vec2<f32>,
    out_size: vec2<f32>,
    ambient: vec4<f32>,
    clear: vec4<f32>,
    // x, y (output pixels), scale, unused
    viewport: vec4<f32>,
    // light repeats (a W2 perf control), unused x3
    knobs: vec4<u32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

fn to_clip(p: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(p.x / frame.size.x * 2.0 - 1.0, 1.0 - p.y / frame.size.y * 2.0, 0.0, 1.0);
}


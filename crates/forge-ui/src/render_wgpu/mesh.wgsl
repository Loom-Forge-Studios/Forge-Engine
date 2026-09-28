// forge-ui mesh pipeline (Ch.21 §21.13): lyon-tessellated curves, wires, fills and
// colour fields. Colours arrive linear and premultiplied. Strokes carry `aa`: x is the
// signed position across the stroke (±1 at the edges), y is the half-extent in physical
// px including a 1 px ramp; coverage falls off over that last pixel (AA without MSAA).

struct Globals {
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;

struct V {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) aa: vec2<f32>,
};

struct O {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) aa: vec2<f32>,
};

@vertex
fn vs_mesh(v: V) -> O {
    var o: O;
    let ndc = v.pos / g.size * 2.0 - vec2<f32>(1.0, 1.0);
    o.pos = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    o.color = v.color;
    o.aa = v.aa;
    return o;
}

@fragment
fn fs_mesh(o: O) -> @location(0) vec4<f32> {
    if (o.aa.y <= 0.0) {
        return o.color;
    }
    let d = abs(o.aa.x) * o.aa.y;
    let cov = clamp(o.aa.y - d, 0.0, 1.0);
    return o.color * cov;
}

// forge-ui uber-pipeline (Ch.21 §21.13): rounded/bordered quads, analytic-blur shadows,
// mask and colour glyphs, and images, all instanced through one pipeline. Colours arrive
// linear and premultiplied; the UI target is Rgba8UnormSrgb, so blending is gamma-correct.

struct Globals {
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var mask_tex: texture_2d<f32>;
@group(0) @binding(2) var color_tex: texture_2d<f32>;
@group(0) @binding(3) var image_tex: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;

struct Inst {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) border_color: vec4<f32>,
    @location(4) radii: vec4<f32>,
    @location(5) params: vec4<f32>,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) uv: vec4<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) border_color: vec4<f32>,
    @location(5) @interpolate(flat) radii: vec4<f32>,
    @location(6) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, i: Inst) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[vi];
    let px = i.rect.xy + c * i.rect.zw;
    var o: VOut;
    let ndc = px / g.size * 2.0 - vec2<f32>(1.0, 1.0);
    o.pos = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    o.p = px;
    o.rect = i.rect;
    o.uv = i.uv;
    o.color = i.color;
    o.border_color = i.border_color;
    o.radii = i.radii;
    o.params = i.params;
    return o;
}

// Signed distance to a rounded box centred at the origin with half-size `h`, radius `r`.
fn sd_round_box(p: vec2<f32>, h: vec2<f32>, r: f32) -> f32 {
    let rr = min(r, min(h.x, h.y));
    let q = abs(p) - h + vec2<f32>(rr, rr);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - rr;
}

fn corner_radius(p: vec2<f32>, c: vec2<f32>, radii: vec4<f32>) -> f32 {
    // radii = (tl, tr, br, bl)
    if (p.x < c.x) {
        if (p.y < c.y) { return radii.x; }
        return radii.w;
    }
    if (p.y < c.y) { return radii.y; }
    return radii.z;
}

@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
    let kind = v.params.z;
    if (kind < 0.5) {
        // Quad: fill + border, anti-aliased by the SDF.
        let c = v.rect.xy + v.rect.zw * 0.5;
        let h = v.rect.zw * 0.5;
        let r = corner_radius(v.p, c, v.radii);
        let d = sd_round_box(v.p - c, h, r);
        let outer = clamp(0.5 - d, 0.0, 1.0);
        let bw = v.params.x;
        var inner = outer;
        if (bw > 0.0) {
            inner = clamp(0.5 - (d + bw), 0.0, 1.0);
        }
        return v.color * inner + v.border_color * (outer - inner);
    } else if (kind < 1.5) {
        // Shadow: the shape rect is in `uv`; analytic blur of its SDF.
        let c = v.uv.xy + v.uv.zw * 0.5;
        let h = v.uv.zw * 0.5;
        let r = corner_radius(v.p, c, v.radii);
        let d = sd_round_box(v.p - c, h, r);
        let blur = max(v.params.y, 0.5);
        let a = 1.0 - smoothstep(-blur, blur, d);
        return v.color * a;
    } else if (kind < 2.5) {
        // Mask glyph: exact texel fetch (glyphs sit on the pixel grid).
        let t = vec2<i32>(floor(v.uv.xy + (v.p - v.rect.xy)));
        let cov = textureLoad(mask_tex, t, 0).r;
        return v.color * cov;
    } else if (kind < 3.5) {
        // Colour glyph (emoji): straight alpha from an sRGB texture, premultiply.
        let t = vec2<i32>(floor(v.uv.xy + (v.p - v.rect.xy)));
        let s = textureLoad(color_tex, t, 0);
        return vec4<f32>(s.rgb * s.a, s.a);
    }
    // Image: filtered sample, premultiplied, tinted.
    let dims = vec2<f32>(textureDimensions(image_tex));
    let local = (v.p - v.rect.xy) / max(v.rect.zw, vec2<f32>(1.0, 1.0));
    let uvn = (v.uv.xy + local * v.uv.zw) / dims;
    let s = textureSampleLevel(image_tex, samp, uvn, 0.0);
    return vec4<f32>(s.rgb * s.a, s.a) * v.color;
}


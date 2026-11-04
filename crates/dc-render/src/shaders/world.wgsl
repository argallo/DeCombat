// World section passes of the DeCombat renderer, one fragment entry point per pass family.
// Written for DeCombat from the pass recipes of rust/specs/RENDER_2009.md section 5.2 (texture-stage combiners
// re-expressed as arithmetic); no shader text of any other origin. The 2009 world path is fixed function.
//
// Notation as in the spec: T0/T1 = stage textures, D = vertex colour, F_lm = lightmap texture factor,
// "x2" = MODULATE2X when the `saturate` setting is on. Stage results are clamped to [0, 1] after every
// combiner, like the fixed-function pipeline does.

struct Frame {
    clip_from_world: mat4x4<f32>,
    // x: vertex-lit modulate scale (2 with `saturate`, else 1)
    // y: lightmap-pass modulate scale (2 with `full_bright`, else 1)
    // z: 1 when the flat bucket doubles the vertex colour (`saturate`), else 0
    // w: unused
    params: vec4<f32>,
    // F_lm as RGBA in [0, 1]
    lm_factor: vec4<f32>,
    // linear fog: x far, y 1 / (far - near), z 1 when on; RENDER_2009.md section 3.3 (table fog, world units)
    fog: vec4<f32>,
    // fog colour in [0, 1]
    fog_colour: vec4<f32>,
};

struct Material {
    // x: alpha-test reference in 0..255 (alpha GREATER ref passes), y: 1 = alpha test on
    // z: LOD bias of stage 0, w: LOD bias of stage 1
    alpha: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<uniform> mat: Material;
@group(1) @binding(1) var tex0: texture_2d<f32>;
@group(1) @binding(2) var tex1: texture_2d<f32>;
@group(1) @binding(3) var samp0: sampler;
@group(1) @binding(4) var samp1: sampler;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) colour: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
    @location(1) uv0: vec2<f32>,
    @location(2) uv1: vec2<f32>,
    // view-space depth (clip w of the perspective projection), the fog distance
    @location(3) depth: f32,
};

@vertex
fn vs_world(v: VertexIn) -> VertexOut {
    var o: VertexOut;
    o.clip = frame.clip_from_world * vec4<f32>(v.pos, 1.0);
    o.colour = v.colour;
    o.uv0 = v.uv0;
    o.uv1 = v.uv1;
    o.depth = o.clip.w;
    return o;
}

// Fixed-function linear fog, applied to the colour after the texture stages (public Direct3D 9 definition:
// f = (end - d) / (end - start), colour = f * c + (1 - f) * fog colour); each pass gives its own fog colour.
fn fogged(c: vec3<f32>, depth: f32, fog_colour: vec3<f32>) -> vec3<f32> {
    if (frame.fog.z < 0.5) {
        return c;
    }
    let f = clamp((frame.fog.x - depth) * frame.fog.y, 0.0, 1.0);
    return mix(fog_colour, c, f);
}

// The multiply passes' fog colour: grey 0x808080 with `saturate`, else white, the neutral value of the multiply
// (RENDER_2009.md section 5.2, buckets 15..22), so the lightmap pass's fog is not applied twice.
fn neutral_fog() -> vec3<f32> {
    if (frame.params.z > 0.5) {
        return vec3<f32>(128.0 / 255.0);
    }
    return vec3<f32>(1.0);
}

// ALPHAFUNC GREATER against the texture's reference (world pass state, RENDER_2009 5.1 step 4 and 10.3).
fn alpha_test(a: f32) {
    if (mat.alpha.y > 0.5 && round(a * 255.0) <= mat.alpha.x) {
        discard;
    }
}

fn sample0(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleBias(tex0, samp0, uv, mat.alpha.z);
}

fn sample1(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleBias(tex1, samp1, uv, mat.alpha.w);
}

// Bucket 0: vertex colour only, doubled (ADD of D with itself) with `saturate`.
fn flat_colour(i: VertexOut) -> vec4<f32> {
    let c = i.colour.rgb * (1.0 + frame.params.z);
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), i.colour.a);
}

@fragment
fn fs_flat(i: VertexOut) -> @location(0) vec4<f32> {
    let c = flat_colour(i);
    return vec4<f32>(fogged(c.rgb, i.depth, frame.fog_colour.rgb), c.a);
}

// Bucket 1: T0 * D (x2); alpha T0.a * D.a.
fn vertex_lit(i: VertexOut) -> vec4<f32> {
    let t = sample0(i.uv0);
    let a = t.a * i.colour.a;
    alpha_test(a);
    let c = clamp(t.rgb * i.colour.rgb * frame.params.x, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(c, a);
}

@fragment
fn fs_vertex_lit(i: VertexOut) -> @location(0) vec4<f32> {
    let c = vertex_lit(i);
    return vec4<f32>(fogged(c.rgb, i.depth, frame.fog_colour.rgb), c.a);
}

// Bucket 10: lerp(T1, T0, D.a * T0.a), then * D (x2); alpha D.a * T0.a.
fn blend_vertex_lit(i: VertexOut) -> vec4<f32> {
    let t0 = sample0(i.uv0);
    let t1 = sample1(i.uv1);
    let a = i.colour.a * t0.a;
    alpha_test(a);
    let blended = mix(t1.rgb, t0.rgb, a);
    let c = clamp(blended * i.colour.rgb * frame.params.x, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(c, a);
}

@fragment
fn fs_blend_vertex_lit(i: VertexOut) -> @location(0) vec4<f32> {
    let c = blend_vertex_lit(i);
    return vec4<f32>(fogged(c.rgb, i.depth, frame.fog_colour.rgb), c.a);
}

// Bucket 14: the section lightmap times F_lm (MODULATE2X with `full_bright`), opaque, writes depth.
// The lightmap is sampled with CLAMP addressing. Coordinates: OBSERVED over all 21 ROM worlds, the kind-2
// vertices carry the lightmap coordinates in the FIRST set (inside [0.5/256, 1 - 0.5/256], i.e. texel centres)
// and their second set is zero; the second set is used only by kinds 8 and 9 for T1.
@fragment
fn fs_lightmap(i: VertexOut) -> @location(0) vec4<f32> {
    let l = textureSample(tex0, samp0, i.uv0);
    let c = clamp(l.rgb * frame.lm_factor.rgb * frame.params.y, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(fogged(c, i.depth, frame.fog_colour.rgb), 1.0);
}

// Bucket 15: T0 * F_lm; the blend state multiplies it into the frame (dst * src + src * dst = 2 * dst * src
// with `saturate`, else dst * src).
@fragment
fn fs_lightmapped_texture(i: VertexOut) -> @location(0) vec4<f32> {
    let t = sample0(i.uv0);
    alpha_test(t.a);
    let c = clamp(t.rgb * frame.lm_factor.rgb * frame.params.y, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(fogged(c, i.depth, neutral_fog()), t.a);
}

// Bucket 20: bucket 10 without the vertex colour stage, multiplied into the frame like bucket 15.
@fragment
fn fs_blend_lightmapped(i: VertexOut) -> @location(0) vec4<f32> {
    let t0 = sample0(i.uv0);
    let t1 = sample1(i.uv1);
    let a = i.colour.a * t0.a;
    alpha_test(a);
    return vec4<f32>(fogged(mix(t1.rgb, t0.rgb, a), i.depth, neutral_fog()), a);
}

// ---- Translucent and additive world models (RENDER_2009 5.1 step 6, 7.1 steps 7-9) ----
// The object queue draws them with blending on and Z write off, back to front; the section recipe stays the
// base recipe of its family. DeCombat: the object's alpha property scales the fragment alpha (group 2); kinds
// 4 and 9 in a blended model are drawn with their vertex-lit counterpart (the lightmap multiply cannot blend),
// kind 2 is left out (OBSERVED: 1 kind-2 and 1 kind-4 section among the 467 sections of the 339 blended world
// models of the ROM).

struct Object {
    // x: alpha scale, y: 1 when additive (fog colour black), zw unused
    alpha: vec4<f32>,
};

@group(2) @binding(0) var<uniform> obj: Object;

// RENDER_2009.md section 5.1 step 6 (additive world models: FOGCOLOR black)
fn object_fog(i: VertexOut, c: vec4<f32>) -> vec4<f32> {
    var fc = frame.fog_colour.rgb;
    if (obj.alpha.y > 0.5) {
        fc = vec3<f32>(0.0);
    }
    return vec4<f32>(fogged(c.rgb, i.depth, fc), c.a * obj.alpha.x);
}

@fragment
fn fs_flat_blend(i: VertexOut) -> @location(0) vec4<f32> {
    return object_fog(i, flat_colour(i));
}

@fragment
fn fs_vertex_lit_blend(i: VertexOut) -> @location(0) vec4<f32> {
    return object_fog(i, vertex_lit(i));
}

@fragment
fn fs_blend_vertex_lit_blend(i: VertexOut) -> @location(0) vec4<f32> {
    return object_fog(i, blend_vertex_lit(i));
}

// Model pass of the DeCombat renderer: skinning with a bone palette, per-vertex lighting with the render style's
// material (ambient plus up to eight lights), then the render style's texture stages. Written for DeCombat from
// rust/specs/RENDER_2009.md sections 6.3 (ambient from the light grid plus point lights, at most 8 lights), 7.2
// (LIGHTING per piece), 7.3 (render-style pass state) and 7.4 (four weights per vertex, bone palette), and from
// the draft FMT_RENDERSTYLE_2009.md (stage records, material, pass state of the 2009 render-style files).
// The light equation and the stage operations are the public Direct3D 9 fixed-function definitions (vertex
// lighting: emissive + ambient x material ambient + sum of attenuation x spot x max(N.L, 0) x light x material
// diffuse, specular with the half vector of a local viewer; D3DTEXTUREOP / D3DTA semantics with saturation after
// each stage), evaluated with the falloff forms of the engine's light records (engine+0x1ba460). Op and argument
// codes are the public D3DTEXTUREOP / D3DTA values. No shader text of any other origin.

struct Light {
    // xyz position (world), w radius
    pos_radius: vec4<f32>,
    // rgb colour in [0, 1] (light colour / 255), w unused
    colour: vec4<f32>,
    // xyz direction the light travels (spot / directional), w cosine of the half cone angle
    dir_cos: vec4<f32>,
    // x shape (0 point, 1 spot, 2 directional), y falloff (0 d3d, 1 linear, 2 quartic), zw unused
    kind: vec4<f32>,
    // d3d coefficients c0, c1, c2, unused
    coefs: vec4<f32>,
};

struct Instance {
    clip_from_model: mat4x4<f32>,
    world_from_model: mat4x4<f32>,
    // camera space from world (view matrix)
    view: mat4x4<f32>,
    // rgb ambient in [0, 1]; w = number of lights
    ambient: vec4<f32>,
    // xyz eye position (world), w the effect clock in seconds
    eye: vec4<f32>,
    lights: array<Light, 8>,
    // linear fog of the world pass: x far, y 1 / (far - near), z 1 when on (RENDER_2009.md section 3.3)
    fog: vec4<f32>,
    fog_colour: vec4<f32>,
};

struct Stage {
    // x colour op, y colour arg 1, z colour arg 2, w alpha op
    ops: vec4<u32>,
    // x alpha arg 1, y alpha arg 2, z uv source (0..3 set, 4 normal, 5 position, 6 reflection, +8 world),
    // w flags (bit 0 transform, bit 1 projected, bits 4..7 coordinate count)
    ops2: vec4<u32>,
    // texture transform, applied to the row vector of the coordinates (stored row-major = these columns)
    xform: mat4x4<f32>,
    // x mip bias, yzw unused
    extra: vec4<f32>,
};

struct Pass {
    stages: array<Stage, 4>,
    // TEXTUREFACTOR rgba in [0, 1]
    factor: vec4<f32>,
    // x alpha reference 0..255, y alpha compare code (0 off), z active stage count, w lighting (1 on)
    alpha: vec4<f32>,
    mat_ambient: vec4<f32>,
    mat_diffuse: vec4<f32>,
    mat_emissive: vec4<f32>,
    // rgb specular colour, w power (0: specular off)
    mat_specular: vec4<f32>,
    // a camo effect (DC_DEFAULTS.md VAR-4): x on, y pattern repeats across the texture, zw drift in repeats per second
    fx_a: vec4<f32>,
    // x lowest gain, y highest gain, z seconds per pulse (0: steady at the highest gain), w unused
    fx_b: vec4<f32>,
    // a camo flow (VAR-12): x on, y swirl in repeats, z seconds per swirl, w the stage whose texture it colours. The
    // effect's tile, drift and repeats are then the paint's, and it adds no light.
    flow: vec4<f32>,
};

@group(0) @binding(0) var<uniform> inst: Instance;
@group(0) @binding(1) var<uniform> bones: array<mat4x4<f32>, 256>;
@group(1) @binding(0) var<uniform> pass_u: Pass;
@group(1) @binding(1) var tex0: texture_2d<f32>;
@group(1) @binding(2) var tex1: texture_2d<f32>;
@group(1) @binding(3) var tex2: texture_2d<f32>;
@group(1) @binding(4) var tex3: texture_2d<f32>;
@group(1) @binding(5) var samp0: sampler;
@group(1) @binding(6) var samp1: sampler;
@group(1) @binding(7) var samp2: sampler;
@group(1) @binding(8) var samp3: sampler;
@group(1) @binding(9) var fx_tile: texture_2d<f32>;
@group(1) @binding(10) var fx_mask: texture_2d<f32>;
@group(1) @binding(11) var fx_samp: sampler;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bones: vec4<u32>,
    @location(4) weights: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) diffuse: vec4<f32>,
    @location(1) specular: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) cam_pos: vec3<f32>,
    @location(4) cam_normal: vec3<f32>,
};

fn attenuation(l: Light, d2: f32) -> f32 {
    let r = l.pos_radius.w;
    if (!(d2 < r * r)) {
        return 0.0;
    }
    let d = sqrt(d2);
    let kind = u32(l.kind.y + 0.5);
    if (kind == 1u) {
        return max(1.0 - d / r, 0.0);
    }
    if (kind == 2u) {
        let t = 1.0 - d2 / (r * r);
        return t * t;
    }
    let den = l.coefs.x + l.coefs.y * d + l.coefs.z * d2;
    if (den <= 1e-12) {
        return 0.0;
    }
    return 1.0 / den;
}

@vertex
fn vs_model(v: VertexIn) -> VertexOut {
    // skinning: model space of the pose (bind-pose vertices, bone = pose * inverse bind)
    var skin = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    for (var k = 0u; k < 4u; k = k + 1u) {
        let w = v.weights[k];
        if (w != 0.0) {
            skin = skin + bones[min(v.bones[k], 255u)] * w;
        }
    }
    let p_model = skin * vec4<f32>(v.pos, 1.0);
    let n_model = (skin * vec4<f32>(v.normal, 0.0)).xyz;
    let p_world = (inst.world_from_model * p_model).xyz;
    var n_world = (inst.world_from_model * vec4<f32>(n_model, 0.0)).xyz;
    let nl = length(n_world);
    if (nl > 1e-6) {
        n_world = n_world / nl;
    }
    var o: VertexOut;
    o.clip = inst.clip_from_model * p_model;
    o.uv = v.uv;
    o.cam_pos = (inst.view * vec4<f32>(p_world, 1.0)).xyz;
    o.cam_normal = normalize((inst.view * vec4<f32>(n_world, 0.0)).xyz + vec3<f32>(0.0, 0.0, 1e-9));
    if (pass_u.alpha.w < 0.5) {
        // LIGHTING off and no vertex colour in the vertex format: diffuse white, no specular
        o.diffuse = vec4<f32>(1.0);
        o.specular = vec3<f32>(0.0);
        return o;
    }
    var dif = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    let power = pass_u.mat_specular.w;
    let to_eye = normalize(inst.eye.xyz - p_world);
    let count = u32(inst.ambient.w + 0.5);
    for (var i = 0u; i < 8u; i = i + 1u) {
        if (i >= count) {
            break;
        }
        let l = inst.lights[i];
        let shape = u32(l.kind.x + 0.5);
        var dir_to_light: vec3<f32>;
        var a = 1.0;
        if (shape == 2u) {
            dir_to_light = -l.dir_cos.xyz;
        } else {
            let v_l = l.pos_radius.xyz - p_world;
            let d2 = dot(v_l, v_l);
            a = attenuation(l, d2);
            if (a <= 0.0 || d2 <= 0.0) {
                continue;
            }
            dir_to_light = v_l / sqrt(d2);
            if (shape == 1u) {
                let cos_a = dot(l.dir_cos.xyz, -dir_to_light);
                let ch = l.dir_cos.w;
                if (cos_a <= ch || ch >= 1.0) {
                    continue;
                }
                a = a * (cos_a - ch) / (1.0 - ch);
            }
        }
        let ndl = dot(n_world, dir_to_light);
        dif = dif + l.colour.rgb * a * max(ndl, 0.0);
        if (power > 0.0 && ndl > 0.0) {
            let h = normalize(dir_to_light + to_eye);
            spec = spec + l.colour.rgb * a * pow(max(dot(n_world, h), 0.0), power);
        }
    }
    let c = pass_u.mat_emissive.rgb + pass_u.mat_ambient.rgb * inst.ambient.rgb
        + pass_u.mat_diffuse.rgb * dif;
    o.diffuse = vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(pass_u.mat_diffuse.a, 0.0, 1.0));
    o.specular = clamp(pass_u.mat_specular.rgb * spec, vec3<f32>(0.0), vec3<f32>(1.0));
    return o;
}

fn coords(s: Stage, i: VertexOut) -> vec2<f32> {
    let src = s.ops2.z & 7u;
    var c = vec4<f32>(i.uv, 1.0, 0.0);
    if (src >= 4u) {
        var g = i.cam_normal;
        if (src == 5u) {
            g = i.cam_pos;
        } else if (src == 6u) {
            // reflection of the vertex-to-eye vector about the normal (local viewer)
            let e = normalize(-i.cam_pos);
            g = 2.0 * dot(e, i.cam_normal) * i.cam_normal - e;
        }
        c = vec4<f32>(g, 1.0);
    }
    if ((s.ops2.w & 1u) != 0u) {
        let t = s.xform * c;
        let n = (s.ops2.w >> 4u) & 15u;
        if ((s.ops2.w & 2u) != 0u && n >= 2u) {
            let q = t[min(n, 4u) - 1u];
            if (abs(q) > 1e-12) {
                return t.xy / q;
            }
        }
        return t.xy;
    }
    return c.xy;
}

fn arg(code: u32, cur: vec4<f32>, dif: vec4<f32>, tex: vec4<f32>) -> vec4<f32> {
    if (code == 0u) {
        return dif;
    }
    if (code == 2u) {
        return tex;
    }
    if (code == 3u) {
        return pass_u.factor;
    }
    return cur;
}

// One D3DTEXTUREOP on rgb (alpha uses the same function on .aaaa); `cur_a`, `tex_a`, `dif_a` for the blend ops.
fn op(code: u32, a1: vec4<f32>, a2: vec4<f32>, cur: vec4<f32>, dif: vec4<f32>, tex: vec4<f32>) -> vec4<f32> {
    switch code {
        case 2u: { return a1; }
        case 3u: { return a2; }
        case 4u: { return a1 * a2; }
        case 5u: { return a1 * a2 * 2.0; }
        case 6u: { return a1 * a2 * 4.0; }
        case 7u: { return a1 + a2; }
        case 8u: { return a1 + a2 - 0.5; }
        case 9u: { return (a1 + a2 - 0.5) * 2.0; }
        case 10u: { return a1 - a2; }
        case 11u: { return a1 + a2 - a1 * a2; }
        case 12u: { return mix(a2, a1, dif.a); }
        case 13u: { return mix(a2, a1, tex.a); }
        case 14u: { return mix(a2, a1, pass_u.factor.a); }
        case 16u: { return mix(a2, a1, cur.a); }
        case 18u: { return vec4<f32>(a1.rgb + a1.a * a2.rgb, a1.a); }
        case 20u: { return vec4<f32>(a1.rgb + (1.0 - a1.a) * a2.rgb, a1.a); }
        case 24u: {
            let d = 4.0 * dot(a1.rgb - 0.5, a2.rgb - 0.5);
            return vec4<f32>(d);
        }
        // bump ops (22, 23): DC-DEFAULT REN-3 of the staged DeCombat defaults (proposed): the current value
        // passes through, no coordinate perturbation
        default: { return cur; }
    }
}

fn sample_stage(k: u32, uv: vec2<f32>, bias: f32) -> vec4<f32> {
    if (k == 0u) {
        return textureSampleBias(tex0, samp0, uv, bias);
    }
    if (k == 1u) {
        return textureSampleBias(tex1, samp1, uv, bias);
    }
    if (k == 2u) {
        return textureSampleBias(tex2, samp2, uv, bias);
    }
    return textureSampleBias(tex3, samp3, uv, bias);
}

fn compare_pass(code: u32, a: f32, r: f32) -> bool {
    switch code {
        case 1u: { return false; }
        case 2u: { return a < r; }
        case 3u: { return a == r; }
        case 4u: { return a <= r; }
        case 5u: { return a > r; }
        case 6u: { return a != r; }
        case 7u: { return a >= r; }
        default: { return true; }
    }
}

// A camo effect's light: the pattern, drifting and pulsing, where its mask lets it show. Our own effect, not the 2009 renderer's.
fn fx_light(uv: vec2<f32>) -> vec3<f32> {
    if (pass_u.fx_a.x < 0.5 || pass_u.flow.x > 0.5) {
        return vec3<f32>(0.0);
    }
    let t = inst.eye.w;
    var gain = pass_u.fx_b.y;
    if (pass_u.fx_b.z > 0.0) {
        gain = mix(pass_u.fx_b.x, pass_u.fx_b.y, 0.5 + 0.5 * sin(6.2831853 * t / pass_u.fx_b.z));
    }
    let moved = uv * pass_u.fx_a.y + pass_u.fx_a.zw * t;
    return textureSample(fx_tile, fx_samp, moved).rgb * textureSample(fx_mask, fx_samp, uv).r * gain;
}

// A camo flow's colour (VAR-12): the pattern drifting and pushed about by two slow waves on each axis, so its blobs travel,
// stretch and fold like a lava lamp. The texture holds only the gun's shading, painted grey (camo::FLOW_GREY = 170), so the
// colour is the pattern times 255/170; where the mask keeps the gun's own look (a lens) it is 1. Whole-number wave counts
// keep the swirl seamless where the pattern repeats. Our own effect, not the 2009 renderer's.
fn flow_colour(uv: vec2<f32>) -> vec3<f32> {
    if (pass_u.flow.x < 0.5) {
        return vec3<f32>(1.0);
    }
    let tau = 6.2831853;
    let t = inst.eye.w;
    let p = uv * pass_u.fx_a.y + pass_u.fx_a.zw * t;
    let a = tau * t / max(pass_u.flow.z, 1.0);
    let wave = vec2<f32>(
        sin(tau * p.y + a) + 0.6 * sin(tau * 2.0 * (p.x + p.y) - 1.7 * a),
        cos(tau * p.x - 0.8 * a) + 0.6 * cos(tau * 2.0 * (p.x - p.y) + 1.3 * a),
    );
    let colour = textureSample(fx_tile, fx_samp, p + wave * (pass_u.flow.y / 1.6)).rgb * (255.0 / 170.0);
    return mix(vec3<f32>(1.0), colour, textureSample(fx_mask, fx_samp, uv).r);
}

@fragment
fn fs_model(i: VertexOut) -> @location(0) vec4<f32> {
    let fx = fx_light(i.uv);
    let flow = flow_colour(i.uv);
    let dif = i.diffuse;
    var cur = dif;
    let n = u32(pass_u.alpha.z + 0.5);
    // every texture is sampled in uniform control flow; the stage loop then picks what it needs
    var texs: array<vec4<f32>, 4>;
    texs[0] = sample_stage(0u, coords(pass_u.stages[0], i), pass_u.stages[0].extra.x);
    texs[1] = sample_stage(1u, coords(pass_u.stages[1], i), pass_u.stages[1].extra.x);
    texs[2] = sample_stage(2u, coords(pass_u.stages[2], i), pass_u.stages[2].extra.x);
    texs[3] = sample_stage(3u, coords(pass_u.stages[3], i), pass_u.stages[3].extra.x);
    if (pass_u.flow.x > 0.5) {
        let k = min(u32(pass_u.flow.w + 0.5), 3u);
        texs[k] = vec4<f32>(texs[k].rgb * flow, texs[k].a);
    }
    for (var k = 0u; k < 4u; k = k + 1u) {
        if (k >= n) {
            break;
        }
        let s = pass_u.stages[k];
        let t = texs[k];
        let c1 = arg(s.ops.y, cur, dif, t);
        let c2 = arg(s.ops.z, cur, dif, t);
        let rgb = op(s.ops.x, c1, c2, cur, dif, t).rgb;
        var a = cur.a;
        if (s.ops.w != 1u) {
            let a1 = arg(s.ops2.x, cur, dif, t);
            let a2 = arg(s.ops2.y, cur, dif, t);
            a = op(s.ops.w, a1.aaaa, a2.aaaa, cur.aaaa, dif, t).a;
        }
        cur = clamp(vec4<f32>(rgb, a), vec4<f32>(0.0), vec4<f32>(1.0));
    }
    let code = u32(pass_u.alpha.y + 0.5);
    if (code != 0u && !compare_pass(code, round(cur.a * 255.0), pass_u.alpha.x)) {
        discard;
    }
    // SPECULARENABLE: the specular colour is added after the stages
    var rgb = clamp(cur.rgb + i.specular + fx, vec3<f32>(0.0), vec3<f32>(1.0));
    // then the fixed-function linear fog on the view-space depth (public Direct3D 9 definition)
    if (inst.fog.z > 0.5) {
        let f = clamp((inst.fog.x - i.cam_pos.z) * inst.fog.y, 0.0, 1.0);
        rgb = mix(inst.fog_colour.rgb, rgb, f);
    }
    return vec4<f32>(rgb, cur.a);
}

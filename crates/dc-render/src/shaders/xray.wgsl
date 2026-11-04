// X-ray styles of DeCombat's render mods (DC_DEFAULTS CLI-1624): flat two-tone "chams" of a skinned model and
// screen-space quads (skeleton lines, joints, boxes). Every fragment sits at the near end of the world depth range
// (z = XRAY_DEPTH in a full [0, 1] viewport) and is tested with LESS against the frame's depth without writing it:
// it shows through every world surface and model, but never over the first-person models (their depth range ends at
// 0.1, RENDER_2009.md section 4.3). Written for DeCombat; no shader text of any other origin.

const XRAY_DEPTH: f32 = 0.1;

struct Light {
    pos_radius: vec4<f32>,
    colour: vec4<f32>,
    dir_cos: vec4<f32>,
    kind: vec4<f32>,
    coefs: vec4<f32>,
};

// the model pass's instance block (shaders/model.wgsl), read for the matrices and the eye only
struct Instance {
    clip_from_model: mat4x4<f32>,
    world_from_model: mat4x4<f32>,
    view: mat4x4<f32>,
    ambient: vec4<f32>,
    eye: vec4<f32>,
    lights: array<Light, 8>,
    fog: vec4<f32>,
    fog_colour: vec4<f32>,
};

struct Tones {
    primary: vec4<f32>,
    secondary: vec4<f32>,
    // x: the facing threshold (|N.V| at or above it: primary, below: secondary)
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> inst: Instance;
@group(0) @binding(1) var<uniform> bones: array<mat4x4<f32>, 256>;
@group(1) @binding(0) var<uniform> tones: Tones;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bones: vec4<u32>,
    @location(4) weights: vec4<f32>,
};

struct ChamsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) to_eye: vec3<f32>,
};

@vertex
fn vs_chams(v: VertexIn) -> ChamsOut {
    var skin = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    for (var k = 0u; k < 4u; k = k + 1u) {
        let w = v.weights[k];
        if (w != 0.0) {
            skin = skin + bones[min(v.bones[k], 255u)] * w;
        }
    }
    let p_model = skin * vec4<f32>(v.pos, 1.0);
    let n_model = (skin * vec4<f32>(v.normal, 0.0)).xyz;
    var o: ChamsOut;
    var c = inst.clip_from_model * p_model;
    c.z = XRAY_DEPTH * c.w;
    o.clip = c;
    o.normal = (inst.world_from_model * vec4<f32>(n_model, 0.0)).xyz;
    o.to_eye = inst.eye.xyz - (inst.world_from_model * p_model).xyz;
    return o;
}

@fragment
fn fs_chams(i: ChamsOut) -> @location(0) vec4<f32> {
    let n = normalize(i.normal + vec3<f32>(0.0, 0.0, 1e-9));
    let e = normalize(i.to_eye + vec3<f32>(0.0, 0.0, 1e-9));
    if (abs(dot(n, e)) >= tones.params.x) {
        return tones.primary;
    }
    return tones.secondary;
}

struct QuadIn {
    @location(0) ndc: vec2<f32>,
    @location(1) colour: vec4<f32>,
};

struct QuadOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
};

@vertex
fn vs_quad(v: QuadIn) -> QuadOut {
    var o: QuadOut;
    o.clip = vec4<f32>(v.ndc, XRAY_DEPTH, 1.0);
    o.colour = v.colour;
    return o;
}

@fragment
fn fs_quad(i: QuadOut) -> @location(0) vec4<f32> {
    return i.colour;
}

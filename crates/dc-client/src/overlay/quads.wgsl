// The overlay's batched quads (dc-client overlay/batch.rs): solid fills, glyphs and textured images, blended
// straight alpha over the target in instance order. Each fragment computes the value the CPU compositor
// (hud/compose.rs, overlay/text.rs) would blend at that pixel. Our own code.

// canvas size and where the canvas's top-left lands on the target; target size
struct View {
    canvas: vec4<f32>,  // canvas w, h, at x, at y
    dest: vec4<f32>,  // target w, h, 0, 0
};
@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var atlas: texture_2d<f32>;

struct QIn {
    @location(0) rect: vec4<f32>,
    @location(1) colour: vec4<f32>,
    @location(2) a: vec4<f32>,
    @location(3) b: vec4<f32>,
    @location(4) c: vec4<f32>,
    @location(5) d: vec4<u32>,
    @location(6) e: vec4<f32>,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) colour: vec4<f32>,
    @location(1) @interpolate(flat) a: vec4<f32>,
    @location(2) @interpolate(flat) b: vec4<f32>,
    @location(3) @interpolate(flat) c: vec4<f32>,
    @location(4) @interpolate(flat) d: vec4<u32>,
    @location(5) @interpolate(flat) e: vec4<f32>,
};

@vertex
fn vs_quad(@builtin(vertex_index) i: u32, q: QIn) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
    let k = corners[i];
    let px = mix(q.rect.x, q.rect.z, k.x) + view.canvas.z;
    let py = mix(q.rect.y, q.rect.w, k.y) + view.canvas.w;
    var o: VOut;
    o.pos = vec4<f32>(px / view.dest.x * 2.0 - 1.0, 1.0 - py / view.dest.y * 2.0, 0.0, 1.0);
    o.colour = q.colour;
    o.a = q.a;
    o.b = q.b;
    o.c = q.c;
    o.d = q.d;
    o.e = q.e;
    return o;
}

// one atlas texel as 0..255 values
fn texel(x: u32, y: u32) -> vec4<f32> {
    return round(textureLoad(atlas, vec2<u32>(x, y), 0) * 255.0);
}

// bilinear over texel centres, clamped to [0, max) of the source rectangle (as the compositor's `sample`)
fn sample_bilinear(u_in: f32, v_in: f32, max_u: f32, max_v: f32, ox: u32, oy: u32, lim: u32) -> vec4<f32> {
    let u = clamp(u_in - 0.5, 0.0, max(max_u - 1.0, 0.0));
    let v = clamp(v_in - 0.5, 0.0, max(max_v - 1.0, 0.0));
    let fx0 = floor(u);
    let fy0 = floor(v);
    let fx = u - fx0;
    let fy = v - fy0;
    let lim_x = lim & 0xffffu;
    let lim_y = lim >> 16u;
    let x0 = u32(fx0);
    let y0 = u32(fy0);
    let x1 = min(x0 + 1u, lim_x);
    let y1 = min(y0 + 1u, lim_y);
    let xa = min(x0, lim_x);
    let ya = min(y0, lim_y);
    let ta = texel(ox + xa, oy + ya);
    let tb = texel(ox + x1, oy + ya);
    let tc = texel(ox + xa, oy + y1);
    let td = texel(ox + x1, oy + y1);
    let top = ta + (tb - ta) * fx;
    let bot = tc + (td - tc) * fx;
    return top + (bot - top) * fy;
}

@fragment
fn fs_quad(v: VOut) -> @location(0) vec4<f32> {
    // the pixel centre on the canvas (x + 0.5, y + 0.5)
    let p = v.pos.xy - view.canvas.zw;
    let kind = v.d.x & 0xffu;
    if kind == 1u {
        // glyph: coverage times the colour's alpha in integers (cov * a / 255), as the CPU path
        let lx = u32(i32(floor(p.x)) - i32(v.a.x));
        let ly = u32(i32(floor(p.y)) - i32(v.a.y));
        let cov = u32(texel(u32(v.a.z) + lx, u32(v.a.w) + ly).a);
        let a = cov * u32(v.colour.a) / 255u;
        return vec4<f32>(v.colour.rgb / 255.0, f32(a) / 255.0);
    }
    if kind == 2u {
        // image: inverse turn of the pixel centre (pivot in b.xy, matrix rows b.zw / e.xy), inside test,
        // bilinear texel, tint
        let x0 = v.a.x;
        let y0 = v.a.y;
        let x1 = v.a.z;
        let y1 = v.a.w;
        let cx = p.x - v.b.x;
        let cy = p.y - v.b.y;
        let ux = v.b.x + cx * v.b.z + cy * v.b.w;
        let uy = v.b.y + cx * v.e.x + cy * v.e.y;
        if ux < x0 || ux >= x1 || uy < y0 || uy >= y1 {
            discard;
        }
        var s = vec4<f32>(255.0);
        if (v.d.x & 0x100u) != 0u {
            let u = (ux - x0) / (x1 - x0) * v.c.x;
            let w = (uy - y0) / (y1 - y0) * v.c.y;
            s = sample_bilinear(u, w, v.c.z, v.c.w, v.d.y, v.d.z, v.d.w);
        }
        // the compositor stores each value rounded to 8 bits before the image is blended: round here too, so a
        // pixel covered by one item blends exactly the same values
        let col = round(clamp(s * v.colour, vec4<f32>(0.0), vec4<f32>(255.0)));
        return vec4<f32>(col.rgb / 255.0, col.a / 255.0);
    }
    // fill
    return vec4<f32>(v.colour.rgb / 255.0, min(v.colour.a / 255.0, 1.0));
}

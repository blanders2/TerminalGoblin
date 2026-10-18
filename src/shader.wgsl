// One instanced quad pipeline for everything: rounded rects, borders, soft shadows and glyphs.

struct Globals {
    screen: vec2<f32>,
    atlas: vec2<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_smp: sampler;

struct Inst {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) radii: vec4<f32>,
    @location(4) clip: vec4<f32>,
    @location(5) border_color: vec4<f32>,
    // x: kind (0 rect, 1 mask glyph, 2 color glyph, 3 shadow, 4 corner filler),
    // y: border width, z: shadow blur
    @location(6) params: vec4<f32>,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) radii: vec4<f32>,
    @location(4) clip: vec4<f32>,
    @location(5) border_color: vec4<f32>,
    @location(6) params: vec4<f32>,
    @location(7) rect: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    var rect = inst.rect;
    if (inst.params.x > 2.5 && inst.params.x < 3.5) {
        // Shadows bleed outside their rect.
        let pad = inst.params.z * 2.0;
        rect = vec4<f32>(rect.xy - vec2<f32>(pad), rect.zw + vec2<f32>(pad * 2.0));
    }
    let world = rect.xy + corner * rect.zw;
    var out: VOut;
    out.pos = vec4<f32>(world.x / g.screen.x * 2.0 - 1.0, 1.0 - world.y / g.screen.y * 2.0, 0.0, 1.0);
    out.world = world;
    out.uv = (inst.uv.xy + corner * inst.uv.zw) / g.atlas;
    out.color = inst.color;
    out.radii = inst.radii;
    out.clip = inst.clip;
    out.border_color = inst.border_color;
    out.params = inst.params;
    out.rect = inst.rect;
    return out;
}

// Signed distance to a rounded box. radii: top-left, top-right, bottom-right, bottom-left.
fn sd_round_box(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    var r: f32;
    if (p.x < 0.0) {
        r = select(radii.w, radii.x, p.y < 0.0);
    } else {
        r = select(radii.z, radii.y, p.y < 0.0);
    }
    r = min(r, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

fn premul(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    if (in.world.x < in.clip.x || in.world.y < in.clip.y
        || in.world.x > in.clip.x + in.clip.z || in.world.y > in.clip.y + in.clip.w) {
        discard;
    }
    let kind = in.params.x;
    let tex = textureSampleLevel(atlas_tex, atlas_smp, in.uv, 0.0);
    if (kind < 0.5) {
        let half = in.rect.zw * 0.5;
        let p = in.world - (in.rect.xy + half);
        let d = sd_round_box(p, half, in.radii);
        let outer = clamp(0.5 - d, 0.0, 1.0);
        let bw = in.params.y;
        if (bw > 0.0) {
            let inner = clamp(0.5 - (d + bw), 0.0, 1.0);
            let fill = premul(in.color) * inner;
            let border = premul(in.border_color) * (outer - inner);
            return fill + border * (1.0 - fill.a);
        }
        return premul(in.color) * outer;
    } else if (kind < 1.5) {
        return premul(in.color) * tex.a;
    } else if (kind < 2.5) {
        return tex * in.color.a;
    } else if (kind > 3.5) {
        // Corner filler: radii = (center.x, center.y, radius).
        let d = length(in.world - in.radii.xy) - in.radii.z;
        return premul(in.color) * clamp(0.5 + d, 0.0, 1.0);
    }
    let half = in.rect.zw * 0.5;
    let p = in.world - (in.rect.xy + half);
    let blur = max(in.params.z, 0.5);
    let d = sd_round_box(p, half, in.radii);
    let a = 1.0 - smoothstep(-blur, blur, d);
    return premul(in.color) * a;
}

// The sun's map: the terrain's caster mesh (surface nets over cells of two voxels) and the leaf cards, drawn from
// the sun into a depth texture, as vita/shaders/shadow_v.cg and shadow_f.cg draw them into the PS Vita's packed
// distance map. A draw's instance is its chunk's row of the table.

@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var smooth_: sampler;

@vertex
fn caster_vs(@location(0) pos: vec4<i32>, @builtin(instance_index) slot: u32) -> @builtin(position) vec4<f32> {
    let p = vec3<f32>(pos.xyz) * (1.0 / 256.0) + origin(slot);
    return frame.light_vp * vec4<f32>(p, 1.0);
}

struct CutOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn cut_vs(@location(0) pos: vec4<i32>, @location(1) uv: vec2<f32>, @builtin(instance_index) slot: u32) -> CutOut {
    var local = vec3<f32>(pos.xyz) * (1.0 / 256.0);
    local += cos(local.yzx * 1000.0 + frame.eye.w * 0.8) * 0.16;
    var out: CutOut;
    out.clip = frame.light_vp * vec4<f32>(local + origin(slot), 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn cut_fs(i: CutOut) {
    // A fixed level of the atlas: seen from the sun, many cards stand edge-on, and the level the slope of their
    // coordinates picks is so coarse that a card's cut-out becomes a few blocks a voxel wide. Level 2 keeps 32
    // texels of a 128-texel tile.
    if (textureSampleLevel(atlas, smooth_, i.uv, 2.0).a < 0.5) {
        discard;
    }
}

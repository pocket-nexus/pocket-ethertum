// Crossed cards of tall grass, flowers and leaves, as vita/shaders/foliage_v.cg and foliage_f.cg draw them. Each
// vertex sways as the reference's foliage program moves it: by the cosine of its own position (×1000) and the
// time, 0.16 voxel at most. The reference lights the cards as faces that look up. A draw's instance is its
// chunk's row of the table.
//
// The light goes through the display's curve per pixel, with the texel's own colour, where the PS Vita folds the
// curve into each corner for the atlas's mean texel: the grass's tile is far brighter than that mean, and the fold
// takes its texels in full sun to white.

@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var smooth_: sampler;

struct CardIn {
    @location(0) pos: vec4<i32>,
    @location(1) uv: vec2<f32>,
    @location(2) ao: vec4<f32>,
    // the way to the voxel's centre, over two voxels
    @location(3) centre: vec4<f32>,
};

struct CardOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // the light on the card (rgb) and what the fade leaves of it (a)
    @location(1) light: vec4<f32>,
    @location(2) fog: vec3<f32>,
    @location(3) height: f32,
};

@vertex
fn foliage_vs(v: CardIn, @builtin(instance_index) slot: u32) -> CardOut {
    var local = vec3<f32>(v.pos.xyz) * (1.0 / 256.0);
    local += cos(local.yzx * 1000.0 + frame.eye.w * 0.8) * 0.16;
    let from_eye = local + origin(slot);
    let p = mirrored(from_eye);
    var out: CardOut;
    out.clip = frame.vp * vec4<f32>(p, 1.0);
    out.uv = v.uv;
    out.height = from_eye.y + frame.eye.y;
    let fade = fade_of(p);
    out.fog = fade.fog;
    // The sky's light from straight up, the sun's by its height. The reference tests every pixel of a card
    // against the sun's shadow; here each corner is tested at a point three quarters of the way to its voxel's
    // centre, inside the crown where the card's leaves are, not at the corner, which stands out past the crown.
    var sun = max(frame.sun.y, 0.0) * frame.sun.w;
    if (frame.flags.y > 0.5 && frame.flags.x < 0.5) {
        sun *= sunlit_vertex(from_eye + v.centre.xyz * 1.5);
    }
    out.light = vec4<f32>(frame.zenith.rgb * v.ao.x + sun, 1.0 - fade.share);
    return out;
}

@fragment
fn foliage_fs(i: CardOut) -> @location(0) vec4<f32> {
    let t = textureSample(atlas, smooth_, i.uv);
    // The reference's cut-out: texels under half alpha are not drawn.
    if (t.a < 0.5 || (frame.flags.x > 0.5 && i.height < 0.0)) {
        discard;
    }
    return vec4<f32>(display(t.rgb * t.rgb * i.light.rgb) * i.light.a + i.fog, 1.0);
}

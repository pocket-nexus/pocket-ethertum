// The sky box around the eye, as vita/shaders/sky_v.cg and sky_f.cg draw it: the picture's faces as the display
// shows them (crates/ether-look), here one cube read along each pixel's ray. Bevy reads its cube maps as
// left-handed (its sky box samples the direction with z negated), so the ray's z is negated too; in the
// reflection's pass the sky is turned upside down.

@group(1) @binding(0) var sky: texture_cube<f32>;
@group(1) @binding(1) var smooth_: sampler;

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn sky_vs(@builtin(vertex_index) i: u32) -> SkyOut {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var out: SkyOut;
    out.clip = vec4<f32>(xy, 1.0, 1.0);
    out.ndc = xy;
    return out;
}

@fragment
fn sky_fs(i: SkyOut) -> @location(0) vec4<f32> {
    let h = frame.inv_vp * vec4<f32>(i.ndc, 1.0, 1.0);
    var d = h.xyz / h.w;
    if (frame.flags.x > 0.5) {
        d.y = -d.y;
    }
    let c = textureSampleLevel(sky, smooth_, vec3<f32>(d.x, d.y, -d.z), 0.0).rgb;
    let day = frame.horizon.w;
    return vec4<f32>(c * day + vec3<f32>(0.02, 0.025, 0.05) * (1.0 - day), 1.0);
}

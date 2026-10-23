// Faces of water voxels, as vita/shaders/water_v.cg and water_f.cg draw them. The reference draws water opaque and
// mirror-like (deferred, black base colour, roughness 0), the normal of every face rippled by four octaves of a
// normal map at world.xz / 100; three octaves are kept. A draw's instance is its chunk's row of the table.

@group(1) @binding(0) var normals: texture_2d<f32>;
@group(1) @binding(1) var repeat: sampler;
@group(1) @binding(2) var reflection: texture_2d<f32>;

struct WaterIn {
    @location(0) pos: vec4<i32>,
    @location(1) normal: vec4<f32>,
};

struct WaterOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) n0: vec2<f32>,
    @location(1) n1: vec2<f32>,
    @location(2) n2: vec2<f32>,
    // towards the eye; w: 1 on a face that looks up, 0 on a side face
    @location(3) view: vec4<f32>,
    // the fade's sky; w: what the fade leaves
    @location(4) fog: vec4<f32>,
    // the reference's distance fog, which only its water takes: what it adds in linear light, and in w what it
    // leaves of the surface
    @location(5) air: vec4<f32>,
};

@vertex
fn water_vs(v: WaterIn, @builtin(instance_index) slot: u32) -> WaterOut {
    let local = vec3<f32>(v.pos.xyz) * (1.0 / 256.0);
    let p = local + origin(slot);
    var out: WaterOut;
    out.clip = frame.vp * vec4<f32>(p, 1.0);
    let w = chunks[slot].ripple.xz + local.xz * 0.01;
    let t = frame.eye.w;
    out.n0 = w * 1.0 + vec2<f32>(0.080, 0.059) * t;
    out.n1 = w * 2.1 + vec2<f32>(0.073, -0.062) * t;
    out.n2 = w * 7.9 + vec2<f32>(0.153, 0.138) * t;
    out.view = vec4<f32>(normalize(-p), clamp(v.normal.y, 0.0, 1.0));
    let fade = fade_of(p);
    out.fog = vec4<f32>(fade.fog, 1.0 - fade.share);
    // Atmospheric, visibility 1200 m, extinction colour (0.35, 0.5, 0.66), inscattering (110, 230, 255) / 255 and
    // fog colour (0, 0.666, 1), through Bevy's formulas; what it leaves is the mean of its three channels'.
    let d = length(p);
    let added = (1.0 - exp(-d * vec3<f32>(0.0000160, 0.0013212, 0.0024964))) * vec3<f32>(0.0, 0.4011, 1.0);
    let kept = exp(-d * vec3<f32>(0.001872, 0.001297, 0.000642));
    out.air = vec4<f32>(added, (kept.x + kept.y + kept.z) * (1.0 / 3.0));
    return out;
}

fn octave(uv: vec2<f32>) -> vec2<f32> {
    // R and G hold the normal's x and z.
    return textureSample(normals, repeat, uv).rg * 2.0 - 1.0;
}

@fragment
fn water_fs(i: WaterOut) -> @location(0) vec4<f32> {
    // The reference mixes each octave from straight up by its strength (0.16, 0.18, 0.093, 0.044), normalizes each,
    // and normalizes their sum: the sum's tilt is the octaves' over four.
    let xz = (octave(i.n0) * 0.16 + octave(i.n1) * 0.18 + octave(i.n2) * 0.093) * (0.25 * frame.look.y);
    let n = vec3<f32>(xz.x, 1.0, xz.y);
    let v = i.view.xyz;
    let nv = clamp(dot(n, v), 0.0, 1.0);
    let k = 1.0 - nv;
    let k2 = k * k;
    // A black mirror (Schlick's Fresnel): what it shows is what it reflects, in linear light.
    let fresnel = frame.look.x + (1.0 - frame.look.x) * k2 * k2 * k;
    // The scene mirrored in the sea, read where this point is on the screen and moved by the ripples; a side face
    // stands where the mirror image is not, and reflects the sky at the horizon. Back to linear light through the
    // inverse of the display's curve.
    let at = i.clip.xy * frame.screen.zw + xz * frame.look.z;
    var shown = mix(frame.shown.rgb, textureSample(reflection, clamped, at).rgb, i.view.w);
    shown = min(shown, vec3<f32>(0.97));
    let s2 = shown * shown;
    let reflected = 0.8 * s2 / (1.0 - s2);
    // The sun's glint: the reference's specular at its least roughness (α = 0.0079), D·V·F·π·sun with
    // D = α² / π(θ² + α²)², θ the angle between the normal and the half vector, half the reflection's angle from
    // the sun. 4000 ≈ the peak of D·π/4.
    let r = 2.0 * dot(n, v) / dot(n, n) * n - v;
    let c = dot(r, frame.sun.xyz) * inverseSqrt(dot(r, r));
    let tt = (1.0 - c) * 0.5 + 6.2e-5;
    let lobe = 3.84e-9 / (tt * tt);
    let glint = min(4000.0 * lobe * frame.sun.w * frame.look.w * fresnel / max(nv, 0.1), 1000.0);
    let x = (fresnel * reflected + glint) * i.air.w + i.air.rgb;
    return vec4<f32>(display(x) * i.fog.w + i.fog.rgb, 1.0);
}

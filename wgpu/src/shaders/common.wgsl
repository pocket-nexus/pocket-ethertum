// What every program of the world takes: the frame (group 0), and the functions the PS Vita's programs share
// (vita/shaders/frame.cg, shadow.cg), in the same numbers.
//
// Light is linear and in the reference's units after its camera's exposure (crates/ether-look): a texel t (sRGB,
// taken as t²) under light E shows as display(t² E), the reference's tone map and sRGB encoding in one curve.
// Positions are from the eye: a vertex is its chunk's corner (the table, by the draw's instance) less the eye,
// plus its own 1/256 voxels, so the matrices carry no translation and far chunks keep their precision.

struct Frame {
    // projection × the view's rotation
    vp: mat4x4<f32>,
    // its inverse, for the sky's directions
    inv_vp: mat4x4<f32>,
    // a point from the eye → the sun's map: x and y −1…1 across the rays, z 0…1 from the map's top plane
    light_vp: mat4x4<f32>,
    // xyz: the eye; w: seconds
    eye: vec4<f32>,
    // xyz: towards the sun; w: the sun's light on a face that faces it
    sun: vec4<f32>,
    // rgb: the sky's light from straight up; w: 1 / the distance where the world ends
    zenith: vec4<f32>,
    // rgb: from straight down
    nadir: vec4<f32>,
    // rgb: from the horizon; w: daylight on the sky picture
    horizon: vec4<f32>,
    // rgb: the sky at the horizon as shown, which a water's side face reflects
    shown: vec4<f32>,
    // x: 1 in the reflection's pass (points mirrored in the sea's plane); y: 1 with the sun's map;
    // z: the terrain's bias in the map's units; w: a leaf card's
    flags: vec4<f32>,
    // the water: reflectance looking straight down, ripples' tilt, their shift of the reflection, the sun's glint
    look: vec4<f32>,
    // the target: width, height, 1/width, 1/height
    screen: vec4<f32>,
};

struct Chunk {
    corner: vec4<f32>,
    uv: vec4<f32>,
    ripple: vec4<f32>,
    spare: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var ring: texture_2d<f32>;
@group(0) @binding(2) var clamped: sampler;
@group(0) @binding(3) var sun_map: texture_depth_2d;
@group(0) @binding(4) var sun_compare: sampler_comparison;
@group(0) @binding(5) var<storage, read> chunks: array<Chunk>;

// A chunk's corner from the eye.
fn origin(slot: u32) -> vec3<f32> {
    return chunks[slot].corner.xyz - frame.eye.xyz;
}

// The reflection's pass draws each point at its mirror image across the sea's plane, y = 0, as the eye sees it.
fn mirrored(p: vec3<f32>) -> vec3<f32> {
    if (frame.flags.x > 0.5) {
        return vec3<f32>(p.x, -p.y - 2.0 * frame.eye.y, p.z);
    }
    return p;
}

// The reference lights a face with its sky box read in the face's normal: the sky's light by height, from the
// horizon up to the zenith and down to the nadir (which it nears faster), less the occlusion.
fn ambient_of(n: vec3<f32>, ao: f32) -> vec3<f32> {
    var sky: vec3<f32>;
    if (n.y >= 0.0) {
        sky = mix(frame.horizon.rgb, frame.zenith.rgb, n.y);
    } else {
        sky = mix(frame.horizon.rgb, frame.nadir.rgb, sqrt(max(-n.y, 0.0)));
    }
    return sky * ao;
}

// The sun's light on a face: N·L by its light.
fn sun_of(n: vec3<f32>) -> f32 {
    return max(dot(n, frame.sun.xyz), 0.0) * frame.sun.w;
}

// The display's curve folded into a vertex's light for a texel whose t² is a0: the fragment shows t · curve_of(E),
// which is display(t² E) for that texel and near it for the rest of the material (ether_look::DISPLAY_K).
fn curve_of(e: vec3<f32>, a0: vec3<f32>) -> vec3<f32> {
    return sqrt(e / (a0 * e + 0.8));
}

// The curve itself, per fragment (the water's).
fn display(x: vec3<f32>) -> vec3<f32> {
    return sqrt(x / (x + 0.8));
}

// The world fades into the sky over the last quarter of the loaded distance: towards the sky picture's own colour
// in the direction of the point, read from the octahedral map of the sky as shown.
fn fog_of(p: vec3<f32>) -> f32 {
    let d = clamp((length(p) * frame.zenith.w - 0.75) * 4.0, 0.0, 1.0);
    return d * d;
}

fn sky_toward(d: vec3<f32>) -> vec3<f32> {
    // (the octahedral fold needs no unit vector and no angle)
    var q = d.xz / (abs(d.x) + abs(d.y) + abs(d.z));
    if (d.y < 0.0) {
        q = (1.0 - abs(q.yx)) * vec2<f32>(select(-1.0, 1.0, q.x >= 0.0), select(-1.0, 1.0, q.y >= 0.0));
    }
    let day = frame.horizon.w;
    return textureSampleLevel(ring, clamped, q * 0.5 + 0.5, 0.0).rgb * day + vec3<f32>(0.02, 0.025, 0.05) * (1.0 - day);
}

// The fade's share at p, and in `fog` the sky's colour behind p by that share. The reflection's sky is the sky
// turned upside down.
struct Fade {
    share: f32,
    fog: vec3<f32>,
};

fn fade_of(p: vec3<f32>) -> Fade {
    let f = fog_of(p);
    var d = p;
    if (frame.flags.x > 0.5) {
        d.y = -d.y;
    }
    return Fade(f, sky_toward(d) * f);
}

// Where a point from the eye is in the sun's map: the texture's coordinates and the depth from the top plane.
fn sun_coord(p: vec3<f32>) -> vec3<f32> {
    let c = frame.light_vp * vec4<f32>(p, 1.0);
    return vec3<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5, c.z);
}

fn inside(s: vec3<f32>) -> bool {
    return s.x > 0.002 && s.x < 0.998 && s.y > 0.002 && s.y < 0.998 && s.z > 0.0 && s.z < 1.0;
}

// The same test in a vertex program (the leaf cards): one texel, its depth against the point's, less the bias.
fn sunlit_vertex(p: vec3<f32>) -> f32 {
    let s = sun_coord(p);
    if (!inside(s)) {
        return 1.0;
    }
    let size = vec2<f32>(textureDimensions(sun_map));
    let stored = textureLoad(sun_map, vec2<i32>(s.xy * size), 0);
    return select(0.0, 1.0, s.z - frame.flags.w <= stored);
}

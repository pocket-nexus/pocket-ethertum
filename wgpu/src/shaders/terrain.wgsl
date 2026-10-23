// The isosurface and placed cubes of one material, textured from three sides (the reference's triplanar mapping),
// as vita/shaders/terrain_v.cg and terrain_f.cg draw them. A draw's instance is its chunk's row of the table times
// 32 plus the material's index (`render.rs`); the materials are the layers of one texture array.

struct Materials {
    // each material's mean texel, squared per channel (t²): the vertex folds the display's curve for it
    albedo: array<vec4<f32>, 32>,
};

@group(1) @binding(0) var terrain: texture_2d_array<f32>;
@group(1) @binding(1) var repeat: sampler;
@group(1) @binding(2) var<uniform> materials: Materials;

struct TerrainIn {
    @location(0) pos: vec4<i32>,
    @location(1) normal: vec4<f32>,
    // occlusion, the batch material's weight at this vertex, the x and z projections' shares
    @location(2) misc: vec4<f32>,
};

struct TerrainOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv_x: vec2<f32>,
    @location(1) uv_y: vec2<f32>,
    @location(2) uv_z: vec2<f32>,
    // the three projections' shares, and the batch material's weight (a layer's alpha)
    @location(3) blend: vec4<f32>,
    // the light in shade (or lit, with no sun's map), through the curve, at a third and scaled by what the fade
    // leaves; w: what the sun adds, the same way
    @location(4) light: vec4<f32>,
    @location(5) fog: vec3<f32>,
    // where the point is in the sun's map, half a voxel out along its normal
    @location(6) sun: vec3<f32>,
    // height above the sea (the reflection keeps only what stands above it)
    @location(7) height: f32,
    @location(8) @interpolate(flat) material: u32,
};

@vertex
fn terrain_vs(v: TerrainIn, @builtin(instance_index) instance: u32) -> TerrainOut {
    let slot = instance / 32u;
    let material = instance % 32u;
    let local = vec3<f32>(v.pos.xyz) * (1.0 / 256.0);
    let from_eye = local + origin(slot);
    let p = mirrored(from_eye);
    var out: TerrainOut;
    out.clip = frame.vp * vec4<f32>(p, 1.0);
    out.height = from_eye.y + frame.eye.y;
    // the chunk corner's texture coordinates, wrapped, and one over the sample scale (1.5)
    let c = chunks[slot].uv;
    let t = c.xyz + local * c.w;
    out.uv_x = vec2<f32>(-t.z, -t.y);
    out.uv_y = vec2<f32>(t.x, t.z);
    out.uv_z = vec2<f32>(t.x, -t.y);
    let n = v.normal.xyz;
    let ao = v.misc.x;
    out.blend = vec4<f32>(v.misc.z, max(1.0 - v.misc.z - v.misc.w, 0.0), v.misc.w, v.misc.y);
    out.material = material;
    let fade = fade_of(p);
    out.fog = fade.fog;
    let a0 = materials.albedo[material].rgb;
    let shade = ambient_of(n, ao);
    let lit = curve_of(shade + sun_of(n), a0);
    let scale = (1.0 - fade.share) * (1.0 / 3.0);
    if (frame.flags.y > 0.5 && frame.flags.x < 0.5) {
        let dark = curve_of(shade, a0);
        out.light = vec4<f32>(dark * scale, dot(lit - dark, vec3<f32>(1.0 / 3.0)) * scale);
        out.sun = sun_coord(from_eye + n * 0.5);
    } else {
        out.light = vec4<f32>(lit * scale, 0.0);
        out.sun = vec3<f32>(-1.0);
    }
    return out;
}

@fragment
fn terrain_fs(i: TerrainOut) -> @location(0) vec4<f32> {
    let m = i32(i.material);
    let c = textureSample(terrain, repeat, i.uv_x, m).rgb * i.blend.x + textureSample(terrain, repeat, i.uv_y, m).rgb * i.blend.y + textureSample(terrain, repeat, i.uv_z, m).rgb * i.blend.z;
    var light = i.light.rgb;
    if (i.light.w > 0.0) {
        var lit = 1.0;
        if (inside(i.sun)) {
            lit = textureSampleCompareLevel(sun_map, sun_compare, i.sun.xy, i.sun.z - frame.flags.z);
        }
        light += i.light.w * lit;
    }
    if (frame.flags.x > 0.5 && i.height < 0.0) {
        discard;
    }
    return vec4<f32>(c * light * 3.0 + i.fog, i.blend.w);
}

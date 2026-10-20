//! Chunk meshes on the GPU and the passes that draw them.
//!
//! Each meshed chunk owns one range of the [`Heap`]: its terrain vertices and
//! indices, then its foliage's, then its water's. A frame draws:
//!
//! 1. the sky box, six faces around the eye, without depth;
//! 2. every visible chunk's terrain, one draw per material with that
//!    material's texture, depth written, nearest chunk first;
//! 3. the layers where materials meet, blended by each vertex's weight of the
//!    material over the opaque pass (the reference blends materials per
//!    fragment from three texture reads each; here a triangle that touches a
//!    second material is drawn once more for it);
//! 4. the water, opaque and mirror-like as the reference draws it;
//! 5. the foliage cards, with the reference's cut-out at half alpha.

use std::collections::HashMap;

use ether_voxel::glam::{IVec3, Mat4, Vec3, Vec4};
use ether_voxel::mesh::{Batch, ChunkMesh, FoliageVertex, LiquidVertex, TerrainVertex};
use pocket_vita_gxm::mem::{Block, Kind};
use vita2d_sys as g;

use crate::gpu::{self, Blend, Cull, Gpu, Param, Program, Stream, Uniforms};
use crate::heap::{Heap, Range};
use crate::textures::Textures;

const FRAME_CG: &str = include_str!("../shaders/frame.cg");

/// The reference's `sample_scale`: a terrain texture repeats every 1.5 voxels.
const SAMPLE_SCALE: f32 = 1.5;

struct Part {
    vb: usize,
    ib: usize,
    count: u32,
}

struct ChunkGpu {
    range: Range,
    terrain_vb: usize,
    terrain_ib: usize,
    batches: Vec<Batch>,
    foliage: Part,
    water: Part,
    tris: u32,
    /// The last frame that drew it.
    drawn: u64,
}

/// What a frame drew.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub chunks: u32,
    pub culled: u32,
    pub draws: u32,
    pub tris: u32,
    /// Triangles by pass: terrain (opaque), terrain layers, water, foliage.
    pub by_pass: [u32; 4],
}

/// The view of one frame: eye, rotation and projection, and the frame's light.
pub struct View {
    pub eye: Vec3,
    /// Projection × rotation (no translation: positions arrive relative to the eye).
    pub vp: Mat4,
    pub frame: [f32; 16],
    pub fog: [f32; 3],
    pub sky_zenith: [f32; 3],
    pub sky_horizon: [f32; 3],
    /// Daylight on the sky picture, and the night's own colour.
    pub sky_tint: [f32; 3],
    pub sky_night: [f32; 3],
    pub sun_dir: Vec3,
    pub sun: f32,
    /// Chunks farther than this (in voxels, on the larger axis) are not drawn.
    pub reach: f32,
    /// Which passes to draw (all, unless a measurement or a check turns one off).
    pub foliage: bool,
    pub terrain: bool,
    pub water: bool,
    /// Fog and the occlusion term off, for checking what the textures alone give.
    pub plain: bool,
}

struct Params3 {
    vp: Param,
    origin: Param,
    extra: Param,
    frame: Param,
    look: Param,
}

pub struct Scene {
    pub heap: Heap,
    pub textures: Textures,
    chunks: HashMap<IVec3, ChunkGpu>,
    terrain: Program,
    tp: Params3,
    foliage: Program,
    fp: Params3,
    water: Program,
    wp: Params3,
    sky: Program,
    sky_vp: Param,
    sky_tint: Param,
    sky_night: Param,
    sky_box: Block,
    /// Visible chunks: distance², position, and whether any of it lies in the fade band (drawn blended).
    order: Vec<(f32, IVec3, bool)>,
}

const TERRAIN_STRIDE: u16 = core::mem::size_of::<TerrainVertex>() as u16;
const FOLIAGE_STRIDE: u16 = core::mem::size_of::<FoliageVertex>() as u16;
const WATER_STRIDE: u16 = core::mem::size_of::<LiquidVertex>() as u16;

/// The sky box's corners: per face (+X, −X, +Y, −Y, +Z, −Z) four of (position, uv), with `s = 2u − 1`,
/// `t = 2v − 1` and the face's direction as `ether-pack` documents the faces.
fn sky_box_vertices() -> Vec<f32> {
    let face = |f: usize, s: f32, t: f32| -> [f32; 3] {
        match f {
            0 => [1.0, -t, -s],
            1 => [-1.0, -t, s],
            2 => [s, 1.0, t],
            3 => [s, -1.0, -t],
            4 => [s, -t, 1.0],
            _ => [-s, -t, -1.0],
        }
    };
    let mut v = Vec::new();
    for f in 0..6 {
        for (u, w) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            let p = face(f, u * 2.0 - 1.0, w * 2.0 - 1.0);
            v.extend_from_slice(&[p[0] * 10.0, p[1] * 10.0, p[2] * 10.0, u, w]);
        }
    }
    v
}

impl Scene {
    pub unsafe fn new(gpu: &mut Gpu, msaa: u32, heap_bytes: usize, textures: Textures) -> Result<Scene, String> {
        let s16 = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_S16;
        let s8n = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_S8N;
        let u8n = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_U8N;
        let u16n = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_U16N;
        let f32_ = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_F32;
        let terrain = gpu.program(
            "terrain",
            FRAME_CG,
            include_str!("../shaders/terrain_v.cg"),
            include_str!("../shaders/terrain_f.cg"),
            &Stream { stride: TERRAIN_STRIDE, attrs: &[("aPosition", 0, s16, 3), ("aNormal", 6, s8n, 3), ("aAo", 9, u8n, 1), ("aWeight", 10, u8n, 1)] },
            msaa,
            &[Blend::Opaque, Blend::Alpha],
        )?;
        let foliage = gpu.program(
            "foliage",
            FRAME_CG,
            include_str!("../shaders/foliage_v.cg"),
            include_str!("../shaders/foliage_f.cg"),
            &Stream { stride: FOLIAGE_STRIDE, attrs: &[("aPosition", 0, s16, 3), ("aUv", 6, u16n, 2), ("aAo", 10, u8n, 1)] },
            msaa,
            &[Blend::Opaque, Blend::Alpha],
        )?;
        let water = gpu.program(
            "water",
            FRAME_CG,
            include_str!("../shaders/water_v.cg"),
            include_str!("../shaders/water_f.cg"),
            &Stream { stride: WATER_STRIDE, attrs: &[("aPosition", 0, s16, 3), ("aNormal", 6, s8n, 3)] },
            msaa,
            &[Blend::Opaque, Blend::Alpha],
        )?;
        let sky = gpu.program("sky", "", include_str!("../shaders/sky_v.cg"), include_str!("../shaders/sky_f.cg"), &Stream { stride: 20, attrs: &[("aPosition", 0, f32_, 3), ("aUv", 12, f32_, 2)] }, msaa, &[Blend::Opaque])?;
        let tp = Params3 { vp: terrain.vparam("uVp"), origin: terrain.vparam("uOrigin"), extra: terrain.vparam("uUv"), frame: terrain.vparam("uFrame"), look: terrain.fparam("uFog") };
        let fp = Params3 { vp: foliage.vparam("uVp"), origin: foliage.vparam("uOrigin"), extra: core::ptr::null(), frame: foliage.vparam("uFrame"), look: foliage.fparam("uFog") };
        let wp = Params3 { vp: water.vparam("uVp"), origin: water.vparam("uOrigin"), extra: water.vparam("uWorld"), frame: water.vparam("uFrame"), look: water.fparam("uLook") };
        let (sky_vp, sky_tint, sky_night) = (sky.vparam("uVp"), sky.fparam("uTint"), sky.fparam("uNight"));
        let verts = sky_box_vertices();
        let mut sky_box = Block::with_access(Kind::Main, 4096, false)?;
        let vb = sky_box.alloc(verts.len() * 4, 16).ok_or("sky box")?;
        core::ptr::copy_nonoverlapping(verts.as_ptr().cast::<u8>(), vb, verts.len() * 4);
        let ib = sky_box.alloc(8, 16).ok_or("sky box")? as *mut u16;
        for (i, k) in [0u16, 1, 2, 3].iter().enumerate() {
            *ib.add(i) = *k;
        }
        Ok(Scene {
            heap: Heap::new(heap_bytes)?,
            textures,
            chunks: HashMap::new(),
            terrain,
            tp,
            foliage,
            fp,
            water,
            wp,
            sky,
            sky_vp,
            sky_tint,
            sky_night,
            sky_box,
            order: Vec::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Puts a chunk's mesh on the GPU in place of its previous one. False when the heap has no room.
    pub unsafe fn upload(&mut self, at: IVec3, mesh: &ChunkMesh, frame: u64) -> bool {
        self.remove(at, frame);
        let (t, fo, w) = (&mesh.terrain, &mesh.foliage, &mesh.liquid);
        if t.indices.is_empty() && fo.indices.is_empty() && w.indices.is_empty() {
            return true;
        }
        let a16 = |n: usize| n.div_ceil(16) * 16;
        let sizes = [
            t.vertices.len() * TERRAIN_STRIDE as usize,
            t.indices.len() * 2,
            fo.vertices.len() * FOLIAGE_STRIDE as usize,
            fo.indices.len() * 2,
            w.vertices.len() * WATER_STRIDE as usize,
            w.indices.len() * 2,
        ];
        let total: usize = sizes.iter().map(|&n| a16(n)).sum();
        let Some(range) = self.heap.alloc(total) else { return false };
        let sources: [*const u8; 6] = [t.vertices.as_ptr().cast(), t.indices.as_ptr().cast(), fo.vertices.as_ptr().cast(), fo.indices.as_ptr().cast(), w.vertices.as_ptr().cast(), w.indices.as_ptr().cast()];
        let mut offsets = [0usize; 6];
        let mut at_b = 0;
        for i in 0..6 {
            offsets[i] = at_b;
            self.heap.write(range, at_b, core::slice::from_raw_parts(sources[i], sizes[i]));
            at_b += a16(sizes[i]);
        }
        let tris = ((t.indices.len() + fo.indices.len() + w.indices.len()) / 3) as u32;
        self.chunks.insert(
            at,
            ChunkGpu {
                range,
                terrain_vb: offsets[0],
                terrain_ib: offsets[1],
                batches: t.batches.clone(),
                foliage: Part { vb: offsets[2], ib: offsets[3], count: fo.indices.len() as u32 },
                water: Part { vb: offsets[4], ib: offsets[5], count: w.indices.len() as u32 },
                tris,
                drawn: 0,
            },
        );
        true
    }

    /// Drops a chunk's mesh; its memory returns once the frames that drew it are done.
    pub fn remove(&mut self, at: IVec3, frame: u64) {
        if let Some(c) = self.chunks.remove(&at) {
            self.heap.retire(c.range, c.drawn.max(frame));
        }
    }

    unsafe fn chunk_uniforms(&self, ctx: *mut g::SceGxmContext, p: &Params3, vp: &[f32; 16], v: &View, at: IVec3) {
        let corner = (at * 16).as_vec3();
        let origin = corner - v.eye;
        let u = Uniforms::vertex(ctx);
        u.set(p.vp, vp);
        u.set(p.origin, &[origin.x, origin.y, origin.z, 0.0]);
        u.set(p.frame, &v.frame);
        if !p.extra.is_null() {
            u.set(p.extra, &extra(p as *const _ == &self.wp as *const _, at));
        }
    }

    /// Draws the frame's scene into the open display scene. `frame` is the number of this frame.
    pub unsafe fn draw(&mut self, ctx: *mut g::SceGxmContext, v: &View, frame: u64) -> Stats {
        let mut st = Stats::default();
        g::sceGxmSetViewport(ctx, 480.0, 480.0, 272.0, -272.0, 0.5, 0.5);
        // Cg reads a float4x4 uniform row by row (`mul(M, v)` dots each row with v); glam stores columns.
        let vp = v.vp.transpose().to_cols_array();
        self.draw_sky(ctx, &vp, v);

        // Visible chunks, nearest first.
        let planes = frustum(&v.vp);
        self.order.clear();
        for at in self.chunks.keys() {
            let min = (*at * 16).as_vec3() - v.eye;
            let d = (min + 8.0).abs().max_element();
            if d - 8.0 > v.reach {
                continue;
            }
            if !visible(&planes, min, min + 16.0) {
                st.culled += 1;
                continue;
            }
            self.order.push(((min + 8.0).length_squared(), *at, false));
        }
        self.order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        st.chunks = self.order.len() as u32;

        // Terrain: one draw per material, its texture bound. The base pass draws the near chunks opaque, then the
        // ones in the fade band blended (depth still written, so a chunk's nearest surface is the one that fades);
        // the layer pass is blended throughout.
        g::sceGxmSetVertexTexture(ctx, 0, &self.textures.ring.gxm);
        if v.terrain {
            gpu::state_opaque(ctx, Cull::Back);
            for far in [false] {
                self.terrain.bind(ctx, far as usize);
                self.terrain_pass(ctx, &vp, v, &mut st, 0, Some(far));
            }
            gpu::state_blend(ctx, Cull::Back);
            self.terrain.bind(ctx, 1);
            self.terrain_pass(ctx, &vp, v, &mut st, 1, None);
        }
        for i in 0..self.order.len() {
            if let Some(c) = self.chunks.get_mut(&self.order[i].1) {
                c.drawn = frame;
            }
        }

        // Water.
        gpu::state_opaque(ctx, Cull::Back);
        g::sceGxmSetFragmentTexture(ctx, 0, &self.textures.water.gxm);
        let look = [0.02, 0.06, 0.07, 0.0, 0.0, 0.0, 0.0, 0.0, v.sky_zenith[0], v.sky_zenith[1], v.sky_zenith[2], 0.0, v.sky_horizon[0], v.sky_horizon[1], v.sky_horizon[2], v.sun, v.sun_dir.x, v.sun_dir.y, v.sun_dir.z, 0.0];
        for far in [false] {
            if !v.water {
                break;
            }
            self.water.bind(ctx, far as usize);
            for i in 0..self.order.len() {
                let (_, at, f) = self.order[i];
                let c = &self.chunks[&at];
                if c.water.count == 0 || f != far {
                    continue;
                }
                self.chunk_uniforms(ctx, &self.wp, &vp, v, at);
                let u = Uniforms::fragment(ctx);
                u.set(self.wp.look, &look);
                gpu::draw(ctx, self.heap.ptr(c.range, c.water.vb), self.heap.ptr(c.range, c.water.ib).cast(), c.water.count);
                st.draws += 1;
                st.by_pass[2] += c.water.count / 3;
            }
        }

        // Foliage: two-sided cards.
        if v.foliage {
            gpu::state_opaque(ctx, Cull::None);
            g::sceGxmSetFragmentTexture(ctx, 0, &self.textures.foliage.gxm);
            for far in [false] {
                self.foliage.bind(ctx, far as usize);
                for i in 0..self.order.len() {
                    let (d2, at, f) = self.order[i];
                    let c = &self.chunks[&at];
                    if c.foliage.count == 0 || f != far {
                        continue;
                    }
                    // Cards are stored in a hashed order (`mesh::Builder`): a prefix is an even share of them. All
                    // are drawn within 24 voxels, a quarter at the edge of the loaded radius.
                    let d = d2.sqrt();
                    let share = (1.0 - (d - 24.0).max(0.0) / (v.reach - 24.0).max(1.0) * 0.75).clamp(0.25, 1.0);
                    let count = (((c.foliage.count / 6) as f32 * share).ceil() as u32 * 6).min(c.foliage.count);
                    self.chunk_uniforms(ctx, &self.fp, &vp, v, at);
                    gpu::draw(ctx, self.heap.ptr(c.range, c.foliage.vb), self.heap.ptr(c.range, c.foliage.ib).cast(), count);
                    st.draws += 1;
                    st.by_pass[3] += count / 3;
                }
            }
        }
        st.tris = st.by_pass.iter().sum();
        st
    }

    /// One terrain pass: `pass` 0 draws the base batches (of the near or the far chunks, `far`), 1 the layers.
    unsafe fn terrain_pass(&self, ctx: *mut g::SceGxmContext, vp: &[f32; 16], v: &View, st: &mut Stats, pass: usize, far: Option<bool>) {
        let mut bound = 0u8;
        for &(_, at, f) in &self.order {
            if far.is_some_and(|x| x != f) {
                continue;
            }
            let c = &self.chunks[&at];
            for b in &c.batches {
                let (first, count) = if pass == 0 { b.opaque } else { b.layer };
                if count == 0 {
                    continue;
                }
                if bound != b.material {
                    let t = &self.textures.terrain[(b.material.max(1) - 1).min(self.textures.terrain.len() as u8 - 1) as usize];
                    g::sceGxmSetFragmentTexture(ctx, 0, &t.gxm);
                    bound = b.material;
                }
                self.chunk_uniforms(ctx, &self.tp, vp, v, at);
                let vb = self.heap.ptr(c.range, c.terrain_vb + b.base as usize * TERRAIN_STRIDE as usize);
                let ib = self.heap.ptr(c.range, c.terrain_ib + first as usize * 2).cast();
                gpu::draw(ctx, vb, ib, count);
                st.draws += 1;
                st.by_pass[pass] += count / 3;
            }
        }
    }

    unsafe fn draw_sky(&mut self, ctx: *mut g::SceGxmContext, vp: &[f32; 16], v: &View) {
        gpu::state_overlay(ctx);
        self.sky.bind(ctx, 0);
        let base = self.sky_box.base();
        for face in 0..6 {
            g::sceGxmSetFragmentTexture(ctx, 0, &self.textures.sky[face].gxm);
            let u = Uniforms::vertex(ctx);
            u.set(self.sky_vp, vp);
            let f = Uniforms::fragment(ctx);
            f.set(self.sky_tint, &[v.sky_tint[0], v.sky_tint[1], v.sky_tint[2], 0.0]);
            f.set(self.sky_night, &[v.sky_night[0], v.sky_night[1], v.sky_night[2], 0.0]);
            g::sceGxmSetVertexStream(ctx, 0, base.add(face * 4 * 20).cast());
            // The four indices follow the 480 bytes of corners.
            g::sceGxmDraw(ctx, g::SceGxmPrimitiveType_SCE_GXM_PRIMITIVE_TRIANGLE_STRIP, g::SceGxmIndexFormat_SCE_GXM_INDEX_FORMAT_U16, base.add(480).cast(), 4);
        }
    }

    /// Bytes of chunk meshes on the GPU.
    pub fn bytes(&self) -> usize {
        self.heap.used
    }
}

/// Per-chunk coordinates kept small: the terrain's texture coordinates of the chunk's corner wrapped at whole
/// repeats of the texture (and 1 / the sample scale), or the water's at whole periods of its ripples.
fn extra(water: bool, at: IVec3) -> [f32; 4] {
    let c = at * 16;
    if water {
        // Ripples repeat every 100 voxels at the first octave; 2100 is a common period of the first three
        // (scales 1, 2.1 and 7.9 cycle together over 1000 periods; 2100 keeps the first two whole).
        let w = |x: i32| (x.rem_euclid(2100)) as f32 * 0.01;
        [w(c.x), 0.0, w(c.z), 0.0]
    } else {
        // One repeat is 1.5 voxels: 3 voxels hold two whole repeats.
        let w = |x: i32| (x.rem_euclid(3 * 256)) as f32 / SAMPLE_SCALE;
        [w(c.x), w(c.y), w(c.z), 1.0 / SAMPLE_SCALE]
    }
}

/// The six planes of a view-projection, as `(normal, d)` with points inside where `n·p + d >= 0`.
fn frustum(m: &Mat4) -> [Vec4; 6] {
    let r = |i: usize| m.row(i);
    let (r0, r1, r2, r3) = (r(0), r(1), r(2), r(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2]
}

fn visible(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| {
        let c = Vec3::new(if p.x >= 0.0 { max.x } else { min.x }, if p.y >= 0.0 { max.y } else { min.y }, if p.z >= 0.0 { max.z } else { min.z });
        p.x * c.x + p.y * c.y + p.z * c.z + p.w >= 0.0
    })
}

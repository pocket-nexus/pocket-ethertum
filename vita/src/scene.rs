//! Chunk meshes on the GPU and the passes that draw them.
//!
//! Each meshed chunk owns one range of the [`Heap`]: its terrain vertices and
//! indices, then its foliage's, then its water's. A frame draws the sky, then
//! every visible chunk's terrain batches opaque (one draw per material), then
//! the blended layers where materials meet, then the water, nearest chunk
//! first in each pass.

use std::collections::HashMap;

use ether_voxel::glam::{IVec3, Mat4, Vec3, Vec4};
use ether_voxel::mesh::{Batch, ChunkMesh, FoliageVertex, LiquidVertex, TerrainVertex};
use pocket_vita_gxm::mem::Ring;
use vita2d_sys as g;

use crate::gpu::{self, Blend, Cull, Gpu, Param, Program, Stream, Uniforms};
use crate::heap::{Heap, Range};

const FRAME_CG: &str = include_str!("../shaders/frame.cg");

/// The average colour of each terrain tile of the reference's diffuse atlas (sRGB), material id − 1. They stand
/// in for the textures until the pack carries them.
const TILE_COLOUR: [[u8; 3]; 24] = [
    [53, 45, 36], [116, 92, 68], [64, 63, 57], [117, 116, 111], [54, 53, 48], [100, 69, 50], [82, 58, 37], [52, 62, 38],
    [106, 105, 105], [102, 108, 109], [86, 85, 82], [48, 59, 16], [58, 61, 17], [83, 71, 48], [95, 73, 54], [74, 70, 68],
    [63, 63, 28], [77, 77, 74], [148, 124, 95], [81, 77, 72], [59, 52, 45], [90, 86, 84], [72, 69, 65], [9, 142, 126],
];

fn tile_colour(material: u8) -> [f32; 4] {
    let c = TILE_COLOUR[(material.max(1) - 1).min(23) as usize];
    // Into linear light and back would be the honest path; the picture is lit in gamma space throughout.
    [c[0] as f32 / 255.0 * 1.6, c[1] as f32 / 255.0 * 1.6, c[2] as f32 / 255.0 * 1.6, 1.0]
}

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
    pub sun_dir: Vec3,
    pub sun: f32,
    /// Chunks farther than this (in voxels, on the larger axis) are not drawn.
    pub reach: f32,
}

struct TerrainParams {
    vp: Param,
    origin: Param,
    frame: Param,
    look: Param,
}

pub struct Scene {
    pub heap: Heap,
    chunks: HashMap<IVec3, ChunkGpu>,
    terrain: Program,
    tp: TerrainParams,
    water: Program,
    wp: TerrainParams,
    sky: Program,
    sky_param: Param,
    ring: Ring,
    order: Vec<(f32, IVec3)>,
}

const TERRAIN_STRIDE: u16 = core::mem::size_of::<TerrainVertex>() as u16;
const WATER_STRIDE: u16 = core::mem::size_of::<LiquidVertex>() as u16;

impl Scene {
    pub unsafe fn new(gpu: &mut Gpu, msaa: u32, heap_bytes: usize) -> Result<Scene, String> {
        let s16 = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_S16;
        let s8n = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_S8N;
        let u8n = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_U8N;
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
        let water = gpu.program(
            "water",
            FRAME_CG,
            include_str!("../shaders/water_v.cg"),
            include_str!("../shaders/water_f.cg"),
            &Stream { stride: WATER_STRIDE, attrs: &[("aPosition", 0, s16, 3), ("aNormal", 6, s8n, 3)] },
            msaa,
            &[Blend::Alpha],
        )?;
        let sky = gpu.program("sky", "", include_str!("../shaders/sky_v.cg"), include_str!("../shaders/sky_f.cg"), &Stream { stride: 20, attrs: &[("aPosition", 0, f32_, 2), ("aDirection", 8, f32_, 3)] }, msaa, &[Blend::Opaque])?;
        let tp = TerrainParams { vp: terrain.vparam("uVp"), origin: terrain.vparam("uOrigin"), frame: terrain.vparam("uFrame"), look: terrain.fparam("uLook") };
        let wp = TerrainParams { vp: water.vparam("uVp"), origin: water.vparam("uOrigin"), frame: water.vparam("uFrame"), look: water.fparam("uLook") };
        let sky_param = sky.fparam("uSky");
        Ok(Scene {
            heap: Heap::new(heap_bytes)?,
            chunks: HashMap::new(),
            terrain,
            tp,
            water,
            wp,
            sky,
            sky_param,
            ring: Ring::new(4096, 3)?,
            order: Vec::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Puts a chunk's mesh on the GPU in place of its previous one. False when the heap has no room.
    pub unsafe fn upload(&mut self, at: IVec3, mesh: &ChunkMesh, frame: u64) -> bool {
        self.remove(at, frame);
        let t = &mesh.terrain;
        let w = &mesh.liquid;
        if t.indices.is_empty() && w.indices.is_empty() {
            return true;
        }
        let a16 = |n: usize| n.div_ceil(16) * 16;
        let tv = t.vertices.len() * TERRAIN_STRIDE as usize;
        let ti = t.indices.len() * 2;
        let wv = w.vertices.len() * WATER_STRIDE as usize;
        let wi = w.indices.len() * 2;
        let total = a16(tv) + a16(ti) + a16(wv) + a16(wi);
        let Some(range) = self.heap.alloc(total) else { return false };
        let bytes = |v: &[u8]| v.to_vec();
        let as_bytes = |p: *const u8, n: usize| core::slice::from_raw_parts(p, n);
        let mut at_b = 0;
        let terrain_vb = at_b;
        self.heap.write(range, at_b, as_bytes(t.vertices.as_ptr().cast(), tv));
        at_b += a16(tv);
        let terrain_ib = at_b;
        self.heap.write(range, at_b, as_bytes(t.indices.as_ptr().cast(), ti));
        at_b += a16(ti);
        let water_vb = at_b;
        self.heap.write(range, at_b, as_bytes(w.vertices.as_ptr().cast(), wv));
        at_b += a16(wv);
        let water_ib = at_b;
        self.heap.write(range, at_b, as_bytes(w.indices.as_ptr().cast(), wi));
        let _ = bytes;
        let tris = ((t.indices.len() + w.indices.len()) / 3) as u32;
        self.chunks.insert(at, ChunkGpu { range, terrain_vb, terrain_ib, batches: t.batches.clone(), water: Part { vb: water_vb, ib: water_ib, count: w.indices.len() as u32 }, tris, drawn: 0 });
        true
    }

    /// Drops a chunk's mesh; its memory returns once the frames that drew it are done.
    pub fn remove(&mut self, at: IVec3, frame: u64) {
        if let Some(c) = self.chunks.remove(&at) {
            self.heap.retire(c.range, c.drawn.max(frame));
        }
    }

    /// Draws the frame's scene into the open display scene. `frame` is the number of this frame.
    pub unsafe fn draw(&mut self, ctx: *mut g::SceGxmContext, v: &View, frame: u64) -> Stats {
        let mut st = Stats::default();
        g::sceGxmSetViewport(ctx, 480.0, 480.0, 272.0, -272.0, 0.5, 0.5);
        self.ring.next_frame();
        self.draw_sky(ctx, v);

        // Visible chunks, nearest first.
        let planes = frustum(&v.vp);
        self.order.clear();
        for (at, c) in self.chunks.iter() {
            let min = (*at * 16).as_vec3() - v.eye;
            let d = (min + 8.0).abs().max_element();
            if d - 8.0 > v.reach {
                continue;
            }
            if !visible(&planes, min, min + 16.0) {
                st.culled += 1;
                continue;
            }
            let _ = c;
            self.order.push(((min + 8.0).length_squared(), *at));
        }
        self.order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        st.chunks = self.order.len() as u32;

        // Cg reads a float4x4 uniform row by row (`mul(M, v)` dots each row with v); glam stores columns.
        let vp = v.vp.transpose().to_cols_array();
        // Opaque terrain.
        gpu::state_opaque(ctx, Cull::Back);
        self.terrain.bind(ctx, 0);
        for i in 0..self.order.len() {
            let at = self.order[i].1;
            let c = self.chunks.get_mut(&at).unwrap();
            c.drawn = frame;
            st.tris += c.tris;
            let origin = (at * 16).as_vec3() - v.eye;
            for b in &c.batches {
                if b.opaque.1 == 0 {
                    continue;
                }
                let u = Uniforms::vertex(ctx);
                u.set(self.tp.vp, &vp);
                u.set(self.tp.origin, &[origin.x, origin.y, origin.z, 0.0]);
                u.set(self.tp.frame, &v.frame);
                let f = Uniforms::fragment(ctx);
                let col = tile_colour(b.material);
                f.set(self.tp.look, &[col[0], col[1], col[2], 1.0, v.fog[0], v.fog[1], v.fog[2], 0.0]);
                let vb = self.heap.ptr(c.range, c.terrain_vb + b.base as usize * TERRAIN_STRIDE as usize);
                let ib = self.heap.ptr(c.range, c.terrain_ib + b.opaque.0 as usize * 2).cast();
                gpu::draw(ctx, vb, ib, b.opaque.1);
                st.draws += 1;
            }
        }
        // Where materials meet: each other material blended over by its weight.
        gpu::state_blend(ctx, Cull::Back);
        self.terrain.bind(ctx, 1);
        for i in 0..self.order.len() {
            let at = self.order[i].1;
            let c = &self.chunks[&at];
            let origin = (at * 16).as_vec3() - v.eye;
            for b in &c.batches {
                if b.layer.1 == 0 {
                    continue;
                }
                let u = Uniforms::vertex(ctx);
                u.set(self.tp.vp, &vp);
                u.set(self.tp.origin, &[origin.x, origin.y, origin.z, 0.0]);
                u.set(self.tp.frame, &v.frame);
                let f = Uniforms::fragment(ctx);
                let col = tile_colour(b.material);
                f.set(self.tp.look, &[col[0], col[1], col[2], 1.0, v.fog[0], v.fog[1], v.fog[2], 0.0]);
                let vb = self.heap.ptr(c.range, c.terrain_vb + b.base as usize * TERRAIN_STRIDE as usize);
                let ib = self.heap.ptr(c.range, c.terrain_ib + b.layer.0 as usize * 2).cast();
                gpu::draw(ctx, vb, ib, b.layer.1);
                st.draws += 1;
            }
        }
        // Water, far to near so its faces blend over one another in order.
        self.water.bind(ctx, 0);
        for i in (0..self.order.len()).rev() {
            let at = self.order[i].1;
            let c = &self.chunks[&at];
            if c.water.count == 0 {
                continue;
            }
            let origin = (at * 16).as_vec3() - v.eye;
            let u = Uniforms::vertex(ctx);
            u.set(self.wp.vp, &vp);
            u.set(self.wp.origin, &[origin.x, origin.y, origin.z, 0.0]);
            u.set(self.wp.frame, &v.frame);
            let f = Uniforms::fragment(ctx);
            f.set(self.wp.look, &[0.05, 0.22, 0.26, 0.78, v.fog[0], v.fog[1], v.fog[2], 0.0, v.sky_horizon[0], v.sky_horizon[1], v.sky_horizon[2], 0.0]);
            gpu::draw(ctx, self.heap.ptr(c.range, c.water.vb), self.heap.ptr(c.range, c.water.ib).cast(), c.water.count);
            st.draws += 1;
        }
        st
    }

    unsafe fn draw_sky(&mut self, ctx: *mut g::SceGxmContext, v: &View) {
        let inv = v.vp.inverse();
        let Some(p) = self.ring.alloc(4 * 20 + 8, 16) else { return };
        let verts = p as *mut f32;
        let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)];
        for (i, (x, y)) in corners.iter().enumerate() {
            let far = inv * Vec4::new(*x, *y, 1.0, 1.0);
            let near = inv * Vec4::new(*x, *y, -1.0, 1.0);
            let d = (far.truncate() / far.w - near.truncate() / near.w).normalize();
            let o = verts.add(i * 5);
            *o = *x;
            *o.add(1) = *y;
            *o.add(2) = d.x;
            *o.add(3) = d.y;
            *o.add(4) = d.z;
        }
        let idx = verts.add(20) as *mut u16;
        for (i, k) in [0u16, 1, 2, 3].iter().enumerate() {
            *idx.add(i) = *k;
        }
        gpu::state_overlay(ctx);
        self.sky.bind(ctx, 0);
        let f = Uniforms::fragment(ctx);
        let s = v.sun.max(0.0);
        f.set(self.sky_param, &[v.sky_zenith[0], v.sky_zenith[1], v.sky_zenith[2], 0.0, v.sky_horizon[0], v.sky_horizon[1], v.sky_horizon[2], 0.0, v.sun_dir.x, v.sun_dir.y, v.sun_dir.z, s, 1.0, 0.85, 0.6, 0.0]);
        g::sceGxmSetVertexStream(ctx, 0, verts.cast());
        g::sceGxmDraw(ctx, g::SceGxmPrimitiveType_SCE_GXM_PRIMITIVE_TRIANGLE_STRIP, g::SceGxmIndexFormat_SCE_GXM_INDEX_FORMAT_U16, idx.cast(), 4);
    }

    /// Bytes of chunk meshes on the GPU.
    pub fn bytes(&self) -> usize {
        self.heap.used
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
        // The box corner farthest along the plane's normal.
        let c = Vec3::new(if p.x >= 0.0 { max.x } else { min.x }, if p.y >= 0.0 { max.y } else { min.y }, if p.z >= 0.0 { max.z } else { min.z });
        p.x * c.x + p.y * c.y + p.z * c.z + p.w >= 0.0
    })
}

#[allow(dead_code)]
fn _foliage_vertex_size() -> usize {
    core::mem::size_of::<FoliageVertex>()
}

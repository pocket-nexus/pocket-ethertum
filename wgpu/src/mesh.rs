//! A chunk's meshes on this GPU, from the `ChunkMesh` the world's meshing hands over (`ether_voxel::mesh`).
//!
//! The device vertices are laid out for GXM, which reads an attribute at any byte. WebGPU reads one at a multiple
//! of four bytes, so the terrain's 14-byte, the foliage's 16-byte and the water's 12-byte vertices are laid out
//! again here, field for field; the casters' 8 bytes are taken as they are.
//!
//! | Mesh | Here | Fields |
//! | --- | --- | --- |
//! | terrain | 16 bytes | position `sint16x4` (w 0), normal `snorm8x4`, occlusion, weight and the two blends `unorm8x4` |
//! | foliage | 20 bytes | position `sint16x4`, atlas coordinates `unorm16x2`, occlusion `unorm8x4`, the way to the voxel's centre `snorm8x4` |
//! | water | 12 bytes | position `sint16x4`, normal `snorm8x4` |
//! | casters | 8 bytes | position `sint16x4` (w the vertex's padding) |
//!
//! Each chunk has a slot in a table the programs read by the draw's instance (`ChunkInfo`): its corner, the
//! terrain's texture coordinates there, and the water's ripples there.

use ether_voxel::glam::{IVec3, Vec3};
use ether_voxel::mesh::{Batch, ChunkMesh};
use pocket_web_wgpu::gpu::Gpu;
use pocket_web_wgpu::wgpu::{self, util::DeviceExt, Buffer};

pub const TERRAIN_STRIDE: u64 = 16;
pub const FOLIAGE_STRIDE: u64 = 20;
pub const WATER_STRIDE: u64 = 12;
pub const CASTER_STRIDE: u64 = 8;

/// The reference's triplanar sample scale: a texture repeats every 1.5 voxels.
pub const SAMPLE_SCALE: f32 = 1.5;

/// A chunk's row of the table (`Chunk` in common.wgsl), 64 bytes.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkInfo {
    /// The chunk's corner, in voxels.
    pub corner: [f32; 4],
    /// The terrain's texture coordinates at the corner, wrapped (three voxels hold two whole repeats), and one over
    /// the sample scale.
    pub uv: [f32; 4],
    /// The corner over 100, wrapped where the water's first three octaves repeat together (2 100 voxels).
    pub ripple: [f32; 4],
    pub spare: [f32; 4],
}

impl ChunkInfo {
    pub fn of(at: IVec3) -> ChunkInfo {
        let c = at * 16;
        let uv = |x: i32| (x.rem_euclid(3 * 256)) as f32 / SAMPLE_SCALE;
        let ripple = |x: i32| (x.rem_euclid(2100)) as f32 * 0.01;
        ChunkInfo { corner: [c.x as f32, c.y as f32, c.z as f32, 0.0], uv: [uv(c.x), uv(c.y), uv(c.z), 1.0 / SAMPLE_SCALE], ripple: [ripple(c.x), 0.0, ripple(c.z), 0.0], spare: [0.0; 4] }
    }
}

pub struct Part {
    pub vb: Buffer,
    pub ib: Buffer,
    /// Indices.
    pub count: u32,
}

pub struct ChunkGpu {
    pub at: IVec3,
    /// Its row of the table.
    pub slot: u32,
    pub terrain: Option<Part>,
    pub batches: Vec<Batch>,
    pub foliage: Option<Part>,
    /// The foliage's indices sorted eight ways (`ChunkMesh::foliage_sorted`), each list `foliage.count` long.
    pub foliage_sorted: Option<Buffer>,
    pub water: Option<Part>,
    /// The water's bounds in the chunk, in voxels from its corner.
    pub water_box: (Vec3, Vec3),
    pub caster: Option<Part>,
    pub bytes: u64,
}

fn indices(gpu: &Gpu, label: &str, list: &[u16]) -> Buffer {
    // (a buffer's size is a multiple of four bytes)
    let mut bytes: Vec<u8> = bytemuck::cast_slice(list).to_vec();
    bytes.resize(bytes.len().div_ceil(4) * 4, 0);
    gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: &bytes, usage: wgpu::BufferUsages::INDEX })
}

fn vertices(gpu: &Gpu, label: &str, bytes: &[u8]) -> Buffer {
    gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage: wgpu::BufferUsages::VERTEX })
}

fn pos(out: &mut Vec<u8>, p: [i16; 3], w: i16) {
    for v in [p[0], p[1], p[2], w] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

impl ChunkGpu {
    /// The chunk's meshes on the GPU, in row `slot` of the table.
    pub fn upload(gpu: &Gpu, at: IVec3, slot: u32, mesh: &ChunkMesh) -> ChunkGpu {
        let part = |label: &str, vb: Vec<u8>, list: &[u16]| -> Option<Part> {
            if list.is_empty() {
                return None;
            }
            Some(Part { vb: vertices(gpu, label, &vb), ib: indices(gpu, label, list), count: list.len() as u32 })
        };
        let t = &mesh.terrain;
        let mut vb = Vec::with_capacity(t.vertices.len() * TERRAIN_STRIDE as usize);
        for v in &t.vertices {
            pos(&mut vb, v.pos, 0);
            vb.extend_from_slice(&[v.normal[0] as u8, v.normal[1] as u8, v.normal[2] as u8, 0, v.ao, v.weight, v.blend[0], v.blend[1]]);
        }
        let terrain = part("terrain", vb, &t.indices);
        let f = &mesh.foliage;
        let mut vb = Vec::with_capacity(f.vertices.len() * FOLIAGE_STRIDE as usize);
        for v in &f.vertices {
            pos(&mut vb, v.pos, 0);
            vb.extend_from_slice(&v.uv[0].to_le_bytes());
            vb.extend_from_slice(&v.uv[1].to_le_bytes());
            vb.extend_from_slice(&[v.ao, 0, 0, 0, v.centre[0] as u8, v.centre[1] as u8, v.centre[2] as u8, 0]);
        }
        let foliage = part("foliage", vb, &f.indices);
        let foliage_sorted = (!mesh.foliage_sorted.is_empty() && foliage.is_some()).then(|| indices(gpu, "foliage sorted", &mesh.foliage_sorted));
        let w = &mesh.liquid;
        let mut vb = Vec::with_capacity(w.vertices.len() * WATER_STRIDE as usize);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &w.vertices {
            pos(&mut vb, v.pos, 0);
            vb.extend_from_slice(&[v.normal[0] as u8, v.normal[1] as u8, v.normal[2] as u8, 0]);
            let p = Vec3::new(v.pos[0] as f32, v.pos[1] as f32, v.pos[2] as f32) / 256.0;
            (lo, hi) = (lo.min(p), hi.max(p));
        }
        let water = part("water", vb, &w.indices);
        let c = &mesh.caster;
        let mut vb = Vec::with_capacity(c.vertices.len() * CASTER_STRIDE as usize);
        for v in &c.vertices {
            pos(&mut vb, v.pos, v._pad);
        }
        let caster = part("casters", vb, &c.indices);
        let size = |p: &Option<Part>| p.as_ref().map_or(0, |p| p.vb.size() + p.ib.size());
        let bytes = size(&terrain) + size(&foliage) + size(&water) + size(&caster) + foliage_sorted.as_ref().map_or(0, |b| b.size());
        ChunkGpu { at, slot, terrain, batches: t.batches.clone(), foliage, foliage_sorted, water, water_box: (lo, hi), caster, bytes }
    }

    pub fn is_empty(&self) -> bool {
        self.terrain.is_none() && self.foliage.is_none() && self.water.is_none() && self.caster.is_none()
    }
}

//! Meshing: surface nets for the isosurface, faces for cubes and water, crossed
//! cards for foliage.
//!
//! Two layers:
//!
//! - **The reference stream.** [`terrain`], [`foliage`] and [`liquid`] call
//!   back once per vertex, in the order Ethertum's `meshgen` pushes vertices,
//!   with the same position, `uv` and normal bits. Surface nets computes a cell's
//!   feature point, gradient and material once and reuses them for every quad
//!   that touches the cell (the reference recomputes them per quad corner); the
//!   values are the same because the operations are. `ether-oracle` compares
//!   these streams with the reference's own `meshgen`.
//! - **The device mesh.** [`build`] turns the streams into indexed meshes with
//!   16-bit positions and per-vertex occlusion, the terrain split into batches
//!   by material (see [`TerrainMesh`]).
//!
//! Every function reads a [`Pad`]: the chunk and a two-voxel border copied from
//! its neighbours, so meshing runs on any thread without touching the world.

use alloc::vec::Vec;
use core::f32::consts::PI;
use glam::{ivec3, vec2, vec3, IVec3, Quat, Vec2, Vec3};

use crate::chunk::{idx, Hood, LEN};
use crate::vox::{tex, Shape, Vox};

/// Voxels the pad keeps on each side of the chunk.
pub const BORDER: i32 = 2;
/// Voxels along one axis of the pad.
pub const SIDE: i32 = LEN + 2 * BORDER;
pub const PAD_LEN: usize = (SIDE * SIDE * SIDE) as usize;

/// A chunk with a border of its neighbours' voxels, for meshing.
pub struct Pad {
    pub vox: [Vox; PAD_LEN],
    /// Which of the 27 chunks were present when the pad was filled.
    pub present: u32,
}

#[inline(always)]
fn pidx(p: IVec3) -> usize {
    ((p.x + BORDER) * SIDE * SIDE + (p.y + BORDER) * SIDE + (p.z + BORDER)) as usize
}

impl Pad {
    pub fn new() -> Pad {
        Pad { vox: [Vox::AIR; PAD_LEN], present: 0 }
    }

    /// Copies the centre chunk of `hood` and its border. A missing neighbour leaves empty voxels.
    pub fn fill(&mut self, hood: &Hood) {
        self.fill_from(|slot| hood.chunks[slot].as_deref());
    }

    /// Copies the chunk and its border from the 27 chunks `chunk(slot)` gives (slots as [`crate::chunk::hood_slot`]).
    pub fn fill_from<'a>(&mut self, chunk: impl Fn(usize) -> Option<&'a crate::chunk::Voxels>) {
        let chunks: [Option<&crate::chunk::Voxels>; 27] = core::array::from_fn(&chunk);
        self.present = 0;
        for (slot, c) in chunks.iter().enumerate() {
            if c.is_some() {
                self.present |= 1 << slot;
            }
        }
        for x in -BORDER..LEN + BORDER {
            for y in -BORDER..LEN + BORDER {
                for z in -BORDER..LEN + BORDER {
                    let d = ivec3(x >> 4, y >> 4, z >> 4);
                    let v = chunks[crate::chunk::hood_slot(d)].map(|c| c[idx(x & 15, y & 15, z & 15)]).unwrap_or_default();
                    self.vox[pidx(ivec3(x, y, z))] = v;
                }
            }
        }
    }

    /// The voxel at a position relative to the chunk's corner, in `-2..18`.
    #[inline(always)]
    pub fn at(&self, p: IVec3) -> Vox {
        self.vox[pidx(p)]
    }

    #[inline(always)]
    fn iso(&self, p: IVec3) -> f32 {
        self.vox[pidx(p)].isovalue()
    }

    /// Whether all 27 chunks were present.
    pub fn complete(&self) -> bool {
        self.present == (1 << 27) - 1
    }

    /// No vertex can come out: nothing but one material, no cube, no foliage, no water.
    pub fn is_blank(&self) -> bool {
        let first = self.vox[0];
        let sign = first.isovalue() > 0.0;
        self.vox.iter().all(|v| v.shape == Shape::Isosurface && v.tex != tex::WATER && (v.isovalue() > 0.0) == sign && (v.tex == tex::NIL) == first.is_nil())
    }
}

impl Default for Pad {
    fn default() -> Self {
        Pad::new()
    }
}

/// A vertex as the reference pushes it: position in the chunk, `uv` (material and light for the terrain, atlas
/// coordinates for foliage) and normal.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RefVertex {
    pub pos: Vec3,
    pub uv: Vec2,
    pub norm: Vec3,
}

const AXES: [IVec3; 3] = [ivec3(1, 0, 0), ivec3(0, 1, 0), ivec3(0, 0, 1)];
const ADJACENT: [[IVec3; 6]; 3] = [
    [ivec3(0, 0, 0), ivec3(0, -1, 0), ivec3(0, -1, -1), ivec3(0, -1, -1), ivec3(0, 0, -1), ivec3(0, 0, 0)],
    [ivec3(0, 0, 0), ivec3(0, 0, -1), ivec3(-1, 0, -1), ivec3(-1, 0, -1), ivec3(-1, 0, 0), ivec3(0, 0, 0)],
    [ivec3(0, 0, 0), ivec3(-1, 0, 0), ivec3(-1, -1, 0), ivec3(-1, -1, 0), ivec3(0, -1, 0), ivec3(0, 0, 0)],
];
const VERT: [IVec3; 8] = [ivec3(0, 0, 0), ivec3(0, 0, 1), ivec3(0, 1, 0), ivec3(0, 1, 1), ivec3(1, 0, 0), ivec3(1, 0, 1), ivec3(1, 1, 0), ivec3(1, 1, 1)];
const EDGE: [[usize; 2]; 12] = [[0, 4], [1, 5], [2, 6], [3, 7], [5, 7], [1, 3], [4, 6], [0, 2], [4, 5], [0, 1], [6, 7], [2, 3]];

/// Cells whose values a chunk's quads can use: `-1..16` on each axis.
const CELLS: i32 = LEN + 1;

#[derive(Clone, Copy)]
struct Cell {
    fp: Vec3,
    norm: Vec3,
    tex: u8,
    lit: u8,
    done: bool,
}

/// Per-cell values computed once per mesh.
pub struct CellCache {
    cells: Vec<Cell>,
}

impl CellCache {
    pub fn new() -> CellCache {
        CellCache { cells: alloc::vec![Cell { fp: Vec3::ZERO, norm: Vec3::ZERO, tex: 0, lit: 0, done: false }; (CELLS * CELLS * CELLS) as usize] }
    }

    fn clear(&mut self) {
        for c in self.cells.iter_mut() {
            c.done = false;
        }
    }
}

impl Default for CellCache {
    fn default() -> Self {
        CellCache::new()
    }
}

#[inline(always)]
fn cell_id(p: IVec3) -> u32 {
    ((p.x + 1) * CELLS * CELLS + (p.y + 1) * CELLS + (p.z + 1)) as u32
}

#[inline(always)]
fn sign_changed(a: f32, b: f32) -> bool {
    (a > 0.) != (b > 0.)
}

/// egui's `inverse_lerp`, which the reference uses for the crossing on an edge.
#[inline(always)]
fn inverse_lerp(min: f32, max: f32, value: f32) -> Option<f32> {
    if min == max {
        None
    } else {
        Some((value - min) / (max - min))
    }
}

fn feature_point(pad: &Pad, p: IVec3) -> Vec3 {
    let mut sign_changes = 0;
    let mut fp_sum = Vec3::ZERO;
    for edge in EDGE {
        let v0 = VERT[edge[0]];
        let v1 = VERT[edge[1]];
        let c0 = pad.iso(p + v0);
        let c1 = pad.iso(p + v1);
        if sign_changed(c0, c1) {
            if let Some(t) = inverse_lerp(c0, c1, 0.0) {
                let q = t * (v1 - v0).as_vec3() + v0.as_vec3();
                fp_sum += q;
                sign_changes += 1;
            }
        }
    }
    if sign_changes == 0 {
        return Vec3::ONE * 0.5;
    }
    fp_sum / (sign_changes as f32)
}

fn gradient(pad: &Pad, p: IVec3) -> Vec3 {
    let val = pad.iso(p);
    vec3(pad.iso(p + IVec3::X) - val, pad.iso(p + IVec3::Y) - val, pad.iso(p + IVec3::Z) - val).try_normalize().unwrap_or(Vec3::NEG_Y)
}

fn cell<'c>(cache: &'c mut CellCache, pad: &Pad, p: IVec3) -> &'c Cell {
    let id = cell_id(p) as usize;
    if !cache.cells[id].done {
        let fp = feature_point(pad, p);
        let norm = -gradient(pad, p);
        let c = pad.at(p);
        let mut nearest_val = f32::INFINITY;
        let mut nearest_tex = c.tex;
        let mut nearest_lit = 0;
        for vert in VERT {
            let c = pad.at(p + vert);
            if !c.is_isoval_empty() && c.isovalue() < nearest_val {
                nearest_val = c.isovalue();
                nearest_tex = c.tex;
                nearest_lit = c.block_light();
            }
        }
        cache.cells[id] = Cell { fp, norm, tex: nearest_tex, lit: nearest_lit, done: true };
    }
    &cache.cells[id]
}

/// The terrain stream: surface nets, then cubes. `emit(vertex, key)`: `key` names the vertex's source for
/// indexing (a cell of the net, or a cube corner with its face).
pub fn terrain(pad: &Pad, cache: &mut CellCache, mut emit: impl FnMut(RefVertex, u32)) {
    cache.clear();
    for ly in 0..LEN {
        for lz in 0..LEN {
            for lx in 0..LEN {
                let lp = ivec3(lx, ly, lz);
                let c0 = pad.iso(lp);
                for axis in 0..3 {
                    let c1 = pad.iso(lp + AXES[axis]);
                    if !sign_changed(c0, c1) {
                        continue;
                    }
                    let winding_flip = c0 <= 0.0;
                    for quadvert in 0..6 {
                        let wi = if winding_flip { 5 - quadvert } else { quadvert };
                        let p = lp + ADJACENT[axis][wi];
                        let c = *cell(cache, pad, p);
                        emit(RefVertex { pos: p.as_vec3() + c.fp + 0.5, uv: vec2(c.tex as f32, c.lit as f32), norm: c.norm }, cell_id(p));
                    }
                }
            }
        }
    }
    for ly in 0..LEN {
        for lz in 0..LEN {
            for lx in 0..LEN {
                let lp = ivec3(lx, ly, lz);
                let v = pad.at(lp);
                if !v.is_nil() && v.is_cube() {
                    put_cube(pad, lp, v, &mut emit);
                }
            }
        }
    }
}

/// The foliage stream: crossed cards for leaves and tall grass, with atlas coordinates.
pub fn foliage(pad: &Pad, mut emit: impl FnMut(RefVertex, u32)) {
    for ly in 0..LEN {
        for lz in 0..LEN {
            for lx in 0..LEN {
                let lp = ivec3(lx, ly, lz);
                let c = pad.at(lp);
                if c.tex != 0 {
                    if c.shape == Shape::Leaves {
                        put_leaves(lp.as_vec3(), c.tex, &mut emit);
                    } else if c.shape == Shape::Grass {
                        put_grass(lp.as_vec3(), c.tex, &mut emit);
                    }
                }
            }
        }
    }
}

/// The water stream: faces of water voxels that meet neither water nor an opaque cube.
pub fn liquid(pad: &Pad, mut emit: impl FnMut(RefVertex, u32)) {
    for ly in 0..LEN {
        for lz in 0..LEN {
            for lx in 0..LEN {
                let lp = ivec3(lx, ly, lz);
                let c = pad.at(lp);
                if c.tex == tex::WATER {
                    put_cube(pad, lp, c, &mut emit);
                }
            }
        }
    }
}

#[rustfmt::skip]
static CUBE_POS: [f32; 6 * 6 * 3] = [
    0., 0., 1., 0., 1., 1., 0., 1., 0.,   0., 0., 1., 0., 1., 0., 0., 0., 0., // -X
    1., 0., 0., 1., 1., 0., 1., 1., 1.,   1., 0., 0., 1., 1., 1., 1., 0., 1., // +X
    0., 0., 1., 0., 0., 0., 1., 0., 0.,   0., 0., 1., 1., 0., 0., 1., 0., 1., // -Y
    0., 1., 1., 1., 1., 1., 1., 1., 0.,   0., 1., 1., 1., 1., 0., 0., 1., 0., // +Y
    0., 0., 0., 0., 1., 0., 1., 1., 0.,   0., 0., 0., 1., 1., 0., 1., 0., 0., // -Z
    1., 0., 1., 1., 1., 1., 0., 1., 1.,   1., 0., 1., 0., 1., 1., 0., 0., 1., // +Z
];

#[rustfmt::skip]
static CUBE_UV: [f32; 6 * 2] = [1., 1., 1., 0., 0., 0., 1., 1., 0., 0., 0., 1.];

static FACE_DIR: [IVec3; 6] = [ivec3(-1, 0, 0), ivec3(1, 0, 0), ivec3(0, -1, 0), ivec3(0, 1, 0), ivec3(0, 0, -1), ivec3(0, 0, 1)];

fn put_cube(pad: &Pad, lp: IVec3, vox: Vox, emit: &mut impl FnMut(RefVertex, u32)) {
    for (face, dir) in FACE_DIR.iter().enumerate() {
        let neib = pad.at(lp + *dir);
        if neib.is_opaque_cube() {
            continue;
        }
        if vox.tex == tex::WATER && neib.tex == tex::WATER {
            continue;
        }
        let norm = dir.as_vec3();
        for vert in 0..6 {
            let at = face * 18 + vert * 3;
            let corner = Vec3::from_slice(&CUBE_POS[at..at + 3]);
            let key = 0x8000_0000 | ((idx(lp.x, lp.y, lp.z) as u32) << 8) | ((face as u32) << 3) | corner_bits(corner);
            emit(RefVertex { pos: corner + lp.as_vec3(), uv: vec2(vox.tex as f32, neib.block_light() as f32), norm }, key);
        }
    }
}

#[inline(always)]
fn corner_bits(c: Vec3) -> u32 {
    (c.x as u32) << 2 | (c.y as u32) << 1 | c.z as u32
}

/// The reference's `VoxTex::map_uv`: a tile of the 24-wide atlas.
#[inline(always)]
fn map_uv(uv: Vec2, tex_id: u8) -> Vec2 {
    const TEX_CAP: f32 = 24.;
    let t = tex_id as u16 - 1;
    Vec2::new(uv.x / TEX_CAP + t as f32 / TEX_CAP, uv.y)
}

/// One card through the middle of a voxel: the -X face of a unit cube, scaled, turned and moved to `pos`.
fn put_face(tex_id: u8, pos: Vec3, rot: Quat, scale: Vec2, card: u32, emit: &mut impl FnMut(RefVertex, u32)) {
    for i in 0..6 {
        let p = Vec3::from_slice(&CUBE_POS[i * 3..i * 3 + 3]) - vec3(0.0, 0.5, 0.5);
        let p = (rot * (p * vec3(1.0, scale.y, scale.x))) + pos;
        let n = rot * Vec3::NEG_X;
        let uv = map_uv(Vec2::from_slice(&CUBE_UV[i * 2..i * 2 + 2]), tex_id);
        let corner = Vec3::from_slice(&CUBE_POS[i * 3..i * 3 + 3]);
        emit(RefVertex { pos: p, uv, norm: n }, card << 2 | (corner.y as u32) << 1 | corner.z as u32);
    }
}

fn card_base(pos: Vec3) -> u32 {
    let p = pos.as_ivec3();
    (idx(p.x, p.y, p.z) as u32) << 3
}

fn put_leaves(pos: Vec3, tex_id: u8, emit: &mut impl FnMut(RefVertex, u32)) {
    let deg45 = PI / 4.0;
    let siz = 1.4;
    let b = card_base(pos);
    put_face(tex_id, pos + 0.5, Quat::from_axis_angle(Vec3::Y, deg45), vec2(1.4, 1.0) * siz, b, emit);
    put_face(tex_id, pos + 0.5, Quat::from_axis_angle(Vec3::Y, deg45 * 3.0), vec2(1.4, 1.0) * siz, b + 1, emit);
    put_face(tex_id, pos + 0.5, Quat::from_axis_angle(Vec3::Z, -deg45), vec2(1.0, 1.4) * siz, b + 2, emit);
    put_face(tex_id, pos + 0.5, Quat::from_axis_angle(Vec3::Z, -deg45 * 3.0), vec2(1.0, 1.4) * siz, b + 3, emit);
}

fn put_grass(pos: Vec3, tex_id: u8, emit: &mut impl FnMut(RefVertex, u32)) {
    let ang = PI / 3.0;
    let siz = 1.4;
    let b = card_base(pos);
    put_face(tex_id, pos + 0.5, Quat::from_axis_angle(Vec3::Y, ang), Vec2::ONE * siz, b, emit);
    put_face(tex_id, pos + 0.5, Quat::from_axis_angle(Vec3::Y, ang * 2.), Vec2::ONE * siz, b + 1, emit);
    put_face(tex_id, pos + 0.5, Quat::from_axis_angle(Vec3::Y, ang * 3.), Vec2::ONE * siz, b + 2, emit);
}

// ---------------------------------------------------------------------------------------------- device meshes

/// Fixed-point position: 1/256 voxel.
pub const POS_SCALE: f32 = 256.0;

/// A terrain vertex: position (1/256 voxel, from the chunk's corner), normal, ambient occlusion, and the weight
/// of its batch's material at this vertex (255 where the vertex is of that material, else 0). 12 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainVertex {
    pub pos: [i16; 3],
    pub normal: [i8; 3],
    pub ao: u8,
    pub weight: u8,
    pub _pad: [u8; 2],
}

/// A foliage vertex: position, coordinates in the device's foliage atlas (1/65535 of it), occlusion. 12 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FoliageVertex {
    pub pos: [i16; 3],
    pub uv: [u16; 2],
    pub ao: u8,
    pub _pad: u8,
}

/// A water vertex: position and normal. 10 bytes, padded to 12.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiquidVertex {
    pub pos: [i16; 3],
    pub normal: [i8; 3],
    pub _pad: [u8; 3],
}

/// The triangles of one material in a chunk's terrain. `opaque` are the triangles whose first material this is,
/// drawn with depth writes; `layer` are triangles that also touch this material, blended over the opaque pass
/// with the vertices' weights, so materials meet in a gradient across a triangle as the reference's per-fragment
/// blend does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub material: u8,
    /// First index and count, into `TerrainMesh::indices`.
    pub opaque: (u32, u32),
    pub layer: (u32, u32),
    /// First vertex of the batch; its indices are relative to it.
    pub base: u32,
}

#[derive(Clone, Debug, Default)]
pub struct TerrainMesh {
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u16>,
    pub batches: Vec<Batch>,
}

#[derive(Clone, Debug, Default)]
pub struct Indexed<V> {
    pub vertices: Vec<V>,
    pub indices: Vec<u16>,
}

/// Everything a device draws and collides with for one chunk.
#[derive(Clone, Debug, Default)]
pub struct ChunkMesh {
    pub terrain: TerrainMesh,
    pub foliage: Indexed<FoliageVertex>,
    pub liquid: Indexed<LiquidVertex>,
    /// The terrain's triangles in chunk coordinates, as the reference's collider takes them: every triangle of
    /// the stream, including those a renderer leaves out.
    pub collision: Vec<[Vec3; 3]>,
}

impl ChunkMesh {
    pub fn is_empty(&self) -> bool {
        self.terrain.indices.is_empty() && self.foliage.indices.is_empty() && self.liquid.indices.is_empty() && self.collision.is_empty()
    }

    pub fn triangles(&self) -> usize {
        (self.terrain.indices.len() + self.foliage.indices.len() + self.liquid.indices.len()) / 3
    }
}

#[inline(always)]
fn fixed(p: Vec3) -> [i16; 3] {
    let f = |v: f32| libm::roundf(v * POS_SCALE).clamp(-32768.0, 32767.0) as i16;
    [f(p.x), f(p.y), f(p.z)]
}

#[inline(always)]
fn snorm(n: Vec3) -> [i8; 3] {
    let f = |v: f32| libm::roundf(v.clamp(-1.0, 1.0) * 127.0) as i8;
    [f(n.x), f(n.y), f(n.z)]
}

/// Columns and rows of 128² tiles in the device's foliage atlas (`ether-pack`'s `FOLI`: 1024 × 512, tile `t` at
/// column `t % 8`, row `t / 8`).
pub const FOLIAGE_COLUMNS: u32 = 8;
pub const FOLIAGE_ROWS: u32 = 4;

/// A foliage vertex's coordinates in the device atlas. The reference addresses a strip of 24 tiles
/// (`u = (f + t) / 24`); the tile is the card's voxel's material (the voxel index is in the vertex key) and `f`,
/// the side of the card, is what `u` has beyond it.
fn atlas_uv(pad: &Pad, key: u32, uv: Vec2) -> (f32, f32) {
    let voxel = (key >> 5) as i32;
    let tex = pad.at(ivec3((voxel >> 8) & 15, (voxel >> 4) & 15, voxel & 15)).tex.max(1) as u32;
    let t = tex - 1;
    let f = libm::roundf(uv.x * 24.0 - t as f32).clamp(0.0, 1.0);
    ((((t % FOLIAGE_COLUMNS) as f32) + f) / FOLIAGE_COLUMNS as f32, (((t / FOLIAGE_COLUMNS) as f32) + uv.y) / FOLIAGE_ROWS as f32)
}

/// Occlusion at a point: the share of non-empty voxels in the 4×4×4 block around it, as 255 (open) to 0.
fn occlusion(pad: &Pad, at: Vec3) -> u8 {
    let c = (at - 0.5).floor().as_ivec3();
    let mut solid = 0;
    for x in -1..3 {
        for y in -1..3 {
            for z in -1..3 {
                let p = (c + ivec3(x, y, z)).clamp(IVec3::splat(-BORDER), IVec3::splat(LEN + BORDER - 1));
                let v = pad.at(p);
                if v.isovalue() > 0.0 || v.is_opaque_cube() {
                    solid += 1;
                }
            }
        }
    }
    // Half the block is solid on an open plane: that is full light.
    let open = 1.0 - ((solid as f32 / 64.0) - 0.5).max(0.0) * 2.0;
    (open.clamp(0.0, 1.0) * 255.0) as u8
}

/// Vertex keys to indices: open addressing, cleared in O(1) by a generation count, doubled when half full.
struct KeyMap {
    slots: Vec<(u64, u16, u32)>,
    gen: u32,
    len: usize,
}

impl KeyMap {
    fn new() -> KeyMap {
        KeyMap { slots: alloc::vec![(0, 0, 0); 1 << 12], gen: 1, len: 0 }
    }

    fn clear(&mut self) {
        self.len = 0;
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            self.slots.iter_mut().for_each(|s| s.2 = 0);
            self.gen = 1;
        }
    }

    #[inline(always)]
    fn home(&self, key: u64) -> usize {
        let bits = self.slots.len().trailing_zeros();
        (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - bits)) as usize
    }

    fn grow(&mut self) {
        let old = core::mem::replace(&mut self.slots, alloc::vec![(0, 0, 0); 0]);
        self.slots = alloc::vec![(0, 0, 0); old.len() * 2];
        let mask = self.slots.len() - 1;
        for s in old.into_iter().filter(|s| s.2 == self.gen) {
            let mut h = self.home(s.0);
            while self.slots[h].2 == self.gen {
                h = (h + 1) & mask;
            }
            self.slots[h] = s;
        }
    }

    /// The index stored for `key`, or `make()` stored and returned.
    #[inline]
    fn get_or(&mut self, key: u64, make: impl FnOnce() -> u16) -> u16 {
        if self.len * 2 >= self.slots.len() {
            self.grow();
        }
        let mask = self.slots.len() - 1;
        let mut h = self.home(key);
        loop {
            let s = &mut self.slots[h];
            if s.2 != self.gen {
                let v = make();
                *s = (key, v, self.gen);
                self.len += 1;
                return v;
            }
            if s.0 == key {
                return s.1;
            }
            h = (h + 1) & mask;
        }
    }
}

/// Scratch reused between meshes on one thread.
pub struct Builder {
    pub cache: CellCache,
    stream: Vec<(RefVertex, u32)>,
    keys: KeyMap,
}

impl Default for Builder {
    fn default() -> Self {
        Builder { cache: CellCache::new(), stream: Vec::new(), keys: KeyMap::new() }
    }
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }

    /// The device meshes of the pad's chunk.
    pub fn build(&mut self, pad: &Pad) -> ChunkMesh {
        let mut out = ChunkMesh::default();
        if pad.is_blank() {
            return out;
        }

        // Terrain.
        self.stream.clear();
        let stream = &mut self.stream;
        terrain(pad, &mut self.cache, |v, k| stream.push((v, k)));
        let tris = self.stream.len() / 3;
        out.collision.reserve(tris);
        for t in self.stream.chunks_exact(3) {
            out.collision.push([t[0].0.pos, t[1].0.pos, t[2].0.pos]);
        }
        // Triangles by material: (first material, the set of all three).
        let mut materials: Vec<u8> = Vec::new();
        for t in self.stream.chunks_exact(3) {
            for v in t {
                let m = v.0.uv.x as u8;
                if m != 0 && !materials.contains(&m) {
                    materials.push(m);
                }
            }
        }
        materials.sort_unstable();
        for &m in &materials {
            let base = out.terrain.vertices.len() as u32;
            let mut batch = Batch { material: m, base, ..Default::default() };
            self.keys.clear();
            let mut opaque: Vec<u16> = Vec::new();
            let mut layer: Vec<u16> = Vec::new();
            for t in self.stream.chunks_exact(3) {
                let ms = [t[0].0.uv.x as u8, t[1].0.uv.x as u8, t[2].0.uv.x as u8];
                // The reference's terrain program discards a triangle with an empty material at any corner.
                if ms.contains(&0) || !ms.contains(&m) {
                    continue;
                }
                let first = if ms[1] == ms[2] { ms[1] } else { ms[0] };
                let target = if first == m { &mut opaque } else { &mut layer };
                for (v, key) in t {
                    let verts = &mut out.terrain.vertices;
                    let at = self.keys.get_or(*key as u64, || {
                        let i = (verts.len() as u32 - base) as u16;
                        verts.push(TerrainVertex { pos: fixed(v.pos), normal: snorm(v.norm), ao: occlusion(pad, v.pos), weight: if v.uv.x as u8 == m { 255 } else { 0 }, _pad: [0; 2] });
                        i
                    });
                    target.push(at);
                }
            }
            batch.opaque = (out.terrain.indices.len() as u32, opaque.len() as u32);
            out.terrain.indices.extend_from_slice(&opaque);
            batch.layer = (out.terrain.indices.len() as u32, layer.len() as u32);
            out.terrain.indices.extend_from_slice(&layer);
            out.terrain.batches.push(batch);
        }

        // Foliage: cards share their four corners. A leaf voxel with leaves or solid ground on all six sides is
        // inside a crown, and its cards are left out. Cards are stored in the order of a hash of their voxel, so
        // any prefix of the indices is an even share of the cards (`ChunkMesh::foliage` is drawn thinner with
        // distance by drawing a prefix).
        self.stream.clear();
        let stream = &mut self.stream;
        foliage(pad, |v, k| stream.push((v, k)));
        let buried = |voxel: i32| -> bool {
            let p = ivec3((voxel >> 8) & 15, (voxel >> 4) & 15, voxel & 15);
            if pad.at(p).shape != Shape::Leaves {
                return false;
            }
            FACE_DIR.iter().all(|d| {
                let n = pad.at(p + *d);
                n.shape == Shape::Leaves || n.isovalue() > 0.0 || n.is_opaque_cube()
            })
        };
        let mut cards: Vec<(u32, usize)> = self.stream.chunks_exact(6).enumerate().filter(|(_, c)| !buried((c[0].1 >> 5) as i32)).map(|(i, c)| ((c[0].1 >> 5).wrapping_mul(0x9e37_79b9) ^ (c[0].1 >> 2 & 7), i)).collect();
        cards.sort_unstable();
        let order: Vec<(RefVertex, u32)> = cards.iter().flat_map(|(_, i)| self.stream[i * 6..i * 6 + 6].iter().copied()).collect();
        self.stream = order;
        self.keys.clear();
        for (v, key) in self.stream.iter() {
            let verts = &mut out.foliage.vertices;
            let at = self.keys.get_or(*key as u64, || {
                let i = verts.len() as u16;
                let (u, w) = atlas_uv(pad, *key, v.uv);
                let uv = |f: f32| libm::roundf(f.clamp(0.0, 1.0) * 65535.0) as u16;
                verts.push(FoliageVertex { pos: fixed(v.pos), uv: [uv(u), uv(w)], ao: occlusion(pad, v.pos), _pad: 0 });
                i
            });
            out.foliage.indices.push(at);
        }

        // Water: the faces that meet air (the sea's surface, a pocket dug under it). A face against terrain is left
        // out: against solid terrain it lies under the opaque surface or inside the terrain's own surface, and
        // against terrain whose iso value rounds to 127 (stone a hair inside the surface, which the byte stores as
        // -1/255, empty to the surface nets) it stood out of the sea floor as a dark slab; the reference meshes
        // both.
        self.stream.clear();
        let stream = &mut self.stream;
        liquid(pad, |v, k| stream.push((v, k)));
        let hidden = |key: u32| -> bool {
            let voxel = ((key >> 8) & 0xfff) as i32;
            let face = ((key >> 3) & 7) as usize;
            let p = ivec3((voxel >> 8) & 15, (voxel >> 4) & 15, voxel & 15);
            let n = pad.at(p + FACE_DIR[face]);
            n.isovalue() > 0.0 || (n.tex != tex::NIL && n.shape == Shape::Isosurface)
        };
        let kept: Vec<(RefVertex, u32)> = self.stream.chunks_exact(6).filter(|f| !hidden(f[0].1)).flat_map(|f| f.iter().copied()).collect();
        self.stream = kept;
        self.keys.clear();
        for (v, key) in self.stream.iter() {
            let verts = &mut out.liquid.vertices;
            let at = self.keys.get_or(*key as u64, || {
                let i = verts.len() as u16;
                verts.push(LiquidVertex { pos: fixed(v.pos), normal: snorm(v.norm), _pad: [0; 3] });
                i
            });
            out.liquid.indices.push(at);
        }
        out
    }
}

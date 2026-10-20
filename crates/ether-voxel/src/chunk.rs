//! Chunks of 16³ voxels and the 3×3×3 neighbourhood that population writes
//! into.
//!
//! A chunk is named by its corner in voxels (a multiple of 16 on each axis),
//! as in Ethertum. Voxel `(x, y, z)` of a chunk is at `x << 8 | y << 4 | z`.

use glam::IVec3;

use crate::vox::Vox;

pub const LEN: i32 = 16;
pub const LEN3: usize = 4096;

pub type Voxels = [Vox; LEN3];

#[inline(always)]
pub fn idx(x: i32, y: i32, z: i32) -> usize {
    ((x << 8) | (y << 4) | z) as usize
}

#[inline(always)]
pub fn idx_v(p: IVec3) -> usize {
    idx(p.x, p.y, p.z)
}

/// The chunk a voxel is in, by its corner.
#[inline(always)]
pub fn chunk_of(p: IVec3) -> IVec3 {
    IVec3::new(p.x & !15, p.y & !15, p.z & !15)
}

/// A voxel's place in its chunk.
#[inline(always)]
pub fn local_of(p: IVec3) -> IVec3 {
    IVec3::new(p.x & 15, p.y & 15, p.z & 15)
}

#[inline(always)]
pub fn is_local(p: IVec3) -> bool {
    p.x >= 0 && p.x < 16 && p.y >= 0 && p.y < 16 && p.z >= 0 && p.z < 16
}

/// Slot of a neighbour in a [`Hood`]: `(d.x + 1) + 3 (d.y + 1) + 9 (d.z + 1)`, 13 for the centre.
#[inline(always)]
pub fn hood_slot(d: IVec3) -> usize {
    ((d.x + 1) + 3 * (d.y + 1) + 9 * (d.z + 1)) as usize
}

/// The chunk offset of a slot.
pub fn hood_dir(slot: usize) -> IVec3 {
    let s = slot as i32;
    IVec3::new(s % 3 - 1, (s / 3) % 3 - 1, s / 9 - 1)
}

/// A chunk and its 26 neighbours, borrowed for one population or one copy into a
/// [`crate::mesh::Pad`]. A missing neighbour reads as `None`, as an unloaded
/// chunk does in the reference.
pub struct Hood<'a> {
    pub chunks: [Option<&'a mut Voxels>; 27],
}

impl<'a> Hood<'a> {
    pub fn new() -> Hood<'a> {
        Hood { chunks: Default::default() }
    }

    /// The voxel at `rel` from the centre chunk's corner, if that chunk is here.
    #[inline]
    pub fn get_rel(&self, rel: IVec3) -> Option<Vox> {
        let d = IVec3::new(rel.x >> 4, rel.y >> 4, rel.z >> 4);
        if d.abs().max_element() > 1 {
            return None;
        }
        let c = self.chunks[hood_slot(d)].as_deref()?;
        Some(c[idx_v(local_of(rel))])
    }

    #[inline]
    pub fn get_rel_or_default(&self, rel: IVec3) -> Vox {
        self.get_rel(rel).unwrap_or_default()
    }

    /// Runs `f` on the voxel at `rel` when its chunk is here.
    #[inline]
    pub fn set_rel(&mut self, rel: IVec3, f: impl FnOnce(&mut Vox)) -> Option<Vox> {
        let d = IVec3::new(rel.x >> 4, rel.y >> 4, rel.z >> 4);
        if d.abs().max_element() > 1 {
            return None;
        }
        let c = self.chunks[hood_slot(d)].as_deref_mut()?;
        let v = &mut c[idx_v(local_of(rel))];
        f(v);
        Some(*v)
    }

    /// The centre chunk's voxel at a local position.
    #[inline(always)]
    pub fn at(&mut self, lp: IVec3) -> &mut Vox {
        &mut self.chunks[13].as_deref_mut().expect("a hood has its centre")[idx_v(lp)]
    }
}

impl Default for Hood<'_> {
    fn default() -> Self {
        Hood::new()
    }
}

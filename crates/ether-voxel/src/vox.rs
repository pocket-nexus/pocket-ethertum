//! The voxel: a material, a shape and a signed distance in one `u32`.
//!
//! Ethertum's `Vox` is a 16-bit texture id, a shape, a 16-bit light word and
//! an 8-bit iso value. Its texture ids stop at 24 and its light word is only
//! written by a debug panel, so this one keeps each field in a byte: material,
//! shape, iso value, and a light byte (sky in the high nibble, block light in
//! the low one) for the renderer. The iso value quantization is the
//! reference's own, so the same terrain gives the same bytes.

/// Material ids, as Ethertum numbers them (`VoxTex`). A terrain material `m`
/// is tile `m - 1` of the diffuse atlas; a foliage material is the same tile of
/// the foliage atlas. 0 is empty.
pub mod tex {
    pub const NIL: u8 = 0;
    pub const DIRT: u8 = 1;
    pub const GRASS: u8 = 12;
    pub const LOG: u8 = 13;
    pub const SHORT_GRASS: u8 = 13;
    pub const BUSH: u8 = 14;
    pub const ROSE: u8 = 15;
    pub const FERN: u8 = 16;
    pub const SAND: u8 = 19;
    pub const STONE: u8 = 22;
    pub const LEAVES: u8 = 23;
    pub const WATER: u8 = 24;
    /// Tiles in each atlas.
    pub const TILES: u8 = 24;
}

/// How a voxel is drawn (`VoxShape`). Only the first four are produced or meshed by the reference; the slabs and
/// the fence keep their numbers.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Shape {
    #[default]
    Isosurface = 0,
    Cube = 1,
    Leaves = 2,
    Grass = 3,
    SlabYMin = 4,
    SlabYMax = 5,
    SlabXMin = 6,
    SlabXMax = 7,
    SlabZMin = 8,
    SlabZMax = 9,
    Fence = 10,
}

impl Shape {
    pub fn from_u8(v: u8) -> Shape {
        match v {
            1 => Shape::Cube,
            2 => Shape::Leaves,
            3 => Shape::Grass,
            4 => Shape::SlabYMin,
            5 => Shape::SlabYMax,
            6 => Shape::SlabXMin,
            7 => Shape::SlabXMax,
            8 => Shape::SlabZMin,
            9 => Shape::SlabZMax,
            10 => Shape::Fence,
            _ => Shape::Isosurface,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Vox {
    pub tex: u8,
    pub shape: Shape,
    /// Signed distance quantized to a byte: 0 is -1 (empty), 255 is +1 (solid).
    pub iso: u8,
    /// Sky light (high nibble) and block light (low nibble), 0..=15 each.
    pub light: u8,
}

/// `isoval_u8(0.0)`: the empty voxel's iso value.
pub const ISO_ZERO: u8 = 127;

impl Default for Vox {
    fn default() -> Vox {
        Vox::AIR
    }
}

impl Vox {
    pub const AIR: Vox = Vox { tex: tex::NIL, shape: Shape::Isosurface, iso: ISO_ZERO, light: 0 };

    pub fn new(tex: u8, shape: Shape, isovalue: f32) -> Vox {
        Vox { tex, shape, iso: iso_u8(isovalue), light: 0 }
    }

    #[inline(always)]
    pub fn is_nil(&self) -> bool {
        self.tex == tex::NIL
    }

    #[inline(always)]
    pub fn is_cube(&self) -> bool {
        self.shape == Shape::Cube
    }

    /// The signed distance in `[-1, 1]`; 0 for a voxel that is not part of the isosurface.
    #[inline(always)]
    pub fn isovalue(&self) -> f32 {
        if self.shape != Shape::Isosurface {
            return 0.0;
        }
        iso_f32(self.iso)
    }

    #[inline(always)]
    pub fn set_isovalue(&mut self, v: f32) {
        self.iso = iso_u8(v);
    }

    #[inline(always)]
    pub fn is_isoval_empty(&self) -> bool {
        self.isovalue() <= 0.0
    }

    #[inline(always)]
    pub fn is_opaque_cube(&self) -> bool {
        self.is_cube() && !self.is_nil()
    }

    #[inline(always)]
    pub fn sky(&self) -> u8 {
        self.light >> 4
    }

    #[inline(always)]
    pub fn block_light(&self) -> u8 {
        self.light & 15
    }

    pub fn to_bits(self) -> u32 {
        u32::from_le_bytes([self.tex, self.shape as u8, self.iso, self.light])
    }

    pub fn from_bits(b: u32) -> Vox {
        let [tex, shape, iso, light] = b.to_le_bytes();
        Vox { tex, shape: Shape::from_u8(shape), iso, light }
    }
}

/// Ethertum's `isoval_u8`: `[-1, 1]` onto `0..=255`, truncated.
#[inline(always)]
pub fn iso_u8(f: f32) -> u8 {
    ((f.clamp(-1.0, 1.0) + 1.0) / 2.0 * 255.0) as u8
}

/// Ethertum's `isoval_ndc`.
#[inline(always)]
pub fn iso_f32(u: u8) -> f32 {
    u as f32 / 255.0 * 2.0 - 1.0
}

/// The reference's integer hash onto `[0, 1]` (`util::hash`).
pub fn hash(i: i32) -> f32 {
    let i = (i << 13) ^ i;
    let i = i.wrapping_mul(i).wrapping_mul(15731).wrapping_add(789221).wrapping_mul(i).wrapping_add(1376312589);
    i as u32 as f32 / 0xffff_ffffu32 as f32
}

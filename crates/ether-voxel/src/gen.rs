//! Terrain generation: one chunk from its position alone.
//!
//! The reference's rule (`worldgen::generate_chunk`): a height field from 2D
//! fractal noise, carved and lifted by 3D fractal noise from the same
//! generator,
//!
//! ```text
//! val = fbm(x/130, z/130) - y/18 + 4.5 · fbm(x/90, y/90, z/90)
//! ```
//!
//! stone where `val > 0`, water at `val = -0.1` below sea level, empty above.
//! [`Params`] carries the constants so a mod can change them; at their defaults
//! the bytes are the reference's. The 2D term is computed once per column
//! instead of once per voxel, and the 3D term through [`Fbm::grid3`]: the same
//! numbers for a tenth of the work.

use glam::IVec3;

use crate::chunk::{idx, Voxels, LEN};
use crate::noise::Fbm;
use crate::vox::{tex, Shape, Vox};

/// What shapes the terrain. The defaults are Ethertum's.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    pub seed: u32,
    pub octaves: u32,
    /// Horizontal scale of the height field, in voxels.
    pub terrain_scale: f64,
    /// Scale of the 3D noise.
    pub cave_scale: f64,
    /// Voxels of height per unit of the field.
    pub height_div: f32,
    /// Weight of the 3D noise.
    pub cave_amp: f32,
    /// Water fills empty voxels below this height.
    pub sea_level: i32,
    /// The water voxel's iso value.
    pub water_iso: f32,
    pub stone: u8,
    pub water: u8,
}

impl Default for Params {
    fn default() -> Params {
        Params { seed: 100, octaves: 5, terrain_scale: 130.0, cave_scale: 90.0, height_div: 18.0, cave_amp: 4.5, sea_level: 0, water_iso: -0.1, stone: tex::STONE, water: tex::WATER }
    }
}

pub struct Terrain {
    pub params: Params,
    fbm: Fbm,
}

/// Bytes of scratch a generation needs (the 3D noise of one chunk).
pub type Scratch = [f64; 4096];

impl Terrain {
    pub fn new(params: Params) -> Terrain {
        let fbm = Fbm::ethertum(params.seed, params.octaves as usize);
        Terrain { params, fbm }
    }

    /// Fills `out` with the chunk whose corner is `cp`.
    pub fn generate(&self, cp: IVec3, out: &mut Voxels, scratch: &mut Scratch) {
        let p = &self.params;
        self.fbm.grid3(cp.x, cp.y, cp.z, p.cave_scale, 16, scratch);
        for lx in 0..LEN {
            for lz in 0..LEN {
                let (x, z) = (cp.x + lx, cp.z + lz);
                let f_terr = self.fbm.get2(x as f64 / p.terrain_scale, z as f64 / p.terrain_scale) as f32;
                for ly in 0..LEN {
                    let y = cp.y + ly;
                    let f_3d = scratch[idx(lx, ly, lz)] as f32;
                    out[idx(lx, ly, lz)] = self.voxel(f_terr, f_3d, y);
                }
            }
        }
    }

    /// One voxel from its two noise values, as the reference writes it.
    #[inline(always)]
    pub fn voxel(&self, f_terr: f32, f_3d: f32, y: i32) -> Vox {
        let p = &self.params;
        let mut val = f_terr - (y as f32) / p.height_div + f_3d * p.cave_amp;
        let mut t = tex::NIL;
        if val > 0.0 {
            t = p.stone;
        } else if y < p.sea_level && val < 0.0 {
            val = p.water_iso;
            t = p.water;
        }
        Vox::new(t, Shape::Isosurface, val)
    }

    /// The reference's per-voxel evaluation, for checking [`Terrain::generate`].
    pub fn generate_slow(&self, cp: IVec3, out: &mut Voxels) {
        let p = &self.params;
        for ly in 0..LEN {
            for lz in 0..LEN {
                for lx in 0..LEN {
                    let w = cp + IVec3::new(lx, ly, lz);
                    let f_terr = self.fbm.get2(w.x as f64 / p.terrain_scale, w.z as f64 / p.terrain_scale) as f32;
                    let f_3d = self.fbm.get3(w.x as f64 / p.cave_scale, w.y as f64 / p.cave_scale, w.z as f64 / p.cave_scale) as f32;
                    out[idx(lx, ly, lz)] = self.voxel(f_terr, f_3d, w.y);
                }
            }
        }
    }
}

//! Population: surface materials, tall grass, vines and trees, written once a
//! chunk's 26 neighbours are generated.
//!
//! The rules are the reference's (`worldgen::populate_chunk`), step for step:
//!
//! 1. Down each column, counting voxels since the last empty one (three voxels
//!    of the chunk above start the count): stone within two of an opening below
//!    height 2 becomes sand where 2D noise is above 0.1, the top stone becomes
//!    grass, the two under it dirt.
//! 2. Then per column: one tuft of tall grass, bush, fern or rose on the
//!    lowest grass with air above where noise is positive; a vine hanging from
//!    the lowest stone ceiling in about 7% of columns; a tree on every grass
//!    voxel of about 1.2% of columns. A tree's leaves may land in a neighbour.
//!
//! [`Decor::params`] holds the constants a mod can change.

use glam::{ivec3, IVec3};

use crate::chunk::{Hood, LEN};
use crate::noise::Perm;
use crate::vox::{hash, tex, Shape};

#[derive(Clone, Debug, PartialEq)]
pub struct DecorParams {
    pub seed: u32,
    /// Columns per 256 that hang a vine, and its longest run.
    pub vines_per_256: f32,
    pub vine_max: f32,
    /// Columns per 256 that grow trees.
    pub trees_per_256: f32,
    /// Noise thresholds for tall grass: anything above 0 grows, then bush, fern and rose above these.
    pub bush: f64,
    pub fern: f64,
    pub rose: f64,
    pub sand_below: i32,
    pub sand_noise: f64,
}

impl Default for DecorParams {
    fn default() -> DecorParams {
        DecorParams { seed: 123, vines_per_256: 18.0, vine_max: 12.0, trees_per_256: 3.0, bush: 0.24, fern: 0.8, rose: 0.94, sand_below: 2, sand_noise: 0.1 }
    }
}

pub struct Decor {
    pub params: DecorParams,
    perlin: Perm,
}

impl Decor {
    pub fn new(params: DecorParams) -> Decor {
        let perlin = Perm::new(params.seed);
        Decor { params, perlin }
    }

    /// Populates the centre chunk of `hood`, whose corner is `cp`.
    pub fn populate(&self, hood: &mut Hood, cp: IVec3) {
        let pr = &self.params;
        for lx in 0..LEN {
            for lz in 0..LEN {
                let mut air_dist = 0;
                for i in 0..3 {
                    if !hood.get_rel_or_default(ivec3(lx, LEN + i, lz)).is_nil() {
                        air_dist += 1;
                    }
                }
                for ly in (0..LEN).rev() {
                    let lp = ivec3(lx, ly, lz);
                    let c = hood.at(lp);
                    if c.is_nil() {
                        air_dist = 0;
                    } else {
                        air_dist += 1;
                    }
                    let p = cp + lp;
                    if c.tex == tex::STONE {
                        let mut replace = c.tex;
                        if p.y < pr.sand_below && air_dist <= 2 && self.perlin.perlin2(p.x as f64 / 32., p.z as f64 / 32.) > pr.sand_noise {
                            replace = tex::SAND;
                        } else if air_dist <= 1 {
                            replace = tex::GRASS;
                        } else if air_dist < 3 {
                            replace = tex::DIRT;
                        }
                        c.tex = replace;
                    }
                }
            }
        }

        for lx in 0..LEN {
            for lz in 0..LEN {
                let x = cp.x + lx;
                let z = cp.z + lz;

                let g = self.perlin.perlin2(x as f64 / 18., z as f64 / 18.);
                if g > 0.0 {
                    for ly in 0..LEN - 1 {
                        let lp = ivec3(lx, ly, lz);
                        if hood.at(lp).tex == tex::GRASS && hood.at(lp + IVec3::Y).is_nil() {
                            let c = hood.at(lp + IVec3::Y);
                            c.tex = if g > pr.rose {
                                tex::ROSE
                            } else if g > pr.fern {
                                tex::FERN
                            } else if g > pr.bush {
                                tex::BUSH
                            } else {
                                tex::SHORT_GRASS
                            };
                            c.shape = Shape::Grass;
                            break;
                        }
                    }
                }

                if hash(x ^ z.wrapping_mul(7384)) < (pr.vines_per_256 / 256.0) {
                    for ly in 0..LEN - 1 {
                        let lp = ivec3(lx, ly, lz);
                        if hood.at(lp).is_nil() && hood.at(lp + IVec3::Y).tex == tex::STONE {
                            for i in 0..(pr.vine_max * hash(x ^ z.wrapping_mul(121))) as i32 {
                                let lp = lp + IVec3::NEG_Y * i;
                                if lp.y < 0 {
                                    break;
                                }
                                let c = hood.at(lp);
                                if !c.is_nil() {
                                    break;
                                }
                                c.tex = tex::LEAVES;
                                c.shape = Shape::Leaves;
                            }
                            break;
                        }
                    }
                }

                if hash(x ^ z.wrapping_mul(9572)) < (pr.trees_per_256 / 256.0) {
                    for ly in 0..LEN {
                        let lp = ivec3(lx, ly, lz);
                        if hood.at(lp).tex != tex::GRASS {
                            continue;
                        }
                        let siz = hash(x ^ ly ^ z);
                        tree(hood, lp, siz);
                    }
                }
            }
        }
    }
}

/// A trunk of log rising from `lp` and a ball of leaves over it (`worldgen::gen_tree`).
pub fn tree(hood: &mut Hood, lp: IVec3, siz: f32) {
    let trunk_height = 3 + (siz * 6.0) as i32;
    let leaves_rad = 2 + (siz * 5.0) as i32;

    let n = leaves_rad;
    for ry in -n..=n {
        for rz in -n..=n {
            for rx in -n..=n {
                let rp = ivec3(rx, ry, rz);
                if rp.length_squared() >= leaves_rad * leaves_rad {
                    continue;
                }
                let at = lp + IVec3::Y * trunk_height + rp;
                hood.set_rel(at, |v| {
                    v.tex = tex::LEAVES;
                    v.shape = Shape::Leaves;
                });
            }
        }
    }

    for i in 0..trunk_height {
        if i + lp.y > 15 {
            break;
        }
        let c = hood.at(lp + IVec3::Y * i);
        c.tex = tex::LOG;
        c.shape = Shape::Isosurface;
        c.set_isovalue(2.0 * (1.2 - i as f32 / trunk_height as f32));
    }
}

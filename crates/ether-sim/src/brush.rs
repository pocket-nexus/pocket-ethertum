//! Breaking and placing (`voxel_client::raycast`, `VoxelBrush`).
//!
//! Around the hit voxel, every voxel of the cube `−n..=n` (`n` = the brush
//! size, truncated) moves its iso value by `f = max(n − |offset|, 0) ×
//! strength`: down to break, up to place. Where `f > 0` (or the size is 0), a
//! place also writes the brush's material and shape (a shape other than the
//! isosurface goes in at iso value 0), and a break empties the material of a
//! voxel left at or below zero. A place starts one voxel out along the hit's
//! normal, as the reference computes it: `normal.normalize_or_zero().as_ivec3()`,
//! which truncates, so only a face square to an axis moves it.
//!
//! [`apply`] returns every voxel it changed. The reference remeshes only the
//! chunk of each; a voxel within two of a chunk's side is also in its
//! neighbour's mesh, so the world remeshes every chunk whose meshing border
//! holds one (a deliberate fix: the reference leaves seams there).

use ether_voxel::vox::{tex, Shape};
use glam::{IVec3, Vec3};

use crate::ray::HitResult;
use crate::Terrain;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelBrush {
    pub shape: Shape,
    pub tex: u8,
    pub size: f32,
    pub strength: f32,
}

impl Default for VoxelBrush {
    fn default() -> VoxelBrush {
        VoxelBrush { size: 4.0, strength: 0.8, shape: Shape::Isosurface, tex: 10 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stroke {
    Break,
    Place,
}

/// Applies the brush at `hit`; the voxels changed, in the order visited (y, then z, then x).
pub fn apply(terrain: &mut dyn Terrain, hit: &HitResult, brush: &VoxelBrush, stroke: Stroke) -> Vec<IVec3> {
    let mut touched = Vec::new();
    if !hit.is_hit {
        return touched;
    }
    let do_place = stroke == Stroke::Place;
    let do_break = stroke == Stroke::Break;
    let n = brush.size as i32;
    let offset = if do_place { 1 } else { 0 } * hit.normal.normalize_or_zero().as_ivec3();
    for ly in -n..=n {
        for lz in -n..=n {
            for lx in -n..=n {
                let lp = IVec3::new(lx, ly, lz);
                let p = hit.voxel_pos + lp + offset;
                let f = (n as f32 - lp.as_vec3().length()).max(0.) * brush.strength;
                let changed = terrain.edit(p, &mut |v| {
                    v.set_isovalue(v.isovalue() + if do_break { -f } else { f });
                    if f > 0.0 || (n == 0 && f == 0.0) {
                        if do_place {
                            v.tex = brush.tex;
                            v.shape = brush.shape;
                            if brush.shape != Shape::Isosurface {
                                v.set_isovalue(0.0);
                            }
                        } else if v.is_isoval_empty() {
                            v.tex = tex::NIL;
                        }
                    }
                });
                if changed {
                    touched.push(p);
                }
            }
        }
    }
    touched
}

/// The chunks (by corner) whose meshes read a voxel at `p`: its own and every neighbour whose two-voxel
/// meshing border reaches it.
pub fn chunks_reading(p: IVec3, border: i32, out: &mut Vec<IVec3>) {
    let lo = IVec3::new((p.x - border) & !15, (p.y - border) & !15, (p.z - border) & !15);
    let hi = IVec3::new((p.x + border) & !15, (p.y + border) & !15, (p.z + border) & !15);
    let mut c = lo;
    while c.x <= hi.x {
        c.y = lo.y;
        while c.y <= hi.y {
            c.z = lo.z;
            while c.z <= hi.z {
                if !out.contains(&c) {
                    out.push(c);
                }
                c.z += 16;
            }
            c.y += 16;
        }
        c.x += 16;
    }
}

/// `Vec3` of the reference's `hit.normal`, for callers that keep only the brush module in scope.
pub fn place_offset(normal: Vec3) -> IVec3 {
    normal.normalize_or_zero().as_ivec3()
}

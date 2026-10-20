//! The crosshair's ray (`voxel_client::raycast`, avian's `cast_ray`): from the
//! eye along the view, up to 100 m, against the terrain's triangles.
//!
//! The ray is walked in 2 m pieces; each piece asks the terrain for the
//! triangles around it and tests them, nearest first wins, so a hit close to
//! the eye costs one query. The hit's normal faces the eye (a triangle is hit
//! from either side, as a trimesh collider is). The voxel the hit names is the
//! one just behind the surface: `floor(position − 0.01 · normal)`.

use glam::{IVec3, Vec3};

use crate::geom::{face_normal, ray_triangle};
use crate::Terrain;

pub const REACH: f32 = 100.0;
const PIECE: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HitResult {
    pub is_hit: bool,
    pub position: Vec3,
    pub normal: Vec3,
    pub distance: f32,
    pub voxel_pos: IVec3,
}

/// Casts from `origin` along the unit vector `dir`; `scratch` holds the triangles of a piece.
pub fn cast(terrain: &dyn Terrain, origin: Vec3, dir: Vec3, max: f32, scratch: &mut Vec<[Vec3; 3]>) -> HitResult {
    let mut from = 0.0f32;
    while from < max {
        let to = (from + PIECE).min(max);
        let (a, b) = (origin + dir * from, origin + dir * to);
        let pad = Vec3::splat(1e-3);
        scratch.clear();
        terrain.triangles(a.min(b) - pad, a.max(b) + pad, scratch);
        let mut best: Option<(f32, &[Vec3; 3])> = None;
        for tri in scratch.iter() {
            if let Some(t) = ray_triangle(origin, dir, tri[0], tri[1], tri[2]) {
                if t >= from && t <= to && best.is_none_or(|(bt, _)| t < bt) {
                    best = Some((t, tri));
                }
            }
        }
        if let Some((t, tri)) = best {
            let mut normal = face_normal(tri);
            if normal.dot(dir) > 0.0 {
                normal = -normal;
            }
            let position = origin + dir * t;
            return HitResult { is_hit: true, position, normal, distance: t, voxel_pos: (position + -0.01 * normal).floor().as_ivec3() };
        }
        from = to;
    }
    HitResult::default()
}

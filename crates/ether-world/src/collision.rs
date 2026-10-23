//! A chunk's collision triangles bucketed by cell, so that a query reads only the triangles near its box.
//!
//! The chunk is cut into 4 × 4 × 4 cells of 4 voxels. Each triangle is listed in every cell its bounds overlap
//! (clamped to the chunk; the surface net's border triangles reach a little past it). A query visits the cells
//! its box overlaps and takes a triangle listed in several of them only in the first one it visits (the
//! triangle's lowest cell raised to the query's lowest), so each triangle comes back once.
//!
//! The crosshair's ray asks for the triangles along 2-metre pieces of its 100 metres; scanning whole chunks for
//! each piece cost 9 ms a frame on the console when the ray ran far.

use ether_voxel::chunk::LEN;
use ether_voxel::glam::Vec3;

const CELL: f32 = 4.0;
const CELLS: usize = (LEN / 4) as usize;

pub struct Collision {
    /// In chunk coordinates.
    tris: Vec<[Vec3; 3]>,
    /// Each triangle's lowest and highest cell on each axis.
    span: Vec<[[u8; 3]; 2]>,
    /// Cell `i`'s triangles are `list[start[i]..start[i + 1]]`; cell `(x, y, z)` is `i = (z·4 + y)·4 + x`.
    start: Vec<u32>,
    list: Vec<u32>,
}

fn cell(v: f32) -> u8 {
    ((v / CELL).floor() as i32).clamp(0, CELLS as i32 - 1) as u8
}

fn bounds(t: &[Vec3; 3]) -> (Vec3, Vec3) {
    (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]))
}

fn index(x: u8, y: u8, z: u8) -> usize {
    (z as usize * CELLS + y as usize) * CELLS + x as usize
}

impl Collision {
    pub fn new(tris: Vec<[Vec3; 3]>) -> Collision {
        let span: Vec<[[u8; 3]; 2]> = tris
            .iter()
            .map(|t| {
                let (lo, hi) = bounds(t);
                [[cell(lo.x), cell(lo.y), cell(lo.z)], [cell(hi.x), cell(hi.y), cell(hi.z)]]
            })
            .collect();
        // Counting sort: sizes, offsets, then the lists.
        let mut start = vec![0u32; CELLS * CELLS * CELLS + 1];
        let each = |s: &[[u8; 3]; 2], f: &mut dyn FnMut(usize)| {
            for z in s[0][2]..=s[1][2] {
                for y in s[0][1]..=s[1][1] {
                    for x in s[0][0]..=s[1][0] {
                        f(index(x, y, z));
                    }
                }
            }
        };
        for s in &span {
            each(s, &mut |i| start[i + 1] += 1);
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        let mut fill = start.clone();
        let mut list = vec![0u32; *start.last().unwrap() as usize];
        for (t, s) in span.iter().enumerate() {
            each(s, &mut |i| {
                list[fill[i] as usize] = t as u32;
                fill[i] += 1;
            });
        }
        Collision { tris, span, start, list }
    }

    pub fn empty() -> Collision {
        Collision::new(Vec::new())
    }

    pub fn len(&self) -> usize {
        self.tris.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Appends, moved by `offset`, every triangle whose bounds overlap `min..max` (chunk coordinates).
    pub fn query(&self, min: Vec3, max: Vec3, offset: Vec3, out: &mut Vec<[Vec3; 3]>) {
        if self.tris.is_empty() {
            return;
        }
        let lo = [cell(min.x), cell(min.y), cell(min.z)];
        let hi = [cell(max.x), cell(max.y), cell(max.z)];
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let i = index(x, y, z);
                    for &t in &self.list[self.start[i] as usize..self.start[i + 1] as usize] {
                        let s = &self.span[t as usize][0];
                        if [s[0].max(lo[0]), s[1].max(lo[1]), s[2].max(lo[2])] != [x, y, z] {
                            continue;
                        }
                        let tri = &self.tris[t as usize];
                        let (tmin, tmax) = bounds(tri);
                        if tmax.cmpge(min).all() && tmin.cmple(max).all() {
                            out.push([tri[0] + offset, tri[1] + offset, tri[2] + offset]);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index gives the same triangles as a scan of all of them, each once.
    #[test]
    fn query_matches_a_scan() {
        let mut seed = 7u32;
        let mut r = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        let mut tris = Vec::new();
        for _ in 0..3000 {
            let p = Vec3::new(r() * 18.0 - 1.0, r() * 18.0 - 1.0, r() * 18.0 - 1.0);
            let s = if r() < 0.1 { 6.0 } else { 1.2 };
            tris.push([p, p + Vec3::new(r(), r(), r()) * s, p + Vec3::new(r(), r(), r()) * s]);
        }
        let c = Collision::new(tris.clone());
        let o = Vec3::new(16.0, -32.0, 0.0);
        for _ in 0..500 {
            let a = Vec3::new(r() * 20.0 - 2.0, r() * 20.0 - 2.0, r() * 20.0 - 2.0);
            let b = a + Vec3::new(r(), r(), r()) * if r() < 0.2 { 12.0 } else { 2.0 };
            let mut got = Vec::new();
            c.query(a, b, o, &mut got);
            let want: Vec<[Vec3; 3]> = tris
                .iter()
                .filter(|t| {
                    let (lo, hi) = bounds(t);
                    hi.cmpge(a).all() && lo.cmple(b).all()
                })
                .map(|t| [t[0] + o, t[1] + o, t[2] + o])
                .collect();
            let key = |v: &[Vec3; 3]| v.iter().flat_map(|p| p.to_array()).map(f32::to_bits).collect::<Vec<u32>>();
            let mut g: Vec<_> = got.iter().map(key).collect();
            let mut w: Vec<_> = want.iter().map(key).collect();
            g.sort();
            w.sort();
            assert_eq!(g, w);
        }
    }
}

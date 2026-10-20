//! Perlin noise and its fractal sum, value for value as the reference world
//! is generated.
//!
//! Ethertum generates terrain with the `noise` crate (0.9): `Fbm<Perlin>` and
//! `Perlin`. This module computes the same numbers without that crate, so the
//! engine builds without `std`:
//!
//! - **The permutation table.** `noise` seeds a XorShift generator with
//!   `[1, seed, seed, seed]` and shuffles `0..=255` the way `rand` 0.8 shuffles
//!   a slice (Fisher–Yates from the end, each index drawn by widening multiply
//!   with rejection). [`Perm::new`] repeats both.
//! - **Two quirks kept on purpose.** A coordinate `<= 0.0` floors to one less
//!   than its truncation, so `floor(0.0)` is `-1` and the cell offset is `1.0`.
//!   And Ethertum sets `fbm.octaves = 5` on a generator built for 6: the sum runs
//!   five octaves but is scaled by the six-octave factor.
//! - **The same f64 operations in the same order**: a lerp is
//!   `b * t + a * (1 - t)`, the quintic is `x³(x(6x - 15) + 10)`, every result is
//!   clamped to `[-1, 1]` before it is weighted.
//!
//! `ether-oracle` compares this module with the `noise` crate itself.

/// Ethertum's fractal parameters (`noise::Fbm` defaults).
pub const LACUNARITY: f64 = core::f64::consts::PI * 2.0 / 3.0;
pub const PERSISTENCE: f64 = 0.5;
const SCALE_3D: f64 = 1.154_700_538_379_251_5;
const SCALE_2D: f64 = 2.0 / core::f64::consts::SQRT_2;

/// A seeded permutation of `0..=255`.
#[derive(Clone)]
pub struct Perm(pub [u8; 256]);

struct XorShift {
    x: u32,
    y: u32,
    z: u32,
    w: u32,
}

impl XorShift {
    fn next(&mut self) -> u32 {
        let x = self.x;
        let t = x ^ (x << 11);
        self.x = self.y;
        self.y = self.z;
        self.z = self.w;
        let w = self.w;
        self.w = w ^ (w >> 19) ^ (t ^ (t >> 8));
        self.w
    }

    /// A value in `0..n`, as `rand` 0.8 draws `gen_range(0..n)` for `u32`.
    fn below(&mut self, n: u32) -> u32 {
        let zone = (n << n.leading_zeros()).wrapping_sub(1);
        loop {
            let m = self.next() as u64 * n as u64;
            if (m as u32) <= zone {
                return (m >> 32) as u32;
            }
        }
    }
}

impl Perm {
    pub fn new(seed: u32) -> Perm {
        let mut rng = XorShift { x: 1, y: seed, z: seed, w: seed };
        let mut v = [0u8; 256];
        for (i, b) in v.iter_mut().enumerate() {
            *b = i as u8;
        }
        for i in (1..256usize).rev() {
            let j = rng.below(i as u32 + 1) as usize;
            v.swap(i, j);
        }
        Perm(v)
    }

    #[inline(always)]
    fn h2(&self, a: isize, b: isize) -> usize {
        let p = &self.0;
        p[(p[(a & 0xff) as usize] as usize) ^ (b & 0xff) as usize] as usize
    }

    #[inline(always)]
    fn h3(&self, a: isize, b: isize, c: isize) -> usize {
        let p = &self.0;
        p[(p[(p[(a & 0xff) as usize] as usize) ^ (b & 0xff) as usize] as usize) ^ (c & 0xff) as usize] as usize
    }

    pub fn perlin2(&self, x: f64, y: f64) -> f64 {
        let (cx, cy) = (corner(x), corner(y));
        let (dx, dy) = (x - cx as f64, y - cy as f64);
        let g = |ox: isize, oy: isize| -> f64 {
            let (px, py) = (dx - ox as f64, dy - oy as f64);
            match self.h2(cx + ox, cy + oy) & 0b11 {
                0 => px + py,
                1 => -px + py,
                2 => px - py,
                _ => -px - py,
            }
        };
        let (g00, g10, g01, g11) = (g(0, 0), g(1, 0), g(0, 1), g(1, 1));
        let (ux, uy) = (quintic(dx), quintic(dy));
        let r = lerp(lerp(g00, g01, uy), lerp(g10, g11, uy), ux) * SCALE_2D;
        r.clamp(-1.0, 1.0)
    }

    pub fn perlin3(&self, x: f64, y: f64, z: f64) -> f64 {
        let ax = Axis::new(x);
        let ay = Axis::new(y);
        let az = Axis::new(z);
        self.perlin3_axes(&ax, &ay, &az)
    }

    /// Perlin noise at a point given per axis, so a caller that walks a grid
    /// computes each axis's corner, offsets and curve once (see [`Fbm::chunk3`]).
    #[inline(always)]
    fn perlin3_axes(&self, ax: &Axis, ay: &Axis, az: &Axis) -> f64 {
        let g = |ox: usize, oy: usize, oz: usize| -> f64 {
            let (px, py, pz) = (ax.d[ox], ay.d[oy], az.d[oz]);
            match self.h3(ax.c + ox as isize, ay.c + oy as isize, az.c + oz as isize) & 0b1111 {
                0 | 12 => px + py,
                1 | 13 => -px + py,
                2 => px - py,
                3 => -px - py,
                4 => px + pz,
                5 => -px + pz,
                6 => px - pz,
                7 => -px - pz,
                8 => py + pz,
                9 | 14 => -py + pz,
                10 => py - pz,
                _ => -py - pz,
            }
        };
        let r = lerp_u(
            lerp_u(lerp_u(g(0, 0, 0), g(0, 0, 1), az), lerp_u(g(0, 1, 0), g(0, 1, 1), az), ay),
            lerp_u(lerp_u(g(1, 0, 0), g(1, 0, 1), az), lerp_u(g(1, 1, 0), g(1, 1, 1), az), ay),
            ax,
        ) * SCALE_3D;
        r.clamp(-1.0, 1.0)
    }
}

/// One coordinate of a lattice point: its corner, its offsets from the two
/// corners, and its curve `u` with `1 - u`.
#[derive(Clone, Copy)]
struct Axis {
    c: isize,
    d: [f64; 2],
    u: f64,
    v: f64,
}

impl Axis {
    #[inline(always)]
    fn new(x: f64) -> Axis {
        let c = corner(x);
        let d = x - c as f64;
        let u = quintic(d);
        Axis { c, d: [d - 0.0, d - 1.0], u, v: 1.0 - u }
    }
}

/// `noise`'s floor: a coordinate `<= 0.0` goes one below its truncation.
#[inline(always)]
fn corner(x: f64) -> isize {
    if x <= 0.0 {
        x as isize - 1
    } else {
        x as isize
    }
}

#[inline(always)]
fn quintic(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

#[inline(always)]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    b * t + a * (1.0 - t)
}

#[inline(always)]
fn lerp_u(a: f64, b: f64, t: &Axis) -> f64 {
    b * t.u + a * t.v
}

/// Fractal Brownian motion over Perlin noise: `octaves` octaves summed, each
/// from its own table (seed + octave), scaled by the factor of `built` octaves.
#[derive(Clone)]
pub struct Fbm {
    perms: alloc::vec::Vec<Perm>,
    pub octaves: usize,
    pub frequency: f64,
    pub lacunarity: f64,
    pub persistence: f64,
    scale: f64,
}

impl Fbm {
    /// A generator built as `Fbm::<Perlin>::new(seed)` builds one (six octaves) and then summing `octaves` of them,
    /// as Ethertum does by assigning the public field.
    pub fn ethertum(seed: u32, octaves: usize) -> Fbm {
        Fbm::built(seed, 6, octaves)
    }

    pub fn built(seed: u32, built: usize, octaves: usize) -> Fbm {
        let built = built.max(octaves).max(1);
        let perms = (0..built).map(|i| Perm::new(seed.wrapping_add(i as u32))).collect();
        let denom = (1..=built).fold(0.0, |acc, x| acc + PERSISTENCE.powi(x as i32));
        Fbm { perms, octaves, frequency: 1.0, lacunarity: LACUNARITY, persistence: PERSISTENCE, scale: 1.0 / denom }
    }

    pub fn get2(&self, x: f64, y: f64) -> f64 {
        let (mut x, mut y) = (x * self.frequency, y * self.frequency);
        let mut result = 0.0;
        let mut attenuation = self.persistence;
        for perm in &self.perms[..self.octaves] {
            let mut signal = perm.perlin2(x, y);
            signal *= attenuation;
            attenuation *= self.persistence;
            result += signal;
            x *= self.lacunarity;
            y *= self.lacunarity;
        }
        result * self.scale
    }

    pub fn get3(&self, x: f64, y: f64, z: f64) -> f64 {
        let (mut x, mut y, mut z) = (x * self.frequency, y * self.frequency, z * self.frequency);
        let mut result = 0.0;
        let mut attenuation = self.persistence;
        for perm in &self.perms[..self.octaves] {
            let mut signal = perm.perlin3(x, y, z);
            signal *= attenuation;
            attenuation *= self.persistence;
            result += signal;
            x *= self.lacunarity;
            y *= self.lacunarity;
            z *= self.lacunarity;
        }
        result * self.scale
    }

    /// `get3` at every point `(x0 + i, y0 + j, z0 + k) / div` of an `n³` grid, written to `out[i << 8 | j << 4 | k]`
    /// for `n = 16` (`out[(i * n + j) * n + k]` in general). The same numbers as `get3` point by point, from the
    /// same operations: each axis's coordinate, corner, offsets and curve are computed once per octave instead of
    /// once per point, which is most of the arithmetic.
    pub fn grid3(&self, x0: i32, y0: i32, z0: i32, div: f64, n: usize, out: &mut [f64]) {
        assert!(out.len() >= n * n * n && n <= 32);
        let mut coord = [[0f64; 32]; 3];
        for (a, origin) in [x0, y0, z0].into_iter().enumerate() {
            for i in 0..n {
                coord[a][i] = ((origin + i as i32) as f64 / div) * self.frequency;
            }
        }
        out[..n * n * n].fill(0.0);
        let mut attenuation = self.persistence;
        let mut axes = [[Axis { c: 0, d: [0.0; 2], u: 0.0, v: 0.0 }; 32]; 3];
        for perm in &self.perms[..self.octaves] {
            for a in 0..3 {
                for i in 0..n {
                    axes[a][i] = Axis::new(coord[a][i]);
                }
            }
            for i in 0..n {
                for j in 0..n {
                    let row = &mut out[(i * n + j) * n..(i * n + j + 1) * n];
                    for (k, slot) in row.iter_mut().enumerate() {
                        let mut signal = perm.perlin3_axes(&axes[0][i], &axes[1][j], &axes[2][k]);
                        signal *= attenuation;
                        *slot += signal;
                    }
                }
            }
            attenuation *= self.persistence;
            for a in 0..3 {
                for i in 0..n {
                    coord[a][i] *= self.lacunarity;
                }
            }
        }
        for v in out[..n * n * n].iter_mut() {
            *v *= self.scale;
        }
    }
}

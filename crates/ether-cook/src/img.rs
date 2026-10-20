//! Images in linear light and the filters the cook applies to them.
//!
//! Colour sources are sRGB: a texel is decoded through a 256-entry table to
//! linear RGB, filtered there, and encoded back by rounding the sRGB curve.
//! Alpha and normal maps are linear throughout.

use std::sync::OnceLock;

/// An image of linear RGBA, rows top to bottom.
#[derive(Clone)]
pub struct Img {
    pub w: u32,
    pub h: u32,
    pub px: Vec<[f32; 4]>,
}

fn to_linear_table() -> &'static [f32; 256] {
    static T: OnceLock<[f32; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        t
    })
}

pub fn srgb_to_linear(c: u8) -> f32 {
    to_linear_table()[c as usize]
}

pub fn linear_to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

pub fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl Img {
    pub fn new(w: u32, h: u32) -> Img {
        Img { w, h, px: vec![[0.0; 4]; (w * h) as usize] }
    }

    /// An sRGB source: colour to linear light, alpha as is.
    pub fn from_srgb(rgba: &image::RgbaImage) -> Img {
        let (w, h) = rgba.dimensions();
        let px = rgba.pixels().map(|p| [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]), p[3] as f32 / 255.0]).collect();
        Img { w, h, px }
    }

    /// A data source (a normal map): every channel `byte / 255`.
    pub fn from_unorm(rgba: &image::RgbaImage) -> Img {
        let (w, h) = rgba.dimensions();
        let px = rgba.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]).collect();
        Img { w, h, px }
    }

    #[inline]
    pub fn at(&self, x: u32, y: u32) -> [f32; 4] {
        self.px[(y * self.w + x) as usize]
    }

    #[inline]
    pub fn set(&mut self, x: u32, y: u32, v: [f32; 4]) {
        let w = self.w;
        self.px[(y * w + x) as usize] = v;
    }

    pub fn crop(&self, x0: u32, y0: u32, w: u32, h: u32) -> Img {
        let mut out = Img::new(w, h);
        for y in 0..h {
            for x in 0..w {
                out.set(x, y, self.at(x0 + x, y0 + y));
            }
        }
        out
    }

    pub fn paste(&mut self, src: &Img, x0: u32, y0: u32) {
        for y in 0..src.h {
            for x in 0..src.w {
                self.set(x0 + x, y0 + y, src.at(x, y));
            }
        }
    }

    /// Half the size, each texel the mean of a 2×2 block (a 1-wide axis stays 1). With `by_alpha` the colour is
    /// weighted by alpha, falling back to the plain mean where the four texels are all transparent.
    pub fn half(&self, by_alpha: bool) -> Img {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut out = Img::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let xs = [(2 * x).min(self.w - 1), (2 * x + 1).min(self.w - 1)];
                let ys = [(2 * y).min(self.h - 1), (2 * y + 1).min(self.h - 1)];
                let mut sum = [0f32; 4];
                let mut wsum = [0f32; 3];
                let mut asum = 0f32;
                for &sy in &ys {
                    for &sx in &xs {
                        let p = self.at(sx, sy);
                        for c in 0..3 {
                            sum[c] += p[c];
                            wsum[c] += p[c] * p[3];
                        }
                        sum[3] += p[3];
                        asum += p[3];
                    }
                }
                let mut v = [sum[0] / 4.0, sum[1] / 4.0, sum[2] / 4.0, sum[3] / 4.0];
                if by_alpha && asum > 0.0 {
                    for c in 0..3 {
                        v[c] = wsum[c] / asum;
                    }
                }
                out.set(x, y, v);
            }
        }
        out
    }

    /// Mean colour in linear light.
    pub fn mean(&self) -> [f32; 4] {
        let mut s = [0f64; 4];
        for p in &self.px {
            for c in 0..4 {
                s[c] += p[c] as f64;
            }
        }
        let n = self.px.len() as f64;
        [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32, (s[3] / n) as f32]
    }

    /// RGBA8 with colour encoded to sRGB.
    pub fn to_srgb8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            out.extend_from_slice(&[linear_to_srgb(p[0]), linear_to_srgb(p[1]), linear_to_srgb(p[2]), unorm(p[3])]);
        }
        out
    }

    /// RGBA8 with every channel as a plain fraction.
    pub fn to_unorm8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            out.extend_from_slice(&[unorm(p[0]), unorm(p[1]), unorm(p[2]), unorm(p[3])]);
        }
        out
    }

    /// Fills the colour of transparent texels (alpha 0) from their nearest opaque ones, a ring at a time: each
    /// pass gives every unfilled texel next to a filled one the mean of its filled neighbours. Alpha is kept.
    pub fn bleed(&mut self) {
        let (w, h) = (self.w as i32, self.h as i32);
        let mut known: Vec<bool> = self.px.iter().map(|p| p[3] > 0.0).collect();
        if !known.iter().any(|&k| k) {
            return;
        }
        loop {
            let mut fills = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    if known[(y * w + x) as usize] {
                        continue;
                    }
                    let (mut s, mut n) = ([0f32; 3], 0);
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let (nx, ny) = (x + dx, y + dy);
                            if (dx, dy) == (0, 0) || nx < 0 || ny < 0 || nx >= w || ny >= h || !known[(ny * w + nx) as usize] {
                                continue;
                            }
                            let p = self.px[(ny * w + nx) as usize];
                            for c in 0..3 {
                                s[c] += p[c];
                            }
                            n += 1;
                        }
                    }
                    if n > 0 {
                        fills.push(((y * w + x) as usize, [s[0] / n as f32, s[1] / n as f32, s[2] / n as f32]));
                    }
                }
            }
            if fills.is_empty() {
                return;
            }
            for (i, c) in fills {
                let a = self.px[i][3];
                self.px[i] = [c[0], c[1], c[2], a];
                known[i] = true;
            }
        }
    }

    /// Share of texels in a region whose alpha, times `scale`, reaches 0.5 once stored as a byte.
    pub fn coverage(&self, x0: u32, y0: u32, w: u32, h: u32, scale: f32) -> f32 {
        let mut n = 0;
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                if unorm(self.at(x, y)[3] * scale) >= 128 {
                    n += 1;
                }
            }
        }
        n as f32 / (w * h) as f32
    }

    /// Scales a region's alpha so its coverage at 0.5 is the nearest to `target` (bisection on the scale).
    pub fn keep_coverage(&mut self, x0: u32, y0: u32, w: u32, h: u32, target: f32) {
        let (mut lo, mut hi) = (0.0f32, 16.0f32);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if self.coverage(x0, y0, w, h, mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let (a, b) = (self.coverage(x0, y0, w, h, lo), self.coverage(x0, y0, w, h, hi));
        let scale = if (a - target).abs() < (b - target).abs() { lo } else { hi };
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let mut p = self.at(x, y);
                p[3] = (p[3] * scale).min(1.0);
                self.set(x, y, p);
            }
        }
    }
}

/// A normal map's level from the level above: the mean of each 2×2 block of unit vectors, made unit again.
/// Channels are `n * 0.5 + 0.5`; the vector is `(r, g, b) * 2 - 1`.
pub fn half_normals(src: &Img) -> Img {
    let (w, h) = ((src.w / 2).max(1), (src.h / 2).max(1));
    let mut out = Img::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut s = [0f32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let p = src.at((2 * x + dx).min(src.w - 1), (2 * y + dy).min(src.h - 1));
                    for c in 0..3 {
                        s[c] += p[c] * 2.0 - 1.0;
                    }
                }
            }
            let len = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt().max(1e-6);
            out.set(x, y, [s[0] / len * 0.5 + 0.5, s[1] / len * 0.5 + 0.5, s[2] / len * 0.5 + 0.5, 1.0]);
        }
    }
    out
}

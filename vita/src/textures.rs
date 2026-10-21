//! The pack's pictures in video memory.
//!
//! `ether-pack` lays each picture out as GXM takes it (block-compressed levels
//! back to back, or RGBA rows); [`Uploader`] copies each one into video memory,
//! swizzling the compressed ones on the transfer engine.

use ether_pack::{tag, Format as PackFormat, Pack, TextureSet};
use pocket_vita_gxm::mem::{Arena, Kind};
use pocket_vita_gxm::texture::{Format, Texture, Uploader, Wrap};

pub struct Textures {
    /// Terrain materials; texture `m - 1` is material `m`.
    pub terrain: Vec<Texture>,
    pub foliage: Texture,
    /// Sky faces: +X, −X, +Y, −Y, +Z, −Z.
    pub sky: Vec<Texture>,
    pub water: Texture,
    /// Average colour of each terrain material (sRGB), for anything that wants one colour of it.
    pub average: Vec<[u8; 4]>,
    /// The sky picture's colour at the horizon: the mean of the middle rows of the four side faces (0..1, sRGB).
    pub horizon: [f32; 3],
    /// The sky box folded into one octahedral map, 128 × 128 ([`octahedral`]): what far terrain fades towards,
    /// read per vertex in the direction of the vertex.
    pub ring: Texture,
    _vram: Arena,
    pub bytes: usize,
}

fn format(f: PackFormat) -> Format {
    match f {
        PackFormat::Rgba8 => Format::Rgba8,
        PackFormat::Bc1 => Format::Bc1,
        PackFormat::Bc3 => Format::Bc3,
        PackFormat::Bc5 => Format::Bc5,
    }
}

unsafe fn upload(up: &mut Uploader, vram: &mut Arena, set: &TextureSet, i: u32, wrap: Wrap, mipmapped: bool) -> Result<Texture, String> {
    let mut t = up.texture(vram, format(set.format), set.width, set.height, set.mips, set.texture(i))?;
    t.set_wrap(wrap, wrap);
    t.set_filter(true, mipmapped);
    Ok(t)
}

impl Textures {
    /// # Safety
    /// GXM is started and no scene is open.
    pub unsafe fn load(bytes: &[u8]) -> Result<Textures, String> {
        let pack = Pack::parse(bytes).map_err(|e| format!("ether.pack: {e:?}"))?;
        let mut vram = Arena::new(Kind::Cdram, 4 * 1024 * 1024);
        let mut up = Uploader::new(2 * 1024 * 1024)?;
        let terr = pack.textures(tag::TERR).map_err(|e| format!("TERR: {e:?}"))?;
        let mut terrain = Vec::new();
        for i in 0..terr.count {
            terrain.push(upload(&mut up, &mut vram, &terr, i, Wrap::Repeat, true)?);
            up.flush();
        }
        let average = (1..=terr.count as u8).map(|m| ether_pack::material_colour(&terr, m).unwrap_or([128, 128, 128, 255])).collect();
        let foli = pack.textures(tag::FOLI).map_err(|e| format!("FOLI: {e:?}"))?;
        let foliage = upload(&mut up, &mut vram, &foli, 0, Wrap::Clamp, true)?;
        up.flush();
        let sky_set = pack.textures(tag::SKY).map_err(|e| format!("SKY_: {e:?}"))?;
        // Rows a little above the middle of +X, −X, +Z and −Z (the band where far terrain meets the sky).
        let mut sum = [0f64; 3];
        let mut n = 0f64;
        if sky_set.format == PackFormat::Rgba8 {
            for face in [0u32, 1, 4, 5] {
                let px = sky_set.texture(face);
                let w = sky_set.width as usize;
                for row in (sky_set.height as usize * 7 / 16)..(sky_set.height as usize / 2) {
                    for x in 0..w {
                        let i = (row * w + x) * 4;
                        for c in 0..3 {
                            sum[c] += px[i + c] as f64;
                        }
                        n += 1.0;
                    }
                }
            }
        }
        let ring_bytes = octahedral(&sky_set);
        let horizon = if n > 0.0 { [(sum[0] / n / 255.0) as f32, (sum[1] / n / 255.0) as f32, (sum[2] / n / 255.0) as f32] } else { [0.6, 0.7, 0.8] };
        let mut sky = Vec::new();
        for i in 0..sky_set.count {
            sky.push(upload(&mut up, &mut vram, &sky_set, i, Wrap::Clamp, false)?);
            up.flush();
        }
        let mut ring = up.texture(&mut vram, Format::Rgba8, RING, RING, 1, &ring_bytes)?;
        ring.set_wrap(Wrap::Clamp, Wrap::Clamp);
        ring.set_filter(true, false);
        up.flush();
        let wn = pack.textures(tag::WNRM).map_err(|e| format!("WNRM: {e:?}"))?;
        let water = upload(&mut up, &mut vram, &wn, 0, Wrap::Repeat, true)?;
        up.flush();
        up.free();
        let bytes = vram.used();
        Ok(Textures { terrain, foliage, sky, water, average, horizon, ring, _vram: vram, bytes })
    }
}

const RING: u32 = 128;

/// Resamples the six sky faces (RGBA8, +X −X +Y −Y +Z −Z, in `ether-pack`'s face convention) into an octahedral
/// map with +Y up: a direction d goes to `q = (d.x, d.z) / (|d.x| + |d.y| + |d.z|)`, folded across the diamond's
/// edges below the horizon (`q ← (1 − |q.yx|)·sign(q)` when d.y < 0), and to the texel at `q / 2 + 1/2`. The
/// horizon is the diamond's edge and the map is continuous across it, so filtering there needs no seam; the
/// vertex programs compute q with no angle (`sky_toward` in frame.cg). Each texel averages a 3 × 3 spread.
fn octahedral(sky: &TextureSet) -> Vec<u8> {
    let n = RING as usize;
    let mut out = vec![0u8; n * n * 4];
    if sky.format != PackFormat::Rgba8 || sky.count < 6 {
        return out;
    }
    let size = sky.width as f32;
    let sample = |d: [f32; 3]| -> [f32; 3] {
        let (x, y, z) = (d[0], d[1], d[2]);
        let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
        // Face and (s, t) by the inverse of the face convention.
        let (face, s, t) = if ax >= ay && ax >= az {
            if x > 0.0 { (0, -z / ax, -y / ax) } else { (1, z / ax, -y / ax) }
        } else if ay >= az {
            if y > 0.0 { (2, x / ay, z / ay) } else { (3, x / ay, -z / ay) }
        } else if z > 0.0 {
            (4, x / az, -y / az)
        } else {
            (5, -x / az, -y / az)
        };
        let px = sky.texture(face);
        let u = (((s + 1.0) * 0.5 * size) as i32).clamp(0, size as i32 - 1) as usize;
        let v = (((t + 1.0) * 0.5 * size) as i32).clamp(0, size as i32 - 1) as usize;
        let i = (v * sky.width as usize + u) * 4;
        [px[i] as f32, px[i + 1] as f32, px[i + 2] as f32]
    };
    // The direction a point of the map stands for, the inverse of the fold.
    let direction = |u: f32, v: f32| -> [f32; 3] {
        let (mut x, mut z) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
        let y = 1.0 - x.abs() - z.abs();
        if y < 0.0 {
            (x, z) = ((1.0 - z.abs()) * x.signum(), (1.0 - x.abs()) * z.signum());
        }
        [x, y, z]
    };
    for j in 0..n {
        for i in 0..n {
            let mut c = [0f32; 3];
            for dj in -1..=1 {
                for di in -1..=1 {
                    let d = direction((i as f32 + 0.5 + di as f32 / 3.0) / n as f32, (j as f32 + 0.5 + dj as f32 / 3.0) / n as f32);
                    let s = sample(d);
                    for k in 0..3 {
                        c[k] += s[k] / 9.0;
                    }
                }
            }
            let o = (j * n + i) * 4;
            out[o] = c[0] as u8;
            out[o + 1] = c[1] as u8;
            out[o + 2] = c[2] as u8;
            out[o + 3] = 255;
        }
    }
    out
}

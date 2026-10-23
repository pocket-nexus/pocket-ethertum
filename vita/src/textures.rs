//! The pack's pictures in video memory.
//!
//! `ether-pack` lays each picture out as GXM takes it (block-compressed levels
//! back to back, or RGBA rows); [`Uploader`] copies each one into video memory,
//! swizzling the compressed ones on the transfer engine.

use ether_pack::{tag, Format as PackFormat, Pack, TextureSet};
use pocket_vita_gxm::mem::{Arena, Kind};
use pocket_vita_gxm::texture::{Format, Texture, Uploader, Wrap};

/// The display's tone curve: the reference's tone map (TonyMcMapface) and sRGB encoding, fitted as one curve. A
/// texel t (sRGB, taken as t² in linear light) under light E shows as `sqrt(x / (x + DISPLAY_K))` with x = t²·E
/// (`display` in frame.cg). Against Bevy's own lookup table, over texels 0.08..0.85 and lights 0.15..6, the fit
/// is off by 0.012 RMS and 0.025 at most.
pub const DISPLAY_K: f32 = 0.8;
/// The reference's camera exposure, 2^−9.7 / 1.2 (Bevy's default, EV100 9.7), times the brightness of its sky
/// box and of its environment light, 1000: the factor from a sky texel to its light.
pub const SKY_EXPOSED: f32 = 1.0019;

/// The sky picture's light on a face, in linear light after the exposure: the reference lights every face with
/// its sky box read in the face's normal (an environment light of the raw picture). Means over the cap within
/// 26° of straight up, the band within 9° of the horizon and the cap around straight down.
#[derive(Clone, Copy)]
pub struct SkyLight {
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    pub nadir: [f32; 3],
}

pub struct Textures {
    /// Terrain materials; texture `m - 1` is material `m`.
    pub terrain: Vec<Texture>,
    pub foliage: Texture,
    /// Sky faces: +X, −X, +Y, −Y, +Z, −Z.
    pub sky: Vec<Texture>,
    pub water: Texture,
    /// Average colour of each terrain material (sRGB), for anything that wants one colour of it.
    pub average: Vec<[u8; 4]>,
    /// The sky picture's colour at the horizon as shown: the mean of the middle rows of the four side faces (0..1).
    pub horizon: [f32; 3],
    pub light: SkyLight,
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
        if sky_set.format != PackFormat::Rgba8 || sky_set.count < 6 {
            return Err("SKY_: six RGBA faces expected".into());
        }
        let size = sky_set.width as usize;
        let light = sky_light(&sky_set);
        // The faces as the display shows them: through the tone curve, as the reference shows its sky box.
        let curve: Vec<u8> = (0..256)
            .map(|b| {
                let t = b as f32 / 255.0;
                let x = t * t * SKY_EXPOSED;
                ((x / (x + DISPLAY_K)).sqrt() * 255.0 + 0.5) as u8
            })
            .collect();
        let faces: Vec<Vec<u8>> = (0..6)
            .map(|f| {
                let mut px = sky_set.texture(f).to_vec();
                for (i, b) in px.iter_mut().enumerate() {
                    if i % 4 != 3 {
                        *b = curve[*b as usize];
                    }
                }
                px
            })
            .collect();
        // Rows a little above the middle of +X, −X, +Z and −Z (the band where far terrain meets the sky).
        let mut sum = [0f64; 3];
        let mut n = 0f64;
        for face in [0, 1, 4, 5] {
            let px = &faces[face];
            for row in (size * 7 / 16)..(size / 2) {
                for x in 0..size {
                    let i = (row * size + x) * 4;
                    for c in 0..3 {
                        sum[c] += px[i + c] as f64;
                    }
                    n += 1.0;
                }
            }
        }
        let horizon = [(sum[0] / n / 255.0) as f32, (sum[1] / n / 255.0) as f32, (sum[2] / n / 255.0) as f32];
        let ring_bytes = octahedral(&faces, size);
        let mut sky = Vec::new();
        for face in &faces {
            let mut t = up.texture(&mut vram, Format::Rgba8, size as u32, size as u32, 1, face)?;
            t.set_wrap(Wrap::Clamp, Wrap::Clamp);
            t.set_filter(true, false);
            sky.push(t);
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
        Ok(Textures { terrain, foliage, sky, water, average, horizon, light, ring, _vram: vram, bytes })
    }
}

const RING: u32 = 128;

/// Resamples the six sky faces (RGBA8, +X −X +Y −Y +Z −Z, in `ether-pack`'s face convention) into an octahedral
/// map with +Y up: a direction d goes to `q = (d.x, d.z) / (|d.x| + |d.y| + |d.z|)`, folded across the diamond's
/// edges below the horizon (`q ← (1 − |q.yx|)·sign(q)` when d.y < 0), and to the texel at `q / 2 + 1/2`. The
/// horizon is the diamond's edge and the map is continuous across it, so filtering there needs no seam; the
/// vertex programs compute q with no angle (`sky_toward` in frame.cg). Each texel averages a 3 × 3 spread.
fn octahedral(faces: &[Vec<u8>], width: usize) -> Vec<u8> {
    let n = RING as usize;
    let mut out = vec![0u8; n * n * 4];
    let size = width as f32;
    // A world direction reads the cube with z negated, as the reference's sky box does (`sky_box_vertices`).
    let sample = |d: [f32; 3]| -> [f32; 3] {
        let (x, y, z) = (d[0], d[1], -d[2]);
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
        let px = &faces[face];
        let u = (((s + 1.0) * 0.5 * size) as i32).clamp(0, size as i32 - 1) as usize;
        let v = (((t + 1.0) * 0.5 * size) as i32).clamp(0, size as i32 - 1) as usize;
        let i = (v * width + u) * 4;
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

/// The sky's light by height ([`SkyLight`]), from the faces as the pack holds them (sRGB). Every fourth texel on
/// each axis, weighted by the solid angle it covers.
fn sky_light(sky: &TextureSet) -> SkyLight {
    let size = sky.width as usize;
    let linear: Vec<f32> = (0..256)
        .map(|b| {
            let c = b as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        })
        .collect();
    let mut sums = [[0f64; 4]; 3];
    for face in 0..6u32 {
        let px = sky.texture(face);
        for j in (0..size).step_by(4) {
            for i in (0..size).step_by(4) {
                let s = (i as f32 + 0.5) / size as f32 * 2.0 - 1.0;
                let t = (j as f32 + 0.5) / size as f32 * 2.0 - 1.0;
                // The height of the texel's direction (the faces as `sky_box_vertices` in scene.rs lays them).
                let up = match face {
                    2 => 1.0,
                    3 => -1.0,
                    _ => -t,
                };
                let len2 = 1.0 + s * s + t * t;
                let y = up / len2.sqrt();
                let band = if y > 0.9 {
                    0
                } else if y.abs() < 0.15 {
                    1
                } else if y < -0.9 {
                    2
                } else {
                    continue;
                };
                let w = 1.0 / (len2 * len2.sqrt()) as f64;
                let k = (j * size + i) * 4;
                for c in 0..3 {
                    sums[band][c] += linear[px[k + c] as usize] as f64 * w;
                }
                sums[band][3] += w;
            }
        }
    }
    let mean = |b: usize| {
        let s = &sums[b];
        let w = s[3].max(1e-9);
        [(s[0] / w) as f32 * SKY_EXPOSED, (s[1] / w) as f32 * SKY_EXPOSED, (s[2] / w) as f32 * SKY_EXPOSED]
    };
    SkyLight { zenith: mean(0), horizon: mean(1), nadir: mean(2) }
}

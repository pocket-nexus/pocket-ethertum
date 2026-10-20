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
    /// Distant terrain fades into it.
    pub horizon: [f32; 3],
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
        let horizon = if n > 0.0 { [(sum[0] / n / 255.0) as f32, (sum[1] / n / 255.0) as f32, (sum[2] / n / 255.0) as f32] } else { [0.6, 0.7, 0.8] };
        let mut sky = Vec::new();
        for i in 0..sky_set.count {
            sky.push(upload(&mut up, &mut vram, &sky_set, i, Wrap::Clamp, false)?);
            up.flush();
        }
        let wn = pack.textures(tag::WNRM).map_err(|e| format!("WNRM: {e:?}"))?;
        let water = upload(&mut up, &mut vram, &wn, 0, Wrap::Repeat, true)?;
        up.flush();
        up.free();
        let bytes = vram.used();
        Ok(Textures { terrain, foliage, sky, water, average, horizon, _vram: vram, bytes })
    }
}

//! The PS Vita's pack (`ether.pack`, `bun tools/cook.ts`) on this GPU.
//!
//! The pack lays its pictures out as GXM takes them: block-compressed levels back to back (`TERR` BC1, `FOLI` BC3,
//! `WNRM` BC5) and the sky's six faces as RGBA rows. Where the adapter reads BC blocks
//! (`texture-compression-bc`, every desktop GPU) the levels go up as they are; elsewhere (a phone's GPU) each level
//! is taken apart into rows of RGBA here. The sky is what `ether-look` derives from it on every device: the faces
//! through the display's curve, as one cube map, the octahedral map far terrain fades towards, and the light the
//! sky casts by height.

use ether_pack::{tag, Format as PackFormat, Pack, TextureSet};
use pocket_web_wgpu::gpu::Gpu;
use pocket_web_wgpu::wgpu::{self, Texture, TextureFormat, TextureView};

pub use ether_look::SkyLight;

pub struct Pictures {
    /// The terrain's materials as the layers of one array: layer `m - 1` is material `m`.
    pub terrain: TextureView,
    pub foliage: TextureView,
    /// The water's normal map: x in red, z in green.
    pub water: TextureView,
    /// The sky's faces as shown, as a cube.
    pub sky: TextureView,
    /// The octahedral map of the sky as shown (`ether_look::octahedral`).
    pub ring: TextureView,
    /// Each terrain material's mean texel (sRGB).
    pub average: Vec<[u8; 4]>,
    pub light: SkyLight,
    /// The sky at the horizon as shown.
    pub horizon: [f32; 3],
    /// The pictures went up as blocks.
    pub blocks: bool,
    pub bytes: usize,
    _textures: Vec<Texture>,
}

/// What a pack's picture format is on this GPU, as blocks.
fn block_format(f: PackFormat) -> Option<TextureFormat> {
    match f {
        PackFormat::Bc1 => Some(TextureFormat::Bc1RgbaUnorm),
        PackFormat::Bc3 => Some(TextureFormat::Bc3RgbaUnorm),
        PackFormat::Bc5 => Some(TextureFormat::Bc5RgUnorm),
        PackFormat::Rgba8 => None,
    }
}

/// A texture of `layers` pictures of a set, every level, with the set's format where the GPU takes blocks.
fn upload_set(gpu: &Gpu, label: &str, set: &TextureSet, first: u32, layers: u32, blocks: bool, out: &mut Vec<Texture>, bytes: &mut usize) -> Texture {
    let block = if blocks { block_format(set.format) } else { None };
    let format = block.unwrap_or(TextureFormat::Rgba8Unorm);
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: set.width, height: set.height, depth_or_array_layers: layers },
        mip_level_count: set.mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut rgba = Vec::new();
    for layer in 0..layers {
        for level in 0..set.mips {
            let (w, h) = set.level_size(level);
            let data = set.level(first + layer, level);
            let place = wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All };
            if block.is_some() {
                // A level of blocks goes up as whole blocks: a level under four texels a side is one block's.
                let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
                let per = if set.format == PackFormat::Bc1 { 8 } else { 16 };
                gpu.queue.write_texture(place, data, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bw * per), rows_per_image: Some(bh) }, wgpu::Extent3d { width: bw * 4, height: bh * 4, depth_or_array_layers: 1 });
                *bytes += data.len();
            } else {
                rgba.clear();
                decode(set.format, data, w, h, &mut rgba);
                gpu.queue.write_texture(place, &rgba, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) }, wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
                *bytes += rgba.len();
            }
        }
    }
    out.push(texture.clone());
    texture
}

fn rgba_texture(gpu: &Gpu, label: &str, size: u32, layers: u32, faces: &[&[u8]], out: &mut Vec<Texture>, bytes: &mut usize) -> Texture {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, face) in faces.iter().enumerate() {
        let place = wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 }, aspect: wgpu::TextureAspect::All };
        gpu.queue.write_texture(place, face, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: Some(size) }, wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 });
        *bytes += face.len();
    }
    out.push(texture.clone());
    texture
}

impl Pictures {
    /// The pack's pictures, as blocks when `blocks` (the device has `texture-compression-bc`).
    pub fn load(gpu: &Gpu, bytes: &[u8], blocks: bool) -> Result<Pictures, String> {
        let pack = Pack::parse(bytes).map_err(|e| format!("ether.pack: {e:?}"))?;
        let mut textures = Vec::new();
        let mut held = 0usize;
        let terr = pack.textures(tag::TERR).map_err(|e| format!("TERR: {e:?}"))?;
        let terrain = upload_set(gpu, "terrain", &terr, 0, terr.count, blocks, &mut textures, &mut held);
        let average = (1..=terr.count as u8).map(|m| ether_pack::material_colour(&terr, m).unwrap_or([128, 128, 128, 255])).collect();
        let foli = pack.textures(tag::FOLI).map_err(|e| format!("FOLI: {e:?}"))?;
        let foliage = upload_set(gpu, "foliage", &foli, 0, 1, blocks, &mut textures, &mut held);
        let wn = pack.textures(tag::WNRM).map_err(|e| format!("WNRM: {e:?}"))?;
        let water = upload_set(gpu, "water normals", &wn, 0, 1, blocks, &mut textures, &mut held);
        let sky_set = pack.textures(tag::SKY).map_err(|e| format!("SKY_: {e:?}"))?;
        let sky = ether_look::Sky::of(&sky_set)?;
        let faces: Vec<&[u8]> = sky.faces.iter().map(|f| f.as_slice()).collect();
        let cube = rgba_texture(gpu, "sky", sky.size, 6, &faces, &mut textures, &mut held);
        let ring = rgba_texture(gpu, "ring", ether_look::RING, 1, &[&sky.ring], &mut textures, &mut held);
        let view = |t: &Texture, dimension| t.create_view(&wgpu::TextureViewDescriptor { dimension: Some(dimension), ..Default::default() });
        Ok(Pictures {
            terrain: view(&terrain, wgpu::TextureViewDimension::D2Array),
            foliage: view(&foliage, wgpu::TextureViewDimension::D2),
            water: view(&water, wgpu::TextureViewDimension::D2),
            sky: view(&cube, wgpu::TextureViewDimension::Cube),
            ring: view(&ring, wgpu::TextureViewDimension::D2),
            average,
            light: sky.light,
            horizon: sky.horizon,
            blocks,
            bytes: held,
            _textures: textures,
        })
    }
}

/// A level of `w` × `h` texels of a pack's format as rows of RGBA.
pub fn decode(format: PackFormat, data: &[u8], w: u32, h: u32, out: &mut Vec<u8>) {
    let (w, h) = (w as usize, h as usize);
    out.resize(w * h * 4, 0);
    if format == PackFormat::Rgba8 {
        out.copy_from_slice(&data[..w * h * 4]);
        return;
    }
    let per = if format == PackFormat::Bc1 { 8 } else { 16 };
    let bw = w.div_ceil(4);
    for (b, block) in data.chunks_exact(per).enumerate() {
        let (bx, by) = ((b % bw) * 4, (b / bw) * 4);
        let mut texels = [[0u8; 4]; 16];
        match format {
            PackFormat::Bc1 => colour_block(block, true, &mut texels),
            PackFormat::Bc3 => {
                colour_block(&block[8..], false, &mut texels);
                let mut alpha = [0u8; 16];
                value_block(&block[..8], &mut alpha);
                for (t, a) in texels.iter_mut().zip(alpha) {
                    t[3] = a;
                }
            }
            _ => {
                let (mut r, mut g) = ([0u8; 16], [0u8; 16]);
                value_block(&block[..8], &mut r);
                value_block(&block[8..], &mut g);
                for i in 0..16 {
                    texels[i] = [r[i], g[i], 0, 255];
                }
            }
        }
        for (i, t) in texels.iter().enumerate() {
            let (x, y) = (bx + i % 4, by + i / 4);
            if x < w && y < h {
                out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(t);
            }
        }
    }
}

fn rgb565(c: u16) -> [u8; 3] {
    let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
    [((r << 3) | (r >> 2)) as u8, ((g << 2) | (g >> 4)) as u8, ((b << 3) | (b >> 2)) as u8]
}

/// A BC1 colour block (8 bytes). `bc1`: the block's own order of its two colours may mean three colours and a
/// transparent one, as BC1 has; BC3's colour block always has four.
fn colour_block(block: &[u8], bc1: bool, out: &mut [[u8; 4]; 16]) {
    let (c0, c1) = (u16::from_le_bytes([block[0], block[1]]), u16::from_le_bytes([block[2], block[3]]));
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |p: u16, q: u16, d: u16| -> [u8; 4] {
        let f = |i: usize| ((a[i] as u16 * p + b[i] as u16 * q) / d) as u8;
        [f(0), f(1), f(2), 255]
    };
    let colours = if c0 > c1 || !bc1 { [[a[0], a[1], a[2], 255], [b[0], b[1], b[2], 255], mix(2, 1, 3), mix(1, 2, 3)] } else { [[a[0], a[1], a[2], 255], [b[0], b[1], b[2], 255], mix(1, 1, 2), [0, 0, 0, 0]] };
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for (i, t) in out.iter_mut().enumerate() {
        *t = colours[((bits >> (i * 2)) & 3) as usize];
    }
}

/// A BC4 value block (8 bytes): BC3's alpha, and each of BC5's two channels.
fn value_block(block: &[u8], out: &mut [u8; 16]) {
    let (a, b) = (block[0] as u32, block[1] as u32);
    let mut values = [0u32; 8];
    values[0] = a;
    values[1] = b;
    if a > b {
        for i in 1..7 {
            values[i + 1] = ((7 - i as u32) * a + i as u32 * b) / 7;
        }
    } else {
        for i in 1..5 {
            values[i + 1] = ((5 - i as u32) * a + i as u32 * b) / 5;
        }
        values[6] = 0;
        values[7] = 255;
    }
    let bits = u64::from_le_bytes([block[2], block[3], block[4], block[5], block[6], block[7], 0, 0]);
    for (i, o) in out.iter_mut().enumerate() {
        *o = values[((bits >> (i * 3)) & 7) as usize] as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A BC1 block of two colours picks them and their mixes by its two bits a texel.
    #[test]
    fn a_colour_block_is_taken_apart() {
        // red 565 = 0xf800, blue = 0x001f; texel bits 0,1,2,3 in the first row
        let block = [0x00, 0xf8, 0x1f, 0x00, 0b1110_0100, 0, 0, 0];
        let mut out = Vec::new();
        decode(PackFormat::Bc1, &block, 4, 4, &mut out);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out[4..8], &[0, 0, 255, 255]);
        assert_eq!(&out[8..12], &[170, 0, 85, 255]);
        assert_eq!(&out[12..16], &[85, 0, 170, 255]);
        assert_eq!(&out[16..20], &[255, 0, 0, 255]);
    }

    /// A value block's eight values run between its two ends.
    #[test]
    fn a_value_block_is_taken_apart() {
        let mut block = [200u8, 10, 0, 0, 0, 0, 0, 0];
        // texel 0 → index 0 (200), texel 1 → index 1 (10), texel 2 → index 2
        block[2] = 0b10_001_000;
        let mut out = [0u8; 16];
        value_block(&block, &mut out);
        // (index 2 of a block whose first end is the larger: six sevenths of it and a seventh of the other)
        assert_eq!((out[0], out[1], out[2]), (200, 10, 172));
    }
}

//! The sections, cooked from the reference's files.
//!
//! | Section | Source | What happens |
//! | --- | --- | --- |
//! | `TERR` | `assets/baked/atlas_diff.png` | each 256² tile kept at 256², a mip run in linear light, BC1 per level |
//! | `FOLI` | `assets/baked/atlas_diff_foli.png` | each tile's colour bled into its transparent texels, halved to 128², placed 8 to a row in 1024 × 512, a mip run weighted by alpha with each tile's coverage at 0.5 kept, BC3 per level |
//! | `SKY_` | `assets/table_mountain_2_puresky_4k_cubemap.jpg` | six 512² faces stacked down the image, each halved to 256², RGBA8 |
//! | `WNRM` | `assets/water_normals.png` | halved to 256², a mip run of renormalized means, BC5 (R and G) |
//! | `ICON` | `assets/baked/items.png` | ten 64² icons, RGBA8 as they are |
//!
//! BC1 keeps a terrain material at a sixteenth of RGBA8's bytes (all 24 with
//! their mips in 1.0 MB) and is read swizzled by the GPU. The terrain tiles are
//! opaque, so BC1's single-bit alpha is not needed. The foliage needs a real
//! alpha for its cutout: BC3's alpha block keeps 8 levels per 4×4 block, and
//! the cook counts how many texels change side of 0.5 against the source
//! (`FOLI.cutout` in `META`).

use ether_pack::{full_mips, Format, FOLIAGE_COLUMNS, FOLIAGE_ROWS, FOLIAGE_TILE, MATERIALS};
use serde_json::{json, Value};
use texpresso::{Algorithm, Params};

use crate::img::{half_normals, Img};

/// The reference's tiles in its atlases.
pub const SOURCE_TILE: u32 = 256;
pub const TERRAIN_SIZE: u32 = 256;
pub const SKY_SIZE: u32 = 256;
pub const WATER_SIZE: u32 = 256;
pub const ICON_SIZE: u32 = 64;
pub const ICONS: u32 = 10;

fn texpresso_format(f: Format) -> texpresso::Format {
    match f {
        Format::Bc1 => texpresso::Format::Bc1,
        Format::Bc3 => texpresso::Format::Bc3,
        Format::Bc5 => texpresso::Format::Bc5,
        Format::Rgba8 => unreachable!("RGBA8 is stored as it is"),
    }
}

/// One level in `format`'s layout from RGBA8.
pub fn encode(format: Format, rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    if format == Format::Rgba8 {
        return rgba.to_vec();
    }
    let f = texpresso_format(format);
    let mut out = vec![0u8; f.compressed_size(w as usize, h as usize)];
    let params = Params { algorithm: Algorithm::ClusterFit, ..Params::default() };
    f.compress(rgba, w as usize, h as usize, params, &mut out);
    debug_assert_eq!(out.len(), format.level_bytes(w, h));
    out
}

/// One level back to RGBA8.
pub fn decode(format: Format, data: &[u8], w: u32, h: u32) -> Vec<u8> {
    if format == Format::Rgba8 {
        return data.to_vec();
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    texpresso_format(format).decompress(data, w as usize, h as usize, &mut out);
    out
}

/// A mip run of a colour image: level 0 is `base`, each level the half of the one before.
fn colour_chain(base: Img, by_alpha: bool) -> Vec<Img> {
    let n = full_mips(base.w, base.h);
    let mut levels = vec![base];
    for _ in 1..n {
        let next = levels.last().unwrap().half(by_alpha);
        levels.push(next);
    }
    levels
}

pub struct Section {
    pub levels: Vec<Vec<Vec<u8>>>,
    pub extra: Vec<u8>,
    pub meta: Value,
}

/// `TERR`: the 24 terrain materials and their average colours.
pub fn terrain(atlas: &image::RgbaImage) -> Section {
    let atlas = Img::from_srgb(atlas);
    assert_eq!((atlas.w, atlas.h), (SOURCE_TILE * MATERIALS as u32, SOURCE_TILE), "atlas_diff.png is 24 tiles of 256²");
    let mut levels = Vec::new();
    let mut extra = Vec::new();
    for t in 0..MATERIALS as u32 {
        let mut tile = atlas.crop(t * SOURCE_TILE, 0, SOURCE_TILE, SOURCE_TILE);
        for p in tile.px.iter_mut() {
            p[3] = 1.0;
        }
        while tile.w > TERRAIN_SIZE {
            tile = tile.half(false);
        }
        let m = tile.mean();
        extra.extend_from_slice(&[crate::img::linear_to_srgb(m[0]), crate::img::linear_to_srgb(m[1]), crate::img::linear_to_srgb(m[2]), 255]);
        levels.push(colour_chain(tile, false).iter().map(|l| encode(Format::Bc1, &l.to_srgb8(), l.w, l.h)).collect());
    }
    let colours: Vec<String> = extra.chunks(4).map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect();
    let meta = json!({"textures": MATERIALS, "texture": "material t + 1 (VoxTex id)", "size": TERRAIN_SIZE, "format": "BC1", "mips": full_mips(TERRAIN_SIZE, TERRAIN_SIZE), "mipsIn": "linear light", "extra": "average colour per material, sRGB RGBA", "colours": colours});
    Section { levels, extra, meta }
}

/// `FOLI`: the foliage atlas. `cutout` reports how many texels the encoding moved across the 0.5 alpha line.
pub fn foliage(atlas: &image::RgbaImage) -> Section {
    let src = Img::from_srgb(atlas);
    assert_eq!((src.w, src.h), (SOURCE_TILE * MATERIALS as u32, SOURCE_TILE), "atlas_diff_foli.png is 24 tiles of 256²");
    let (aw, ah) = (FOLIAGE_TILE * FOLIAGE_COLUMNS, FOLIAGE_TILE * FOLIAGE_ROWS);
    let mut base = Img::new(aw, ah);
    let mut cutout_tiles = Vec::new();
    for t in 0..MATERIALS as u32 {
        let mut tile = src.crop(t * SOURCE_TILE, 0, SOURCE_TILE, SOURCE_TILE);
        if tile.px.iter().any(|p| p[3] < 1.0) {
            cutout_tiles.push(t);
        }
        tile.bleed();
        while tile.w > FOLIAGE_TILE {
            tile = tile.half(true);
        }
        base.paste(&tile, (t % FOLIAGE_COLUMNS) * FOLIAGE_TILE, (t / FOLIAGE_COLUMNS) * FOLIAGE_TILE);
    }
    // Row 3 holds no tile; it stays transparent, and alpha-weighted halving keeps it out of the tiles' colour.
    let target: Vec<f32> = (0..MATERIALS as u32).map(|t| base.coverage((t % FOLIAGE_COLUMNS) * FOLIAGE_TILE, (t / FOLIAGE_COLUMNS) * FOLIAGE_TILE, FOLIAGE_TILE, FOLIAGE_TILE, 1.0)).collect();

    // The unscaled run, then each level's alpha scaled per tile.
    let plain = colour_chain(base, true);
    let mut scaled = plain.clone();
    for (l, level) in scaled.iter_mut().enumerate().skip(1) {
        let ts = FOLIAGE_TILE >> l;
        if ts == 0 {
            break;
        }
        for t in 0..MATERIALS as u32 {
            let c = target[t as usize];
            if c <= 0.0 || c >= 1.0 {
                continue;
            }
            level.keep_coverage((t % FOLIAGE_COLUMNS) * ts, (t / FOLIAGE_COLUMNS) * ts, ts, ts, c);
        }
    }
    let encoded: Vec<Vec<u8>> = scaled.iter().map(|l| encode(Format::Bc3, &l.to_srgb8(), l.w, l.h)).collect();

    // How the encoding moved the cutout at level 0, over the tiles that have one.
    let source8 = scaled[0].to_srgb8();
    let decoded = decode(Format::Bc3, &encoded[0], aw, ah);
    let (mut moved, mut texels) = (0u64, 0u64);
    for &t in &cutout_tiles {
        let (x0, y0) = ((t % FOLIAGE_COLUMNS) * FOLIAGE_TILE, (t / FOLIAGE_COLUMNS) * FOLIAGE_TILE);
        for y in y0..y0 + FOLIAGE_TILE {
            for x in x0..x0 + FOLIAGE_TILE {
                let i = ((y * aw + x) * 4 + 3) as usize;
                texels += 1;
                if (source8[i] >= 128) != (decoded[i] >= 128) {
                    moved += 1;
                }
            }
        }
    }
    let coverage: Vec<Value> = cutout_tiles
        .iter()
        .map(|&t| {
            let per_level: Vec<f32> = (0..4).map(|l| {
                let ts = FOLIAGE_TILE >> l;
                let (w, h) = (scaled[l].w, scaled[l].h);
                let d = decode(Format::Bc3, &encoded[l], w, h);
                let (x0, y0) = ((t % FOLIAGE_COLUMNS) * ts, (t / FOLIAGE_COLUMNS) * ts);
                let mut n = 0;
                for y in y0..y0 + ts {
                    for x in x0..x0 + ts {
                        if d[((y * w + x) * 4 + 3) as usize] >= 128 {
                            n += 1;
                        }
                    }
                }
                (n as f32 / (ts * ts) as f32 * 1000.0).round() / 1000.0
            }).collect();
            json!({"tile": t, "coverage": per_level})
        })
        .collect();
    let meta = json!({
        "textures": 1, "size": [aw, ah], "format": "BC3", "mips": encoded.len(),
        "tiles": {"size": FOLIAGE_TILE, "columns": FOLIAGE_COLUMNS, "rows": FOLIAGE_ROWS, "tile": "t at column t % 8, row t / 8 (material t + 1)"},
        "cutoutTiles": cutout_tiles,
        "cutout": {"movedTexels": moved, "ofTexels": texels, "share": moved as f64 / texels.max(1) as f64},
        "coverageAtHalf": coverage,
    });
    Section { levels: vec![encoded], extra: Vec::new(), meta }
}

/// `SKY_`: six faces, +X −X +Y −Y +Z −Z, as Bevy reads the stacked image.
pub fn sky(stacked: &image::RgbaImage) -> Section {
    let src = Img::from_srgb(stacked);
    let face = src.w;
    assert_eq!(src.h, face * 6, "the sky is six square faces stacked down the image");
    let mut levels = Vec::new();
    for f in 0..6 {
        let mut img = src.crop(0, f * face, face, face);
        while img.w > SKY_SIZE {
            img = img.half(false);
        }
        levels.push(vec![img.to_srgb8()]);
    }
    let meta = json!({"textures": 6, "faces": ["+X", "-X", "+Y", "-Y", "+Z", "-Z"], "size": SKY_SIZE, "format": "RGBA8", "mips": 1, "source": format!("{face}² faces stacked, 512 × 3072")});
    Section { levels, extra: Vec::new(), meta }
}

/// `WNRM`: the water's normal map, X and Z in R and G.
pub fn water(normals: &image::RgbaImage) -> Section {
    let mut img = Img::from_unorm(normals);
    while img.w > WATER_SIZE {
        img = half_normals(&img);
    }
    let mut chain = vec![img];
    for _ in 1..full_mips(WATER_SIZE, WATER_SIZE) {
        let next = half_normals(chain.last().unwrap());
        chain.push(next);
    }
    let levels: Vec<Vec<u8>> = chain.iter().map(|l| encode(Format::Bc5, &l.to_unorm8(), l.w, l.h)).collect();
    let meta = json!({"textures": 1, "size": WATER_SIZE, "format": "BC5", "mips": levels.len(), "channels": "R = normal X, G = normal Z (world, Y up), as n * 0.5 + 0.5; Y = sqrt(1 - x² - z²)"});
    Section { levels: vec![levels], extra: Vec::new(), meta }
}

/// `ICON`: the items' icons.
pub fn icons(items: &image::RgbaImage) -> Section {
    assert_eq!(items.dimensions(), (ICON_SIZE * ICONS, ICON_SIZE), "items.png is ten 64² icons in a row");
    let mut levels = Vec::new();
    for i in 0..ICONS {
        let mut out = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                out.extend_from_slice(&items.get_pixel(i * ICON_SIZE + x, y).0);
            }
        }
        levels.push(vec![out]);
    }
    let meta = json!({"textures": ICONS, "icon": "i is item i + 1 in the reference's registry order", "size": ICON_SIZE, "format": "RGBA8", "mips": 1});
    Section { levels, extra: Vec::new(), meta }
}

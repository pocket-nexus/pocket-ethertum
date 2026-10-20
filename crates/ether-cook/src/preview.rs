//! Pictures of what was cooked, decoded back from the pack's bytes:
//!
//! - `terrain.png`: the 24 materials at level 0, six to a row, then a row of one material's levels;
//! - `foliage.png`: the atlas over a checkerboard; `foliage-cutout.png`: levels 0 to 4, each drawn at the size of
//!   level 0 with an alpha test at 0.5 over magenta;
//! - `sky.png`: the six faces as a cross; `sky-panorama.png`: the sky sampled around the horizon through the cube
//!   convention of `ether_pack`, which shows whether the faces meet;
//! - `water.png`: the normal map with its Y rebuilt; `icons.png`: the ten icons.

use std::path::Path;

use ether_pack::{tag, Format, Pack, TextureSet};
use image::{Rgba, RgbaImage};

use crate::cook::decode;

fn level_image(set: &TextureSet, i: u32, l: u32) -> RgbaImage {
    let (w, h) = set.level_size(l);
    RgbaImage::from_raw(w, h, decode(set.format, set.level(i, l), w, h)).unwrap()
}

fn checker(x: u32, y: u32) -> [f32; 3] {
    if ((x / 16) + (y / 16)).is_multiple_of(2) {
        [0.55, 0.55, 0.55]
    } else {
        [0.8, 0.8, 0.8]
    }
}

fn over(dst: &mut RgbaImage, x: u32, y: u32, p: [u8; 4]) {
    let a = p[3] as f32 / 255.0;
    let b = checker(x, y);
    let mix = |c: u8, k: f32| ((c as f32 / 255.0) * a + k * (1.0 - a)) * 255.0;
    dst.put_pixel(x, y, Rgba([mix(p[0], b[0]) as u8, mix(p[1], b[1]) as u8, mix(p[2], b[2]) as u8, 255]));
}

fn save(img: &RgbaImage, dir: &Path, name: &str) -> Result<(), String> {
    img.save(dir.join(name)).map_err(|e| format!("{name}: {e}"))
}

pub fn write(pack: &[u8], dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let p = Pack::parse(pack).map_err(|e| format!("{e:?}"))?;

    // Terrain.
    let terr = p.textures(tag::TERR).map_err(|e| format!("{e:?}"))?;
    let mut sheet = RgbaImage::from_pixel(6 * 256, 4 * 256 + 256, Rgba([0, 0, 0, 255]));
    for t in 0..terr.count {
        let img = level_image(&terr, t, 0);
        image::imageops::overlay(&mut sheet, &img, ((t % 6) * 256) as i64, ((t / 6) * 256) as i64);
    }
    let mut x = 0;
    for l in 0..terr.mips {
        let img = level_image(&terr, 21, l);
        image::imageops::overlay(&mut sheet, &img, x, (4 * 256) as i64);
        x += img.width() as i64 + 4;
    }
    save(&sheet, dir, "terrain.png")?;

    // Foliage.
    let foli = p.textures(tag::FOLI).map_err(|e| format!("{e:?}"))?;
    let atlas = level_image(&foli, 0, 0);
    let mut shown = RgbaImage::new(atlas.width(), atlas.height());
    for (x, y, px) in atlas.enumerate_pixels() {
        over(&mut shown, x, y, px.0);
    }
    save(&shown, dir, "foliage.png")?;
    let (aw, ah) = (atlas.width(), atlas.height());
    let mut cut = RgbaImage::new(aw, ah * 5);
    for l in 0..5u32 {
        let img = level_image(&foli, 0, l);
        for y in 0..ah {
            for x in 0..aw {
                let p = img.get_pixel(x >> l, y >> l).0;
                let v = if p[3] >= 128 { [p[0], p[1], p[2], 255] } else { [255, 0, 255, 255] };
                cut.put_pixel(x, y + l * ah, Rgba(v));
            }
        }
    }
    save(&cut, dir, "foliage-cutout.png")?;

    // Sky.
    let sky = p.textures(tag::SKY).map_err(|e| format!("{e:?}"))?;
    let faces: Vec<RgbaImage> = (0..6).map(|f| level_image(&sky, f, 0)).collect();
    let s = sky.width;
    // Cross: +Y above, then −X +Z +X −Z, −Y below.
    let mut cross = RgbaImage::from_pixel(4 * s, 3 * s, Rgba([0, 0, 0, 255]));
    for (f, (cx, cy)) in [(0, (2, 1)), (1, (0, 1)), (2, (1, 0)), (3, (1, 2)), (4, (1, 1)), (5, (3, 1))] {
        image::imageops::overlay(&mut cross, &faces[f], (cx * s) as i64, (cy * s) as i64);
    }
    save(&cross, dir, "sky.png")?;
    let (pw, ph) = (1024u32, 512u32);
    let mut pano = RgbaImage::new(pw, ph);
    for py in 0..ph {
        for px in 0..pw {
            let lon = (px as f32 + 0.5) / pw as f32 * std::f32::consts::TAU;
            let lat = std::f32::consts::FRAC_PI_2 - (py as f32 + 0.5) / ph as f32 * std::f32::consts::PI;
            let d = [lat.cos() * lon.sin(), lat.sin(), -lat.cos() * lon.cos()];
            let (f, sc, tc, m) = cube_face(d);
            let (u, v) = ((sc / m + 1.0) / 2.0, (tc / m + 1.0) / 2.0);
            let (fx, fy) = (((u * s as f32) as u32).min(s - 1), ((v * s as f32) as u32).min(s - 1));
            pano.put_pixel(px, py, *faces[f].get_pixel(fx, fy));
        }
    }
    save(&pano, dir, "sky-panorama.png")?;

    // Water: X and Z from R and G, Y rebuilt, shown as RGB = (X, Y, Z) * 0.5 + 0.5.
    let wn = p.textures(tag::WNRM).map_err(|e| format!("{e:?}"))?;
    let w0 = level_image(&wn, 0, 0);
    let mut water = RgbaImage::new(w0.width(), w0.height());
    for (x, y, p) in w0.enumerate_pixels() {
        let (nx, nz) = (p[0] as f32 / 255.0 * 2.0 - 1.0, p[1] as f32 / 255.0 * 2.0 - 1.0);
        let ny = (1.0 - nx * nx - nz * nz).max(0.0).sqrt();
        let e = |v: f32| ((v * 0.5 + 0.5) * 255.0) as u8;
        water.put_pixel(x, y, Rgba([e(nx), e(ny), e(nz), 255]));
    }
    save(&water, dir, "water.png")?;

    // Icons.
    let icons = p.textures(tag::ICON).map_err(|e| format!("{e:?}"))?;
    assert_eq!(icons.format, Format::Rgba8);
    let mut row = RgbaImage::new(icons.count * icons.width, icons.height);
    for i in 0..icons.count {
        let img = level_image(&icons, i, 0);
        for (x, y, p) in img.enumerate_pixels() {
            over(&mut row, i * icons.width + x, y, p.0);
        }
    }
    save(&row, dir, "icons.png")
}

/// The face a direction falls on and its unnormalized `(sc, tc)` with the major axis' magnitude, inverting the
/// table in `ether_pack`'s docs.
fn cube_face(d: [f32; 3]) -> (usize, f32, f32, f32) {
    let [x, y, z] = d;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ax >= ay && ax >= az {
        if x > 0.0 {
            (0, -z, -y, ax)
        } else {
            (1, z, -y, ax)
        }
    } else if ay >= az {
        if y > 0.0 {
            (2, x, z, ay)
        } else {
            (3, x, -z, ay)
        }
    } else if z > 0.0 {
        (4, x, -y, az)
    } else {
        (5, -x, -y, az)
    }
}

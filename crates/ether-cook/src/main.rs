//! `ether-cook`: the reference's art into `ether.pack`.
//!
//! ```text
//! cargo run --release -p ether-cook -- --ref ref/ethertum --out .pocket-build/pack [--no-preview]
//! ```
//!
//! Writes `ether.pack` (layout: `ether_pack`), `receipt.json` (the pack's
//! sha256 and size, which the device tool compares with the copy on the
//! console) and, unless `--no-preview`, pictures decoded back from the pack in
//! `preview/`. The same sources give the same bytes.

mod cook;
mod img;
mod preview;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ether_pack::{tag, write::texture_set, write::Writer, Format, FOLIAGE_COLUMNS, FOLIAGE_ROWS, FOLIAGE_TILE, VERSION};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The reference's files the pack is cooked from, relative to its checkout.
pub const SOURCES: [&str; 5] = ["assets/baked/atlas_diff.png", "assets/baked/atlas_diff_foli.png", "assets/table_mountain_2_puresky_4k_cubemap.jpg", "assets/water_normals.png", "assets/baked/items.png"];

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn load(reference: &Path, rel: &str) -> Result<image::RgbaImage, String> {
    let path = reference.join(rel);
    Ok(image::open(&path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8())
}

/// The commit the reference checkout is at, read from its git directory.
fn reference_commit(reference: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(reference)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Cooks the pack in memory: its bytes and its `META`.
pub fn cook_pack(reference: &Path) -> Result<(Vec<u8>, Value), String> {
    let mut sources = Map::new();
    for rel in SOURCES {
        let bytes = std::fs::read(reference.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
        sources.insert(rel.into(), json!(sha256_hex(&bytes)));
    }
    let terr = cook::terrain(&load(reference, SOURCES[0])?);
    let foli = cook::foliage(&load(reference, SOURCES[1])?);
    let sky = cook::sky(&load(reference, SOURCES[2])?);
    let water = cook::water(&load(reference, SOURCES[3])?);
    let icons = cook::icons(&load(reference, SOURCES[4])?);

    let meta = json!({
        "pack": "Pocket Ethertum",
        "version": VERSION,
        "reference": {"repository": "https://github.com/Dreamtowards/Ethertum", "commit": reference_commit(reference), "license": "GPL-3.0"},
        "sky": "Table Mountain 2 (Pure Sky), Poly Haven, CC0, as the reference ships it",
        "sources": sources,
        "sections": {"TERR": terr.meta, "FOLI": foli.meta, "SKY_": sky.meta, "WNRM": water.meta, "ICON": icons.meta, "META": "this JSON"},
    });

    let mut w = Writer::new();
    w.section(tag::META, serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?);
    w.section(tag::TERR, texture_set(Format::Bc1, cook::TERRAIN_SIZE, cook::TERRAIN_SIZE, &terr.levels, &terr.extra));
    w.section(tag::FOLI, texture_set(Format::Bc3, FOLIAGE_TILE * FOLIAGE_COLUMNS, FOLIAGE_TILE * FOLIAGE_ROWS, &foli.levels, &[]));
    w.section(tag::SKY, texture_set(Format::Rgba8, cook::SKY_SIZE, cook::SKY_SIZE, &sky.levels, &[]));
    w.section(tag::WNRM, texture_set(Format::Bc5, cook::WATER_SIZE, cook::WATER_SIZE, &water.levels, &[]));
    w.section(tag::ICON, texture_set(Format::Rgba8, cook::ICON_SIZE, cook::ICON_SIZE, &icons.levels, &[]));
    Ok((w.finish(), meta))
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let (mut reference, mut out, mut preview) = (PathBuf::from("ref/ethertum"), PathBuf::from(".pocket-build/pack"), true);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--ref" => reference = args.next().ok_or("--ref needs a path")?.into(),
            "--out" => out = args.next().ok_or("--out needs a path")?.into(),
            "--no-preview" => preview = false,
            _ => return Err(format!("unknown argument {a}; usage: ether-cook [--ref DIR] [--out DIR] [--no-preview]")),
        }
    }
    let t = std::time::Instant::now();
    let (pack, meta) = cook_pack(&reference)?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let file = out.join("ether.pack");
    std::fs::write(&file, &pack).map_err(|e| format!("{}: {e}", file.display()))?;
    let sha = sha256_hex(&pack);
    let receipt = json!({"pack": {"file": "ether.pack", "sha256": sha, "bytes": pack.len()}, "version": VERSION, "reference": meta["reference"]["commit"]});
    std::fs::write(out.join("receipt.json"), serde_json::to_string_pretty(&receipt).unwrap() + "\n").map_err(|e| e.to_string())?;
    let p = ether_pack::Pack::parse(&pack).map_err(|e| format!("the written pack does not parse: {e:?}"))?;
    for i in 0..p.len() {
        let (tag, offset, bytes) = p.section_at(i).unwrap();
        println!("ether-cook: {} at {offset:>9} {:>9} bytes", String::from_utf8_lossy(&tag), bytes.len());
    }
    let cut = &meta["sections"]["FOLI"]["cutout"];
    println!("ether-cook: foliage cutout moved by BC3 at level 0: {} of {} texels", cut["movedTexels"], cut["ofTexels"]);
    if preview {
        preview::write(&pack, &out.join("preview"))?;
        println!("ether-cook: previews in {}", out.join("preview").display());
    }
    println!("ether-cook: {} ({} bytes, sha256 {sha}) in {:.1} s", file.display(), pack.len(), t.elapsed().as_secs_f32());
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ether-cook: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ether_pack::Pack;

    fn reference() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ref/ethertum")
    }

    #[test]
    fn deterministic_and_complete() {
        let (a, _) = cook_pack(&reference()).unwrap();
        let (b, _) = cook_pack(&reference()).unwrap();
        assert_eq!(sha256_hex(&a), sha256_hex(&b), "the same sources cook to the same bytes");
        let p = Pack::parse(&a).unwrap();
        let terr = p.textures(tag::TERR).unwrap();
        assert_eq!((terr.count, terr.width, terr.mips, terr.format), (24, 256, 9, Format::Bc1));
        assert_eq!(terr.extra().len(), 24 * 4);
        let foli = p.textures(tag::FOLI).unwrap();
        assert_eq!((foli.count, foli.width, foli.height, foli.mips, foli.format), (1, 1024, 512, 11, Format::Bc3));
        let sky = p.textures(tag::SKY).unwrap();
        assert_eq!((sky.count, sky.width, sky.mips, sky.format), (6, 256, 1, Format::Rgba8));
        let w = p.textures(tag::WNRM).unwrap();
        assert_eq!((w.count, w.width, w.mips, w.format), (1, 256, 9, Format::Bc5));
        let icons = p.textures(tag::ICON).unwrap();
        assert_eq!((icons.count, icons.width, icons.format), (10, 64, Format::Rgba8));
        assert!(p.meta().unwrap().contains("\"commit\""));
    }

    /// Each tile lands where the layout says: a terrain material is its atlas tile, a foliage tile is the source
    /// tile halved at column `t % 8`, row `t / 8`.
    #[test]
    fn tiles_are_in_place() {
        let r = reference();
        let terr_src = load(&r, SOURCES[0]).unwrap();
        let foli_src = load(&r, SOURCES[1]).unwrap();
        let (pack, _) = cook_pack(&r).unwrap();
        let p = Pack::parse(&pack).unwrap();

        // Terrain: mean colour of each decoded level 0 against its source tile.
        let terr = p.textures(tag::TERR).unwrap();
        for t in 0..24u32 {
            let d = cook::decode(Format::Bc1, terr.level(t, 0), 256, 256);
            let mean = |f: &dyn Fn(u32, u32) -> [u8; 4]| {
                let mut s = [0u64; 3];
                for y in 0..256 {
                    for x in 0..256 {
                        let p = f(x, y);
                        for c in 0..3 {
                            s[c] += p[c] as u64;
                        }
                    }
                }
                s.map(|v| v as f64 / 65536.0)
            };
            let ours = mean(&|x, y| {
                let i = ((y * 256 + x) * 4) as usize;
                [d[i], d[i + 1], d[i + 2], d[i + 3]]
            });
            let src = mean(&|x, y| terr_src.get_pixel(t * 256 + x, y).0);
            for c in 0..3 {
                assert!((ours[c] - src[c]).abs() < 3.0, "material {} channel {c}: {} vs {}", t + 1, ours[c], src[c]);
            }
        }

        // Foliage: the decoded alpha mask of each cutout tile against its source tile at half size.
        let foli = p.textures(tag::FOLI).unwrap();
        let d = cook::decode(Format::Bc3, foli.level(0, 0), 1024, 512);
        for t in [12u32, 13, 14, 15, 22] {
            let (x0, y0) = ((t % 8) * 128, (t / 8) * 128);
            let (mut agree, mut n) = (0, 0);
            for y in 0..128 {
                for x in 0..128 {
                    let a = d[(((y0 + y) * 1024 + x0 + x) * 4 + 3) as usize] >= 128;
                    let src: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().map(|(dx, dy)| foli_src.get_pixel(t * 256 + 2 * x + dx, 2 * y + dy).0[3] as u32).sum();
                    agree += (a == (src >= 4 * 128)) as u32;
                    n += 1;
                }
            }
            // A tile in the wrong cell agrees on 50 to 70 %; BC3 and the halving move thin outlines by a few %.
            assert!(agree as f32 / n as f32 > 0.95, "foliage tile {t}: {agree} of {n} texels agree with the source");
        }
        // The empty fourth row stays transparent.
        assert!((384..512).all(|y| (0..1024).step_by(37).all(|x| d[((y * 1024 + x) * 4 + 3) as usize] == 0)));
    }
}

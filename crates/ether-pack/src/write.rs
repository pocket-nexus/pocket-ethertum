//! Writing a pack: sections in the order they are added, each padded to 16
//! bytes, the table first. The cooker uses it; the device only reads.

use alloc::vec::Vec;

use crate::{level_size, Format, ALIGN, ENTRY, HEADER, MAGIC, MAX_LEVELS, TEXTURE_HEADER, VERSION};

fn pad(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(ALIGN) {
        v.push(0);
    }
}

fn put(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

#[derive(Default)]
pub struct Writer {
    sections: Vec<([u8; 4], Vec<u8>)>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }

    pub fn section(&mut self, tag: [u8; 4], bytes: Vec<u8>) {
        self.sections.push((tag, bytes));
    }

    pub fn finish(self) -> Vec<u8> {
        let n = self.sections.len();
        let mut at = (HEADER + n * ENTRY).next_multiple_of(ALIGN);
        let mut table = Vec::new();
        for (tag, bytes) in &self.sections {
            table.push((*tag, at as u32, bytes.len() as u32));
            at = (at + bytes.len()).next_multiple_of(ALIGN);
        }
        let mut out = Vec::with_capacity(at);
        out.extend_from_slice(&MAGIC);
        put(&mut out, VERSION);
        put(&mut out, n as u32);
        put(&mut out, at as u32);
        for (tag, offset, size) in &table {
            out.extend_from_slice(tag);
            put(&mut out, *offset);
            put(&mut out, *size);
        }
        pad(&mut out);
        for (_, bytes) in &self.sections {
            out.extend_from_slice(bytes);
            pad(&mut out);
        }
        debug_assert_eq!(out.len(), at);
        out
    }
}

/// Builds a texture section. `textures[i][l]` is level `l` of texture `i`, already in `format`'s layout.
pub fn texture_set(format: Format, width: u32, height: u32, textures: &[Vec<Vec<u8>>], extra: &[u8]) -> Vec<u8> {
    let mips = textures.first().map_or(1, |t| t.len()) as u32;
    assert!(mips >= 1 && mips as usize <= MAX_LEVELS);
    let mut levels = [0u32; MAX_LEVELS];
    let mut size = 0usize;
    for l in 0..mips {
        levels[l as usize] = size as u32;
        let (w, h) = level_size(width, height, l);
        size += format.level_bytes(w, h);
    }
    for t in textures {
        assert_eq!(t.len() as u32, mips, "every texture of a set has the same levels");
        for (l, data) in t.iter().enumerate() {
            let (w, h) = level_size(width, height, l as u32);
            assert_eq!(data.len(), format.level_bytes(w, h), "level {l} of a {width}x{height} {format:?} texture");
        }
    }
    let stride = size.next_multiple_of(ALIGN);
    let extra_at = if extra.is_empty() { 0 } else { TEXTURE_HEADER };
    let data = (TEXTURE_HEADER + extra.len()).next_multiple_of(ALIGN);

    let mut out = Vec::with_capacity(data + stride * textures.len());
    for v in [textures.len() as u32, width, height, mips, format as u32, stride as u32, data as u32, extra_at as u32, extra.len() as u32, 0, 0, 0] {
        put(&mut out, v);
    }
    for l in levels {
        put(&mut out, l);
    }
    debug_assert_eq!(out.len(), TEXTURE_HEADER);
    out.extend_from_slice(extra);
    pad(&mut out);
    for t in textures {
        for level in t {
            out.extend_from_slice(level);
        }
        pad(&mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{full_mips, tag, Pack, TextureSet};
    use alloc::vec;

    fn rgba_chain(w: u32, h: u32, seed: u8) -> Vec<Vec<u8>> {
        (0..full_mips(w, h)).map(|l| {
            let (lw, lh) = level_size(w, h, l);
            (0..lw * lh * 4).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed).wrapping_add(l as u8)).collect()
        }).collect()
    }

    #[test]
    fn round_trip() {
        let a = vec![rgba_chain(8, 4, 1), rgba_chain(8, 4, 2), rgba_chain(8, 4, 3)];
        let extra = [1u8, 2, 3, 4, 5, 6, 7];
        let bc = vec![(0..full_mips(16, 16)).map(|l| {
            let (w, h) = level_size(16, 16, l);
            vec![l as u8; Format::Bc1.level_bytes(w, h)]
        }).collect::<Vec<_>>()];
        let mut w = Writer::new();
        w.section(tag::META, b"{\"version\":1}".to_vec());
        w.section(tag::ICON, texture_set(Format::Rgba8, 8, 4, &a, &extra));
        w.section(tag::TERR, texture_set(Format::Bc1, 16, 16, &bc, &[]));
        let bytes = w.finish();

        let p = Pack::parse(&bytes).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p.meta(), Some("{\"version\":1}"));
        for i in 0..p.len() {
            assert_eq!(p.section_at(i).unwrap().1 % ALIGN, 0);
        }
        let icons = p.textures(tag::ICON).unwrap();
        assert_eq!((icons.count, icons.width, icons.height, icons.mips, icons.format), (3, 8, 4, 4, Format::Rgba8));
        assert_eq!(icons.extra(), &extra);
        for (i, t) in a.iter().enumerate() {
            for (l, level) in t.iter().enumerate() {
                assert_eq!(icons.level(i as u32, l as u32), &level[..]);
            }
            assert_eq!(icons.texture(i as u32), &t.concat()[..]);
        }
        let terr = p.textures(tag::TERR).unwrap();
        assert_eq!((terr.mips, terr.format), (5, Format::Bc1));
        assert_eq!(terr.level(0, 4), &[4u8; 8][..]);
        assert_eq!(terr.texture(0).len(), 128 + 32 + 8 + 8 + 8);

        let mut bad = bytes.clone();
        bad[4] = 9;
        assert_eq!(Pack::parse(&bad).err(), Some(crate::Error::Version(9)));
        assert_eq!(Pack::parse(&bytes[..bytes.len() - 1]).err(), Some(crate::Error::Truncated));
        assert!(TextureSet::parse(&[0u8; 8]).is_err());
    }

    #[test]
    fn foliage_layout() {
        assert_eq!(crate::foliage_tile_origin(0), (0.0, 0.0));
        assert_eq!(crate::foliage_tile_origin(12), (0.5, 0.25));
        assert_eq!(crate::foliage_tile_origin(22), (0.75, 0.5));
        assert_eq!(full_mips(1024, 512), 11);
        assert_eq!(full_mips(256, 256), 9);
    }
}

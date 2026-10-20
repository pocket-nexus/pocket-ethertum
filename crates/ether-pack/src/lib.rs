//! The cooked pack: the reference's art in the layouts the PS Vita uploads.
//!
//! One file, little-endian, read in place:
//!
//! | Offset | Field |
//! | --- | --- |
//! | 0 | magic `ETHP` |
//! | 4 | [`VERSION`] (`u32`) |
//! | 8 | section count `n` (`u32`) |
//! | 12 | file size in bytes (`u32`) |
//! | 16 | `n` entries of 12 bytes: tag (`[u8; 4]`), offset from the file's start (`u32`), size in bytes (`u32`) |
//!
//! Every section starts on a multiple of 16. Tags are in [`tag`].
//!
//! Five sections are [`TextureSet`]s: one header, then textures of one size
//! and format, each a full run of mip levels packed back to back, largest
//! first. A level is what `pocket_vita_gxm::texture::Uploader::texture` takes:
//! for [`Format::Rgba8`] `w × h × 4` bytes in rows, for the block formats
//! `ceil(w / 4) × ceil(h / 4)` blocks of 8 ([`Format::Bc1`]) or 16
//! ([`Format::Bc3`], [`Format::Bc5`]) bytes in block rows. Levels shrink by
//! halves down to 1, so a `w × h` texture has `log2(max(w, h)) + 1` levels.
//!
//! | Tag | Textures | Size | Format | Levels | Extra |
//! | --- | --- | --- | --- | --- | --- |
//! | `TERR` | 24 terrain materials; texture `t` is material `t + 1` | 256² | BC1 | 9 | average colour of each, sRGB RGBA (`count × 4` bytes) |
//! | `FOLI` | 1 foliage atlas | 1024 × 512 | BC3 | 11 | none |
//! | `SKY_` | 6 sky faces: +X, −X, +Y, −Y, +Z, −Z | 256² | RGBA8 | 1 | none |
//! | `WNRM` | 1 water normal map | 256² | BC5 | 9 | none |
//! | `ICON` | 10 item icons; icon `i` is item `i + 1` | 64² | RGBA8 | 1 | none |
//!
//! `META` is UTF-8 JSON: the pack's version, the reference commit, the
//! sha256 of every source file, and what each section holds.
//!
//! **Colour.** Terrain, foliage, sky and icons are sRGB, as the source PNGs
//! are; their mip levels are averaged in linear light. The water normal map
//! is not colour: R and G hold the normal's X and Z (world, Y up) as
//! `n * 0.5 + 0.5`; the reader rebuilds Y as `sqrt(1 - x² - z²)`. The
//! reference reads its source as `rgb.rbg * 2 - 1`, the same axes.
//!
//! **Foliage atlas.** Tile `t` (material `t + 1`) is the 128² cell at column
//! `t % 8`, row `t / 8` (rows 0 to 2; row 3 is empty). The reference's
//! foliage coordinates address a 24-wide strip (`u = (f + t) / 24`, `v`); in
//! this atlas they are `u = (t % 8 + f) / 8`, `v = (t / 8 + v) / 4`, with `v`
//! down the image as in the source. Transparent texels carry their tile's
//! colour, so filtering does not darken edges, and each level's alpha is
//! scaled per tile so the share of texels at or above 0.5 stays the share of
//! level 0: an alpha test at 0.5 keeps the cards as full at a distance.
//!
//! **Sky faces** follow the cube map convention of OpenGL and Vulkan, which
//! the reference's Bevy renderer uses. With `u` across and `v` down a face,
//! `s = 2u - 1`, `t = 2v - 1`, the direction a texel looks along is
//! +X `(1, -t, -s)`, −X `(-1, -t, s)`, +Y `(s, 1, t)`, −Y `(s, -1, -t)`,
//! +Z `(s, -t, 1)`, −Z `(-s, -t, -1)`.

#![no_std]

extern crate alloc;

pub mod write;

/// The container's version. A change of any layout raises it.
pub const VERSION: u32 = 1;
pub const MAGIC: [u8; 4] = *b"ETHP";
/// Bytes before the section table.
pub const HEADER: usize = 16;
/// Bytes of one table entry.
pub const ENTRY: usize = 12;
/// Every section and every texture starts on a multiple of this.
pub const ALIGN: usize = 16;

pub mod tag {
    pub const TERR: [u8; 4] = *b"TERR";
    pub const FOLI: [u8; 4] = *b"FOLI";
    pub const SKY: [u8; 4] = *b"SKY_";
    pub const WNRM: [u8; 4] = *b"WNRM";
    pub const ICON: [u8; 4] = *b"ICON";
    pub const META: [u8; 4] = *b"META";
}

/// Terrain materials and foliage tiles: the reference's 24 tiles.
pub const MATERIALS: usize = 24;
/// The foliage atlas: 128² tiles, 8 to a row.
pub const FOLIAGE_TILE: u32 = 128;
pub const FOLIAGE_COLUMNS: u32 = 8;
pub const FOLIAGE_ROWS: u32 = 4;

/// Where tile `t` of the foliage atlas starts, as a fraction of the atlas: `(u, v)` of its top left corner; the
/// tile spans `1 / FOLIAGE_COLUMNS` across and `1 / FOLIAGE_ROWS` down.
pub fn foliage_tile_origin(t: u32) -> (f32, f32) {
    ((t % FOLIAGE_COLUMNS) as f32 / FOLIAGE_COLUMNS as f32, (t / FOLIAGE_COLUMNS) as f32 / FOLIAGE_ROWS as f32)
}

/// A texel format, numbered as stored in a [`TextureSet`] header.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Rgba8 = 0,
    Bc1 = 1,
    Bc3 = 2,
    Bc5 = 3,
}

impl Format {
    pub fn from_u32(v: u32) -> Option<Format> {
        match v {
            0 => Some(Format::Rgba8),
            1 => Some(Format::Bc1),
            2 => Some(Format::Bc3),
            3 => Some(Format::Bc5),
            _ => None,
        }
    }

    pub fn compressed(self) -> bool {
        self != Format::Rgba8
    }

    /// Bytes of one level, as packed in the pack and as the uploader takes it.
    pub fn level_bytes(self, w: u32, h: u32) -> usize {
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        match self {
            Format::Rgba8 => w * h * 4,
            Format::Bc1 => w.div_ceil(4) * h.div_ceil(4) * 8,
            Format::Bc3 | Format::Bc5 => w.div_ceil(4) * h.div_ceil(4) * 16,
        }
    }
}

/// Size of level `l` of a `w × h` texture.
pub fn level_size(w: u32, h: u32, l: u32) -> (u32, u32) {
    ((w >> l).max(1), (h >> l).max(1))
}

/// Levels of a full run for `w × h`: down to 1 × 1.
pub fn full_mips(w: u32, h: u32) -> u32 {
    32 - w.max(h).max(1).leading_zeros()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Magic,
    Version(u32),
    Truncated,
    Misaligned,
    Missing([u8; 4]),
    Format(u32),
    Header,
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, Error> {
    let s = b.get(at..at + 4).ok_or(Error::Truncated)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// A pack read in place.
#[derive(Clone, Copy)]
pub struct Pack<'a> {
    bytes: &'a [u8],
    count: usize,
}

impl<'a> Pack<'a> {
    /// Checks the header and that every section lies inside the file on a multiple of 16.
    pub fn parse(bytes: &'a [u8]) -> Result<Pack<'a>, Error> {
        if bytes.get(0..4) != Some(&MAGIC[..]) {
            return Err(Error::Magic);
        }
        let version = u32_at(bytes, 4)?;
        if version != VERSION {
            return Err(Error::Version(version));
        }
        let count = u32_at(bytes, 8)? as usize;
        if u32_at(bytes, 12)? as usize != bytes.len() {
            return Err(Error::Truncated);
        }
        let pack = Pack { bytes, count };
        for i in 0..count {
            let (_, offset, size) = pack.entry(i)?;
            if !offset.is_multiple_of(ALIGN) {
                return Err(Error::Misaligned);
            }
            if offset.checked_add(size).is_none_or(|end| end > bytes.len()) {
                return Err(Error::Truncated);
            }
        }
        Ok(pack)
    }

    fn entry(&self, i: usize) -> Result<([u8; 4], usize, usize), Error> {
        let at = HEADER + i * ENTRY;
        let t = self.bytes.get(at..at + 4).ok_or(Error::Truncated)?;
        Ok(([t[0], t[1], t[2], t[3]], u32_at(self.bytes, at + 4)? as usize, u32_at(self.bytes, at + 8)? as usize))
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Tag, offset in the file and bytes of section `i`.
    pub fn section_at(&self, i: usize) -> Option<([u8; 4], usize, &'a [u8])> {
        let (tag, offset, size) = self.entry(i).ok()?;
        Some((tag, offset, &self.bytes[offset..offset + size]))
    }

    pub fn section(&self, tag: [u8; 4]) -> Option<&'a [u8]> {
        (0..self.count).filter_map(|i| self.section_at(i)).find(|s| s.0 == tag).map(|s| s.2)
    }

    /// A section read as a [`TextureSet`].
    pub fn textures(&self, tag: [u8; 4]) -> Result<TextureSet<'a>, Error> {
        TextureSet::parse(self.section(tag).ok_or(Error::Missing(tag))?)
    }

    /// The `META` section's JSON.
    pub fn meta(&self) -> Option<&'a str> {
        core::str::from_utf8(self.section(tag::META)?).ok()
    }
}

/// Bytes of a [`TextureSet`] header.
pub const TEXTURE_HEADER: usize = 112;
/// Level offsets a header has room for.
pub const MAX_LEVELS: usize = 16;

/// A texture section's header, at the section's start:
///
/// | Offset | Field |
/// | --- | --- |
/// | 0 | `count`: textures in the set |
/// | 4 | `width` |
/// | 8 | `height` |
/// | 12 | `mips`: levels of each texture |
/// | 16 | `format` ([`Format`] as `u32`) |
/// | 20 | `stride`: bytes from one texture's start to the next (a multiple of 16) |
/// | 24 | `data`: offset of texture 0 from the section's start (a multiple of 16) |
/// | 28 | `extra`: offset of the section's own data from the section's start, 0 if none |
/// | 32 | `extra_bytes` |
/// | 36 | three reserved `u32`, 0 |
/// | 48 | `levels`: 16 `u32`, the offset of each level from its texture's start (0 past `mips`) |
///
/// Texture `i`'s level `l` is at `data + i * stride + levels[l]`, `Format::level_bytes(level_size(w, h, l))` long.
#[derive(Clone, Copy, Debug)]
pub struct TextureSet<'a> {
    pub count: u32,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    pub format: Format,
    pub stride: u32,
    pub levels: [u32; MAX_LEVELS],
    section: &'a [u8],
    data: u32,
    extra: u32,
    extra_bytes: u32,
}

impl<'a> TextureSet<'a> {
    pub fn parse(section: &'a [u8]) -> Result<TextureSet<'a>, Error> {
        let f = |i: usize| u32_at(section, i * 4);
        let format = Format::from_u32(f(4)?).ok_or(Error::Format(f(4)?))?;
        let mut levels = [0u32; MAX_LEVELS];
        for (l, slot) in levels.iter_mut().enumerate() {
            *slot = f(12 + l)?;
        }
        let set = TextureSet { count: f(0)?, width: f(1)?, height: f(2)?, mips: f(3)?, format, stride: f(5)?, data: f(6)?, extra: f(7)?, extra_bytes: f(8)?, levels, section };
        if set.mips == 0 || set.mips as usize > MAX_LEVELS || !(set.data as usize).is_multiple_of(ALIGN) || !(set.stride as usize).is_multiple_of(ALIGN) {
            return Err(Error::Header);
        }
        let last = set.mips - 1;
        let (lw, lh) = level_size(set.width, set.height, last);
        let texture_bytes = set.levels[last as usize] as usize + format.level_bytes(lw, lh);
        if texture_bytes > set.stride as usize && set.count > 1 {
            return Err(Error::Header);
        }
        let end = set.data as usize + set.stride as usize * set.count.saturating_sub(1) as usize + texture_bytes;
        if end > section.len() || set.extra as usize + set.extra_bytes as usize > section.len() {
            return Err(Error::Truncated);
        }
        Ok(set)
    }

    /// Size of level `l`.
    pub fn level_size(&self, l: u32) -> (u32, u32) {
        level_size(self.width, self.height, l)
    }

    /// Texture `i`, every level, as the uploader takes it.
    pub fn texture(&self, i: u32) -> &'a [u8] {
        let start = (self.data + i * self.stride) as usize;
        let last = self.mips - 1;
        let (w, h) = self.level_size(last);
        &self.section[start..start + self.levels[last as usize] as usize + self.format.level_bytes(w, h)]
    }

    /// Level `l` of texture `i`.
    pub fn level(&self, i: u32, l: u32) -> &'a [u8] {
        let start = (self.data + i * self.stride + self.levels[l as usize]) as usize;
        let (w, h) = self.level_size(l);
        &self.section[start..start + self.format.level_bytes(w, h)]
    }

    /// The section's own data (`TERR`: the materials' average colours).
    pub fn extra(&self) -> &'a [u8] {
        &self.section[self.extra as usize..(self.extra + self.extra_bytes) as usize]
    }
}

/// `TERR`'s extra: material `m`'s (1-based) average colour, sRGB RGBA.
pub fn material_colour(terr: &TextureSet, m: u8) -> Option<[u8; 4]> {
    let i = (m as usize).checked_sub(1)? * 4;
    let c = terr.extra().get(i..i + 4)?;
    Some([c[0], c[1], c[2], c[3]])
}

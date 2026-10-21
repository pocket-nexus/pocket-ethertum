//! Off-screen targets and the passes between them.
//!
//! - The **reflection**: the scene mirrored in the sea's plane (y = 0), at half the display's size, which the
//!   water reads at its own place on the screen, rippled.
//! - The **profile** target: with `{"profile": true}` the scene is drawn here, with a completion notice of its
//!   own, then copied to the display. The display scene's own notice waits for the refresh, so it cannot time
//!   the GPU; this one can.

use pocket_vita_gxm::mem::{Arena, Block, Kind};
use pocket_vita_gxm::target::{ColorFormat, Depth, Msaa, Target};
use vita2d_sys as g;

use crate::gpu::{self, Blend, Gpu, Program, Stream};

pub const W: u32 = 960;
pub const H: u32 = 544;

pub struct Post {
    tri: (*const u8, *const u16),
    _block: Block,
    copy: Program,
    copy_unit: u32,
    pub reflection: Target,
    pub profile: Option<Target>,
    vram: Arena,
    main: Arena,
}

impl Post {
    pub unsafe fn new(gpu: &mut Gpu, msaa: Msaa, profile: bool) -> Result<Post, String> {
        let mut block = Block::with_access(Kind::Main, 4096, false)?;
        let vb = block.alloc(24, 16).ok_or("post triangle")?.cast::<f32>();
        let ib = block.alloc(8, 16).ok_or("post triangle")?.cast::<u16>();
        for (i, v) in [-1.0f32, -1.0, 3.0, -1.0, -1.0, 3.0].into_iter().enumerate() {
            *vb.add(i) = v;
        }
        for i in 0..3 {
            *ib.add(i) = i as u16;
        }
        let f32_ = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_F32;
        let copy = gpu.program("copy", "", include_str!("../shaders/post_v.cg"), include_str!("../shaders/copy_f.cg"), &Stream { stride: 8, attrs: &[("aPosition", 0, f32_, 2)] }, msaa.gxm(), &[Blend::Opaque])?;
        let copy_unit = copy.fs.sampler_index("uTex").unwrap_or(0);
        let mut vram = Arena::new(Kind::Cdram, 4 * 1024 * 1024);
        let mut main = Arena::new(Kind::Main, 1024 * 1024);
        let reflection = Target::new(&mut vram, &mut main, W / 2, H / 2, ColorFormat::Rgba8, Msaa::None, Depth::Transient)?;
        let profile = if profile { Some(Target::new(&mut vram, &mut main, W, H, ColorFormat::Rgba8, msaa, Depth::Transient)?) } else { None };
        Ok(Post { tri: (vb as *const u8, ib as *const u16), _block: block, copy, copy_unit, reflection, profile, vram, main })
    }

    /// Bytes of video memory the targets hold.
    pub fn bytes(&self) -> usize {
        self.vram.used() + self.main.used()
    }

    /// Copies a target's colour over the whole of the open scene.
    pub unsafe fn copy(&self, ctx: *mut g::SceGxmContext, from: &g::SceGxmTexture) {
        viewport(ctx, W, H);
        gpu::state_overlay(ctx);
        self.copy.bind(ctx, 0);
        g::sceGxmSetFragmentTexture(ctx, self.copy_unit, from);
        gpu::draw(ctx, self.tri.0, self.tri.1, 3);
    }
}

/// The viewport of a `w` × `h` target, NDC +y up at its top row, depth 0..1 from NDC −1..1.
pub unsafe fn viewport(ctx: *mut g::SceGxmContext, w: u32, h: u32) {
    let (hw, hh) = (w as f32 * 0.5, h as f32 * 0.5);
    g::sceGxmSetViewport(ctx, hw, hw, hh, -hh, 0.5, 0.5);
}

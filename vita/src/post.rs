//! Off-screen targets and the passes between them.
//!
//! - The **reflection**: the scene mirrored in the sea's plane (y = 0), at half the display's size, which the
//!   water reads at its own place on the screen, rippled.
//! - The **scene** target, with the profile switch or the bloom: the scene is drawn here and then copied to the
//!   display. With `{"profile": true}` it gets a completion notice of its own: the display scene's notice waits
//!   for the refresh, so it cannot time the GPU; this one can.
//! - The **bloom** ([`Bloom`]): what is brighter than a threshold in the scene, at a quarter of its size, blurred
//!   across and down, mixed back as the scene is copied to the display. Its defaults are the reference's (Bevy's
//!   `Bloom::default()`): no threshold, and energy kept: the display shows 85% of the scene and 15% of its blur.

use pocket_vita_gxm::mem::{Arena, Block, Kind};
use pocket_vita_gxm::target::{ColorFormat, Depth, Msaa, Target};
use vita2d_sys as g;

use crate::gpu::{self, Blend, Gpu, Param, Program, Stream, Uniforms};

pub const W: u32 = 960;
pub const H: u32 = 544;

pub struct Post {
    tri: (*const u8, *const u16),
    _block: Block,
    copy: Program,
    copy_unit: u32,
    pub reflection: Target,
    pub scene: Option<Target>,
    pub bloom: Option<Bloom>,
    vram: Arena,
    main: Arena,
}

/// The bloom's two quarter-size targets and its three programs.
pub struct Bloom {
    a: Target,
    b: Target,
    down: Program,
    down_params: (Param, u32),
    blur: Program,
    blur_params: (Param, u32),
    compose: Program,
    compose_units: (u32, u32),
    compose_mix: Param,
    /// What of the scene's colour (0..1 each channel) is above `threshold` comes through, times `gain`; the scene
    /// itself is kept at `keep`.
    pub threshold: f32,
    pub gain: f32,
    pub keep: f32,
}

impl Post {
    pub unsafe fn new(gpu: &mut Gpu, msaa: Msaa, profile: bool, bloom: bool) -> Result<Post, String> {
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
        let scene = if profile || bloom { Some(Target::new(&mut vram, &mut main, W, H, ColorFormat::Rgba8, msaa, Depth::Transient)?) } else { None };
        let bloom = if bloom {
            let stream = Stream { stride: 8, attrs: &[("aPosition", 0, f32_, 2)] };
            let none = Msaa::None.gxm();
            let vs = include_str!("../shaders/post_v.cg");
            let down = gpu.program("bloom_down", "", vs, include_str!("../shaders/bloom_down_f.cg"), &stream, none, &[Blend::Opaque])?;
            let blur = gpu.program("blur", "", vs, include_str!("../shaders/blur_f.cg"), &stream, none, &[Blend::Opaque])?;
            let compose = gpu.program("compose", "", vs, include_str!("../shaders/compose_f.cg"), &stream, msaa.gxm(), &[Blend::Opaque])?;
            let down_params = (down.fparam("uBloom"), down.fs.sampler_index("uTex").unwrap_or(0));
            let blur_params = (blur.fparam("uStep"), blur.fs.sampler_index("uTex").unwrap_or(0));
            let compose_units = (compose.fs.sampler_index("uTex").unwrap_or(0), compose.fs.sampler_index("uBloom").unwrap_or(1));
            let compose_mix = compose.fparam("uMix");
            let a = Target::new(&mut vram, &mut main, W / 4, H / 4, ColorFormat::Rgba8, Msaa::None, Depth::None)?;
            let b = Target::new(&mut vram, &mut main, W / 4, H / 4, ColorFormat::Rgba8, Msaa::None, Depth::None)?;
            Some(Bloom { a, b, down, down_params, blur, blur_params, compose, compose_units, compose_mix, threshold: 0.0, gain: 0.15, keep: 0.85 })
        } else {
            None
        };
        Ok(Post { tri: (vb as *const u8, ib as *const u16), _block: block, copy, copy_unit, reflection, scene, bloom, vram, main })
    }

    /// The bloom's three scenes, from the scene target's colour. Call between the scene and the display.
    pub unsafe fn bloom(&mut self, ctx: *mut g::SceGxmContext) {
        let (Some(b), Some(scene)) = (self.bloom.as_mut(), self.scene.as_ref()) else { return };
        let tri = self.tri;
        let pass = |ctx: *mut g::SceGxmContext, target: &mut Target, program: &Program, params: (Param, u32), from: &g::SceGxmTexture, values: [f32; 4]| {
            if target.begin(ctx, 1.0).is_err() {
                return;
            }
            viewport(ctx, target.width, target.height);
            gpu::state_overlay(ctx);
            program.bind(ctx, 0);
            g::sceGxmSetFragmentTexture(ctx, params.1, from);
            Uniforms::fragment(ctx).set(params.0, &values);
            gpu::draw(ctx, tri.0, tri.1, 3);
            target.end(ctx, None);
        };
        let (qw, qh) = (1.0 / (W / 4) as f32, 1.0 / (H / 4) as f32);
        pass(ctx, &mut b.a, &b.down, b.down_params, &scene.texture, [b.threshold, b.gain, 1.0 / W as f32, 1.0 / H as f32]);
        let a_tex = b.a.texture;
        pass(ctx, &mut b.b, &b.blur, b.blur_params, &a_tex, [qw, 0.0, 0.0, 0.0]);
        let b_tex = b.b.texture;
        pass(ctx, &mut b.a, &b.blur, b.blur_params, &b_tex, [0.0, qh, 0.0, 0.0]);
    }

    /// Draws the scene target over the whole open display scene, with the bloom added when `bloom`.
    pub unsafe fn present(&self, ctx: *mut g::SceGxmContext, bloom: bool) {
        let Some(scene) = self.scene.as_ref() else { return };
        match self.bloom.as_ref().filter(|_| bloom) {
            Some(b) => {
                viewport(ctx, W, H);
                gpu::state_overlay(ctx);
                b.compose.bind(ctx, 0);
                g::sceGxmSetFragmentTexture(ctx, b.compose_units.0, &scene.texture);
                g::sceGxmSetFragmentTexture(ctx, b.compose_units.1, &b.a.texture);
                Uniforms::fragment(ctx).set(b.compose_mix, &[b.keep, 0.0, 0.0, 0.0]);
                gpu::draw(ctx, self.tri.0, self.tri.1, 3);
            }
            None => self.copy(ctx, &scene.texture),
        }
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

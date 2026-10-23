//! The sun's shadow: a distance map seen from the sun, world-aligned and repeating, refreshed a band at a time.
//!
//! The map's texels are fixed in light space: texel column `floor(u / τ) mod N`, row `floor(v / τ) mod N`, where u
//! and v are a point's coordinates across the sun's rays and τ is the texel's size. Its window, the N × N texels
//! around the player, moves in steps of 32 texels. Each frame draws one band of the map's rows, so the whole map
//! is drawn once every `BANDS` frames: texels that the window's move brought in, the sun's slow turn, chunks
//! arriving and the player's edits reach it within that time.
//!
//! Each band is a render target of its own over its rows of the one texture the receivers read. A band is two
//! rectangles where the window wraps across the map's columns, each drawn with a viewport and a region clip of its
//! own, from the chunks whose shadow can fall in it: their caster meshes (two-voxel surface nets, `mesh::casters`)
//! and their leaf cards.
//!
//! Each texel holds the distance of the nearest caster from the map's top plane, over `DEPTH` metres, packed
//! into 24 bits of RGB (`shadow_f.cg`). The receivers (`shadow.cg`) compare their own distance with it.

use ether_voxel::glam::{DVec3, Mat4, Vec3, Vec4};
use pocket_vita_gxm::mem::{Arena, Block, Kind};
use pocket_vita_gxm::target::Fence;
use vita2d_sys as g;

use crate::gpu::{self, Blend, Cull, Gpu, Param, Program, Stream, Uniforms};
use crate::scene::Scene;

/// Texels on a side, and the map's size in metres.
pub const N: i64 = 1024;
pub const SIZE: f64 = 96.0;
/// Bands the map is redrawn in, one a frame.
pub const BANDS: i64 = 16;
/// Metres of distance along the sun's rays the map covers.
const DEPTH: f64 = 512.0;
const TILE: i64 = 32;
/// Metres a leaf card's corner may lie behind the nearest caster and still be lit: its own card and the cards
/// crossing it in the same voxel cast within that.
pub const FOLIAGE_BIAS: f32 = 0.25;
/// The alpha under which a leaf card's texel casts no shadow: the reference's cut-out.
pub const LEAF_CUT: f32 = 0.5;

/// What the receivers are given this frame.
#[derive(Clone, Copy)]
pub struct ShadowView {
    /// `uShadow[0..4]` (see `shadow.cg`).
    pub rows: [f32; 16],
    /// The test's bias, in the map's distance units.
    pub bias: f32,
}

struct Caster {
    program: Program,
    vp: Param,
    origin: Param,
    depth: Param,
    cut: Param,
    unit: Option<u32>,
}

struct Band {
    rt: *mut g::SceGxmRenderTarget,
    color: g::SceGxmColorSurface,
}

pub struct Shadow {
    bands: Vec<Band>,
    depth: g::SceGxmDepthStencilSurface,
    pub texture: g::SceGxmTexture,
    terrain: Caster,
    cut: Caster,
    clear: Program,
    tri: (*const u8, *const u16),
    _block: Block,
    _vram: Arena,
    _main: Arena,
    fence: Fence,
    band: i64,
    /// The window's first column and row (absolute texels), the distance of its top plane, and the sun it was
    /// drawn for.
    base: Option<(i64, i64)>,
    top: f64,
    drawn_sun: Vec3,
    /// Rectangles and caster draws of the last frame, for the status receipt.
    pub rects: u32,
    pub draws: u32,
    pub tris: u32,
    pub full: u32,
    /// With the profile switch: wait for the map's scene and time it.
    pub timed: bool,
    pub gpu_ms: f32,
    /// Which casters are drawn: the terrain, the leaf cards (for measurements).
    pub casters: [bool; 2],
    /// Face culling of the terrain casters: 0 none, 1 back, 2 front (for measurements).
    pub cull: u8,
    /// Metres a leaf card's corner may lie behind the nearest caster and be lit ([`FOLIAGE_BIAS`]).
    pub leaf_bias: f32,
    /// The alpha under which a leaf card's texel casts no shadow ([`LEAF_CUT`]).
    pub leaf_cut: f32,
    /// The share of the leaf cards that cast (a prefix of their hashed order), times the share the display draws
    /// ([`Scene::density`], which the governor lowers where the leaves cost more than a refresh).
    pub leaf_share: f32,
}

/// Light space for a sun: two axes across its rays and the direction towards it.
fn basis(sun: Vec3) -> (DVec3, DVec3, DVec3) {
    let l = sun.as_dvec3().normalize();
    let reference = if l.z.abs() < 0.9 { DVec3::Z } else { DVec3::X };
    let u = l.cross(reference).normalize();
    let v = u.cross(l).normalize();
    (u, v, l)
}

impl Shadow {
    pub unsafe fn new(gpu: &mut Gpu) -> Result<Shadow, String> {
        let mut vram = Arena::new(Kind::Cdram, 17 * 1024 * 1024);
        let mut main = Arena::new(Kind::Main, 3 * 1024 * 1024);
        let (n, h) = (N as u32, (N / BANDS) as u32);
        let data = vram.alloc((n * n * 4) as usize, 4096)?;
        core::ptr::write_bytes(data, 0xff, (n * n * 4) as usize);
        let mut texture: g::SceGxmTexture = core::mem::zeroed();
        let r = g::sceGxmTextureInitLinear(&mut texture, data.cast(), g::SceGxmTextureFormat_SCE_GXM_TEXTURE_FORMAT_U8U8U8U8_ABGR, n, n, 0);
        if r < 0 {
            return Err(format!("shadow texture 0x{:08x}", r as u32));
        }
        // The packed distance must not be filtered; the map repeats.
        g::sceGxmTextureSetMinFilter(&mut texture, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_POINT);
        g::sceGxmTextureSetMagFilter(&mut texture, g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_POINT);
        g::sceGxmTextureSetUAddrMode(&mut texture, g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_REPEAT);
        g::sceGxmTextureSetVAddrMode(&mut texture, g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_REPEAT);
        let mut bands = Vec::new();
        for b in 0..BANDS as u32 {
            let mut params: g::SceGxmRenderTargetParams = core::mem::zeroed();
            params.width = n as u16;
            params.height = h as u16;
            params.scenesPerFrame = 1;
            params.multisampleMode = g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_NONE as u16;
            params.driverMemBlock = -1;
            let mut rt = core::ptr::null_mut();
            let r = g::sceGxmCreateRenderTarget(&params, &mut rt);
            if r < 0 {
                return Err(format!("shadow band target 0x{:08x}", r as u32));
            }
            let mut color: g::SceGxmColorSurface = core::mem::zeroed();
            let rows = data.add((b * h * n * 4) as usize);
            let r = g::sceGxmColorSurfaceInit(
                &mut color,
                g::SceGxmColorFormat_SCE_GXM_COLOR_FORMAT_U8U8U8U8_ABGR,
                g::SceGxmColorSurfaceType_SCE_GXM_COLOR_SURFACE_LINEAR,
                g::SceGxmColorSurfaceScaleMode_SCE_GXM_COLOR_SURFACE_SCALE_NONE,
                g::SceGxmOutputRegisterSize_SCE_GXM_OUTPUT_REGISTER_SIZE_32BIT,
                n,
                h,
                n,
                rows.cast(),
            );
            if r < 0 {
                return Err(format!("shadow band surface 0x{:08x}", r as u32));
            }
            bands.push(Band { rt, color });
        }
        // One depth buffer for every band: a band's scene clears it and nothing keeps it.
        let ddata = main.alloc((n * h * 4) as usize, 4096)?;
        let mut depth: g::SceGxmDepthStencilSurface = core::mem::zeroed();
        let r = g::sceGxmDepthStencilSurfaceInit(&mut depth, g::SceGxmDepthStencilFormat_SCE_GXM_DEPTH_STENCIL_FORMAT_DF32M, g::SceGxmDepthStencilSurfaceType_SCE_GXM_DEPTH_STENCIL_SURFACE_TILED, n, ddata.cast(), core::ptr::null_mut());
        if r < 0 {
            return Err(format!("shadow depth 0x{:08x}", r as u32));
        }
        g::sceGxmDepthStencilSurfaceSetBackgroundDepth(&mut depth, 1.0);
        let s16 = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_S16;
        let u16n = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_U16N;
        let f32_ = g::SceGxmAttributeFormat_SCE_GXM_ATTRIBUTE_FORMAT_F32;
        let none = g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_NONE;
        let vs = include_str!("../shaders/shadow_v.cg");
        let fs = include_str!("../shaders/shadow_f.cg");
        // The terrain casts from its coarse caster mesh, the leaves from their cards (`scene.rs` strides).
        let terrain = gpu.program("shadow", "", vs, fs, &Stream { stride: crate::scene::CASTER_STRIDE, attrs: &[("aPosition", 0, s16, 3)] }, none, &[Blend::Opaque])?;
        let cut = gpu.program("shadow_cut", "#define CUT 1\n", vs, fs, &Stream { stride: crate::scene::FOLIAGE_STRIDE, attrs: &[("aPosition", 0, s16, 3), ("aUv", 6, u16n, 2)] }, none, &[Blend::Opaque])?;
        let clear = gpu.program("shadow_clear", "", include_str!("../shaders/post_v.cg"), "float4 main() : COLOR { return float4(1.0, 1.0, 1.0, 1.0); }\n", &Stream { stride: 8, attrs: &[("aPosition", 0, f32_, 2)] }, none, &[Blend::Opaque])?;
        let caster = |p: Program| {
            let unit = p.fs.sampler_index("uTex");
            Caster { vp: p.vparam("uVp"), origin: p.vparam("uOrigin"), depth: p.vparam("uDepth"), cut: p.fparam("uCut"), unit, program: p }
        };
        let mut block = Block::with_access(Kind::Main, 4096, false)?;
        let vb = block.alloc(24, 16).ok_or("shadow triangle")?.cast::<f32>();
        let ib = block.alloc(8, 16).ok_or("shadow triangle")?.cast::<u16>();
        for (i, v) in [-1.0f32, -1.0, 3.0, -1.0, -1.0, 3.0].into_iter().enumerate() {
            *vb.add(i) = v;
        }
        for i in 0..3 {
            *ib.add(i) = i as u16;
        }
        Ok(Shadow {
            bands,
            depth,
            texture,
            terrain: caster(terrain),
            cut: caster(cut),
            clear,
            tri: (vb as *const u8, ib as *const u16),
            _block: block,
            _vram: vram,
            _main: main,
            fence: Fence::new(4, 2),
            band: 0,
            base: None,
            top: 0.0,
            drawn_sun: Vec3::ZERO,
            rects: 0,
            draws: 0,
            tris: 0,
            full: 0,
            timed: false,
            gpu_ms: 0.0,
            casters: [true, true],
            cull: 2,
            leaf_share: 1.0,
            leaf_bias: FOLIAGE_BIAS,
            leaf_cut: LEAF_CUT,
        })
    }

    /// Draws this frame's share of the map and returns what the receivers need; `None` while the sun is down.
    ///
    /// # Safety
    /// No scene open on `ctx`.
    pub unsafe fn update(&mut self, ctx: *mut g::SceGxmContext, scene: &Scene, eye: Vec3, sun: Vec3, strength: f32) -> Option<ShadowView> {
        self.rects = 0;
        self.draws = 0;
        self.tris = 0;
        if strength <= 0.0 || sun.y <= 0.02 {
            self.base = None;
            return None;
        }
        let tau = SIZE / N as f64;
        let (u, v, l) = basis(sun);
        let e = eye.as_dvec3();
        let (eu, ev, el) = (e.dot(u), e.dot(v), e.dot(l));
        let snap = |x: f64| ((x / tau - N as f64 / 2.0) / TILE as f64).floor() as i64 * TILE;
        let base = (snap(eu), snap(ev));
        // The top plane stays while the eye is within a quarter of the range of where it was set; when it moves,
        // the bands take the new one as they are drawn.
        if self.base.is_none() || (el - (self.top - DEPTH / 2.0)).abs() > DEPTH / 4.0 {
            self.top = el + DEPTH / 2.0;
        }
        // The first frame draws every band; after that, one a frame.
        let bands: Vec<i64> = if self.base.is_none() {
            self.full += 1;
            (0..BANDS).collect()
        } else {
            let b = self.band;
            self.band = (self.band + 1) % BANDS;
            vec![b]
        };
        self.base = Some(base);
        let h = N / BANDS;
        for b in bands {
            // The band's texture rows hold the window's absolute rows that land there.
            let r0 = b * h;
            let a0 = base.1 + (r0 - base.1).rem_euclid(N);
            let band = &self.bands[b as usize];
            let r = g::sceGxmBeginScene(ctx, 0, band.rt, core::ptr::null(), core::ptr::null_mut(), core::ptr::null_mut(), &band.color, &self.depth);
            if r < 0 {
                return None;
            }
            for (cu0, cu1) in split(base.0, base.0 + N) {
                self.rect(ctx, scene, eye, (cu0, cu1, a0, a0 + h), (eu, ev, el), (u, v, l));
            }
            g::sceGxmSetRegionClip(ctx, g::SceGxmRegionClipMode_SCE_GXM_REGION_CLIP_NONE, 0, 0, N as u32 - 1, h as u32 - 1);
            g::sceGxmEndScene(ctx, core::ptr::null(), self.fence.signal(0));
        }
        if self.timed {
            let t = std::time::Instant::now();
            self.fence.wait(0);
            self.gpu_ms = self.gpu_ms * 0.9 + t.elapsed().as_secs_f32() * 100.0;
        }

        // The receivers: u and v over the map's size, repeating; the distance below the top plane.
        let s = SIZE;
        let fu = (eu / s).rem_euclid(1.0);
        let fv = (ev / s).rem_euclid(1.0);
        let (wu, wv) = ((eu - base.0 as f64 * tau) / s, (ev - base.1 as f64 * tau) / s);
        let (u, v, ld) = (u / s, v / s, -l / DEPTH);
        let rows = [
            u.x as f32, u.y as f32, u.z as f32, fu as f32,
            v.x as f32, v.y as f32, v.z as f32, fv as f32,
            ld.x as f32, ld.y as f32, ld.z as f32, ((self.top - el) / DEPTH) as f32,
            wu as f32, wv as f32, 1.0, (self.leaf_bias as f64 / DEPTH) as f32,
        ];
        Some(ShadowView { rows, bias: (0.08 / DEPTH) as f32 })
    }

    /// Draws one rectangle of absolute texels `[u0, u1) × [v0, v1)` that does not wrap.
    #[allow(clippy::too_many_arguments)]
    unsafe fn rect(&mut self, ctx: *mut g::SceGxmContext, scene: &Scene, scene_eye: Vec3, (u0, u1, v0, v1): (i64, i64, i64, i64), (eu, ev, el): (f64, f64, f64), (u, v, l): (DVec3, DVec3, DVec3)) {
        self.rects += 1;
        let tau = SIZE / N as f64;
        // Columns of the texture; rows of the band's own target (its first row is 0).
        let (c0, r0) = (u0.rem_euclid(N), 0);
        let (c1, r1) = (c0 + (u1 - u0), v1 - v0);
        let (w, h) = ((c1 - c0) as f32, (r1 - r0) as f32);
        g::sceGxmSetViewport(ctx, (c0 + c1) as f32 * 0.5, w * 0.5, (r0 + r1) as f32 * 0.5, -h * 0.5, 0.5, 0.5);
        g::sceGxmSetRegionClip(ctx, g::SceGxmRegionClipMode_SCE_GXM_REGION_CLIP_OUTSIDE, c0 as u32, r0 as u32, c1 as u32 - 1, r1 as u32 - 1);
        // Every texel starts at the far end of the range; the depth buffer starts cleared with the scene.
        gpu::state_overlay(ctx);
        self.clear.bind(ctx, 0);
        gpu::draw(ctx, self.tri.0, self.tri.1, 3);

        // From the eye to the rectangle's clip space: x and y across the rectangle, z the distance along the rays.
        let (du, dv) = ((u1 - u0) as f64 * tau, (v1 - v0) as f64 * tau);
        let (uu0, vv0) = (u0 as f64 * tau, v0 as f64 * tau);
        let rx = (u * (2.0 / du), (eu - uu0) * 2.0 / du - 1.0);
        let ry = (v * (-2.0 / dv), 1.0 - (ev - vv0) * 2.0 / dv);
        let rz = (l * (-2.0 / DEPTH), 2.0 * (self.top - el) / DEPTH - 1.0);
        #[rustfmt::skip]
        let vp: [f32; 16] = [
            rx.0.x as f32, rx.0.y as f32, rx.0.z as f32, rx.1 as f32,
            ry.0.x as f32, ry.0.y as f32, ry.0.z as f32, ry.1 as f32,
            rz.0.x as f32, rz.0.y as f32, rz.0.z as f32, rz.1 as f32,
            0.0, 0.0, 0.0, 1.0,
        ];
        let ld = -l / DEPTH;
        let depth = [ld.x as f32, ld.y as f32, ld.z as f32, ((self.top - el) / DEPTH) as f32];
        // The rectangle in light space, with the eye's offset, for choosing the chunks that cast into it.
        let lo = Vec4::new((uu0 - eu) as f32, (vv0 - ev) as f32, 0.0, 0.0);
        let hi = Vec4::new((uu0 + du - eu) as f32, (vv0 + dv - ev) as f32, 0.0, 0.0);
        let axes = (u.as_vec3(), v.as_vec3());
        gpu::state_opaque(ctx, match self.cull { 1 => Cull::Back, 2 => Cull::Front, _ => Cull::None });
        let casters = self.casters;
        let terrain = &self.terrain;
        terrain.program.bind(ctx, 0);
        let eye = Vec3::new(0.0, 0.0, 0.0) + scene_eye;
        let (d, t) = if !casters[0] { (0, 0) } else { scene.draw_casters(ctx, eye, axes, (lo, hi), None, &mut |ctx, origin| {
            let un = Uniforms::vertex(ctx);
            un.set(terrain.vp, &vp);
            un.set(terrain.origin, &[origin.x, origin.y, origin.z, 0.0]);
            un.set(terrain.depth, &depth);
        }) };
        self.draws += d;
        self.tris += t;
        gpu::state_opaque(ctx, Cull::None);
        let cut = &self.cut;
        let leaf_cut = self.leaf_cut;
        cut.program.bind(ctx, 0);
        if let Some(unit) = cut.unit {
            g::sceGxmSetFragmentTexture(ctx, unit, &scene.textures.foliage.gxm);
        }
        let (d, t) = if !casters[1] { (0, 0) } else { scene.draw_casters(ctx, eye, axes, (lo, hi), Some(self.leaf_share * scene.density), &mut |ctx, origin| {
            let un = Uniforms::vertex(ctx);
            un.set(cut.vp, &vp);
            un.set(cut.origin, &[origin.x, origin.y, origin.z, 0.0]);
            un.set(cut.depth, &depth);
            Uniforms::fragment(ctx).set(cut.cut, &[leaf_cut, 0.0, 0.0, 0.0]);
        }) };
        self.draws += d;
        self.tris += t;
    }
}

/// An absolute texel range in pieces that do not cross the texture's wrap.
fn split(a0: i64, a1: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut a = a0;
    while a < a1 {
        let next = ((a.div_euclid(N)) + 1) * N;
        let b = next.min(a1);
        out.push((a, b));
        a = b;
    }
    out
}

#[allow(dead_code)]
fn _mat(_: Mat4) {}

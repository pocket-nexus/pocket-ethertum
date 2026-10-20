//! Programs and draw state.
//!
//! The Cg sources are part of the binary. Each compiles once on the console
//! through SceShaccCg and is cached as a GXP under `ux0:data/pocket-ethertum/gxp`,
//! keyed by a hash of its source; a development build also copies it to the USB
//! share, and the standalone package ships those copies (`app0:gxp`) and never
//! loads the compiler.

use pocket_vita_gxm::patcher::Patcher;
pub use pocket_vita_gxm::program::Blend;
use pocket_vita_gxm::program::{self, Gxp, Output, Registered};
use pocket_vita_gxm::shacccg::{Compiler, Stage};
use vita2d_sys as g;

use crate::paths;

pub type Param = *const g::SceGxmProgramParameter;

pub struct Program {
    pub vs: Registered,
    pub fs: Registered,
    pub vp: *mut g::SceGxmVertexProgram,
    fps: Vec<*mut g::SceGxmFragmentProgram>,
}

impl Program {
    /// Binds the program with the fragment program patched for its `blend`-th blend mode.
    pub unsafe fn bind(&self, ctx: *mut g::SceGxmContext, blend: usize) {
        g::sceGxmSetVertexProgram(ctx, self.vp);
        g::sceGxmSetFragmentProgram(ctx, self.fps[blend]);
    }

    pub fn vparam(&self, name: &str) -> Param {
        self.vs.param(name)
    }

    pub fn fparam(&self, name: &str) -> Param {
        self.fs.param(name)
    }
}

/// A draw's default uniform buffer, reserved from the context, written by parameter.
pub struct Uniforms(*mut core::ffi::c_void);

impl Uniforms {
    pub unsafe fn vertex(ctx: *mut g::SceGxmContext) -> Uniforms {
        let mut buf = core::ptr::null_mut();
        g::sceGxmReserveVertexDefaultUniformBuffer(ctx, &mut buf);
        Uniforms(buf)
    }

    pub unsafe fn fragment(ctx: *mut g::SceGxmContext) -> Uniforms {
        let mut buf = core::ptr::null_mut();
        g::sceGxmReserveFragmentDefaultUniformBuffer(ctx, &mut buf);
        Uniforms(buf)
    }

    #[inline]
    pub unsafe fn set(&self, param: Param, values: &[f32]) {
        program::set_uniform(self.0, param, values);
    }
}

/// One vertex stream: its stride and its attributes as (name, offset, format, components).
pub struct Stream<'a> {
    pub stride: u16,
    pub attrs: &'a [(&'a str, u16, u32, u8)],
}

fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for p in parts {
        for &b in *p {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub struct Gpu {
    pub patcher: Patcher,
    compiler: Option<Compiler>,
    /// Programs compiled this run, and taken from a cache.
    pub compiled: u32,
    pub cached: u32,
    /// Hashes of every program in use, for packaging.
    pub manifest: Vec<String>,
    live: bool,
}

impl Gpu {
    pub unsafe fn new(live: bool) -> Result<Gpu, String> {
        let _ = std::fs::create_dir_all(paths::GXP_CACHE);
        Ok(Gpu { patcher: Patcher::new(512 * 1024, 256 * 1024, 512 * 1024)?, compiler: None, compiled: 0, cached: 0, manifest: Vec::new(), live })
    }

    unsafe fn gxp(&mut self, name: &str, source: &str, stage: Stage) -> Result<Vec<u8>, String> {
        let tag: &[u8] = match stage {
            Stage::Vertex => b"v",
            Stage::Fragment => b"f",
        };
        let hash = format!("{:016x}", fnv64(&[source.as_bytes(), tag]));
        self.manifest.push(hash.clone());
        for dir in [paths::GXP_CACHE, paths::GXP_PACKAGED] {
            if let Ok(bytes) = std::fs::read(format!("{dir}/{hash}.gxp")) {
                if bytes.len() > 16 {
                    self.cached += 1;
                    return Ok(bytes);
                }
            }
        }
        if self.compiler.is_none() {
            self.compiler = Some(Compiler::load().map_err(|e| format!("{name}: no cached program and no shader compiler ({e})"))?);
        }
        let out = self.compiler.as_ref().unwrap().compile(name, source, stage, &[]).map_err(|d| {
            let lines: Vec<String> = d.iter().map(|x| format!("{}:{}: {} {}", x.line, x.column, x.level, x.message)).collect();
            format!("{name}: {}", lines.join("; "))
        })?;
        self.compiled += 1;
        let _ = std::fs::write(format!("{}/{hash}.gxp", paths::GXP_CACHE), &out.program);
        if self.live {
            let _ = crate::hostfs::write(&format!("{}/{hash}.gxp", paths::GXP_HOST), &out.program);
        }
        Ok(out.program)
    }

    /// Compiles (or loads) a vertex and a fragment source and patches them for `stream`, with one fragment
    /// program per entry of `blends`.
    pub unsafe fn program(&mut self, name: &str, defines: &str, vs: &str, fs: &str, stream: &Stream, msaa: u32, blends: &[Blend]) -> Result<Program, String> {
        let vsrc = format!("{defines}{vs}");
        let fsrc = format!("{defines}{fs}");
        let vbytes = self.gxp(&format!("{name}_v"), &vsrc, Stage::Vertex)?;
        let fbytes = self.gxp(&format!("{name}_f"), &fsrc, Stage::Fragment)?;
        let vs = Registered::new(self.patcher.raw, Gxp::new(&vbytes)?)?;
        let fs = Registered::new(self.patcher.raw, Gxp::new(&fbytes)?)?;
        let mut attributes = Vec::new();
        for (attr, offset, format, count) in stream.attrs {
            let Some(reg) = vs.attribute_index(attr) else { continue };
            attributes.push(g::SceGxmVertexAttribute { streamIndex: 0, offset: *offset, format: *format as u8, componentCount: *count, regIndex: reg });
        }
        let layout = [g::SceGxmVertexStream { stride: stream.stride, indexSource: g::SceGxmIndexSource_SCE_GXM_INDEX_SOURCE_INDEX_16BIT as u16 }];
        let mut vp: *mut g::SceGxmVertexProgram = core::ptr::null_mut();
        let r = g::sceGxmShaderPatcherCreateVertexProgram(self.patcher.raw, vs.id, attributes.as_ptr(), attributes.len() as u32, layout.as_ptr(), 1, &mut vp);
        if r < 0 {
            return Err(format!("{name}: sceGxmShaderPatcherCreateVertexProgram 0x{:08x}", r as u32));
        }
        let mut fps = Vec::new();
        for b in blends {
            fps.push(program::fragment_program(self.patcher.raw, &fs, Output::Uchar4, msaa, *b, vs.program())?);
        }
        Ok(Program { vs, fs, vp, fps })
    }

    /// The compiler is only needed while programs build.
    pub fn finish(&mut self) {
        if let Some(c) = self.compiler.take() {
            c.unload();
        }
        if self.live {
            let _ = crate::hostfs::write(&format!("{}/manifest.txt", paths::GXP_HOST), self.manifest.join("\n").as_bytes());
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cull {
    Back,
    None,
}

/// Depth-tested and written: opaque geometry.
pub unsafe fn state_opaque(ctx: *mut g::SceGxmContext, cull: Cull) {
    depth(ctx, g::SceGxmDepthFunc_SCE_GXM_DEPTH_FUNC_LESS_EQUAL, true);
    g::sceGxmSetCullMode(ctx, back(cull));
}

/// Depth-tested, not written: blended geometry.
pub unsafe fn state_blend(ctx: *mut g::SceGxmContext, cull: Cull) {
    depth(ctx, g::SceGxmDepthFunc_SCE_GXM_DEPTH_FUNC_LESS_EQUAL, false);
    g::sceGxmSetCullMode(ctx, back(cull));
}

/// Ethertum's triangles wind counter-clockwise seen from their front (Bevy's convention). GXM tells winding in
/// the projected coordinates before the viewport's flip of y, so that front stays counter-clockwise and the back
/// to cull is clockwise (seen on the console: culling CCW drew the inside of the sea).
fn back(cull: Cull) -> u32 {
    if cull == Cull::Back {
        g::SceGxmCullMode_SCE_GXM_CULL_CW
    } else {
        g::SceGxmCullMode_SCE_GXM_CULL_NONE
    }
}

/// No depth test or write, two-sided: the sky, and vita2d's own draws.
pub unsafe fn state_overlay(ctx: *mut g::SceGxmContext) {
    depth(ctx, g::SceGxmDepthFunc_SCE_GXM_DEPTH_FUNC_ALWAYS, false);
    g::sceGxmSetCullMode(ctx, g::SceGxmCullMode_SCE_GXM_CULL_NONE);
}

unsafe fn depth(ctx: *mut g::SceGxmContext, func: u32, write: bool) {
    let w = if write { g::SceGxmDepthWriteMode_SCE_GXM_DEPTH_WRITE_ENABLED } else { g::SceGxmDepthWriteMode_SCE_GXM_DEPTH_WRITE_DISABLED };
    g::sceGxmSetFrontDepthFunc(ctx, func);
    g::sceGxmSetFrontDepthWriteEnable(ctx, w);
    g::sceGxmSetBackDepthFunc(ctx, func);
    g::sceGxmSetBackDepthWriteEnable(ctx, w);
}

#[inline]
pub unsafe fn draw(ctx: *mut g::SceGxmContext, vb: *const u8, ib: *const u16, count: u32) {
    g::sceGxmSetVertexStream(ctx, 0, vb.cast());
    g::sceGxmDraw(ctx, g::SceGxmPrimitiveType_SCE_GXM_PRIMITIVE_TRIANGLES, g::SceGxmIndexFormat_SCE_GXM_INDEX_FORMAT_U16, ib.cast(), count);
}

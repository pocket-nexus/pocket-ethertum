//! The renderer: the PS Vita's passes (`vita/src/scene.rs`, `shadow.rs`, `post.rs`) as WGSL programs.
//!
//! A frame is three passes, then the interface:
//!
//! 1. **The sun's map** (when the sun is up and the map is on): the terrain's caster meshes and the leaf cards
//!    from the sun, into a depth texture of [`SHADOW_SIZE`]² texels over [`SHADOW_METRES`] around the eye, its
//!    window moved in whole texels so a still view's shadows hold still. The PS Vita draws a sixteenth of its map
//!    a frame; a tab's GPU draws the whole.
//! 2. **The sea's reflection** (when water is in view): the sky and the terrain mirrored in the sea's plane, at
//!    half the screen's size, everything under the plane cut away.
//! 3. **The scene**, in the screen's samples: the sky, the terrain's opaque batches, its blended layers, the
//!    water, the leaf cards (front to back for the view's octant where a chunk's cards are all drawn).
//!
//! Each chunk has a row of a table (`mesh::ChunkInfo`); a draw names it by its instance, the terrain's with the
//! material's index too (row × 32 + material), so one bind group serves every draw of a pipeline.

use std::collections::HashMap;

use ether_voxel::glam::{IVec3, Mat4, Vec3, Vec4};
use pocket_web_wgpu::gpu::{Frame, Gpu, Screen, DEPTH};
use pocket_web_wgpu::overlay::Overlay;
use pocket_web_wgpu::wgpu::{self, util::DeviceExt, Buffer, BindGroup, RenderPipeline, Texture, TextureFormat, TextureView};

use crate::mesh::{ChunkGpu, ChunkInfo, CASTER_STRIDE, FOLIAGE_STRIDE, TERRAIN_STRIDE, WATER_STRIDE};
use crate::pack::Pictures;

/// Rows of the chunks' table.
pub const SLOTS: u32 = 8192;
/// Texels on a side of the sun's map, and metres it covers.
pub const SHADOW_SIZE: u32 = 2048;
pub const SHADOW_METRES: f32 = 128.0;
/// Metres of distance along the sun's rays the map covers.
const SHADOW_DEPTH: f32 = 512.0;
/// Metres a terrain fragment and a leaf card's test point may lie behind the nearest caster and be lit.
const TERRAIN_BIAS: f32 = 0.08;
const LEAF_BIAS: f32 = 0.25;

/// The frame's numbers (`Frame` in common.wgsl), 336 bytes.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    vp: [f32; 16],
    inv_vp: [f32; 16],
    light_vp: [f32; 16],
    eye: [f32; 4],
    sun: [f32; 4],
    zenith: [f32; 4],
    nadir: [f32; 4],
    horizon: [f32; 4],
    shown: [f32; 4],
    flags: [f32; 4],
    look: [f32; 4],
    screen: [f32; 4],
}

/// What a frame shows.
pub struct View {
    pub eye: Vec3,
    /// The direction the eye looks in.
    pub forward: Vec3,
    /// Projection × the view's rotation (positions arrive from the eye).
    pub vp: Mat4,
    /// Towards the sun, and its light on a face that faces it (`ether_look::SUN_LIGHT` × the reference's sin²).
    pub sun_dir: Vec3,
    pub sun_light: f32,
    /// Daylight on the sky picture.
    pub day: f32,
    pub seconds: f32,
    /// Chunks farther than this (in voxels, on the larger axis) are not drawn.
    pub reach: f32,
    pub shadows: bool,
    /// The water's reflectance looking down, ripples' tilt, their shift of the reflection, the sun's glint.
    pub water_look: [f32; 4],
    /// The share of every chunk's leaf cards drawn (1: all).
    pub density: f32,
    /// Which passes to draw: terrain, water, foliage (all, unless a check turns one off).
    pub show: [bool; 3],
}

/// What a frame drew.
#[derive(Clone, Copy, Default, Debug)]
pub struct Drawn {
    pub draws: u32,
    pub tris: u32,
    pub chunks: u32,
    pub reflected: u32,
    pub casters: u32,
}

pub struct Renderer {
    pub pictures: Pictures,
    format: TextureFormat,
    samples: u32,
    frame_main: Buffer,
    frame_mirror: Buffer,
    pub table: Buffer,
    group_main: BindGroup,
    group_mirror: BindGroup,
    group_caster: BindGroup,
    terrain_group: BindGroup,
    foliage_group: BindGroup,
    water_group: Option<BindGroup>,
    water_layout: wgpu::BindGroupLayout,
    sky_group: BindGroup,
    repeat: wgpu::Sampler,
    clamped: wgpu::Sampler,
    sky: RenderPipeline,
    sky_mirror: RenderPipeline,
    terrain: RenderPipeline,
    terrain_layer: RenderPipeline,
    terrain_mirror: RenderPipeline,
    water: RenderPipeline,
    foliage: RenderPipeline,
    caster: RenderPipeline,
    cut: RenderPipeline,
    _shadow: Texture,
    shadow_view: TextureView,
    reflection: Option<(Texture, TextureView, TextureView, u32, u32)>,
    /// Bytes of the GPU's memory this renderer made, meshes apart.
    pub bytes: u64,
}

fn layout_entry(binding: u32, visibility: wgpu::ShaderStages, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None }
}

fn texture_entry(binding: u32, dimension: wgpu::TextureViewDimension, kind: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    layout_entry(binding, wgpu::ShaderStages::VERTEX_FRAGMENT, wgpu::BindingType::Texture { sample_type: kind, view_dimension: dimension, multisampled: false })
}

fn sampler_entry(binding: u32, kind: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    layout_entry(binding, wgpu::ShaderStages::VERTEX_FRAGMENT, wgpu::BindingType::Sampler(kind))
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    layout_entry(binding, wgpu::ShaderStages::VERTEX_FRAGMENT, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None })
}

/// The six planes of a view-projection whose depth runs 0…1, as `(normal, d)` with points inside where
/// `n·p + d >= 0`.
fn frustum(m: &Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2]
}

fn visible(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| {
        let c = Vec3::new(if p.x >= 0.0 { max.x } else { min.x }, if p.y >= 0.0 { max.y } else { min.y }, if p.z >= 0.0 { max.z } else { min.z });
        p.x * c.x + p.y * c.y + p.z * c.z + p.w >= 0.0
    })
}

/// The sun's light space: two axes across its rays and the direction towards it.
fn basis(sun: Vec3) -> (Vec3, Vec3, Vec3) {
    let l = sun.normalize();
    let reference = if l.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let u = l.cross(reference).normalize();
    let v = u.cross(l).normalize();
    (u, v, l)
}

impl Renderer {
    pub fn new(gpu: &Gpu, format: TextureFormat, samples: u32, pictures: Pictures) -> Renderer {
        let device = &gpu.device;
        let frame_buffer = |label| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: std::mem::size_of::<FrameUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let frame_main = frame_buffer("frame");
        let frame_mirror = frame_buffer("frame mirrored");
        let table = device.create_buffer(&wgpu::BufferDescriptor { label: Some("chunks"), size: SLOTS as u64 * 64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("repeat"), address_mode_u: wgpu::AddressMode::Repeat, address_mode_v: wgpu::AddressMode::Repeat, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, mipmap_filter: wgpu::FilterMode::Linear, anisotropy_clamp: 8, ..Default::default() });
        let clamped = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("clamped"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, mipmap_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let compare = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("sun"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, compare: Some(wgpu::CompareFunction::LessEqual), ..Default::default() });

        // The sun's map, and a texel of depth that stands for it in the pass that draws it.
        let depth_texture = |label, size| device.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: TextureFormat::Depth32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] });
        let shadow = depth_texture("sun's map", SHADOW_SIZE);
        let shadow_view = shadow.create_view(&Default::default());
        let stand_in = depth_texture("no sun's map", 1);
        let stand_in_view = stand_in.create_view(&Default::default());

        let group0_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                uniform_entry(0),
                texture_entry(1, wgpu::TextureViewDimension::D2, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(2, wgpu::SamplerBindingType::Filtering),
                texture_entry(3, wgpu::TextureViewDimension::D2, wgpu::TextureSampleType::Depth),
                sampler_entry(4, wgpu::SamplerBindingType::Comparison),
                layout_entry(5, wgpu::ShaderStages::VERTEX_FRAGMENT, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }),
            ],
        });
        let group0 = |label, frame: &Buffer, sun: &TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &group0_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: frame.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&pictures.ring) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&clamped) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(sun) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&compare) },
                    wgpu::BindGroupEntry { binding: 5, resource: table.as_entire_binding() },
                ],
            })
        };
        let group_main = group0("frame", &frame_main, &shadow_view);
        let group_mirror = group0("frame mirrored", &frame_mirror, &stand_in_view);
        let group_caster = group0("frame of the sun's map", &frame_main, &stand_in_view);

        // The terrain's materials: the texture array, and each material's mean texel squared.
        let terrain_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &[texture_entry(0, wgpu::TextureViewDimension::D2Array, wgpu::TextureSampleType::Float { filterable: true }), sampler_entry(1, wgpu::SamplerBindingType::Filtering), uniform_entry(2)],
        });
        let mut albedo = [[0f32; 4]; 32];
        for (m, c) in pictures.average.iter().enumerate().take(32) {
            let t = |b: u8| (b as f32 / 255.0) * (b as f32 / 255.0);
            albedo[m] = [t(c[0]), t(c[1]), t(c[2]), 0.0];
        }
        let materials = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("materials"), contents: bytemuck::cast_slice(&albedo), usage: wgpu::BufferUsages::UNIFORM });
        let terrain_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain"),
            layout: &terrain_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&pictures.terrain) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&repeat) },
                wgpu::BindGroupEntry { binding: 2, resource: materials.as_entire_binding() },
            ],
        });
        let picture_layout = |label, dimension| device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries: &[texture_entry(0, dimension, wgpu::TextureSampleType::Float { filterable: true }), sampler_entry(1, wgpu::SamplerBindingType::Filtering)] });
        let foliage_layout = picture_layout("foliage", wgpu::TextureViewDimension::D2);
        let foliage_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("foliage"),
            layout: &foliage_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&pictures.foliage) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&clamped) }],
        });
        let sky_layout = picture_layout("sky", wgpu::TextureViewDimension::Cube);
        let sky_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky"),
            layout: &sky_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&pictures.sky) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&clamped) }],
        });
        let water_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[texture_entry(0, wgpu::TextureViewDimension::D2, wgpu::TextureSampleType::Float { filterable: true }), sampler_entry(1, wgpu::SamplerBindingType::Filtering), texture_entry(2, wgpu::TextureViewDimension::D2, wgpu::TextureSampleType::Float { filterable: true })],
        });

        let common = include_str!("shaders/common.wgsl");
        let module = |label, source: &str| device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(format!("{common}\n{source}").into()) });
        let terrain_module = module("terrain", include_str!("shaders/terrain.wgsl"));
        let foliage_module = module("foliage", include_str!("shaders/foliage.wgsl"));
        let water_module = module("water", include_str!("shaders/water.wgsl"));
        let sky_module = module("sky", include_str!("shaders/sky.wgsl"));
        let shadow_module = module("shadow", include_str!("shaders/shadow.wgsl"));
        let pipeline_layout = |label, second: &wgpu::BindGroupLayout| device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[&group0_layout, second], push_constant_ranges: &[] });
        let terrain_pl = pipeline_layout("terrain", &terrain_layout);
        let foliage_pl = pipeline_layout("foliage", &foliage_layout);
        let water_pl = pipeline_layout("water", &water_layout);
        let sky_pl = pipeline_layout("sky", &sky_layout);

        use wgpu::VertexFormat as F;
        let attributes = |list: &'static [(u32, F, u64)]| -> Vec<wgpu::VertexAttribute> { list.iter().map(|&(location, format, offset)| wgpu::VertexAttribute { format, offset, shader_location: location }).collect() };
        let terrain_attrs = attributes(&[(0, F::Sint16x4, 0), (1, F::Snorm8x4, 8), (2, F::Unorm8x4, 12)]);
        let foliage_attrs = attributes(&[(0, F::Sint16x4, 0), (1, F::Unorm16x2, 8), (2, F::Unorm8x4, 12), (3, F::Snorm8x4, 16)]);
        let water_attrs = attributes(&[(0, F::Sint16x4, 0), (1, F::Snorm8x4, 8)]);
        let caster_attrs = attributes(&[(0, F::Sint16x4, 0)]);
        let cut_attrs = attributes(&[(0, F::Sint16x4, 0), (1, F::Unorm16x2, 8)]);

        struct Shape<'a> {
            label: &'a str,
            layout: &'a wgpu::PipelineLayout,
            module: &'a wgpu::ShaderModule,
            vs: &'a str,
            fs: Option<&'a str>,
            stride: u64,
            attrs: &'a [wgpu::VertexAttribute],
            cull: Option<wgpu::Face>,
            depth: (bool, wgpu::CompareFunction),
            blend: Option<wgpu::BlendState>,
            /// The colour target's format and samples; `None` for a pass of depth alone.
            colour: Option<(TextureFormat, u32)>,
            depth_format: TextureFormat,
            bias: wgpu::DepthBiasState,
        }
        let make = |s: Shape| -> RenderPipeline {
            let buffers = [wgpu::VertexBufferLayout { array_stride: s.stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: s.attrs }];
            let targets = s.colour.map(|(format, _)| [Some(wgpu::ColorTargetState { format, blend: s.blend, write_mask: wgpu::ColorWrites::ALL })]);
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(s.label),
                layout: Some(s.layout),
                vertex: wgpu::VertexState { module: s.module, entry_point: Some(s.vs), compilation_options: Default::default(), buffers: if s.stride == 0 { &[] } else { &buffers } },
                fragment: s.fs.map(|fs| wgpu::FragmentState { module: s.module, entry_point: Some(fs), compilation_options: Default::default(), targets: targets.as_ref().map_or(&[][..], |t| &t[..]) }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, front_face: wgpu::FrontFace::Ccw, cull_mode: s.cull, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: s.depth_format, depth_write_enabled: s.depth.0, depth_compare: s.depth.1, stencil: Default::default(), bias: s.bias }),
                multisample: wgpu::MultisampleState { count: s.colour.map_or(1, |c| c.1), mask: !0, alpha_to_coverage_enabled: false },
                multiview: None,
                cache: None,
            })
        };
        let none = wgpu::DepthBiasState::default();
        let main = Some((format, samples));
        let mirror = Some((format, 1));
        let less = (true, wgpu::CompareFunction::LessEqual);
        let sky = make(Shape { label: "sky", layout: &sky_pl, module: &sky_module, vs: "sky_vs", fs: Some("sky_fs"), stride: 0, attrs: &[], cull: None, depth: (false, wgpu::CompareFunction::Always), blend: None, colour: main, depth_format: DEPTH, bias: none });
        let sky_mirror = make(Shape { label: "sky mirrored", layout: &sky_pl, module: &sky_module, vs: "sky_vs", fs: Some("sky_fs"), stride: 0, attrs: &[], cull: None, depth: (false, wgpu::CompareFunction::Always), blend: None, colour: mirror, depth_format: DEPTH, bias: none });
        // Ethertum's triangles wind counter-clockwise seen from their front (Bevy's convention); a mirror image
        // winds the other way.
        let terrain = make(Shape { label: "terrain", layout: &terrain_pl, module: &terrain_module, vs: "terrain_vs", fs: Some("terrain_fs"), stride: TERRAIN_STRIDE, attrs: &terrain_attrs, cull: Some(wgpu::Face::Back), depth: less, blend: None, colour: main, depth_format: DEPTH, bias: none });
        let terrain_layer = make(Shape { label: "terrain layers", layout: &terrain_pl, module: &terrain_module, vs: "terrain_vs", fs: Some("terrain_fs"), stride: TERRAIN_STRIDE, attrs: &terrain_attrs, cull: Some(wgpu::Face::Back), depth: (false, wgpu::CompareFunction::LessEqual), blend: Some(wgpu::BlendState::ALPHA_BLENDING), colour: main, depth_format: DEPTH, bias: none });
        let terrain_mirror = make(Shape { label: "terrain mirrored", layout: &terrain_pl, module: &terrain_module, vs: "terrain_vs", fs: Some("terrain_fs"), stride: TERRAIN_STRIDE, attrs: &terrain_attrs, cull: Some(wgpu::Face::Front), depth: less, blend: None, colour: mirror, depth_format: DEPTH, bias: none });
        let water = make(Shape { label: "water", layout: &water_pl, module: &water_module, vs: "water_vs", fs: Some("water_fs"), stride: WATER_STRIDE, attrs: &water_attrs, cull: Some(wgpu::Face::Back), depth: less, blend: None, colour: main, depth_format: DEPTH, bias: none });
        let foliage = make(Shape { label: "foliage", layout: &foliage_pl, module: &foliage_module, vs: "foliage_vs", fs: Some("foliage_fs"), stride: FOLIAGE_STRIDE, attrs: &foliage_attrs, cull: None, depth: less, blend: None, colour: main, depth_format: DEPTH, bias: none });
        // The terrain's casters draw the faces turned away from the sun (as the PS Vita's do, `Shadow::cull`): a
        // lit surface is never behind its own caster, which stands above it in places (the caster mesh is coarser
        // than the drawn one). The casters are pushed back a little in the map, by their slope, too.
        let push = wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 };
        let caster = make(Shape { label: "casters", layout: &foliage_pl, module: &shadow_module, vs: "caster_vs", fs: None, stride: CASTER_STRIDE, attrs: &caster_attrs, cull: Some(wgpu::Face::Front), depth: less, blend: None, colour: None, depth_format: TextureFormat::Depth32Float, bias: push });
        let cut = make(Shape { label: "leaf casters", layout: &foliage_pl, module: &shadow_module, vs: "cut_vs", fs: Some("cut_fs"), stride: FOLIAGE_STRIDE, attrs: &cut_attrs, cull: None, depth: less, blend: None, colour: None, depth_format: TextureFormat::Depth32Float, bias: push });

        let bytes = (SHADOW_SIZE as u64).pow(2) * 4 + SLOTS as u64 * 64 + pictures.bytes as u64;
        Renderer {
            pictures,
            format,
            samples,
            frame_main,
            frame_mirror,
            table,
            group_main,
            group_mirror,
            group_caster,
            terrain_group,
            foliage_group,
            water_group: None,
            water_layout,
            sky_group,
            repeat,
            clamped,
            sky,
            sky_mirror,
            terrain,
            terrain_layer,
            terrain_mirror,
            water,
            foliage,
            caster,
            cut,
            _shadow: shadow,
            shadow_view,
            reflection: None,
            bytes,
        }
    }

    /// Writes a chunk's row of the table.
    pub fn place(&self, gpu: &Gpu, slot: u32, at: IVec3) {
        gpu.queue.write_buffer(&self.table, slot as u64 * 64, bytemuck::bytes_of(&ChunkInfo::of(at)));
    }

    /// The reflection's target for a screen of `width` × `height`, and the water's bind group that reads it.
    fn reflection_for(&mut self, gpu: &Gpu, width: u32, height: u32) {
        let (w, h) = ((width / 2).max(1), (height / 2).max(1));
        if matches!(&self.reflection, Some((_, _, _, rw, rh)) if *rw == w && *rh == h) {
            return;
        }
        let colour = gpu.device.create_texture(&wgpu::TextureDescriptor { label: Some("reflection"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: self.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] });
        let depth = gpu.device.create_texture(&wgpu::TextureDescriptor { label: Some("reflection depth"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: DEPTH, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] });
        let view = colour.create_view(&Default::default());
        self.water_group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water"),
            layout: &self.water_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.pictures.water) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.repeat) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view) },
            ],
        }));
        let _ = &self.clamped;
        self.reflection = Some((colour, view, depth.create_view(&Default::default()), w, h));
    }

    /// The sun's map for the eye at `eye`: its matrix (a point from the eye to the map's coordinates and depth),
    /// and the axes and window that choose the chunks that cast into it.
    fn light(&self, eye: Vec3, sun: Vec3) -> (Mat4, (Vec3, Vec3, f32, f32)) {
        let (u, v, l) = basis(sun);
        let texel = SHADOW_METRES / SHADOW_SIZE as f32;
        let snap = |x: f32| (x / texel).round() * texel;
        let (eu, ev) = (eye.dot(u), eye.dot(v));
        let (cu, cv) = (snap(eu), snap(ev));
        let s = 2.0 / SHADOW_METRES;
        let rows = [Vec4::new(u.x * s, u.y * s, u.z * s, (eu - cu) * s), Vec4::new(v.x * s, v.y * s, v.z * s, (ev - cv) * s), Vec4::new(-l.x / SHADOW_DEPTH, -l.y / SHADOW_DEPTH, -l.z / SHADOW_DEPTH, 0.5), Vec4::new(0.0, 0.0, 0.0, 1.0)];
        (Mat4::from_cols(rows[0], rows[1], rows[2], rows[3]).transpose(), (u, v, cu - eu, cv - ev))
    }

    fn write_frame(&self, gpu: &Gpu, view: &View, light_vp: &Mat4, width: u32, height: u32, mirror: bool) {
        let light = &self.pictures.light;
        let h = self.pictures.horizon;
        let u = FrameUniform {
            vp: view.vp.to_cols_array(),
            inv_vp: view.vp.inverse().to_cols_array(),
            light_vp: light_vp.to_cols_array(),
            eye: [view.eye.x, view.eye.y, view.eye.z, view.seconds],
            sun: [view.sun_dir.x, view.sun_dir.y, view.sun_dir.z, view.sun_light],
            zenith: [light.zenith[0], light.zenith[1], light.zenith[2], 1.0 / view.reach],
            nadir: [light.nadir[0], light.nadir[1], light.nadir[2], 0.0],
            horizon: [light.horizon[0], light.horizon[1], light.horizon[2], view.day],
            shown: [h[0], h[1], h[2], 0.0],
            flags: [mirror as u32 as f32, (view.shadows && view.sun_light > 0.0) as u32 as f32, TERRAIN_BIAS / SHADOW_DEPTH, LEAF_BIAS / SHADOW_DEPTH],
            look: view.water_look,
            screen: [width as f32, height as f32, 1.0 / width as f32, 1.0 / height as f32],
        };
        gpu.queue.write_buffer(if mirror { &self.frame_mirror } else { &self.frame_main }, 0, bytemuck::bytes_of(&u));
    }

    /// One frame onto `screen`, with the interface's picture over it.
    pub fn frame(&mut self, gpu: &Gpu, screen: &Screen, overlay: &Overlay, chunks: &HashMap<IVec3, ChunkGpu>, view: &View) -> Result<Drawn, String> {
        let mut drawn = Drawn::default();
        self.reflection_for(gpu, screen.width, screen.height);
        let sun_up = view.shadows && view.sun_light > 0.0 && view.sun_dir.y > 0.02;
        let (light_vp, (lu, lv, wu, wv)) = self.light(view.eye, view.sun_dir);
        self.write_frame(gpu, view, &light_vp, screen.width, screen.height, false);
        let (rw, rh) = self.reflection.as_ref().map(|r| (r.3, r.4)).unwrap_or((1, 1));
        self.write_frame(gpu, view, &light_vp, rw, rh, true);

        // The chunks in view, nearest first.
        let planes = frustum(&view.vp);
        let mut order: Vec<(f32, &ChunkGpu)> = Vec::new();
        let mut water_seen = false;
        for c in chunks.values() {
            let min = (c.at * 16).as_vec3() - view.eye;
            if (min + 8.0).abs().max_element() - 8.0 > view.reach || !visible(&planes, min, min + 16.0) {
                continue;
            }
            water_seen |= c.water.is_some();
            order.push(((min + 8.0).length_squared(), c));
        }
        order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        drawn.chunks = order.len() as u32;

        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        // 1. The sun's map.
        if sun_up {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sun's map"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.shadow_view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.group_caster, &[]);
            pass.set_bind_group(1, &self.foliage_group, &[]);
            let half = SHADOW_METRES * 0.5;
            let casts = |c: &ChunkGpu| {
                // The chunk's box across the sun's rays against the map's window.
                let origin = (c.at * 16).as_vec3() - view.eye;
                let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
                for k in 0..8 {
                    let p = origin + Vec3::new((k & 1) as f32, ((k >> 1) & 1) as f32, (k >> 2) as f32) * 16.0;
                    let (a, b) = (p.dot(lu), p.dot(lv));
                    lo = (lo.0.min(a), lo.1.min(b));
                    hi = (hi.0.max(a), hi.1.max(b));
                }
                hi.0 >= wu - half && lo.0 <= wu + half && hi.1 >= wv - half && lo.1 <= wv + half
            };
            let casting: Vec<&ChunkGpu> = chunks.values().filter(|c| (c.caster.is_some() || c.foliage.is_some()) && casts(c)).collect();
            pass.set_pipeline(&self.caster);
            for c in &casting {
                if let Some(p) = &c.caster {
                    pass.set_vertex_buffer(0, p.vb.slice(..));
                    pass.set_index_buffer(p.ib.slice(..), wgpu::IndexFormat::Uint16);
                    pass.draw_indexed(0..p.count, 0, c.slot..c.slot + 1);
                    drawn.draws += 1;
                    drawn.casters += p.count / 3;
                }
            }
            pass.set_pipeline(&self.cut);
            for c in &casting {
                if let Some(p) = &c.foliage {
                    pass.set_vertex_buffer(0, p.vb.slice(..));
                    pass.set_index_buffer(p.ib.slice(..), wgpu::IndexFormat::Uint16);
                    pass.draw_indexed(0..p.count, 0, c.slot..c.slot + 1);
                    drawn.draws += 1;
                    drawn.casters += p.count / 3;
                }
            }
        }

        // 2. The sea's reflection: the sky and the terrain that stands above the sea, mirrored in its plane.
        if water_seen {
            let (_, colour, depth, _, _) = self.reflection.as_ref().expect("reflection target");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("reflection"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: colour, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.group_mirror, &[]);
            pass.set_bind_group(1, &self.sky_group, &[]);
            pass.set_pipeline(&self.sky_mirror);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.terrain_mirror);
            pass.set_bind_group(1, &self.terrain_group, &[]);
            for c in chunks.values() {
                let corner = (c.at * 16).as_vec3();
                if corner.y + 16.0 <= 0.0 {
                    continue;
                }
                let mut min = corner - view.eye;
                min.y = -(corner.y + 16.0) - view.eye.y;
                if (min + 8.0).abs().max_element() - 8.0 > view.reach || !visible(&planes, min, min + 16.0) {
                    continue;
                }
                if let Some(t) = &c.terrain {
                    pass.set_vertex_buffer(0, t.vb.slice(..));
                    pass.set_index_buffer(t.ib.slice(..), wgpu::IndexFormat::Uint16);
                    for b in &c.batches {
                        if b.opaque.1 > 0 {
                            let m = (b.material.max(1) - 1) as u32;
                            pass.draw_indexed(b.opaque.0..b.opaque.0 + b.opaque.1, b.base as i32, c.slot * 32 + m..c.slot * 32 + m + 1);
                            drawn.draws += 1;
                            drawn.reflected += b.opaque.1 / 3;
                        }
                    }
                }
            }
        }

        // 3. The scene.
        let target: Frame = screen.frame(gpu)?;
        {
            let mut pass = target.pass(&mut encoder, [0.0, 0.0, 0.0]);
            pass.set_bind_group(0, &self.group_main, &[]);
            pass.set_bind_group(1, &self.sky_group, &[]);
            pass.set_pipeline(&self.sky);
            pass.draw(0..3, 0..1);
            // Terrain: one draw per material; then the layers where materials meet, blended.
            pass.set_bind_group(1, &self.terrain_group, &[]);
            for (pipeline, layers) in [(&self.terrain, false), (&self.terrain_layer, true)] {
                if !view.show[0] {
                    break;
                }
                pass.set_pipeline(pipeline);
                for (_, c) in &order {
                    let Some(t) = &c.terrain else { continue };
                    pass.set_vertex_buffer(0, t.vb.slice(..));
                    pass.set_index_buffer(t.ib.slice(..), wgpu::IndexFormat::Uint16);
                    for b in &c.batches {
                        let (first, count) = if layers { b.layer } else { b.opaque };
                        if count == 0 {
                            continue;
                        }
                        let m = (b.material.max(1) - 1) as u32;
                        pass.draw_indexed(first..first + count, b.base as i32, c.slot * 32 + m..c.slot * 32 + m + 1);
                        drawn.draws += 1;
                        drawn.tris += count / 3;
                    }
                }
            }
            // Water: the reflection read at each point's place on the screen.
            if let Some(group) = self.water_group.as_ref().filter(|_| view.show[1]) {
                pass.set_pipeline(&self.water);
                pass.set_bind_group(1, group, &[]);
                for (_, c) in &order {
                    let Some(w) = &c.water else { continue };
                    pass.set_vertex_buffer(0, w.vb.slice(..));
                    pass.set_index_buffer(w.ib.slice(..), wgpu::IndexFormat::Uint16);
                    pass.draw_indexed(0..w.count, 0, c.slot..c.slot + 1);
                    drawn.draws += 1;
                    drawn.tris += w.count / 3;
                }
            }
            // Foliage: two-sided cards. A chunk drawn whole takes the copy of its cards sorted front to back for
            // the view's octant, so a near card lets the depth test reject the cut-out cards behind it.
            pass.set_pipeline(&self.foliage);
            pass.set_bind_group(1, &self.foliage_group, &[]);
            let f = view.forward;
            let order = if view.show[2] { &order[..] } else { &[] };
            let octant = (f.x < 0.0) as u64 | ((f.y < 0.0) as u64) << 1 | ((f.z < 0.0) as u64) << 2;
            for (_, c) in order {
                let Some(p) = &c.foliage else { continue };
                let count = (((p.count / 6) as f32 * view.density).ceil() as u32 * 6).min(p.count);
                match &c.foliage_sorted {
                    Some(sorted) if count == p.count => {
                        let bytes = p.count as u64 * 2;
                        pass.set_index_buffer(sorted.slice(octant * bytes..octant * bytes + bytes), wgpu::IndexFormat::Uint16);
                    }
                    _ => pass.set_index_buffer(p.ib.slice(..), wgpu::IndexFormat::Uint16),
                }
                pass.set_vertex_buffer(0, p.vb.slice(..));
                pass.draw_indexed(0..count, 0, c.slot..c.slot + 1);
                drawn.draws += 1;
                drawn.tris += count / 3;
            }
        }
        overlay.draw(&mut encoder, &target);
        gpu.queue.submit([encoder.finish()]);
        target.present();
        let _ = self.samples;
        Ok(drawn)
    }
}

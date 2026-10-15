use std::{mem, ops::{Bound, RangeBounds}};

use bytemuck::{Pod, Zeroable};
use skrifa::{MetadataProvider, instance::Size};
use wgpu::{
    util::{self, BufferInitDescriptor, DeviceExt}, vertex_attr_array, BindGroup, BindGroupDescriptor,
    BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, 
    BindingType, BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer, BufferBinding, 
    BufferBindingType, BufferSize, BufferUsages, ColorTargetState, ColorWrites,
    Device, FilterMode, FragmentState, FrontFace, MultisampleState, PipelineLayoutDescriptor, 
    PrimitiveState, PrimitiveTopology, RenderPass, RenderPipeline, RenderPipelineDescriptor, 
    SamplerBindingType, SamplerDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages, 
    TextureSampleType, TextureViewDimension, VertexAttribute, VertexBufferLayout,
    VertexFormat, VertexState, VertexStepMode
};

use crate::{ortho::orthographic_projection_matrix, typewriter::Paragraph, FontStore};

mod bitmap;

/// Glyph rendering strategy.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RenderMode {
    /// Analytic coverage for scaling, animation, large sizes and LCD.
    #[default]
    Analytic,
    /// GPU rasterized coverage for repeated drawing at the same size.
    /// Snaps pen x to quarter pixels and each line’s baseline to whole pixels. LCD uses Analytic.
    Cached,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Horizontal physical subpixel order. LCD requires dual-source blending.
#[non_exhaustive]
pub enum SubpixelOrder {
    /// Grayscale antialiasing (the default).
    #[default]
    None,
    /// Red, green, blue from left to right.
    Rgb,
    /// Blue, green, red from left to right.
    Bgr,
}

/// GPU pipeline and prepared glyph instances for a target format.
/// Screen coordinates are physical pixels, origin top left, y downward.
pub struct TextRenderer {
    bitmap: Option<bitmap::BitmapCache>,
    bitmap_limits: Option<(u32, u32)>,
    prepared_cached: Vec<Paragraph>,
    constant_layout: BindGroupLayout,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
    cached: Vec<bool>,
    subpixel_order: SubpixelOrder,
    pipeline: RenderPipeline,
    uniforms: Buffer,
    has_valid_size: bool,
    vertices: Buffer,
    indices: Buffer,
    instances_buffer: Option<Buffer>,
    instances: Vec<Instance>,
    paragraph_offsets: Vec<u32>,
    constants: BindGroup,
    texture: BindGroup,
    texture_version: usize,
    texture_layout: BindGroupLayout,
}

impl TextRenderer {
    /// Create a grayscale renderer for `format` and the store's atlas.
    /// Call [`Self::resize`] before the first render and on every viewport change.
    pub fn new(device: &Device, format: wgpu::TextureFormat, store: &FontStore) -> Self {
        Self::with_subpixel_order(device, format, store, SubpixelOrder::None)
    }

    /// Create an LCD renderer when `DUAL_SOURCE_BLENDING` is enabled on the device,
    /// otherwise fall back to grayscale. Use an opaque background and an unscaled,
    /// unrotated pixel transform. Call [`Self::resize`] before the first render
    /// and on every viewport change.
    pub fn with_subpixel_order(
        device: &Device,
        format: wgpu::TextureFormat,
        store: &FontStore,
        order: SubpixelOrder,
    ) -> Self {
        let atlas = store.atlas();
        let subpixel_order = if device.features().contains(wgpu::Features::DUAL_SOURCE_BLENDING) {
            order
        } else {
            SubpixelOrder::None
        };
        let lcd = subpixel_order != SubpixelOrder::None;
        let screen_size = [0, 0];

        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("Text sampler"),
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0f32,
            lod_max_clamp: 0f32,
            ..Default::default()
        });

        let constant_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Text constants layout"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size:BufferSize::new(
                            mem::size_of::<Params>() as u64,
                        ),
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(
                        SamplerBindingType::NonFiltering,
                    ),
                    count: None,
                }
            ],
        });

        let uniforms = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Text uniforms buffer"),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            contents: bytemuck::bytes_of(&Params {
                screen_resolution: Resolution {
                    width: screen_size[0],
                    height: screen_size[1],
                },
                _pad: [0, 0],
                transform: [0.; 16]
            }),
        });

        let constant_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Text texture bind group"),
            layout: &constant_layout,
            entries:  &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer(
                        BufferBinding {
                            buffer: &uniforms,
                            offset: 0,
                            size: None,
                        },
                    ),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&sampler),
                },
            ],
        });

        let texture_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Text texture layout"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: false },
                        view_dimension: TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: BufferSize::new(4),
                    },
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("text pipeline layout"),
            bind_group_layouts: &[Some(&constant_layout), Some(&texture_layout)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Text shader"),
            source: ShaderSource::Wgsl(if lcd {
                std::borrow::Cow::Owned(format!("enable dual_source_blending;\n{}\n{}",
                    include_str!("shader.wgsl"), include_str!("lcd.wgsl")))
            } else {
                std::borrow::Cow::Borrowed(include_str!("shader.wgsl"))
            }),
        });

        let lcd_constants = [("LCD_BGR", if subpixel_order == SubpixelOrder::Bgr { 1. } else { 0. })];
        let descriptor = RenderPipelineDescriptor {
            label: Some("Text pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some(if lcd { "vs_lcd" } else { "vs_main" }),
                compilation_options: Default::default(),
                buffers: &instance_layouts(),
            },
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                front_face: FrontFace::Cw,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some(if lcd { "fs_lcd" } else { "fs_main" }),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: if lcd { &lcd_constants } else { &[] },
                    ..Default::default()
                },
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState {
                        color: BlendComponent {
                            src_factor: if lcd { BlendFactor::Src1 } else { BlendFactor::SrcAlpha },
                            dst_factor: if lcd { BlendFactor::OneMinusSrc1 } else { BlendFactor::OneMinusSrcAlpha },
                            operation: BlendOperation::Add,
                        },
                        alpha: BlendComponent {
                            src_factor: BlendFactor::One,
                            dst_factor: BlendFactor::OneMinusSrcAlpha,
                            operation: BlendOperation::Add,
                        },
                    }),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        };

        let pipeline = device.create_render_pipeline(&descriptor);

        let vertices = device.create_buffer_init(&util::BufferInitDescriptor {
            label: Some("Text vertex buffer"),
            contents: bytemuck::cast_slice(&VERTICES),
            usage: BufferUsages::VERTEX,
        });

        let indices = device.create_buffer_init(&util::BufferInitDescriptor {
            label: Some("Text indice buffer"),
            contents: bytemuck::cast_slice(&INDICES),
            usage: BufferUsages::INDEX,
        });

        let texture = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Text texture atlas bind group"),
            layout: &texture_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(atlas.view()),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: atlas.offsets_buffer().as_entire_binding(),
                },
            ],
        });

        Self {
            bitmap: None,
            bitmap_limits: None,
            prepared_cached: Vec::new(),
            constant_layout,
            sampler,
            format,
            cached: Vec::new(),
            subpixel_order,
            pipeline,
            uniforms,
            has_valid_size: false,
            vertices,
            indices,
            instances_buffer: None,
            instances: Vec::new(),
            paragraph_offsets: Vec::new(),
            constants: constant_bind_group,
            texture,
            texture_version: atlas.version,
            texture_layout,
        }
    }

    #[cfg(feature = "unstable-internals")]
    pub(crate) fn set_bitmap_cache_limits(&mut self, device: &Device, initial: u32, max: u32) {
        self.bitmap_limits = Some((initial, max));
        self.prepared_cached.clear();
        if let Some(bitmap) = &mut self.bitmap { bitmap.reset(device, initial, max); }
    }
    #[cfg(feature = "unstable-internals")]
    pub(crate) fn bitmap_cache_size(&self) -> u32 { self.bitmap.as_ref().map_or(0, |b| b.size) }
    #[cfg(feature = "unstable-internals")]
    pub(crate) fn bitmap_cache_counts(&self) -> (usize, usize) { (self.bitmap.as_ref().map_or(0, |b| b.count()), self.cached.iter().filter(|&&x| x).count()) }
    /// Whether this renderer actually uses LCD antialiasing.
    pub fn lcd_active(&self) -> bool {
        self.subpixel_order != SubpixelOrder::None
    }

    /// Replace prepared instances, skipping unknown fonts, uncached glyphs, and
    /// paragraphs whose size is not finite or is <= 0.0. Skipped paragraphs keep
    /// their index in the input sequence. An empty sequence clears the draw batch.
    /// Refresh the atlas binding after loading fonts and reuse the instance buffer
    /// when its capacity suffices.
    ///
    /// Call `prepare` once per queue submit with every paragraph of the frame.
    /// A second `prepare` before the submit can change what earlier `render` calls
    /// of that submit draw. Use [`Self::render_range`] to draw subsets at different
    /// points of a pass or in different passes.
    pub fn prepare<'a>(
        &mut self,
        device: &Device,
        queue: &wgpu::Queue,
        paragraphs: impl IntoIterator<Item = &'a Paragraph>,
        store: &FontStore,
    ) {
        let atlas = store.atlas();
        let atlas_changed = self.texture_version != atlas.version;
        if atlas_changed {
            self.texture = device.create_bind_group(&BindGroupDescriptor {
                label: Some("Text texture atlas bind group"),
                layout: &self.texture_layout,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(atlas.view()),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: atlas.offsets_buffer().as_entire_binding(),
                    },
                ],
            });
            self.texture_version = atlas.version;
        }
        let paragraphs: Vec<_> = paragraphs.into_iter().collect();
        let has_cached = !self.lcd_active() && paragraphs.iter().any(|p| p.render_mode() == RenderMode::Cached);
        // Paragraphs are mutable and cloneable: compare their drawing inputs, not addresses.
        // A warm hit avoids glyph hashing, instance building, uploads and queue submissions.
        if has_cached && !atlas_changed && paragraphs.len() == self.prepared_cached.len()
            && paragraphs.iter().zip(&self.prepared_cached).all(|(a,b)| same_paragraph(a,b)) {
            return;
        }
        if has_cached && self.bitmap.is_none() {
            let mut bitmap = bitmap::BitmapCache::new(device, self.format, &self.constant_layout, &self.texture_layout, &self.sampler);
            if let Some((initial, max)) = self.bitmap_limits { bitmap.reset(device, initial, max); }
            self.bitmap = Some(bitmap);
        }
        self.prepared_cached.clear();
        if has_cached { self.prepared_cached.extend(paragraphs.iter().map(|p| (*p).clone())); }
        let used = bitmap::used_keys(&paragraphs, !has_cached);
        let mut encoder = None;
        let mut jobs = Vec::new();
        self.cached.clear();
        self.instances.clear();
        self.paragraph_offsets.clear();

        paragraphs.into_iter().for_each(|paragraph| {
            self.paragraph_offsets.push(self.instances.len() as u32);
            if !paragraph.size().is_finite() || paragraph.size() <= 0.0 {
                return;
            }
            let Some(font) = store.get(paragraph.font()) else { return; };

            let units_per_em = font.face().metrics(Size::unscaled(), &font.location).units_per_em as f32;

            let baseline =
                paragraph.position()[1] + font.metrics.ascent * (paragraph.size() / units_per_em);
            let mut glyph_x = paragraph.position()[0];

            paragraph.glyphs().iter().for_each(|shaped| {
                if let Some(glyph) = font.glyph_cache.get(&(shaped.glyph.0 as u32))
                    .filter(|glyph| glyph.bbox.width() > 0 && glyph.bbox.height() > 0) {
                    let glyph_y = paragraph.position()[1] + ((glyph.y_offset as f32 + font.ascender_delta) / units_per_em * paragraph.size()) + (f32::abs(glyph.descent as f32) / units_per_em * paragraph.size());

                    let size = [
                        (glyph.bbox.width() as f32 * paragraph.size() / units_per_em),
                        (glyph.bbox.height() as f32 * paragraph.size() / units_per_em),
                    ];

                    let instance = Instance {
                        _position: [glyph_x + shaped.offset[0] + glyph.left_side_bearing as f32 * paragraph.size() / units_per_em, glyph_y + shaped.offset[1]],
                        _left_side_bearing: glyph.left_side_bearing as f32,
                        _font_size: paragraph.size(),
                        _size: size,
                        _position_in_atlas: [glyph.allocation.position()[0] as f32, glyph.allocation.position()[1] as f32],
                        _size_in_atlas: glyph.allocation.size(),
                        _units_per_em: units_per_em,
                        _layer: glyph.allocation.layer() as u32,
                        _color: paragraph.color(),
                        _bands: [
                            glyph.bands.position()[0] as f32,
                            glyph.bands.position()[1] as f32,
                            glyph.bands.layer() as f32,
                        ],
                    };

                    let cached = paragraph.render_mode() == RenderMode::Cached && !self.lcd_active();
                    let converted = if cached {
                        self.bitmap.as_mut().and_then(|bitmap| {
                            bitmap.glyph(
                                device,
                                &mut encoder,
                                &mut jobs,
                                &used,
                                bitmap::key(paragraph, shaped.glyph.0, glyph_x + shaped.offset[0]),
                                (instance, [glyph_x + shaped.offset[0], baseline]),
                            )
                        })
                    } else {
                        None
                    };
                    self.cached.push(converted.is_some());
                    self.instances.push(converted.unwrap_or(instance));
                }
                
                glyph_x += shaped.advance;
            })
        });

        self.paragraph_offsets.push(self.instances.len() as u32);

        if !jobs.is_empty() {
            if let Some(bitmap) = &self.bitmap {
                bitmap.rasterize(device, encoder.get_or_insert_with(|| device.create_command_encoder(&Default::default())), &jobs, &self.texture, &self.vertices, &self.indices);
            }
        }
        if let Some(encoder) = encoder { queue.submit(Some(encoder.finish())); }
        // Exact byte size on growth; retain the high-water buffer across smaller batches.
        let bytes = bytemuck::cast_slice(&self.instances);
        if !bytes.is_empty() {
            if let Some(buffer) = self.instances_buffer.as_ref().filter(|buffer| buffer.size() >= bytes.len() as u64) {
                queue.write_buffer(buffer, 0, bytes);
            } else {
                self.instances_buffer = Some(device.create_buffer_init(&BufferInitDescriptor {
                    label: Some("Text instances buffer"),
                    usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                    contents: bytes,
                }));
            }
        }
    }

    #[cfg(feature = "unstable-internals")]
    pub(crate) fn instance_count(&self) -> usize {
        self.instances.len()
    }

    #[cfg(feature = "unstable-internals")]
    pub(crate) fn instance_buffer(&self) -> Option<&Buffer> {
        self.instances_buffer.as_ref()
    }

    /// Set the nonzero viewport dimensions in physical pixels.
    /// Must be called before the first [`Self::render`] and on every viewport
    /// change. Writes the existing uniform buffer using the renderer's queue.
    /// If either dimension is zero, returns without changing the previous size.
    /// Until a resize with two nonzero dimensions, rendering leaves the pass untouched.
    pub fn resize(&mut self, queue: &wgpu::Queue, size: [u32; 2]) {
        if size[0] == 0 || size[1] == 0 {
            return;
        }
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&Params {
            screen_resolution: Resolution { width: size[0], height: size[1] },
            _pad: [0, 0],
            transform: orthographic_projection_matrix(0., size[0] as f32, size[1] as f32, 0.),
        }));
        self.has_valid_size = true;
    }

    /// Draw all prepared paragraphs into a pass with the configured target format.
    /// Equivalent to [`Self::render_range`] with `..`. An empty batch does nothing.
    /// Until a resize with two nonzero dimensions, leaves the pass untouched.
    /// Zero-dimension resize calls keep any previous valid size.
    /// Call [`Self::resize`] before the first render and on every viewport change.
    /// The pass's scissor rectangle is preserved.
    ///
    /// Call `prepare` once per queue submit with every paragraph of the frame.
    /// A second `prepare` before the submit can change what earlier `render` calls
    /// of that submit draw. Use [`Self::render_range`] to draw subsets at different
    /// points of a pass or in different passes.
    pub fn render(&self, pass: &mut RenderPass<'_>) {
        self.render_range(pass, ..);
    }

    /// Draw paragraphs by their position in the sequence given to the last
    /// [`Self::prepare`]. Skipped paragraphs keep their index and draw nothing.
    /// Accepts any range syntax, including `2..4`, `2..`, `..2`, `..=1`, and `..`.
    /// Bounds are clamped to the paragraph count; empty or inverted ranges and
    /// calls before any preparation draw nothing. Bounds never overflow or panic.
    /// Until a resize with two nonzero dimensions, leaves the pass untouched.
    /// Zero-dimension resize calls keep any previous valid size.
    /// The pass's scissor is preserved.
    /// Call [`Self::resize`] before the first render and on every viewport change.
    ///
    /// Call `prepare` once per queue submit with every paragraph of the frame.
    /// A second `prepare` before the submit can change what earlier `render` calls
    /// of that submit draw. Use `render_range` to draw subsets at different points
    /// of a pass or in different passes.
    pub fn render_range(&self, pass: &mut RenderPass<'_>, paragraphs: impl RangeBounds<usize>) {
        if !self.has_valid_size {
            return;
        }
        let count = self.paragraph_offsets.len().saturating_sub(1);
        let start = match paragraphs.start_bound() {
            Bound::Included(&index) => index,
            Bound::Excluded(&index) => index.saturating_add(1),
            Bound::Unbounded => 0,
        }.min(count);
        let end = match paragraphs.end_bound() {
            Bound::Included(&index) => index.saturating_add(1),
            Bound::Excluded(&index) => index,
            Bound::Unbounded => count,
        }.min(count);
        if start >= end {
            return;
        }
        let instances = self.paragraph_offsets[start]..self.paragraph_offsets[end];
        if instances.is_empty() {
            return;
        }
        let Some(buffer) = self.instances_buffer.as_ref() else { return; };

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.constants, &[]);
        pass.set_bind_group(1, &self.texture, &[]);
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_vertex_buffer(1, buffer.slice(..));
        let mut start = instances.start;
        while start < instances.end {
            let cached = self.cached[start as usize];
            let mut end = start + 1;
            while end < instances.end && self.cached[end as usize] == cached { end += 1; }
            if let Some(bitmap) = self.bitmap.as_ref().filter(|_| cached) {
                pass.set_pipeline(&bitmap.draw);
                pass.set_bind_group(1, &bitmap.binding, &[]);
            } else {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(1, &self.texture, &[]);
            }
            pass.draw_indexed(0..INDICES.len() as u32, 0, start..end);
            start = end;
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Pod, Zeroable)]
/// Viewport dimensions in physical pixels.
struct Resolution {
    /// Horizontal pixel count.
    pub width: u32,
    /// Vertical pixel count.
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
/// GPU uniform layout: pixel viewport dimensions and column-major clip transform.
struct Params {
    screen_resolution: Resolution,
    _pad: [u32; 2],
    transform: [f32; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Zeroable, Pod)]
struct Instance {
    _position: [f32; 2],
    _left_side_bearing: f32,
    _font_size: f32,
    _size: [f32; 2],
    _position_in_atlas: [f32; 2],
    _size_in_atlas: u32,
    _units_per_em: f32,
    _layer: u32,
    _color: [f32; 4],
    _bands: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Vertex {
    _position: [f32; 3],
}

const INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

const VERTICES: [Vertex; 4] = [
    Vertex {
        _position: [0., 0., 0.]
    },
    Vertex {
        _position: [1., 0., 0.]
    },
    Vertex {
        _position: [1., 1., 0.]
    },
    Vertex {
        _position: [0., 1., 0.]
    }
];

fn same_paragraph(a: &Paragraph, b: &Paragraph) -> bool {
    a.font() == b.font()
        && a.position() == b.position()
        && a.size().to_bits() == b.size().to_bits()
        && a.color() == b.color()
        && a.render_mode() == b.render_mode()
        && a.glyphs() == b.glyphs()
}

fn instance_layouts() -> [Option<VertexBufferLayout<'static>>; 2] {
    [
        Some(VertexBufferLayout {
            array_stride: mem::size_of::<Vertex>() as u64,
            step_mode: VertexStepMode::Vertex,
            attributes: &[VertexAttribute {
                shader_location: 0,
                format: VertexFormat::Float32x2,
                offset: 0,
            }],
        }),
        Some(VertexBufferLayout {
            array_stride: mem::size_of::<Instance>() as u64,
            step_mode: VertexStepMode::Instance,
            attributes: &INSTANCE_ATTRIBUTES,
        }),
    ]
}

const INSTANCE_ATTRIBUTES: [VertexAttribute; 10] = vertex_attr_array!(
    1 => Float32x2,
    2 => Float32,
    3 => Float32,
    4 => Float32x2,
    5 => Float32x2,
    6 => Uint32,
    7 => Float32,
    8 => Sint32,
    9 => Float32x4,
    10 => Float32x3
);

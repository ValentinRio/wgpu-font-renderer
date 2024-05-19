use std::mem;

use bytemuck::{Pod, Zeroable};
use owned_ttf_parser::AsFaceRef;
use wgpu::{
    util::{self, BufferInitDescriptor, DeviceExt, StagingBelt}, vertex_attr_array, BindGroup, BindGroupDescriptor, 
    BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, 
    BindingType, BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer, BufferBinding, 
    BufferBindingType, BufferDescriptor, BufferSize, BufferUsages, ColorTargetState, ColorWrites, 
    Device, FilterMode, FragmentState, FrontFace, MultisampleState, PipelineLayoutDescriptor, 
    PrimitiveState, PrimitiveTopology, RenderPass, RenderPipeline, RenderPipelineDescriptor, 
    SamplerBindingType, SamplerDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages, 
    SurfaceConfiguration, TextureSampleType, TextureViewDimension, VertexAttribute, VertexBufferLayout, 
    VertexFormat, VertexState, VertexStepMode
};

use crate::{atlas::Atlas, ortho::orthographic_projection_matrix, typewriter::Paragraph, FontStore};

pub struct TextRenderer {
    pipeline: RenderPipeline,
    uniforms: Buffer,
    vertices: Buffer,
    indices: Buffer,
    instances_buffer: Option<Buffer>,
    instances: Vec<Instance>,
    constants: BindGroup,
    texture: BindGroup,
    texture_version: usize,
    texture_layout: BindGroupLayout,
    screen_size: [u32; 2],
}

impl TextRenderer {
    pub fn new(device: &Device, surface_config: &SurfaceConfiguration, atlas: &Atlas) -> Self {
        let screen_size = [surface_config.width, surface_config.height];

        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("Text sampler"),
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Nearest,
            mipmap_filter: FilterMode::Nearest,
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
                transform: orthographic_projection_matrix(0., screen_size[0] as f32, screen_size[1] as f32, 0.)
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
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                }
            ],
        });

        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("text pipeline layout"),
            bind_group_layouts: &[&constant_layout, &texture_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Text shader"),
            source: ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!("shader.wgsl"))),
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Text pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: "vs_main",
                compilation_options: Default::default(),
                buffers: &[
                    VertexBufferLayout {
                        array_stride: mem::size_of::<Vertex>() as u64,
                        step_mode: VertexStepMode::Vertex,
                        attributes: &[VertexAttribute {
                            shader_location: 0,
                            format: VertexFormat::Float32x2,
                            offset: 0,
                        }],
                    },
                    VertexBufferLayout {
                        array_stride: mem::size_of::<Instance>() as u64,
                        step_mode: VertexStepMode::Instance,
                        attributes: &vertex_attr_array!(
                            1 => Float32x2,
                            2 => Float32,
                            3 => Float32,
                            4 => Float32x2,
                            5 => Float32x2,
                            6 => Uint32,
                            7 => Float32,
                            8 => Sint32,
                            9 => Float32x4
                        ),
                    }
                ],
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
                entry_point: "fs_main",
                compilation_options: Default::default(),
                targets: &[Some(ColorTargetState {
                    format: surface_config.format,
                    blend: Some(BlendState {
                        color: BlendComponent {
                            src_factor: BlendFactor::SrcAlpha,
                            dst_factor: BlendFactor::OneMinusSrcAlpha,
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
            multiview: None,
        });

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
            entries:  &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(
                        atlas.view(),
                    ),
                },
            ],
        });

        Self {
            pipeline,
            uniforms,
            vertices,
            indices,
            instances_buffer: None,
            instances: Vec::new(),
            constants: constant_bind_group,
            texture,
            texture_version: atlas.layer_count(),
            texture_layout,
            screen_size,
        }
    }

    pub fn prepare(&mut self, device: &Device, paragraphs: &Vec<Paragraph>, store: &FontStore) {
        self.instances = Vec::new();
        let mut glyph_count = 0;

        paragraphs.iter().for_each(|paragraph| {

            let font = store.get(paragraph.font_key).expect("Paragraph has been created without valid font");

            let units_per_em = font.face.as_face_ref().units_per_em() as f32;

            let mut glyph_x = paragraph.position[0];

            paragraph.glyphs.iter().for_each(|(glyph_id, left)| {
                if let Some(glyph) = font.glyph_cache.get(glyph_id) {
                    let glyph_y = paragraph.position[0] + (glyph.y_offset as f32 / units_per_em * paragraph.size as f32) + (f32::abs(glyph.descent as f32) / units_per_em * paragraph.size as f32);

                    let size = [
                        (glyph.bbox.width() as f32 * paragraph.size as f32 / units_per_em),
                        (glyph.bbox.height() as f32 * paragraph.size as f32 / units_per_em),
                    ];

                    let instance = Instance {
                        _position: [glyph_x, glyph_y],
                        _left_side_bearing: glyph.left_side_bearing as f32,
                        _font_size: paragraph.size as f32,
                        _size: size,
                        _position_in_atlas: [glyph.allocation.position()[0] as f32, glyph.allocation.position()[1] as f32],
                        _size_in_atlas: glyph.allocation.size(),
                        _units_per_em: units_per_em,
                        _layer: glyph.allocation.layer() as u32,
                        _color: paragraph.color,
                    };

                    self.instances.push(instance);

                    glyph_x += left;
                    glyph_count += 1;
                }
            })
        });

        // println!("{:#?}", self.instances);

        self.instances_buffer = Some(device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Text instances buffer"),
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            contents: bytemuck::cast_slice(&self.instances[0..self.instances.len()])
        }));
    }

    pub fn update_uniforms(&mut self, device: &Device, screen_size: [u32; 2]) {
        self.uniforms = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Text uniforms buffer"),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            contents: bytemuck::bytes_of(&Params {
                screen_resolution: Resolution {
                    width: screen_size[0],
                    height: screen_size[1],
                },
                _pad: [0, 0],
                transform: orthographic_projection_matrix(0., screen_size[0] as f32, screen_size[1] as f32, 0.)
            }),
        });
    }

    pub fn render<'rpass>(&'rpass mut self, render_pass: &mut RenderPass<'rpass>, screen_size: [u32; 2]) {

        if self.instances.is_empty() {
            return;
        }

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.constants, &[]);
        render_pass.set_bind_group(1, &self.texture, &[]);
        render_pass.set_index_buffer(
            self.indices.slice(..),
            wgpu::IndexFormat::Uint16,
        );
        render_pass.set_vertex_buffer(0, self.vertices.slice(..));
        render_pass.set_vertex_buffer(1, self.instances_buffer.as_ref().unwrap().slice(..));

        render_pass.set_scissor_rect(0, 0, screen_size[0], screen_size[1]);

        render_pass.draw_indexed(0..INDICES.len() as u32, 0, 0..self.instances.len() as u32);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Pod, Zeroable)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Params {
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
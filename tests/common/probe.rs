pub fn probe_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &wgpu::TextureView,
    source: &str,
) -> Vec<u8> {
    probe_pixels_with_blend(device, queue, atlas, source, None, wgpu::Color::WHITE)
}

pub fn probe_pixels_with_blend(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &wgpu::TextureView,
    source: &str,
    blend: Option<wgpu::BlendState>,
    background: wgpu::Color,
) -> Vec<u8> {
    Probe::new(device, atlas, source, blend).pixels(device, queue, background, None)
}

/// Reuse a pipeline and readback target when only uniform probe inputs change.
pub struct Probe {
    pipeline: wgpu::RenderPipeline,
    constants: wgpu::BindGroup,
    texture_group: wgpu::BindGroup,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    inputs: wgpu::Buffer,
    row_bytes: u32,
}

impl Probe {
    pub fn new(
        device: &wgpu::Device,
        atlas: &wgpu::TextureView,
        source: &str,
        blend: Option<wgpu::BlendState>,
    ) -> Self {
        Self::with_format(
            device,
            atlas,
            source,
            blend,
            wgpu::TextureFormat::Rgba8Unorm,
        )
    }

    #[allow(dead_code)] // Only continuity probes need float readback.
    pub fn new_float(device: &wgpu::Device, atlas: &wgpu::TextureView, source: &str) -> Self {
        Self::with_format(
            device,
            atlas,
            source,
            None,
            wgpu::TextureFormat::Rgba32Float,
        )
    }

    fn with_format(
        device: &wgpu::Device,
        atlas: &wgpu::TextureView,
        source: &str,
        blend: Option<wgpu::BlendState>,
        format: wgpu::TextureFormat,
    ) -> Self {
        let row_bytes = 256 * format.block_copy_size(None).unwrap();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Band equivalence probe"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let constants_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&constants_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Band equivalence probe"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("probe_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("probe_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let inputs = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Probe inputs"),
            size: 1024 * 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let constants = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: inputs.as_entire_binding(),
                },
            ],
        });
        let texture_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(1),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(atlas),
            }],
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 256,
                height: 768,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row_bytes) * 768,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            constants,
            texture_group,
            target,
            view,
            readback,
            inputs,
            row_bytes,
        }
    }

    pub fn pixels(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        background: wgpu::Color,
        inputs: Option<&[[f32; 4]]>,
    ) -> Vec<u8> {
        if let Some(inputs) = inputs {
            queue.write_buffer(&self.inputs, 0, bytemuck::cast_slice(inputs));
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(background),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.constants, &[]);
            pass.set_bind_group(1, &self.texture_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.row_bytes),
                    rows_per_image: Some(768),
                },
            },
            self.target.size(),
        );
        queue.submit(Some(encoder.finish()));
        let (sender, receiver) = std::sync::mpsc::channel();
        self.readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                sender.send(result).unwrap()
            });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receiver.recv().unwrap().unwrap();
        let pixels = self.readback.get_mapped_range(..).unwrap().to_vec();
        self.readback.unmap();
        pixels
    }
}

// Probes render black glyph coverage into alpha, in twelve 256x64 size tiles.
// Require 64 pixels with at least 1/8 coverage in every reference tile.
pub fn assert_ink_tiles(pixels: &[u8]) {
    assert_eq!(
        pixels.len(),
        256 * 768 * 4,
        "reference must contain all size tiles"
    );
    for (index, tile) in pixels.as_chunks::<{ 256 * 64 * 4 }>().0.iter().enumerate() {
        let ink = tile
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[..3] == [0; 3] && p[3] >= 32)
            .count();
        assert!(
            ink >= 64,
            "reference size tile {index} has only {ink} inked pixels"
        );
    }
}

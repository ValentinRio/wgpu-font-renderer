use super::*;
use etagere::{size2, Allocation, BucketedAtlasAllocator};
use std::collections::{HashMap, HashSet};

type Key = (crate::FontId, u16, u32, u8, u32);

// 16 MiB of resident R8 coverage (4096²). Growth copies temporarily retain
// replaced textures until their queued GPU work completes.
const MAX_BYTES: u32 = 16 * 1024 * 1024;
fn max_dimension(device_limit: u32) -> u32 {
    device_limit.min(MAX_BYTES.isqrt())
}

fn origin(instance: &Instance, pen: [f32; 2]) -> [f32; 2] {
    [
        instance._position[0] + ((pen[0] * 4.).floor() / 4. - pen[0]),
        instance._position[1] + (pen[1].round() - pen[1]),
    ]
}
pub(super) fn key(p: &Paragraph, glyph: u16, instance: &Instance, pen: [f32; 2]) -> Key {
    // Include the phase actually rasterized, after baseline snapping AND shaping.
    // Whole-pixel vertical offsets may share coverage; fractional mark offsets cannot.
    let y = origin(instance, pen)[1];
    (
        p.font(),
        glyph,
        p.size().to_bits(),
        ((pen[0] * 4.).floor().rem_euclid(4.)) as u8,
        (y - y.floor()).to_bits(),
    )
}
#[derive(Clone, Copy)]
struct Entry {
    allocation: Allocation,
    dims: [f32; 2],
}

pub(super) struct BitmapCache {
    pub draw: RenderPipeline,
    bake: RenderPipeline,
    clear: RenderPipeline,
    pub binding: BindGroup,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    constants: BindGroup,
    constant_layout: BindGroupLayout,
    texture_layout: BindGroupLayout,
    sampler: wgpu::Sampler,
    // Bucketed allocation supports in-place growth while retaining coordinates.
    // Eviction releases unused buckets; pinned buckets may force analytic fallback.
    allocator: BucketedAtlasAllocator,
    entries: HashMap<Key, Entry>,
    pub size: u32,
    pub max: u32,
}

const SHADER: &str = r#"
@vertex
fn vs_bitmap_raster(input: VertexInput) -> VertexOutput {
    var output = vs_main(input);
    let local = input.v_pos * (input.size + vec2(1.)) - vec2(.5);
    let pixel = input.pos + local;
    output.position = vec4(pixel / input.color.xy * vec2(2., -2.) + vec2(-1., 1.), 0., 1.);
    return output;
}

@vertex
fn vs_clear(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4(input.v_pos * vec2(2., -2.) + vec2(-1., 1.), 0., 1.);
    return output;
}
@fragment
fn fs_clear() -> @location(0) f32 { return 0.; }
@fragment
fn fs_bitmap_raster(input: VertexOutput) -> @location(0) f32 {
    return analytic_coverage(input, vec3(0.)).y;
}
@vertex
fn vs_bitmap(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let local = input.v_pos * input.size;
    output.position = params.transform * vec4(input.pos + local, 0., 1.);
    output.uv = input.atlas_pos + local;
    output.color = input.color;
    return output;
}
@fragment
fn fs_bitmap(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4(input.color.rgb, textureLoad(atlas_texture, vec2<i32>(input.uv), 0, 0).r * input.color.a);
}
"#;

impl BitmapCache {
    #[cfg(feature = "unstable-internals")]
    pub fn count(&self) -> usize {
        self.entries.len()
    }
    pub fn reset(&mut self, device: &Device, initial: u32, max: u32) {
        self.max = max.clamp(1, max_dimension(device.limits().max_texture_dimension_2d));
        self.size = initial.clamp(1, self.max);
        let (texture, view, binding, constants) = Self::texture(
            device,
            self.size,
            &self.constant_layout,
            &self.texture_layout,
            &self.sampler,
        );
        self.texture = texture;
        self.view = view;
        self.binding = binding;
        self.constants = constants;
        self.entries = HashMap::new();
        self.allocator = BucketedAtlasAllocator::new(size2(self.size as i32, self.size as i32));
    }

    pub fn new(
        device: &Device,
        format: wgpu::TextureFormat,
        constants: &BindGroupLayout,
        textures: &BindGroupLayout,
        sampler: &wgpu::Sampler,
    ) -> Self {
        let source = include_str!("../shader.wgsl");
        let vertex_entry = "@vertex\nfn vs_main";
        assert_eq!(
            source.matches(vertex_entry).count(),
            1,
            "bitmap shader requires exactly one analytic vertex entry"
        );
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Bitmap coverage shader"),
            source: ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    source.replacen(vertex_entry, "fn vs_main", 1),
                    SHADER
                )
                .into(),
            ),
        });
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Bitmap pipeline layout"),
            bind_group_layouts: &[Some(constants), Some(textures)],
            immediate_size: 0,
        });
        let mut desc = RenderPipelineDescriptor {
            label: Some("Bitmap pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_bitmap"),
                compilation_options: Default::default(),
                buffers: &instance_layouts(),
            },
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                front_face: FrontFace::Cw,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        };
        let mut fragment = FragmentState {
            module: &shader,
            entry_point: Some("fs_bitmap"),
            compilation_options: Default::default(),
            targets: &[],
        };
        let draw_targets = [Some(ColorTargetState {
            format,
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
        })];
        fragment.targets = &draw_targets;
        desc.fragment = Some(fragment.clone());
        let draw = device.create_render_pipeline(&desc);
        desc.vertex.entry_point = Some("vs_bitmap_raster");
        fragment.entry_point = Some("fs_bitmap_raster");
        let targets = [Some(ColorTargetState {
            format: wgpu::TextureFormat::R8Unorm,
            blend: None,
            write_mask: ColorWrites::ALL,
        })];
        fragment.targets = &targets;
        desc.fragment = Some(fragment);
        let bake = device.create_render_pipeline(&desc);
        desc.vertex.entry_point = Some("vs_clear");
        desc.fragment.as_mut().unwrap().entry_point = Some("fs_clear");
        let clear = device.create_render_pipeline(&desc);
        let max = max_dimension(device.limits().max_texture_dimension_2d);
        let size = 512.min(max);
        let (texture, view, binding, constant_binding) =
            Self::texture(device, size, constants, textures, sampler);
        Self {
            draw,
            bake,
            clear,
            binding,
            texture,
            view,
            constants: constant_binding,
            constant_layout: constants.clone(),
            texture_layout: textures.clone(),
            sampler: sampler.clone(),
            allocator: BucketedAtlasAllocator::new(size2(size as i32, size as i32)),
            entries: HashMap::new(),
            size,
            max,
        }
    }
    fn texture(
        device: &Device,
        size: u32,
        constants: &BindGroupLayout,
        textures: &BindGroupLayout,
        sampler: &wgpu::Sampler,
    ) -> (wgpu::Texture, wgpu::TextureView, BindGroup, BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Bitmap glyph atlas"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let array = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2Array),
            ..Default::default()
        });
        let binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Bitmap atlas binding"),
            layout: textures,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(&array),
            }],
        });
        let buffer = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Bitmap uniforms buffer"),
            usage: BufferUsages::UNIFORM,
            contents: bytemuck::bytes_of(&Params {
                screen_resolution: Resolution {
                    width: size,
                    height: size,
                },
                _pad: [0; 2],
                transform: orthographic_projection_matrix(0., size as f32, size as f32, 0.),
            }),
        });
        let constant_binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("Bitmap raster constants"),
            layout: constants,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(sampler),
                },
            ],
        });
        (texture, view, binding, constant_binding)
    }
    pub fn glyph(
        &mut self,
        device: &Device,
        encoder: &mut Option<wgpu::CommandEncoder>,
        jobs: &mut Vec<(Instance, Allocation)>,
        used: &HashSet<Key>,
        key: Key,
        input: (Instance, [f32; 2]),
    ) -> Option<Instance> {
        let (mut instance, pen) = input;
        if !pen.iter().all(|x| x.is_finite()) {
            return None;
        }
        let origin = origin(&instance, pen);
        let min = [(origin[0] - 0.5).floor(), (origin[1] - 0.5).floor()];
        let dims = [
            (origin[0] + instance._size[0] + 0.5).ceil() - min[0],
            (origin[1] + instance._size[1] + 0.5).ceil() - min[1],
        ];
        if dims
            .iter()
            .any(|&x| !x.is_finite() || x + 2. > self.max as f32)
        {
            return None;
        }
        let entry = if let Some(entry) = self.entries.get(&key) {
            *entry
        } else {
            let request = size2(dims[0] as i32 + 2, dims[1] as i32 + 2);
            let mut allocation = self.allocator.allocate(request);
            // Release previous-frame coverage before retaining a larger texture.
            if allocation.is_none() {
                self.entries.retain(|k, entry| {
                    if used.contains(k) {
                        true
                    } else {
                        self.allocator.deallocate(entry.allocation.id);
                        false
                    }
                });
                allocation = self.allocator.allocate(request);
            }
            while allocation.is_none() && self.size < self.max {
                let next = (self.size * 2).min(self.max);
                let (texture, view, binding, constants) = Self::texture(
                    device,
                    next,
                    &self.constant_layout,
                    &self.texture_layout,
                    &self.sampler,
                );
                // Copy existing coverage, retaining all allocation coordinates.
                encoder
                    .get_or_insert_with(|| device.create_command_encoder(&Default::default()))
                    .copy_texture_to_texture(
                        self.texture.as_image_copy(),
                        texture.as_image_copy(),
                        wgpu::Extent3d {
                            width: self.size,
                            height: self.size,
                            depth_or_array_layers: 1,
                        },
                    );
                self.texture = texture;
                self.view = view;
                self.binding = binding;
                self.constants = constants;
                self.size = next;
                self.allocator.grow(size2(next as i32, next as i32));
                allocation = self.allocator.allocate(request);
            }
            let a = allocation?;
            let entry = Entry {
                allocation: a,
                dims,
            };
            self.entries.insert(key, entry);
            let mut raster = instance;
            raster._position = [origin[0] - min[0] + 1., origin[1] - min[1] + 1.];
            // Coverage ignores colour; carry the local viewport extent here.
            raster._color = [
                a.rectangle.width() as f32,
                a.rectangle.height() as f32,
                0.,
                0.,
            ];
            jobs.push((raster, a));
            entry
        };
        instance._position = min;
        instance._size = entry.dims;
        instance._position_in_atlas = [
            entry.allocation.rectangle.min.x as f32 + 1.,
            entry.allocation.rectangle.min.y as f32 + 1.,
        ];
        Some(instance)
    }
    pub fn rasterize(
        &self,
        device: &Device,
        encoder: &mut wgpu::CommandEncoder,
        jobs: &[(Instance, Allocation)],
        outlines: &BindGroup,
        vertices: &Buffer,
        indices: &Buffer,
    ) {
        let instances: Vec<_> = jobs.iter().map(|j| j.0).collect();
        let buffer = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("Bitmap raster instances"),
            usage: BufferUsages::VERTEX,
            contents: bytemuck::cast_slice(&instances),
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Rasterize bitmap glyphs"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.bake);
        pass.set_bind_group(0, &self.constants, &[]);
        pass.set_bind_group(1, outlines, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_vertex_buffer(1, buffer.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint16);
        for (i, (_, a)) in jobs.iter().enumerate() {
            let r = a.rectangle;
            pass.set_viewport(
                r.min.x as f32,
                r.min.y as f32,
                r.width() as f32,
                r.height() as f32,
                0.,
                1.,
            );
            pass.set_scissor_rect(
                r.min.x as u32,
                r.min.y as u32,
                r.width() as u32,
                r.height() as u32,
            );
            pass.set_pipeline(&self.clear);
            pass.draw_indexed(0..6, 0, i as u32..i as u32 + 1);
            pass.set_pipeline(&self.bake);
            pass.draw_indexed(0..6, 0, i as u32..i as u32 + 1);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    #[test]
    fn byte_cap_applies_even_above_device_test_overrides() {
        assert_eq!(max_dimension(32768), 4096);
        assert_eq!(max_dimension(128), 128);
        assert_eq!(max_dimension(u32::MAX).pow(2), MAX_BYTES);
    }
    #[test]
    fn cache_hit_uses_resident_dimensions() {
        let Some((device, queue, _)) = crate::atlas_tests::common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(
                &device,
                &queue,
                include_bytes!("../../fonts/Roboto-Regular.ttf"),
                "g",
            )
            .unwrap();
        let mut p = crate::TypeWriter::new()
            .shape_text(&store, font, [12., 20.], 24., [0., 0., 0., 1.], "g")
            .unwrap();
        p.set_render_mode(RenderMode::Cached);
        let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
        renderer.prepare(&device, &queue, [&p], &store);
        let bitmap = renderer.bitmap.as_mut().unwrap();
        let (&key, &entry) = bitmap.entries.iter().next().unwrap();
        let mut request = renderer.instances[0];
        // Request geometry must never override the dimensions of a resident bitmap.
        request._size = [1., 1.];
        let mut jobs = Vec::new();
        let hit = bitmap
            .glyph(
                &device,
                &mut None,
                &mut jobs,
                &HashSet::from([key]),
                key,
                (request, [12., 20.]),
            )
            .unwrap();
        assert_eq!(hit._size, entry.dims);
        assert!(jobs.is_empty());
    }
}

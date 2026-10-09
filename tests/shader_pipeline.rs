#![cfg(not(target_arch = "wasm32"))]

use wgpu_font_renderer::{FontStore, TextRenderer};

mod common;

#[test]
fn shader_pipeline_validates() {
    let Some((device, _queue, _config)) = common::gpu() else {
        return;
    };
    pollster::block_on(async {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        for format in [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ] {
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: wgpu::SurfaceColorSpace::Srgb,
                width: 800,
                height: 600,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            let store = FontStore::new(&device, &config);
            let mut renderer = TextRenderer::new(&device, &config, store.atlas());
            renderer.update_uniforms(&device, [1024, 768]);
        }
        let error = scope.pop().await;
        assert!(error.is_none(), "Pipeline validation failed: {error:?}");
    });
}

// Frozen pre-band shader: this comparison also catches changes to its fallback behavior.
#[test]
fn bands_match_original_shader_pixels() {
    use owned_ttf_parser::AsFaceRef;
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    for (bytes, grow) in [
        (&include_bytes!("../examples/Roboto-Regular.ttf")[..], false),
        (&include_bytes!("fixtures/Cantarell-VF.otf")[..], false),
        (&include_bytes!("fixtures/Cantarell-VF.otf")[..], true),
    ] {
        let mut store = FontStore::new(&device, &config);
        let mut preset =
            "igB@éABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".to_owned();
        if grow {
            let inspection_key = store.load_from_bytes(&device, &queue, bytes, "").unwrap();
            let face = store.get(inspection_key).unwrap().face.as_face_ref();
            let chars: Vec<_> = (0..=0xffff)
                .filter_map(char::from_u32)
                .filter(|&c| {
                    face.glyph_index(c)
                        .and_then(|id| face.glyph_bounding_box(id))
                        .is_some()
                })
                .take(1400)
                .collect();
            assert!(
                chars.len() > 1000,
                "growth fixture must load over 1000 outlined characters"
            );
            // Consecutive repeats retain each character's last upload. Subsequent
            // glyphs can fill old-layer holes after growth, exposing copy ordering.
            preset.extend(chars.into_iter().flat_map(|c| std::iter::repeat_n(c, 8)));
        }
        let key = store
            .load_from_bytes(&device, &queue, bytes, &preset)
            .unwrap();
        if grow {
            assert!(
                store.atlas().layer_count() > 2,
                "band layers must grow mid-load"
            );
        }
        let font = store.get(key).unwrap();
        let wrapped = preset.chars().find(|&c| {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            let glyph = &font.glyph_cache[&id];
            glyph.allocation.position()[0] + glyph.allocation.size() > 2048
        });
        assert!(
            wrapped.is_some(),
            "fixture must exercise the row-crossing fallback"
        );
        // These are selection guards, not the proof of band use: each growth
        // probe below must also pass the instrumented GPU path assertion.
        let can_use_bands = |c| {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            font.glyph_cache.get(&id).is_some_and(|g| {
                g.allocation.layer() == 0
                    && g.allocation.position()[0] + g.allocation.size() <= 2048
            })
        };
        let old_layer_tail = preset.chars().find(|&c| {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            can_use_bands(c)
                && font
                    .glyph_cache
                    .get(&id)
                    .and_then(|g| g.bands.as_ref())
                    .is_some_and(|b| b.layer() == 1 && b.position()[1] >= 2016)
        });
        let new_layer = preset.chars().find(|&c| {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            can_use_bands(c)
                && font
                    .glyph_cache
                    .get(&id)
                    .and_then(|g| g.bands.as_ref())
                    .is_some_and(|b| b.layer() >= 3)
        });
        if grow {
            assert!(old_layer_tail.is_some() && new_layer.is_some());
        }
        // Keep row-wrap coverage in the ordinary comparison; the growth
        // comparison probes only glyphs that actually complete the band path.
        let probes: Vec<_> = if grow {
            old_layer_tail.into_iter().chain(new_layer).collect()
        } else {
            "igB@é".chars().chain(wrapped).collect()
        };
        for c in probes {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            let glyph = &font.glyph_cache[&id];
            let [x, y] = glyph.allocation.position();
            let bands = glyph.bands.as_ref().unwrap();
            let [bx, by] = bands.position();
            let probe = format!(
                r#"
@vertex
fn probe_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {{
    let p = array(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.));
    return vec4(p[index], 0., 1.);
}}
@fragment
fn probe_fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
    let sizes = array(1., 12., 32., 72., 100., 162., 163., 200., 400., 557., 558., 1024.);
    var input: VertexOutput;
    input.font_size = sizes[u32(p.y) / 64u];
    input.uv = vec2(p.x / 256., (p.y % 64.) / 64.);
    // Include exact vertical contour hits, beyond their endpoint ranges too.
    if p.x < 1. {{ input.uv.x = 0.; }}
    if p.x < 2. && p.x >= 1. {{ input.uv.x = 1.; }}
    input.size = vec2({width}., {height}.) * input.font_size / 2048.;
    input.units_per_em = 2048.;
    input.left_side_bearing = {bearing}.;
    input.atlas_pos = vec2({x}., {y}.);
    input.atlas_size = {count};
    input.layer = {layer}.;
    input.color = vec4(0., 0., 0., 1.);
    // Force exact band-boundary y values, avoiding normalized-UV rounding.
    if u32(p.y) % 64u < 16u {{
        let header = {by} * 2048 + {bx};
        let min_y = probe_float(header, {band_layer});
        let height = probe_float(header + 1, {band_layer});
        let count = u32(probe_float(header + 2, {band_layer}));
        input.pos = vec2(min_y + f32(u32(p.y) % 64u % count) * height, 1.);
    }}
    BAND_INPUT
    return coverage(input);
}}
"#,
                width = glyph.bbox.width(),
                height = glyph.bbox.height(),
                bearing = glyph.left_side_bearing,
                count = glyph.allocation.size(),
                layer = glyph.allocation.layer(),
                band_layer = bands.layer(),
            );
            let reference = include_str!("fixtures/full-list.wgsl");
            let candidate = include_str!("../src/shader.wgsl");
            let source = |shader: &str, band_input: &str| {
                let shader = shader
                    .replace("    uv.y = remap(uv.y, 0., 1., 0., input.size.y * input.units_per_em / font_size);",
                        "    uv.y = remap(uv.y, 0., 1., 0., input.size.y * input.units_per_em / font_size);\n    if input.pos.y == 1. { uv.y = input.pos.x; }")
                    .replace("@fragment\nfn fs_main", "fn coverage")
                    .replace(
                        "fn coverage(input: VertexOutput) -> @location(0) vec4<f32>",
                        "fn coverage(input: VertexOutput) -> vec4<f32>",
                    );
                format!("{shader}\nfn probe_float(offset: i32, layer: i32) -> f32 {{ return textureLoad(atlas_texture, vec2<i32>(offset % 2048, offset / 2048), layer, 0).x; }}\n{}", probe.replace("BAND_INPUT", band_input))
            };
            let expected = probe_pixels(
                &device,
                &queue,
                store.atlas().view(),
                &source(reference, ""),
            );
            let actual = probe_pixels(
                &device,
                &queue,
                store.atlas().view(),
                &source(
                    candidate,
                    &format!("input.bands = vec3({bx}., {by}., {}.);", bands.layer()),
                ),
            );
            if grow || c == 'i' {
                // Instrument only the test source; no diagnostic branch or
                // varying is compiled into the release shader.
                let debug = source(candidate, &format!("input.bands = vec3({bx}., {by}., {}.);", bands.layer()))
                    .replace("    var nearest =", "    var probe_used_band = false;\n    var nearest =")
                    .replace("nearest = band_curves(uv, start, curve_count, layer);",
                        "nearest = band_curves(uv, start, curve_count, layer);\n            probe_used_band = nearest.w != FULL_LIST_REQUIRED;")
                    .replace("return vec4(input.color.rgb, 1 - triplet_alpha.r);",
                        "return vec4(select(0., 1., probe_used_band), 0., 0., 1.);");
                let path_pixels = probe_pixels(&device, &queue, store.atlas().view(), &debug);
                let tile =
                    |index: usize| &path_pixels[index * 256 * 64 * 4..(index + 1) * 256 * 64 * 4];
                assert!(
                    tile(8).as_chunks::<4>().0.iter().any(|p| p[0] == 255),
                    "glyph {c:?}, grow={grow}: size 400 must actually use bands"
                );
                assert!(
                    tile(9).as_chunks::<4>().0.iter().any(|p| p[0] == 255),
                    "glyph {c:?}, grow={grow}: size 557 must actually use bands"
                );
                assert!(
                    tile(10).as_chunks::<4>().0.iter().all(|p| p[0] == 0),
                    "size 558 must take the full-list fallback"
                );
            }
            let differing = actual
                .as_chunks::<4>()
                .0
                .iter()
                .zip(expected.as_chunks::<4>().0)
                .filter(|(a, b)| a != b)
                .count();
            if differing != 0 {
                let per_size: Vec<_> = actual
                    .chunks_exact(256 * 64 * 4)
                    .zip(expected.chunks_exact(256 * 64 * 4))
                    .map(|(a, b)| {
                        a.as_chunks::<4>()
                            .0
                            .iter()
                            .zip(b.as_chunks::<4>().0)
                            .filter(|(a, b)| a != b)
                            .count()
                    })
                    .collect();
                eprintln!("flat start [{x}, {y}], count {}, band start [{bx}, {by}], differences {per_size:?}", glyph.allocation.size());
            }
            assert_eq!(
                differing, 0,
                "glyph {c}: band output must match the original at every sampled size"
            );
        }
    }
}

fn probe_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &wgpu::TextureView,
    source: &str,
) -> Vec<u8> {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Band equivalence probe"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let constants_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            count: None,
        }],
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
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let constants = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::Sampler(&sampler),
        }],
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
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 768 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &constants, &[]);
        pass.set_bind_group(1, &texture_group, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1024),
                rows_per_image: Some(768),
            },
        },
        target.size(),
    );
    queue.submit(Some(encoder.finish()));
    let (sender, receiver) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        sender.send(result).unwrap()
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receiver.recv().unwrap().unwrap();
    readback.get_mapped_range(..).unwrap().to_vec()
}

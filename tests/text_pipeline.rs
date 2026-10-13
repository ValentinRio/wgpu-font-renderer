#![cfg(not(target_arch = "wasm32"))]

use owned_ttf_parser::AsFaceRef;
use wgpu_font_renderer::{FontStore, LoadingError, TextRenderer, TypeWriter};

mod common;

use common::gpu;

#[test]
fn shape_text_preserves_glyphs_advances_and_rejects_unknown_font() {
    let Some((device, queue, _config)) = gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    assert!(matches!(
        store.load_from_bytes(&device, &queue, b"bad font", "Hi"),
        Err(LoadingError::InvalidFont)
    ));
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "Hi",
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    let paragraph = writer
        .shape_text(&store, key, [24., 24.], 32., [0., 0., 0., 1.], "Hi Hi")
        .unwrap();
    let face = wgpu_font_renderer::internals::font(&store, key).unwrap().face.as_face_ref();
    let expected: Vec<_> = "Hi Hi"
        .chars()
        .map(|c| face.glyph_index(c).unwrap())
        .collect();
    assert_eq!(
        paragraph.glyphs()
            .iter()
            .map(|(id, _)| owned_ttf_parser::GlyphId(id.0))
            .collect::<Vec<_>>(),
        expected
    );
    let expected_width: f32 = expected
        .iter()
        .map(|id| face.glyph_hor_advance(*id).unwrap() as f32 * 32. / face.units_per_em() as f32)
        .sum();
    assert!(
        (paragraph.width() - expected_width).abs() < 1e-5,
        "shaped advances must scale font units into pixels"
    );
    let mut pen = paragraph.position()[0];
    for (_, advance) in paragraph.glyphs() {
        let next = pen + advance;
        assert!(
            next > pen,
            "every glyph, including the uncached space, advances the pen"
        );
        pen = next;
    }
    assert!((pen - paragraph.position()[0] - paragraph.width()).abs() < 1e-5);
    let mut other_store = FontStore::new(&device);
    let unknown = other_store.load_from_bytes(&device, &queue,
        include_bytes!("../fonts/Roboto-Regular.ttf"), "Hi").unwrap();
    assert_ne!(key, unknown, "font IDs from different stores must differ");
    for size in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(writer.shape_text(&store, key, [0., 0.], size, [1.; 4], "Hi").is_none());
    }
    assert!(writer
        .shape_text(
            &store,
            unknown,
            [0., 0.],
            32.,
            [0., 0., 0., 1.],
            "Hi"
        )
        .is_none());
}

// Read actual render output: GPU validation alone cannot detect a stale uniform binding.
fn draw_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut TextRenderer,
    size: [u32; 2],
) -> Vec<u8> {
    renderer.resize(queue, size);
    draw_targets(device, queue, renderer, size, &[None]).remove(0)
}

// Each entry draws to its own target, using one encoder and one submit.
// None exercises render; Some exercises render_range. Does not change the viewport.
fn draw_targets(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &TextRenderer,
    size: [u32; 2],
    draws: &[Option<std::ops::Range<usize>>],
) -> Vec<Vec<u8>> {
    draw_range_targets(device, queue, renderer, size, draws)
}

fn draw_range_targets<R: std::ops::RangeBounds<usize> + Clone>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &TextRenderer,
    size: [u32; 2],
    draws: &[Option<R>],
) -> Vec<Vec<u8>> {
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut buffers = Vec::new();
    for range in draws {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Text test target"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let stride = (size[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Text test readback"),
            size: u64::from(stride) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
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
            if let Some(range) = range {
                renderer.render_range(&mut pass, range.clone());
            } else {
                renderer.render(&mut pass);
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(size[1]),
                },
            },
            texture.size(),
        );
        buffers.push((buffer, stride));
    }
    queue.submit(Some(encoder.finish()));
    buffers.into_iter().map(|(buffer, stride)| {
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            sender.send(result).unwrap()
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receiver.recv().unwrap().unwrap();
        let mapped = buffer.get_mapped_range(..).unwrap();
        mapped
            .chunks_exact(stride as usize)
            .flat_map(|row| row[..size[0] as usize * 4].iter().copied())
            .collect()
    }).collect()
}

fn ink_bounds(pixels: &[u8], width: u32) -> [u32; 4] {
    let mut bounds = [u32::MAX, u32::MAX, 0, 0];
    for (i, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if pixel[..3].iter().any(|channel| *channel < 128) {
            let x = i as u32 % width;
            let y = i as u32 / width;
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
        }
    }
    assert_ne!(bounds[0], u32::MAX, "text must produce non-white pixels");
    bounds
}

// The correct llvmpipe renders contain 220 dark pixels (Roboto) and 175
// (Cantarell). Allow 15% variation for antialiasing across GPU backends.
fn assert_hi_fill(pixels: &[u8], dark_band: std::ops::RangeInclusive<usize>) {
    let pixel = |x: usize, y: usize| &pixels[(y * 128 + x) * 4..(y * 128 + x) * 4 + 3];
    // Both fonts from fonts/ at [24, 48], 32 px/em: this is the centre of H's left
    // stem above its crossbar, and the open gap between its stems.
    assert!(
        pixel(25, 61).iter().all(|c| *c < 64),
        "H stem must be dark: {:?}",
        pixel(25, 61)
    );
    assert!(
        pixel(32, 61).iter().all(|c| *c > 240),
        "H gap must be white: {:?}",
        pixel(32, 61)
    );
    let count = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[..3].iter().any(|c| *c < 128))
        .count();
    assert!(
        dark_band.contains(&count),
        "dark count {count} outside {dark_band:?}"
    );
}

#[test]
fn prepare_and_resize_keep_ink_at_pixel_coordinates() {
    let Some((device, queue, config)) = gpu() else {
        return;
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut store = FontStore::new(&device);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "Hi",
        )
        .unwrap();
    let paragraph: wgpu_font_renderer::Paragraph = TypeWriter::new()
        .shape_text(&store, key, [24., 48.], 32., [0., 0., 0., 1.], "Hi")
        .unwrap();
    let mut renderer = TextRenderer::new(&device, config.format, &store);
    renderer.prepare(&device, &queue, &[paragraph], &store);
    let pixels = draw_pixels(&device, &queue, &mut renderer, [128, 128]);
    assert_hi_fill(&pixels, 187..=253);
    let before = ink_bounds(&pixels, 128);
    assert!(
        before[0] >= 24 && before[1] >= 48 && before[2] < 80 && before[3] < 96,
        "all ink must be within the expected box: {before:?}"
    );
    for size in [[256, 192], [128, 128]] {
        renderer.resize(&queue, size);
        let after = ink_bounds(&draw_pixels(&device, &queue, &mut renderer, size), size[0]);
        assert!(
            after
                .iter()
                .zip(before)
                .all(|(actual, expected)| actual.abs_diff(expected) <= 1),
            "resizing must preserve physical pixel placement within 1 px: {before:?} -> {after:?}"
        );
    }
    renderer.prepare(&device, &queue, &[], &store);
    assert!(
        draw_pixels(&device, &queue, &mut renderer, [128, 128])
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 255, 255, 255]),
        "empty prepare must clear previous instances"
    );
    assert!(pollster::block_on(scope.pop()).is_none());
}

#[test]
fn prepare_uses_paragraph_y_coordinate() {
    let Some((device, queue, config)) = gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "Hi",
        )
        .unwrap();
    let paragraph = TypeWriter::new()
        .shape_text(&store, key, [24., 64.], 32., [0., 0., 0., 1.], "Hi")
        .unwrap();
    let mut renderer = TextRenderer::new(&device, config.format, &store);
    renderer.prepare(&device, &queue, &[paragraph], &store);
    let bounds = ink_bounds(
        &draw_pixels(&device, &queue, &mut renderer, [128, 128]),
        128,
    );
    assert!(
        bounds[1] >= 64 && bounds[3] < 112,
        "ink must follow the y anchor: {bounds:?}"
    );
}

#[test]
fn cantarell_cff2_loads_outlines_and_renders_ink() {
    let Some((device, queue, config)) = gpu() else {
        return;
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut store = FontStore::new(&device);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Cantarell-VF.otf"),
            "Hi",
        )
        .unwrap();
    let font = wgpu_font_renderer::internals::font(&store, key).unwrap();
    for c in "Hi".chars() {
        let id = font.face.as_face_ref().glyph_index(c).unwrap();
        let glyph = font
            .glyph_cache
            .get(&id)
            .expect("CFF2 glyph must be cached");
        assert!(!glyph.curves.is_empty());
        let pad = font.face.as_face_ref().units_per_em() as f32 * 0.15;
        for segment in glyph.curves.as_chunks::<8>().0 {
            for p in segment[..6].as_chunks::<2>().0 {
                assert!(
                    p[0] >= glyph.bbox.x_min as f32 - pad
                        && p[0] <= glyph.bbox.x_max as f32 + pad
                        && p[1] >= -pad
                        && p[1] <= glyph.bbox.height() as f32 + pad
                );
            }
        }
    }
    let paragraph = TypeWriter::new()
        .shape_text(&store, key, [24., 48.], 32., [0., 0., 0., 1.], "Hi")
        .unwrap();
    let mut renderer = TextRenderer::new(&device, config.format, &store);
    renderer.prepare(&device, &queue, &[paragraph], &store);
    let pixels = draw_pixels(&device, &queue, &mut renderer, [128, 128]);
    assert_hi_fill(&pixels, 149..=201);
    let bounds = ink_bounds(&pixels, 128);
    assert!(
        bounds[0] >= 24 && bounds[1] >= 48 && bounds[2] < 80 && bounds[3] < 96,
        "CFF2 ink must be within the expected box: {bounds:?}"
    );
    assert!(pollster::block_on(scope.pop()).is_none());
}

#[test]
fn missing_font_file_preserves_io_source() {
    use std::error::Error;
    let Some((device, queue, _config)) = gpu() else { return; };
    let error = FontStore::new(&device)
        .load(&device, &queue, "fonts/missing-font-file.otf", "Hi")
        .unwrap_err();
    assert!(matches!(error, LoadingError::Io(_)));
    assert!(error.source().is_some());
}

#[test]
fn unknown_font_and_empty_batches_draw_nothing_and_instances_are_reused() {
    let Some((device, queue, config)) = gpu() else { return; };
    let mut store = FontStore::new(&device);
    let bytes = include_bytes!("../fonts/Roboto-Regular.ttf");
    let font = store.load_from_bytes(&device, &queue, bytes, "Hi").unwrap();
    let mut other = FontStore::new(&device);
    let unknown = other.load_from_bytes(&device, &queue, bytes, "Hi").unwrap();
    let mut writer = TypeWriter::new();
    let paragraph = writer.shape_text(&store, font, [24., 24.], 32., [0., 0., 0., 1.], "Hi").unwrap();
    let foreign = writer.shape_text(&other, unknown, [24., 24.], 32., [0., 0., 0., 1.], "Hi").unwrap();
    let mut renderer = TextRenderer::new(&device, config.format, &store);
    renderer.prepare(&device, &queue, &[], &store);
    assert!(draw_pixels(&device, &queue, &mut renderer, [128, 128]).iter().all(|&b| b == 255));
    renderer.prepare(&device, &queue, std::slice::from_ref(&paragraph), &store);
    let buffer = wgpu_font_renderer::internals::instance_buffer(&renderer).unwrap().clone();
    renderer.prepare(&device, &queue, std::slice::from_ref(&paragraph), &store);
    assert_eq!(&buffer, wgpu_font_renderer::internals::instance_buffer(&renderer).unwrap());
    let expected = draw_pixels(&device, &queue, &mut renderer, [128, 128]);
    renderer.prepare(&device, &queue, &[paragraph, foreign.clone()], &store);
    assert_eq!(expected, draw_pixels(&device, &queue, &mut renderer, [128, 128]));
    renderer.prepare(&device, &queue, &[foreign], &store);
    assert!(draw_pixels(&device, &queue, &mut renderer, [128, 128]).iter().all(|&b| b == 255));
}

#[test]
fn fonts_loaded_after_renderer_construction_refresh_the_atlas() {
    let Some((device, queue, config)) = gpu() else { return; };
    let mut store = FontStore::new(&device);
    store.load_from_bytes(&device, &queue,
        include_bytes!("../fonts/Roboto-Regular.ttf"), "Hi").unwrap();
    let mut renderer = TextRenderer::new(&device, config.format, &store);
    let layers = wgpu_font_renderer::internals::atlas_layer_count(&store);
    // Repeated uploads force growth; the final Hi outlines reside in new layers.
    let preset = "@".repeat(12000) + "Hi";
    let font = store.load_from_bytes(&device, &queue,
        include_bytes!("../fonts/Cantarell-VF.otf"), &preset).unwrap();
    assert!(wgpu_font_renderer::internals::atlas_layer_count(&store) > layers);
    let paragraph = TypeWriter::new().shape_text(&store, font,
        [24., 24.], 32., [0., 0., 0., 1.], "Hi").unwrap();
    renderer.prepare(&device, &queue, &[paragraph], &store);
    let pixels = draw_pixels(&device, &queue, &mut renderer, [128, 128]);
    assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[..3] != [255; 3]));
}


#[test]
fn prepared_ranges_draw_layers_in_one_submit_and_preserve_indices() {
    let Some((device, queue, config)) = gpu() else { return; };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut store = FontStore::new(&device);
    let bytes = include_bytes!("../fonts/Roboto-Regular.ttf");
    let font = store.load_from_bytes(&device, &queue, bytes, "AB").unwrap();
    let mut writer = TypeWriter::new();
    let first = Vec::from([writer.shape_text(&store, font, [16., 16.], 32., [0., 0., 0., 1.], "A").unwrap()]);
    let last = Vec::from([writer.shape_text(&store, font, [80., 80.], 32., [0., 0., 0., 1.], "B").unwrap()]);
    let mut renderer = TextRenderer::new(&device, config.format, &store);
    renderer.resize(&queue, [128, 128]);

    // Borrow two separate vectors without cloning their paragraphs.
    renderer.prepare(&device, &queue, first.iter().chain(last.iter()), &store);
    assert_eq!(wgpu_font_renderer::internals::instance_count(&renderer), 2);
    let mut pixels = draw_targets(&device, &queue, &renderer, [128, 128],
        &[Some(0..1), Some(1..2), None, Some(0..2)]);
    pixels.push(draw_range_targets(&device, &queue, &renderer, [128, 128], &[Some(..)]).remove(0));
    for (target, area) in pixels[..2].iter().zip([[16, 16, 48, 64], [80, 80, 112, 128]]) {
        ink_bounds(target, 128); // Each target must contain ink.
        for (i, pixel) in target.as_chunks::<4>().0.iter().enumerate() {
            let x = i as u32 % 128;
            let y = i as u32 / 128;
            if x < area[0] || y < area[1] || x >= area[2] || y >= area[3] {
                assert_eq!(*pixel, [255; 4], "ink outside this paragraph's area at {x},{y}");
            }
        }
    }
    assert_eq!(pixels[2], pixels[3], "render and full render_range must match");
    assert_eq!(pixels[2], pixels[4], "range end must be clamped");
    assert_eq!(pixels[2], draw_range_targets(&device, &queue, &renderer, [128, 128], &[Some(..)])[0]);
    assert_eq!(pixels[1], draw_range_targets(&device, &queue, &renderer, [128, 128], &[Some(1..)])[0]);
    assert_eq!(pixels[0], draw_range_targets(&device, &queue, &renderer, [128, 128], &[Some(..1)])[0]);
    assert_eq!(pixels[0], draw_range_targets(&device, &queue, &renderer, [128, 128], &[Some(..=0)])[0]);
    use std::ops::Bound;
    let extreme = draw_range_targets(&device, &queue, &renderer, [128, 128], &[
        Some((Bound::Excluded(usize::MAX), Bound::Unbounded)),
        Some((Bound::Unbounded, Bound::Included(usize::MAX))),
    ]);
    assert!(extreme[0].iter().all(|&b| b == 255));
    assert_eq!(pixels[2], extreme[1], "inclusive maximum end must be clamped");

    renderer.prepare(&device, &queue, &first, &store);
    assert_eq!(pixels[0], draw_targets(&device, &queue, &renderer, [128, 128], &[None])[0],
        "range selecting A must exactly match preparing A alone");
    renderer.prepare(&device, &queue, &last, &store);
    assert_eq!(pixels[1], draw_targets(&device, &queue, &renderer, [128, 128], &[None])[0],
        "range selecting B must exactly match preparing B alone");

    let mut other = FontStore::new(&device);
    let foreign_font = other.load_from_bytes(&device, &queue, bytes, "A").unwrap();
    let foreign = writer.shape_text(&other, foreign_font, [48., 48.], 32., [1.; 4], "A").unwrap();
    renderer.prepare(&device, &queue,
        first.iter().chain(std::iter::once(&foreign)).chain(last.iter()), &store);
    assert_eq!(wgpu_font_renderer::internals::instance_count(&renderer), 2);
    let after_skip = draw_targets(&device, &queue, &renderer, [128, 128],
        &[Some(2..3), Some(1..2)]);
    assert_eq!(pixels[1], after_skip[0], "skipping a paragraph must preserve later indices");
    assert!(after_skip[1].iter().all(|&b| b == 255));
    renderer.prepare(&device, &queue, &first, &store);
    let stale = draw_targets(&device, &queue, &renderer, [128, 128], &[Some(1..2)]);
    assert!(stale[0].iter().all(|&b| b == 255), "a shorter prepare must clear stale offsets");
    assert!(pollster::block_on(scope.pop()).is_none());
}

#[test]
fn invalid_ranges_and_sizes_draw_nothing_and_zero_resize_keeps_viewport() {
    let Some((device, queue, config)) = gpu() else { return; };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut store = FontStore::new(&device);
    let font = store.load_from_bytes(&device, &queue,
        include_bytes!("../fonts/Roboto-Regular.ttf"), "Hi").unwrap();
    let mut renderer = TextRenderer::new(&device, config.format, &store);
    renderer.resize(&queue, [128, 128]);
    let inverted = std::ops::Range { start: 2, end: 1 };
    let ranges = [Some(5..10), Some(1..1), Some(inverted.clone()), None];
    let mut before_prepare = draw_targets(&device, &queue, &renderer, [128, 128], &ranges);
    before_prepare.push(draw_range_targets(&device, &queue, &renderer, [128, 128], &[Some(..)]).remove(0));
    assert!(before_prepare.iter().all(|target| target.iter().all(|&b| b == 255)));

    let paragraph = TypeWriter::new().shape_text(&store, font,
        [24., 48.], 32., [0., 0., 0., 1.], "Hi").unwrap();
    renderer.prepare(&device, &queue, std::slice::from_ref(&paragraph), &store);
    let empty = draw_targets(&device, &queue, &renderer, [128, 128],
        &[Some(5..10), Some(1..1), Some(inverted), Some(usize::MAX..usize::MAX)]);
    assert!(empty.iter().all(|target| target.iter().all(|&b| b == 255)));
    let expected = draw_targets(&device, &queue, &renderer, [128, 128], &[None]).remove(0);
    ink_bounds(&expected, 128);
    for size in [[0, 600], [800, 0]] {
        renderer.resize(&queue, size);
        assert_eq!(expected, draw_targets(&device, &queue, &renderer, [128, 128], &[None])[0]);
    }

    // Build invalid paragraphs by hand with a cached glyph so skipping cannot
    // accidentally pass because the paragraph is empty or the glyph is uncached.
    for size in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let mut invalid = wgpu_font_renderer::Paragraph::new(font, [24., 48.], size, [1.; 4]);
        invalid.push(paragraph.glyphs()[0].0, 12.);
        renderer.prepare(&device, &queue, &[invalid], &store);
        assert_eq!(wgpu_font_renderer::internals::instance_count(&renderer), 0);
        assert!(draw_targets(&device, &queue, &renderer, [128, 128], &[None])[0]
            .iter().all(|&b| b == 255));
    }
    assert!(pollster::block_on(scope.pop()).is_none());
}

#[test]
fn rendering_without_a_valid_resize_preserves_clear_colour() {
    let Some((device, queue, config)) = gpu() else { return; };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut store = FontStore::new(&device);
    let font = store.load_from_bytes(&device, &queue,
        include_bytes!("../fonts/Roboto-Regular.ttf"), "Hi").unwrap();
    let paragraph = TypeWriter::new().shape_text(&store, font,
        [24., 48.], 32., [0., 0., 0., 1.], "Hi").unwrap();
    let mut renderer = TextRenderer::new(&device, config.format, &store);
    renderer.prepare(&device, &queue, &[paragraph], &store);
    assert!(wgpu_font_renderer::internals::instance_count(&renderer) > 0);
    let assert_clear = |renderer: &TextRenderer| {
        let targets = draw_targets(&device, &queue, renderer, [128, 128], &[None, Some(0..1)]);
        assert!(targets.iter().all(|target| target.iter().all(|&b| b == 255)),
            "render and render_range must preserve the clear colour before a valid resize");
    };
    assert_clear(&renderer);
    for size in [[0, 600], [800, 0], [0, 0]] {
        renderer.resize(&queue, size);
        assert_clear(&renderer);
    }
    assert!(pollster::block_on(scope.pop()).is_none());
}

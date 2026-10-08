#![cfg(not(target_arch = "wasm32"))]

use owned_ttf_parser::AsFaceRef;
use swash::CacheKey;
use wgpu_font_renderer::{FontStore, LoadingError, TextRenderer, TypeWriter};

mod common;

use common::gpu;

#[test]
fn shape_text_preserves_glyphs_advances_and_rejects_unknown_font() {
    let Some((device, queue, config)) = gpu() else {
        return;
    };
    let mut store = FontStore::new(&device, &config);
    assert!(matches!(
        store.load_from_bytes(&device, &queue, b"bad font", "Hi"),
        Err(LoadingError::InvalidFile)
    ));
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../examples/Roboto-Regular.ttf"),
            "Hi",
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    let paragraph = writer
        .shape_text(&store, key, [24., 24.], 32, [0., 0., 0., 1.], "Hi Hi")
        .unwrap();
    let face = store.get(key).unwrap().face.as_face_ref();
    let expected: Vec<_> = "Hi Hi"
        .chars()
        .map(|c| face.glyph_index(c).unwrap())
        .collect();
    assert_eq!(
        paragraph
            .glyphs
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        expected
    );
    let expected_width: f32 = expected
        .iter()
        .map(|id| face.glyph_hor_advance(*id).unwrap() as f32 * 32. / face.units_per_em() as f32)
        .sum();
    assert!(
        (paragraph.width - expected_width).abs() < 1e-5,
        "shaped advances must scale font units into pixels"
    );
    let mut pen = paragraph.position[0];
    for (_, advance) in &paragraph.glyphs {
        let next = pen + advance;
        assert!(
            next > pen,
            "every glyph, including the uncached space, advances the pen"
        );
        pen = next;
    }
    assert!((pen - paragraph.position[0] - paragraph.width).abs() < 1e-5);
    assert!(writer
        .shape_text(
            &store,
            CacheKey::new(),
            [0., 0.],
            32,
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
        renderer.render(&mut pass, size);
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
    queue.submit(Some(encoder.finish()));
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

#[test]
fn prepare_and_resize_keep_ink_at_pixel_coordinates() {
    let Some((device, queue, config)) = gpu() else {
        return;
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut store = FontStore::new(&device, &config);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../examples/Roboto-Regular.ttf"),
            "Hi",
        )
        .unwrap();
    let paragraph = TypeWriter::new()
        .shape_text(&store, key, [24., 24.], 32, [0., 0., 0., 1.], "Hi")
        .unwrap();
    let mut renderer = TextRenderer::new(&device, &config, store.atlas());
    renderer.prepare(&device, &vec![paragraph], &store);
    let before = ink_bounds(
        &draw_pixels(&device, &queue, &mut renderer, [128, 128]),
        128,
    );
    assert!(
        before[0] >= 24 && before[1] >= 24 && before[2] < 80 && before[3] < 72,
        "all ink must be within the expected box: {before:?}"
    );
    for size in [[256, 192], [128, 128]] {
        renderer.update_uniforms(&device, size);
        let after = ink_bounds(&draw_pixels(&device, &queue, &mut renderer, size), size[0]);
        assert!(
            after
                .iter()
                .zip(before)
                .all(|(actual, expected)| actual.abs_diff(expected) <= 1),
            "resizing must preserve physical pixel placement within 1 px: {before:?} -> {after:?}"
        );
    }
    renderer.prepare(&device, &vec![], &store);
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
#[ignore = "bug: prepare uses paragraph x instead of y for vertical placement"]
fn prepare_uses_paragraph_y_coordinate() {
    let Some((device, queue, config)) = gpu() else {
        return;
    };
    let mut store = FontStore::new(&device, &config);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../examples/Roboto-Regular.ttf"),
            "Hi",
        )
        .unwrap();
    let paragraph = TypeWriter::new()
        .shape_text(&store, key, [24., 64.], 32, [0., 0., 0., 1.], "Hi")
        .unwrap();
    let mut renderer = TextRenderer::new(&device, &config, store.atlas());
    renderer.prepare(&device, &vec![paragraph], &store);
    let bounds = ink_bounds(
        &draw_pixels(&device, &queue, &mut renderer, [128, 128]),
        128,
    );
    assert!(
        bounds[1] >= 64 && bounds[3] < 112,
        "ink must follow the y anchor: {bounds:?}"
    );
}

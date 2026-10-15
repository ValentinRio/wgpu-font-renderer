#![cfg(all(not(target_arch = "wasm32"), feature = "unstable-internals"))]
use wgpu_font_renderer::{internals, FontStore, RenderMode, TextRenderer, TypeWriter};
mod common;
#[path = "common/draw.rs"]
mod draw;

#[test]
fn parity_and_subpixel_bins() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let mut writer = TypeWriter::new();
    let mut failures = Vec::new();
    for (bytes, text) in [
        (
            &include_bytes!("../fonts/Roboto-Regular.ttf")[..],
            "AgjypfQ@W",
        ),
        (
            &include_bytes!("../fonts/NotoSerifSC-Subset.otf")[..],
            "龍鬱",
        ),
    ] {
        let font = store.load_from_bytes(&device, &queue, bytes, text).unwrap();
        let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
        for size in [12., 16., 24., 48.] {
            for x in [12., 12.13, 12.26, 12.61, 12.99] {
                // One glyph per paragraph lets the test set each pen exactly.
                for ch in text.chars() {
                    for alpha in [1., 0.5] {
                        let mut p = writer
                            .shape_text(
                                &store,
                                font,
                                [x, 20.],
                                size,
                                [0.2, 0.4, 0.7, alpha],
                                &ch.to_string(),
                            )
                            .unwrap();
                        assert_eq!(p.render_mode(), RenderMode::Analytic);
                        let ascent = store.metrics(font, size).unwrap().ascent;
                        let snapped_y = (20_f32 + ascent).round() - ascent;
                        p.set_position([(x * 4_f32).floor() / 4., snapped_y]);
                        renderer.prepare(&device, &queue, [&p], &store);
                        let a = draw::draw_pixels(
                            &device,
                            &queue,
                            &mut renderer,
                            [128, 128],
                            wgpu::Color::WHITE,
                        );
                        p.set_position([x, 20.]);
                        p.set_render_mode(RenderMode::Cached);
                        renderer.prepare(&device, &queue, [&p], &store);
                        assert_eq!(internals::bitmap_cache_counts(&renderer).1, 1);
                        let b = draw::draw_pixels(
                            &device,
                            &queue,
                            &mut renderer,
                            [128, 128],
                            wgpu::Color::WHITE,
                        );
                        let error = a.iter().zip(&b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
                        if error > 1 {
                            failures.push((size, x, ch, error));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "parity failures: {failures:?}");
}

#[test]
fn growth_eviction_and_fallback_keep_ink() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let text = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let font = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            text,
        )
        .unwrap();
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    internals::set_bitmap_cache_limits(&mut renderer, &device, 16, 64);
    let mut writer = TypeWriter::new();
    let mut paragraphs = Vec::new();
    for ch in text.chars() {
        let mut p = writer
            .shape_text(
                &store,
                font,
                [8., 8.],
                24.,
                [0., 0., 0., 1.],
                &ch.to_string(),
            )
            .unwrap();
        p.set_render_mode(RenderMode::Cached);
        renderer.prepare(&device, &queue, [&p], &store);
        assert_eq!(
            internals::bitmap_cache_counts(&renderer).1,
            1,
            "eviction must admit {ch}"
        );
        let pixels =
            draw::draw_pixels(&device, &queue, &mut renderer, [64, 64], wgpu::Color::WHITE);
        assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[0] < 200));
        let i = paragraphs.len();
        p.set_position([8. + (i % 8) as f32 * 48., 8. + (i / 8) as f32 * 48.]);
        paragraphs.push(p);
    }
    assert_eq!(internals::bitmap_cache_size(&renderer), 64);
    renderer.prepare(&device, &queue, &paragraphs, &store);
    assert!(
        internals::bitmap_cache_counts(&renderer).1 < paragraphs.len(),
        "must fall back"
    );
    let pixels = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [384, 384],
        wgpu::Color::WHITE,
    );
    for i in 0..paragraphs.len() {
        let x = i % 8 * 48;
        let y = i / 8 * 48;
        assert!(
            (y..y + 48).any(|row| (x..x + 48).any(|col| pixels[(row * 384 + col) * 4] < 200)),
            "missing ink for glyph {i}"
        );
    }
}

#[test]
fn lcd_requests_stay_analytic() {
    use wgpu_font_renderer::SubpixelOrder;
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let font = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "Ag",
        )
        .unwrap();
    let mut renderer = TextRenderer::with_subpixel_order(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        &store,
        SubpixelOrder::Rgb,
    );
    if !renderer.lcd_active() {
        return;
    }
    let mut p = TypeWriter::new()
        .shape_text(&store, font, [12.13, 20.3], 24., [0., 0., 0., 1.], "Ag")
        .unwrap();
    renderer.prepare(&device, &queue, [&p], &store);
    let analytic = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [128, 128],
        wgpu::Color::WHITE,
    );
    p.set_render_mode(RenderMode::Cached);
    renderer.prepare(&device, &queue, [&p], &store);
    assert_eq!(internals::bitmap_cache_counts(&renderer), (0, 0));
    assert_eq!(
        analytic,
        draw::draw_pixels(
            &device,
            &queue,
            &mut renderer,
            [128, 128],
            wgpu::Color::WHITE
        )
    );
}

#[test]
fn analytic_only_allocates_no_bitmap_resources() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let font = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "Hello",
        )
        .unwrap();
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    assert_eq!(internals::bitmap_cache_size(&renderer), 0);
    let mut p = TypeWriter::new()
        .shape_text(&store, font, [12., 20.], 24., [0., 0., 0., 1.], "Hello")
        .unwrap();
    renderer.prepare(&device, &queue, [&p], &store);
    assert_eq!(internals::bitmap_cache_size(&renderer), 0);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    if let Some(report) = device.generate_allocator_report() {
        assert!(!report
            .allocations
            .iter()
            .any(|a| a.name == "Bitmap glyph atlas" || a.name == "Bitmap uniforms buffer"));
    }
    p.set_render_mode(RenderMode::Cached);
    renderer.prepare(&device, &queue, [&p], &store);
    assert!(internals::bitmap_cache_size(&renderer) > 0);
}

#[test]
fn block_snaps_each_baseline_and_preserves_line_offsets() {
    use wgpu_font_renderer::TextLayout;
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let font = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "gjQ",
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    for size in [12., 16., 24., 48.] {
        let mut block = writer
            .layout_text(
                &store,
                font,
                [12.13, 10.2],
                size,
                [0.2, 0.4, 0.6, 0.8],
                "g\nj\nQ",
                &TextLayout::default(),
            )
            .unwrap();
        let ascent = store.metrics(font, size).unwrap().ascent;
        let mut reference = block.lines().to_vec();
        for p in &mut reference {
            p.set_position([12., (p.position()[1] + ascent).round() - ascent]);
        }
        let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
        renderer.prepare(&device, &queue, &reference, &store);
        let expected = draw::draw_pixels(
            &device,
            &queue,
            &mut renderer,
            [128, 256],
            wgpu::Color::WHITE,
        );
        block.set_render_mode(RenderMode::Cached);
        renderer.prepare(&device, &queue, &block, &store);
        let actual = draw::draw_pixels(
            &device,
            &queue,
            &mut renderer,
            [128, 256],
            wgpu::Color::WHITE,
        );
        assert!(
            expected.iter().zip(actual).all(|(a, b)| a.abs_diff(b) <= 1),
            "size={size}"
        );
    }
}

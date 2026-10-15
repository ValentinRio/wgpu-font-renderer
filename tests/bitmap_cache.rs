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
    assert!(internals::bitmap_cache_size(&renderer) > 16);
    renderer.prepare(&device, &queue, &paragraphs, &store);
    assert_eq!(internals::bitmap_cache_size(&renderer), 64);
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

// An analytic paragraph per glyph preserves accumulated advances, but places each
// pen at the cached x bin and applies the line's common baseline correction.
fn snapped_reference(
    store: &FontStore,
    paragraphs: &[wgpu_font_renderer::Paragraph],
) -> Vec<wgpu_font_renderer::Paragraph> {
    use wgpu_font_renderer::Paragraph;
    let mut reference = Vec::new();
    for p in paragraphs {
        let ascent = store.metrics(p.font(), p.size()).unwrap().ascent;
        let y = p.position()[1] + ((p.position()[1] + ascent).round() - (p.position()[1] + ascent));
        let mut x = p.position()[0];
        for g in p.glyphs() {
            let mut glyph = Paragraph::new(
                p.font(),
                [((x + g.offset[0]) * 4.).floor() / 4., y],
                p.size(),
                p.color(),
            );
            glyph.push_at(g.glyph, 0., [0., g.offset[1]]);
            reference.push(glyph);
            x += g.advance;
        }
    }
    reference
}
fn compare_cached(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut TextRenderer,
    store: &FontStore,
    paragraphs: &[wgpu_font_renderer::Paragraph],
) {
    let reference = snapped_reference(store, paragraphs);
    renderer.prepare(device, queue, &reference, store);
    let expected = draw::draw_pixels(device, queue, renderer, [512, 160], wgpu::Color::WHITE);
    renderer.prepare(device, queue, paragraphs, store);
    let actual = draw::draw_pixels(device, queue, renderer, [512, 160], wgpu::Color::WHITE);
    let error = expected
        .iter()
        .zip(&actual)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(
        expected.iter().any(|&v| v < 240),
        "reference must contain ink"
    );
    assert!(
        error <= 1,
        "cached/reference max channel difference: {error}"
    );
}

#[test]
fn fractional_vertical_mark_offsets_have_distinct_cache_entries() {
    use wgpu_font_renderer::Paragraph;
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let font = store
        .load_from_bytes(&device, &queue, &common::decomposed_font(), "e\u{301}")
        .unwrap();
    let mark = TypeWriter::new()
        .shape_text(&store, font, [0.; 2], 48., [0.; 4], "e\u{301}")
        .unwrap()
        .glyphs()[1]
        .glyph;
    let mut p = Paragraph::new(font, [16.13, 20.2], 48., [0.2, 0.4, 0.7, 0.8]);
    // Same mark, size, and x bin; only the actual vertical raster phase differs.
    p.push_at(mark, 32., [0., 0.125]);
    p.push_at(mark, 32., [0., 0.625]);
    p.set_render_mode(RenderMode::Cached);
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    compare_cached(&device, &queue, &mut renderer, &store, &[p]);
    assert_eq!(internals::bitmap_cache_counts(&renderer), (2, 2));
}

#[test]
fn multi_glyph_runs_and_two_fonts_match_snapped_analytic() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let latin = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "AgjypfQ@W",
        )
        .unwrap();
    let cjk = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/NotoSerifSC-Subset.otf"),
            "龍鬱直接字形",
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    for size in [12., 16., 24., 48.] {
        let mut paragraphs = [
            writer
                .shape_text(
                    &store,
                    latin,
                    [12.13, 12.2],
                    size,
                    [0.1, 0.4, 0.8, 0.7],
                    "AgjAgjypfQ@WAgj",
                )
                .unwrap(),
            writer
                .shape_text(
                    &store,
                    cjk,
                    [13.61, 80.3],
                    size,
                    [0.7, 0.2, 0.4, 0.8],
                    "龍鬱直接字形",
                )
                .unwrap(),
        ];
        for p in &mut paragraphs {
            p.set_render_mode(RenderMode::Cached);
        }
        compare_cached(&device, &queue, &mut renderer, &store, &paragraphs);
        assert_eq!(internals::bitmap_cache_counts(&renderer).1, 21);
    }
}

#[test]
fn bitmap_growth_preserves_previously_rasterized_coverage() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let font = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "AgjypfQ@W",
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    let mut a = writer
        .shape_text(&store, font, [12.13, 20.2], 16., [0.2, 0.4, 0.7, 0.8], "A")
        .unwrap();
    a.set_render_mode(RenderMode::Cached);
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    internals::set_bitmap_cache_limits(&mut renderer, &device, 32, 256);
    renderer.prepare(&device, &queue, [&a], &store);
    let before = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [128, 128],
        wgpu::Color::WHITE,
    );
    let original_size = internals::bitmap_cache_size(&renderer);
    let mut more = writer
        .shape_text(&store, font, [12., 70.], 48., [0., 0., 0., 1.], "gjypfQ@W")
        .unwrap();
    more.set_render_mode(RenderMode::Cached);
    renderer.prepare(&device, &queue, [&a, &more], &store);
    assert!(internals::bitmap_cache_size(&renderer) > original_size);
    renderer.prepare(&device, &queue, [&a], &store);
    assert_eq!(
        before,
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
fn size_churn_evicts_before_growing_and_reset_releases_capacity() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let font = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "gW",
        )
        .unwrap();
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    internals::set_bitmap_cache_limits(&mut renderer, &device, 64, 256);
    let mut writer = TypeWriter::new();
    for step in 0..48 {
        let mut p = writer
            .shape_text(
                &store,
                font,
                [12.13, 20.2],
                16. + step as f32 / 8.,
                [0., 0., 0., 1.],
                "g",
            )
            .unwrap();
        p.set_render_mode(RenderMode::Cached);
        compare_cached(&device, &queue, &mut renderer, &store, &[p]);
        assert_eq!(
            internals::bitmap_cache_size(&renderer),
            64,
            "unused glyphs should be evicted before growth"
        );
    }
    let mut large = writer
        .shape_text(&store, font, [0.; 2], 150., [0., 0., 0., 1.], "W")
        .unwrap();
    large.set_render_mode(RenderMode::Cached);
    renderer.prepare(&device, &queue, [&large], &store);
    assert!(internals::bitmap_cache_size(&renderer) > 64);
    queue.submit([]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    internals::set_bitmap_cache_limits(&mut renderer, &device, 16, 256);
    queue.submit([]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert_eq!(internals::bitmap_cache_size(&renderer), 16);
    assert_eq!(internals::bitmap_cache_counts(&renderer).0, 0);
    if let Some(report) = device.generate_allocator_report() {
        let textures: Vec<_> = report
            .allocations
            .iter()
            .filter(|a| a.name == "Bitmap glyph atlas")
            .collect();
        assert_eq!(textures.len(), 1, "reset must release the old atlas");
        assert!(
            textures[0].size < 65536,
            "large atlas allocation is still retained"
        );
    }
}

#[test]
fn full_cache_fallback_matches_analytic_pixels() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let font = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "gjypQ",
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    let mut p = writer
        .shape_text(
            &store,
            font,
            [12.25, 20.],
            48.,
            [0.2, 0.4, 0.7, 0.8],
            "gjypQ",
        )
        .unwrap();
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    renderer.prepare(&device, &queue, [&p], &store);
    let expected = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [256, 128],
        wgpu::Color::WHITE,
    );
    internals::set_bitmap_cache_limits(&mut renderer, &device, 16, 16);
    p.set_render_mode(RenderMode::Cached);
    renderer.prepare(&device, &queue, [&p], &store);
    assert_eq!(internals::bitmap_cache_counts(&renderer), (0, 0));
    let actual = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [256, 128],
        wgpu::Color::WHITE,
    );
    assert!(expected.iter().any(|&v| v < 240));
    assert!(expected.iter().zip(actual).all(|(a, b)| a.abs_diff(b) <= 1));
    // Also exhaust a cache whose glyphs fit individually. Current-frame entries
    // are pinned, so later glyphs must use the same analytic fallback.
    let mut run = wgpu_font_renderer::Paragraph::new(
        font,
        [12.25, 64. - store.metrics(font, 24.).unwrap().ascent],
        24.,
        [0.2, 0.4, 0.7, 0.8],
    );
    for glyph in p.glyphs() {
        run.push(glyph.glyph, 32.);
    }
    renderer.prepare(&device, &queue, [&run], &store);
    let expected = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [256, 128],
        wgpu::Color::WHITE,
    );
    internals::set_bitmap_cache_limits(&mut renderer, &device, 32, 32);
    run.set_render_mode(RenderMode::Cached);
    renderer.prepare(&device, &queue, [&run], &store);
    let cached = internals::bitmap_cache_counts(&renderer).1;
    assert!(
        cached > 0 && cached < run.glyphs().len(),
        "must mix cached and capacity fallback glyphs"
    );
    let actual = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [256, 128],
        wgpu::Color::WHITE,
    );
    assert!(expected.iter().zip(actual).all(|(a, b)| a.abs_diff(b) <= 1));
}

#[test]
fn analytic_and_cached_draw_with_zero_storage_binding_limits() {
    let Some((device, queue, _)) = common::gpu_without_storage() else {
        return;
    };
    assert_eq!(device.limits().max_storage_buffers_per_shader_stage, 0);
    let mut store = FontStore::new(&device);
    let font = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "AgjypfQ",
        )
        .unwrap();
    let mut p = TypeWriter::new()
        .shape_text(
            &store,
            font,
            [12.13, 20.2],
            24.,
            [0.2, 0.4, 0.7, 0.8],
            "AgjypfQ",
        )
        .unwrap();
    p.set_render_mode(RenderMode::Cached);
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    compare_cached(&device, &queue, &mut renderer, &store, &[p]);
    assert_eq!(internals::bitmap_cache_counts(&renderer).1, 7);
}

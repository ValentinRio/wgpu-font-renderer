#![cfg(all(not(target_arch = "wasm32"), feature = "unstable-internals"))]
use wgpu_font_renderer::{internals, FontStore};
mod common;
const LATIN: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,;:!ù*^$=)àç_è-('\"é&²<>+°§/.? ";
const CJK: &str = "龍鬱齉直接从轮廓渲染字形文本";
#[test]
fn outline_storage_measurements() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    for (name, bytes, chars) in [
        (
            "Roboto",
            &include_bytes!("../fonts/Roboto-Regular.ttf")[..],
            LATIN,
        ),
        (
            "Cantarell",
            &include_bytes!("../fonts/Cantarell-VF.otf")[..],
            LATIN,
        ),
        (
            "Noto",
            &include_bytes!("../fonts/NotoSerifSC-Subset.otf")[..],
            CJK,
        ),
    ] {
        let mut store = FontStore::new(&device);
        let id = store
            .load_from_bytes(&device, &queue, bytes, chars)
            .unwrap();
        let font = internals::font(&store, id).unwrap();
        let outline: u64 = font
            .glyph_cache
            .values()
            .map(|g| u64::from(g.allocation.size()) * 4)
            .sum();
        let bands: u64 = font
            .glyph_cache
            .values()
            .map(|g| u64::from(g.bands.size()) * 4)
            .sum();
        eprintln!(
            "STORAGE {name} outlines={outline} bands={bands} written={}",
            outline + bands
        );
    }
}

fn assert_packed(store: &FontStore) -> u64 {
    let texture = internals::atlas_view(store).texture();
    let extent = texture.size();
    let bytes = u64::from(extent.width)
        * u64::from(extent.height)
        * u64::from(extent.depth_or_array_layers)
        * 16;
    let written = internals::atlas_written_bytes(store);
    let row = u64::from(extent.width) * 16;
    assert!(bytes >= written);
    assert!(
        bytes - written < row * u64::from(extent.depth_or_array_layers),
        "allocated={bytes}, written={written}, extent={extent:?}"
    );
    bytes
}

#[test]
fn atlas_is_bounded_by_payload_and_one_row_per_layer() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    for fonts in [
        vec![(
            include_bytes!("../fonts/Roboto-Regular.ttf").as_slice(),
            LATIN,
        )],
        vec![(
            include_bytes!("../fonts/NotoSerifSC-Subset.otf").as_slice(),
            CJK,
        )],
        vec![
            (
                include_bytes!("../fonts/Roboto-Regular.ttf").as_slice(),
                LATIN,
            ),
            (
                include_bytes!("../fonts/NotoSerifSC-Subset.otf").as_slice(),
                CJK,
            ),
        ],
    ] {
        let mut store = FontStore::new(&device);
        let mut payload = 0;
        for (bytes, chars) in fonts {
            let id = store
                .load_from_bytes(&device, &queue, bytes, chars)
                .unwrap();
            payload += internals::font_written_bytes(&store, id)
                .unwrap()
                .into_iter()
                .sum::<u64>();
            assert_eq!(internals::atlas_written_bytes(&store), payload);
            assert_packed(&store);
        }
    }
}

#[path = "common/draw.rs"]
mod draw;
#[test]
fn font_and_variation_growth_preserve_earlier_pixels() {
    use wgpu_font_renderer::{TextRenderer, TypeWriter};
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let text = "AgjypfQ@W";
    let first = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            text,
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    let a = writer
        .shape_text(&store, first, [8., 8.], 24., [0., 0., 0., 1.], text)
        .unwrap();
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    renderer.prepare(&device, &queue, [&a], &store);
    let initial = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [256, 128],
        wgpu::Color::WHITE,
    );
    let mut bytes = assert_packed(&store);
    let second = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Cantarell-VF.otf"),
            text,
        )
        .unwrap();
    assert!(assert_packed(&store) > bytes);
    renderer.prepare(&device, &queue, [&a], &store);
    assert_eq!(
        initial,
        draw::draw_pixels(
            &device,
            &queue,
            &mut renderer,
            [256, 128],
            wgpu::Color::WHITE
        )
    );
    let b = writer
        .shape_text(&store, second, [8., 64.], 24., [0., 0., 0., 1.], text)
        .unwrap();
    renderer.prepare(&device, &queue, [&a, &b], &store);
    let both = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [256, 128],
        wgpu::Color::WHITE,
    );
    bytes = assert_packed(&store);
    let variation = store
        .load_variation(&device, &queue, second, &[(*b"wght", 700.)])
        .unwrap();
    assert!(assert_packed(&store) > bytes);
    renderer.prepare(&device, &queue, [&a, &b], &store);
    assert_eq!(
        both,
        draw::draw_pixels(
            &device,
            &queue,
            &mut renderer,
            [256, 128],
            wgpu::Color::WHITE
        )
    );
    let c = writer
        .shape_text(&store, variation, [8., 8.], 24., [0., 0., 0., 1.], text)
        .unwrap();
    renderer.prepare(&device, &queue, [&c], &store);
    assert!(draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [256, 128],
        wgpu::Color::WHITE
    )
    .iter()
    .any(|&v| v < 255));
}

#[test]
fn growth_to_depth_two_matches_fresh_fonts_and_variation() {
    use wgpu_font_renderer::{TextRenderer, TypeWriter};
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let text = "AgjypfQ@W";
    // The reference puts the variable font and its variation before Roboto.
    let mut fresh = FontStore::new(&device);
    let reference_base = fresh
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Cantarell-VF.otf"),
            LATIN,
        )
        .unwrap();
    let reference_variation = fresh
        .load_variation(&device, &queue, reference_base, &[(*b"wght", 700.)])
        .unwrap();
    let reference_latin = fresh
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            LATIN,
        )
        .unwrap();
    let total_rows = internals::atlas_written_bytes(&fresh).div_ceil(8192) as u32;
    let max_rows = total_rows.div_ceil(2);
    let mut store = FontStore::new(&device);
    internals::set_atlas_height_limit(&mut store, max_rows);
    let latin = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            LATIN,
        )
        .unwrap();
    assert_eq!(
        internals::atlas_view(&store)
            .texture()
            .depth_or_array_layers(),
        1
    );
    let mut writer = TypeWriter::new();
    let mut renderer = TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &store);
    let a = writer
        .shape_text(
            &store,
            latin,
            [12.25, 20.3],
            32.,
            [0.2, 0.4, 0.7, 0.8],
            text,
        )
        .unwrap();
    renderer.prepare(&device, &queue, [&a], &store);
    let before = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [384, 128],
        wgpu::Color::WHITE,
    );
    let base = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Cantarell-VF.otf"),
            LATIN,
        )
        .unwrap();
    let variation = store
        .load_variation(&device, &queue, base, &[(*b"wght", 700.)])
        .unwrap();
    assert_eq!(
        internals::atlas_view(&store)
            .texture()
            .depth_or_array_layers(),
        2
    );
    assert_packed(&store);
    // Nonzero outline/band payload from the earlier font spans both physical layers.
    assert!(internals::font(&store, latin)
        .unwrap()
        .glyph_cache
        .values()
        .any(|g| g.bands.packed_position()[1] > 1));
    renderer.prepare(&device, &queue, [&a], &store);
    assert_eq!(
        before,
        draw::draw_pixels(
            &device,
            &queue,
            &mut renderer,
            [384, 128],
            wgpu::Color::WHITE
        )
    );
    let mut reference_renderer =
        TextRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, &fresh);
    for (font, reference_font, text) in [
        (latin, reference_latin, text),
        (variation, reference_variation, text),
    ] {
        let p = writer
            .shape_text(&store, font, [12.25, 20.3], 32., [0.2, 0.4, 0.7, 0.8], text)
            .unwrap();
        let reference = writer
            .shape_text(
                &fresh,
                reference_font,
                [12.25, 20.3],
                32.,
                [0.2, 0.4, 0.7, 0.8],
                text,
            )
            .unwrap();
        renderer.prepare(&device, &queue, [&p], &store);
        reference_renderer.prepare(&device, &queue, [&reference], &fresh);
        assert_eq!(
            draw::draw_pixels(
                &device,
                &queue,
                &mut renderer,
                [384, 128],
                wgpu::Color::WHITE
            ),
            draw::draw_pixels(
                &device,
                &queue,
                &mut reference_renderer,
                [384, 128],
                wgpu::Color::WHITE
            )
        );
    }
}

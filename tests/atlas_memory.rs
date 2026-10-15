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

#![cfg(all(not(target_arch = "wasm32"), feature = "unstable-internals"))]

use skrifa::MetadataProvider;
use wgpu_font_renderer::{internals, FontStore, GlyphId, Paragraph, SubpixelOrder, TextRenderer};
#[path = "common/analytic.rs"]
#[allow(dead_code)]
mod analytic;
mod common;
#[path = "common/draw.rs"]
mod draw;
#[path = "common/probe.rs"]
#[allow(dead_code)]
mod probe;

const LATIN: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,;:!ù*^$=)àç_è-('\"é&²<>+°§/.? ";
const CJK: &str = "龍鬱齉直接从轮廓渲染字形文本";

#[test]
fn preset_glyphs_are_invariant_under_integer_translation() {
    translation_case(SubpixelOrder::None);
}

#[test]
fn rgb_preset_glyphs_are_invariant_under_integer_translation() {
    translation_case(SubpixelOrder::Rgb);
}

fn translation_case(order: SubpixelOrder) {
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    const TILE: usize = 192;
    const WIDTH: usize = TILE * 8;
    let offsets = [
        [1, 0],
        [0, 1],
        [3, 5],
        [7, 7],
        [16, 16],
        [32, 0],
        [0, 32],
        [32, 32],
    ];
    for (name, bytes, preset) in [
        (
            "Roboto",
            &include_bytes!("../fonts/Roboto-Regular.ttf")[..],
            LATIN,
        ),
        (
            "Cantarell VF",
            &include_bytes!("../fonts/Cantarell-VF.otf")[..],
            LATIN,
        ),
        (
            "Noto Serif SC",
            &include_bytes!("../fonts/NotoSerifSC-Subset.otf")[..],
            CJK,
        ),
    ] {
        let mut store = FontStore::new(&device);
        let key = store
            .load_from_bytes(&device, &queue, bytes, preset)
            .unwrap();
        let font = internals::font(&store, key).unwrap();
        let glyphs: Vec<_> = preset
            .chars()
            .map(|ch| (ch, font.face().charmap().map(ch).unwrap().to_u32() as u16))
            .chain(std::iter::once(('\u{fffd}', 0)))
            .collect();
        let mut renderer = TextRenderer::with_subpixel_order(&device, config.format, &store, order);
        if order == SubpixelOrder::Rgb {
            assert!(renderer.lcd_active(), "RGB translation must exercise LCD");
        }
        for size in [12., 16., 24., 48., 96.] {
            let mut max = 0;
            for batch in glyphs.chunks(64) {
                let height = TILE * batch.len().div_ceil(8);
                let mut paragraphs: Vec<_> = batch
                    .iter()
                    .enumerate()
                    .map(|(i, &(_, id))| {
                        let anchor = [(i % 8 * TILE + 12) as f32, (i / 8 * TILE + 20) as f32];
                        let mut paragraph = Paragraph::new(key, anchor, size, [0., 0., 0., 1.]);
                        paragraph.push(GlyphId(id), 0.);
                        paragraph
                    })
                    .collect();
                renderer.prepare(&device, &queue, &paragraphs, &store);
                let original = draw::draw_pixels(
                    &device,
                    &queue,
                    &mut renderer,
                    [WIDTH as u32, height as u32],
                    wgpu::Color::WHITE,
                );
                for [dx, dy] in offsets {
                    for (i, paragraph) in paragraphs.iter_mut().enumerate() {
                        paragraph.set_position([
                            (i % 8 * TILE + 12 + dx) as f32,
                            (i / 8 * TILE + 20 + dy) as f32,
                        ]);
                    }
                    renderer.prepare(&device, &queue, &paragraphs, &store);
                    let shifted = draw::draw_pixels(
                        &device,
                        &queue,
                        &mut renderer,
                        [WIDTH as u32, height as u32],
                        wgpu::Color::WHITE,
                    );
                    for (i, &(ch, _)) in batch.iter().enumerate() {
                        // The padded tile contains the complete glyph at every offset.
                        for y in 0..TILE {
                            for x in 0..TILE {
                                let a = ((i / 8 * TILE + y) * WIDTH + i % 8 * TILE + x) * 4;
                                let expected = &original[a..a + 4];
                                if x + dx >= TILE || y + dy >= TILE {
                                    assert_eq!(
                                        expected, [255; 4],
                                        "{name} {ch:?} {size}: ink outside translation comparison"
                                    );
                                    continue;
                                }
                                let b = a + (dy * WIDTH + dx) * 4;
                                for channel in 0..4 {
                                    let delta = expected[channel].abs_diff(shifted[b + channel]);
                                    max = max.max(delta);
                                    assert!(delta <= 1, "{name} {ch:?} {size}px offset {dx},{dy}, pixel {x},{y}, channel {channel}: {} -> {} (delta {delta})",
                                        expected[channel], shifted[b + channel]);
                                }
                            }
                        }
                    }
                }
            }
            eprintln!("translation {order:?} {name} {size}px: max channel delta {max}");
        }
    }
}

#[test]
fn coverage_is_continuous_at_shared_extremum() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "g",
        )
        .unwrap();
    let font = internals::font(&store, key).unwrap();
    let id = font.face().charmap().map('g').unwrap().to_u32();
    let glyph = &font.glyph_cache[&id];
    let mut body = analytic::glyph_input(
        (
            glyph.allocation.position(),
            glyph.allocation.size(),
            glyph.allocation.layer(),
        ),
        (glyph.bands.position(), glyph.bands.layer()),
        24. / 2048.,
    );
    body.push_str(
        r#"
        input.uv = vec2(bitcast<f32>(0x440aaaabu), bitcast<f32>(0x448c3fffu + u32(p.x) % 4u));
        let banded = analytic_coverage(input, vec3(0.)).y;
        input.bands.z = -1.;
        let flat = analytic_coverage(input, vec3(0.)).y;
        return vec4(banded, flat, 0., 1.);
    "#,
    );
    let pixels = probe::probe_pixels(
        &device,
        &queue,
        internals::atlas_view(&store),
        &analytic::source(&body),
    );
    eprintln!(
        "adjacent-float coverage (banded/unbanded): {:?}",
        &pixels[..16]
    );
    for channel in 0..2 {
        for sample in 1..4 {
            assert!(
                pixels[channel].abs_diff(pixels[sample * 4 + channel]) <= 1,
                "adjacent floats: {:?}",
                &pixels[..16]
            );
        }
    }
}

#![cfg(not(target_arch = "wasm32"))]

use skrifa::{MetadataProvider, instance::{Size, LocationRef}};
use wgpu_font_renderer::FontStore;
#[path = "common/analytic.rs"]
#[allow(dead_code)]
mod analytic;
mod common;
#[path = "common/probe.rs"]
#[allow(dead_code)]
mod probe;
use common::reference::reference_coverage;

const CACHE_PRESET: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,;:!ù*^$=)àç_è-('\"é&²<>+°§/.? ";
const NOTO_PRESET: &str = "龍鬱齉直接从轮廓渲染字形文本";
use common::centre::CentreModel;
use common::coverage_limits::{C3_DENSE_MEAN_ERRORS, C3_MAX_ERROR, C3_MEAN_ERRORS, READBACK_ERROR};

#[test]
fn c3_every_cached_glyph_matches_method_and_box_error_bounds() {
    let Some((device, queue, _config)) = common::gpu() else {
        return;
    };
    let mut compared = 0_usize;
    let mut method_max = 0_f32;
    let mut method_worst = String::new();
    for (name, bytes, preset, mean_errors) in [
        (
            "Roboto",
            &include_bytes!("../fonts/Roboto-Regular.ttf")[..],
            CACHE_PRESET,
            C3_MEAN_ERRORS,
        ),
        (
            "Cantarell",
            &include_bytes!("../fonts/Cantarell-VF.otf")[..],
            CACHE_PRESET,
            C3_MEAN_ERRORS,
        ),
        (
            "Noto Serif SC",
            &include_bytes!("../fonts/NotoSerifSC-Subset.otf")[..],
            NOTO_PRESET,
            C3_DENSE_MEAN_ERRORS,
        ),
    ] {
        let mut store = FontStore::new(&device);
        let key = store
            .load_from_bytes(&device, &queue, bytes, preset)
            .unwrap();
        let font = wgpu_font_renderer::internals::font(&store, key).unwrap();
        if name == "Noto Serif SC" {
            for ch in NOTO_PRESET.chars() {
                let id = font
                    .face()
                    .charmap().map(ch)
                    .expect("Noto character exists");
                assert!(
                    font.glyph_cache
                        .get(&id.to_u32())
                        .is_some_and(|glyph| !glyph.curves.is_empty()),
                    "Noto {ch} (id {}): outline was not cached",
                    id
                );
            }
        }
        // The shader stays identical across batches and sizes; glyph data is
        // uniform input so the driver compiles one pipeline per font.
        let source = analytic::source(r#"
            let tile = u32(p.y) / 256u;
            let flat = probe_inputs[tile * 3u];
            let band = probe_inputs[tile * 3u + 1u];
            let origin = probe_inputs[tile * 3u + 2u];
            let local = vec2(p.x, p.y - f32(tile) * 256.);
            if flat.z == 0. || local.x >= band.w || local.y >= origin.w { return vec4(0.); }
            var input: VertexOutput;
            input.atlas_pos = flat.xy;
            input.atlas_size = i32(flat.z);
            input.layer = flat.w;
            input.bands = vec3(band.xyz);
            input.pixel_scale = origin.z;
            input.uv = origin.xy + local / input.pixel_scale;
            return vec4(analytic_coverage(input, vec3(0.)), 1.);
        "#);
        let source = format!("@group(0) @binding(2) var<uniform> probe_inputs: array<vec4<f32>, 9>;\n{source}");
        let probe = probe::Probe::new(&device, wgpu_font_renderer::internals::atlas_view(&store), &source, None);
        let mut glyphs: Vec<_> = font.glyph_cache.iter().collect();
        glyphs.sort_by_key(|(id, _)| **id);
        for (size, mean_limit) in mean_errors {
            let scale = size / font.face().metrics(Size::unscaled(), LocationRef::default()).units_per_em as f32;
            let (mut max, mut total, mut count) = (0_f32, 0_f64, 0_usize);
            let mut worst = String::new();
            let mut method_size_max = 0_f32;
            for batch in glyphs.chunks(3) {
                let mut inputs = [[0.; 4]; 9];
                for (tile, (_, glyph)) in batch.iter().enumerate() {
                    let a = &glyph.allocation;
                    assert!(a.size() > 0, "cached glyphs must not become empty probe tiles");
                    let b = &glyph.bands;
                    inputs[tile * 3] = [a.position()[0] as f32, a.position()[1] as f32, a.size() as f32, a.layer() as f32];
                    inputs[tile * 3 + 1] = [b.position()[0] as f32, b.position()[1] as f32, b.layer() as f32,
                        (glyph.bbox.width() as f32 * scale).ceil() + 2.];
                    inputs[tile * 3 + 2] = [glyph.left_side_bearing as f32 - 1. / scale, -1. / scale, scale,
                        (glyph.bbox.height() as f32 * scale).ceil() + 2.];
                }
                let pixels = probe.pixels(&device, &queue, wgpu::Color::WHITE, Some(&inputs));
                for (tile, (id, glyph)) in batch.iter().enumerate() {
                    let ch = preset
                        .chars()
                        .find(|&ch| font.face().charmap().map(ch).map(|id| id.to_u32()) == Some(**id));
                    let width = (glyph.bbox.width() as f32 * scale).ceil() as u32 + 2;
                    let height = (glyph.bbox.height() as f32 * scale).ceil() as u32 + 2;
                    assert!(width <= 256 && height <= 256);
                    let reference = reference_coverage(
                        &glyph.curves,
                        glyph.left_side_bearing as f32 - 1. / scale,
                        -1. / scale,
                        scale,
                        width,
                        height,
                    );
                    let centre = CentreModel::new(&glyph.curves).image(
                        [glyph.left_side_bearing as f32 - 1. / scale, -1. / scale],
                        scale,
                        width,
                        height,
                    );
                    for y in 0..height {
                        for x in 0..width {
                            let index = (y * width + x) as usize;
                            let actual = pixels
                                [((tile * 256 + y as usize) * 256 + x as usize) * 4 + 1]
                                as f32
                                / 255.;
                            let error = (actual - reference[index]).abs();
                            if error > max {
                                max = error;
                                worst = format!(
                                    "{ch:?}, id {}, pixel {x},{y}: GPU {actual:.6}, box {:.6}",
                                    id, reference[index]
                                );
                            }
                            let difference = (actual - centre[index]).abs();
                            method_size_max = method_size_max.max(difference);
                            if difference > method_max {
                                method_max = difference;
                                method_worst = format!(
                                    "{name} {size}, {ch:?}, id {}, pixel {x},{y}: GPU {actual:.8}, CPU {:.8}",
                                    id, centre[index]
                                );
                            }
                            total += f64::from(error);
                            count += 1;
                        }
                    }
                }
            }
            let mean = total / count as f64;
            compared += count;
            eprintln!("C3 {name} {size}: max {max:.6}, mean {mean:.6}, pixels {count}; {worst}");
            assert!(
                method_size_max <= READBACK_ERROR,
                "{name} {size}: GPU/CPU difference {method_size_max:.8}; {method_worst}"
            );
            assert!(
                mean <= mean_limit,
                "{name} {size}: box mean {mean:.8} exceeds {mean_limit}; worst glyph: {worst}"
            );
            assert!(
                max <= C3_MAX_ERROR,
                "{name} {size}: box max {max:.8} exceeds {C3_MAX_ERROR}; {worst}"
            );
        }
    }
    eprintln!("C3 method: pixels {compared}, worst difference {method_max:.8}; {method_worst}");
    assert!(
        method_max <= READBACK_ERROR,
        "GPU/CPU method difference {method_max:.8}: {method_worst}"
    );
}

#[path = "common/draw.rs"]
mod draw;

#[test]
fn c7_dilated_quad_covers_pixels_outside_bbox_without_moving_ink() {
    use wgpu_font_renderer::{Paragraph, TextRenderer};
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "I",
        )
        .unwrap();
    let font = wgpu_font_renderer::internals::font(&store, key).unwrap();
    let id = font.face().charmap().map('I').unwrap();
    let glyph = &font.glyph_cache[&id.to_u32()];
    let scale = 48. / font.face().metrics(Size::unscaled(), LocationRef::default()).units_per_em as f32;
    // Place the ink left edge at x=20.75, including the bearing. Pixel x=20 has its centre outside
    // that quad but intersects the stem by one quarter of a pixel.
    let top = 20.75;
    let left = 20.75;
    let position = [
        left - glyph.left_side_bearing as f32 * scale,
        top - (glyph.y_offset as f32 + f32::from(glyph.descent).abs()) * scale,
    ];
    let mut paragraph = Paragraph::new(key, position, 48., [1.; 4]);
    paragraph.push(wgpu_font_renderer::GlyphId(id.to_u32() as u16), 0.);
    let mut renderer = TextRenderer::new(&device, config.format, &store);
    renderer.prepare(&device, &queue, &[paragraph], &store);
    let pixels = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [128, 128],
        wgpu::Color::BLACK,
    );
    let reference = reference_coverage(
        &glyph.curves,
        -position[0] / scale,
        -top / scale,
        scale,
        128,
        128,
    );
    let y = (top + glyph.bbox.height() as f32 * scale * 0.5) as usize;
    let actual = pixels[(y * 128 + 20) * 4] as f32 / 255.;
    let expected = reference[y * 128 + 20];
    assert!(expected > 0. && expected < 1.);
    assert!(
        (actual - expected).abs() <= 1. / 255.,
        "outside bbox: {actual}, reference {expected}"
    );
    let mut bounds = [128usize, 128, 0, 0];
    for y in 0..128 {
        for x in 0..128 {
            if pixels[(y * 128 + x) * 4] > 16 {
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x);
                bounds[3] = bounds[3].max(y);
            }
        }
    }
    let expected_bounds = [
        position[0] + glyph.left_side_bearing as f32 * scale,
        top,
        position[0] + glyph.bbox.x_max as f32 * scale,
        top + glyph.bbox.height() as f32 * scale,
    ];
    for (a, e) in bounds.into_iter().zip(expected_bounds) {
        assert!((a as f32 - e).abs() <= 1.);
    }
}

#[test]
fn variable_instances_shape_and_draw_at_the_same_location() {
    use skrifa::raw::TableProvider;
    use wgpu_font_renderer::{TextRenderer, TypeWriter};
    let Some((device, queue, config)) = common::gpu() else { return; };
    for (bytes, has_anchor_delta) in [
        (include_bytes!("../fonts/Cantarell-VF.otf").to_vec(), false),
        (common::mvar_anchor_font(), true),
    ] {
        let mut store = FontStore::new(&device);
        let base = store.load_from_bytes(&device, &queue, &bytes, "II").unwrap();
        let axis = *store.axes(base).unwrap().iter().find(|axis| axis.tag == *b"wght").unwrap();
        let mut widths = Vec::new();
        let mut outlines = Vec::new();
        for value in [axis.min, axis.max, 550.] {
            let key = store.load_variation(&device, &queue, base, &[(axis.tag, value)]).unwrap();
            let font = wgpu_font_renderer::internals::font(&store, key).unwrap();
            let face = font.face();
            let location = face.axes().location([("wght", value)]);
            let units = f32::from(face.metrics(Size::unscaled(), &location).units_per_em);
            let scale = 48. / units;
            let glyph_id = face.charmap().map('I').unwrap();
            let glyph = &font.glyph_cache[&glyph_id.to_u32()];
            outlines.push(glyph.curves.clone());
            let instance_bounds = face.glyph_metrics(Size::unscaled(), &location).bounds(glyph_id).unwrap();
            let expected_left = instance_bounds.x_min as i16;
            let position = [20.75 - f32::from(expected_left) * scale, 20.75];
            let paragraph = TypeWriter::new().shape_text(&store, key, position, 48., [1.; 4], "II").unwrap();
            let advance = face.glyph_metrics(Size::unscaled(), &location).advance_width(glyph_id).unwrap() * scale;
            assert!((paragraph.width() - 2. * advance).abs() < 0.01,
                "shaping must use the outline instance's normalized coordinates");
            widths.push(paragraph.width());
            let metrics = store.metrics(key, 48.).unwrap();
            let hhea = face.hhea().unwrap();
            let os2 = face.os2().unwrap();
            let default_ascent = if os2.version() >= 4 && os2.fs_selection().contains(
                skrifa::raw::tables::os2::SelectionFlags::USE_TYPO_METRICS) {
                f32::from(os2.s_typo_ascender())
            } else { f32::from(hhea.ascender().to_i16()) };
            let delta = face.mvar().unwrap().metric_delta(skrifa::raw::tables::mvar::tags::HASC,
                location.coords()).unwrap_or_default().to_f64() as f32;
            eprintln!("Cantarell wght {value}, anchor fixture {has_anchor_delta}: HASC MVAR delta {delta}");
            if has_anchor_delta && value == axis.max { assert_eq!(delta, 100.); }
            assert!((metrics.ascent - (default_ascent + delta) * scale).abs() < 0.01);
            let top = position[1] + (default_ascent + delta - f32::from(glyph.bbox.y_max)) * scale;
            let mut expected = vec![0f32; 128 * 128];
            for pen in [position[0], position[0] + advance] {
                let image = reference_coverage(&glyph.curves, -pen / scale, -top / scale, scale, 128, 128);
                for (pixel, value) in expected.iter_mut().zip(image) { *pixel = (*pixel + value).min(1.); }
            }
            let mut renderer = TextRenderer::new(&device, config.format, &store);
            renderer.prepare(&device, &queue, &[paragraph], &store);
            let actual = draw::draw_pixels(&device, &queue, &mut renderer, [128, 128], wgpu::Color::BLACK);
            let mut max = 0f32;
            let mut total = 0f64;
            let mut bounds = [128usize, 128, 0, 0];
            for (index, expected) in expected.iter().enumerate() {
                let coverage = f32::from(actual[index * 4]) / 255.;
                let error = (coverage - expected).abs();
                max = max.max(error);
                total += f64::from(error);
                if coverage > 16. / 255. {
                    let (x, y) = (index % 128, index / 128);
                    bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x), bounds[3].max(y)];
                }
            }
            assert!(max <= C3_MAX_ERROR, "instance {value}: max error {max}");
            // Use C3's padded glyph area rather than diluting error over the viewport.
            let area = 2. * ((glyph.bbox.width() as f32 * scale).ceil() + 2.)
                * ((glyph.bbox.height() as f32 * scale).ceil() + 2.);
            let mean = total / f64::from(area);
            assert!(mean <= C3_MEAN_ERRORS[3].1, "instance {value}: mean error {mean}");
            assert!((bounds[0] as f32 - (position[0] + f32::from(expected_left) * scale)).abs() <= 1.,
                "ink left edge must follow the instance bearing");
            assert!((bounds[1] as f32 - top).abs() <= 1., "ink must follow the instance ascender");
            // Check the stem stripe against the existing centre-ray readback bound;
            // thin instances can intersect both edges within one box-filter sample.
            let mut centre = vec![0f32; 128 * 128];
            for pen in [position[0], position[0] + advance] {
                let image = CentreModel::new(&glyph.curves).image(
                    [-pen / scale, -top / scale], scale, 128, 128);
                for (pixel, value) in centre.iter_mut().zip(image) { *pixel = (*pixel + value).min(1.); }
            }
            let y = (top + glyph.bbox.height() as f32 * scale * 0.5) as usize;
            for x in 18..=22 {
                let coverage = f32::from(actual[(y * 128 + x) * 4]) / 255.;
                assert!((coverage - centre[y * 128 + x]).abs() <= READBACK_ERROR,
                    "stem stripe wght {value} at {x},{y}: GPU {coverage}, CPU {}", centre[y * 128 + x]);
            }
        }
        assert_ne!(widths[0], widths[1]);
        assert_ne!(outlines[0], outlines[1]);
    }
}

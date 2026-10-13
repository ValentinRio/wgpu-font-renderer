#![cfg(not(target_arch = "wasm32"))]

use owned_ttf_parser::AsFaceRef;
use wgpu_font_renderer::{FontStore, TextRenderer};

mod common;
#[path = "common/probe.rs"]
mod probe;
use probe::{assert_ink_tiles, probe_pixels};
#[path = "common/analytic.rs"]
#[allow(dead_code)]
mod analytic;

#[test]
fn shader_pipeline_validates() {
    let Some((device, queue, _config)) = common::gpu() else {
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
            let store = FontStore::new(&device);
            let mut renderer = TextRenderer::new(&device, config.format, &store);
            renderer.resize(&queue, [1024, 768]);
        }
        let error = scope.pop().await;
        assert!(error.is_none(), "Pipeline validation failed: {error:?}");
    });
}

#[test]
fn c6_bands_match_full_list_shader_pixels_and_grow_mid_load() {
    compare_shader_pixels();
}

fn compare_shader_pixels() {
    let Some((device, queue, _config)) = common::gpu() else {
        return;
    };
    for (bytes, grow) in [
        (&include_bytes!("../fonts/Roboto-Regular.ttf")[..], false),
        (&include_bytes!("../fonts/Cantarell-VF.otf")[..], false),
        (&include_bytes!("../fonts/Cantarell-VF.otf")[..], true),
    ] {
        let mut store = FontStore::new(&device);
        let mut preset =
            "igB@éABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".to_owned();
        if grow {
            let inspection_key = store.load_from_bytes(&device, &queue, bytes, "").unwrap();
            let face = wgpu_font_renderer::internals::font(&store, inspection_key).unwrap().face.as_face_ref();
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
                wgpu_font_renderer::internals::atlas_layer_count(&store) > 2,
                "band layers must grow mid-load"
            );
        }
        let font = wgpu_font_renderer::internals::font(&store, key).unwrap();
        let wrapped = preset.chars().find(|&c| {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            let glyph = &font.glyph_cache[&id];
            glyph.allocation.position()[0] + glyph.allocation.size() > 2048
        });
        assert!(
            wrapped.is_some(),
            "fixture must exercise row-crossing reads"
        );
        let old_layer_tail = preset.chars().find(|&c| {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            font.glyph_cache
                .get(&id)
                .and_then(|g| g.bands.as_ref())
                .is_some_and(|b| b.layer() == 1 && b.position()[1] >= 2016)
        });
        let new_layer = preset.chars().find(|&c| {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            font.glyph_cache
                .get(&id)
                .and_then(|g| g.bands.as_ref())
                .is_some_and(|b| b.layer() >= 3)
        });
        if grow {
            assert!(old_layer_tail.is_some() && new_layer.is_some());
        }
        // Growth probes cover old band-layer tails and new band layers.
        let probes: Vec<_> = if grow {
            old_layer_tail.into_iter().chain(new_layer).collect()
        } else {
            "igB@é".chars().chain(wrapped).collect()
        };
        for c in probes {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            let glyph = &font.glyph_cache[&id];
            let a = &glyph.allocation;
            let b = glyph.bands.as_ref().unwrap();
            let input = analytic::glyph_input(
                (a.position(), a.size(), a.layer()),
                (b.position(), b.layer()),
                1.,
            );
            let body = format!(
                r#"{input}
                let sizes = array(1., 12., 16., 24., 48., 100., 163., 200., 400., 557., 558., 1024.);
                input.pixel_scale = sizes[u32(p.y) / 64u] / {upem}.;
                input.uv = vec2(p.x / 256. * {width}. + {bearing}., (p.y % 64.) / 64. * {height}.);
                return vec4(0., 0., 0., analytic_coverage(input, vec3(0.)).y);
            "#,
                upem = font.face.as_face_ref().units_per_em(),
                width = glyph.bbox.width(),
                height = glyph.bbox.height(),
                bearing = glyph.left_side_bearing
            );
            let source = analytic::source(&body);
            let full = analytic::full(&source);
            let expected = probe_pixels(&device, &queue, wgpu_font_renderer::internals::atlas_view(&store), &full);
            assert_ink_tiles(&expected);
            let actual = probe_pixels(&device, &queue, wgpu_font_renderer::internals::atlas_view(&store), &source);
            let differences: Vec<_> = actual
                .as_chunks::<4>()
                .0
                .iter()
                .zip(expected.as_chunks::<4>().0)
                .enumerate()
                .filter(|(_, (a, e))| a != e)
                .take(8)
                .collect();
            assert!(
                differences.is_empty(),
                "glyph {c}, growth {grow}: {differences:?}"
            );
            let paths = probe_pixels(
                &device,
                &queue,
                wgpu_font_renderer::internals::atlas_view(&store),
                &analytic::band_debug(&source),
            );
            assert!(
                paths.as_chunks::<4>().0.iter().all(|p| p[3] == 0),
                "instrumented band path returns G=0 as alpha"
            );
        }
    }
}

#[test]
fn analytic_inside_opaque() {
    let Some((device, queue, _config)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "B",
        )
        .unwrap();
    let font = wgpu_font_renderer::internals::font(&store, key).unwrap();
    let glyph = &font.glyph_cache[&font.face.as_face_ref().glyph_index('B').unwrap()];
    let a = &glyph.allocation;
    let b = glyph.bands.as_ref().unwrap();
    let input = analytic::glyph_input(
        (a.position(), a.size(), a.layer()),
        (b.position(), b.layer()),
        1.,
    );
    let body = format!(
        r#"{input}
        let sizes = array(163., 200., 400., 1024.);
        input.pixel_scale = sizes[min(u32(p.y) / 64u, 3u)] / 2048.;
        input.uv = vec2({bearing}. + .08 * {width}., .5 * {height}.);
        return vec4(analytic_coverage(input, vec3(0.)), 1.);
    "#,
        bearing = glyph.left_side_bearing,
        width = glyph.bbox.width(),
        height = glyph.bbox.height()
    );
    let pixels = probe_pixels(
        &device,
        &queue,
        wgpu_font_renderer::internals::atlas_view(&store),
        &analytic::source(&body),
    );
    assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[0] == 255));
}

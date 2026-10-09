#![cfg(not(target_arch = "wasm32"))]

use wgpu_font_renderer::{FontStore, TextRenderer};

mod common;
#[path = "common/probe.rs"]
mod probe;
use probe::{assert_ink_tiles, probe_pixels};

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

// Full-list reference with corrected linear atlas reads.
#[test]
fn bands_match_full_list_shader_pixels() {
    compare_shader_pixels(false);
}

#[test]
fn unwrapped_layer_zero_matches_frozen_main_shader_pixels() {
    compare_shader_pixels(true);
}

fn compare_shader_pixels(frozen_main: bool) {
    use owned_ttf_parser::AsFaceRef;
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    for (bytes, grow) in [
        (&include_bytes!("../examples/Roboto-Regular.ttf")[..], false),
        (&include_bytes!("fixtures/Cantarell-VF.otf")[..], false),
        (&include_bytes!("fixtures/Cantarell-VF.otf")[..], true),
    ] {
        if frozen_main && grow {
            continue;
        }
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
        let probes: Vec<_> = if frozen_main {
            "igB@é".chars().collect()
        } else if grow {
            old_layer_tail.into_iter().chain(new_layer).collect()
        } else {
            "igB@é".chars().chain(wrapped).collect()
        };
        let mut compared = 0;
        for c in probes {
            let id = font.face.as_face_ref().glyph_index(c).unwrap();
            let glyph = &font.glyph_cache[&id];
            let [x, y] = glyph.allocation.position();
            if frozen_main && (glyph.allocation.layer() != 0 || x + glyph.allocation.size() > 2048)
            {
                continue;
            }
            compared += 1;
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
            let reference = if frozen_main {
                include_str!("fixtures/original-main.wgsl")
            } else {
                include_str!("fixtures/full-list.wgsl")
            };
            let candidate = include_str!("../src/shader.wgsl");
            let source = |shader: &str, band_input: &str| {
                let shader = shader
                    // Only update the frozen oracle's varying interface; its
                    // curve loop and coverage math remain unchanged.
                    .replace("@location(5) atlas_pos: vec2<f32>,\n    @location(6) @interpolate(flat)",
                        "@location(5) @interpolate(flat) atlas_pos: vec2<f32>,\n    @location(6) @interpolate(flat)")
                    .replace("@location(8) layer: f32,", "@location(8) @interpolate(flat) layer: f32,")
                    .replace("    uv.y = remap(uv.y, 0., 1., 0., input.size.y * input.units_per_em / font_size);",
                        "    uv.y = remap(uv.y, 0., 1., 0., input.size.y * input.units_per_em / font_size);\n    if input.pos.y == 1. { uv.y = input.pos.x; }")
                    .replace("@fragment\nfn fs_main", "fn coverage")
                    .replace(
                        "fn coverage(input: VertexOutput) -> @location(0) vec4<f32>",
                        "fn coverage(input: VertexOutput) -> vec4<f32>",
                    );
                format!("{shader}\nfn probe_float(offset: i32, layer: i32) -> f32 {{ let texel = offset / 4; let width = i32(textureDimensions(atlas_texture).x); return textureLoad(atlas_texture, vec2<i32>(texel % width, texel / width), layer, 0)[offset % 4]; }}\n{}", probe.replace("BAND_INPUT", band_input))
            };
            let expected = probe_pixels(
                &device,
                &queue,
                store.atlas().view(),
                &source(reference, ""),
            );
            assert_ink_tiles(&expected);
            let actual = probe_pixels(
                &device,
                &queue,
                store.atlas().view(),
                &source(
                    candidate,
                    &format!("input.bands = vec3({bx}., {by}., {}.);", bands.layer()),
                ),
            );
            if grow || c == 'i' || Some(c) == wrapped {
                // Instrument only the test source; no diagnostic branch or
                // varying is compiled into the release shader.
                let debug = source(candidate, &format!("input.bands = vec3({bx}., {by}., {}.);", bands.layer()))
                    .replace("    var nearest =", "    var probe_used_band = false;\n    var nearest =")
                    .replace("nearest = band_curves(uv, start, curve_count, winding_start, winding_count, layer, window);",
                        "nearest = band_curves(uv, start, curve_count, winding_start, winding_count, layer, window);\n            probe_used_band = nearest.w != FULL_LIST_REQUIRED;")
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
                "glyph {c}: band output must match the full list at every sampled size"
            );
        }
        if frozen_main {
            assert!(
                compared >= 3,
                "must compare several correctly placed glyphs"
            );
        }
    }
}

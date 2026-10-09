// Loader-level tests need access to the private band builder.
#![cfg(not(target_arch = "wasm32"))]

use super::{build_bands, BezierBuilder};
use crate::atlas_tests::{common, probe};
use owned_ttf_parser::OutlineBuilder;

#[test]
fn winding_includes_crossings_when_f32_rounds_into_neighbor_band() {
    let mut glyph = BezierBuilder::new(0.);
    // Font-space y is flipped by the builder. This rectangle ends just below
    // the encoded band boundary at zero, with a pixel strictly inside it.
    glyph.move_to(0., 1.);
    glyph.line_to(10., 1.);
    glyph.line_to(10., 0.00005);
    glyph.line_to(0., 0.00005);
    glyph.close();
    for y in [-4096., 4096.] {
        glyph.move_to(20., y);
        glyph.line_to(21., y);
        glyph.close();
    }
    let data = build_bands(&glyph.curves);
    assert_eq!(&data[..3], &[-4096., 512., 16.]);
    let uv = [5., -0.0001_f32];
    let band = ((uv[1] - data[0]) / data[1]).floor() as usize;
    assert_eq!(band, 8);
    assert!(uv[1] < data[0] + band as f32 * data[1]);
    let start = data[12 + band * 8] as usize;
    let len = data[13 + band * 8] as usize;
    // The right vertical edge crosses the pixel's rightward ray. Use the
    // shader's strict endpoint test, independently of the overlap predicate.
    let crossings = |curves: &[f32]| -> Vec<[f32; 8]> {
        curves
            .as_chunks::<8>()
            .0
            .iter()
            .copied()
            .filter(|s| {
                s[0] == s[4]
                    && s[0] > uv[0]
                    && ((uv[1] > s[1] && uv[1] < s[5]) || (uv[1] > s[5] && uv[1] < s[1]))
            })
            .collect()
    };
    let full = crossings(&glyph.curves);
    assert_eq!(full.len(), 1);
    assert_eq!(
        crossings(&data[start..start + len]),
        full,
        "rounded neighboring band must retain the full-list ray crossing"
    );
    let retained: Vec<_> = glyph
        .curves
        .as_chunks::<8>()
        .0
        .iter()
        .filter(|s| data[start..start + len].as_chunks::<8>().0.contains(s))
        .flat_map(|s| *s)
        .collect();
    assert_eq!(&data[start..start + len], retained);
}

#[test]
fn first_distance_curve_exact_zero_matches_fixed_full_list_pixel() {
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    let mut atlas = crate::atlas::Atlas::new(&device, &config);
    let mut glyph = BezierBuilder::new(0.);
    glyph.move_to(3000., -3000.);
    glyph.line_to(3200., -3000.);
    glyph.line_to(3200., -3200.);
    glyph.line_to(3000., -3200.);
    glyph.close();
    // At these coordinates the shader's 1e-4 control perturbation rounds away;
    // a midpoint control gives an exact straight-segment distance of zero.
    glyph.curves[2] = 3100.;
    let data = build_bands(&glyph.curves);
    let start = data[8] as usize;
    assert_eq!(data[start + 6], 0., "first distance curve is the hit edge");
    assert_eq!(&data[start..start + 6], &glyph.curves[..6]);
    let mut encoder = device.create_command_encoder(&Default::default());
    let flat = atlas
        .upload(
            glyph.curves.len() as u32,
            bytemuck::cast_slice(&glyph.curves),
            &device,
            &mut encoder,
            &queue,
        )
        .unwrap();
    let bands = atlas
        .upload_bands(
            data.len() as u32,
            bytemuck::cast_slice(&data),
            &device,
            &mut encoder,
            &queue,
        )
        .unwrap();
    queue.submit(Some(encoder.finish()));
    let [x, y] = flat.position();
    let [bx, by] = bands.position();
    let source = |shader: &str| {
        let band_input = if shader.contains("bands: vec3<f32>") {
            format!("input.bands = vec3({bx}., {by}., {}.);", bands.layer())
        } else {
            String::new()
        };
        let shader = shader
            .replace("@fragment\nfn fs_main", "fn coverage")
            .replace(
                "fn coverage(input: VertexOutput) -> @location(0) vec4<f32>",
                "fn coverage(input: VertexOutput) -> vec4<f32>",
            );
        format!(
            r#"{shader}
@vertex
fn probe_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {{
    let p = array(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.));
    return vec4(p[index], 0., 1.);
}}
@fragment
fn probe_fragment() -> @location(0) vec4<f32> {{
    var input: VertexOutput;
    input.font_size = 100.;
    input.uv = vec2(.5, 3000.);
    input.size = vec2(200., 1.);
    input.units_per_em = 100.;
    input.left_side_bearing = 3000.;
    input.atlas_pos = vec2({x}., {y}.);
    input.atlas_size = {count};
    input.layer = {layer}.;
    input.color = vec4(0., 0., 0., 1.);
    {band_input}
    return coverage(input);
}}
"#,
            count = flat.size(),
            layer = flat.layer()
        )
    };
    let candidate = source(include_str!("../../src/shader.wgsl"));
    let hit = candidate.replace(
        "return coverage(input);",
        r#"
    let d = sd_bezier(vec2(3000., 3000.), vec2(3100., 3000.),
        vec2(3200., 3000.), vec2(3100., 3000.));
    return vec4(select(0., 1., d == 0.), 0., 0., 1.);
"#,
    );
    assert_eq!(
        probe::probe_pixels(&device, &queue, atlas.view(), &hit)[0],
        255,
        "first distance curve must have an exact-zero GPU distance"
    );
    let expected = probe::probe_pixels(
        &device,
        &queue,
        atlas.view(),
        &source(include_str!("../fixtures/full-list.wgsl")),
    );
    probe::assert_ink_tiles(&expected);
    assert!(
        expected[3] > 64 && expected[3] < 192,
        "edge pixel must have partial coverage"
    );
    let actual = probe::probe_pixels(&device, &queue, atlas.view(), &candidate);
    assert_eq!(
        actual, expected,
        "first-curve zero must match fixed full_list pixels"
    );
}

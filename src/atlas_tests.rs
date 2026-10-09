use crate::{atlas, loader};
#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/probe.rs"]
mod probe;
use probe::{assert_ink_tiles, probe_pixels};

#[test]
fn nonzero_layer_pixels_match_layer_zero() {
    placement_regression(2048 * 2048, 0, false);
}

#[test]
fn row_wrapped_pixels_match_unwrapped() {
    for prefix in [2048 - 8, 2048 - 3] {
        placement_regression(prefix, 0, false);
    }
}

#[test]
fn nonzero_layer_uses_bands_at_400() {
    placement_regression(2048 * 2048, 0, true);
}

#[test]
fn row_wrapped_uses_bands_at_400() {
    placement_regression(2048 - 3, 0, true);
}

#[test]
fn row_wrapped_band_data_matches_unwrapped_and_uses_bands_at_400() {
    placement_regression(0, 2048 - 3, true);
}

fn placement_regression(prefix: u32, band_prefix: u32, check_path: bool) {
    use owned_ttf_parser::AsFaceRef;
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    for (c, bytes) in [
        ('g', &include_bytes!("../examples/Roboto-Regular.ttf")[..]),
        ('B', &include_bytes!("../examples/Roboto-Regular.ttf")[..]),
        (
            '@',
            &include_bytes!("../tests/fixtures/Cantarell-VF.otf")[..],
        ),
    ] {
        let mut results = Vec::new();
        for (padding, band_padding) in [(0, 0), (prefix, band_prefix)] {
            let mut atlas = atlas::Atlas::new(&device, &config);
            let mut encoder = device.create_command_encoder(&Default::default());
            if padding > 0 {
                atlas
                    .upload(
                        padding,
                        &vec![0; padding as usize * 4],
                        &device,
                        &mut encoder,
                        &queue,
                    )
                    .unwrap();
            }
            if band_padding > 0 {
                atlas
                    .upload_bands(
                        band_padding,
                        &vec![0; band_padding as usize * 4],
                        &device,
                        &mut encoder,
                        &queue,
                    )
                    .unwrap();
            }
            let font = loader::Font::from_bytes(
                &device,
                &mut encoder,
                &queue,
                bytes.to_vec(),
                0,
                &c.to_string(),
                &mut atlas,
            )
            .unwrap();
            queue.submit(Some(encoder.finish()));
            let glyph = &font.glyph_cache[&font.face.as_face_ref().glyph_index(c).unwrap()];
            let [x, y] = glyph.allocation.position();
            let layer = glyph.allocation.layer();
            if padding == 2048 * 2048 {
                assert!(layer > 0);
            }
            if padding > 0 && padding < 2048 {
                assert!(x + glyph.allocation.size() > 2048);
            }
            let bands = glyph.bands.as_ref().unwrap();
            let [bx, by] = bands.position();
            if band_padding > 0 {
                assert!(
                    bx > 0 && bx + bands.size() > 2048,
                    "glyph {c}: band allocation must start at a nonzero column and wrap"
                );
            }
            let shader = include_str!("shader.wgsl")
                .replace("@fragment\nfn fs_main", "fn coverage")
                .replace(
                    "fn coverage(input: VertexOutput) -> @location(0) vec4<f32>",
                    "fn coverage(input: VertexOutput) -> vec4<f32>",
                );
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
    input.size = vec2({width}., {height}.) * input.font_size / 2048.;
    input.units_per_em = 2048.;
    input.left_side_bearing = {bearing}.;
    input.atlas_pos = vec2({x}., {y}.);
    input.atlas_size = {count};
    input.layer = {layer}.;
    input.color = vec4(0., 0., 0., 1.);
    input.bands = vec3({bx}., {by}., {band_layer}.);
    return coverage(input);
}}
"#,
                width = glyph.bbox.width(),
                height = glyph.bbox.height(),
                bearing = glyph.left_side_bearing,
                count = glyph.allocation.size(),
                band_layer = bands.layer()
            );
            let source = format!("{shader}\n{probe}");
            if check_path {
                let debug = source
                .replace("    var nearest =", "    var probe_used_band = false;\n    var nearest =")
                .replace("nearest = band_curves(uv, start, curve_count, layer);",
                    "nearest = band_curves(uv, start, curve_count, layer);\n            probe_used_band = nearest.w != FULL_LIST_REQUIRED;")
                .replace("return vec4(input.color.rgb, 1 - triplet_alpha.r);",
                    "return vec4(select(0., 1., probe_used_band), 0., 0., 1.);");
                let pixels = probe_pixels(&device, &queue, atlas.view(), &debug);
                assert!(
                    pixels[8 * 256 * 64 * 4..9 * 256 * 64 * 4]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|p| p[0] == 255),
                    "glyph {c} at [{x}, {y}], layer {layer} must use bands at size 400"
                );
            }
            let full = source.replace(
                "if window <= BAND_MARGIN",
                "if false && window <= BAND_MARGIN",
            );
            let reference = probe_pixels(&device, &queue, atlas.view(), &full);
            assert_ink_tiles(&reference);
            results.push((
                probe_pixels(&device, &queue, atlas.view(), &source),
                reference,
            ));
        }
        let differences = |a: &[u8], b: &[u8]| {
            a.as_chunks::<4>()
                .0
                .iter()
                .zip(b.as_chunks::<4>().0)
                .filter(|(a, b)| a != b)
                .count()
        };
        assert_eq!(
            differences(&results[0].1, &results[1].1),
            0,
            "full-list placement must not change pixels"
        );
        assert_eq!(
            differences(&results[0].0, &results[1].0),
            0,
            "banded placement must not change pixels"
        );
        for (banded, full) in results {
            assert_eq!(differences(&banded, &full), 0, "bands must match full list");
        }
    }
}

const SD_UNSIGNED_VERTEX: &str = r#"
@vertex
fn probe_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = array(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.));
    return vec4(p[index], 0., 1.);
}
"#;

fn sd_unsigned_distance_probe(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &wgpu::TextureView,
    points: [[f32; 2]; 4],
) -> f32 {
    let [a, b, c, p] = points;
    let source = format!(
        r#"{}
{SD_UNSIGNED_VERTEX}
@fragment
fn probe_fragment() -> @location(0) vec4<f32> {{
    let distance = sd_bezier(vec2({:?}, {:?}), vec2({:?}, {:?}), vec2({:?}, {:?}), vec2({:?}, {:?}));
    let bits = bitcast<u32>(distance);
    return vec4<f32>(f32(bits & 255u), f32((bits >> 8u) & 255u),
        f32((bits >> 16u) & 255u), f32(bits >> 24u)) / 255.;
}}
"#,
        include_str!("shader.wgsl"),
        a[0], a[1], b[0], b[1], c[0], c[1], p[0], p[1],
    );
    let pixels = probe_pixels(device, queue, atlas, &source);
    f32::from_le_bytes(pixels[..4].try_into().unwrap())
}

fn sd_unsigned_segment_distance(a: [f64; 2], c: [f64; 2], p: [f64; 2]) -> f64 {
    let segment = [c[0] - a[0], c[1] - a[1]];
    let squared_length = segment[0] * segment[0] + segment[1] * segment[1];
    let t = if squared_length == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * segment[0] + (p[1] - a[1]) * segment[1]) / squared_length)
            .clamp(0.0, 1.0)
    };
    (a[0] + t * segment[0] - p[0]).hypot(a[1] + t * segment[1] - p[1])
}

#[test]
fn sd_unsigned_straight_extension_distance() {
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    let atlas = atlas::Atlas::new(&device, &config);
    let got = sd_unsigned_distance_probe(
        &device, &queue, atlas.view(),
        [[0., 100.], [0., 101.], [0., 101.], [0., 10.]],
    );
    let expected = sd_unsigned_segment_distance([0., 100.], [0., 101.], [0., 10.]);
    assert!((f64::from(got) - expected).abs() <= 1e-3,
        "straight extension distance: got {got:?}, expected {expected}");
}

#[test]
fn sd_unsigned_midpoint_and_zero_length_distance() {
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    let atlas = atlas::Atlas::new(&device, &config);
    for end in [[3002., 3002.], [3000., 3000.]] {
        let start = [3000., 3000.];
        let control = [(start[0] + end[0]) / 2., (start[1] + end[1]) / 2.];
        let p = [3000., 3010.];
        let got = sd_unsigned_distance_probe(&device, &queue, atlas.view(), [start, control, end, p]);
        let expected = sd_unsigned_segment_distance(
            start.map(f64::from), end.map(f64::from), p.map(f64::from),
        );
        assert!(got.is_finite(), "midpoint distance must be finite: got {got:?}, end {end:?}");
        assert!((f64::from(got) - expected).abs() <= 1e-3,
            "midpoint distance: got {got:?}, expected {expected}, end {end:?}");
    }
}

#[test]
fn sd_unsigned_straight_contour_pixel() {
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    let mut atlas = atlas::Atlas::new(&device, &config);
    let mut curves = Vec::<f32>::new();
    for contour in [
        [[0., 101.], [0., 100.], [1., 101.], [0., 101.]],
        [[1000., 0.], [1001., 0.], [1000., 1.], [1000., 0.]],
    ] {
        for edge in contour.windows(2) {
            curves.extend_from_slice(&[
                edge[0][0], edge[0][1], edge[1][0], edge[1][1],
                edge[1][0], edge[1][1], 0., 0.,
            ]);
        }
    }
    let mut encoder = device.create_command_encoder(&Default::default());
    let allocation = atlas.upload(
        curves.len() as u32, bytemuck::cast_slice(&curves), &device, &mut encoder, &queue,
    ).unwrap();
    queue.submit(Some(encoder.finish()));
    let [x, y] = allocation.position();
    let shader = include_str!("shader.wgsl")
        .replace("@fragment\nfn fs_main", "fn coverage")
        .replace("fn coverage(input: VertexOutput) -> @location(0) vec4<f32>",
            "fn coverage(input: VertexOutput) -> vec4<f32>");
    let band_input = if shader.contains("bands: vec3<f32>") {
        "input.bands = vec3(0., 0., -1.);"
    } else {
        ""
    };
    let source = format!(r#"{shader}
{SD_UNSIGNED_VERTEX}
@fragment
fn probe_fragment() -> @location(0) vec4<f32> {{
    var input: VertexOutput;
    input.font_size = 100.;
    input.uv = vec2(1./3., 10.);
    input.size = vec2(100., 100.);
    input.units_per_em = 1.;
    input.left_side_bearing = 0.;
    input.atlas_pos = vec2({x}., {y}.);
    input.atlas_size = {count};
    input.layer = {layer}.;
    input.color = vec4(0., 0., 0., 1.);
    {band_input}
    return coverage(input);
}}
"#, count = allocation.size(), layer = allocation.layer());
    let pixels = probe_pixels(&device, &queue, atlas.view(), &source);
    let alpha = f32::from(pixels[3]) / 255.;
    assert!(alpha <= 1. / 255., "straight contour pixel alpha: got {alpha}");
}

#[test]
fn sd_unsigned_exact_zero_edge_coverage() {
    let Some((device, queue, config)) = common::gpu() else {
        return;
    };
    let mut atlas = atlas::Atlas::new(&device, &config);
    assert_eq!(sd_unsigned_distance_probe(
        &device, &queue, atlas.view(),
        [[3000., 3000.], [3100., 3000.], [3200., 3000.], [3100., 3000.]],
    ), 0.0);
    let contour = [
        [3000., 3000.], [3200., 3000.], [3200., 3200.],
        [3000., 3200.], [3000., 3000.],
    ];
    let mut curves = Vec::<f32>::new();
    for edge in contour.windows(2) {
        curves.extend_from_slice(&[
            edge[0][0], edge[0][1],
            (edge[0][0] + edge[1][0]) / 2., (edge[0][1] + edge[1][1]) / 2.,
            edge[1][0], edge[1][1], 0., 0.,
        ]);
    }
    let mut encoder = device.create_command_encoder(&Default::default());
    let allocation = atlas.upload(
        curves.len() as u32, bytemuck::cast_slice(&curves), &device, &mut encoder, &queue,
    ).unwrap();
    queue.submit(Some(encoder.finish()));
    let [x, y] = allocation.position();
    let shader = include_str!("shader.wgsl")
        .replace("@fragment\nfn fs_main", "fn coverage")
        .replace("fn coverage(input: VertexOutput) -> @location(0) vec4<f32>",
            "fn coverage(input: VertexOutput) -> vec4<f32>");
    let source = format!(r#"{shader}
{SD_UNSIGNED_VERTEX}
@fragment
fn probe_fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
    var input: VertexOutput;
    input.font_size = 100.;
    input.uv = vec2(.5, 3000. + p.y - .5);
    input.size = vec2(200., 1.);
    input.units_per_em = 100.;
    input.left_side_bearing = 3000.;
    input.atlas_pos = vec2({x}., {y}.);
    input.atlas_size = {count};
    input.layer = {layer}.;
    input.color = vec4(0., 0., 0., 1.);
    input.bands = vec3(0., 0., -1.);
    return coverage(input);
}}
"#, count = allocation.size(), layer = allocation.layer());
    let pixels = probe_pixels(&device, &queue, atlas.view(), &source);
    let alpha = f32::from(pixels[3]) / 255.;
    assert!(alpha > 0.25 && alpha < 0.75, "exact-zero edge pixel alpha: got {alpha}");
}

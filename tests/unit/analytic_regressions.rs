use super::{build_slug_bands, BezierBuilder};
use crate::atlas_tests::{common, probe};
use owned_ttf_parser::OutlineBuilder;

fn square(lo: [f32; 2], hi: [f32; 2], reverse: bool) -> Vec<f32> {
    let mut points = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
    if reverse {
        points.reverse();
    }
    (0..4)
        .flat_map(|i| {
            let a = points[i];
            let b = points[(i + 1) % 4];
            [a[0], a[1], b[0], b[1], b[0], b[1], 0., 0.]
        })
        .collect()
}

fn pixels(
    curves: &[f32],
    origin: [f32; 2],
    scale: f32,
    weighted: bool,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let (device, queue, config) = common::gpu()?;
    let mut atlas = crate::atlas::Atlas::new(&device, &config);
    let mut encoder = device.create_command_encoder(&Default::default());
    let a = atlas
        .upload(
            curves.len() as u32,
            bytemuck::cast_slice(curves),
            &device,
            &mut encoder,
            &queue,
        )
        .unwrap();
    let data = build_slug_bands(curves);
    let b = atlas
        .upload_bands(
            data.len() as u32,
            bytemuck::cast_slice(&data),
            &device,
            &mut encoder,
            &queue,
        )
        .unwrap();
    queue.submit(Some(encoder.finish()));
    let [x, y] = a.position();
    let [bx, by] = b.position();
    let shader = include_str!("../../src/shader.wgsl").replace(
        "const WEIGHTED_COVERAGE: bool = true;",
        &format!("const WEIGHTED_COVERAGE: bool = {weighted};"),
    );
    let source = format!(
        r#"{shader}
@vertex fn probe_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {{
    let p = array(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.));
    return vec4(p[index], 0., 1.);
}}
@fragment fn probe_fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
    var input: VertexOutput;
    input.uv = vec2({ox:?}, {oy:?}) + p.xy / {scale:?};
    input.pixel_scale = {scale:?};
    input.atlas_pos = vec2({x}., {y}.);
    input.atlas_size = {count};
    input.layer = {layer}.;
    input.bands = vec3({bx}., {by}., {blayer}.);
    return vec4(analytic_coverage(input, vec3(0.)), 1.);
}}
"#,
        ox = origin[0],
        oy = origin[1],
        count = a.size(),
        layer = a.layer(),
        blayer = b.layer()
    );
    let full = source.replace(
        "if input.bands.z >= 0. && input.atlas_size > 0 {",
        "if false {",
    );
    assert_ne!(full, source);
    let actual = probe::probe_pixels(&device, &queue, atlas.view(), &source);
    let expected = probe::probe_pixels(&device, &queue, atlas.view(), &full);
    assert_eq!(
        actual, expected,
        "bands including early exit must match all curves"
    );
    Some((actual, expected))
}

#[test]
fn c4_nonzero_overlapping_squares_and_nested_hole() {
    for hole in [false, true] {
        let mut curves = square([0., 0.], [8., 8.], false);
        curves.extend(square(
            [2., 2.],
            if hole { [6., 6.] } else { [10., 10.] },
            hole,
        ));
        let Some((actual, _)) = pixels(&curves, [0., 0.], 1., true) else {
            return;
        };
        let reference = common::reference::reference_coverage(&curves, 0., 0., 1., 12, 12);
        for y in 0..12 {
            for x in 0..12 {
                assert!(
                    (actual[(y * 256 + x) * 4] as f32 / 255. - reference[y * 12 + x]).abs()
                        <= 1. / 255.
                );
            }
        }
        assert_eq!(actual[(3 * 256 + 3) * 4], if hole { 0 } else { 255 });
        if !hole {
            assert_eq!(actual[(9 * 256 + 9) * 4], 255);
        }
    }
}

#[test]
fn c5_endpoints_extrema_lines_and_degenerate_records_have_no_streaks() {
    let mut curves = square([20.5, 20.5], [80.5, 80.5], false);
    // Curved cap: endpoints and its extremum all coincide with pixel centres.
    curves.extend([
        100.5, 40.5, 120.5, 0.5, 140.5, 40.5, 0., 0., 140.5, 40.5, 100.5, 40.5, 100.5, 40.5, 0., 0.,
    ]);
    // Linear polynomial and zero-length point; both must stay finite.
    curves.extend([
        160.5, 40.5, 170.5, 40.5, 180.5, 40.5, 0., 0., 180.5, 40.5, 170.5, 40.5, 160.5, 40.5, 0.,
        0., 200.5, 40.5, 200.5, 40.5, 200.5, 40.5, 0., 0.,
    ]);
    let Some((actual, _)) = pixels(&curves, [0., 0.], 1., true) else {
        return;
    };
    use common::centre::CentreModel;
    use common::coverage_limits::{
        C3_MAX_ERROR, C5_NO_STREAK_ERROR, C5_STRAIGHT_ERROR, READBACK_ERROR,
    };
    let reference = common::reference::reference_coverage(&curves, 0., 0., 1., 256, 768);
    let centre = CentreModel::new(&curves).image([0., 0.], 1., 256, 768);
    let vertices: Vec<_> = curves
        .as_chunks::<8>()
        .0
        .iter()
        .map(|r| [r[0], r[1]])
        .collect();
    let (mut method_max, mut away_max, mut nearby_max) = (0_f32, 0_f32, 0_f32);
    let (mut method_worst, mut away_worst) = (String::new(), String::new());
    let mut away_count = 0;
    let mut straight_max = 0_f32;
    for y in 0..768 {
        for x in 0..256 {
            let index = y * 256 + x;
            let a = actual[index * 4] as f32 / 255.;
            let difference = (a - centre[index]).abs();
            if difference > method_max {
                method_max = difference;
                method_worst = format!("pixel {x},{y}: GPU {a:.8}, CPU {:.8}", centre[index]);
            }
            // Distance from the entire pixel box to each on-curve vertex.
            // This excludes pixels containing a vertex and its one-pixel halo.
            let away = vertices.iter().all(|v| {
                let dx = (x as f32 - v[0]).max(v[0] - (x + 1) as f32).max(0.);
                let dy = (y as f32 - v[1]).max(v[1] - (y + 1) as f32).max(0.);
                dx.hypot(dy) > 1.
            });
            let error = (a - reference[index]).abs();
            if away {
                away_count += 1;
                // Regions containing only the square's straight edges or empty
                // space; the curved cap occupies x=100.5..140.5.
                if !(90..=145).contains(&x) {
                    straight_max = straight_max.max(error);
                }
                if error > away_max {
                    away_max = error;
                    away_worst = format!("pixel {x},{y}: GPU {a:.8}, box {:.8}", reference[index]);
                }
            } else {
                nearby_max = nearby_max.max(error);
            }
        }
    }
    eprintln!(
        "C5 method: pixels {}, worst {method_max:.8}; {method_worst}",
        centre.len()
    );
    eprintln!("C5 no streaks: pixels {away_count}, worst {away_max:.8}; {away_worst}; nearby max {nearby_max:.8}");
    eprintln!("C5 straight no-streak worst {straight_max:.8}");
    assert!(
        method_max <= READBACK_ERROR,
        "GPU/CPU method: {method_worst}"
    );
    assert!(
        away_max <= C5_NO_STREAK_ERROR,
        "no-streak error {away_max:.8}: {away_worst}"
    );
    assert!(
        straight_max <= C5_STRAIGHT_ERROR,
        "straight-edge streak error {straight_max:.8}"
    );
    assert!(
        nearby_max <= C3_MAX_ERROR,
        "near-vertex error {nearby_max:.8}"
    );
}

#[test]
fn thin_horizontal_and_vertical_features_compare_combination_methods() {
    for vertical in [false, true] {
        let hi = if vertical { [20.75, 64.] } else { [64., 20.75] };
        let lo = if vertical { [20.25, 0.] } else { [0., 20.25] };
        let curves = square(lo, hi, false);
        let reference = common::reference::reference_coverage(&curves, 0., 0., 1., 64, 64);
        let mut results = Vec::new();
        for weighted in [false, true] {
            let Some((actual, _)) = pixels(&curves, [0., 0.], 1., weighted) else {
                return;
            };
            let mut max = 0_f32;
            let mut sum = 0_f64;
            for y in 0..64 {
                for x in 0..64 {
                    let error =
                        (actual[(y * 256 + x) * 4] as f32 / 255. - reference[y * 64 + x]).abs();
                    max = max.max(error);
                    sum += f64::from(error);
                }
            }
            eprintln!(
                "thin vertical={vertical} weighted={weighted}: max {max:.6}, mean {:.6}",
                sum / 4096.
            );
            results.push(sum);
        }
        assert!(results[1] < results[0]);
    }
}

#[test]
fn rounded_neighbor_band_retains_ray_crossings() {
    let mut glyph = BezierBuilder::new(0.);
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
    let data = build_slug_bands(&glyph.curves);
    let band = ((-0.0001_f32 - data[1]) / data[3]).floor() as usize;
    assert_eq!(band, 8);
    let start = data[8 + band * 8] as usize;
    let len = data[9 + band * 8] as usize;
    assert!(data[start..start + len]
        .as_chunks::<8>()
        .0
        .iter()
        .any(|s| s[0] == 10.
            && s[4] == 10.
            && s[1].min(s[5]) < -0.0001
            && s[1].max(s[5]) > -0.0001));
}

#[test]
fn straight_extensions_midpoints_zero_length_and_exact_edges_have_correct_coverage() {
    let mut curves = square([3000., 3000.], [3200., 3200.], false);
    // Exercise the near-linear polynomial alongside control=end line records.
    curves[2] = 3100.;
    curves.extend([3400., 3000., 3400., 3000., 3400., 3000., 0., 0.]);
    let Some((actual, _)) = pixels(&curves, [2999.5, 2999.5], 1., true) else {
        return;
    };
    let edge = actual[100 * 4] as f32 / 255.;
    assert!(
        (edge - 0.5).abs() <= 1. / 255.,
        "exact horizontal edge: {edge}"
    );
    assert_eq!(actual[220 * 4], 0, "line extension must have no coverage");
    assert_eq!(
        actual[(201 * 256 + 100) * 4],
        0,
        "outside the endpoint range"
    );
    let Some((point, _)) = pixels(&curves[32..], [3399.5, 2999.5], 1., true) else {
        return;
    };
    assert!(
        point.as_chunks::<4>().0.iter().all(|p| p[..3] == [0; 3]),
        "zero-length record must stay finite and empty"
    );
}

#[test]
fn diagonal_edge_and_extensions_match_full_list() {
    let contour = [[0., 0.], [128., 128.], [128., 512.], [0., 384.], [0., 0.]];
    let curves: Vec<f32> = contour
        .windows(2)
        .flat_map(|edge| {
            [
                edge[0][0],
                edge[0][1],
                (edge[0][0] + edge[1][0]) * 0.5,
                (edge[0][1] + edge[1][1]) * 0.5,
                edge[1][0],
                edge[1][1],
                0.,
                0.,
            ]
        })
        .collect();
    let Some((actual, _)) = pixels(&curves, [63.5, 63.5], 1., true) else {
        return;
    };
    for offset in [0, 96, 192] {
        let origin = 63.5 + offset as f32;
        // The diagonal bisects this pixel exactly. Unlike box area, the
        // 16x16 oracle has sample-centre ties on this diagonal (1/32 bias).
        let expected = if offset == 0 { 0.5 } else { 0. };
        let observed = actual[(offset * 256 + offset) * 4] as f32 / 255.;
        assert!(
            (observed - expected).abs() <= 1. / 255.,
            "diagonal at {origin}: {observed}, {expected}"
        );
    }
}

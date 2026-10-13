use super::analytic_regressions::pixels;
use crate::atlas_tests::{common, probe};
use common::centre::CentreModel;
use common::coverage_limits::{C5_NO_STREAK_ERROR, READBACK_ERROR};
#[path = "../common/analytic.rs"]
#[allow(dead_code)]
mod analytic;

// Every extremum is on a pixel centre, at t=m/(m+n): 1/3, 2/5, 2/7,
// 3/7, 3/11 and 5/13. The dyadic control values give exact extrema while
// none of these root parameters is representable as a binary float.
fn caps(transpose: bool) -> Vec<f32> {
    let mut curves = Vec::new();
    for (index, (m, n)) in [(1., 2.), (2., 3.), (2., 5.), (3., 4.), (3., 8.), (5., 8.)]
        .into_iter()
        .enumerate()
    {
        let x = 8.125 + (index % 3) as f32 * 68.;
        let y = 20.5 + (index / 3) as f32 * 120.;
        let length = (m + n) * 4.;
        // Endpoints stay off centre lines throughout the sweep, isolating the
        // interior-extremum rule from cross-curve endpoint classification.
        let curvature = 131. / 128.;
        let a = [x, y + curvature * m * m];
        let c = [x + length * 0.5, y - curvature * m * n];
        let b = [x + length, y + curvature * n * n];
        for points in [[a, c, b], [b, a, a]] {
            for p in points {
                curves.extend(if transpose { [p[1], p[0]] } else { p });
            }
            curves.extend([0., 0.]);
        }
    }
    curves
}

#[test]
fn non_dyadic_tangents_match_method_and_box_filter_on_rows_and_columns() {
    let mut method_max = 0_f32;
    let mut box_max = 0_f32;
    let mut count = 0;
    let mut method_count = 0;
    for transpose in [false, true] {
        let curves = caps(transpose);
        let Some((actual, _)) = pixels(&curves, [0., 0.], 1., true) else {
            return;
        };
        let centre = CentreModel::new(&curves).image([0., 0.], 1., 256, 768);
        let reference = common::reference::reference_coverage(&curves, 0., 0., 1., 256, 768);
        let vertices: Vec<_> = curves
            .as_chunks::<8>()
            .0
            .iter()
            .map(|r| [r[0], r[1]])
            .collect();
        method_count += centre.len();
        for y in 0..768 {
            for x in 0..256 {
                let index = y * 256 + x;
                let observed = actual[index * 4] as f32 / 255.;
                let difference = (observed - centre[index]).abs();
                method_max = method_max.max(difference);
                assert!(
                    difference <= READBACK_ERROR,
                    "transpose {transpose}, pixel {x},{y}: GPU {observed}, CPU {}",
                    centre[index]
                );
                let on_tangent = [20, 140].contains(&if transpose { x } else { y });
                let away = vertices.iter().all(|v| {
                    let dx = (x as f32 - v[0]).max(v[0] - (x + 1) as f32).max(0.);
                    let dy = (y as f32 - v[1]).max(v[1] - (y + 1) as f32).max(0.);
                    dx.hypot(dy) > 1.
                });
                if on_tangent && away {
                    count += 1;
                    let error = (observed - reference[index]).abs();
                    box_max = box_max.max(error);
                    assert!(
                        error <= C5_NO_STREAK_ERROR,
                        "transpose {transpose}, tangent pixel {x},{y}: {error}"
                    );
                }
            }
        }
    }
    eprintln!("non-dyadic tangents: {method_count} method pixels, method worst {method_max:.8}; {count} vertex-free tangent pixels, box worst {box_max:.8}");
}

// Measured worst: 17/255 = 0.06666667 for 1/256 px steps (11% headroom).
const CONTINUITY_STEP_ERROR: f32 = 0.074;

#[test]
fn non_dyadic_extrema_move_continuously_across_pixel_centres() {
    let mut worst = 0_f32;
    let mut detail = String::new();
    for transpose in [false, true] {
        for axis in 0..2 {
            let mut previous: Option<Vec<u8>> = None;
            for step in -4..=4 {
                let shift = step as f32 / 256.;
                let mut curves = caps(transpose);
                for record in curves.as_chunks_mut::<8>().0 {
                    for point in 0..3 {
                        record[point * 2 + axis] += shift;
                    }
                }
                let Some((actual, _)) = pixels(&curves, [0., 0.], 1., true) else {
                    return;
                };
                let centre = CentreModel::new(&curves).image([0., 0.], 1., 256, 768);
                for (index, &expected) in centre.iter().enumerate() {
                    let observed = actual[index * 4] as f32 / 255.;
                    assert!((observed - expected).abs() <= READBACK_ERROR,
                        "transpose {transpose}, axis {axis}, step {step}, pixel {},{}: GPU {observed}, CPU {expected}", index % 256, index / 256);
                    if let Some(before) = &previous {
                        let difference =
                            actual[index * 4].abs_diff(before[index * 4]) as f32 / 255.;
                        if difference > worst {
                            worst = difference;
                            detail = format!(
                                "transpose {transpose}, axis {axis}, step {step}, pixel {},{}",
                                index % 256,
                                index / 256
                            );
                        }
                    }
                }
                previous = Some(actual);
            }
        }
    }
    eprintln!("continuity: ±1/64 px, 1/256 px steps, worst {worst:.8}; {detail}");
    assert!(
        worst <= CONTINUITY_STEP_ERROR,
        "continuity jump {worst:.8}: {detail}"
    );
}

const GRAZING_Q: [f32; 3] = [
    1000. - 63. / 16384.,
    -1000. - 32. / 16384.,
    1000. + 127. / 16384.,
];

#[test]
fn glyph_scale_discriminant_is_positive_despite_old_f32_cancellation() {
    let [x, y, z] = GRAZING_Q;
    let a = x - 2. * y + z;
    let b = y - x;
    let true_discriminant = f64::from(y) * f64::from(y) - f64::from(x) * f64::from(z);
    let old = b * b - a * x;
    let stable = y * y - x * z;
    assert!(true_discriminant > 0.);
    assert!(old < 0.);
    assert!(stable > 0.);
    eprintln!(
        "grazing discriminant: true {true_discriminant:.12}, old f32 {old}, stable f32 {stable}"
    );
}

#[test]
fn glyph_scale_grazing_ray_keeps_both_gpu_crossings() {
    let Some((device, queue, _config)) = common::gpu() else {
        return;
    };
    let atlas = crate::atlas::Atlas::new(&device);
    let [x, y, z] = GRAZING_Q;
    let body = format!(
        r#"
        let roots = ray_roots(vec3({x:?}, {y:?}, {z:?}), false);
        return vec4(abs(roots.winding), min(abs(roots.t.x - roots.t.y) * 1024., 1.), 1.);
    "#
    );
    let actual = probe::probe_pixels(&device, &queue, atlas.view(), &analytic::source(&body));
    assert!(
        actual
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[0] == 255 && p[1] == 255 && p[2] > 0),
        "a positive discriminant must retain both distinct crossings"
    );
}

#[test]
fn differently_rounded_non_dyadic_double_roots_match_method_and_box_filter() {
    let mut method_max = 0_f32;
    let mut box_max = 0_f32;
    let mut count = 0;
    for m in [4017_f32, 4021., 4025.] {
        let n = 2048.;
        let q = [m * m / 65536., -m * n / 65536., n * n / 65536.];
        // These exact tangents have differently rounded parameter formulas.
        // Validate the fixture rather than relying on a particular decimal.
        let a = q[0] - 2. * q[1] + q[2];
        let b = q[1] - q[0];
        assert_eq!(q[1] * q[1] - q[0] * q[2], 0.);
        assert_ne!((q[0] / -b).to_bits(), (-b / a).to_bits());
        let top = -235.5;
        let points = [
            [32.125, top + q[0]],
            [128.125, top + q[1]],
            [224.125, top + q[2]],
        ];
        for transpose in [false, true] {
            let mut curves = Vec::new();
            for contour in [points, [points[2], points[0], points[0]]] {
                for p in contour {
                    curves.extend(if transpose { [p[1], p[0]] } else { p });
                }
                curves.extend([0., 0.]);
            }
            let origin = if transpose { [-256., 0.] } else { [0., -256.] };
            let Some((actual, _)) = pixels(&curves, origin, 1., true) else {
                return;
            };
            let centre = CentreModel::new(&curves).image(origin, 1., 256, 768);
            let reference =
                common::reference::reference_coverage(&curves, origin[0], origin[1], 1., 256, 768);
            for y in 0..768 {
                for x in 0..256 {
                    let index = y * 256 + x;
                    let observed = actual[index * 4] as f32 / 255.;
                    let difference = (observed - centre[index]).abs();
                    method_max = method_max.max(difference);
                    assert!(
                        difference <= READBACK_ERROR,
                        "m {m}, transpose {transpose}, pixel {x},{y}: GPU {observed}, CPU {}",
                        centre[index]
                    );
                    if (if transpose { x } else { y }) == 20 {
                        // Both endpoints are at least 64 units above this ray.
                        count += 1;
                        let error = (observed - reference[index]).abs();
                        box_max = box_max.max(error);
                        assert!(
                            error <= C5_NO_STREAK_ERROR,
                            "m {m}, transpose {transpose}, pixel {x},{y}: box error {error}"
                        );
                    }
                }
            }
        }
    }
    eprintln!("differently rounded tangents: 1179648 method pixels, method worst {method_max:.8}; {count} tangent pixels, box worst {box_max:.8}");
}

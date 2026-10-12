#![cfg(not(target_arch = "wasm32"))]

#[allow(dead_code)]
mod common;
use common::reference::reference_coverage;

fn square(low: [f32; 2], high: [f32; 2], reverse: bool) -> Vec<f32> {
    let mut points = [low, [high[0], low[1]], high, [low[0], high[1]]];
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

#[test]
fn square_has_exact_box_coverage() {
    let curves = square([0., 0.], [2., 2.], false);
    for (origin, expected) in [
        ([0., 0.], 1.),
        ([3., 3.], 0.),
        ([-0.5, 0.], 0.5),
        ([-0.5, -0.5], 0.25),
    ] {
        assert_eq!(
            reference_coverage(&curves, origin[0], origin[1], 1., 1, 1),
            [expected]
        );
    }
    assert_eq!(
        reference_coverage(&curves, -1., -1., 1., 4, 4),
        [0., 0., 0., 0., 0., 1., 1., 0., 0., 1., 1., 0., 0., 0., 0., 0.,]
    );
    assert_eq!(reference_coverage(&curves, 0., 0., 2., 1, 1), [1.]);
    assert_eq!(reference_coverage(&[], 0., 0., 1., 1, 1), [0.]);
}

#[test]
fn overlapping_squares_use_nonzero_winding() {
    let mut curves = square([0., 0.], [3., 3.], false);
    curves.extend(square([1., 1.], [4., 4.], false));
    assert_eq!(reference_coverage(&curves, 1., 1., 1., 2, 2), [1.; 4]);
}

#[test]
fn opposite_winding_creates_a_hole() {
    let mut curves = square([0., 0.], [4., 4.], true);
    curves.extend(square([1., 1.], [3., 3.], false));
    assert_eq!(reference_coverage(&curves, 1., 1., 1., 2, 2), [0.; 4]);
    assert_eq!(reference_coverage(&curves, 0., 0., 1., 1, 1), [1.]);
}

#[test]
fn quadratic_matches_independent_dense_estimate() {
    // The curved edge is y=(x-1)^2, closed by the line y=1.
    let curves = [
        0., 1., 1., -1., 2., 1., 0., 0., 2., 1., 0., 1., 0., 1., 0., 0.,
    ];
    let actual = reference_coverage(&curves, -0.25, -0.25, 2., 5, 3);
    for j in 0..3 {
        for i in 0..5 {
            let mut inside = 0;
            for sy in 0..256 {
                for sx in 0..256 {
                    let x = -0.25 + (i as f64 + (sx as f64 + 0.5) / 256.) / 2.;
                    let y = -0.25 + (j as f64 + (sy as f64 + 0.5) / 256.) / 2.;
                    inside += u32::from((0. ..2.).contains(&x) && y >= (x - 1.).powi(2) && y < 1.);
                }
            }
            let expected = inside as f32 / 65536.;
            assert!(
                (actual[j * 5 + i] - expected).abs() < 0.01,
                "pixel ({i}, {j}): {} vs {expected}",
                actual[j * 5 + i]
            );
        }
    }
}

#[test]
fn ray_through_shared_endpoints_and_tangent_is_half_open() {
    // A diamond puts shared vertices exactly on the sample rows.
    let points = [[0., 0.03125], [1., 1.03125], [2., 0.03125], [1., -0.96875]];
    let curves: Vec<_> = (0..4)
        .flat_map(|i| {
            let a = points[i];
            let b = points[(i + 1) % 4];
            [a[0], a[1], b[0], b[1], b[0], b[1], 0., 0.]
        })
        .collect();
    let actual = reference_coverage(&curves, -0.37, 0., 1., 3, 1);
    for (i, &coverage) in actual.iter().enumerate() {
        let mut inside = 0;
        for sy in 0..16 {
            for sx in 0..16 {
                let x = f64::from(-0.37_f32) + i as f64 + (sx as f64 + 0.5) / 16.;
                let y = (sy as f64 + 0.5) / 16.;
                inside += u32::from((x - 1.).abs() + (y - 0.03125).abs() < 1.);
            }
        }
        assert_eq!(coverage, inside as f32 / 256.);
    }
    let tangent = [
        0., 1.03125, 1., -0.96875, 2., 1.03125, 0., 0., 2., 1.03125, 0., 1.03125, 0., 1.03125, 0.,
        0.,
    ];
    assert_eq!(reference_coverage(&tangent, -1., 0., 1., 1, 1), [0.]);
}

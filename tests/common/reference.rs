struct Crossing {
    points: [f64; 6],
    start: f64,
    end: f64,
    y_start: f64,
    y_end: f64,
}

fn coordinate(points: &[f64; 6], axis: usize, t: f64) -> f64 {
    let u = 1. - t;
    u * u * points[axis] + 2. * u * t * points[axis + 2] + t * t * points[axis + 4]
}

/// Box-filtered nonzero coverage, using a 16 by 16 grid of sample centers.
/// Coordinates are already y-down; scale is pixels per font unit.
pub fn reference_coverage(
    curves: &[f32],
    x0: f32,
    y0: f32,
    scale: f32,
    width: u32,
    height: u32,
) -> Vec<f32> {
    assert!(scale.is_finite() && scale > 0.);
    assert_eq!(curves.len() % 8, 0);
    let mut crossings = Vec::new();
    for record in curves.as_chunks::<8>().0 {
        let points = std::array::from_fn(|i| f64::from(record[i]));
        let denominator = points[1] - 2. * points[3] + points[5];
        let extremum = (points[1] - points[3]) / denominator;
        let split = if extremum > 0. && extremum < 1. {
            extremum
        } else {
            1.
        };
        for (start, end) in [(0., split), (split, 1.)] {
            let y_start = coordinate(&points, 1, start);
            let y_end = coordinate(&points, 1, end);
            if y_start != y_end {
                crossings.push(Crossing {
                    points,
                    start,
                    end,
                    y_start,
                    y_end,
                });
            }
        }
    }
    let mut coverage = Vec::with_capacity(width as usize * height as usize);
    for j in 0..height {
        for i in 0..width {
            let mut inside = 0;
            for sy in 0..16 {
                let y =
                    f64::from(y0) + (f64::from(j) + (f64::from(sy) + 0.5) / 16.) / f64::from(scale);
                for sx in 0..16 {
                    let x = f64::from(x0)
                        + (f64::from(i) + (f64::from(sx) + 0.5) / 16.) / f64::from(scale);
                    let mut winding = 0;
                    for c in &crossings {
                        // Lower y inclusive, upper y exclusive, including at extrema.
                        if y < c.y_start.min(c.y_end) || y >= c.y_start.max(c.y_end) {
                            continue;
                        }
                        let increasing = c.y_end > c.y_start;
                        let (mut lo, mut hi) = (c.start, c.end);
                        // Monotone bisection avoids cancellation in quadratic roots.
                        for _ in 0..48 {
                            let mid = (lo + hi) * 0.5;
                            if (coordinate(&c.points, 1, mid) < y) == increasing {
                                lo = mid;
                            } else {
                                hi = mid;
                            }
                        }
                        if coordinate(&c.points, 0, (lo + hi) * 0.5) > x {
                            winding += if increasing { 1 } else { -1 };
                        }
                    }
                    inside += u32::from(winding != 0);
                }
            }
            coverage.push(inside as f32 / 256.);
        }
    }
    coverage
}

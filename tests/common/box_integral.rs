// Independent f64 box integral. Split at every x/y clipping boundary, classify
// each open parameter interval by its midpoint, and integrate x dy exactly.
// The shader instead clips monotone y pieces and integrates positive x parts.
struct Curve {
    points: [[f64; 2]; 3],
    bounds: [[f64; 2]; 2],
}

impl Curve {
    fn at(&self, t: f64) -> [f64; 2] {
        let u = 1. - t;
        std::array::from_fn(|axis| {
            u * u * self.points[0][axis]
                + 2. * u * t * self.points[1][axis]
                + t * t * self.points[2][axis]
        })
    }

    fn cuts(&self, axis: usize, boundary: f64, cuts: &mut [f64; 10], count: &mut usize) {
        let [p0, p1, p2] = self.points.map(|p| p[axis] - boundary);
        if p0.min(p1).min(p2) >= 0. || p0.max(p1).max(p2) <= 0. {
            return;
        }
        let a = p0 - 2. * p1 + p2;
        let b = p1 - p0;
        let mut add = |t| {
            if t > 0. && t < 1. {
                cuts[*count] = t;
                *count += 1;
            }
        };
        if a == 0. {
            if b != 0. {
                add(-p0 / (2. * b));
            }
        } else {
            let discriminant = p1 * p1 - p0 * p2;
            if discriminant < 0. {
                return;
            }
            let d = discriminant.sqrt();
            // Compute one non-cancelling numerator and the other root by Vieta.
            let q = -b - d.copysign(b);
            if q != 0. {
                add(q / a);
                add(p0 / q);
            } else {
                add(-b / a);
            }
        }
    }
}

pub struct BoxIntegralModel {
    curves: Vec<Curve>,
}

impl BoxIntegralModel {
    pub fn new(records: &[f32]) -> Self {
        let curves = records
            .as_chunks::<8>()
            .0
            .iter()
            .map(|r| {
                let mut points = [
                    [f64::from(r[0]), f64::from(r[1])],
                    [f64::from(r[2]), f64::from(r[3])],
                    [f64::from(r[4]), f64::from(r[5])],
                ];
                if points[1] == points[2] {
                    points[1] =
                        std::array::from_fn(|axis| (points[0][axis] + points[2][axis]) * 0.5);
                }
                let bounds = [
                    std::array::from_fn(|axis| {
                        points[0][axis].min(points[1][axis]).min(points[2][axis])
                    }),
                    std::array::from_fn(|axis| {
                        points[0][axis].max(points[1][axis]).max(points[2][axis])
                    }),
                ];
                Curve { points, bounds }
            })
            .collect();
        Self { curves }
    }

    /// Bound the density range of the signed winding field, hence the slope
    /// of its unit-box convolution. At any scanline winding is between minus
    /// the decreasing-crossing count and the increasing-crossing count.
    pub fn slope_bound(&self) -> f32 {
        let mut limit = usize::MAX;
        for axis in 0..2 {
            let mut events = Vec::new();
            for curve in &self.curves {
                let [a, c, b] = curve.points.map(|p| p[axis]);
                let split = ((a - c) / (a - 2. * c + b)).clamp(0., 1.);
                let split = if split.is_nan() { 1. } else { split };
                for (start, end) in [(0., split), (split, 1.)] {
                    let lo = curve.at(start)[axis];
                    let hi = curve.at(end)[axis];
                    if lo == hi {
                        continue;
                    }
                    let direction = usize::from(hi > lo);
                    events.push((lo.min(hi), direction, 1_i32));
                    events.push((lo.max(hi), direction, -1_i32));
                }
            }
            events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
            let mut counts = [0_i32; 2];
            let mut maxima = [0_i32; 2];
            for (_, direction, delta) in events {
                counts[direction] += delta;
                maxima[direction] = maxima[direction].max(counts[direction]);
            }
            limit = limit.min((maxima[0] + maxima[1]) as usize);
        }
        limit as f32
    }

    pub fn sample(&self, point: [f32; 2], scale: f32) -> f32 {
        let point = point.map(f64::from);
        let scale = f64::from(scale);
        let low = point.map(|p| p - 0.5 / scale);
        let high = point.map(|p| p + 0.5 / scale);
        let mut sum = 0.;
        for curve in &self.curves {
            if curve.bounds[1][0] <= low[0]
                || curve.bounds[1][1] <= low[1]
                || curve.bounds[0][1] >= high[1]
            {
                continue;
            }
            if curve.bounds[0][0] >= high[0] {
                sum += ((curve.points[2][1] - point[1]) * scale + 0.5).clamp(0., 1.)
                    - ((curve.points[0][1] - point[1]) * scale + 0.5).clamp(0., 1.);
                continue;
            }
            let mut cuts = [0.; 10];
            cuts[1] = 1.;
            let mut count = 2;
            for axis in 0..2 {
                curve.cuts(axis, low[axis], &mut cuts, &mut count);
                curve.cuts(axis, high[axis], &mut cuts, &mut count);
            }
            cuts[..count].sort_by(f64::total_cmp);
            for interval in cuts[..count].windows(2) {
                let [start, end] = [interval[0], interval[1]];
                let middle = curve.at((start + end) * 0.5);
                if middle[1] <= low[1] || middle[1] >= high[1] || middle[0] <= low[0] {
                    continue;
                }
                let a = curve.at(start);
                let b = curve.at(end);
                if middle[0] >= high[0] {
                    sum += (b[1] - a[1]) * scale;
                } else {
                    let c: [f64; 2] = std::array::from_fn(|axis| {
                        a[axis]
                            + (end - start)
                                * (curve.points[1][axis] - curve.points[0][axis]
                                    + start
                                        * (curve.points[0][axis] - 2. * curve.points[1][axis]
                                            + curve.points[2][axis]))
                    });
                    let x = [a, c, b].map(|p| (p[0] - point[0]) * scale + 0.5);
                    let y = [a, c, b].map(|p| (p[1] - point[1]) * scale);
                    sum += (x[0] * (-3. * y[0] + 2. * y[1] + y[2])
                        + 2. * x[1] * (y[2] - y[0])
                        + x[2] * (-y[0] - 2. * y[1] + 3. * y[2]))
                        / 6.;
                }
            }
        }
        sum.abs().clamp(0., 1.) as f32
    }

    /// Origins are pixel corners in font units; arithmetic matches the probe.
    pub fn image(&self, origin: [f32; 2], scale: f32, width: u32, height: u32) -> Vec<f32> {
        (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    self.sample(
                        [
                            origin[0] + (x as f32 + 0.5) / scale,
                            origin[1] + (y as f32 + 0.5) / scale,
                        ],
                        scale,
                    )
                })
            })
            .collect()
    }
}

/// Rounded endpoints and interior extrema, in font coordinates.
pub fn critical_points(records: &[f32]) -> Vec<[f32; 2]> {
    let mut points = Vec::new();
    for c in records.as_chunks::<8>().0 {
        points.extend([[c[0], c[1]], [c[4], c[5]]]);
        // Lines use control=end; their geometric trace has no interior extremum.
        if c[2..4] == c[4..6] {
            continue;
        }
        for axis in 0..2 {
            let [a, control, b] = [c[axis], c[axis + 2], c[axis + 4]].map(f64::from);
            let t = (a - control) / (a - 2. * control + b);
            if t > 0. && t < 1. {
                let u = 1. - t;
                points.push(std::array::from_fn(|j| {
                    (u * u * f64::from(c[j])
                        + 2. * u * t * f64::from(c[j + 2])
                        + t * t * f64::from(c[j + 4])) as f32
                }));
            }
        }
    }
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    points
}

// Independent unbanded model. Curves are split into monotone pieces and roots
// are found by f64 bisection, rather than the shader's sign table and formula.
struct Curve {
    points: [f64; 6],
    split: [f64; 2],
}

impl Curve {
    fn at(&self, axis: usize, t: f64) -> f64 {
        let u = 1. - t;
        u * u * self.points[axis]
            + 2. * u * t * self.points[axis + 2]
            + t * t * self.points[axis + 4]
    }
}

pub struct CentreModel {
    curves: Vec<Curve>,
}

impl CentreModel {
    pub fn new(records: &[f32]) -> Self {
        let curves = records
            .as_chunks::<8>()
            .0
            .iter()
            .map(|r| {
                let points = std::array::from_fn(|i| f64::from(r[i]));
                let split = std::array::from_fn(|axis| {
                    let denominator = points[axis] - 2. * points[axis + 2] + points[axis + 4];
                    let t = (points[axis] - points[axis + 2]) / denominator;
                    if t > 0. && t < 1. {
                        t
                    } else {
                        1.
                    }
                });
                Curve { points, split }
            })
            .collect();
        Self { curves }
    }

    fn intersections(&self, axis: usize, origin: f32) -> Vec<(f64, f64, bool)> {
        let other = 1 - axis;
        let origin = f64::from(origin);
        let mut hits = Vec::new();
        for curve in &self.curves {
            let split = curve.split[other];
            if split < 1. && origin == curve.at(other, split) {
                // An interior extremum touches the ray twice. The signs cancel,
                // and this zero-width intersection has no weight.
                let position = curve.at(axis, split);
                hits.extend([(position, -1., false), (position, 1., false)]);
                continue;
            }
            for (start, end) in [(0., split), (split, 1.)] {
                let q0 = curve.at(other, start);
                let q1 = curve.at(other, end);
                // Positive -> nonpositive transitions: lower inclusive, upper
                // exclusive. Shared endpoints belong to exactly one piece.
                if origin < q0.min(q1) || origin >= q0.max(q1) {
                    continue;
                }
                let increasing = q1 > q0;
                let t = if origin == q0 {
                    start
                } else if origin == q1 {
                    end
                } else {
                    let (mut lo, mut hi) = (start, end);
                    for _ in 0..60 {
                        let mid = (lo + hi) * 0.5;
                        if (curve.at(other, mid) < origin) == increasing {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    (lo + hi) * 0.5
                };
                hits.push((curve.at(axis, t), if increasing { 1. } else { -1. }, true));
            }
        }
        hits
    }

    fn ray(hits: &[(f64, f64, bool)], origin: f32, scale: f32) -> (f64, f64) {
        let mut sum = 0_f64;
        let mut nearest = f64::INFINITY;
        for &(position, winding, transverse) in hits {
            let distance = (position - f64::from(origin)) * f64::from(scale);
            sum += (0.5 + distance).clamp(0., 1.) * winding;
            if transverse {
                nearest = nearest.min(distance.abs());
            }
        }
        (sum.abs().clamp(0., 1.), (1. - 2. * nearest).clamp(0., 1.))
    }

    /// Origins are pixel corners in font units; arithmetic matches the probe's
    /// coordinate mapping, without using its root calculation or band lists.
    pub fn image(&self, origin: [f32; 2], scale: f32, width: u32, height: u32) -> Vec<f32> {
        let xs: Vec<_> = (0..width)
            .map(|x| origin[0] + (x as f32 + 0.5) / scale)
            .collect();
        let ys: Vec<_> = (0..height)
            .map(|y| origin[1] + (y as f32 + 0.5) / scale)
            .collect();
        let horizontal: Vec<_> = ys.iter().map(|&y| self.intersections(0, y)).collect();
        let vertical: Vec<_> = xs.iter().map(|&x| self.intersections(1, x)).collect();
        let mut coverage = Vec::with_capacity(width as usize * height as usize);
        for (j, &y) in ys.iter().enumerate() {
            for (i, &x) in xs.iter().enumerate() {
                let (h, wh) = Self::ray(&horizontal[j], x, scale);
                let (v, wv) = Self::ray(&vertical[i], y, scale);
                let value = if wh + wv > 0. {
                    (h * wh + v * wv) / (wh + wv)
                } else {
                    (h + v) * 0.5
                };
                coverage.push(value as f32);
            }
        }
        coverage
    }
}

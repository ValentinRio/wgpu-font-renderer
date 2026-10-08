/// Column-major transform from pixel bounds to clip coordinates (x/y in -1..=1).
/// Pass top=0 and bottom=height for a top-left origin with y downward.
/// Equal horizontal or vertical bounds produce nonfinite values; no validation
/// is performed. Depth uses the GL-style -1..=1 clip convention, with z' = -z.
pub fn orthographic_projection_matrix(left: f32, right: f32, bottom: f32, top: f32) -> [f32; 16] {
    let near = -1.;
    let far = 1.;

    let tx = - (right + left) / (right - left);
    let ty = - (top + bottom) / (top - bottom);
    let tz = - (far + near) / (far - near);

    [2. / (right - left), 0., 0., 0.,
    0., 2. / (top - bottom), 0., 0.,
    0., 0., -2. / (far - near), 0.,
    tx, ty, tz, 1.]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_maps_offset_viewport_corners_and_center() {
        let m = orthographic_projection_matrix(10., 810., 620., 20.);
        for (point, expected) in [
            ([10., 20.], [-1., 1.]),
            ([810., 620.], [1., -1.]),
            ([410., 320.], [0., 0.]),
        ] {
            let transformed = [
                m[0] * point[0] + m[4] * point[1] + m[12],
                m[1] * point[0] + m[5] * point[1] + m[13],
            ];
            assert!(
                transformed
                    .iter()
                    .zip(expected)
                    .all(|(actual, want)| (actual - want).abs() < 1e-6),
                "{point:?} -> {transformed:?}"
            );
        }
        assert_eq!([m[10], m[15]], [-1., 1.]);
    }
}

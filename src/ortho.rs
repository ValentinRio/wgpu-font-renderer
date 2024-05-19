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
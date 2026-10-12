pub const BAND_GATE: &str = "if input.bands.z >= 0. && input.atlas_size > 0 {";

pub fn source(body: &str) -> String {
    format!("{}\n{}\n@fragment fn probe_fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{ {body} }}",
        include_str!("../../src/shader.wgsl"),
        r#"@vertex fn probe_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
            let p = array(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.));
            return vec4(p[index], 0., 1.);
        }"#)
}

pub fn full(source: &str) -> String {
    let result = source.replace(BAND_GATE, "if false {");
    assert_ne!(result, source);
    result
}

pub fn band_debug(source: &str) -> String {
    let result = source.replace(BAND_GATE, &format!("{BAND_GATE} return vec3(1., 0., 0.);"));
    assert_ne!(result, source);
    result
}

pub fn glyph_input(flat: ([u32; 2], u32, usize), bands: ([u32; 2], usize), scale: f32) -> String {
    let ([x, y], count, layer) = flat;
    let ([bx, by], band_layer) = bands;
    format!(
        r#"
    var input: VertexOutput;
    input.pixel_scale = {scale:?};
    input.atlas_pos = vec2({x}., {y}.);
    input.atlas_size = {count};
    input.layer = {layer}.;
    input.bands = vec3({bx}., {by}., {band_layer}.);
    input.color = vec4(0., 0., 0., 1.);
    "#
    )
}

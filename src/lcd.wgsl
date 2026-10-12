// Appended to the shared analytic shader when dual-source blending is available.
override LCD_BGR: bool = false;

fn lcd_coverage(input: VertexOutput) -> vec3<f32> {
    let coverage = analytic_coverage(input, vec3(-1./3., 0., 1./3.));
    return select(coverage, coverage.bgr, LCD_BGR);
}

struct LcdOutput {
    @location(0) @blend_src(0) color: vec4<f32>,
    @location(0) @blend_src(1) coverage: vec4<f32>,
}

@fragment
fn fs_lcd(input: VertexOutput) -> LcdOutput {
    let coverage = lcd_coverage(input) * input.color.a;
    var output: LcdOutput;
    // Alpha uses G coverage; LCD is intended for opaque backgrounds.
    output.color = vec4(input.color.rgb, coverage.g);
    output.coverage = vec4(coverage, coverage.g);
    return output;
}

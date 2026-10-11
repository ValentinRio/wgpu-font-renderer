// Appended after the shared grayscale shader in a feature-enabled module.
override LCD_BGR: bool = false;

struct LcdNearest {
    distance: vec3<f32>,
    side: vec3<f32>,
}

fn lcd_side(uv: vec2<f32>, offset: f32, start: i32, count: i32, layer: i32) -> vec3<f32> {
    var side = vec3(0.);
    for (var i = 0; i < count; i += 8) {
        let ac = atlas_texel(start + i, layer);
        let bi = atlas_texel(start + i + 4, layer);
        if ((uv.y > ac.y && uv.y < bi.y) || (uv.y > bi.y && uv.y < ac.y)) {
            side += vec3(
                sign_bezier(ac.xy, ac.zw, bi.xy, uv - vec2(offset, 0.)),
                sign_bezier(ac.xy, ac.zw, bi.xy, uv),
                sign_bezier(ac.xy, ac.zw, bi.xy, uv + vec2(offset, 0.)));
        }
    }
    return side;
}

fn lcd_curves(uv: vec2<f32>, offset: f32, start: i32, count: i32,
    winding_start: i32, winding_count: i32, layer: i32, window: f32, sorted: bool) -> LcdNearest {
    var result: LcdNearest;
    result.side = lcd_side(uv, offset, winding_start, winding_count, layer);
    result.distance = vec3(1e20);
    for (var i = 0; i < count; i += 8) {
        let bi = atlas_texel(start + i + 4, layer);
        // Every channel may have a different nearest curve. Keep the largest
        // channel reach, measured from G, when rejecting hulls or stopping.
        let reach = max(max(min(result.distance.r, window) + offset,
                            min(result.distance.g, window)),
                            min(result.distance.b, window) + offset) + 0.05;
        if sorted && bi.w - uv.x > reach { break; }
        let ac = atlas_texel(start + i, layer);
        if sorted && hull_distance(uv, ac.xy, ac.zw, bi.xy) > reach { continue; }
        result.distance = min(result.distance, vec3(
            sd_bezier(ac.xy, ac.zw, bi.xy, uv - vec2(offset, 0.)),
            sd_bezier(ac.xy, ac.zw, bi.xy, uv),
            sd_bezier(ac.xy, ac.zw, bi.xy, uv + vec2(offset, 0.))));
    }
    return result;
}

fn lcd_coverage(input: VertexOutput) -> vec3<f32> {
    let font_size = input.font_size;
    let offset = input.units_per_em / (3. * font_size);
    var uv = input.uv;
    uv.x = remap(uv.x, 0., 1., 0., input.size.x * input.units_per_em / font_size);
    uv.x += input.left_side_bearing;
    uv.y = remap(uv.y, 0., 1., 0., input.size.y * input.units_per_em / font_size);
    // Band membership is y-only; horizontal sample offsets do not widen it.
    let window = .5 + edge_width(font_size, input.units_per_em);
    var nearest: LcdNearest;
    if window <= BAND_MARGIN && input.bands.z >= 0. && input.atlas_size > 0 {
        let header = i32(input.bands.y) * atlas_width() + i32(input.bands.x);
        let layer = i32(input.bands.z);
        let header_data = atlas_texel(header, layer);
        let band = clamp(i32(floor((uv.y - header_data.x) / header_data.y)), 0, i32(header_data.z) - 1);
        let descriptor = header + 8 + band * 8;
        let distance_data = atlas_texel(descriptor, layer);
        let winding_data = atlas_texel(descriptor + 4, layer);
        nearest = lcd_curves(uv, offset, header + i32(distance_data.x), i32(distance_data.y),
            header + i32(winding_data.x), i32(winding_data.y), layer, window, true);
    } else {
        let start = i32(input.atlas_pos.y) * atlas_width() + i32(input.atlas_pos.x);
        nearest = lcd_curves(uv, offset, start, input.atlas_size, start, input.atlas_size,
            i32(input.layer), window, false);
    }
    let vgrad = abs(dpdy(nearest.distance.g));
    let alpha = sdf_triplet_alpha(nearest.distance, .5, .6, vgrad, edge_width(font_size, input.units_per_em));
    let coverage = select(vec3(1.) - alpha, alpha, nearest.side == vec3(-2.));
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

struct Params {
    screen_resolution: vec2<f32>,
    _pad: vec2<f32>,
    transform: mat4x4<f32>,
}

struct VertexInput {
    @location(0) v_pos: vec2<f32>,
    @location(1) pos: vec2<f32>,
    @location(2) left_side_bearing: f32,
    @location(3) font_size: f32,
    @location(4) size: vec2<f32>,
    @location(5) atlas_pos: vec2<f32>,
    @location(6) atlas_size: u32,
    @location(7) units_per_em: f32,
    @location(8) layer: i32,
    @location(9) color: vec4<f32>,
    @location(10) bands: vec3<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) pos: vec2<f32>,
    @location(2) left_side_bearing: f32,
    @location(3) font_size: f32,
    @location(4) size: vec2<f32>,
    @location(5) @interpolate(flat) atlas_pos: vec2<f32>,
    @location(6) @interpolate(flat) atlas_size: i32,
    @location(7) units_per_em: f32,
    @location(8) @interpolate(flat) layer: f32,
    @location(9) color: vec4<f32>,
    @location(10) @interpolate(flat) bands: vec3<f32>,
    @location(11) @interpolate(flat) pixel_scale: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var atlas_sampler: sampler;
@group(1) @binding(0) var atlas_texture: texture_2d_array<f32>;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    let local = input.v_pos * (input.size + vec2(1.)) - vec2(.5);
    output.pixel_scale = input.font_size / input.units_per_em;
    output.uv = local / output.pixel_scale + vec2(input.left_side_bearing, 0.);
    output.position = params.transform * vec4(input.pos + local, 0., 1.);
    output.pos = input.pos;
    output.font_size = input.font_size;
    output.size = input.size;
    output.layer = f32(input.layer);
    output.atlas_size = i32(input.atlas_size);
    output.atlas_pos = input.atlas_pos;
    output.left_side_bearing = input.left_side_bearing;
    output.units_per_em = input.units_per_em;
    output.color = input.color;
    output.bands = input.bands;

    return output;
}

// Atlas offsets count floats. Width is a power of two, including in probes.
fn atlas_width() -> i32 {
    return i32(textureDimensions(atlas_texture).x) << 2u;
}

fn atlas_texel(offset: i32, layer: i32) -> vec4<f32> {
    let texel = u32(offset) >> 2u;
    let width = textureDimensions(atlas_texture).x;
    let shift = firstLeadingBit(width);
    return textureLoad(atlas_texture,
        vec2<i32>(i32(texel & (width - 1u)), i32(texel >> shift)), layer, 0);
}

struct Roots {
    t: vec2<f32>,
    winding: vec2<f32>,
}

// Classify signs before solving: endpoint rounding cannot change eligibility.
// Zero belongs to the nonpositive class. Equal endpoint signs and an opposite
// control sign select both roots, whose contributions cancel at a tangent.
fn ray_roots(q: vec3<f32>, line: bool) -> Roots {
    var result: Roots;
    let positive = q > vec3(0.);
    let both = positive.x == positive.z && positive.y != positive.x;
    result.winding = vec2(
        select(0., -1., both || (positive.x && !positive.z)),
        select(0., 1., both || (!positive.x && positive.z)));
    if all(result.winding == vec2(0.)) { return result; }
    let a = q.x - 2. * q.y + q.z;
    let b = q.y - q.x;
    if line || abs(a) < 1e-5 * max(max(abs(q.x), abs(q.y)), max(abs(q.z), 1.)) {
        let delta = select(2. * b, q.z - q.x, line);
        if delta != 0. {
            result.t = vec2(-q.x / delta);
        } else {
            result.winding = vec2(0.);
        }
        return result;
    }
    let discriminant = b * b - a * q.x;
    if discriminant < 0. {
        // No real intersection: cancelling imaginary roots must not influence
        // the nearest-root weight of this ray.
        result.winding = vec2(0.);
        return result;
    }
    let d = sqrt(discriminant);
    // Rationalize the cancelling numerator while preserving derivative order:
    // the first root has negative derivative, the second positive derivative.
    var t = vec2((-b - d) / a, (-b + d) / a);
    if b > 0. && b + d != 0. { t.y = -q.x / (b + d); }
    if b < 0. && -b + d != 0. { t.x = q.x / (-b + d); }
    result.t = t;
    return result;
}

struct RayCoverage {
    sum: vec3<f32>,
    nearest: vec3<f32>,
}

fn ray_coverage(p: vec2<f32>, scale: f32, axis: u32, offsets: vec3<f32>,
    start: i32, count: i32, layer: i32, sorted: bool) -> RayCoverage {
    var result: RayCoverage;
    result.nearest = vec3(1e20);
    let other = 1u - axis;
    // Fixed fractional accumulation makes band sorting independent of addition
    // order. Carry each root into a separate integer winding so large outlines
    // cannot overflow the fractional accumulator (1/65536 pixel precision).
    var winding = vec3<i32>(0);
    var fraction = vec3<i32>(0);
    for (var i = 0; i < count; i += 8) {
        let end = atlas_texel(start + i + 4, layer);
        if sorted && (end.w - p[axis]) * scale < -.5 + min(min(offsets.x, offsets.y), offsets.z) {
            break;
        }
        let ac = atlas_texel(start + i, layer);
        let line = all(ac.zw == end.xy);
        let roots = ray_roots(vec3(ac[other], ac[other + 2u], end[other]) - p[other], line);
        // A double root contributes zero net coverage. It must not weight an
        // empty ray against the transverse ray at an interior extremum.
        let tangent = all(roots.winding != vec2(0.)) && roots.t.x == roots.t.y;
        for (var root = 0u; root < 2u; root += 1u) {
            if roots.winding[root] == 0. { continue; }
            let t = roots.t[root];
            let u = 1. - t;
            var position = u * u * ac[axis] + 2. * u * t * ac[axis + 2u] + t * t * end[axis];
            if line { position = mix(ac[axis], end[axis], t); }
            let distance = vec3((position - p[axis]) * scale) - offsets;
            fraction += vec3<i32>(round(clamp(vec3(.5) + distance, vec3(0.), vec3(1.)) * 65536.)) * i32(roots.winding[root]);
            winding += fraction >> vec3<u32>(16u);
            fraction &= vec3<i32>(65535);
            if !tangent { result.nearest = min(result.nearest, abs(distance)); }
        }
    }
    result.sum = vec3<f32>(winding) + vec3<f32>(fraction) / 65536.;
    return result;
}

const WEIGHTED_COVERAGE: bool = true;

fn combine_rays(h: RayCoverage, v: RayCoverage) -> vec3<f32> {
    let x = clamp(abs(h.sum), vec3(0.), vec3(1.));
    let y = clamp(abs(v.sum), vec3(0.), vec3(1.));
    if WEIGHTED_COVERAGE {
        let wx = clamp(vec3(1.) - 2. * h.nearest, vec3(0.), vec3(1.));
        let wy = clamp(vec3(1.) - 2. * v.nearest, vec3(0.), vec3(1.));
        let denominator = wx + wy;
        return select((x + y) * .5, (x * wx + y * wy) / max(denominator, vec3(1e-20)), denominator > vec3(0.));
    }
    return (x + y) * .5;
}

fn analytic_coverage(input: VertexOutput, offsets: vec3<f32>) -> vec3<f32> {
    let p = input.uv;
    let scale = input.pixel_scale;
    var h: RayCoverage;
    var v: RayCoverage;
    if input.bands.z >= 0. && input.atlas_size > 0 {
        let header = i32(input.bands.y) * atlas_width() + i32(input.bands.x);
        let layer = i32(input.bands.z);
        let bounds = atlas_texel(header, layer);
        let counts = atlas_texel(header + 4, layer);
        let bands = clamp(vec2<i32>(floor((p - bounds.xy) / bounds.zw)), vec2(0), vec2<i32>(counts.yx) - vec2(1));
        let hd = atlas_texel(header + 8 + bands.y * 8, layer);
        let vd = atlas_texel(header + 8 + (i32(counts.x) + bands.x) * 8, layer);
        h = ray_coverage(p, scale, 0u, offsets, header + i32(hd.x), i32(hd.y), layer, true);
        v = ray_coverage(p, scale, 1u, vec3(0.), header + i32(vd.x), i32(vd.y), layer, true);
    } else {
        let start = i32(input.atlas_pos.y) * atlas_width() + i32(input.atlas_pos.x);
        let layer = i32(input.layer);
        h = ray_coverage(p, scale, 0u, offsets, start, input.atlas_size, layer, false);
        v = ray_coverage(p, scale, 1u, vec3(0.), start, input.atlas_size, layer, false);
    }
    return combine_rays(h, v);
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4(input.color.rgb, analytic_coverage(input, vec3(0.)).y);
}

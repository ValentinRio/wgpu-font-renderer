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
    return coverage_vertex(input, vec2(.5));
}

// The raster fringe must contain the filter support. Changing it preserves
// the mapping between screen positions and font coordinates (and ink placement).
fn coverage_vertex(input: VertexInput, fringe: vec2<f32>) -> VertexOutput {
    var output: VertexOutput;

    let local = input.v_pos * (input.size + 2. * fringe) - fringe;
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
struct AtlasAddress {
    float_width: i32,
    mask: u32,
    shift: u32,
}

fn atlas_address() -> AtlasAddress {
    let width = textureDimensions(atlas_texture).x;
    return AtlasAddress(i32(width) << 2u, width - 1u, firstLeadingBit(width));
}

fn atlas_texel(offset: i32, layer: i32, address: AtlasAddress) -> vec4<f32> {
    let texel = u32(offset) >> 2u;
    return textureLoad(atlas_texture,
        vec2<i32>(i32(texel & address.mask), i32(texel >> address.shift)), layer, 0);
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
    if line || a == 0. {
        let delta = select(2. * b, q.z - q.x, line);
        if delta != 0. {
            result.t = vec2(-q.x / delta);
        } else {
            result.winding = vec2(0.);
        }
        return result;
    }
    let discriminant = q.y * q.y - q.x * q.z;
    if discriminant < 0. {
        // No real intersection: cancelling imaginary roots must not influence
        // the positive clipping intervals.
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

// Derivative-ordered roots for a clipping boundary. Out-of-range roots are
// harmless: monotone intervals clamp them to their own parameter range.
fn boundary_roots(q: vec3<f32>) -> vec2<f32> {
    let a = q.x - 2. * q.y + q.z;
    let b = q.y - q.x;
    if a == 0. {
        return vec2(select(0., -q.x / (2. * b), b != 0.));
    }
    let d = sqrt(max(0., q.y * q.y - q.x * q.z));
    var t = vec2((-b - d) / a, (-b + d) / a);
    if b > 0. && b + d != 0. { t.y = -q.x / (b + d); }
    if b < 0. && -b + d != 0. { t.x = q.x / (-b + d); }
    return t;
}

fn monotone_root(q: vec3<f32>, increasing: bool) -> f32 {
    if q.x == 0. { return 0.; }
    let b = q.y - q.x;
    let d = sqrt(max(0., q.y * q.y - q.x * q.z));
    if increasing { return clamp(-q.x / (b + d), 0., 1.); }
    return clamp(q.x / (-b + d), 0., 1.);
}

struct Intervals {
    lo: vec2<f32>,
    hi: vec2<f32>,
}

fn at(q: vec3<f32>, t: vec2<f32>) -> vec2<f32> {
    let u = vec2(1.) - t;
    return u * u * q.x + 2. * u * t * q.y + t * t * q.z;
}

// Split into decreasing/increasing pieces, then clip each to the unit box.
// A boundary root entering or leaving a piece makes an interval of zero length.
fn y_intervals(q: vec3<f32>) -> Intervals {
    if all(abs(q) <= vec3(.5)) { return Intervals(vec2(0.), vec2(1., 0.)); }
    let increasing = q.x <= q.y && q.y <= q.z;
    let decreasing = q.x >= q.y && q.y >= q.z;
    if increasing || decreasing {
        var lo = 0.;
        var hi = 1.;
        if increasing {
            if q.x < -.5 { lo = monotone_root(q + vec3(.5), true); }
            if q.z > .5 { hi = monotone_root(q - vec3(.5), true); }
        } else {
            if q.x > .5 { lo = monotone_root(q - vec3(.5), false); }
            if q.z < -.5 { hi = monotone_root(q + vec3(.5), false); }
        }
        return Intervals(vec2(clamp(lo, 0., 1.), 0.), vec2(clamp(hi, 0., 1.), 0.));
    }
    let a = q.x - 2. * q.y + q.z;
    let b = q.y - q.x;
    var lo = vec2(0.);
    var hi = vec2(1.);
    var start = vec2(q.x);
    var end = vec2(q.z);
    if a != 0. {
        let split = clamp(-b / a, 0., 1.);
        let u = 1. - split;
        let extremum = u * u * q.x + 2. * u * split * q.y + split * split * q.z;
        if a > 0. {
            hi.x = split; lo.y = split;
            end.x = extremum; start.y = extremum;
        } else {
            lo.x = split; hi.y = split;
            start.x = extremum; end.y = extremum;
        }
    } else if b >= 0. { hi.x = 0.; end.x = q.x; }
    else { hi.y = 0.; end.y = q.x; }
    var lower = vec2(0.);
    var upper = vec2(0.);
    if min(min(q.x, q.y), q.z) < -.5 { lower = boundary_roots(q + vec3(.5)); }
    if max(max(q.x, q.y), q.z) > .5 { upper = boundary_roots(q - vec3(.5)); }
    let clipped_lo = select(lo, vec2(upper.x, lower.y), vec2<bool>((start.x > .5), (start.y < -.5)));
    let clipped_hi = select(hi, vec2(lower.x, upper.y), vec2<bool>((end.x < -.5), (end.y > .5)));
    let intersects = vec2<bool>((start.x > -.5 && end.x < .5), (end.y > -.5 && start.y < .5));
    return Intervals(clamp(clipped_lo, lo, hi), select(lo, clamp(clipped_hi, lo, hi), intersects));
}

fn positive_intervals(q: vec3<f32>) -> Intervals {
    let roots = ray_roots(q, false);
    if all(roots.winding == vec2(0.)) {
        return Intervals(vec2(0.), vec2(select(0., 1., q.x > 0.), 0.));
    }
    let t = clamp(roots.t, vec2(0.), vec2(1.));
    if roots.winding.x == 0. { return Intervals(vec2(t.y, 0.), vec2(1., 0.)); }
    if roots.winding.y == 0. { return Intervals(vec2(0.), vec2(t.x, 0.)); }
    if q.x - 2. * q.y + q.z >= 0. { return Intervals(vec2(0., t.y), vec2(t.x, 1.)); }
    return Intervals(vec2(t.y, 0.), vec2(t.x, 0.));
}

// Green primitive on a curve already clipped to one pixel in y. Its local
// coefficients avoid subtracting primitives in the original font coordinates.
fn integral(x: vec3<f32>, y: vec3<f32>, lo: vec2<f32>, hi: vec2<f32>) -> f32 {
    let a = vec2(x.x - 2. * x.y + x.z, y.x - 2. * y.y + y.z);
    let b = vec2(x.y - x.x, y.y - y.x);
    let c1 = 2. * x.x * b.y;
    let c2 = x.x * a.y + 2. * b.x * b.y;
    let c3 = (4. * b.x * a.y + 2. * a.x * b.y) / 3.;
    let c4 = .5 * a.x * a.y;
    let upper = hi * (vec2(c1) + hi * (vec2(c2) + hi * (vec2(c3) + hi * c4)));
    let lower = lo * (vec2(c1) + lo * (vec2(c2) + lo * (vec2(c3) + lo * c4)));
    let area = upper - lower;
    return area.x + area.y;
}

fn full_integral(x: vec3<f32>, y: vec3<f32>) -> f32 {
    let dc = y.y - y.x;
    let db = y.z - y.x;
    return (x.x * (2. * dc + db) + 2. * x.y * db + x.z * (3. * db - 2. * dc)) / 6.;
}

fn primitive(x: vec3<f32>, y: vec3<f32>, t: f32) -> f32 {
    let a = vec2(x.x - 2. * x.y + x.z, y.x - 2. * y.y + y.z);
    let b = vec2(x.y - x.x, y.y - y.x);
    let c1 = 2. * x.x * b.y;
    let c2 = x.x * a.y + 2. * b.x * b.y;
    let c3 = (4. * b.x * a.y + 2. * a.x * b.y) / 3.;
    let c4 = .5 * a.x * a.y;
    return t * (c1 + t * (c2 + t * (c3 + t * c4)));
}

fn positive_integral(x: vec3<f32>, y: vec3<f32>) -> f32 {
    if all(x >= vec3(0.)) { return full_integral(x, y); }
    if all(x <= vec3(0.)) { return 0.; }
    let increasing = x.x <= x.y && x.y <= x.z;
    let decreasing = x.x >= x.y && x.y >= x.z;
    if increasing || decreasing {
        // Opposite endpoint signs make this discriminant a sum, avoiding
        // cancellation. Only one boundary root and one primitive are needed.
        let d = sqrt(x.y * x.y - x.x * x.z);
        let b = x.y - x.x;
        if increasing {
            let t = clamp(-x.x / (b + d), 0., 1.);
            return -primitive(x.zyx, y.zyx, 1. - t);
        }
        let t = clamp(x.x / (-b + d), 0., 1.);
        return primitive(x, y, t);
    }
    let intervals = positive_intervals(x);
    return integral(x, y, intervals.lo, intervals.hi);
}

struct ClippedCurve {
    x: vec3<f32>,
    y: vec3<f32>,
}

fn clip_curve(x: vec3<f32>, y: vec3<f32>, lo: f32, hi: f32) -> ClippedCurve {
    let xs = at(x, vec2(lo, hi));
    let ys = at(y, vec2(lo, hi));
    let cx = xs.x + (hi - lo) * (x.y - x.x + lo * (x.x - 2. * x.y + x.z));
    let cy = ys.x + (hi - lo) * (y.y - y.x + lo * (y.x - 2. * y.y + y.z));
    return ClippedCurve(vec3(xs.x, cx, xs.y), vec3(ys.x, cy, ys.y));
}

fn clipped_area(q: ClippedCurve, offset: f32) -> f32 {
    let lower = q.x + vec3(.5 - offset);
    let upper = q.x - vec3(.5 + offset);
    if all(lower <= vec3(0.)) { return 0.; }
    if all(upper >= vec3(0.)) { return q.y.z - q.y.x; }
    if all(lower >= vec3(0.)) && all(upper <= vec3(0.)) { return full_integral(lower, q.y); }
    return positive_integral(lower, q.y) - positive_integral(upper, q.y);
}

fn clipped_rgb(q: ClippedCurve, offsets: vec3<f32>) -> vec3<f32> {
    if offsets.x == offsets.y && offsets.y == offsets.z { return vec3(clipped_area(q, offsets.y)); }
    return vec3(clipped_area(q, offsets.x), clipped_area(q, offsets.y), clipped_area(q, offsets.z));
}

// A line's clipped x ramp is a trapezoid plus a constant rectangle.
// Clip its y parameter first; no quadratic roots or area primitives are needed.
fn line_area(a: vec2<f32>, b: vec2<f32>, offsets: vec3<f32>) -> vec3<f32> {
    let delta = b - a;
    if delta.x == 0. {
        let height = clamp(b.y + .5, 0., 1.) - clamp(a.y + .5, 0., 1.);
        return clamp(vec3(a.x + .5) - offsets, vec3(0.), vec3(1.)) * height;
    }
    if delta.y == 0. { return vec3(0.); }
    let ends = clamp((vec2(-.5, .5) - vec2(a.y)) / delta.y, vec2(0.), vec2(1.));
    let lo = min(ends.x, ends.y);
    let hi = max(ends.x, ends.y);
    let x0 = a.x + lo * delta.x;
    let x1 = a.x + hi * delta.x;
    let f0 = clamp(vec3(x0 + .5) - offsets, vec3(0.), vec3(1.));
    let f1 = clamp(vec3(x1 + .5) - offsets, vec3(0.), vec3(1.));
    let dx = x1 - x0;
    var average = (f0 + f1) * .5;
    if dx != 0. {
        let lower = clamp((offsets - vec3(.5 + x0)) / dx, vec3(0.), vec3(1.));
        let upper = clamp((offsets + vec3(.5 - x0)) / dx, vec3(0.), vec3(1.));
        average = average * abs(upper - lower) + select(vec3(1.) - upper, upper, dx < 0.);
    }
    return average * (delta.y * (hi - lo));
}

// Green's theorem integrates the signed winding over the pixel box:
// integral clamp(x + .5, 0, 1) dy, restricted to -.5 < y < .5.
// Clipping intervals have zero area as roots enter/leave, and adjacent pieces
// telescope at endpoints. No root-presence or nearest-distance weight remains.
// Quantize only the resulting areas so band/list ordering cannot change pixels.
fn box_coverage(p: vec2<f32>, scale: f32, offsets: vec3<f32>, start: i32,
    count: i32, layer: i32, sorted: bool, duplicate_y: f32, window: vec4<f32>, address: AtlasAddress) -> vec3<i32> {
    var sum = vec3<i32>(0);
    let low = window.xy;
    let high = window.zw;
    for (var i = 0; i < count; i += 8) {
        let end = atlas_texel(start + i + 4, layer, address);
        if sorted && end.w < low.x { break; }
        let ac = atlas_texel(start + i, layer, address);
        if ac.y == ac.w && ac.w == end.y { continue; }
        let low_y = min(min(ac.y, ac.w), end.y);
        let high_y = max(max(ac.y, ac.w), end.y);
        if low_y <= duplicate_y || low_y >= high.y || high_y <= low.y { continue; }
        var area: vec3<f32>;
        if min(min(ac.x, ac.z), end.x) >= high.x {
            area = vec3(clamp((end.y - p.y) * scale + .5, 0., 1.) - clamp((ac.y - p.y) * scale + .5, 0., 1.));
        } else if max(max(ac.x, ac.z), end.x) > low.x {
            let x = (vec3(ac.x, ac.z, end.x) - vec3(p.x)) * scale;
            let y = (vec3(ac.y, ac.w, end.y) - vec3(p.y)) * scale;
            if all(ac.zw == end.xy) {
                area = line_area(vec2(x.x, y.x), vec2(x.z, y.z), offsets);
            } else {
                let intervals = y_intervals(y);
                if intervals.hi.x > intervals.lo.x {
                    area += clipped_rgb(clip_curve(x, y, intervals.lo.x, intervals.hi.x), offsets);
                }
                if intervals.hi.y > intervals.lo.y {
                    area += clipped_rgb(clip_curve(x, y, intervals.lo.y, intervals.hi.y), offsets);
                }
            }
        }
        sum += vec3<i32>(round(area * 65536.));
    }
    return sum;
}

fn analytic_coverage(input: VertexOutput, offsets: vec3<f32>) -> vec3<f32> {
    let address = atlas_address();
    let p = input.uv;
    let scale = input.pixel_scale;
    let low = p + vec2(-.5 + min(min(offsets.x, offsets.y), offsets.z), -.5) / scale;
    let high = p + vec2(.5 + max(max(offsets.x, offsets.y), offsets.z), .5) / scale;
    let window = vec4(low, high);
    var sum = vec3<i32>(0);
    if input.bands.z >= 0. && input.atlas_size > 0 {
        let header = i32(input.bands.y) * address.float_width + i32(input.bands.x);
        let layer = i32(input.bands.z);
        let bounds = atlas_texel(header, layer, address);
        let counts = atlas_texel(header + 4, layer, address);
        let first = clamp(i32(floor((low.y - bounds.y) / bounds.w)), 0, i32(counts.x) - 1);
        let last = clamp(i32(floor((high.y - bounds.y) / bounds.w)), 0, i32(counts.x) - 1);
        var previous_high = -1e20;
        for (var band = first; band <= last; band += 1) {
            let descriptor = atlas_texel(header + 8 + band * 8, layer, address);
            // Consecutive band lists overlap. A curve belongs to the first
            // visited list whose high bound reaches its control-hull minimum.
            let duplicate_y = previous_high;
            previous_high = descriptor.z;
            sum += box_coverage(p, scale, offsets, header + i32(descriptor.x), i32(descriptor.y), layer, true, duplicate_y, window, address);
        }
    } else {
        let start = i32(input.atlas_pos.y) * address.float_width + i32(input.atlas_pos.x);
        sum = box_coverage(p, scale, offsets, start, input.atlas_size, i32(input.layer), false, -1e20, window, address);
    }
    return clamp(abs(vec3<f32>(sum) / 65536.), vec3(0.), vec3(1.));
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4(input.color.rgb, analytic_coverage(input, vec3(0.)).y * input.color.a);
}

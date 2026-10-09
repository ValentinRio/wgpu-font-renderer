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
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var atlas_sampler: sampler;
@group(1) @binding(0) var atlas_texture: texture_2d_array<f32>;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    output.uv = vec2<f32>(input.v_pos);
    output.layer = f32(input.layer);

    var transform = mat4x4<f32>(
        vec4<f32>(input.size.x, 0.,           0., 0.),
        vec4<f32>(0.,           input.size.y, 0., 0.),
        vec4<f32>(0.,           0.,           1., 0.),
        vec4<f32>(input.pos,                  0., 1.),
    );

    output.position = params.transform * transform * vec4<f32>(input.v_pos * 1., 0., 1.);
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

fn test_cross(a: vec2<f32>, b: vec2<f32>, p: vec2<f32>) -> f32 {
    return sign((b.y - a.y) * (p.x - a.x) - (b.x - a.x) * (p.y - a.y));
}

fn sign_bezier(A: vec2<f32>, B: vec2<f32>, C: vec2<f32>, p: vec2<f32>) -> f32 {
    let a: vec2<f32> = C - A;
    let b: vec2<f32> = B - A;
    let c: vec2<f32> = p - A;

    let r: f32 = (a.x * b.y - b.x * a.y);

    if abs(r) < 0.001 {
        return test_cross(A, B, p);
    }

    let bary = vec2<f32>(c.x * b.y - b.x * c.y, a.x * c.y - c.x * a.y) / r;
    let d = vec2<f32>(bary.y * .5, 0.) + 1. - bary.x - bary.y;
    return mix(
        sign(d.x * d.x - d.y),
        mix(
            -1.,
            1.,
            step(
                test_cross(A, B, p) * test_cross(B, C, p),
                0.
            )
        ),
        step(
            (d.x - d.y),
            0.
        )
    ) * test_cross(A, C, B);
}

fn remap(value: f32, from1: f32, to1: f32, from2: f32, to2: f32) -> f32 {
    return (value - from1) / (to1 - from1) * (to2 - from2) + from2;
}

fn solve_cubic(a: f32, b: f32, c: f32) -> vec3<f32> {
    var p = b - a * a / 3.;
    var p3 = p * p * p;
    var q = a * (2. * a * a - 9. * b) / 27. + c;
    var d = q * q + 4. * p3 / 27.;
    var offset = -a / 3.;

    if d >= 0. {
        var z = sqrt(d);
        var x = (vec2(z, -z) -q) / 2.;
        var uv = sign(x) * pow(abs(x), vec2(1./3.));
        return vec3<f32>(offset + uv.x + uv.y);
    }

    var v = acos(-sqrt(-27. / p3) * q / 2.) / 3.;
    var m = cos(v);
    var n = sin(v) * 1.732050808;
    return vec3<f32>(m + m, -n - m, n - m) * sqrt(-p / 3.) + offset;
}

// Unsigned distance (>= 0) from p to evaluated points on the curve.
fn sd_bezier(A: vec2<f32>, B: vec2<f32>, C: vec2<f32>, p: vec2<f32>) -> f32 {
    var new_B = mix(B + vec2<f32>(1e-4), B, abs(sign(B * 2. - A - C)));
    var a = new_B - A;
    var b = A - new_B * 2. + C;
    if dot(b, b) == 0. {
        let segment = C - A;
        let squared_length = dot(segment, segment);
        if squared_length == 0. {
            return length(p - A);
        }
        let t = clamp(dot(p - A, segment) / squared_length, 0., 1.);
        return length(A + segment * t - p);
    }
    var c = a * 2.;
    var d = A - p;
    var k = vec3<f32>(3. * dot(a, b), 2. * dot(a, a) + dot(d, b), dot(d, a)) / dot(b, b);
    var t = clamp(solve_cubic(k.x, k.y, k.z), vec3<f32>(0.), vec3<f32>(1.));
    var pos = A + (c + b * t.x) * t.x;
    var dis = length(pos - p);
    pos = A + (c + b * t.y) * t.y;
    dis = min(dis, length(pos - p));
    pos = A + (c + b * t.z) * t.z;
    dis = min(dis, length(pos- p));
    return dis;
}

fn sdf_triplet_alpha(sdf: vec3<f32>, horz_scale: f32, vert_scale: f32, vgrad: f32, doffset: f32) -> vec3<f32> {
    let hdoffset = mix(doffset * horz_scale, doffset * vert_scale, vgrad);
    let rdoffset = mix(doffset, hdoffset, 0.);
    var alpha = smoothstep(vec3(.5 - rdoffset), vec3(.5 + rdoffset), sdf);
    alpha = pow(alpha, vec3(1. + .2 * vgrad * 0.));
    return alpha;
}

// Allocation offsets are floats; each RGBA texel holds four consecutive floats.
fn atlas_width() -> i32 {
    return i32(textureDimensions(atlas_texture).x) * 4;
}

fn atlas_texel(offset: i32, layer: i32) -> vec4<f32> {
    let texel = offset / 4;
    let width = i32(textureDimensions(atlas_texture).x);
    return textureLoad(atlas_texture, vec2<i32>(texel % width, texel / width), layer, 0);
}

fn full_list(uv: vec2<f32>, input: VertexOutput) -> vec4<f32> {
    var sideR = 0.;
    var sideG = 0.;
    var sideB = 0.;

    var distR = 0.;
    var distG = 0.;
    var distB = 0.;
    var has_curve = false;

    let start = i32(input.atlas_pos.y) * atlas_width() + i32(input.atlas_pos.x);
    let layer = i32(input.layer);
    for (var i = 0; i < input.atlas_size; i += 8) {
        let offset = start + i;
        let ac = atlas_texel(offset, layer);
        let bi = atlas_texel(offset + 4, layer);
        let ax = ac.x;
        let ay = ac.y;
        let az = ac.z;
        let aw = ac.w;
        let bx = bi.x;
        let by = bi.y;

        // Only endpoint y-bands contribute crossing signs to the winding test.
        // Three x samples are computed, but output coverage is grayscale from R.
        // Central G drives nearest-curve selection and winding; B is unused.
        if ((uv.y > ay && uv.y < by) || (uv.y > by && uv.y < ay)) {
            let snR = sign_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv - vec2(1./3., 0.));
            let snG = sign_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv);
            let snB = sign_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv + vec2(1./3., 0.));
            sideR += snR;
            sideG += snG;
            sideB += snB;
        }

        let x = abs(sd_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv));
        if !has_curve || x < distG {
            has_curve = true;
            distR = abs(sd_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv - vec2(1./3., 0.)));
            distG = x;
            distB = abs(sd_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv + vec2(1./3., 0.)));
        }
    }

    return vec4(distR, distG, distB, sideG);
}

const BAND_MARGIN: f32 = 64.;
const FULL_LIST_REQUIRED: f32 = 1e20;

// Distance to the triangle hull, including degenerate line/point hulls.
fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let edge = b - a;
    let t = clamp(dot(p - a, edge) / max(dot(edge, edge), 1e-20), 0., 1.);
    return length(p - (a + t * edge));
}

fn hull_distance(p: vec2<f32>, a: vec2<f32>, c: vec2<f32>, b: vec2<f32>) -> f32 {
    let signs = vec3(test_cross(a, c, p), test_cross(c, b, p), test_cross(b, a, p));
    // A nondegenerate hull contains p when its edge signs agree.
    if test_cross(a, c, b) != 0. && (all(signs >= vec3(0.)) || all(signs <= vec3(0.))) {
        return 0.;
    }
    return min(segment_distance(p, a, c), min(segment_distance(p, c, b), segment_distance(p, b, a)));
}

fn band_curves(uv: vec2<f32>, start: i32, count: i32, winding_start: i32, winding_count: i32, layer: i32, window: f32) -> vec4<f32> {
    var sideR = 0.;
    var sideG = 0.;
    var sideB = 0.;
    for (var i = 0; i < winding_count; i += 8) {
        let offset = winding_start + i;
        let ac = atlas_texel(offset, layer);
        let bi = atlas_texel(offset + 4, layer);
        let ax = ac.x;
        let ay = ac.y;
        let az = ac.z;
        let aw = ac.w;
        let bx = bi.x;
        let by = bi.y;
        // Only endpoint y-bands contribute crossing signs to the winding test.
        // Three x samples are computed, but output coverage is grayscale from R.
        // Central G drives nearest-curve selection and winding; B is unused.
        if ((uv.y > ay && uv.y < by) || (uv.y > by && uv.y < ay)) {
            let snR = sign_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv - vec2(1./3., 0.));
            let snG = sign_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv);
            let snB = sign_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv + vec2(1./3., 0.));
            sideR += snR;
            sideG += snG;
            sideB += snB;
        }

    }
    var distR = 1e20;
    var distG = 1e20;
    var distB = 1e20;
    var best_index = 1e20;
    // At a few thousand font units, 0.05 covers many f32 ULPs in hull
    // projection and sd_bezier's clamped polynomial evaluation (including
    // its 1e-4 control perturbation), so rounding cannot cull a winner.
    const EPS: f32 = 0.05;
    for (var i = 0; i < count; i += 8) {
        let offset = start + i;
        let bi = atlas_texel(offset + 4, layer);
        let key = bi.w;
        let reach = min(distG, window) + EPS;
        if key - uv.x > reach {
            break;
        }
        let ac = atlas_texel(offset, layer);
        let a = ac.xy;
        let c = ac.zw;
        let b = bi.xy;
        if hull_distance(uv, a, c, b) > reach {
            continue;
        }
        let x = abs(sd_bezier(a, c, b, uv));
        let original_index = bi.z;
        if x < distG || (x == distG && original_index < best_index) {
            best_index = original_index;
            distR = abs(sd_bezier(a, c, b, uv - vec2(1./3., 0.)));
            distG = x;
            distB = abs(sd_bezier(a, c, b, uv + vec2(1./3., 0.)));
        }
    }
    return vec4(distR, distG, distB, sideG);
}

// Legacy width 26 − 0.16·s peaks in pixels at 81.25 px (13 font units). Past the peak,
// floor it at 0.25 px, capped at the peak width so the width stays continuous for any UPEM.
fn edge_width(font_size: f32, units_per_em: f32) -> f32 {
    let legacy = 26. - 0.16 * font_size;
    if font_size <= 81.25 { return legacy; }
    return max(legacy, min(0.25 * units_per_em, 1056.) / font_size);
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let font_size = input.font_size;
    var uv = input.uv;
    uv.x = remap(uv.x, 0., 1., 0., input.size.x * input.units_per_em / font_size);
    uv.x += input.left_side_bearing;
    uv.y = remap(uv.y, 0., 1., 0., input.size.y * input.units_per_em / font_size);

    let window = .5 + edge_width(font_size, input.units_per_em) + 1./3.;
    var nearest = vec4(0., 0., 0., FULL_LIST_REQUIRED);
    if window <= BAND_MARGIN && input.bands.z >= 0. && input.atlas_size > 0 {
        let header = i32(input.bands.y) * atlas_width() + i32(input.bands.x);
        let layer = i32(input.bands.z);
        let header_data = atlas_texel(header, layer);
        let min_y = header_data.x;
        let height = header_data.y;
        let count = i32(header_data.z);
        let band = clamp(i32(floor((uv.y - min_y) / height)), 0, count - 1);
        let descriptor = header + 8 + band * 8;
        let distance_data = atlas_texel(descriptor, layer);
        let winding_data = atlas_texel(descriptor + 4, layer);
        let start = header + i32(distance_data.x);
        let curve_count = i32(distance_data.y);
        let winding_start = header + i32(winding_data.x);
        let winding_count = i32(winding_data.y);
        nearest = band_curves(uv, start, curve_count, winding_start, winding_count, layer, window);
    }
    if nearest.w == FULL_LIST_REQUIRED {
        nearest = full_list(uv, input);
    }
    let vgrad = abs(dpdy(nearest.y));
    var triplet_alpha = sdf_triplet_alpha(nearest.xyz, .5, .6, vgrad, edge_width(font_size, input.units_per_em));
    if nearest.w == -2. {
        triplet_alpha.r = 1. - triplet_alpha.r;
    }
    return vec4(input.color.rgb, 1 - triplet_alpha.r);
}

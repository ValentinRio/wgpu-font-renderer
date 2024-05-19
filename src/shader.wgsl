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
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) pos: vec2<f32>,
    @location(2) left_side_bearing: f32,
    @location(3) font_size: f32,
    @location(4) size: vec2<f32>,
    @location(5) atlas_pos: vec2<f32>,
    @location(6) atlas_size: i32,
    @location(7) units_per_em: f32,
    @location(8) layer: f32,
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

    return output;
}

fn test_cross(a: vec2<f32>, b: vec2<f32>, p: vec2<f32>) -> f32 {
    return sign((b.y - a.y) * (p.x - a.x) - (b.x - a.x) * (p.y - a.y));
}

fn sign_bezier(A: vec2<f32>, B: vec2<f32>, C: vec2<f32>, p: vec2<f32>) -> f32 {
    let a: vec2<f32> = C - A;
    let b: vec2<f32> = B- A;
    let c: vec2<f32> = p - A;

    let r: f32 = (a.x * b.y - b.x * a.y);

    if abs(r) < 0.001 {
        return test_cross(A, B, p);
    }

    let bary = vec2<f32>(c.x * b.y - b.x * c.y, a.x * c.y - c.x * a.y) / r;
    let d = vec2<f32>(bary.y * .5, 0.) + 1.0 - bary.x - bary.y;
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

fn texel_to_float(tx: vec4<f32>) -> f32 {
    let r = u32(tx.a * 255.);
    let g = u32(tx.b * 255.);
    let b = u32(tx.g * 255.);
    let a = u32(tx.r * 255.);

    var bits: u32 = (r << 24u) | (g << 16u) | (b << 8u) | (a);

    var sign: u32 = (bits & (1u << 31u)) >> 31u;

    var fsign: f32 = -1.;

    if sign == 0u {
        fsign = 1.;
    }

    var e: u32 = (bits & 213909504u) >> 23u;

    var m = 8388608u;

    if e == 0u {
        m = (bits & 8388607u) << 1u;
    } else {
        m = (bits & 8388607u) | 8388608u;
    }

    let p = pow(2., f32(e) - 150.);

    let f = floor(fsign * f32(m) * p);

    return f;
}

fn remap(value: f32, from1: f32, to1: f32, from2: f32, to2: f32) -> f32 {
    return (value - from1) / (to1 - from1) * (to2 - from2) + from2;
}

fn linear_component(u: f32) -> f32 {
    if u < 0.0032308 {
        return u * 12.92;
    } else {
        return 1.055 * pow(u, 0.41666) - 0.055;
    }
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

fn sd_bezier(A: vec2<f32>, B: vec2<f32>, C: vec2<f32>, p: vec2<f32>) -> f32 {
    var new_B = mix(B + vec2<f32>(1e-4), B, abs(sign(B * 2.0 - A - C)));
    var a = new_B - A;
    var b = A - new_B * 2. + C;
    var c = a * 2.;
    var d = A - p;
    var k = vec3<f32>(3. * dot(a, b), 2 * dot(a, a) + dot(d, b), dot(d, a)) / dot(b, b);
    var t = clamp(solve_cubic(k.x, k.y, k.z), vec3<f32>(0.), vec3<f32>(1.));
    var pos = A + (c + b * t.x) * t.x;
    var dis = length(pos - p);
    pos = A + (c + b * t.y) * t.y;
    dis = min(dis, length(pos - p));
    pos = A + (c + b * t.z) * t.z;
    dis = min(dis, length(pos- p));
    return dis * sign_bezier(A, B, C, p);
}

fn sdf_triplet_alpha(sdf: vec3<f32>, horz_scale: f32, vert_scale: f32, vgrad: f32, doffset: f32) -> vec3<f32> {
    let hdoffset = mix(doffset * horz_scale, doffset * vert_scale, vgrad);
    let rdoffset = mix(doffset, hdoffset, 0.);
    var alpha = smoothstep(vec3(.5 - rdoffset), vec3(.5 + rdoffset), sdf);
    alpha = pow(alpha, vec3(1. + .2 * vgrad * 0.));
    return alpha;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let font_size = input.font_size;

    let ratio = input.size / input.size;

    var uv = input.uv * ratio;

    uv.x = remap(uv.x, 0., 1., 0., input.size.x * input.units_per_em / font_size);
    uv.y += input.left_side_bearing;
    uv.x = remap(uv.y, 0., 1., 0., input.size.y * input.units_per_em / font_size);

    var curve_points_count = input.atlas_size;
    var sideR = 0.;
    var sideG = 0.;
    var sideB = 0.;

    var distR = 0.;
    var distG = 0.;
    var distB = 0.;

    {
        var y_offset = input.atlas_pos.y;
        var x_offset = input.atlas_pos.x;
        var i = 0;
        loop {
            if !(i < curve_points_count) {
                break;
            }

            var atlas_x_offset = x_offset + f32(i);

            var pixels_before_eol = 2048. - atlas_x_offset;

            var x_offsets = array(
                atlas_x_offset,
                atlas_x_offset + 1.,
                atlas_x_offset + 2.,
                atlas_x_offset + 3.,
                atlas_x_offset + 4.,
                atlas_x_offset + 5.
            );

            if pixels_before_eol == 0. {
                y_offset = y_offset + 1.;
                x_offset = 0.;
                curve_points_count = curve_points_count - i;
            }

            let curve_Ax = textureSample(atlas_texture, atlas_sampler, vec2<f32>(x_offsets[0] / 2048., y_offset / 2048.), i32(0));
            let curve_Ay = textureSample(atlas_texture, atlas_sampler, vec2<f32>(x_offsets[1] / 2048., y_offset / 2048.), i32(0));
            let curve_Az = textureSample(atlas_texture, atlas_sampler, vec2<f32>(x_offsets[2] / 2048., y_offset / 2048.), i32(0));
            let curve_Aw = textureSample(atlas_texture, atlas_sampler, vec2<f32>(x_offsets[3] / 2048., y_offset / 2048.), i32(0));
            let curve_Bx = textureSample(atlas_texture, atlas_sampler, vec2<f32>(x_offsets[4] / 2048., y_offset / 2048.), i32(0));
            let curve_By = textureSample(atlas_texture, atlas_sampler, vec2<f32>(x_offsets[5] / 2048., y_offset / 2048.), i32(0));

            let ax = vec4<f32>(linear_component(curve_Ax.r), linear_component(curve_Ax.g), linear_component(curve_Ax.b), curve_Ax.a);
            let ay = vec4<f32>(linear_component(curve_Ay.r), linear_component(curve_Ay.g), linear_component(curve_Ay.b), curve_Ay.a);
            let az = vec4<f32>(linear_component(curve_Az.r), linear_component(curve_Az.g), linear_component(curve_Az.b), curve_Az.a);
            let aw = vec4<f32>(linear_component(curve_Aw.r), linear_component(curve_Aw.g), linear_component(curve_Aw.b), curve_Aw.a);
            let bx = vec4<f32>(linear_component(curve_Bx.r), linear_component(curve_Bx.g), linear_component(curve_Bx.b), curve_Bx.a);
            let by = vec4<f32>(linear_component(curve_By.r), linear_component(curve_By.g), linear_component(curve_By.b), curve_By.a);

            let f_ax = texel_to_float(ax) * 4.;
            let f_ay = texel_to_float(ay) * 4.;
            let f_az = texel_to_float(az) * 4.;
            let f_aw = texel_to_float(aw) * 4.;
            let f_bx = texel_to_float(bx) * 4.;
            let f_by = texel_to_float(by) * 4.;

            if ((uv.y > f_ay && uv.y < f_by) || (uv.y > f_by && uv.y < f_ay)) {
                let snR = sign_bezier(vec2<f32>(f_ax, f_ay), vec2<f32>(f_az, f_aw), vec2<f32>(f_bx, f_by), uv - vec2(1./3., 0.));
                let snG = sign_bezier(vec2<f32>(f_ax, f_ay), vec2<f32>(f_az, f_aw), vec2<f32>(f_bx, f_by), uv);
                let snB = sign_bezier(vec2<f32>(f_ax, f_ay), vec2<f32>(f_az, f_aw), vec2<f32>(f_bx, f_by), uv + vec2(1./3., 0.));
                sideR += snR;
                sideG += snG;
                sideB += snB;
            }

            let x = abs(sd_bezier(vec2<f32>(f_ax, f_ay), vec2<f32>(f_az, f_aw), vec2<f32>(f_bx, f_by), uv));
            if distG == 0. || x < distG {
                distR = abs(sd_bezier(vec2<f32>(f_ax, f_ay), vec2<f32>(f_az, f_aw), vec2<f32>(f_bx, f_by), uv - vec2(1./3., 0.)));
                distG = x;
                distB = abs(sd_bezier(vec2<f32>(f_ax, f_ay), vec2<f32>(f_az, f_aw), vec2<f32>(f_bx, f_by), uv + vec2(1./3., 0.)));
            }

            continuing {
                if pixels_before_eol == 0. {
                    i = 0;
                } else {
                    i = i + 8;
                }
            }
        }
    }

    var vgrad = vec2<f32>(dpdx(distG), dpdy(distG));

    let horz_scale = .5;
    let vert_scale = .6;

    var triplet_alpha = sdf_triplet_alpha(vec3(distR, distG, distB), horz_scale, vert_scale, abs(vgrad.y), 26. - 0.16 * font_size);

    var col = vec3(1., 1., 1.);

    if sideG == -2. {
        col = vec3(0., 0., 0.,);
    }

    return vec4(col, triplet_alpha.r);
}
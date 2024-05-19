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
    @location(9) color: vec4<f32>,
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

fn sd_bezier(A: vec2<f32>, B: vec2<f32>, C: vec2<f32>, p: vec2<f32>) -> f32 {
    var new_B = mix(B + vec2<f32>(1e-4), B, abs(sign(B * 2. - A - C)));
    var a = new_B - A;
    var b = A - new_B * 2. + C;
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

    var uv = input.uv;

    uv.x = remap(uv.x, 0., 1., 0., input.size.x * input.units_per_em / font_size);
    uv.x += input.left_side_bearing;
    uv.y = remap(uv.y, 0., 1., 0., input.size.y * input.units_per_em / font_size);

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

            if pixels_before_eol == 0. {
                y_offset = y_offset + 1.;
                x_offset = 0.;
                curve_points_count = curve_points_count - i;
            }

            let ax = textureSample(atlas_texture, atlas_sampler, vec2<f32>(atlas_x_offset / 2048., y_offset / 2048.), i32(0)).x;
            let ay = textureSample(atlas_texture, atlas_sampler, vec2<f32>((atlas_x_offset + 1.) / 2048., y_offset / 2048.), i32(0)).x;
            let az = textureSample(atlas_texture, atlas_sampler, vec2<f32>((atlas_x_offset + 2.) / 2048., y_offset / 2048.), i32(0)).x;
            let aw = textureSample(atlas_texture, atlas_sampler, vec2<f32>((atlas_x_offset + 3.) / 2048., y_offset / 2048.), i32(0)).x;
            let bx = textureSample(atlas_texture, atlas_sampler, vec2<f32>((atlas_x_offset + 4.) / 2048., y_offset / 2048.), i32(0)).x;
            let by = textureSample(atlas_texture, atlas_sampler, vec2<f32>((atlas_x_offset + 5.) / 2048., y_offset / 2048.), i32(0)).x;

            if ((uv.y > ay && uv.y < by) || (uv.y > by && uv.y < ay)) {
                let snR = sign_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv - vec2(1./3., 0.));
                let snG = sign_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv);
                let snB = sign_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv + vec2(1./3., 0.));
                sideR += snR;
                sideG += snG;
                sideB += snB;
            }

            let x = abs(sd_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv));
            if distG == 0. || x < distG {
                distR = abs(sd_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv - vec2(1./3., 0.)));
                distG = x;
                distB = abs(sd_bezier(vec2<f32>(ax, ay), vec2<f32>(az, aw), vec2<f32>(bx, by), uv + vec2(1./3., 0.)));
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

    var vgrad = abs(dpdy(distG));

    let horz_scale = .5;
    let vert_scale = .6;

    var triplet_alpha = sdf_triplet_alpha(vec3(distR, distG, distB), horz_scale, vert_scale, vgrad, 26. - 0.16 * font_size);

    if sideG == -2. {
        triplet_alpha.r = 1. - triplet_alpha.r;
    }

    return vec4(input.color.rgb, 1 - triplet_alpha.r);
}
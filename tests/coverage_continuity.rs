#![cfg(all(not(target_arch = "wasm32"), feature = "unstable-internals"))]
use skrifa::{
    instance::{LocationRef, Size},
    MetadataProvider,
};
use wgpu_font_renderer::{internals, FontStore};
#[path = "common/analytic.rs"]
#[allow(dead_code)]
mod analytic;
mod common;
#[path = "common/probe.rs"]
#[allow(dead_code)]
mod probe;
use common::box_integral::{critical_points, BoxIntegralModel};

// Float readback avoids the byte quantizer's unavoidable one-byte jumps.
// The remaining slack is eight times smaller than one byte and covers f32
// clipping arithmetic plus per-curve 1/65536 area quantization.
const FLOAT_SLACK: f32 = 1. / 2048.;

#[test]
fn wgsl_is_continuous_at_endpoints_and_interior_extrema() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut comparisons = 0;
    let mut worst_excess = f32::NEG_INFINITY;
    for (name, bytes, preset) in [
        (
            "Roboto",
            &include_bytes!("../fonts/Roboto-Regular.ttf")[..],
            "gB@",
        ),
        (
            "Cantarell VF",
            &include_bytes!("../fonts/Cantarell-VF.otf")[..],
            "gB@",
        ),
        (
            "Noto Serif SC",
            &include_bytes!("../fonts/NotoSerifSC-Subset.otf")[..],
            "龍鬱齉",
        ),
    ] {
        let mut store = FontStore::new(&device);
        let key = store
            .load_from_bytes(&device, &queue, bytes, preset)
            .unwrap();
        let font = internals::font(&store, key).unwrap();
        let units = f32::from(
            font.face()
                .metrics(Size::unscaled(), LocationRef::default())
                .units_per_em,
        );
        let body = r#"
            let sweep = u32(p.y) / 2u;
            let index = u32(p.x) + (u32(p.y) % 2u) * 256u;
            let settings = probes[2];
            if sweep >= u32(settings.w) || index >= 260u { return vec4(0.); }
            let centre = probes[3u + sweep];
            let axis = u32(centre.z);
            var input: VertexOutput;
            input.atlas_pos = probes[0].xy;
            input.atlas_size = i32(probes[0].z);
            input.layer = probes[0].w;
            input.bands = probes[1].xyz;
            input.pixel_scale = settings.x;
            input.uv = centre.xy;
            var coordinate = centre[axis] + (f32(index) - 128.) / (128. * settings.x);
            if index >= 257u {
                coordinate = centre[axis];
                if index != 258u {
                    let up = index == 259u;
                    if coordinate == 0. { coordinate = bitcast<f32>(select(0x80000001u, 1u, up)); }
                    else {
                        let bits = bitcast<u32>(coordinate);
                        coordinate = bitcast<f32>(select(bits - 1u, bits + 1u, up == (coordinate > 0.)));
                    }
                }
            }
            input.uv[axis] = coordinate;
            let offsets = select(vec3(0.), vec3(-1./3., 0., 1./3.), settings.z > 0.);
            let banded = analytic_coverage(input, offsets);
            input.bands.z = -1.;
            let full = analytic_coverage(input, offsets);
            let error = abs(banded - full);
            return vec4(banded, max(max(error.x, error.y), error.z));
        "#;
        let source = format!(
            "@group(0) @binding(2) var<uniform> probes: array<vec4<f32>, 1024>;\n{}",
            analytic::source(body)
        );
        let probe = probe::Probe::new_float(&device, internals::atlas_view(&store), &source);
        for ch in preset.chars() {
            let id = font.face().charmap().map(ch).unwrap();
            let glyph = &font.glyph_cache[&id.to_u32()];
            let points = critical_points(&glyph.curves);
            let slope = BoxIntegralModel::new(&glyph.curves).slope_bound();
            for size in [12., 24., 96.] {
                let scale = size / units;
                let mut centres = Vec::new();
                for point in &points {
                    for axis in 0..2 {
                        for offset in [-0.375, 0., 0.375] {
                            let mut p = *point;
                            p[1 - axis] += offset / scale;
                            centres.push([p[0], p[1], axis as f32, 0.]);
                        }
                    }
                }
                for lcd in [false, true] {
                    for batch in centres.chunks(384) {
                        let a = &glyph.allocation;
                        let b = &glyph.bands;
                        let mut inputs = vec![
                            [
                                a.position()[0] as f32,
                                a.position()[1] as f32,
                                a.size() as f32,
                                a.layer() as f32,
                            ],
                            [
                                b.position()[0] as f32,
                                b.position()[1] as f32,
                                b.layer() as f32,
                                0.,
                            ],
                            [scale, slope, u8::from(lcd) as f32, batch.len() as f32],
                        ];
                        inputs.extend_from_slice(batch);
                        let bytes =
                            probe.pixels(&device, &queue, wgpu::Color::BLACK, Some(&inputs));
                        let values: &[f32] = bytemuck::cast_slice(&bytes);
                        for (sweep, centre) in batch.iter().enumerate() {
                            let coordinate = centre[centre[2] as usize];
                            let mut samples: Vec<_> = (0..260)
                                .map(|index| {
                                    let position = match index {
                                        257 => coordinate.next_down(),
                                        258 => coordinate,
                                        259 => coordinate.next_up(),
                                        _ => coordinate + (index as f32 - 128.) / (128. * scale),
                                    };
                                    let pixel = (sweep * 512 + index) * 4;
                                    let coverage = &values[pixel..pixel + 3];
                                    assert!(coverage.iter().all(|v| v.is_finite()));
                                    assert_eq!(
                                        values[pixel + 3],
                                        0.,
                                        "band/full mismatch {name} {ch} {size} {lcd}"
                                    );
                                    (position, coverage)
                                })
                                .collect();
                            samples.sort_by(|a, b| a.0.total_cmp(&b.0));
                            for pair in samples.windows(2) {
                                let bound =
                                    slope * (pair[1].0 - pair[0].0).abs() * scale + FLOAT_SLACK;
                                for channel in 0..3 {
                                    let delta = (pair[1].1[channel] - pair[0].1[channel]).abs();
                                    worst_excess = worst_excess.max(delta - bound);
                                    assert!(delta <= bound, "{name} {ch} {size}px LCD={lcd}, centre {centre:?}, channel {channel}, {} -> {}: delta {delta}, bound {bound}", pair[0].0, pair[1].0);
                                    comparisons += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        eprintln!("WGSL continuity {name}: {comparisons} cumulative channel steps; worst excess {worst_excess}");
    }
    assert!(comparisons > 1_000_000);
    eprintln!(
        "WGSL continuity: {comparisons} steps; slack {FLOAT_SLACK}; worst excess {worst_excess}"
    );
}

use super::{glyph_bounding_box, parse_font, prepare_glyph};
use crate::atlas_tests::common::box_integral::BoxIntegralModel;
use skrifa::{
    instance::{LocationRef, Size},
    MetadataProvider,
};

// A box-filtered signed winding field has directional slope bounded by its
// density range. BoxIntegralModel derives a conservative bound from the maximum
// simultaneous increasing/decreasing crossings of the actual outline. Unlike
// a fixed jump allowance this scales with each step (including adjacent f32s).
// The f64 model only needs f32 output-rounding slack. GPU continuity is
// exercised separately through the production WGSL probe.
#[test]
fn cpu_box_model_is_continuous_at_every_preset_endpoint_and_extremum() {
    const LATIN: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,;:!ù*^$=)àç_è-('\"é&²<>+°§/.? ";
    let mut worst_excess = f32::NEG_INFINITY;
    let mut worst = String::new();
    let mut compared = 0;
    let mut largest_step = 0_f32;
    for (name, bytes, preset) in [
        (
            "Roboto",
            &include_bytes!("../../fonts/Roboto-Regular.ttf")[..],
            LATIN,
        ),
        (
            "Cantarell VF",
            &include_bytes!("../../fonts/Cantarell-VF.otf")[..],
            LATIN,
        ),
        (
            "Noto Serif SC",
            &include_bytes!("../../fonts/NotoSerifSC-Subset.otf")[..],
            "龍鬱齉直接从轮廓渲染字形文本",
        ),
    ] {
        let (face, _, _) = parse_font(bytes, 0).unwrap();
        let units = f32::from(
            face.metrics(Size::unscaled(), LocationRef::default())
                .units_per_em,
        );
        let ids: std::collections::BTreeSet<_> = preset
            .chars()
            .filter_map(|ch| face.charmap().map(ch))
            .chain(std::iter::once(skrifa::GlyphId::NOTDEF))
            .collect();
        let mut font_worst = f32::NEG_INFINITY;
        let mut font_largest = 0_f32;
        let mut font_detail = String::new();
        for id in ids {
            let Some(bounds) = glyph_bounding_box(&face, id) else {
                continue;
            };
            let (_, _, curves, _) = prepare_glyph(&face, id, bounds).unwrap();
            let model = BoxIntegralModel::new(&curves);
            let slope = model.slope_bound();
            let endpoints = crate::atlas_tests::common::box_integral::critical_points(&curves);
            for size in [12., 24., 96.] {
                let scale = size / units;
                for endpoint in &endpoints {
                    let vertex = *endpoint;
                    for axis in 0..2 {
                        let coordinate = vertex[axis];
                        let mut sweep: Vec<_> = (-128..=128)
                            .map(|i| coordinate + i as f32 / (128. * scale))
                            .collect();
                        sweep.extend([coordinate.next_down(), coordinate, coordinate.next_up()]);
                        sweep.sort_by(f32::total_cmp);
                        sweep.dedup();
                        for offset in [-0.375, 0., 0.375] {
                            let mut point = vertex;
                            point[1 - axis] += offset / scale;
                            let mut previous: Option<(f32, f32)> = None;
                            for &position in &sweep {
                                point[axis] = position;
                                let coverage = model.sample(point, scale);
                                assert!(coverage.is_finite());
                                if let Some((before, value)) = previous {
                                    let delta = (coverage - value).abs();
                                    font_largest = font_largest.max(delta);
                                    let bound = slope * (position - before).abs() * scale + 1e-6;
                                    let excess = delta - bound;
                                    if excess > font_worst {
                                        font_worst = excess;
                                        font_detail = format!("{name} glyph {id}, {size}px, vertex {vertex:?}, axis {axis}, offset {offset}: {before:?} -> {position:?}, coverage {value:.9} -> {coverage:.9}, delta {delta:.9}, slope {slope}, bound {bound:.9}");
                                    }
                                    compared += 1;
                                }
                                previous = Some((position, coverage));
                            }
                        }
                    }
                }
            }
        }
        assert!(font_worst.is_finite(), "{name}: no endpoint probes");
        largest_step = largest_step.max(font_largest);
        eprintln!("endpoint sweep worst: excess {font_worst:.9}; {font_detail}; largest step {font_largest:.9}");
        if font_worst > worst_excess {
            worst_excess = font_worst;
            worst = font_detail;
        }
    }
    eprintln!("endpoint sweep: {compared} steps; largest step {largest_step:.9}; worst excess {worst_excess:.9}; {worst}");
    assert!(worst_excess <= 0., "endpoint discontinuity: {worst}");
}

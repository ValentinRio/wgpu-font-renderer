use crate::{atlas, loader};
#[path = "../tests/common/mod.rs"]
pub(crate) mod common;
#[path = "../tests/common/probe.rs"]
pub(crate) mod probe;
use probe::{assert_ink_tiles, probe_pixels};

#[test]
fn roboto_935_cached_left_edge_matches_hmtx_bearing() {
    use skrifa::{raw::TableProvider, MetadataProvider};
    let bytes = include_bytes!("../fonts/Roboto-Regular.ttf");
    let face = skrifa::FontRef::new(bytes).unwrap();
    let id = skrifa::GlyphId::new(935);
    let character = face.charmap().mappings()
        .find_map(|(codepoint, mapped)| (mapped == id).then(|| char::from_u32(codepoint)).flatten())
        .expect("glyph 935 has a character mapping");
    let glyf = face.glyf().unwrap();
    let raw = face.loca(None).unwrap().get_glyf(id, &glyf).unwrap().unwrap();
    let expected = face.hmtx().unwrap().side_bearing(id).unwrap();
    assert_eq!(raw.x_min(), 90);
    assert_ne!(expected, raw.x_min());
    let Some((device, queue, _config)) = common::gpu() else {
        return;
    };
    let mut store = crate::FontStore::new(&device);
    let key = store.load_from_bytes(&device, &queue, bytes, &character.to_string()).unwrap();
    let font = store.get(key).unwrap();
    let glyph = &font.glyph_cache[&id.to_u32()];
    assert_eq!(glyph.bbox.x_min, expected);
    assert_eq!(glyph.left_side_bearing, expected);
}

#[test]
fn nonzero_layer_pixels_match_layer_zero() {
    placement_regression(atlas::SIZE * atlas::SIZE, 0, false);
}

#[test]
fn row_wrapped_pixels_match_unwrapped() {
    for prefix in [atlas::SIZE - 8, atlas::SIZE - 4] {
        placement_regression(prefix, 0, false);
    }
}

#[test]
fn nonzero_layer_uses_bands_at_400() {
    placement_regression(atlas::SIZE * atlas::SIZE, 0, true);
}

#[test]
fn row_wrapped_uses_bands_at_400() {
    placement_regression(atlas::SIZE - 4, 0, true);
}

#[test]
fn row_wrapped_band_data_matches_unwrapped_and_uses_bands_at_400() {
    placement_regression(0, atlas::SIZE - 4, true);
}

fn placement_regression(prefix: u32, band_prefix: u32, check_path: bool) {
    use skrifa::MetadataProvider;
    let Some((device, queue, _config)) = common::gpu() else {
        return;
    };
    for (c, bytes) in [
        ('g', &include_bytes!("../fonts/Roboto-Regular.ttf")[..]),
        ('B', &include_bytes!("../fonts/Roboto-Regular.ttf")[..]),
        (
            '@',
            &include_bytes!("../fonts/Cantarell-VF.otf")[..],
        ),
    ] {
        let mut results = Vec::new();
        for (padding, band_padding) in [(0, 0), (prefix, band_prefix)] {
            let mut atlas = atlas::Atlas::new(&device);
            let mut encoder = device.create_command_encoder(&Default::default());
            if padding > 0 {
                atlas
                    .upload(
                        padding,
                        &vec![0; padding as usize * 4],
                        &device,
                        &mut encoder,
                        &queue,
                    )
                    .unwrap();
            }
            if band_padding > 0 {
                atlas
                    .upload_bands(
                        band_padding,
                        &vec![0; band_padding as usize * 4],
                        &device,
                        &mut encoder,
                        &queue,
                    )
                    .unwrap();
            }
            let font = loader::Font::from_bytes(
                &device,
                &mut encoder,
                &queue,
                bytes.to_vec(),
                0,
                &c.to_string(),
                &mut atlas,
            )
            .unwrap();
            queue.submit(Some(encoder.finish()));
            let glyph = &font.glyph_cache[&font.face().charmap().map(c).unwrap().to_u32()];
            let [x, _] = glyph.allocation.position();
            let layer = glyph.allocation.layer();
            if padding == atlas::SIZE * atlas::SIZE {
                assert!(layer > 0);
            }
            if padding > 0 && padding < atlas::SIZE {
                assert!(x + glyph.allocation.size() > atlas::SIZE);
            }
            let bands = &glyph.bands;
            let [bx, _] = bands.position();
            if band_padding > 0 {
                assert!(
                    bx > 0 && bx + bands.size() > atlas::SIZE,
                    "glyph {c}: band allocation must start at a nonzero column and wrap"
                );
            }
            let [x, y] = glyph.allocation.packed_position();
            let [bx, by] = bands.packed_position();
            let shader = include_str!("shader.wgsl")
                .replace("@fragment\nfn fs_main", "fn coverage")
                .replace(
                    "fn coverage(input: VertexOutput) -> @location(0) vec4<f32>",
                    "fn coverage(input: VertexOutput) -> vec4<f32>",
                );
            let probe = format!(
                r#"
@vertex
fn probe_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {{
    let p = array(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.));
    return vec4(p[index], 0., 1.);
}}
@fragment
fn probe_fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
    let sizes = array(2., 12., 32., 72., 100., 162., 163., 200., 400., 557., 558., 1024.);
    var input: VertexOutput;
    input.font_size = sizes[u32(p.y) / 64u];
    input.uv = vec2(p.x / 256., (p.y % 64.) / 64.);
    input.size = vec2({width}., {height}.) * input.font_size / {units_per_em}.;
    input.units_per_em = {units_per_em}.;
    input.left_side_bearing = {bearing}.;
    input.atlas_pos = vec2({x}., {y}.);
    input.atlas_size = {count};
    input.layer = {layer}.;
    input.color = vec4(0., 0., 0., 1.);
    input.bands = vec3({bx}., {by}., {band_layer}.);
    input.pixel_scale = input.font_size / input.units_per_em;
    input.uv = input.uv * input.size / input.pixel_scale + vec2(input.left_side_bearing, 0.);
    return coverage(input);
}}
"#,
                width = glyph.bbox.width(),
                height = glyph.bbox.height(),
                bearing = glyph.left_side_bearing,
                count = glyph.allocation.size(),
                band_layer = bands.layer(),
                units_per_em = font.face().metrics(skrifa::instance::Size::unscaled(), &font.location).units_per_em
            );
            let source = format!("{shader}\n{probe}");
            if check_path {
                let debug = source.replace(
                    "if input.bands.z >= 0. && input.atlas_size > 0 {",
                    "if input.bands.z >= 0. && input.atlas_size > 0 { return vec3(1., 1., 1.);",
                );
                let pixels = probe_pixels(&device, &queue, atlas.resources(), &debug);
                assert!(
                    pixels[8 * 256 * 64 * 4..9 * 256 * 64 * 4]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|p| p[3] == 255),
                    "glyph {c} at [{x}, {y}], layer {layer} must use bands at size 400"
                );
            }
            let full = source.replace(
                "if input.bands.z >= 0. && input.atlas_size > 0 {",
                "if false {",
            );
            let reference = probe_pixels(&device, &queue, atlas.resources(), &full);
            assert_ink_tiles(&reference);
            results.push((
                probe_pixels(&device, &queue, atlas.resources(), &source),
                reference,
            ));
        }
        let differences = |a: &[u8], b: &[u8]| {
            a.as_chunks::<4>()
                .0
                .iter()
                .zip(b.as_chunks::<4>().0)
                .filter(|(a, b)| a != b)
                .count()
        };
        assert_eq!(
            differences(&results[0].1, &results[1].1),
            0,
            "full-list placement must not change pixels"
        );
        assert_eq!(
            differences(&results[0].0, &results[1].0),
            0,
            "banded placement must not change pixels"
        );
        for (banded, full) in results {
            assert_eq!(differences(&banded, &full), 0, "bands must match full list");
        }
    }
}

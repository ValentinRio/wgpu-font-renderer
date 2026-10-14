use super::*;
use crate::{atlas, atlas_tests::common, GlyphId, Paragraph, TextRenderer, TypeWriter};
#[path = "../common/draw.rs"]
mod draw;

const ROBOTO: &[u8] = include_bytes!("../../fonts/Roboto-Regular.ttf");

fn pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    store: &FontStore,
    paragraph: &Paragraph,
) -> Vec<u8> {
    let mut renderer = TextRenderer::new(device, wgpu::TextureFormat::Rgba8Unorm, store);
    renderer.prepare(device, queue, [paragraph], store);
    draw::draw_pixels(device, queue, &mut renderer, [128, 128], wgpu::Color::WHITE)
}

#[test]
fn missing_character_draws_notdef_and_empty_notdef_only_advances() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let key = store.load_from_bytes(&device, &queue, ROBOTO, "A").unwrap();
    assert!(store.get(key).unwrap().glyph_cache.contains_key(&0));
    let paragraph = TypeWriter::new()
        .shape_text(&store, key, [24.; 2], 32., [0., 0., 0., 1.], "\u{10ffff}")
        .unwrap();
    assert_eq!(paragraph.glyphs()[0].glyph, GlyphId(0));
    assert!(paragraph.width() > 0.);
    assert!(pixels(&device, &queue, &store, &paragraph)
        .iter()
        .any(|&b| b != 255));

    // Make glyph zero empty by giving its two loca entries the same offset.
    use skrifa::raw::TableProvider;
    let face = skrifa::FontRef::new(ROBOTO).unwrap();
    let count = u16::from_be_bytes(ROBOTO[4..6].try_into().unwrap()) as usize;
    let record = (0..count)
        .map(|i| 12 + i * 16)
        .find(|&i| &ROBOTO[i..i + 4] == b"loca")
        .unwrap();
    let offset = u32::from_be_bytes(ROBOTO[record + 8..record + 12].try_into().unwrap()) as usize;
    let entry_size = if face.head().unwrap().index_to_loc_format() == 0 {
        2
    } else {
        4
    };
    let mut bytes = ROBOTO.to_vec();
    bytes.copy_within(offset + entry_size..offset + 2 * entry_size, offset);
    let key = store.load_from_bytes(&device, &queue, &bytes, "").unwrap();
    assert!(!store.get(key).unwrap().glyph_cache.contains_key(&0));
    let paragraph = TypeWriter::new()
        .shape_text(&store, key, [24.; 2], 32., [0., 0., 0., 1.], "\u{10ffff}")
        .unwrap();
    assert!(paragraph.width() > 0.);
    assert!(pixels(&device, &queue, &store, &paragraph)
        .iter()
        .all(|&b| b == 255));
}

#[test]
fn failed_load_names_glyphs_rolls_back_and_preserves_earlier_font() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let key = store.load_from_bytes(&device, &queue, ROBOTO, "A").unwrap();
    let paragraph = TypeWriter::new()
        .shape_text(&store, key, [24.; 2], 32., [0., 0., 0., 1.], "A")
        .unwrap();
    let before = pixels(&device, &queue, &store, &paragraph);
    assert!(before.iter().any(|&b| b != 255));
    // Keep the initial two physical layers; leave room for one .notdef only.
    store.atlas.max_layers = 2;
    let font = store.get(key).unwrap();
    let used: u32 = font.glyph_cache.values().map(|g| g.allocation.size()).sum();
    let notdef_size = font.glyph_cache[&0].allocation.size();
    let mut encoder = device.create_command_encoder(&Default::default());
    let padding = atlas::SIZE * atlas::SIZE - used - notdef_size;
    store
        .atlas
        .upload(
            padding,
            &vec![0; padding as usize * 4],
            &device,
            &mut encoder,
            &queue,
        )
        .unwrap();
    queue.submit(Some(encoder.finish()));
    let count = store.cache.len();
    // Glyph zero consumes the remaining capacity before the requested glyphs fail.
    let error = store
        .load_from_bytes(&device, &queue, ROBOTO, "\u{10ffff}BCB")
        .unwrap_err();
    let cmap = swash::FontRef::from_index(ROBOTO, 0).unwrap().charmap();
    assert!(matches!(error, LoadingError::AtlasFull(ref ids)
        if ids == &[GlyphId(cmap.map('B')), GlyphId(cmap.map('C'))]));
    assert_eq!(store.cache.len(), count);
    assert_eq!(pixels(&device, &queue, &store, &paragraph), before);
    // A subsequent successful load proves the failed load reclaimed its allocation.
    let next = store.load_from_bytes(&device, &queue, ROBOTO, "").unwrap();
    assert!(store.get(next).unwrap().glyph_cache.contains_key(&0));
    assert_eq!(pixels(&device, &queue, &store, &paragraph), before);
}

#[test]
fn decomposed_mark_draws_at_shaped_offset() {
    let Some((device, queue, _)) = common::gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let bytes = common::decomposed_font();
    let key = store
        .load_from_bytes(&device, &queue, &bytes, "e\u{301}")
        .unwrap();
    let paragraph = TypeWriter::new()
        .shape_text(&store, key, [24.; 2], 48., [0., 0., 0., 1.], "e\u{301}")
        .unwrap();
    assert_eq!(paragraph.glyphs().len(), 2);
    let mark = paragraph.glyphs()[1];
    assert_ne!(mark.offset, [0.; 2]);
    assert!(mark.offset[1] <= 0., "the accent must not be offset downward: {:?}", mark.offset);
    let font = store.get(key).unwrap();
    let base_box = font.glyph_cache[&(paragraph.glyphs()[0].glyph.0 as u32)].bbox;
    let mark_box = font.glyph_cache[&(mark.glyph.0 as u32)].bbox;
    if mark.offset[1] == 0. {
        assert!(mark_box.y_min > base_box.y_max, "a zero-offset accent outline must sit above the base");
    }

    let advance = paragraph.glyphs()[0].advance;
    let mut offset_mark = Paragraph::new(key, [24. + advance, 24.], 48., [0., 0., 0., 1.]);
    offset_mark.push_at(mark.glyph, mark.advance, mark.offset);
    let mut positioned_mark = Paragraph::new(
        key,
        [24. + advance + mark.offset[0], 24. + mark.offset[1]],
        48.,
        [0., 0., 0., 1.],
    );
    positioned_mark.push(mark.glyph, mark.advance);
    let mut origin_mark = Paragraph::new(key, [24. + advance, 24.], 48., [0., 0., 0., 1.]);
    origin_mark.push(mark.glyph, mark.advance);
    let at_offset = pixels(&device, &queue, &store, &offset_mark);
    assert!(at_offset.iter().any(|&b| b != 255));
    assert_eq!(at_offset, pixels(&device, &queue, &store, &positioned_mark));
    assert_ne!(at_offset, pixels(&device, &queue, &store, &origin_mark));
    let mut base = Paragraph::new(key, [24.; 2], 48., [0., 0., 0., 1.]);
    base.push(paragraph.glyphs()[0].glyph, advance);
    let base_pixels = pixels(&device, &queue, &store, &base);
    assert!(dark_bounds(&at_offset)[3] < dark_bounds(&base_pixels)[1],
        "accent ink must lie above the base's x-height in a y-down frame");
}


#[test]
fn repeated_and_unmapped_characters_reserve_each_glyph_once() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    for bytes in [ROBOTO, &include_bytes!("../../fonts/Cantarell-VF.otf")[..],
        &include_bytes!("../../fonts/NotoSerifSC-Subset.otf")[..]] {
        let mut repeated = FontStore::new(&device);
        let mut unique = FontStore::new(&device);
        repeated.load_from_bytes(&device, &queue, bytes, "HelloHello\u{ffff}\u{10ffff}\u{ffff}").unwrap();
        unique.load_from_bytes(&device, &queue, bytes, "Helo\u{ffff}").unwrap();
        assert_eq!(repeated.atlas.used_floats(), unique.atlas.used_floats());
    }
}

#[test]
fn band_exhaustion_fails_and_rolls_back_curve_allocations() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let key = store.load_from_bytes(&device, &queue, ROBOTO, "A").unwrap();
    let paragraph = TypeWriter::new().shape_text(&store, key, [24.; 2], 32., [0., 0., 0., 1.], "A").unwrap();
    let before = pixels(&device, &queue, &store, &paragraph);
    store.atlas.max_layers = 2;
    let padding = atlas::SIZE * atlas::SIZE - store.atlas.used_floats()[1];
    let mut encoder = device.create_command_encoder(&Default::default());
    store.atlas.upload_bands(padding, &vec![0; padding as usize * 4], &device, &mut encoder, &queue).unwrap();
    queue.submit(Some(encoder.finish()));
    let used = store.atlas.used_floats();
    let count = store.cache.len();
    let error = store.load_from_bytes(&device, &queue, ROBOTO, "BB").unwrap_err();
    let cmap = swash::FontRef::from_index(ROBOTO, 0).unwrap().charmap();
    assert!(matches!(error, LoadingError::AtlasFull(ref ids) if ids == &[GlyphId(cmap.map('B')), GlyphId(0)]));
    assert_eq!(store.cache.len(), count);
    assert_eq!(store.atlas.used_floats(), used);
    assert_eq!(pixels(&device, &queue, &store, &paragraph), before);
}


fn dark_bounds(pixels: &[u8]) -> [u32; 4] {
    let mut bounds = [u32::MAX, u32::MAX, 0, 0];
    for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if pixel[..3].iter().any(|&channel| channel < 128) {
            let x = index as u32 % 128;
            let y = index as u32 / 128;
            bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x), bounds[3].max(y)];
        }
    }
    assert_ne!(bounds[0], u32::MAX);
    bounds
}

#[cfg(feature = "unstable-internals")]
#[test]
fn bearing_positions_match_cpu_coverage_and_advance_spacing() {
    use skrifa::{MetadataProvider, instance::{Size, LocationRef}};
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let key = store.load_from_bytes(&device, &queue, ROBOTO, "gI").unwrap();
    let font = store.get(key).unwrap();
    let face = font.face();
    let scale = 64. / face.metrics(Size::unscaled(), LocationRef::default()).units_per_em as f32;
    let g = face.charmap().map('g').unwrap();
    let i = face.charmap().map('I').unwrap();
    let advance = face.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(g).unwrap() * scale;
    let mut left_edges = Vec::new();
    let mut bearings = Vec::new();
    for (id, pen) in [(g, 24.), (i, 24. + advance)] {
        let glyph = &font.glyph_cache[&id.to_u32()];
        let bearing = glyph.left_side_bearing as f32 * scale;
        assert!(bearing >= 3.);
        bearings.push(bearing);
        let mut paragraph = Paragraph::new(key, [pen, 24.], 64., [0., 0., 0., 1.]);
        paragraph.push(GlyphId(id.to_u32() as u16), 0.);
        let actual = pixels(&device, &queue, &store, &paragraph);
        left_edges.push(dark_bounds(&actual)[0] as f32);
        let top = 24. + (glyph.y_offset as f32 + f32::from(glyph.descent).abs()) * scale;
        // The CPU reference's screen origin depends on the pen, independently
        // of the renderer's instance origin or bearing handling.
        let reference = common::reference::reference_coverage(&glyph.curves,
            -pen / scale, -top / scale, scale, 128, 128);
        // Check a stripe around the CPU-predicted left edge. Straight I edges
        // and g's middle left stem obey the same 1/255 tolerance as c7.
        let edge = (pen + bearing).floor() as usize;
        let y = (top + glyph.bbox.height() as f32 * scale * 0.5) as usize;
        assert!(reference[y * 128 + edge + 1] > 0.5, "reference stripe must contain ink");
        for x in edge - 2..=edge + 2 {
            for channel in 0..3 {
                let coverage = 1. - f32::from(actual[(y * 128 + x) * 4 + channel]) / 255.;
                assert!((coverage - reference[y * 128 + x]).abs() <= 1. / 255.,
                    "bearing coverage for glyph {} at {x},{y}: GPU {coverage}, CPU {}", id.to_u32(), reference[y * 128 + x]);
            }
        }
    }
    assert_ne!(bearings[0], bearings[1]);
    assert!((left_edges[1] - left_edges[0] - (advance + bearings[1] - bearings[0])).abs() <= 1.);
}

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

#[test]
fn metrics_and_instances_share_bytes_and_preserve_the_character_set() {
    use skrifa::{MetadataProvider, instance::{LocationRef, Size}, raw::TableProvider};
    let Some((device, queue, _)) = common::gpu() else { return; };
    for bytes in [ROBOTO, &include_bytes!("../../fonts/Cantarell-VF.otf")[..]] {
        let mut store = FontStore::new(&device);
        let base = store.load_from_bytes(&device, &queue, bytes, "Hi").unwrap();
        let face = store.get(base).unwrap().face();
        let hhea = face.hhea().unwrap();
        let os2 = face.os2().unwrap();
        let typo = os2.version() >= 4 && os2.fs_selection().contains(
            skrifa::raw::tables::os2::SelectionFlags::USE_TYPO_METRICS);
        let scale = 100. / f32::from(face.metrics(Size::unscaled(), LocationRef::default()).units_per_em);
        let expected = if typo {
            [os2.s_typo_ascender(), os2.s_typo_descender(), os2.s_typo_line_gap()]
        } else {
            [hhea.ascender().to_i16(), hhea.descender().to_i16(), hhea.line_gap().to_i16()]
        };
        let metrics = store.metrics(base, 100.).unwrap();
        for (actual, expected) in [metrics.ascent, metrics.descent, metrics.line_gap]
            .into_iter().zip([f32::from(expected[0]), f32::from(expected[1]).abs(), f32::from(expected[2])]) {
            assert!((actual - expected * scale).abs() < 0.01);
        }
        let glyph = store.get(base).unwrap().glyph_cache.values().next().unwrap();
        let anchor = glyph.y_offset + i32::from(glyph.bbox.y_max) - i32::from(glyph.descent);
        assert!((metrics.ascent - anchor as f32 * scale).abs() < 0.01);
        for size in [0., -1., f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(store.metrics(base, size).is_none());
        }
        let mut foreign = FontStore::new(&device);
        let foreign_id = foreign.load_from_bytes(&device, &queue, bytes, "Hi").unwrap();
        assert!(store.metrics(foreign_id, 100.).is_none());
        assert!(store.axes(foreign_id).is_none());
        assert!(matches!(store.load_variation(&device, &queue, foreign_id, &[]), Err(LoadingError::UnknownFont)));
        let instance = store.load_variation(&device, &queue, base, &[]).unwrap();
        assert_ne!(instance, base);
        let a = store.get(base).unwrap();
        let b = store.get(instance).unwrap();
        assert!(std::sync::Arc::ptr_eq(&a.data, &b.data));
        assert!(std::sync::Arc::ptr_eq(&a.characters, &b.characters));
        assert_eq!(a.location, b.location);
        assert_eq!(a.glyph_cache.len(), b.glyph_cache.len());
        assert!(a.glyph_cache.keys().all(|id| b.glyph_cache.contains_key(id)));
        let mut writer = TypeWriter::new();
        assert_eq!(writer.shape_text(&store, base, [0.; 2], 100., [1.; 4], "Hi").unwrap().glyphs(),
            writer.shape_text(&store, instance, [0.; 2], 100., [1.; 4], "Hi").unwrap().glyphs());
        assert_eq!(pixels(&device, &queue, &store, &writer.shape_text(&store, base, [24.; 2], 32., [0.,0.,0.,1.], "Hi").unwrap()),
            pixels(&device, &queue, &store, &writer.shape_text(&store, instance, [24.; 2], 32., [0.,0.,0.,1.], "Hi").unwrap()));
        if let Some(axis) = store.axes(base).unwrap().first() {
            let axis = *axis;
            let clamp = store.load_variation(&device, &queue, instance, &[(*b"????", 10.), (axis.tag, axis.min), (axis.tag, axis.max + 1000.)]).unwrap();
            let max = store.load_variation(&device, &queue, base, &[(axis.tag, axis.max)]).unwrap();
            assert_eq!(store.get(clamp).unwrap().location, store.get(max).unwrap().location);
            assert_eq!(store.metrics(clamp, 100.), store.metrics(max, 100.));
            let clamp = store.load_variation(&device, &queue, base, &[(axis.tag, axis.min - 1000.)]).unwrap();
            let min = store.load_variation(&device, &queue, base, &[(axis.tag, axis.min)]).unwrap();
            assert_eq!(store.get(clamp).unwrap().location, store.get(min).unwrap().location);
        } else {
            let instance = store.load_variation(&device, &queue, base, &[(*b"wght", 900.)]).unwrap();
            assert_eq!(store.metrics(base, 100.), store.metrics(instance, 100.));
        }
    }
}

#[test]
fn default_shaping_applies_feature_variations_covering_zero() {
    use swash::{shape::ShapeContext, text::Script};
    let Some((device, queue, _)) = common::gpu() else { return; };
    for (bytes, has_substitution) in [
        (include_bytes!("../../fonts/Cantarell-VF.otf").to_vec(), false),
        (common::zero_feature_variation_font(), true),
    ] {
        let mut store = FontStore::new(&device);
        let text = "I II AV To fi ffi e\u{301} 0123!? àÅgß";
        let base = store.load_from_bytes(&device, &queue, &bytes, &format!("{text}H")).unwrap();
        let font = store.get(base).unwrap();
        assert_ne!(font.as_ref().charmap().map('I'), 0);
        assert_ne!(font.as_ref().charmap().map('H'), 0);
        assert!(!font.location.coords().is_empty());
        assert!(font.location.coords().iter().all(|coord| coord.to_bits() == 0));
        let without_coords = TypeWriter::new().shape_glyphs(font.as_ref(), 32., text);
        let actual = TypeWriter::new().shape_text(&store, base, [0.; 2], 32., [1.; 4], text).unwrap();
        let mut context = ShapeContext::new();
        let mut shaper = context.builder(font.as_ref()).script(Script::Latin).size(32.)
            .normalized_coords([0i16]).build();
        shaper.add_str(text);
        let mut with_zero = Vec::new();
        shaper.shape_with(|cluster| with_zero.extend(cluster.glyphs.iter().map(|g| crate::ShapedGlyph {
            glyph: GlyphId(g.id), advance: g.advance, offset: [g.x, -g.y],
        })));
        assert_eq!(actual.glyphs(), with_zero);
        if has_substitution {
            let charmap = font.as_ref().charmap();
            assert_eq!(without_coords[0].glyph, GlyphId(charmap.map('I')));
            assert_eq!(actual.glyphs()[0].glyph, GlyphId(charmap.map('H')));
            assert_ne!(actual.glyphs(), without_coords, "default location must activate I -> H");
            let ink = TypeWriter::new().shape_text(&store, base, [24.; 2], 32.,
                [0., 0., 0., 1.], "I").unwrap();
            assert_eq!(ink.glyphs()[0].glyph, GlyphId(charmap.map('H')));
            assert!(pixels(&device, &queue, &store, &ink).iter().any(|&value| value != 255),
                "the default FeatureVariations substitution must draw cached H ink");
        } else {
            assert_eq!(actual.glyphs(), without_coords);
        }
    }
}

#[test]
fn chained_instances_keep_unmodified_axes_and_borrow_cached_axes() {
    use skrifa::MetadataProvider;
    let Some((device, queue, _)) = common::gpu() else { return; };
    let bytes = common::two_axis_font();
    let mut store = FontStore::new(&device);
    let base = store.load_from_bytes(&device, &queue, &bytes, "Hi").unwrap();
    let axes = store.axes(base).unwrap();
    assert_eq!(axes.len(), 2);
    assert!(std::ptr::eq(axes, store.axes(base).unwrap()));
    let bold = store.load_variation(&device, &queue, base, &[(*b"wght", 700.)]).unwrap();
    let narrow = store.load_variation(&device, &queue, bold, &[(*b"wdth", 75.)]).unwrap();
    let expected = store.get(base).unwrap().face().axes().location([("wght", 700.), ("wdth", 75.)]);
    assert_eq!(store.get(narrow).unwrap().location, expected);
    let inherited = store.load_variation(&device, &queue, narrow, &[]).unwrap();
    assert_eq!(store.get(inherited).unwrap().location, expected);
    let unknown = store.load_variation(&device, &queue, narrow, &[(*b"????", 1.)]).unwrap();
    assert_eq!(store.get(unknown).unwrap().location, expected);
    let reset = store.load_variation(&device, &queue, narrow, &[(*b"wght", 400.)]).unwrap();
    let expected = store.get(base).unwrap().face().axes().location([("wght", 400.), ("wdth", 75.)]);
    assert_eq!(store.get(reset).unwrap().location, expected);
}

#[test]
fn missing_controls_and_default_ignorables_draw_only_neighbouring_letters() {
    let cmap = swash::FontRef::from_index(ROBOTO, 0).unwrap().charmap();
    for ch in ['\n', '\t', '\u{034F}', '\u{FE0F}'] {
        assert_eq!(cmap.map(ch), 0, "fixture must lack {ch:?}");
    }
    assert_eq!(cmap.map('龍'), 0, "fixture must lack the CJK letter");
    assert_ne!(cmap.map('a'), 0);
    assert_ne!(cmap.map('b'), 0);
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let font = store.load_from_bytes(&device, &queue, ROBOTO, "ab").unwrap();
    let mut writer = TypeWriter::new();
    let plain = writer.shape_text(&store, font, [24.; 2], 32., [0., 0., 0., 1.], "ab").unwrap();
    let reference = pixels(&device, &queue, &store, &plain);
    assert_eq!(plain.glyphs().len(), 2);
    for text in ["a\nb", "a\tb", "a\u{034F}b", "a\u{FE0F}b"] {
        let paragraph = writer.shape_text(&store, font, [24.; 2], 32., [0., 0., 0., 1.], text).unwrap();
        assert_eq!(paragraph.glyphs(), plain.glyphs(), "invisible source character in {text:?}");
        assert_eq!(paragraph.width(), plain.width());
        assert_eq!(pixels(&device, &queue, &store, &paragraph), reference,
            "only the two letters may draw for {text:?}");
    }
    let controls = writer.shape_text(&store, font, [24.; 2], 32., [0., 0., 0., 1.], "\n\t\u{034F}").unwrap();
    assert!(controls.glyphs().is_empty());
    assert_eq!(controls.width(), 0.);
    assert!(pixels(&device, &queue, &store, &controls).iter().all(|&value| value == 255));
    let missing = writer.shape_text(&store, font, [24.; 2], 32., [0., 0., 0., 1.], "a龍b").unwrap();
    assert_eq!(missing.glyphs().len(), 3);
    assert_eq!(missing.glyphs()[1].glyph, GlyphId(0));
    assert!(missing.glyphs()[1].advance > 0.);
    assert!(missing.width() > plain.width());
    assert_ne!(pixels(&device, &queue, &store, &missing), reference,
        "missing letters must still draw notdef ink");
    let missing_only = writer.shape_text(&store, font, [24.; 2], 32., [0., 0., 0., 1.], "龍").unwrap();
    assert_eq!(missing_only.glyphs()[0].glyph, GlyphId(0));
    assert!(pixels(&device, &queue, &store, &missing_only).iter().any(|&value| value != 255),
        "the missing letter's notdef must have ink");
}

#[test]
fn mapped_control_keeps_its_real_glyph_and_swash_positioning() {
    use swash::{shape::ShapeContext, text::Script};
    let face = swash::FontRef::from_index(ROBOTO, 0).unwrap();
    let cmap = face.charmap();
    assert_eq!(cmap.map('\r'), 3, "Roboto maps CR to a real glyph");
    assert_ne!(cmap.map('a'), 0);
    assert_ne!(cmap.map('b'), 0);
    let mut context = ShapeContext::new();
    let mut shaper = context.builder(face).script(Script::Latin).size(32.).retain_ignorables(true).build();
    shaper.add_str("a\rb");
    let mut expected = Vec::new();
    shaper.shape_with(|cluster| expected.extend(cluster.glyphs.iter().map(|g| crate::ShapedGlyph {
        glyph: GlyphId(g.id), advance: g.advance, offset: [g.x, -g.y],
    })));
    assert!(expected.iter().any(|g| g.glyph == GlyphId(3)));
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let font = store.load_from_bytes(&device, &queue, ROBOTO, "ab\r").unwrap();
    let paragraph = TypeWriter::new().shape_text(&store, font, [24.; 2], 32., [0., 0., 0., 1.], "a\rb").unwrap();
    assert!(paragraph.glyphs().iter().any(|g| g.glyph == GlyphId(3)));
    assert_eq!(paragraph.glyphs(), expected);
    assert_eq!(paragraph.width(), expected.iter().map(|g| g.advance).sum::<f32>());
}

#[test]
fn swash_hides_mapped_default_ignorables_as_before() {
    let cmap = swash::FontRef::from_index(ROBOTO, 0).unwrap().charmap();
    assert_ne!(cmap.map('\u{00AD}'), 0, "Roboto maps soft hyphen to a real glyph");
    assert_ne!(cmap.map('\u{200B}'), 0, "Roboto maps ZWSP to a real glyph");
    assert_ne!(cmap.map('a'), 0);
    assert_ne!(cmap.map('b'), 0);
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let font = store.load_from_bytes(&device, &queue, ROBOTO, "ab\u{00AD}\u{200B}").unwrap();
    let mut writer = TypeWriter::new();
    let plain = writer.shape_text(&store, font, [24.; 2], 32., [0., 0., 0., 1.], "ab").unwrap();
    for text in ["a\u{00AD}b", "a\u{200B}b"] {
        let paragraph = writer.shape_text(&store, font, [24.; 2], 32., [0., 0., 0., 1.], text).unwrap();
        assert_eq!(paragraph.glyphs(), plain.glyphs());
        assert_eq!(paragraph.width(), plain.width());
        assert_eq!(pixels(&device, &queue, &store, &paragraph), pixels(&device, &queue, &store, &plain));
    }
}

#[test]
fn visible_ligature_starting_at_missing_default_ignorable_is_retained() {
    use swash::{shape::ShapeContext, text::Script};
    let bytes = common::missing_ignorable_ligature_font();
    let face = swash::FontRef::from_index(&bytes, 0).unwrap();
    let charmap = face.charmap();
    assert_eq!(charmap.map('\u{034F}'), 0);
    assert_ne!(charmap.map('a'), 0);
    let output = GlyphId(charmap.map('H'));
    assert_ne!(output, GlyphId(0));
    let mut context = ShapeContext::new();
    let mut shaper = context.builder(face).script(Script::Latin).size(32.).build();
    shaper.add_str("\u{034F}a");
    let mut expected = Vec::new();
    shaper.shape_with(|cluster| expected.extend(cluster.glyphs.iter().map(|g| crate::ShapedGlyph {
        glyph: GlyphId(g.id), advance: g.advance, offset: [g.x, -g.y],
    })));
    assert_eq!(expected.len(), 1, "fixture must form a ligature");
    assert_eq!(expected[0].glyph, output);
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let font = store.load_from_bytes(&device, &queue, &bytes, "aH\u{034F}").unwrap();
    let paragraph = TypeWriter::new().shape_text(&store, font, [24.; 2], 32., [0., 0., 0., 1.], "\u{034F}a").unwrap();
    assert_eq!(paragraph.glyphs(), expected, "a visible ligature must survive its source marker");
    assert_eq!(paragraph.width(), expected[0].advance);
    assert!(pixels(&device, &queue, &store, &paragraph).iter().any(|&value| value != 255));
}

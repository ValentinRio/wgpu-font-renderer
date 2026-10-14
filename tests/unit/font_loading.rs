use super::*;
use crate::{atlas_tests::common, FontStore, TypeWriter};
use std::collections::BTreeSet;

const ROBOTO: &[u8] = include_bytes!("../../fonts/Roboto-Regular.ttf");

fn table_record(bytes: &[u8], tag: &[u8; 4]) -> usize {
    let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
    (0..count).map(|i| 12 + i * 16)
        .find(|&record| &bytes[record..record + 4] == tag).unwrap()
}

fn table_range(bytes: &[u8], tag: &[u8; 4]) -> std::ops::Range<usize> {
    let record = table_record(bytes, tag);
    let offset = u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap()) as usize;
    let length = u32::from_be_bytes(bytes[record + 12..record + 16].try_into().unwrap()) as usize;
    offset..offset + length
}

fn patch_u16(bytes: &mut [u8], tag: &[u8; 4], relative: usize, value: u16) {
    let offset = table_range(bytes, tag).start + relative;
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn with_macintosh_cmap() -> Vec<u8> {
    let mut bytes = ROBOTO.to_vec();
    let cmap = bytes[table_range(&bytes, b"cmap")].to_vec();
    let count = u16::from_be_bytes(cmap[2..4].try_into().unwrap());
    let old_header = 4 + count as usize * 8;
    let new_header = old_header + 8;
    // Put a Macintosh format 0 record and its subtable before every old record.
    let mut replacement = vec![0; new_header + 262];
    replacement[2..4].copy_from_slice(&(count + 1).to_be_bytes());
    replacement[4..6].copy_from_slice(&1u16.to_be_bytes());
    replacement[8..12].copy_from_slice(&(new_header as u32).to_be_bytes());
    for i in 0..count as usize {
        let old = 4 + i * 8;
        let new = old + 8;
        replacement[new..new + 4].copy_from_slice(&cmap[old..old + 4]);
        let offset = u32::from_be_bytes(cmap[old + 4..old + 8].try_into().unwrap());
        replacement[new + 4..new + 8].copy_from_slice(&(offset + 270).to_be_bytes());
    }
    replacement[new_header + 2..new_header + 4].copy_from_slice(&262u16.to_be_bytes());
    let wrong_id = FontRef::from_index(ROBOTO, 0).unwrap().charmap().map('B');
    replacement[new_header + 6 + b'A' as usize] = u8::try_from(wrong_id).unwrap();
    replacement.extend_from_slice(&cmap[old_header..]);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    let offset = bytes.len() as u32;
    let length = replacement.len() as u32;
    bytes.extend_from_slice(&replacement);
    let record = table_record(&bytes, b"cmap");
    bytes[record + 8..record + 12].copy_from_slice(&offset.to_be_bytes());
    bytes[record + 12..record + 16].copy_from_slice(&length.to_be_bytes());
    bytes
}

#[test]
fn bundled_unicode_and_presets_agree_with_swash() {
    // All Unicode mappings, without a runtime cap, plus the coverage, shader,
    // LCD and text presets (and a missing character and glyph-zero mapping).
    let presets = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,;:!ù*^$=)àç_è-('\"é&²<>+°§/.? igB@éHiAVToWAàÅgß龍鬱齉直接从轮廓渲染字形文本\u{ffff}\u{10ffff}";
    let mut cases = Vec::new();
    for (name, bytes) in [
        ("Roboto", ROBOTO),
        ("Cantarell", &include_bytes!("../../fonts/Cantarell-VF.otf")[..]),
        ("Noto subset", &include_bytes!("../../fonts/NotoSerifSC-Subset.otf")[..]),
    ] {
        let face = SkFontRef::new(bytes).unwrap();
        let font = FontRef::from_index(bytes, 0).unwrap();
        let charmap = font.charmap();
        let mut characters: BTreeSet<char> = face.charmap().mappings()
            .filter_map(|(codepoint, _)| char::from_u32(codepoint)).collect();
        charmap.enumerate(|codepoint, _| {
            if let Some(ch) = char::from_u32(codepoint) { characters.insert(ch); }
        });
        characters.extend(presets.chars());
        let preset: String = characters.iter().collect();
        let expected: BTreeSet<_> = glyph_ids(font, &preset)
            .filter(|&id| glyph_bounding_box(&face, id).is_some())
            .map(|id| id.to_u32()).collect();

        let mut writer = TypeWriter::new();
        let mut shaped_characters = Vec::new();
        for &ch in &characters {
            let text = ch.to_string();
            let id = charmap.map(ch);
            let mapped = Some(id as u32);
            let actual = glyph_ids(font, &text).next().map(|id| id.to_u32());
            assert_eq!(actual, mapped,
                "{name} U+{:04X}: loader and charmap IDs", ch as u32);
            // U+00AD SOFT HYPHEN produces an empty cluster in Roboto and Cantarell;
            // there is no first shaped glyph to compare. Keep its ID/cache coverage.
            if ch == '\u{00ad}' { continue; }
            let Some(id) = actual else { continue; };
            if !expected.contains(&id) { continue; }
            let shaped = writer.shape_glyphs(font, 32., &text);
            let first = shaped.first().unwrap_or_else(||
                panic!("{name} U+{:04X}: outlined character shapes a glyph", ch as u32)).glyph.0 as u32;
            if shaped.len() == 1 {
                assert_eq!(first, id,
                    "{name} U+{:04X}: shaped and loader IDs for this character", ch as u32);
            }
            assert!(expected.contains(&first),
                "{name} U+{:04X}: first shaped glyph {first} must have a cached outline", ch as u32);
            shaped_characters.push((ch, id));
        }
        eprintln!("{name}: {} characters checked, no cap", characters.len());
        cases.push((name, bytes, shaped_characters, preset, expected));
    }
    let Some((device, queue, _)) = common::gpu() else { return; };
    for (name, bytes, characters, preset, expected) in cases {
        let mut store = FontStore::new(&device);
        let key = store.load_from_bytes(&device, &queue, bytes, &preset).unwrap();
        let cached: BTreeSet<_> = store.get(key).unwrap().glyph_cache.keys().copied().collect();
        assert_eq!(cached, expected, "{name}: cached keys match loader-selected outlined glyphs");
        if glyph_bounding_box(&SkFontRef::new(bytes).unwrap(), GlyphId::new(0)).is_some() {
            assert!(cached.contains(&0), "{name}: outlined .notdef must be cached");
        }
        let mut writer = TypeWriter::new();
        for (ch, id) in characters {
            let text = ch.to_string();
            let paragraph = writer.shape_text(&store, key, [0.; 2], 32., [1.; 4], &text).unwrap();
            let first = paragraph.glyphs().first().unwrap().glyph.0 as u32;
            if paragraph.glyphs().len() == 1 {
                assert_eq!(first, id,
                    "{name} U+{:04X}: shaped and loader IDs for this character", ch as u32);
            }
            assert!(cached.contains(&first),
                "{name} U+{:04X}: first shaped glyph {first} must be in the font cache", ch as u32);
        }
    }
}

#[test]
fn macintosh_cmap_before_unicode_still_caches_the_shaped_glyph() {
    let bytes = with_macintosh_cmap();
    let face = SkFontRef::new(&bytes).unwrap();
    let font = FontRef::from_index(&bytes, 0).unwrap();
    let unicode = font.charmap().map('A') as u32;
    let macintosh = face.cmap().unwrap().map_codepoint('A').unwrap().to_u32();
    let original = FontRef::from_index(ROBOTO, 0).unwrap().charmap();
    assert_eq!(unicode, original.map('A') as u32);
    assert_eq!(macintosh, original.map('B') as u32);
    assert_ne!(macintosh, unicode, "fixture must expose the first-record lookup bug");
    let loaded = glyph_ids(font, "A").next().unwrap().to_u32();
    assert_eq!(loaded, unicode);
    let shaped = TypeWriter::new().shape_glyphs(font, 32., "A");
    assert_eq!(shaped.len(), 1);
    assert_eq!(shaped[0].glyph.0 as u32, loaded);
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let key = store.load_from_bytes(&device, &queue, &bytes, "A").unwrap();
    let paragraph = TypeWriter::new().shape_text(&store, key, [0.; 2], 32., [1.; 4], "A").unwrap();
    assert_eq!(paragraph.glyphs().len(), 1);
    assert_eq!(paragraph.glyphs()[0].glyph.0 as u32, loaded);
    let cached: BTreeSet<_> = store.get(key).unwrap().glyph_cache.keys().copied().collect();
    assert_eq!(cached, BTreeSet::from([0, unicode]), "loader must cache the Unicode glyph swash shaped");
}

#[test]
fn units_per_em_range_is_validated_before_loading() {
    for upem in [0, 15, 16385] {
        let mut bytes = ROBOTO.to_vec();
        patch_u16(&mut bytes, b"head", 18, upem);
        assert!(matches!(parse_font(&bytes, 0),
            Err(LoadingError::InvalidFont)), "invalid unitsPerEm {upem}");
    }
    for upem in [16, 16384] {
        let mut bytes = ROBOTO.to_vec();
        patch_u16(&mut bytes, b"head", 18, upem);
        assert!(parse_font(&bytes, 0).is_ok(),
            "valid unitsPerEm endpoint {upem}");
    }
}

#[test]
fn ascender_selection_covers_every_021_branch() {
    // tuple: version, USE_TYPO_METRICS, typo ascender, signed Windows ascender
    for (hhea, os2, expected) in [
        (900, None, 900),
        (0, None, 0),
        (900, Some((3, true, 800, 1000)), 900),
        (900, Some((4, true, 800, 1000)), 800),
        (900, Some((4, true, 0, 1000)), 0),
        (900, Some((4, false, 800, 1000)), 900),
        (0, Some((3, true, 800, 1000)), 800),
        (0, Some((4, false, 800, 1000)), 800),
        (0, Some((4, false, 0, 1000)), 1000),
        (0, Some((0, false, 0, -32768)), -32768),
        (0, Some((4, true, 0, 1000)), 0),
    ] {
        assert_eq!(select_ascender(hhea, os2), expected, "hhea={hhea}, OS/2={os2:?}");
    }
}

#[test]
fn ascender_reads_legacy_table_values_including_the_version_gate() {
    for (version, length, use_typo, hhea, typo, windows, expected) in [
        (3, 100, true, 900, 800, 1000, 900),
        (4, 100, true, 900, 800, 1000, 800),
        (4, 100, false, 0, 800, 1000, 800),
        (4, 100, false, 0, 0, 1000, 1000),
        (4, 100, true, 900, 0, 1000, 0),
        (6, 100, true, 900, 800, 1000, 900),
        (4, 78, false, 0, 800, 1000, 0),
    ] {
        let mut bytes = ROBOTO.to_vec();
        patch_u16(&mut bytes, b"hhea", 4, hhea as u16);
        patch_u16(&mut bytes, b"hhea", 6, (-300i16) as u16);
        patch_u16(&mut bytes, b"OS/2", 0, version);
        patch_u16(&mut bytes, b"OS/2", 62, if use_typo { 1 << 7 } else { 0 });
        patch_u16(&mut bytes, b"OS/2", 68, typo as u16);
        patch_u16(&mut bytes, b"OS/2", 74, windows);
        let record = table_record(&bytes, b"OS/2");
        bytes[record + 12..record + 16].copy_from_slice(&(length as u32).to_be_bytes());
        let face = SkFontRef::new(&bytes).unwrap();
        assert!(face.os2().is_ok());
        assert_eq!(face_ascender(&face), expected, "OS/2 version {version}, hhea {hhea}");
    }
}

#[test]
fn changed_hmtx_bearing_shifts_g_box_and_outline_exactly() {
    let mut bytes = ROBOTO.to_vec();
    let face = SkFontRef::new(&bytes).unwrap();
    let id = face.charmap().map('g').unwrap();
    let original = face.hmtx().unwrap().side_bearing(id).unwrap();
    assert_eq!(original, 96);
    let original_bbox = glyph_bounding_box(&face, id).unwrap();
    assert_eq!(original_bbox, Rect { x_min: 96, y_min: -426, x_max: 1010, y_max: 1102 });
    let mut original_outline = BezierBuilder::new(original_bbox.y_max as f32);
    assert_eq!(outline_glyph(&face, id, &mut original_outline), Some(original_bbox));
    original_outline.finish();

    patch_u16(&mut bytes, b"hmtx", id.to_u32() as usize * 4 + 2, (original - 17) as u16);
    let face = SkFontRef::new(&bytes).unwrap();
    let raw = face.glyph_metrics(Size::unscaled(), LocationRef::default()).bounds(id).unwrap();
    assert_eq!([raw.x_min, raw.y_min, raw.x_max, raw.y_max], [96., -426., 1010., 1102.]);
    let glyf = face.glyf().unwrap();
    let glyph = face.loca(None).unwrap().get_glyf(id, &glyf).unwrap().unwrap();
    let bearing = face.hmtx().unwrap().side_bearing(id).unwrap();
    assert_eq!(bearing, 79);
    let shift = (bearing - glyph.x_min()) as f32;
    assert_eq!(shift, -17.);
    let placed = glyph_bounding_box(&face, id).unwrap();
    assert_eq!(placed, Rect { x_min: 79, y_min: -426, x_max: 993, y_max: 1102 });
    assert_eq!([raw.x_min + shift, raw.y_min, raw.x_max + shift, raw.y_max],
        [placed.x_min, placed.y_min, placed.x_max, placed.y_max].map(f32::from));

    let mut outline = BezierBuilder::new(placed.y_max as f32);
    assert_eq!(outline_glyph(&face, id, &mut outline), Some(placed));
    outline.finish();
    assert!(!original_outline.curves.is_empty());
    assert_eq!(outline.curves.len(), original_outline.curves.len());
    for (before, after) in original_outline.curves.as_chunks::<8>().0.iter()
        .zip(outline.curves.as_chunks::<8>().0) {
        let mut expected = *before;
        for x in [0, 2, 4] { expected[x] -= 17.; }
        assert_eq!(*after, expected, "each outline x moves by exactly the bearing change");
    }
}

#[test]
fn roboto_935_box_outline_and_cache_apply_standard_hmtx_placement() {
    let face = SkFontRef::new(ROBOTO).unwrap();
    let glyf = face.glyf().unwrap();
    let loca = face.loca(None).unwrap();
    let id = GlyphId::new(935);
    let glyph = loca.get_glyf(id, &glyf).unwrap().unwrap();
    assert_eq!([glyph.x_min(), glyph.y_min(), glyph.x_max(), glyph.y_max()],
        [90, -436, 1140, 1097]);
    let bearing = face.hmtx().unwrap().side_bearing(id).unwrap();
    assert_eq!(bearing, 41);
    let shift = bearing - glyph.x_min();
    assert_eq!(shift, -49);
    let bbox = glyph_bounding_box(&face, id).unwrap();
    assert_eq!([bbox.x_min, bbox.y_min, bbox.x_max, bbox.y_max],
        [41, -436, 1091, 1097]);
    assert_eq!(bbox.x_min, glyph.x_min() + shift);
    assert_eq!(bbox.x_max, glyph.x_max() + shift);
    let mut builder = BezierBuilder::new(bbox.y_max as f32);
    assert_eq!(outline_glyph(&face, id, &mut builder), Some(bbox));
    builder.finish();
    let xs: Vec<_> = builder.curves.as_chunks::<8>().0.iter()
        .flat_map(|s| [s[0], s[2], s[4]]).collect();
    assert_eq!(xs.iter().copied().reduce(f32::min).unwrap(), 41.);
    assert_eq!(xs.iter().copied().reduce(f32::max).unwrap(), 1091.);
    let ch = face.charmap().mappings().find(|&(_, mapped)| mapped == id).unwrap().0;
    let text = char::from_u32(ch).unwrap().to_string();
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let key = store.load_from_bytes(&device, &queue, ROBOTO, &text).unwrap();
    let cached = &store.get(key).unwrap().glyph_cache[&id.to_u32()];
    assert_eq!(cached.bbox, bbox);
    assert_eq!(cached.left_side_bearing, 41);
    #[cfg(feature = "unstable-internals")]
    assert_eq!(cached.curves, builder.curves);
}

#[test]
fn glyph_selection_deduplicates_before_outline_lookup() {
    let font = FontRef::from_index(ROBOTO, 0).unwrap();
    let ids: Vec<_> = glyph_ids(font, "HelloHello\u{ffff}\u{10ffff}").collect();
    assert_eq!(ids.len(), ids.iter().collect::<std::collections::HashSet<_>>().len());
    assert_eq!(ids.iter().filter(|id| id.to_u32() == 0).count(), 1);
}

#[test]
fn line_metric_families_follow_legacy_ascender_fallbacks() {
    for (version, use_typo, hhea, typo, expected) in [
        (3, true, 900, 800, [900., 300., 20.]),
        (4, true, 900, 800, [800., 250., 30.]),
        (4, false, 0, 800, [800., 250., 30.]),
        (4, false, 0, 0, [1000., 400., 0.]),
        (4, true, 900, 0, [0., 250., 30.]),
        (6, true, 900, 800, [900., 300., 20.]),
    ] {
        let mut bytes = ROBOTO.to_vec();
        for (tag, offset, value) in [
            (b"hhea", 4, hhea as u16), (b"hhea", 6, (-300i16) as u16),
            (b"hhea", 8, 20), (b"OS/2", 0, version),
            (b"OS/2", 62, if use_typo { 1 << 7 } else { 0 }),
            (b"OS/2", 68, typo as u16), (b"OS/2", 70, (-250i16) as u16),
            (b"OS/2", 72, 30), (b"OS/2", 74, 1000), (b"OS/2", 76, 400),
        ] { patch_u16(&mut bytes, tag, offset, value); }
        let face = SkFontRef::new(&bytes).unwrap();
        let metrics = face_line_metrics(&face, LocationRef::default());
        assert_eq!([metrics.ascent, metrics.descent, metrics.line_gap], expected);
    }
}

#[test]
fn normalized_location_changes_outlines_bounds_and_metric_deltas() {
    let face = SkFontRef::new(include_bytes!("../../fonts/Cantarell-VF.otf")).unwrap();
    let axis = face.axes().get_by_tag(skrifa::raw::types::Tag::new(b"wght")).unwrap();
    let id = face.charmap().map('I').unwrap();
    let mut outlines_at_extremes = Vec::new();
    for value in [axis.min_value(), axis.max_value()] {
        let location = face.axes().location([("wght", value)]);
        let bbox = glyph_bounding_box_at(&face, id, (&location).into()).unwrap();
        let (_, _, curves, _) = prepare_glyph_at(&face, id, bbox, (&location).into()).unwrap();
        let mut expected = BezierBuilder::new(f32::from(bbox.y_max));
        face.outline_glyphs().get(id).unwrap().draw(draw_settings((&location).into()), &mut expected).unwrap();
        expected.finish();
        assert_eq!(curves, expected.curves);
        let bounds = face.glyph_metrics(Size::unscaled(), &location).bounds(id).unwrap();
        assert_eq!(bbox.x_min, bounds.x_min as i16);
        assert_eq!(bbox.x_max, bounds.x_max as i16);
        outlines_at_extremes.push((bbox, curves));
    }
    assert_ne!(outlines_at_extremes[0], outlines_at_extremes[1]);
    let bytes = common::mvar_anchor_font();
    let face = SkFontRef::new(&bytes).unwrap();
    let location = face.axes().location([("wght", axis.max_value())]);
    let default = face_line_metrics(&face, LocationRef::default());
    let varied = face_line_metrics(&face, (&location).into());
    assert_eq!(varied.ascent - default.ascent, 100.);
}

#[test]
fn review_metrics_apply_descent_and_line_gap_mvar_deltas() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    let bytes = common::mvar_anchor_font();
    let mut store = FontStore::new(&device);
    let base = store.load_from_bytes(&device, &queue, &bytes, "I").unwrap();
    let instance = store.load_variation(&device, &queue, base, &[(*b"wght", 800.)]).unwrap();
    let face = store.get(base).unwrap().face();
    let location = face.axes().location([("wght", 800.)]);
    let scale = 100. / f32::from(face.metrics(Size::unscaled(), &location).units_per_em);
    let mvar = face.mvar().unwrap();
    use skrifa::raw::tables::mvar::tags::{HDSC, HLGP};
    let descent_delta = mvar.metric_delta(HDSC, location.coords()).unwrap().to_f64() as f32;
    let gap_delta = mvar.metric_delta(HLGP, location.coords()).unwrap().to_f64() as f32;
    assert_eq!(descent_delta, -50.);
    assert_eq!(gap_delta, 30.);
    let default = store.metrics(base, 100.).unwrap();
    let varied = store.metrics(instance, 100.).unwrap();
    assert!((varied.descent - default.descent + descent_delta * scale).abs() < 0.01,
        "descent must include instance HDSC");
    assert!((varied.line_gap - default.line_gap - gap_delta * scale).abs() < 0.01,
        "line gap must include instance HLGP");
}

#[test]
fn review_store_bounds_and_bearings_use_instance_location() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let base = store.load_from_bytes(&device, &queue, include_bytes!("../../fonts/Cantarell-VF.otf"), "Ig").unwrap();
    for value in [100., 800., 550.] {
        let instance = store.load_variation(&device, &queue, base, &[(*b"wght", value)]).unwrap();
        let font = store.get(instance).unwrap();
        let face = font.face();
        let location = face.axes().location([("wght", value)]);
        let mut differs = false;
        for ch in "Ig".chars() {
            let id = face.charmap().map(ch).unwrap();
            let bounds = face.glyph_metrics(Size::unscaled(), &location).bounds(id).unwrap();
            let expected = Rect { x_min: bounds.x_min as i16, y_min: bounds.y_min as i16,
                x_max: bounds.x_max as i16, y_max: bounds.y_max as i16 };
            let glyph = &font.glyph_cache[&id.to_u32()];
            assert_eq!(glyph.bbox, expected, "cached bbox must use instance location");
            assert_eq!(glyph.left_side_bearing, expected.x_min, "cached bearing must use instance location");
            let default = store.get(base).unwrap().glyph_cache[&id.to_u32()].bbox;
            differs |= expected != default && expected.x_min != default.x_min;
        }
        assert!(differs, "fixture must distinguish default bounds and bearings");
    }
}

#[test]
fn review_store_outlines_are_skrifa_instance_outlines() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let base = store.load_from_bytes(&device, &queue, include_bytes!("../../fonts/Cantarell-VF.otf"), "Ig").unwrap();
    for value in [100., 800., 550.] {
        let instance = store.load_variation(&device, &queue, base, &[(*b"wght", value)]).unwrap();
        let font = store.get(instance).unwrap();
        let face = font.face();
        let location = face.axes().location([("wght", value)]);
        for ch in "Ig".chars() {
            let id = face.charmap().map(ch).unwrap();
            let bounds = face.glyph_metrics(Size::unscaled(), &location).bounds(id).unwrap();
            let mut expected = BezierBuilder::new(f32::from(bounds.y_max as i16));
            face.outline_glyphs().get(id).unwrap().draw(
                DrawSettings::unhinted(Size::unscaled(), &location).with_path_style(PathStyle::HarfBuzz),
                &mut expected).unwrap();
            expected.finish();
            // Curves are inspectable under the same feature as GPU coverage tests.
            #[cfg(feature = "unstable-internals")]
            assert_eq!(font.glyph_cache[&id.to_u32()].curves, expected.curves,
                "store outlines must match independent instance draw");
        }
    }
}

#[test]
fn review_avar_midrange_instance_matches_mapped_location_and_advance() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let base = store.load_from_bytes(&device, &queue, include_bytes!("../../fonts/Cantarell-VF.otf"), "II").unwrap();
    let instance = store.load_variation(&device, &queue, base, &[(*b"wght", 550.)]).unwrap();
    let font = store.get(instance).unwrap();
    let face = font.face();
    let location = face.axes().location([("wght", 550.)]);
    let linear = face.axes().iter().next().unwrap().normalize(550.);
    assert_ne!(location.coords()[0], linear, "fixture must exercise avar remapping");
    assert_eq!(font.location, location, "instance location must include avar");
    let id = face.charmap().map('I').unwrap();
    let advance = face.glyph_metrics(Size::unscaled(), &location).advance_width(id).unwrap()
        * 100. / f32::from(face.metrics(Size::unscaled(), &location).units_per_em);
    let paragraph = TypeWriter::new().shape_text(&store, instance, [0.; 2], 100., [1.; 4], "II").unwrap();
    assert!((paragraph.width() - 2. * advance).abs() < 0.01, "midrange advance must follow mapped location");
}

#[test]
fn review_nonvariable_instance_with_axes_has_identical_cached_glyphs() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut store = FontStore::new(&device);
    let base = store.load_from_bytes(&device, &queue, ROBOTO, "Ig").unwrap();
    let instance = store.load_variation(&device, &queue, base, &[(*b"wght", 800.), (*b"wdth", 75.)]).unwrap();
    let a = store.get(base).unwrap();
    let b = store.get(instance).unwrap();
    assert!(a.location.coords().is_empty() && b.location.coords().is_empty());
    assert_eq!(a.glyph_cache.len(), b.glyph_cache.len());
    for (id, expected) in &a.glyph_cache {
        let actual = &b.glyph_cache[id];
        assert_eq!(actual.bbox, expected.bbox);
        assert_eq!(actual.left_side_bearing, expected.left_side_bearing);
        assert_eq!(actual.descent, expected.descent);
        assert_eq!(actual.y_offset, expected.y_offset);
        #[cfg(feature = "unstable-internals")]
        assert_eq!(actual.curves, expected.curves);
    }
}

#[test]
fn review_reversed_axis_ranges_keep_default_and_use_skrifa_clamping() {
    let Some((device, queue, _)) = common::gpu() else { return; };
    let mut bytes = include_bytes!("../../fonts/Cantarell-VF.otf").to_vec();
    let axis = table_range(&bytes, b"fvar").start + 16;
    bytes[axis + 4..axis + 8].copy_from_slice(&(800i32 << 16).to_be_bytes());
    bytes[axis + 12..axis + 16].copy_from_slice(&(100i32 << 16).to_be_bytes());
    let mut store = FontStore::new(&device);
    let base = store.load_from_bytes(&device, &queue, &bytes, "I").unwrap();
    assert!(LocationRef::from(&store.get(base).unwrap().location).is_default());
    let instance = store.load_variation(&device, &queue, base, &[(*b"wght", 550.)]).unwrap();
    let expected = store.get(base).unwrap().face().axes().location([("wght", 550.)]);
    assert_eq!(store.get(instance).unwrap().location, expected);
}

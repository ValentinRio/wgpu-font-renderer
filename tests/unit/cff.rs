use super::*;

#[test]
fn cff_winding_matches_truetype_and_final_close_is_idempotent() {
    fn area(curves: &[f32]) -> f32 {
        curves
            .as_chunks::<8>()
            .0
            .iter()
            .map(|s| {
                (s[0] * s[5] - s[1] * s[4]) / 6.
                    + (s[0] * s[3] - s[1] * s[2] + s[2] * s[5] - s[3] * s[4]) / 3.
            })
            .sum()
    }
    let (roboto, _, _) =
        parse_font(include_bytes!("../../examples/Roboto-Regular.ttf"), 0).unwrap();
    let (cantarell, _, _) = parse_font(include_bytes!("../fixtures/Cantarell-VF.otf"), 0).unwrap();
    let mut areas = Vec::new();
    for (face, reverse) in [
        (roboto.as_face_ref(), false),
        (cantarell.as_face_ref(), true),
    ] {
        let id = face.glyph_index('H').unwrap();
        let bbox = face.glyph_bounding_box(id).unwrap();
        let mut raw = BezierBuilder::new(bbox.y_max as f32);
        face.outline_glyph(id, &mut raw).unwrap();
        let mut encoded = BezierBuilder::new(bbox.y_max as f32);
        face.outline_glyph(id, &mut encoded).unwrap();
        encoded.finish();
        let finished = encoded.curves.clone();
        encoded.finish();
        assert_eq!(encoded.curves, finished, "close must not reverse twice");
        let segments = encoded.curves.as_chunks::<8>().0;
        assert_eq!(segments.last().unwrap()[4..6], segments[0][..2]);
        // H consists solely of straight edges; their control must remain end.
        assert!(segments.iter().all(|s| s[2..4] == s[4..6]));
        if reverse {
            assert!(area(&raw.curves) < 0.);
        } else {
            assert_eq!(
                raw.curves, encoded.curves,
                "TrueType records stay unchanged"
            );
        }
        areas.push(area(&encoded.curves));
    }
    assert_eq!(areas, [675798., 141916.]);
    eprintln!(
        "Encoded H signed areas (y-down): Roboto={}, Cantarell={}",
        areas[0], areas[1]
    );
}

#[test]
fn cantarell_cff2_outlines_chain_inside_padded_bounds() {
    struct Contours {
        builder: BezierBuilder,
        starts: Vec<usize>,
        cubics: usize,
    }
    impl OutlineBuilder for Contours {
        fn move_to(&mut self, x: f32, y: f32) {
            self.starts.push(self.builder.curves.len() / 8);
            self.builder.move_to(x, y);
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.builder.line_to(x, y);
        }
        fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
            self.builder.quad_to(x1, y1, x, y);
        }
        fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
            self.cubics += 1;
            self.builder.curve_to(x1, y1, x2, y2, x, y);
        }
        fn close(&mut self) {
            self.builder.close();
        }
    }
    let (face, _, _) = parse_font(include_bytes!("../fixtures/Cantarell-VF.otf"), 0).unwrap();
    let face = face.as_face_ref();
    assert!(face
        .raw_face()
        .table(owned_ttf_parser::Tag::from_bytes(b"CFF2"))
        .is_some());
    let mut outlined = 0;
    let mut cubics = 0;
    for id in 0..face.number_of_glyphs() {
        let id = GlyphId(id);
        let mut contours = Contours {
            builder: BezierBuilder::new(0.),
            starts: Vec::new(),
            cubics: 0,
        };
        let Some(bbox) = face.outline_glyph(id, &mut contours) else {
            assert!(contours.builder.curves.is_empty(), "partial glyph {id:?}");
            continue;
        };
        contours.builder.finish();
        // Builder height zero encodes y as -y; translating by y_max gives
        // the same y-down coordinates used by the font loader.
        let segments = contours.builder.curves.as_chunks::<8>().0;
        assert!(!segments.is_empty(), "glyph {id:?}");
        contours.starts.push(segments.len());
        for range in contours.starts.windows(2) {
            let contour = &segments[range[0]..range[1]];
            if let Some(first) = contour.first() {
                assert_eq!(
                    contour.last().unwrap()[4..6],
                    first[..2],
                    "closed glyph {id:?}"
                );
            }
            for pair in contour.windows(2) {
                assert_eq!(pair[0][4..6], pair[1][..2], "glyph {id:?}");
            }
        }
        let pad = face.units_per_em() as f32 * 0.15;
        for segment in segments {
            assert_eq!(segment[6..], [0., 0.]);
            for point in segment[..6].as_chunks::<2>().0 {
                assert!(
                    point[0] >= bbox.x_min as f32 - pad
                        && point[0] <= bbox.x_max as f32 + pad
                        && point[1] >= -(bbox.y_max as f32) - pad
                        && point[1] <= -(bbox.y_min as f32) + pad,
                    "glyph {id:?}: {point:?} outside {bbox:?}"
                );
            }
        }
        cubics += contours.cubics;
        outlined += 1;
    }
    assert!(
        outlined > 100 && cubics > 100,
        "must exercise real cubic outlines"
    );
}

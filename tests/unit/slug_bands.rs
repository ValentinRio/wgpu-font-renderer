use super::{build_slug_bands, BezierBuilder};
use super::{outline_glyph, parse_font, GlyphId, LocationRef, MetadataProvider, OutlinePen, Size};

fn check_bands(curves: &[f32]) {
    let data = build_slug_bands(curves);
    let segments = curves.as_chunks::<8>().0;
    assert_eq!(data.len() % 4, 0);
    assert_eq!(data[3], 0.);
    let values: Vec<_> = segments.iter().flat_map(|s| [s[1], s[3], s[5]]).collect();
    let minimum = values.iter().copied().fold(f32::INFINITY, f32::min);
    let maximum = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let (minimum, maximum) = if values.is_empty() {
        (0., 0.)
    } else {
        (minimum, maximum)
    };
    let count = (((maximum - minimum) / 64.).floor() as usize).clamp(1, 16);
    let size = ((maximum - minimum) / count as f32).max(1.);
    assert_eq!(&data[..3], &[minimum, size, count as f32]);
    let mut next = 4 * (1 + count);
    for band in 0..count {
        let descriptor = 4 + band * 4;
        let offset = data[descriptor] as usize;
        let length = data[descriptor + 1] as usize;
        assert_eq!(data[descriptor], offset as f32);
        assert_eq!(data[descriptor + 1], length as f32);
        assert_eq!(
            data[descriptor + 2].to_bits(),
            (minimum + (band + 1) as f32 * size + 0.05).to_bits()
        );
        assert_eq!(data[descriptor + 3], 0.);
        assert_eq!(
            offset, next,
            "lists must follow descriptors without overlap"
        );
        assert_eq!(offset % 4, 0);
        assert_eq!(length % 8, 0);
        assert!(offset + length <= data.len());
        let list = data[offset..offset + length].as_chunks::<8>().0;
        let mut occurrences = vec![0; segments.len()];
        for record in list {
            let index = record[6] as usize;
            assert!(index < segments.len());
            assert_eq!(record[6], index as f32);
            occurrences[index] += 1;
            assert_eq!(&record[..6], &segments[index][..6]);
            let key = segments[index][..6]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| p[0])
                .fold(f32::NEG_INFINITY, f32::max);
            assert_eq!(record[7], key);
        }
        for pair in list.windows(2) {
            assert!(
                pair[0][7] > pair[1][7] || (pair[0][7] == pair[1][7] && pair[0][6] < pair[1][6])
            );
        }
        let low = minimum + band as f32 * size;
        let high = minimum + (band + 1) as f32 * size;
        for (index, s) in segments.iter().enumerate() {
            let extent = [s[1], s[3], s[5]];
            let outside =
                extent.iter().all(|&v| v < low - 0.05) || extent.iter().all(|&v| v > high + 0.05);
            assert_eq!(
                occurrences[index],
                usize::from(!outside),
                "y band {band}, record {index}"
            );
        }
        next = offset + length;
    }
    assert_eq!(next, data.len());
}

#[test]
fn empty_degenerate_ties_and_epsilon_boundaries() {
    assert_eq!(build_slug_bands(&[]), [0., 1., 1., 0., 8., 0., 1.05, 0.],);
    check_bands(&[]);
    check_bands(&[2., -3., 2., -3., 2., -3., 9., 9.]);
    let mut curves = vec![
        0., 0., 0., 0., 0., 0., 9., 9., 128., 128., 128., 128., 128., 128., 9., 9.,
    ];
    for value in [63.949, 63.95, 64., 64.05, 64.051] {
        curves.extend([value, value, value, value, value, value, 7., 8.]);
        curves.extend([value, value, value, value, value, value, 7., 8.]);
    }
    check_bands(&curves);
}

#[test]
fn deterministic_random_outlines() {
    let mut state = 0x6a09e667u32;
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as f32 / u32::MAX as f32
    };
    for iteration in 0..200 {
        let extent = [0., 0.5, 63.9, 64., 128., 1024., 8192.][iteration % 7];
        let mut builder = BezierBuilder::new(0.);
        for _ in 0..1 + iteration % 4 {
            let x = random() * extent - extent / 2.;
            let y = random() * extent - extent / 2.;
            builder.move_to(x, y);
            for _ in 0..1 + iteration % 20 {
                let c = [random() * extent, random() * extent];
                let b = [random() * extent, random() * extent];
                builder.quad_to(c[0], c[1], b[0], b[1]);
            }
            builder.close();
        }
        builder.finish();
        check_bands(&builder.curves);
    }
}

#[test]
fn every_bundled_font_outline() {
    for bytes in [
        &include_bytes!("../../fonts/Roboto-Regular.ttf")[..],
        &include_bytes!("../../fonts/Cantarell-VF.otf")[..],
    ] {
        let face = parse_font(bytes, 0).unwrap().0;
        let mut outlined = 0;
        for id in 0..face
            .glyph_metrics(Size::unscaled(), LocationRef::default())
            .glyph_count()
        {
            let mut builder = BezierBuilder::new(0.);
            if outline_glyph(&face, GlyphId::new(id), &mut builder).is_some() {
                builder.finish();
                check_bands(&builder.curves);
                outlined += 1;
            }
        }
        assert!(outlined > 100);
    }
}

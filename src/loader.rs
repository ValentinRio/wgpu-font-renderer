use std::{collections::HashMap, fmt};
use owned_ttf_parser::{AsFaceRef, GlyphId, OutlineBuilder, OwnedFace, Rect};
use swash::{CacheKey, FontRef};

use crate::atlas::{allocation::Allocation, Atlas};

#[derive(Debug)]
/// Cached outline and placement metrics for one glyph, in font units.
pub struct Glyph {
    /// Eight floats per segment: three x/y pairs, then two zeros.
    /// Lines and quadratics store start, control, end; lines use control=end.
    /// Cubics are subdivided into quadratics within 0.25 font units, with a
    /// depth cap of 10 for pathological inputs (which may exceed that tolerance).
    /// Records chain end-to-start within each contour. CFF/CFF2 contours are
    /// explicitly closed; glyphs with negative total signed area are reversed
    /// to match TrueType winding for the shader.
    /// Coordinates use x rightward and y downward, flipped around `bbox.y_max`.
    pub curves: Vec<f32>,
    /// Atlas location and float-texel count for the encoded segments.
    pub allocation: Allocation,
    /// Original font-unit bounding box, with y increasing upward.
    pub bbox: Rect,
    /// Nonpositive bottom extent below the baseline, in font units.
    pub descent: i16,
    /// `ascender - max(bbox.y_min, 0) - bbox.height()`, in font units.
    /// When `y_min < 0`, this is `ascender - y_max + y_min`; the renderer adds
    /// `abs(descent)` back before scaling to pixels and adding paragraph y.
    pub y_offset: i16,
    /// Bounding-box left edge in font units, used by the shader.
    pub left_side_bearing: i16,
}

/// Owned font bytes, parsed face, and selected cached outlines.
pub struct Font {
    data: Vec<u8>,
    /// Parsed face; metrics and outlines use font units, y upward.
    pub face: OwnedFace,
    /// Byte offset of this face within the font data.
    pub offset: u32,
    /// Unique swash cache key identifying this loaded face.
    pub key: CacheKey,
    /// Outlines successfully cached for the requested characters, by glyph ID.
    pub glyph_cache: HashMap<GlyphId, Glyph>,
}

type Result<T> = std::result::Result<T, LoadingError>;

#[derive(Debug)]
/// Failure to read or parse a font. GPU errors are handled by wgpu instead.
pub enum LoadingError {
    /// The file could not be read, including permission and other I/O failures.
    FileNotFound,
    /// Invalid font bytes or a face index absent from the font collection.
    InvalidFile,
}

impl fmt::Display for LoadingError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            LoadingError::FileNotFound =>
                write!(f, "TTF Font file not found"),
            LoadingError::InvalidFile =>
                write!(f, "TTF Font file provided is invalid"),
        }
    }
}

impl Font {
    /// Read a font file, select a zero-based face index, and upload preset outlines.
    /// Returns `FileNotFound` for I/O errors and `InvalidFile` for parsing failures.
    /// Submit the encoder after loading; device, queue, and atlas must match.
    /// GPU failures follow wgpu validation. Filesystem access is unavailable on wasm.
    pub fn from_file(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        path: &str,
        index: usize,
        cache_preset: &str,
        atlas: &mut Atlas
    ) -> Result<Font> {
        // Read the font file as bytes
        let data = std::fs::read(path).or(Err(LoadingError::FileNotFound))?;
        Self::from_bytes(device, encoder, queue, data, index, cache_preset, atlas)
    }

    /// Parse owned font bytes at a zero-based face index and upload preset outlines.
    /// Variable fonts use the default variation instance.
    /// Returns `InvalidFile` for invalid data or an absent face. Unsupported
    /// characters and outlines without bounding boxes are skipped. A glyph is
    /// also silently skipped if `atlas.upload` returns `None`. Submit the encoder
    /// afterward; device, queue, and atlas must match. GPU failures follow wgpu validation.
    pub fn from_bytes(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        data: Vec<u8>,
        index: usize,
        cache_preset: &str,
        atlas: &mut Atlas
    ) -> Result<Font> {
        let (face, offset, key) = parse_font(&data, index)?;

        // Generate glyph cache for each glyph present in the font file
        let glyph_cache = create_glyph_cache(device, encoder, queue, &face, cache_preset, atlas);

        Ok(Self { data, face, offset, key, glyph_cache })
    }

    /// Borrow the original bytes as a swash font reference with the same cache key.
    pub fn as_ref(&self) -> FontRef<'_> {
        FontRef {
            data: &self.data,
            offset: self.offset,
            key: self.key,
        }
    }
}


// Both parsers must accept the same face; keep its offset/key from swash.
fn parse_font(data: &[u8], index: usize) -> Result<(OwnedFace, u32, CacheKey)> {
    let font = FontRef::from_index(data, index).ok_or(LoadingError::InvalidFile)?;
    let (offset, key) = (font.offset, font.key);
    let face = OwnedFace::from_vec(data.to_vec(), index as u32)
        .or(Err(LoadingError::InvalidFile))?;
    Ok((face, offset, key))
}


fn create_glyph_cache(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    queue: &wgpu::Queue,
    face: &OwnedFace,
    cache_preset: &str,
    atlas: &mut Atlas
) -> HashMap<GlyphId, Glyph> {
    let mut glyph_cache = HashMap::new();

    let face = face.as_face_ref();

    let ascender = face.ascender();

    for code_point in cache_preset.chars() { 
        if let Some(glyph_id) = face.glyph_index(code_point) {
            if let Some(bbox) = face.glyph_bounding_box(glyph_id) {
                let height = bbox.height();
                let left_side_bearing = bbox.x_min;
    
                let (descent, distance_from_baseline) = if bbox.y_min <= 0 {
                    (bbox.y_min, 0)
                } else {
                    (0, bbox.y_min)
                };
    
                let total_height = height + descent + distance_from_baseline;
                let y_offset = ascender - distance_from_baseline - height;
    
                let mut builder = BezierBuilder::new(total_height as f32);
    
                face.outline_glyph(glyph_id, &mut builder);
                // ttf-parser's CFF2 path omits close() for the final contour.
                builder.finish();

                // Each segment occupies eight R32Float texels; padding keeps every
                // segment aligned across atlas rows for the shader's eight-texel stride.
                let curves_count = builder.curves.len() as u32;
    
                let bytes = unsafe {
                    std::slice::from_raw_parts(builder.curves.as_ptr() as *const u8, builder.curves.len() * 4)
                };

                if let Some(allocation) = atlas.upload(curves_count, bytes, device, encoder, queue) {
                    let glyph = Glyph {
                        curves: builder.curves,
                        allocation,
                        bbox,
                        descent: descent,
                        y_offset: y_offset,
                        left_side_bearing,
                    };
        
                    glyph_cache.insert(glyph_id, glyph);
                }
            }
        }
    }

    glyph_cache
}

// Flip font y-up coordinates once during encoding so shader UVs can be y-down.
// Lines use control=end; cubics are subdivided into quadratic records.
// Contours are closed; glyph winding is normalized from geometry for the shader.
// The shader and atlas keep the same eight-float segment format.
struct BezierBuilder {
    last_position: [f32; 2],
    contour_position: [f32; 2],
    contour_start: usize,
    contours: Vec<usize>,
    pub curves: Vec<f32>,
    total_height: f32,
}

const CUBIC_TOLERANCE: f32 = 0.25;

impl BezierBuilder {
    pub fn new(total_height: f32) -> Self {
        Self {
            last_position: [0., 0.],
            contour_position: [0., 0.],
            contour_start: 0,
            contours: Vec::new(),
            curves: Vec::new(),
            total_height,
        }
    }

    fn signed_area(&self) -> f64 {
        self.curves
            .as_chunks::<8>()
            .0
            .iter()
            .map(|s| {
                let [ax, ay, bx, by, cx, cy, _, _] = s.map(f64::from);
                (ax * cy - ay * cx) / 6. + (ax * by - ay * bx + bx * cy - by * cx) / 3.
            })
            .sum()
    }

    fn finish(&mut self) {
        self.close();
        // Positive signed area in y-down coordinates is the shader's TrueType
        // convention. Decide from the entire glyph, independent of font tables.
        if self.signed_area() < 0. {
            for (i, &start) in self.contours.iter().enumerate() {
                let end = self
                    .contours
                    .get(i + 1)
                    .copied()
                    .unwrap_or(self.curves.len());
                let segments = self.curves[start..end].as_chunks_mut::<8>().0;
                segments.reverse();
                for segment in segments {
                    let line = segment[2..4] == segment[4..6];
                    segment.swap(0, 4);
                    segment.swap(1, 5);
                    if line {
                        // The shader's straight-line case requires control=end.
                        segment[2] = segment[4];
                        segment[3] = segment[5];
                    }
                }
            }
        }
    }

    // Accept unordered errors too: a NaN must not keep subdividing.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn cubic_to_quadratics(&mut self, p: [[f32; 2]; 4], depth: u8) {
        let control: [f32; 2] =
            std::array::from_fn(|i| (3. * p[1][i] + 3. * p[2][i] - p[0][i] - p[3][i]) / 4.);
        let d: [f32; 2] = std::array::from_fn(|i| -p[0][i] + 3. * p[1][i] - 3. * p[2][i] + p[3][i]);
        // Degree reduction has error D*t*(t-0.5)*(t-1), whose maximum
        // magnitude is |D|/(12*sqrt(3)). Use the conservative bound |D|/12.
        if !(d[0].hypot(d[1]) > 12. * CUBIC_TOLERANCE) || depth >= 10 {
            self.curves.extend_from_slice(&[
                p[0][0], p[0][1], control[0], control[1], p[3][0], p[3][1], 0., 0.,
            ]);
        } else {
            let midpoint = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.];
            let a = midpoint(p[0], p[1]);
            let b = midpoint(p[1], p[2]);
            let c = midpoint(p[2], p[3]);
            let ab = midpoint(a, b);
            let bc = midpoint(b, c);
            let middle = midpoint(ab, bc);
            self.cubic_to_quadratics([p[0], a, ab, middle], depth + 1);
            self.cubic_to_quadratics([middle, bc, c, p[3]], depth + 1);
        }
    }
}

impl OutlineBuilder for BezierBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.last_position = [x, self.total_height - y];
        self.contour_position = [x, y];
        self.contours.push(self.curves.len());
        self.contour_start = self.curves.len();
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let [x0, y0] = self.last_position;
        self.curves.extend_from_slice(&[
            x0,
            y0,
            x,
            self.total_height - y,
            x,
            self.total_height - y,
            0.,
            0.,
        ]);
        self.last_position = [x, self.total_height - y];
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let [x0, y0] = self.last_position;
        self.curves.extend_from_slice(&[
            x0,
            y0,
            x1,
            self.total_height - y1,
            x,
            self.total_height - y,
            0.,
            0.,
        ]);
        self.last_position = [x, self.total_height - y];
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.cubic_to_quadratics(
            [
                self.last_position,
                [x1, self.total_height - y1],
                [x2, self.total_height - y2],
                [x, self.total_height - y],
            ],
            0,
        );
        self.last_position = [x, self.total_height - y];
    }

    fn close(&mut self) {
        if self.contour_start < self.curves.len() {
            let [x, y] = self.contour_position;
            if self.last_position != [x, self.total_height - y] {
                // Use the original font-space start, avoiding a second y flip.
                self.line_to(x, y);
            }
            self.contour_start = self.curves.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubic_conversion_chains_and_stays_within_tolerance() {
        // An S curve exercises subdivision even though its midpoint lies on the chord.
        let p = [[10., 20.], [40., 200.], [160., -120.], [210., 30.]];
        let mut builder = BezierBuilder::new(0.);
        builder.move_to(p[0][0], -p[0][1]);
        builder.curve_to(p[1][0], -p[1][1], p[2][0], -p[2][1], p[3][0], -p[3][1]);
        let segments = builder.curves.as_chunks::<8>().0;
        assert!(segments.len() > 1);
        assert_eq!(segments[0][..2], p[0]);
        assert_eq!(segments.last().unwrap()[4..6], p[3]);
        for (i, segment) in segments.iter().enumerate() {
            if i > 0 {
                assert_eq!(segment[..2], segments[i - 1][4..6]);
            }
            assert_eq!(segment[6..], [0., 0.]);
            for sample in 0..=100 {
                let u = sample as f32 / 100.;
                // Midpoint subdivision scales D by 1/8 on either half, so this
                // cubic's accepted segments all cover equal parameter intervals.
                let t = (i as f32 + u) / segments.len() as f32;
                let mut error = [0.; 2];
                for axis in 0..2 {
                    let cubic = (1. - t).powi(3) * p[0][axis]
                        + 3. * (1. - t).powi(2) * t * p[1][axis]
                        + 3. * (1. - t) * t * t * p[2][axis]
                        + t.powi(3) * p[3][axis];
                    let quadratic = (1. - u).powi(2) * segment[axis]
                        + 2. * (1. - u) * u * segment[2 + axis]
                        + u * u * segment[4 + axis];
                    error[axis] = cubic - quadratic;
                }
                assert!(error[0].hypot(error[1]) <= CUBIC_TOLERANCE);
            }
        }
        assert_eq!(builder.last_position, p[3]);
    }

    #[test]
    fn winding_uses_geometry_even_for_implicitly_closed_outlines() {
        for reverse in [false, true] {
            let mut builder = BezierBuilder::new(10.);
            let points = if reverse {
                [[0., 0.], [4., 0.], [4., 4.], [0., 4.]]
            } else {
                [[0., 0.], [0., 4.], [4., 4.], [4., 0.]]
            };
            builder.move_to(points[0][0], points[0][1]);
            for p in &points[1..] {
                builder.line_to(p[0], p[1]);
            }
            builder.close();
            // The hole has opposite winding. It must stay a hole when the
            // decision is made for the entire glyph, not for each contour.
            let hole = if reverse {
                [[1., 1.], [1., 3.], [3., 3.], [3., 1.]]
            } else {
                [[1., 1.], [3., 1.], [3., 3.], [1., 3.]]
            };
            builder.move_to(hole[0][0], hole[0][1]);
            for p in &hole[1..] {
                builder.line_to(p[0], p[1]);
            }
            builder.close();
            let closed = builder.curves.clone();
            builder.finish();
            assert_eq!(builder.signed_area(), 12.);
            if !reverse {
                assert_eq!(builder.curves, closed);
            }
            let finished = builder.curves.clone();
            builder.finish();
            assert_eq!(builder.curves, finished, "must not double-flip");
        }
    }

    #[test]
    fn subdivision_stops_for_nan_and_at_depth_cap() {
        for c in [f32::NAN, 1e30] {
            let mut builder = BezierBuilder::new(0.);
            builder.move_to(0., 0.);
            builder.curve_to(c, 1., -c, 2., 3., 4.);
            assert!(!builder.curves.is_empty());
            assert!(builder.curves.len() / 8 <= 1024);
            if c.is_nan() {
                assert_eq!(builder.curves.len(), 8);
            }
        }
    }

    #[test]
    fn subdivision_accepts_at_depth_cap_despite_large_error() {
        let mut builder = BezierBuilder::new(0.);
        builder.cubic_to_quadratics([[0., 0.], [1e30, 1.], [-1e30, 2.], [3., 4.]], 10);
        assert_eq!(builder.curves.len(), 8);
    }

    #[test]
    fn closing_edge_uses_original_start_without_roundtrip() {
        let mut builder = BezierBuilder::new(1000.);
        // Exact closure is required even for fractional font coordinates.
        builder.move_to(2., 1000.1);
        builder.line_to(3., 4.);
        builder.close();
        assert_eq!(builder.curves[..2], builder.curves[12..14]);
        let curves = builder.curves.clone();
        builder.close();
        assert_eq!(builder.curves, curves);
    }

    #[test]
    fn roboto_bytes_produce_padded_y_down_outline() {
        let (face, _, _) = parse_font(include_bytes!("../examples/Roboto-Regular.ttf"), 0)
            .expect("bundled font parses");
        let face = face.as_face_ref();
        let id = face.glyph_index('g').unwrap();
        let bbox = face.glyph_bounding_box(id).unwrap();
        let height = bbox.y_max as f32;
        let mut builder = BezierBuilder::new(height);
        assert_eq!(face.outline_glyph(id, &mut builder), Some(bbox));
        assert!(!builder.curves.is_empty());
        assert_eq!(builder.curves.len() % 8, 0);
        // All three points must be in the flipped font bounding box; padding is zero.
        assert!(builder.curves.as_chunks::<8>().0.iter().all(|segment| {
            segment[6..] == [0., 0.]
                && segment[..6].as_chunks::<2>().0.iter().all(|p| {
                    p[0] >= bbox.x_min as f32
                        && p[0] <= bbox.x_max as f32
                        && p[1] >= 0.
                        && p[1] <= height - bbox.y_min as f32
                })
        }));
        assert!(builder
            .curves
            .as_chunks::<8>()
            .0
            .iter()
            .any(|s| s[2..4] != s[4..6]));
    }

    #[test]
    fn invalid_bytes_and_missing_face_return_loading_error() {
        for bytes in [
            &[][..],
            &b"not a font"[..],
            &include_bytes!("../examples/Roboto-Regular.ttf")[..32],
        ] {
            assert!(matches!(
                parse_font(bytes, 0),
                Err(LoadingError::InvalidFile)
            ));
        }
        assert!(matches!(
            parse_font(include_bytes!("../examples/Roboto-Regular.ttf"), 1),
            Err(LoadingError::InvalidFile)
        ));
    }

    #[test]
    fn line_and_quadratic_encoding_preserves_endpoints() {
        let mut builder = BezierBuilder::new(20.);
        builder.move_to(1., 2.);
        builder.line_to(3., 4.);
        builder.quad_to(5., 6., 7., 8.);
        assert_eq!(
            builder.curves,
            [1., 18., 3., 16., 3., 16., 0., 0., 3., 16., 5., 14., 7., 12., 0., 0.]
        );
    }
}

#[cfg(test)]
#[path = "../tests/unit/cff.rs"]
mod cff_tests;

use std::{collections::HashMap, fmt};
use owned_ttf_parser::{AsFaceRef, GlyphId, OutlineBuilder, OwnedFace, Rect};
use swash::{CacheKey, FontRef};

use crate::atlas::{allocation::Allocation, Atlas};

#[derive(Debug)]
/// Cached outline and placement metrics for one glyph, in font units.
pub struct Glyph {
    /// Eight floats per segment: three x/y pairs, then two zeros.
    /// Lines and quadratics store start, control, end; lines use control=end.
    /// Cubics incorrectly store control1, control2, end and drop the start, so
    /// cubic outlines (including CFF fonts) are not supported correctly yet.
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
    /// `abs(descent)` back before scaling to pixels.
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
// Lines are degenerate quadratics with control=end. Cubics are not supported
// correctly yet: curve_to stores c1, c2, end, dropping the start point, and the
// shader misinterprets those points as a quadratic. CFF fonts render incorrectly.
struct BezierBuilder {
    last_position: [f32; 2],
    pub curves: Vec<f32>,
    total_height: f32,
}

impl BezierBuilder {
    pub fn new(total_height: f32) -> Self {
        Self {
            last_position: [0., 0.],
            curves: Vec::new(),
            total_height,
        }
    }
}

impl OutlineBuilder for BezierBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.last_position = [x, self.total_height - y];
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let [x0, y0] = self.last_position;
        self.curves.extend_from_slice(&[x0, y0, x, self.total_height - y, x, self.total_height - y, 0., 0.]);
        self.last_position = [x, self.total_height - y];
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let [x0, y0] = self.last_position;
        self.curves.extend_from_slice(&[x0, y0, x1, self.total_height - y1, x, self.total_height - y, 0., 0.]);
        self.last_position = [x, self.total_height - y];
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.curves.extend_from_slice(&[x1, self.total_height - y1, x2, self.total_height - y2, x, self.total_height - y, 0., 0.]);
        self.last_position = [x, self.total_height - y];
    }

    fn close(&mut self) {
        
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

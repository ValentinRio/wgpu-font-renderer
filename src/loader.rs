use std::{collections::HashMap, fmt};
use owned_ttf_parser::{AsFaceRef, GlyphId, OutlineBuilder, OwnedFace, Rect};
use swash::{CacheKey, FontRef};

use crate::atlas::Atlas;

#[derive(Debug)]
pub struct AtlasSlot {
    position: [f32; 2], // Start position of the glyph data representation in the texture
    size: u32, // Number of curves that compose the glyph
    layer: u32, // Layer where the glyph is stored
}

#[derive(Debug)]
pub struct Glyph {
    curves: Vec<f32>,
    atlas_slot: AtlasSlot,
    bbox: Rect,
    descent: i16,
    y_offset: i16,
    left_side_bearing: i16,
}

pub struct Font {
    data: Vec<u8>,
    pub face: OwnedFace,
    pub offset: u32,
    pub key: CacheKey,
    pub glyph_cache: HashMap<GlyphId, Glyph>,
}

type Result<T> = std::result::Result<T, LoadingError>;

#[derive(Debug)]
pub enum LoadingError {
    FileNotFound,
    InvalidFile,
}

impl fmt::Display for LoadingError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            LoadingError::FileNotFound =>
                write!(f, "please use a vector with at least one element"),
            LoadingError::InvalidFile =>
                write!(f, "the provided string could not be parsed as int"),
        }
    }
}

impl Font {
    pub fn from_file(path: &str, index: usize) -> Result<Font> {
        // Read the font file as bytes
        let data = std::fs::read(path).or(Err(LoadingError::FileNotFound))?;
        // Create a temporary font reference for the font available in the file at `index`.
        // This will do some basic validation, compute the necessary offset
        // and generate a fresh cache key for us.
        let font = FontRef::from_index(&data, index).ok_or(LoadingError::InvalidFile)?;
        let (offset, key) = (font.offset, font.key);

        // Generate struct that hold TTF face tables
        let face = OwnedFace::from_vec(data.clone(), index as u32).or(Err(LoadingError::InvalidFile))?;

        // Generate glyph cache for each glyph present in the font file
        let glyph_cache = create_glyph_cache(&face);

        Ok(Self { data, face, offset, key, glyph_cache })
    }

    // Create the transient font reference to access swash features
    pub fn as_ref(&self) -> FontRef {
        FontRef {
            data: &self.data,
            offset: self.offset,
            key: self.key,
        }
    }
}


fn create_glyph_cache(face: &OwnedFace) -> HashMap<GlyphId, Glyph> {
    let mut glyph_cache = HashMap::new();

    let face = face.as_face_ref();

    let ascender = face.ascender();

    println!("{:#?}", face.number_of_glyphs());

    for id in 0..face.number_of_glyphs() {
        let glyph_id = GlyphId(id);

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

            let curves_count = builder.curves.len() as u32;

            let glyph = Glyph {
                curves: builder.curves,
                atlas_slot: AtlasSlot { position: [0., 0.], size: curves_count, layer: 0 },
                bbox,
                descent: descent,
                y_offset: y_offset,
                left_side_bearing,
            };
    
            println!("{:#?}", glyph);

            glyph_cache.insert(glyph_id, glyph);
        }
    }

    glyph_cache
}

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
        self.curves.extend_from_slice(&[x1, self.total_height - y1, x2, self.total_height -  y2, x, self.total_height - y, 0., 0.]);
        self.last_position = [x, self.total_height - y];
    }

    fn close(&mut self) {
        
    }
}
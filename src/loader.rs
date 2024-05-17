use std::{collections::HashMap, fmt};
use owned_ttf_parser::{GlyphId, OwnedFace, Rect};
use swash::{CacheKey, FontRef};

pub struct AtlasSlot {
    position: [f32; 2], // Start position of the glyph data representation in the texture
    size: u32, // Number of curves that compose the glyph
    layer: u32, // Layer where the glyph is stored
}

pub struct Glyph {
    curves: Vec<f32>,
    atlas_slot: AtlasSlot,
    bbox: Rect,
    descent: i16,
    glyph_y_offset: i16,
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

        // Generate glyph cache for each glyph present in the font file
        let glyph_cache = HashMap::new();

        // Generate struct that hold TTF face tables
        let face = OwnedFace::from_vec(data.clone(), index as u32).or(Err(LoadingError::InvalidFile))?;

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
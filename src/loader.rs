use std::collections::HashMap;

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

impl Font {
    pub fn from_file(path: &str, index: usize) -> Option<Self> {
        // Read the font file as bytes
        let data = std::fs::read(path).ok()?;
        // Create a temporary font reference for the font available in the file at `index`.
        // This will do some basic validation, compute the necessary offset
        // and generate a fresh cache key for us.
        let font = FontRef::from_index(&data, index)?;
        let (offset, key) = (font.offset, font.key);

        // Generate glyph cache for each glyph present in the font file
        let glyph_cache = HashMap::new();

        if let Ok(face) = OwnedFace::from_vec(data.clone(), index as u32) {
            Some(Self { data, face, offset, key, glyph_cache })
        } else {
            None
        }
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
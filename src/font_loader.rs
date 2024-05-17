use swash::{CacheKey, FontRef};

pub struct Font {
    data: Vec<u8>,
    offset: u32,
    key: CacheKey,
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
        // Return our struct with the original file data and copies of offset and key from the font reference
        Some(Self { data, offset, key })
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

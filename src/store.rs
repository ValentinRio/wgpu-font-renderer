use swash::CacheKey;
use wgpu::CommandEncoderDescriptor;

use std::collections::HashMap;

use crate::{atlas::Atlas, loader::Font, LoadingError};

/// Opaque handle to a font loaded into a particular store.
/// IDs from different stores are never equal. With an ID from another store,
/// [`crate::TextRenderer::prepare`] skips the paragraph and
/// [`crate::TypeWriter::shape_text`] returns `None`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FontId(CacheKey);

/// Owns loaded font data and the shared GPU outline atlas.
/// Later loads are picked up by the renderer on its next preparation.
pub struct FontStore {
    cache: HashMap<FontId, Font>,
    atlas: Atlas,
}

impl FontStore {
    /// Create an empty store and Rgba32Float atlas on `device`.
    /// wgpu validation errors occur if the device cannot support the atlas.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            cache: HashMap::new(),
            atlas: Atlas::new(device),
        }
    }

    /// Read face zero from a font file and cache outlines for `chars`.
    ///
    /// Returns [`LoadingError::Io`] for any file read failure, or
    /// [`LoadingError::InvalidFont`] for invalid font data. On wasm, use
    /// [`Self::load_from_bytes`] instead of filesystem access.
    /// Device and queue must belong together; GPU failures follow wgpu validation.
    pub fn load(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: impl AsRef<std::path::Path>,
        chars: &str
    ) -> Result<FontId, LoadingError>{

        let data = std::fs::read(path).map_err(LoadingError::Io)?;
        self.load_from_bytes(device, queue, &data, chars)
    }

    /// Load face zero from font bytes and upload outlines for `chars`.
    ///
    /// Returns a fresh font key, or [`LoadingError::InvalidFont`] for invalid data.
    /// Glyph zero is always cached if it has an outline, so missing characters
    /// draw `.notdef`. Glyphs without outlines still advance without drawing.
    /// Returns [`LoadingError::AtlasFull`] if an outline or its bands cannot fit
    /// one layer or allocation exceeds the device texture array layer limit.
    /// Each default layer uses 16 MiB (initially 32 MiB; 4 GiB at 256 layers).
    /// Growth temporarily retains both old and new textures. Previous
    /// fonts remain usable and the failed font is not added.
    /// Device and queue must belong together; GPU failures follow wgpu validation.
    /// Outline coordinates in the atlas are font units, not screen pixels.
    pub fn load_from_bytes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        chars: &str
    ) -> Result<FontId, LoadingError> {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: None });

        let font = Font::from_bytes(device, &mut encoder, queue, data.to_vec(), 0, chars, &mut self.atlas)?;

        queue.submit(Some(encoder.finish()));

        let cache_key = FontId(font.key);

        self.cache.insert(cache_key, font);

        Ok(cache_key)
    }

    #[cfg(feature = "unstable-internals")]
    pub(crate) fn pad_bands(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, floats: u32) {
        let mut encoder = device.create_command_encoder(&Default::default());
        self.atlas.upload_bands(floats, &vec![0; floats as usize * 4], device, &mut encoder, queue).expect("test padding fits");
        queue.submit(Some(encoder.finish()));
    }

    /// Borrow the shared GPU outline atlas for renderer construction.
    pub(crate) fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    /// Look up a loaded font by key; returns `None` for an unknown key.
    pub(crate) fn get(&self, font_key: FontId) -> Option<&Font> {
        self.cache.get(&font_key)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "../tests/unit/store_behaviour.rs"]
mod behaviour_tests;

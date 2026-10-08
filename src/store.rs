use swash::CacheKey;
use wgpu::{CommandEncoderDescriptor, SurfaceConfiguration};

use std::collections::HashMap;

use crate::{atlas::Atlas, loader::Font, LoadingError};

/// Owns loaded font data and the shared GPU outline atlas.
/// Load fonts before constructing a renderer, whose atlas binding is fixed.
pub struct FontStore {
    cache: HashMap<CacheKey, Font>,
    atlas: Atlas,
}

impl FontStore {
    /// Create an empty store on `device`; the surface configuration is reserved
    /// for compatibility and currently does not affect the R32Float atlas.
    /// wgpu validation errors occur if the device cannot support the atlas.
    pub fn new(device: &wgpu::Device, surface_config: &SurfaceConfiguration) -> Self {
        Self {
            cache: HashMap::new(),
            atlas: Atlas::new(device, surface_config),  
        }
    }

    /// Read face zero from a font file and cache outlines for `cache_preset`.
    ///
    /// Returns [`LoadingError::FileNotFound`] for any file read failure, or
    /// [`LoadingError::InvalidFile`] for invalid font data. On wasm, use
    /// [`Self::load_from_bytes`] instead of filesystem access.
    /// Device and queue must belong together; GPU failures follow wgpu validation.
    pub fn load(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font_file_path: &str,
        cache_preset: &str
    ) -> Result<CacheKey, LoadingError>{

        let data = std::fs::read(font_file_path).or(Err(LoadingError::FileNotFound))?;
        self.load_from_bytes(device, queue, &data, cache_preset)
    }

    /// Load face zero from font bytes and upload outlines for `cache_preset`.
    ///
    /// Returns a fresh font key, or [`LoadingError::InvalidFile`] for invalid data.
    /// Unsupported characters and glyphs without bounding boxes are not cached.
    /// Device and queue must belong together; GPU failures follow wgpu validation.
    /// Outline coordinates in the atlas are font units, not screen pixels.
    pub fn load_from_bytes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        cache_preset: &str
    ) -> Result<CacheKey, LoadingError> {
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: None });

        let font = Font::from_bytes(device, &mut encoder, queue, data.to_vec(), 0, cache_preset, &mut self.atlas)?;

        queue.submit(Some(encoder.finish()));

        let cache_key = font.key;

        self.cache.insert(cache_key, font);

        Ok(cache_key)
    }

    /// Borrow the shared GPU outline atlas for renderer construction.
    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    /// Look up a loaded font by key; returns `None` for an unknown key.
    pub fn get(&self, font_key: CacheKey) -> Option<&Font> {
        self.cache.get(&font_key)
    }
}

use swash::CacheKey;
use wgpu::SurfaceConfiguration;

use std::collections::HashMap;

use crate::{atlas::Atlas, loader::Font, LoadingError};

pub struct Store {
    cache: HashMap<CacheKey, Font>,
    atlas: Atlas,
}

impl Store {
    pub fn new(device: &wgpu::Device, surface_config: &SurfaceConfiguration) -> Self {
        Self {
            cache: HashMap::new(),
            atlas: Atlas::new(device, surface_config),  
        }
    }

    pub fn load(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        font_file_path: &str,
        cache_preset: &str
    ) -> Result<CacheKey, LoadingError>{
        let font = Font::from_file(device, encoder, queue, font_file_path, 0, cache_preset, &mut self.atlas)?;

        let cache_key = font.key.clone();

        self.cache.insert(cache_key, font);

        Ok(cache_key)
    }
}

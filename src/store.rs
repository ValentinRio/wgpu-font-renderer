use swash::CacheKey;
use wgpu::{CommandEncoderDescriptor, SurfaceConfiguration};

use std::collections::HashMap;

use crate::{atlas::Atlas, loader::Font, LoadingError};

pub struct FontStore {
    cache: HashMap<CacheKey, Font>,
    atlas: Atlas,
}

impl FontStore {
    pub fn new(device: &wgpu::Device, surface_config: &SurfaceConfiguration) -> Self {
        Self {
            cache: HashMap::new(),
            atlas: Atlas::new(device, surface_config),  
        }
    }

    pub fn load(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font_file_path: &str,
        cache_preset: &str
    ) -> Result<CacheKey, LoadingError>{

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: None });

        let font = Font::from_file(device, &mut encoder, queue, font_file_path, 0, cache_preset, &mut self.atlas)?;

        queue.submit(Some(encoder.finish()));

        let cache_key = font.key.clone();

        self.cache.insert(cache_key, font);

        Ok(cache_key)
    }

    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    pub fn get(&self, font_key: CacheKey) -> Option<&Font> {
        self.cache.get(&font_key)
    }
}

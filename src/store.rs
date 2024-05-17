use swash::CacheKey;

use std::collections::HashMap;

use crate::{atlas::Atlas, loader::Font, LoadingError};

pub struct Store {
    cache: HashMap<CacheKey, Font>,
    atlas: Atlas,
}

impl Store {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            atlas: Atlas::new(),  
        }
    }

    pub fn load(&mut self, font_file_path: &str) -> Result<CacheKey, LoadingError>{
        let font = Font::from_file(font_file_path, 0)?;

        println!("{:#?}", font.as_ref().features().fold(String::new(), |mut acc, feature| {
            acc.push_str(&format!("{}", feature.name().unwrap()));
            acc
        }));

        let cache_key = font.key.clone();

        self.cache.insert(cache_key, font);

        Ok(cache_key)
    }
}

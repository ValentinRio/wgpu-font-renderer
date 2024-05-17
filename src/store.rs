use swash::CacheKey;

use std::collections::HashMap;

use crate::{atlas::Atlas, loader::Font};

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

    pub fn load(&mut self, font_file_path: &str) {
        let font = Font::from_file(font_file_path, 0);
        if let Some(font) = font {
            println!("{:#?}", font.as_ref().features().fold(String::new(), |mut acc, feature| {
                acc.push_str(&format!("{}", feature.name().unwrap()));
                acc
            }));
            self.cache.insert(font.key, font);
        }
    }
}

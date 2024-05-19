use owned_ttf_parser::{AsFaceRef, GlyphId};
use swash::{shape::ShapeContext, text::Script, CacheKey};

use crate::{loader::Glyph, FontStore};

pub struct Paragraph {
    pub glyphs: Vec<(GlyphId, f32)>,
    pub position: [f32; 2],
    pub width: f32,
    pub size: u16,
    pub font_key: CacheKey,
    pub color: [f32; 4],
}

impl Paragraph {
    pub fn new(position: [f32; 2], size: u16, color: [f32; 4], font_key: CacheKey) -> Self {
        Self {
            glyphs: Vec::new(),
            position,
            width: 0.,
            size,
            font_key,
            color,
        }
    }

    pub fn append(&mut self, glyph_id: GlyphId, left: f32) {
        self.width += left;
        self.glyphs.push((glyph_id, left));
    }


}

pub struct TypeWriter {
    context: ShapeContext,
}

impl TypeWriter {

    pub fn new() -> Self {
        Self {
            context: ShapeContext::new()
        }
    }

    pub fn shape_text(&mut self, font_store: &FontStore, font_key: CacheKey, position: [f32; 2], size: u16, color: [f32; 4], text: &str) -> Option<Paragraph> {
        if let Some(font) = font_store.get(font_key) {
            let mut shaper = self.context.builder(font.as_ref())
                .script(Script::Latin)
                .size(size as f32)
                .build();

            let face = font.face.as_face_ref();

            let mut paragraph = Paragraph::new(position, size, color, font_key);

            shaper.add_str(text);
            shaper.shape_with(|cluster| {
                let glyph_id = GlyphId(cluster.glyphs[0].id);
                if let None = font.glyph_cache.get(&glyph_id) {
                    return;
                }

                paragraph.append(glyph_id, cluster.glyphs[0].advance)
            });

            Some(paragraph)
        } else {
            None
        }
    } 
}
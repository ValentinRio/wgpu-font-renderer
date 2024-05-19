use owned_ttf_parser::{AsFaceRef, GlyphId};
use swash::{shape::ShapeContext, text::Script, CacheKey};

use crate::{loader::Glyph, Store};

pub struct Paragraph {
    glyphs: Vec<(GlyphId, f32)>,
    position: [f32; 2],
    width: f32,
    size: u16,
    units_per_em: u16,
}

impl Paragraph {
    pub fn new(position: [f32; 2], size: u16, units_per_em: u16) -> Self {
        Self {
            glyphs: Vec::new(),
            position,
            width: 0.,
            size,
            units_per_em,
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

    pub fn shape_text(&mut self, font_store: Store, font_key: CacheKey, position: [f32; 2], size: u16, text: &str) -> Option<Paragraph> {
        if let Some(font) = font_store.get(font_key) {
            let mut shaper = self.context.builder(font.as_ref())
                .script(Script::Latin)
                .size(size as f32)
                .build();

            let face = font.face.as_face_ref();

            let mut paragraph = Paragraph::new(position, size, face.units_per_em());

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
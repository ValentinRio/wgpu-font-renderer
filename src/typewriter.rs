use owned_ttf_parser::GlyphId;
use swash::{shape::ShapeContext, text::Script, CacheKey};

use crate::FontStore;

/// A shaped run of Latin text; advances are in physical pixels.
///
/// Glyph IDs must belong to `font_key`; uncached outlines are skipped by the renderer.
pub struct Paragraph {
    /// Glyph IDs paired with horizontal advances in pixels, in shaping order.
    pub glyphs: Vec<(GlyphId, f32)>,
    /// Top-left text anchor in pixels, x rightward and y downward.
    pub position: [f32; 2],
    /// Sum of horizontal advances in pixels, including glyphs without outlines.
    pub width: f32,
    /// Font size in physical pixels per em.
    pub size: u16,
    /// Key of the font used to shape this run.
    pub font_key: CacheKey,
    /// RGBA color components, normally in 0..=1; the shader currently ignores alpha.
    pub color: [f32; 4],
}

impl Paragraph {
    /// Create an empty run at a pixel anchor with a pixel-per-em size.
    /// Color is RGBA; no input validation is performed.
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

    /// Append a glyph and add its horizontal pixel advance to the run width.
    /// The glyph must belong to this run's font; no validation is performed.
    pub fn append(&mut self, glyph_id: GlyphId, left: f32) {
        self.width += left;
        self.glyphs.push((glyph_id, left));
    }


}

/// Reusable shaping context for Latin text.
pub struct TypeWriter {
    context: ShapeContext,
}

impl TypeWriter {

    /// Create an empty shaping context.
    pub fn new() -> Self {
        Self {
            context: ShapeContext::new()
        }
    }

    /// Shape Latin text at a top-left pixel anchor, with size in pixels per em.
    ///
    /// Returns `None` for an unknown font key. Keeps only the first glyph of each
    /// shaping cluster, ignores cluster offsets, and does not implement line wrapping.
    /// Color is RGBA (the renderer ignores alpha). Missing cached outlines still
    /// advance the pen. Panics if the shaper emits an empty glyph cluster.
    pub fn shape_text(&mut self, font_store: &FontStore, font_key: CacheKey, position: [f32; 2], size: u16, color: [f32; 4], text: &str) -> Option<Paragraph> {
        if let Some(font) = font_store.get(font_key) {
            let mut shaper = self.context.builder(font.as_ref())
                .script(Script::Latin)
                .size(size as f32)
                .build();


            let mut paragraph = Paragraph::new(position, size, color, font_key);

            shaper.add_str(text);
            shaper.shape_with(|cluster| {
                let glyph_id = GlyphId(cluster.glyphs[0].id);
                paragraph.append(glyph_id, cluster.glyphs[0].advance)
            });

            Some(paragraph)
        } else {
            None
        }
    } 
}
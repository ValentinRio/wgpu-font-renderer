use swash::{shape::ShapeContext, text::Script};

use crate::{FontId, FontStore};

/// Glyph index within a font.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct GlyphId(
    /// The font’s glyph index.
    pub u16,
);

/// A shaped run of Latin text; advances are in physical pixels.
///
/// Glyph IDs must belong to the run’s font; uncached outlines are skipped by the renderer.
#[derive(Debug, Clone)]
pub struct Paragraph {
    /// Glyph IDs paired with horizontal advances in pixels, in shaping order.
    glyphs: Vec<(GlyphId, f32)>,
    /// Top-left text anchor in pixels, x rightward and y downward.
    position: [f32; 2],
    /// Sum of horizontal advances in pixels, including glyphs without outlines.
    width: f32,
    /// Font size in physical pixels per em.
    size: f32,
    /// Key of the font used to shape this run.
    font_key: FontId,
    /// RGBA color components, normally in 0..=1. LCD scales coverage by alpha;
    /// grayscale (including the feature fallback) ignores alpha.
    color: [f32; 4],
}

impl Paragraph {
    /// Create an empty run at a pixel anchor with a pixel-per-em size.
    /// Color is RGBA; no input validation is performed.
    pub fn new(font: FontId, position: [f32; 2], size: f32, color: [f32; 4]) -> Self {
        Self {
            glyphs: Vec::new(),
            position,
            width: 0.,
            size,
            font_key: font,
            color,
        }
    }

    /// Append a glyph and add its horizontal pixel advance to the run width.
    /// The glyph must belong to this run's font; no validation is performed.
    pub fn push(&mut self, glyph: GlyphId, advance: f32) {
        self.width += advance;
        self.glyphs.push((glyph, advance));
    }
    /// Font used by this run.
    pub fn font(&self) -> FontId { self.font_key }
    /// Top-left anchor in physical pixels.
    pub fn position(&self) -> [f32; 2] { self.position }
    /// Font size in pixels per em.
    pub fn size(&self) -> f32 { self.size }
    /// RGBA color.
    pub fn color(&self) -> [f32; 4] { self.color }
    /// Sum of horizontal advances in physical pixels.
    pub fn width(&self) -> f32 { self.width }
    /// Glyph indices and horizontal pixel advances in shaping order.
    pub fn glyphs(&self) -> &[(GlyphId, f32)] { &self.glyphs }
    /// Set the top-left anchor in physical pixels.
    pub fn set_position(&mut self, position: [f32; 2]) { self.position = position; }
    /// Set the RGBA color.
    pub fn set_color(&mut self, color: [f32; 4]) { self.color = color; }
}


/// Reusable shaping context for Latin text.
pub struct TypeWriter {
    context: ShapeContext,
}

impl Default for TypeWriter {
    fn default() -> Self {
        Self::new()
    }
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
    /// Returns `None` for an unknown font or a size that is not finite or is <= 0.0.
    /// Keeps only the first glyph of each shaping cluster, ignores cluster offsets,
    /// and does not implement line wrapping.
    /// Color is RGBA; LCD scales coverage by alpha, while grayscale ignores it.
    /// Missing cached outlines still advance the pen. Empty glyph clusters are skipped.
    pub fn shape_text(&mut self, store: &FontStore, font: FontId, position: [f32; 2], size: f32, color: [f32; 4], text: &str) -> Option<Paragraph> {
        if !size.is_finite() || size <= 0.0 {
            return None;
        }
        let mut paragraph = Paragraph::new(font, position, size, color);
        if let Some(font) = store.get(font) {
            for (id, advance) in self.shape_glyphs(font.as_ref(), size, text) {
                paragraph.push(id, advance);
            }

            Some(paragraph)
        } else {
            None
        }
    } 

    pub(crate) fn shape_glyphs(&mut self, font: swash::FontRef<'_>, size: f32, text: &str) -> Vec<(GlyphId, f32)> {
        let mut glyphs = Vec::new();
        let mut shaper = self.context.builder(font)
            .script(Script::Latin)
            .size(size)
            .build();
        shaper.add_str(text);
        shaper.shape_with(|cluster| {
            if let Some(glyph) = cluster.glyphs.first() {
                glyphs.push((GlyphId(glyph.id), glyph.advance));
            }
        });
        glyphs
    }

}
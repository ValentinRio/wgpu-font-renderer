use swash::{
    shape::ShapeContext,
    text::{analyze, Script},
};

use crate::{FontId, FontStore};

/// Glyph index within a font.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct GlyphId(
    /// The font’s glyph index.
    pub u16,
);

/// One glyph and its positioning in physical pixels.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    /// Glyph index within the paragraph's font.
    pub glyph: GlyphId,
    /// Horizontal pen advance in physical pixels.
    pub advance: f32,
    /// Offset from the pen in physical pixels, x rightward and y downward.
    pub offset: [f32; 2],
}

/// Shaped text in logical run order; advances are in physical pixels.
///
/// Glyph IDs must belong to the run’s font; uncached outlines are skipped by the renderer.
#[derive(Debug, Clone)]
pub struct Paragraph {
    /// Glyphs with advances and offsets in pixels, in shaping order.
    glyphs: Vec<ShapedGlyph>,
    /// Top-left text anchor in pixels, x rightward and y downward.
    position: [f32; 2],
    /// Sum of horizontal advances in pixels, including glyphs without outlines.
    width: f32,
    /// Font size in physical pixels per em.
    size: f32,
    /// Key of the font used to shape this run.
    font_key: FontId,
    /// RGBA color components, normally in 0..=1. Both LCD and
    /// grayscale (including the feature fallback) scale coverage by alpha.
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

    /// Append a glyph with zero offset and add its pixel advance to the run width.
    /// The glyph must belong to this run's font; no validation is performed.
    pub fn push(&mut self, glyph: GlyphId, advance: f32) {
        self.push_at(glyph, advance, [0.; 2]);
    }
    /// Append a glyph with an offset from the pen, x rightward and y downward.
    /// The advance contributes to width; the offset does not.
    pub fn push_at(&mut self, glyph: GlyphId, advance: f32, offset: [f32; 2]) {
        self.width += advance;
        self.glyphs.push(ShapedGlyph {
            glyph,
            advance,
            offset,
        });
    }
    /// Font used by this run.
    pub fn font(&self) -> FontId {
        self.font_key
    }
    /// Top-left anchor in physical pixels.
    pub fn position(&self) -> [f32; 2] {
        self.position
    }
    /// Font size in pixels per em.
    pub fn size(&self) -> f32 {
        self.size
    }
    /// RGBA color.
    pub fn color(&self) -> [f32; 4] {
        self.color
    }
    /// Sum of horizontal advances in physical pixels.
    pub fn width(&self) -> f32 {
        self.width
    }
    /// Glyphs, horizontal advances and offsets in shaping order.
    pub fn glyphs(&self) -> &[ShapedGlyph] {
        &self.glyphs
    }
    /// Set the top-left anchor in physical pixels.
    pub fn set_position(&mut self, position: [f32; 2]) {
        self.position = position;
    }
    /// Set the RGBA color.
    pub fn set_color(&mut self, color: [f32; 4]) {
        self.color = color;
    }
}

/// Reusable shaping context for text in logical script-run order.
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
            context: ShapeContext::new(),
        }
    }

    /// Shape text at a top-left pixel anchor, with size in pixels per em.
    ///
    /// Returns `None` for an unknown font or a size that is not finite or is <= 0.0.
    /// Retains every glyph and offset, shaping each script run independently.
    /// Common and inherited characters join the preceding run, or the following
    /// run at the start. Runs stay in logical order; bidi and line wrapping are
    /// not implemented. Text with no strong script is shaped as Latin.
    /// Color is RGBA; both LCD and grayscale scale coverage by alpha.
    /// Missing cached outlines still advance the pen. Empty glyph clusters are skipped.
    pub fn shape_text(
        &mut self,
        store: &FontStore,
        font: FontId,
        position: [f32; 2],
        size: f32,
        color: [f32; 4],
        text: &str,
    ) -> Option<Paragraph> {
        if !size.is_finite() || size <= 0.0 {
            return None;
        }
        let mut paragraph = Paragraph::new(font, position, size, color);
        if let Some(font) = store.get(font) {
            for glyph in self.shape_glyphs_at(font.as_ref(), font.location.coords().iter().map(|c| c.to_bits()), size, text) {
                paragraph.push_at(glyph.glyph, glyph.advance, glyph.offset);
            }

            Some(paragraph)
        } else {
            None
        }
    }

    #[cfg(any(test, feature = "unstable-internals"))]
    pub(crate) fn shape_glyphs(
        &mut self,
        font: swash::FontRef<'_>,
        size: f32,
        text: &str,
    ) -> Vec<ShapedGlyph> {
        self.shape_glyphs_at(font, std::iter::empty(), size, text)
    }

    fn shape_glyphs_at(&mut self, font: swash::FontRef<'_>,
        coords: impl Iterator<Item = i16> + Clone, size: f32, text: &str) -> Vec<ShapedGlyph> {
        let mut glyphs = Vec::new();
        for (run, script) in script_runs(text) {
            let mut shaper = self.context.builder(font).script(script).size(size).normalized_coords(coords.clone()).build();
            shaper.add_str(run);
            shaper.shape_with(|cluster| {
                glyphs.extend(cluster.glyphs.iter().map(|glyph| ShapedGlyph {
                    glyph: GlyphId(glyph.id),
                    advance: glyph.advance,
                    offset: [glyph.x, -glyph.y],
                }));
            });
        }
        glyphs
    }
}
fn script_runs(text: &str) -> Vec<(&str, Script)> {
    let strong = |script| !matches!(script, Script::Common | Script::Inherited | Script::Unknown);
    let initial = analyze(text.chars())
        .map(|(p, _)| p.script())
        .find(|&s| strong(s))
        .unwrap_or(Script::Latin);
    let mut script = initial;
    let mut start = 0;
    let mut runs = Vec::new();
    for ((index, _), (properties, _)) in text.char_indices().zip(analyze(text.chars())) {
        let next = properties.script();
        if strong(next) && next != script {
            runs.push((&text[start..index], script));
            start = index;
            script = next;
        }
    }
    if start < text.len() {
        runs.push((&text[start..], script));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_keep_common_and_inherited_with_neighbours() {
        assert_eq!(
            script_runs(" (Hi\u{301}) 直接从轮廓渲染字形文本!"),
            vec![
                (" (Hi\u{301}) ", Script::Latin),
                ("直接从轮廓渲染字形文本!", Script::Han)
            ]
        );
        assert_eq!(script_runs(" 直接!"), vec![(" 直接!", Script::Han)]);
        assert!(script_runs("").is_empty());
    }

    #[test]
    fn unknown_script_is_neutral() {
        assert_eq!(script_runs("\u{e000}\u{0378}"), vec![("\u{e000}\u{0378}", Script::Latin)]);
        assert_eq!(script_runs("\u{e000}直接\u{0378}!"), vec![("\u{e000}直接\u{0378}!", Script::Han)]);
        assert_eq!(script_runs("Hi\u{e000}直接\u{0378}"),
            vec![("Hi\u{e000}", Script::Latin), ("直接\u{0378}", Script::Han)]);
    }

    #[test]
    fn common_only_text_matches_latin_shaping() {
        for bytes in [
            &include_bytes!("../fonts/Roboto-Regular.ttf")[..],
            &include_bytes!("../fonts/Cantarell-VF.otf")[..],
        ] {
            let font = swash::FontRef::from_index(bytes, 0).unwrap();
            for text in ["2026", "(1)", "  !?., ()  "] {
                assert_eq!(script_runs(text), vec![(text, Script::Latin)]);
                let mut context = ShapeContext::new();
                let mut shaper = context.builder(font).script(Script::Latin).size(32.).build();
                shaper.add_str(text);
                let mut expected = Vec::new();
                shaper.shape_with(|cluster| expected.extend(cluster.glyphs.iter()
                    .map(|g| (GlyphId(g.id), g.advance))));
                let actual: Vec<_> = TypeWriter::new().shape_glyphs(font, 32., text).iter()
                    .map(|g| (g.glyph, g.advance)).collect();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn latin_ids_advances_and_all_cluster_offsets_match_swash() {
        let font =
            swash::FontRef::from_index(include_bytes!("../fonts/Roboto-Regular.ttf"), 0).unwrap();
        for text in ["Hi Hi", "AVToWAàÅgß", "HiB@", "e\u{301}"] {
            let mut context = ShapeContext::new();
            let mut shaper = context
                .builder(font)
                .script(Script::Latin)
                .size(32.)
                .build();
            shaper.add_str(text);
            let mut expected = Vec::new();
            shaper.shape_with(|cluster| {
                expected.extend(cluster.glyphs.iter().map(|g| ShapedGlyph {
                    glyph: GlyphId(g.id),
                    advance: g.advance,
                    offset: [g.x, -g.y],
                }));
            });
            assert_eq!(TypeWriter::new().shape_glyphs(font, 32., text), expected);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod mark_test {
    use super::*;

    #[test]
    fn decomposed_sequence_keeps_both_glyphs_on_font_with_combining_mark() {
        let bytes = crate::atlas_tests::common::decomposed_font();
        let font = swash::FontRef::from_index(&bytes, 0).unwrap();
        assert_ne!(font.charmap().map('e'), 0);
        assert_ne!(font.charmap().map('\u{301}'), 0);
        let glyphs = TypeWriter::new().shape_glyphs(font, 32., "e\u{301}");
        let mut context = ShapeContext::new();
        let mut shaper = context
            .builder(font)
            .script(Script::Latin)
            .size(32.)
            .build();
        shaper.add_str("e\u{301}");
        let mut expected = Vec::new();
        shaper.shape_with(|cluster| {
            expected.extend(cluster.glyphs.iter().map(|g| ShapedGlyph {
                glyph: GlyphId(g.id),
                advance: g.advance,
                offset: [g.x, -g.y],
            }))
        });
        assert_eq!(glyphs, expected);
        assert_eq!(glyphs.len(), 2);
        assert_ne!(glyphs[1].offset, [0.; 2]);
    }
}

use swash::{
    shape::ShapeContext,
    text::{analyze, cluster::{Boundary, CharCluster, Parser, Token}, Codepoint, LineBreak, Script},
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
    /// Horizontal width in pixels, excluding hanging spaces from layout.
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
    /// Horizontal width in physical pixels. Layout excludes hanging breakable spaces
    /// at every line end; otherwise this is the sum of glyph advances.
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

/// Horizontal alignment inside a text block's alignment box.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default)]
pub enum Align {
    /// Align to the left edge.
    #[default]
    Left,
    /// Centre each line.
    Center,
    /// Align to the right edge.
    Right,
}

/// Distance between paragraph tops, in physical pixels.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default)]
pub enum LineHeight {
    /// Use ascent + descent + line gap, falling back to one em if invalid.
    #[default]
    Font,
    /// Multiply the font line height (including its fallback) by this factor.
    Scale(f32),
    /// Use an explicit pixel distance.
    Pixels(f32),
}

/// Greedy Unicode line wrapping and alignment options.
///
/// Defaults to no maximum width, left alignment and the font line height.
#[non_exhaustive]
#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    max_width: Option<f32>,
    align: Align,
    line_height: LineHeight,
}

impl TextLayout {
    /// Set the alignment box width in pixels; zero breaks at every opportunity.
    pub fn with_max_width(mut self, width: f32) -> Self {
        self.max_width = Some(width);
        self
    }
    /// Set horizontal alignment.
    pub fn with_align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
    /// Set the distance between line tops.
    pub fn with_line_height(mut self, height: LineHeight) -> Self {
        self.line_height = height;
        self
    }
}

/// Laid-out paragraphs that move and change colour together.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct TextBlock {
    lines: Vec<Paragraph>,
    offsets: Vec<[f32; 2]>,
    position: [f32; 2],
    size: [f32; 2],
}

impl TextBlock {
    /// Read the lines in logical order.
    pub fn lines(&self) -> &[Paragraph] {
        &self.lines
    }
    /// Place each line at the block anchor plus its original layout offset.
    pub fn set_position(&mut self, position: [f32; 2]) {
        for (line, offset) in self.lines.iter_mut().zip(&self.offsets) {
            line.position = [position[0] + offset[0], position[1] + offset[1]];
        }
        self.position = position;
    }
    /// Set the RGBA colour of every line.
    pub fn set_color(&mut self, color: [f32; 4]) {
        for line in &mut self.lines {
            line.set_color(color);
        }
    }
    /// Top-left anchor of the alignment box in physical pixels.
    pub fn position(&self) -> [f32; 2] {
        self.position
    }
    /// Alignment box width and line count times line height, in physical pixels.
    pub fn size(&self) -> [f32; 2] {
        self.size
    }
    /// Number of lines, including empty lines from hard breaks.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

impl<'a> IntoIterator for &'a TextBlock {
    type Item = &'a Paragraph;
    type IntoIter = std::slice::Iter<'a, Paragraph>;
    fn into_iter(self) -> Self::IntoIter {
        self.lines.iter()
    }
}

struct ShapedCluster {
    source: std::ops::Range<usize>,
    glyphs: Vec<ShapedGlyph>,
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
    /// Retains glyphs and offsets, shaping each script run independently.
    /// Output glyph zero (`.notdef`) is omitted when its source is a Cc control
    /// or default-ignorable character, contributing no advance. Every other
    /// output stays as swash shaped it, including visible substitutions and
    /// ligatures originating at those characters. Other missing characters
    /// still draw `.notdef` if it has an outline. Mapped Cc controls enter ordinary
    /// glyph shaping so swash emits their font glyphs instead of hiding them.
    /// Swash also hides default-ignorables mapped to visible glyphs (for example
    /// Roboto's soft hyphen), as HarfBuzz does by default. This existing behaviour
    /// is retained from 0.2.
    /// Common and inherited characters join the preceding run, or the following
    /// run at the start. Runs stay in logical order without bidi reordering.
    /// This method stays single-line; use [`Self::layout_text`] for wrapping.
    /// Text with no strong script is shaped as Latin.
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

    /// Shape each hard-break-delimited paragraph once, then wrap greedily
    /// at Unicode line-break opportunities.
    ///
    /// Breaks fall between clusters; words wider than the box overflow intact.
    /// Hard breaks draw nothing, CRLF counts as one break, and a final hard
    /// break adds no extra line. Trailing breakable spaces hang at every line
    /// end: their glyphs remain, but contribute no width or alignment offset.
    /// No-break spaces retain their width. Leading spaces stay with the first
    /// content on their line and count toward its width. Reordered source spans
    /// stay intact. Hard-break characters are never passed to the shaper.
    /// Without a maximum width the box is as wide as its widest line.
    /// Baselines use the same font ascent as [`Self::shape_text`]. Bidi
    /// reordering is not performed. Empty text produces zero lines.
    ///
    /// Returns `None` in the same cases as [`Self::shape_text`], or for a
    /// non-finite or negative maximum width or a non-finite/nonpositive
    /// `Scale` or `Pixels` line height, or a non-finite origin, line width or block size.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_text(
        &mut self,
        store: &FontStore,
        font: FontId,
        position: [f32; 2],
        size: f32,
        color: [f32; 4],
        text: &str,
        layout: &TextLayout,
    ) -> Option<TextBlock> {
        let metrics = store.metrics(font, size)?;
        if position.iter().any(|value| !value.is_finite())
            || layout.max_width.is_some_and(|w| !w.is_finite() || w < 0.)
        {
            return None;
        }
        let font_height = metrics.ascent + metrics.descent + metrics.line_gap;
        let font_height = if font_height.is_finite() && font_height > 0. {
            font_height
        } else {
            size
        };
        let line_height = match layout.line_height {
            LineHeight::Font => font_height,
            LineHeight::Scale(k) if k.is_finite() && k > 0. => font_height * k,
            LineHeight::Pixels(p) if p.is_finite() && p > 0. => p,
            _ => return None,
        };
        if !line_height.is_finite() {
            return None;
        }
        let instance = store.get(font)?;
        let mut lines = Vec::new();
        for (source, hard) in hard_sections(text) {
            let piece = &text[source];
            if piece.is_empty() {
                if hard {
                    lines.push(Paragraph::new(font, position, size, color));
                }
                continue;
            }
            let clusters = self.shape_clusters_at(
                instance.as_ref(),
                instance.location.coords().iter().map(|c| c.to_bits()),
                size,
                piece,
            );
            lines.extend(
                break_lines(piece, &clusters, layout.max_width)
                    .into_iter()
                    .map(|line| {
                        let mut paragraph = Paragraph::new(font, position, size, color);
                        paragraph.glyphs = line.glyphs;
                        paragraph.width = line.width;
                        paragraph
                    }),
            );
        }
        if lines.iter().any(|line| !line.width.is_finite()) {
            return None;
        }
        let width = layout
            .max_width
            .unwrap_or_else(|| lines.iter().map(Paragraph::width).fold(0., f32::max));
        let mut offsets = Vec::with_capacity(lines.len());
        for (n, line) in lines.iter_mut().enumerate() {
            let shift = match layout.align {
                Align::Left => 0.,
                Align::Center => (width - line.width) / 2.,
                Align::Right => width - line.width,
            };
            let offset = [shift, n as f32 * line_height];
            line.position = [position[0] + offset[0], position[1] + offset[1]];
            if line.position.iter().any(|value| !value.is_finite()) {
                return None;
            }
            offsets.push(offset);
        }
        let height = lines.len() as f32 * line_height;
        if !width.is_finite() || !height.is_finite() {
            return None;
        }
        Some(TextBlock {
            lines,
            offsets,
            position,
            size: [width, height],
        })
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

    fn shape_glyphs_at(
        &mut self,
        font: swash::FontRef<'_>,
        coords: impl Iterator<Item = i16> + Clone,
        size: f32,
        text: &str,
    ) -> Vec<ShapedGlyph> {
        let mut glyphs = Vec::new();
        self.shape_with_at(font, coords, size, text, |_, cluster| {
            glyphs.extend(shaped_glyphs(cluster.glyphs));
        });
        glyphs
    }

    fn shape_clusters_at(
        &mut self,
        font: swash::FontRef<'_>,
        coords: impl Iterator<Item = i16> + Clone,
        size: f32,
        text: &str,
    ) -> Vec<ShapedCluster> {
        let mut clusters = Vec::new();
        self.shape_with_at(font, coords, size, text, |run_offset, cluster| {
            clusters.push(ShapedCluster {
                source: run_offset + cluster.source.start as usize
                    ..run_offset + cluster.source.end as usize,
                glyphs: shaped_glyphs(cluster.glyphs).collect(),
            });
        });
        clusters
    }

    fn shape_with_at(
        &mut self,
        font: swash::FontRef<'_>,
        coords: impl Iterator<Item = i16> + Clone,
        size: f32,
        text: &str,
        mut emit: impl FnMut(usize, &swash::shape::cluster::GlyphCluster<'_>),
    ) {
        for (run, script) in script_runs(text) {
            let mut shaper = self
                .context
                .builder(font)
                .script(script)
                .size(size)
                .normalized_coords(coords.clone())
                .build();
            // Carry the source category through normalization and substitution.
            // Only .notdef is hidden: a ligature may inherit this marker and be visible.
            let charmap = font.charmap();
            let mut cluster = CharCluster::new();
            let mut parser = Parser::new(
                script,
                run.char_indices().map(|(offset, ch)| Token {
                    ch,
                    offset: offset as u32,
                    len: ch.len_utf8() as u8,
                    // Swash otherwise discards mapped Cc before emitting any glyph.
                    // Give them ordinary properties while keeping the actual cmap character.
                    info: if ch.is_control() && charmap.map(ch) != 0 {
                        ' '.into()
                    } else {
                        ch.into()
                    },
                    data: u32::from(ch.is_control() || is_default_ignorable(ch)),
                }),
            );
            while parser.next(&mut cluster) {
                cluster.map(|ch| charmap.map(ch));
                shaper.add_cluster(&cluster);
            }
            let run_offset = run.as_ptr() as usize - text.as_ptr() as usize;
            shaper.shape_with(|cluster| emit(run_offset, cluster));
        }
    }
}

#[derive(Default)]
struct LayoutLine {
    glyphs: Vec<ShapedGlyph>,
    width: f32,
}

fn hard_sections(text: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    let mut sections = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if is_hard_break(ch) {
            sections.push((start..offset, true));
            start = offset + ch.len_utf8();
            if ch == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                let (i, c) = chars.next().unwrap();
                start = i + c.len_utf8();
            }
        }
    }
    sections.push((start..text.len(), false));
    sections
}

fn break_lines(text: &str, clusters: &[ShapedCluster], max_width: Option<f32>) -> Vec<LayoutLine> {
    let mut opportunities = Vec::new();
    let sections = hard_sections(text);
    for ((offset, ch), (_, boundary)) in text.char_indices().zip(analyze(text.chars())) {
        if !is_hard_break(ch) && matches!(boundary, Boundary::Line | Boundary::Mandatory) {
            opportunities.push(offset);
        }
    }

    // Source order is used only for membership. Glyph order remains the
    // shaper's order, including reordering within Indic/Myanmar syllables.
    let mut by_source: Vec<_> = (0..clusters.len()).collect();
    by_source.sort_unstable_by_key(|&i| (clusters[i].source.start, i));
    let mut lines = Vec::new();
    for (section, hard) in sections {
        let first = by_source.partition_point(|&i| clusters[i].source.start < section.start);
        let last = by_source.partition_point(|&i| clusters[i].source.start < section.end);
        let mut members: Vec<_> = by_source[first..last]
            .iter()
            .copied()
            // Hard-break output is excluded by source containment, regardless
            // of its glyph ID or its position in the shaper's output.
            .filter(|&i| clusters[i].source.end <= section.end)
            .collect();
        members.sort_unstable();
        let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
        for &i in &members {
            let mut span = clusters[i].source.clone();
            // Coalesce overlaps and inversions into an unbreakable source span.
            // Merging backwards also covers non-adjacent inversions.
            while spans
                .last()
                .is_some_and(|previous| previous.end > span.start)
            {
                let previous = spans.pop().unwrap();
                span.start = span.start.min(previous.start);
                span.end = span.end.max(previous.end);
            }
            spans.push(span);
        }
        let first = opportunities.partition_point(|&offset| offset <= section.start);
        let last = opportunities.partition_point(|&offset| offset < section.end);
        let ends = opportunities[first..last]
            .iter()
            .copied()
            .filter(|&offset| {
                let i = spans.partition_point(|span| span.end <= offset);
                !spans.get(i).is_some_and(|span| span.start < offset)
            })
            .chain(std::iter::once(section.end));
        let mut line = LayoutLine::default();
        let mut raw_width = 0.;
        let mut has_unit = false;
        let mut line_has_content = false;
        let mut unit_start = section.start;
        for end in ends {
            let trimmed_end = unit_start
                + text[unit_start..end]
                    .trim_end_matches(is_hanging_space)
                    .len();
            let first = by_source.partition_point(|&i| clusters[i].source.start < unit_start);
            let last = by_source.partition_point(|&i| clusters[i].source.start < end);
            let mut unit: Vec<_> = by_source[first..last]
                .iter()
                .copied()
                .filter(|&i| clusters[i].source.end <= end)
                .collect();
            unit.sort_unstable();
            let mut raw = 0.;
            let mut trimmed = 0.;
            for &i in &unit {
                let width: f32 = clusters[i].glyphs.iter().map(|g| g.advance).sum();
                raw += width;
                if clusters[i].source.start < trimmed_end {
                    trimmed += width;
                }
            }
            let has_content = trimmed_end > unit_start;
            // Keep hanging-only prefixes with the first content on the line.
            if line_has_content && has_content && max_width.is_some_and(|w| raw_width + trimmed > w)
            {
                lines.push(line);
                line = LayoutLine::default();
                raw_width = 0.;
            }
            if has_content {
                line.width = raw_width + trimmed;
            }
            raw_width += raw;
            line.glyphs
                .extend(unit.iter().flat_map(|&i| &clusters[i].glyphs).copied());
            line_has_content |= has_content;
            has_unit |= end > unit_start;
            unit_start = end;
        }
        if has_unit || hard {
            lines.push(line);
        }
    }
    lines
}

fn is_hanging_space(ch: char) -> bool {
    ch.is_whitespace() && matches!(ch.properties().line_break(), LineBreak::BA | LineBreak::SP)
}

fn shaped_glyphs(
    glyphs: &[swash::shape::cluster::Glyph],
) -> impl Iterator<Item = ShapedGlyph> + '_ {
    glyphs
        .iter()
        .filter(|g| g.id != 0 || g.data == 0)
        .map(|g| ShapedGlyph {
            glyph: GlyphId(g.id),
            advance: g.advance,
            offset: [g.x, -g.y],
        })
}

fn is_hard_break(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

// Unicode 17.0 Default_Ignorable_Code_Point. Swash does not expose this property
// and its shaping-specific ignorable flag excludes some members (e.g. CGJ).
// https://www.unicode.org/Public/17.0.0/ucd/DerivedCoreProperties.txt
fn is_default_ignorable(ch: char) -> bool {
    matches!(ch, '\u{AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}'..='\u{1160}'
        | '\u{17B4}'..='\u{17B5}' | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{3164}'
        | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF0}'..='\u{FFF8}'
        | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0000}'..='\u{E0FFF}')
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod layout_tests {
    use super::*;
    use crate::atlas_tests::common;

    const ROBOTO: &[u8] = include_bytes!("../fonts/Roboto-Regular.ttf");

    // Take expected glyphs from the single-line public API. The shaper is
    // consulted only for byte ranges, never for the layout's filtering rules.
    fn whole_with_sources(
        writer: &mut TypeWriter,
        store: &FontStore,
        font: FontId,
        text: &str,
    ) -> (Paragraph, Vec<std::ops::Range<usize>>) {
        let whole = writer
            .shape_text(store, font, [0.; 2], 32., [1.; 4], text)
            .unwrap();
        let instance = store.get(font).unwrap();
        let mut sources = Vec::new();
        writer.shape_with_at(
            instance.as_ref(),
            instance.location.coords().iter().map(|c| c.to_bits()),
            32.,
            text,
            |offset, cluster| {
                sources.extend(std::iter::repeat_n(
                    offset + cluster.source.start as usize..offset + cluster.source.end as usize,
                    cluster.glyphs.len(),
                ));
            },
        );
        assert_eq!(sources.len(), whole.glyphs().len());
        (whole, sources)
    }

    #[test]
    fn wrapping_alignment_hanging_space_and_block_edits() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(&device, &queue, ROBOTO, "Hi Wide")
            .unwrap();
        let mut writer = TypeWriter::new();
        let whole = writer
            .shape_text(&store, font, [10., 20.], 32., [1.; 4], "Hi Hi Hi")
            .unwrap();
        let word_width = whole.glyphs()[..2].iter().map(|g| g.advance).sum::<f32>();
        assert_eq!(word_width, 30.578125); // Fixed Roboto advances at 32 px/em.
        for (align, shift) in [
            (Align::Left, 0.),
            (Align::Center, (40. - word_width) / 2.),
            (Align::Right, 40. - word_width),
        ] {
            let layout = TextLayout::default()
                .with_max_width(40.)
                .with_align(align)
                .with_line_height(LineHeight::Pixels(48.));
            let mut block = writer
                .layout_text(&store, font, [10., 20.], 32., [1.; 4], "Hi Hi Hi", &layout)
                .unwrap();
            assert_eq!(block.line_count(), 3);
            assert_eq!(block.size(), [40., 144.]);
            for (n, line) in block.lines().iter().enumerate() {
                assert_eq!(line.position(), [10. + shift, 20. + n as f32 * 48.]);
                assert_eq!(line.width(), word_width);
                let end = if n < 2 {
                    n * 3 + 3
                } else {
                    whole.glyphs().len()
                };
                assert_eq!(line.glyphs(), &whole.glyphs()[n * 3..end]);
            }
            let before: Vec<_> = block.lines().iter().map(Paragraph::position).collect();
            block.set_position([30., 50.]);
            block.set_color([0., 0., 0., 1.]);
            assert_eq!(block.position(), [30., 50.]);
            assert_eq!(block.size(), [40., 144.]);
            for (line, old) in (&block).into_iter().zip(before) {
                assert_eq!(line.position(), [old[0] + 20., old[1] + 30.]);
                assert_eq!(line.color(), [0., 0., 0., 1.]);
            }
        }
        for width in [0., 10., 40.] {
            let block = writer
                .layout_text(
                    &store,
                    font,
                    [0.; 2],
                    32.,
                    [1.; 4],
                    "Hi Wide Hi",
                    &TextLayout::default().with_max_width(width),
                )
                .unwrap();
            assert_eq!(block.line_count(), 3);
            assert!(block.lines()[1].width() > width);
            let shaped = writer
                .shape_text(&store, font, [0.; 2], 32., [1.; 4], "Hi Wide Hi")
                .unwrap();
            assert_eq!(
                block
                    .lines()
                    .iter()
                    .flat_map(|p| p.glyphs())
                    .copied()
                    .collect::<Vec<_>>(),
                shaped.glyphs()
            );
        }
        for (text, count) in [
            ("Hi-Hi", 2),
            ("Hi\u{a0}Hi", 1),
            ("中文中文", 4),
            ("e\u{301}", 1),
        ] {
            let block = writer
                .layout_text(
                    &store,
                    font,
                    [0.; 2],
                    32.,
                    [1.; 4],
                    text,
                    &TextLayout::default().with_max_width(0.),
                )
                .unwrap();
            assert_eq!(block.line_count(), count, "{text:?}");
        }
        for (align, factor) in [(Align::Left, 0.), (Align::Center, 0.5), (Align::Right, 1.)] {
            let block = writer
                .layout_text(
                    &store,
                    font,
                    [10., 20.],
                    32.,
                    [1.; 4],
                    "Hi\nWide",
                    &TextLayout::default().with_align(align),
                )
                .unwrap();
            let widest = block.lines()[1].width();
            assert_eq!(block.size()[0], widest);
            assert_eq!(
                block.lines()[0].position()[0],
                10. + (widest - word_width) * factor
            );
            assert_eq!(block.lines()[1].position()[0], 10.);
        }
        let metrics = store.metrics(font, 32.).unwrap();
        let pitch = metrics.ascent + metrics.descent + metrics.line_gap;
        let block = writer
            .layout_text(
                &store,
                font,
                [0.; 2],
                32.,
                [1.; 4],
                "Hi\nHi",
                &TextLayout::default()
                    .with_align(Align::Center)
                    .with_line_height(LineHeight::Scale(1.5)),
            )
            .unwrap();
        assert_eq!(block.size(), [word_width, 2. * pitch * 1.5]);
        assert_eq!(block.lines()[1].position(), [0., pitch * 1.5]);
    }

    #[test]
    fn hard_breaks_drop_mapped_controls_and_preserve_shaped_slices() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let face = swash::FontRef::from_index(ROBOTO, 0).unwrap();
        assert_eq!(
            face.charmap().map('\r'),
            3,
            "regression requires a real CR glyph"
        );
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(&device, &queue, ROBOTO, "HiAVé")
            .unwrap();
        let mut writer = TypeWriter::new();
        let whole = writer
            .shape_text(&store, font, [0.; 2], 32., [1.; 4], "Hi\rHi")
            .unwrap();
        assert!(whole.glyphs().iter().any(|g| g.glyph == GlyphId(3)));
        let block = writer
            .layout_text(
                &store,
                font,
                [0.; 2],
                32.,
                [1.; 4],
                "Hi\rHi",
                &TextLayout::default(),
            )
            .unwrap();
        assert!(block
            .lines()
            .iter()
            .flat_map(|p| p.glyphs())
            .all(|g| g.glyph != GlyphId(3)));
        for (text, count) in [
            ("", 0),
            ("\n", 1),
            ("\n\n", 2),
            ("Hi\n", 1),
            ("Hi\r\n", 1),
            ("Hi\nHi", 2),
            ("Hi\r\nHi", 2),
            ("Hi\n\nHi", 3),
            ("Hi\r\rHi", 3),
            ("\r\n\r\nHi", 3),
            ("Hi\u{b}Hi\u{c}Hi\u{85}Hi\u{2028}Hi\u{2029}Hi", 6),
        ] {
            let block = writer
                .layout_text(
                    &store,
                    font,
                    [0.; 2],
                    32.,
                    [1.; 4],
                    text,
                    &TextLayout::default(),
                )
                .unwrap();
            assert_eq!(block.line_count(), count, "{text:?}");
            let metrics = store.metrics(font, 32.).unwrap();
            let pitch = metrics.ascent + metrics.descent + metrics.line_gap;
            assert_eq!(block.size()[1], count as f32 * pitch);
            if !text.contains("Hi") {
                assert_eq!(block.size()[0], 0.);
            }
            for (n, line) in block.lines().iter().enumerate() {
                assert_eq!(line.position()[1], n as f32 * pitch);
            }
            let mut ranges = Vec::new();
            let mut start = 0;
            let mut previous_cr = false;
            for (offset, ch) in text.char_indices() {
                if ch == '\n' && previous_cr {
                    start = offset + 1;
                } else if matches!(
                    ch,
                    '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
                ) {
                    ranges.push(start..offset);
                    start = offset + ch.len_utf8();
                }
                previous_cr = ch == '\r';
            }
            if start < text.len() {
                ranges.push(start..text.len());
            }
            assert_eq!(ranges.len(), count);
            for (line, range) in block.lines().iter().zip(ranges) {
                let expected = writer
                    .shape_text(&store, font, [0.; 2], 32., [1.; 4], &text[range.clone()])
                    .unwrap();
                assert_eq!(line.glyphs(), expected.glyphs(), "{text:?}: {range:?}");
            }
        }
        // Each soft line is an exact slice of its own paragraph's shaping.
        // Byte ends are fixed independently of the layout's cluster membership.
        for (text, pieces) in [
            (
                "AV é\u{301} office\r\nHi",
                vec![("AV é\u{301} office", vec![3, 8, 14]), ("Hi", vec![2])],
            ),
            (
                "Hi 中文\nAV",
                vec![("Hi 中文", vec![3, 6, 9]), ("AV", vec![2])],
            ),
            ("e\u{301} Hi Hi", vec![("e\u{301} Hi Hi", vec![4, 7, 9])]),
        ] {
            let mut expected = Vec::new();
            for (piece, ends) in pieces {
                let (whole, sources) = whole_with_sources(&mut writer, &store, font, piece);
                let mut first = 0;
                for end in ends {
                    let last = sources.partition_point(|source| source.start < end);
                    expected.push(whole.glyphs()[first..last].to_vec());
                    first = last;
                }
                assert_eq!(first, whole.glyphs().len());
            }
            let block = writer
                .layout_text(
                    &store,
                    font,
                    [0.; 2],
                    32.,
                    [1.; 4],
                    text,
                    &TextLayout::default().with_max_width(0.),
                )
                .unwrap();
            assert_eq!(block.line_count(), expected.len());
            for (line, glyphs) in block.lines().iter().zip(expected) {
                assert_eq!(line.glyphs(), glyphs, "{text:?}");
            }
        }
    }

    #[test]
    fn opportunity_inside_a_ligature_is_ignored() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut bytes = common::missing_ignorable_ligature_font();
        let face = swash::FontRef::from_index(&bytes, 0).unwrap();
        let space = face.charmap().map(' ');
        let h = face.charmap().map('H');
        let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
        let record = (0..count)
            .map(|i| 12 + i * 16)
            .find(|&i| &bytes[i..i + 4] == b"GSUB")
            .unwrap();
        let offset =
            u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap()) as usize;
        let len = u32::from_be_bytes(bytes[record + 12..record + 16].try_into().unwrap()) as usize;
        // Change the fixture's ligature from .notdef + a to space + H.
        bytes[offset + len - 8..offset + len - 6].copy_from_slice(&h.to_be_bytes());
        bytes[offset + len - 2..offset + len].copy_from_slice(&space.to_be_bytes());
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(&device, &queue, &bytes, "Hi ")
            .unwrap();
        let mut writer = TypeWriter::new();
        let instance = store.get(font).unwrap();
        let clusters = writer.shape_clusters_at(instance.as_ref(), std::iter::empty(), 32., "Hi H");
        assert!(clusters
            .iter()
            .any(|c| c.source.start < 3 && c.source.end > 3));
        assert_eq!(analyze("Hi H".chars()).nth(3).unwrap().1, Boundary::Line);
        let whole = writer
            .shape_text(&store, font, [0.; 2], 32., [1.; 4], "Hi H")
            .unwrap();
        let block = writer
            .layout_text(
                &store,
                font,
                [0.; 2],
                32.,
                [1.; 4],
                "Hi H",
                &TextLayout::default().with_max_width(0.),
            )
            .unwrap();
        assert_eq!(block.line_count(), 1);
        assert_eq!(block.lines()[0].glyphs(), whole.glyphs());
    }

    #[test]
    fn invalid_options_fallback_metrics_and_variation_instances() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(&device, &queue, ROBOTO, "Hi")
            .unwrap();
        let mut writer = TypeWriter::new();
        for value in [-1., f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(writer
                .layout_text(
                    &store,
                    font,
                    [0.; 2],
                    32.,
                    [1.; 4],
                    "Hi",
                    &TextLayout::default().with_max_width(value)
                )
                .is_none());
        }
        for value in [0., -1., f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for height in [LineHeight::Scale(value), LineHeight::Pixels(value)] {
                assert!(writer
                    .layout_text(
                        &store,
                        font,
                        [0.; 2],
                        32.,
                        [1.; 4],
                        "Hi",
                        &TextLayout::default().with_line_height(height)
                    )
                    .is_none());
            }
            assert!(writer
                .layout_text(
                    &store,
                    font,
                    [0.; 2],
                    value,
                    [1.; 4],
                    "Hi",
                    &TextLayout::default()
                )
                .is_none());
        }
        let mut other = FontStore::new(&device);
        let foreign = other
            .load_from_bytes(&device, &queue, ROBOTO, "Hi")
            .unwrap();
        assert!(writer
            .layout_text(
                &store,
                foreign,
                [0.; 2],
                32.,
                [1.; 4],
                "Hi",
                &TextLayout::default()
            )
            .is_none());
        let huge_metrics = store.metrics(font, f32::MAX).unwrap();
        assert!(!(huge_metrics.ascent + huge_metrics.descent + huge_metrics.line_gap).is_finite());
        let huge = writer
            .layout_text(
                &store,
                font,
                [0.; 2],
                f32::MAX,
                [1.; 4],
                "Hi",
                &TextLayout::default(),
            )
            .unwrap();
        assert_eq!(huge.size()[1], f32::MAX);
        // Select hhea metrics with negative ascent and a nonpositive sum.
        let mut bytes = ROBOTO.to_vec();
        let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
        for (tag, fields) in [
            (b"hhea", vec![(4, -100i16), (6, 0), (8, -20)]),
            (b"OS/2", vec![(0, 3), (62, 0)]),
        ] {
            let record = (0..count)
                .map(|i| 12 + i * 16)
                .find(|&i| &bytes[i..i + 4] == tag)
                .unwrap();
            let offset =
                u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap()) as usize;
            for (field, value) in fields {
                bytes[offset + field..offset + field + 2].copy_from_slice(&value.to_be_bytes());
            }
        }
        let unusual = store
            .load_from_bytes(&device, &queue, &bytes, "Hi")
            .unwrap();
        let metrics = store.metrics(unusual, 32.).unwrap();
        assert!(metrics.ascent < 0. && metrics.ascent + metrics.descent + metrics.line_gap <= 0.);
        let block = writer
            .layout_text(
                &store,
                unusual,
                [0.; 2],
                32.,
                [1.; 4],
                "Hi\nHi",
                &TextLayout::default(),
            )
            .unwrap();
        assert_eq!(block.size()[1], 64.);
        assert_eq!(block.lines()[1].position()[1], 32.);
        let base = store
            .load_from_bytes(&device, &queue, &common::mvar_anchor_font(), "Hi")
            .unwrap();
        let axis = store.axes(base).unwrap()[0];
        let varied = store
            .load_variation(&device, &queue, base, &[(axis.tag, axis.max)])
            .unwrap();
        let metrics = store.metrics(varied, 32.).unwrap();
        let pitch = metrics.ascent + metrics.descent + metrics.line_gap;
        let block = writer
            .layout_text(
                &store,
                varied,
                [0.; 2],
                32.,
                [1.; 4],
                "Hi\nHi",
                &TextLayout::default(),
            )
            .unwrap();
        assert_eq!(block.size()[1], 2. * pitch);
        assert_eq!(block.lines()[1].position()[1], pitch);
        let (whole, _) = whole_with_sources(&mut writer, &store, varied, "Hi");
        for line in block.lines() {
            assert_eq!(line.glyphs(), whole.glyphs());
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod round2_tests {
    use super::*;
    use crate::atlas_tests::common;
    const ROBOTO: &[u8] = include_bytes!("../fonts/Roboto-Regular.ttf");

    #[test]
    fn round2_hanging_at_hard_and_final_ends() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(&device, &queue, ROBOTO, "ab cdefx")
            .unwrap();
        let mut writer = TypeWriter::new();
        let width = writer
            .shape_text(&store, font, [0.; 2], 32., [1.; 4], "ab cdef")
            .unwrap()
            .width();
        let whole = writer
            .shape_text(&store, font, [0.; 2], 32., [1.; 4], "ab cdef   ")
            .unwrap();
        let block = writer
            .layout_text(
                &store,
                font,
                [0.; 2],
                32.,
                [1.; 4],
                "ab cdef   ",
                &TextLayout::default().with_max_width(width),
            )
            .unwrap();
        assert_eq!(block.line_count(), 1);
        assert_eq!(block.lines()[0].width(), width);
        assert_eq!(block.lines()[0].glyphs(), whole.glyphs());
        let ab = writer
            .shape_text(&store, font, [0.; 2], 32., [1.; 4], "ab")
            .unwrap()
            .width();
        for text in ["ab   ", "ab   \nx", "ab   \r\nx"] {
            let block = writer
                .layout_text(
                    &store,
                    font,
                    [10., 20.],
                    32.,
                    [1.; 4],
                    text,
                    &TextLayout::default()
                        .with_max_width(100.)
                        .with_align(Align::Right),
                )
                .unwrap();
            assert_eq!(block.lines()[0].width(), ab, "{text:?}");
            assert_eq!(block.lines()[0].position()[0], 110. - ab, "{text:?}");
        }
    }

    #[test]
    fn round2_no_break_spaces_keep_width() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(&device, &queue, ROBOTO, "ab cd")
            .unwrap();
        let mut writer = TypeWriter::new();
        for space in ['\u{a0}', '\u{2007}', '\u{202f}'] {
            let prefix = format!("ab{space}");
            let expected = writer
                .shape_text(&store, font, [0.; 2], 32., [1.; 4], &prefix)
                .unwrap()
                .width();
            let text = format!("{prefix} cd");
            let block = writer
                .layout_text(
                    &store,
                    font,
                    [0.; 2],
                    32.,
                    [1.; 4],
                    &text,
                    &TextLayout::default().with_max_width(expected),
                )
                .unwrap();
            assert_eq!(block.line_count(), 2);
            assert_eq!(block.lines()[0].width(), expected, "{space:?}");
        }
    }

    #[test]
    fn round2_position_round_trip_preserves_offsets() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(&device, &queue, ROBOTO, "abc")
            .unwrap();
        let mut block = TypeWriter::new()
            .layout_text(
                &store,
                font,
                [0.; 2],
                32.,
                [1.; 4],
                "a\nb\nc",
                &TextLayout::default().with_line_height(LineHeight::Pixels(1.)),
            )
            .unwrap();
        block.set_position([0., 1e8]);
        block.set_position([0.; 2]);
        assert_eq!(
            block
                .lines()
                .iter()
                .map(|p| p.position()[1])
                .collect::<Vec<_>>(),
            [0., 1., 2.]
        );
    }

    #[test]
    fn round2_reordered_sources_stay_together_and_remove_hard_glyphs() {
        let glyph = |id| ShapedGlyph {
            glyph: GlyphId(id),
            advance: 1.,
            offset: [0.; 2],
        };
        let soft: Vec<_> = [(0..1, 1), (2..3, 3), (1..2, 2), (3..4, 4)]
            .into_iter()
            .map(|(source, id)| ShapedCluster {
                source,
                glyphs: vec![glyph(id)],
            })
            .collect();
        let lines = break_lines("a bZ", &soft, Some(0.));
        assert_eq!(lines.len(), 1, "the decreasing-source span is unbreakable");
        assert_eq!(lines[0].glyphs, [glyph(1), glyph(3), glyph(2), glyph(4)]);
        let ba = [
            ShapedCluster {
                source: 0..1,
                glyphs: vec![glyph(1)],
            },
            ShapedCluster {
                source: 1..2,
                glyphs: vec![glyph(2)],
            },
        ];
        let hung = break_lines("a\t", &ba, None);
        assert_eq!(hung[0].width, 1.);
        assert_eq!(hung[0].glyphs, [glyph(1), glyph(2)]);
        // A reordered syllable spans a space opportunity; CR output occurs
        // before clusters with smaller source offsets in shaping order.
        let clusters: Vec<_> = [
            (0..1, 1),
            (4..5, 4),
            (2..3, 2),
            (3..4, 3),
            (1..2, 9),
            (5..6, 5),
        ]
        .into_iter()
        .map(|(source, id)| ShapedCluster {
            source,
            glyphs: vec![glyph(id)],
        })
        .collect();
        let lines = break_lines("a\rb cZ", &clusters, Some(0.));
        let actual: Vec<Vec<_>> = lines
            .iter()
            .map(|line| line.glyphs.iter().map(|g| g.glyph.0).collect())
            .collect();
        assert_eq!(actual, [vec![1], vec![4, 2, 3, 5]]);
        let reversed = [
            ShapedCluster {
                source: 2..3,
                glyphs: vec![glyph(3)],
            },
            ShapedCluster {
                source: 1..2,
                glyphs: vec![glyph(9)],
            },
            ShapedCluster {
                source: 0..1,
                glyphs: vec![glyph(1)],
            },
        ];
        let lines = break_lines("a\rb", &reversed, None);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].glyphs, [glyph(1)]);
        assert_eq!(lines[1].glyphs, [glyph(3)]);
    }

    #[test]
    fn round2_nonfinite_widths_and_origins_are_rejected() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store.load_from_bytes(&device, &queue, ROBOTO, "W").unwrap();
        let mut writer = TypeWriter::new();
        for align in [Align::Left, Align::Center, Align::Right] {
            assert!(
                writer
                    .layout_text(
                        &store,
                        font,
                        [0.; 2],
                        f32::MAX,
                        [1.; 4],
                        "WWWW",
                        &TextLayout::default().with_align(align)
                    )
                    .is_none(),
                "{align:?}"
            );
        }
        assert!(writer
            .layout_text(
                &store,
                font,
                [f32::NAN, 0.],
                32.,
                [1.; 4],
                "W",
                &TextLayout::default()
            )
            .is_none());
        assert!(writer
            .layout_text(
                &store,
                font,
                [0., f32::MAX],
                32.,
                [1.; 4],
                "W\nW",
                &TextLayout::default().with_line_height(LineHeight::Pixels(f32::MAX))
            )
            .is_none());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod round3_tests {
    use super::*;
    use crate::atlas_tests::common;
    const ROBOTO: &[u8] = include_bytes!("../fonts/Roboto-Regular.ttf");

    #[test]
    fn round3_mark_after_hard_break_is_shaped_with_its_paragraph() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(&device, &queue, ROBOTO, "ab\u{301}")
            .unwrap();
        let mut writer = TypeWriter::new();
        let expected = writer
            .shape_text(&store, font, [0.; 2], 32., [1.; 4], "\u{301}b")
            .unwrap();
        let face = swash::FontRef::from_index(ROBOTO, 0).unwrap();
        let mark = face.charmap().map('\u{301}');
        assert_ne!(mark, 0);
        assert_eq!(expected.glyphs().len(), 2);
        assert!(expected.glyphs().iter().any(|g| g.glyph == GlyphId(mark)));
        assert!(expected
            .glyphs()
            .iter()
            .any(|g| g.glyph == GlyphId(face.charmap().map('b'))));
        for separator in [
            "\n", "\r\n", "\r", "\u{b}", "\u{c}", "\u{85}", "\u{2028}", "\u{2029}",
        ] {
            let text = format!("a{separator}\u{301}b");
            let block = writer
                .layout_text(
                    &store,
                    font,
                    [0.; 2],
                    32.,
                    [1.; 4],
                    &text,
                    &TextLayout::default(),
                )
                .unwrap();
            assert_eq!(block.line_count(), 2);
            assert_eq!(
                block.lines()[1].glyphs(),
                expected.glyphs(),
                "{separator:?}"
            );
        }
    }

    #[test]
    fn round3_leading_spaces_do_not_create_an_empty_line() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store
            .load_from_bytes(&device, &queue, ROBOTO, " abc")
            .unwrap();
        let mut writer = TypeWriter::new();
        let width = writer
            .shape_text(&store, font, [0.; 2], 32., [1.; 4], "abc")
            .unwrap()
            .width();
        let whole = writer
            .shape_text(&store, font, [0.; 2], 32., [1.; 4], "   abc")
            .unwrap();
        for max in [0., width] {
            let block = writer
                .layout_text(
                    &store,
                    font,
                    [0.; 2],
                    32.,
                    [1.; 4],
                    "   abc",
                    &TextLayout::default().with_max_width(max),
                )
                .unwrap();
            assert_eq!(block.line_count(), 1);
            assert_eq!(block.lines()[0].glyphs(), whole.glyphs());
            assert_eq!(block.lines()[0].width(), whole.width());
            assert!(block.lines()[0].width() > max);
        }
    }

    #[test]
    fn round3_empty_text_rejects_nonfinite_origins() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store.load_from_bytes(&device, &queue, ROBOTO, "W").unwrap();
        for position in [[f32::NAN, 0.], [0., f32::INFINITY], [f32::NEG_INFINITY, 0.]] {
            assert!(TypeWriter::new()
                .layout_text(
                    &store,
                    font,
                    position,
                    32.,
                    [1.; 4],
                    "",
                    &TextLayout::default()
                )
                .is_none());
        }
    }

    #[test]
    fn round3_nonfinite_block_size_is_rejected() {
        let Some((device, queue, _)) = common::gpu() else {
            return;
        };
        let mut store = FontStore::new(&device);
        let font = store.load_from_bytes(&device, &queue, ROBOTO, "W").unwrap();
        assert!(TypeWriter::new()
            .layout_text(
                &store,
                font,
                [0.; 2],
                32.,
                [1.; 4],
                "W\nW",
                &TextLayout::default().with_line_height(LineHeight::Pixels(f32::MAX))
            )
            .is_none());
    }
}

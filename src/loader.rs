use std::{collections::HashMap, fmt, sync::Arc};
use skrifa::{
    instance::{Location, LocationRef, Size},
    outline::{pen::{PathStyle, ControlBoundsPen}, DrawSettings, OutlineGlyphCollection, OutlineGlyphFormat, OutlinePen},
    raw::TableProvider,
    FontRef as SkFontRef, GlyphId,
};
use swash::{CacheKey, FontRef};
use skrifa::MetadataProvider;

use crate::atlas::{allocation::Allocation, Atlas};

#[derive(Debug)]
/// Cached outline and placement metrics for one glyph, in font units.
pub struct Glyph {
    /// Eight floats per segment: three x/y pairs, then two zeros.
    /// Lines and quadratics store start, control, end; lines use control=end.
    /// Cubics are subdivided into quadratics within 0.25 font units, with a
    /// depth cap of 10 for pathological inputs (which may exceed that tolerance).
    /// Records chain end-to-start within each contour. CFF/CFF2 contours are
    /// explicitly closed; glyphs with negative total signed area are reversed
    /// to match TrueType winding for the shader.
    /// Coordinates use x rightward and y downward, flipped around `bbox.y_max`.
    #[cfg(feature = "unstable-internals")]
    pub curves: Vec<f32>,
    /// Atlas location and float-texel count for the encoded segments.
    pub allocation: Allocation,
    /// Atlas band header, descriptors and duplicated curves. Loaded glyphs have bands.
    pub bands: Allocation,
    /// Original font-unit bounding box, with y increasing upward.
    pub bbox: Rect,
    /// Nonpositive bottom extent below the baseline, in font units.
    pub descent: i16,
    /// `default ascender - max(bbox.y_min, 0) - bbox.height()`, in font units.
    /// The renderer adds the instance ascender’s MVAR delta before scaling.
    /// When `y_min < 0`, this is `ascender - y_max + y_min`; the renderer adds
    /// `abs(descent)` back before scaling to pixels and adding paragraph y.
    pub y_offset: i32,
    /// Bounding-box left edge in font units, used by the shader.
    pub left_side_bearing: i16,
}

/// Owned font bytes, parsed face, and selected cached outlines.
pub struct Font {
    pub(crate) data: Arc<[u8]>,
    pub(crate) characters: Arc<str>,
    pub(crate) location: Location,
    pub(crate) axes: Vec<crate::Axis>,
    // Retain design values so later overrides normalize together, including avar.
    pub(crate) settings: Vec<([u8; 4], f32)>,
    pub(crate) metrics: crate::FontMetrics,
    // Keep the legacy integer glyph offsets unchanged, including default rounding.
    pub(crate) ascender_delta: f32,
    index: u32,
    /// Byte offset of this face within the font data.
    pub(crate) offset: u32,
    /// Unique swash cache key identifying this loaded face.
    pub(crate) key: CacheKey,
    /// Cached preset and GSUB-closure outlines, by glyph ID. Includes
    /// empty glyph entries with zero-sized allocations and boxes.
    pub glyph_cache: HashMap<u32, Glyph>,
}

type Result<T> = std::result::Result<T, LoadingError>;

#[derive(Debug)]
/// Failure to read, parse or cache a font. GPU errors are handled by wgpu instead.
#[non_exhaustive]
pub enum LoadingError {
    /// The file could not be read, including permission and other I/O failures.
    Io(std::io::Error),
    /// Invalid font bytes or a face index absent from the font collection.
    InvalidFont,
    /// The base font ID does not belong to this store.
    UnknownFont,
    /// Outlined glyphs that could not fit in the atlas. The font was not loaded.
    AtlasFull(Vec<crate::GlyphId>),
    /// A glyph's outline exceeds the loader's work limits: more than 65,535
    /// points or 16,384 contours after composite expansion, more than 4,096
    /// composite component references, or more than 16,384 encoded segments.
    /// These are loader policy, not font validity rules: a valid but very
    /// complex glyph can exceed them. The font was not loaded.
    GlyphTooComplex(crate::GlyphId),
    /// GSUB closure exceeded 128 passes or 4,000,000 work units.
    SubstitutionTooComplex,
}

impl fmt::Display for LoadingError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::SubstitutionTooComplex => write!(f, "GSUB closure exceeds the loader's limits"),
            Self::Io(error) => write!(f, "Could not read font: {error}"),
            Self::UnknownFont => write!(f, "Unknown font ID"),
            Self::InvalidFont => write!(f, "Invalid font data"),
            Self::AtlasFull(glyphs) => write!(f, "Glyphs did not fit in the atlas: {glyphs:?}"),
            Self::GlyphTooComplex(glyph) => write!(f, "Glyph outline exceeds the loader's limits: {glyph:?}"),
        }
    }
}

impl std::error::Error for LoadingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::SubstitutionTooComplex | Self::UnknownFont | Self::InvalidFont | Self::AtlasFull(_) | Self::GlyphTooComplex(_) => None,
        }
    }
}

impl Font {
    /// Load a default face for internal GPU tests.
    #[cfg(test)]
    pub(crate) fn from_bytes(
        device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, queue: &wgpu::Queue,
        data: Vec<u8>, index: usize, cache_preset: &str, atlas: &mut Atlas,
    ) -> Result<Font> {
        if index != 0 { return Err(LoadingError::InvalidFont); }
        Self::from_instance(device, encoder, queue, data.into(), cache_preset.into(), &[], atlas)
    }

    pub(crate) fn from_instance(
        device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, queue: &wgpu::Queue,
        data: Arc<[u8]>, characters: Arc<str>, axes: &[([u8; 4], f32)], atlas: &mut Atlas,
    ) -> Result<Font> {
        let (face, offset, key) = parse_font(&data, 0)?;
        let axis_info: Vec<_> = face.axes().iter().map(|axis| crate::Axis {
            tag: axis.tag().to_be_bytes(), min: axis.min_value(),
            default: axis.default_value(), max: axis.max_value(),
        }).collect();
        let settings: Vec<_> = axis_info.iter().filter_map(|axis|
            axes.iter().rev().find(|(tag, _)| *tag == axis.tag).copied()
        ).collect();
        let location = face.axes().location(settings.iter().map(|&(tag, value)|
            (skrifa::raw::types::Tag::new(&tag), value)));
        let metrics = face_line_metrics(&face, (&location).into());
        let ascender_delta = metrics.ascent - f32::from(face_ascender(&face));
        let font = FontRef { data: &data, offset, key };
        let ids = closure_glyph_ids(&face, font, &characters)?;
        let checkpoint = atlas.checkpoint();
        let glyph_cache = create_glyph_cache(device, encoder, queue, &face,
            ids.into_iter(), (&location).into(), atlas);
        let glyph_cache = match glyph_cache {
            Ok(cache) => cache,
            Err(error) => { atlas.restore(checkpoint); return Err(error); }
        };
        Ok(Self { data, characters, location, axes: axis_info, settings, metrics, ascender_delta,
            index: 0, offset, key, glyph_cache })
    }

    /// Borrow the original bytes as a swash font reference with the same cache key.
    pub(crate) fn as_ref(&self) -> FontRef<'_> {
        FontRef {
            data: &self.data,
            offset: self.offset,
            key: self.key,
        }
    }

    /// Borrow the selected face; metrics and outlines use font units, y upward.
    pub fn face(&self) -> SkFontRef<'_> {
        SkFontRef::from_index(&self.data, self.index).expect("validated font face")
    }
}


// Both parsers must accept the same face; keep its offset/key from swash.
fn parse_font(data: &[u8], index: usize) -> Result<(SkFontRef<'_>, u32, CacheKey)> {
    let face_index = u32::try_from(index).map_err(|_| LoadingError::InvalidFont)?;
    let font = FontRef::from_index(data, index).ok_or(LoadingError::InvalidFont)?;
    let (offset, key) = (font.offset, font.key);
    let face = SkFontRef::from_index(data, face_index).map_err(|_| LoadingError::InvalidFont)?;
    // Require the same core face tables as the shaping parser.
    let head = face.head().map_err(|_| LoadingError::InvalidFont)?;
    if !(16..=16384).contains(&head.units_per_em()) {
        return Err(LoadingError::InvalidFont);
    }
    face.hhea().map_err(|_| LoadingError::InvalidFont)?;
    face.maxp().map_err(|_| LoadingError::InvalidFont)?;
    Ok((face, offset, key))
}

// Default-instance selection from the 0.21 parser: only OS/2 v4+ can request
// typo metrics, and hhea's ascender alone controls the fallback.
fn select_ascender(hhea: i16, os2: Option<(u16, bool, i16, i16)>) -> i16 {
    if let Some((version, use_typo, typo, windows)) = os2 {
        if version >= 4 && use_typo {
            return typo;
        }
        if hhea == 0 {
            return if typo != 0 { typo } else { windows };
        }
    }
    hhea
}

fn face_ascender(face: &SkFontRef<'_>) -> i16 {
    use skrifa::raw::tables::os2::SelectionFlags;
    // Match the old parser's version and minimum-length checks.
    let os2 = valid_os2(face).map(|table| (
        table.version(), table.fs_selection().contains(SelectionFlags::USE_TYPO_METRICS),
        table.s_typo_ascender(), table.us_win_ascent() as i16,
    ));
    select_ascender(face.hhea().expect("validated hhea table").ascender().to_i16(), os2)
}

fn valid_os2<'a>(face: &SkFontRef<'a>) -> Option<skrifa::raw::tables::os2::Os2<'a>> {
    face.os2().ok().filter(|table| {
        let minimum = match table.version() {
            0 => 78,
            1 => 86,
            2..=4 => 96,
            5 => 100,
            _ => return false,
        };
        table.offset_data().len() >= minimum
    })
}

fn face_line_metrics(face: &SkFontRef<'_>, location: LocationRef<'_>) -> crate::FontMetrics {
    use skrifa::raw::{tables::{os2::SelectionFlags, mvar::tags::{HASC, HDSC, HLGP}}, types::Tag};
    let hhea = face.hhea().expect("validated hhea table");
    let mut descent = f32::from(hhea.descender().to_i16());
    let mut line_gap = f32::from(hhea.line_gap().to_i16());
    if let Some(os2) = valid_os2(face) {
        if (os2.version() >= 4 && os2.fs_selection().contains(SelectionFlags::USE_TYPO_METRICS))
            || (hhea.ascender().to_i16() == 0 && os2.s_typo_ascender() != 0) {
            descent = f32::from(os2.s_typo_descender());
            line_gap = f32::from(os2.s_typo_line_gap());
        } else if hhea.ascender().to_i16() == 0 {
            descent = -f32::from(os2.us_win_descent());
            line_gap = 0.;
        }
    }
    let delta = |tag: Tag| face.mvar().ok().and_then(|mvar|
        mvar.metric_delta(tag, location.coords()).ok()).map_or(0., |v| v.to_f64() as f32);
    crate::FontMetrics {
        ascent: f32::from(face_ascender(face)) + delta(HASC),
        descent: (descent + delta(HDSC)).abs(),
        line_gap: line_gap + delta(HLGP),
    }
}

fn glyph_ids<'a>(font: FontRef<'a>, characters: &'a str) -> impl Iterator<Item = GlyphId> + 'a {
    let charmap = font.charmap();
    let mut seen = std::collections::HashSet::new();
    characters.chars().map(move |ch| {
        GlyphId::new(charmap.map(ch) as u32)
    }).chain(std::iter::once(GlyphId::new(0))).filter(move |id| seen.insert(*id))
}

fn normalization_glyph_ids(font: FontRef<'_>, characters: &str) -> Vec<GlyphId> {
    use swash::text::Codepoint;
    let mut alphabet: std::collections::HashSet<char> = characters.chars().collect();
    for ch in characters.chars() {
        alphabet.extend(ch.decompose());
    }
    let charmap = font.charmap();
    let mut ids: Vec<_> = glyph_ids(font, characters).collect();
    let mut seen: std::collections::HashSet<_> = ids.iter().copied().collect();
    let mut add = |ch| {
        let id = GlyphId::new(charmap.map(ch) as u32);
        if seen.insert(id) {
            ids.push(id);
        }
    };
    for ch in characters.chars().flat_map(Codepoint::decompose) {
        add(ch);
    }
    // Khmer's parser inserts a vowel that is not a Unicode decomposition.
    // Ask the same parser rather than maintaining a second split-vowel table.
    use swash::text::{
        cluster::{CharCluster, Parser, Token},
        Script,
    };
    for &ch in alphabet.iter().filter(|ch| ch.script() == Script::Khmer) {
        let token = Token {
            ch,
            len: ch.len_utf8() as u8,
            info: ch.properties().into(),
            ..Default::default()
        };
        let mut parser = Parser::new(Script::Khmer, std::iter::once(token));
        let mut cluster = CharCluster::new();
        while parser.next(&mut cluster) {
            for part in cluster.chars() {
                add(part.ch);
            }
        }
    }
    // The Unicode domain is fixed and bounded. Cache its nontrivial canonical
    // decompositions once; never enumerate potentially hostile cmap ranges.
    static DECOMPOSABLE: std::sync::OnceLock<Vec<char>> = std::sync::OnceLock::new();
    let decomposable = DECOMPOSABLE.get_or_init(|| {
        (0..=0x10ffff)
            .filter_map(char::from_u32)
            .filter(|&ch| ch.decompose().chars() != [ch])
            .collect()
    });
    // Expanded alphabet also covers partial recomposition within a cluster.
    for &ch in decomposable {
        if ch.decompose().all(|part| alphabet.contains(&part)) {
            add(ch);
        }
    }
    ids
}

fn closure_glyph_ids(face: &SkFontRef<'_>, font: FontRef<'_>, characters: &str) -> Result<Vec<GlyphId>> {
    // Missing GSUB is normal; a present but malformed table is a loading error.
    let gsub = if face.table_data(skrifa::raw::types::Tag::new(b"GSUB")).is_some() {
        Some(face.gsub().map_err(|_| LoadingError::InvalidFont)?)
    } else { None };
    crate::gsub::closure(gsub, normalization_glyph_ids(font, characters).into_iter())
}

fn create_glyph_cache(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    queue: &wgpu::Queue,
    face: &SkFontRef<'_>,
    glyph_ids: impl Iterator<Item = GlyphId>,
    location: LocationRef<'_>,
    atlas: &mut Atlas
) -> Result<HashMap<u32, Glyph>> {
    let mut glyph_cache = HashMap::new();
    let mut failed = Vec::new();

    for glyph_id in glyph_ids {
        check_composite_budget(face, glyph_id)?;
        if let Some(bbox) = glyph_bounding_box_at(face, glyph_id, location) {
            let (descent, y_offset, curves, band_data) = prepare_glyph_at(face, glyph_id, bbox, location)?;
            let left_side_bearing = bbox.x_min;

            // Each eight-float segment occupies two RGBA32F texels, including
            // the original index and sort-key padding slots.
            let too_complex = || LoadingError::GlyphTooComplex(crate::GlyphId(glyph_id.to_u32() as u16));
            let curves_count = u32::try_from(curves.len()).map_err(|_| too_complex())?;
            let bands_count = u32::try_from(band_data.len()).map_err(|_| too_complex())?;
    
            let bytes = unsafe {
                std::slice::from_raw_parts(curves.as_ptr() as *const u8, curves.len() * 4)
            };

            if let Some(allocation) = atlas.upload(curves_count, bytes, device, encoder, queue) {
                let bands = atlas.upload_bands(
                    bands_count,
                    bytemuck::cast_slice(&band_data),
                    device, encoder, queue,
                );
                let Some(bands) = bands else {
                    failed.push(crate::GlyphId(glyph_id.to_u32() as u16));
                    continue;
                };
                let glyph = Glyph {
                    #[cfg(feature = "unstable-internals")]
                    curves,
                    allocation,
                    bands,
                    bbox,
                    descent,
                    y_offset,
                    left_side_bearing,
                };
        
                glyph_cache.insert(glyph_id.to_u32(), glyph);
            } else {
                let id = crate::GlyphId(glyph_id.to_u32() as u16);
                failed.push(id);
            }
        } else {
            // Record empty closure members (e.g. spaces) without atlas storage.
            // The renderer skips their zero-area boxes while retaining advance.
            glyph_cache.insert(glyph_id.to_u32(), Glyph {
                #[cfg(feature = "unstable-internals")]
                curves: Vec::new(),
                allocation: Allocation::default(), bands: Allocation::default(),
                bbox: Rect { x_min: 0, y_min: 0, x_max: 0, y_max: 0 },
                descent: 0, y_offset: 0, left_side_bearing: 0,
            });
        }
    }

    if failed.is_empty() { Ok(glyph_cache) } else { Err(LoadingError::AtlasFull(failed)) }
}

#[cfg(feature = "unstable-internals")]
fn prepare_glyph(face: &SkFontRef<'_>, id: GlyphId, bbox: Rect) -> Result<(i16, i32, Vec<f32>, Vec<f32>)> {
    prepare_glyph_at(face, id, bbox, LocationRef::default())
}

fn prepare_glyph_at(
    face: &SkFontRef<'_>, id: GlyphId, bbox: Rect, location: LocationRef<'_>,
) -> Result<(i16, i32, Vec<f32>, Vec<f32>)> {
    // Any legal i16 box spans at most 65,535 units, so i32 holds every derived value.
    let height = bbox.height();
    let descent = bbox.y_min.min(0);
    let distance = i32::from(bbox.y_min.max(0));
    let total_height = height + i32::from(descent) + distance;
    let y_offset = i32::from(face_ascender(face)) - distance - height;
    let mut builder = BezierBuilder::new(total_height as f32);
    outline_glyph_at(face, id, &mut builder, location);
    if builder.over_budget {
        return Err(LoadingError::GlyphTooComplex(crate::GlyphId(id.to_u32() as u16)));
    }
    builder.finish();
    let bands = build_slug_bands(&builder.curves);
    Ok((descent, y_offset, builder.curves, bands))
}

// Exercise parsing, selection, metrics, outlines and bands at the default
// location, as in a default GPU load, without allocating an atlas.
#[cfg(feature = "unstable-internals")]
pub(crate) fn check_font_bytes(data: &[u8], characters: &str) -> Result<usize> {
    let (face, offset, key) = parse_font(data, 0)?;
    let _metrics = face_line_metrics(&face, LocationRef::default());
    let mut prepared = 0;
    for id in closure_glyph_ids(&face, FontRef { data, offset, key }, characters)? {
        check_composite_budget(&face, id)?;
        if let Some(bbox) = glyph_bounding_box(&face, id) {
            prepare_glyph(&face, id, bbox)?;
            prepared += 1;
        }
    }
    Ok(prepared)
}

const MAX_GLYPH_POINTS: usize = 65_535;
const MAX_COMPONENT_REFERENCES: usize = 4_096;
const MAX_GLYPH_SEGMENTS: usize = 16_384;
// skrifa stops composite recursion past this depth; deeper references never draw.
const MAX_COMPOSITE_DEPTH: usize = 32;

// skrifa expands a TrueType composite and sizes its scratch memory before any
// point limit applies, and bounds only recursion depth, not fan-out. Walk the
// component graph first with a fixed budget so a crafted composite is
// rejected before skrifa is asked to draw it. Parse errors are left to skrifa.
fn check_composite_budget(face: &SkFontRef<'_>, id: GlyphId) -> Result<()> {
    use skrifa::raw::tables::glyf::Glyph as Outline;
    let (Ok(glyf), Ok(loca)) = (face.glyf(), face.loca(None)) else {
        return Ok(());
    };
    let too_complex = || LoadingError::GlyphTooComplex(crate::GlyphId(id.to_u32() as u16));
    let mut pending = vec![(id, 0)];
    let (mut points, mut contours, mut references) = (0usize, 0usize, 0usize);
    while let Some((glyph, depth)) = pending.pop() {
        match loca.get_glyf(glyph, &glyf) {
            Ok(Some(Outline::Simple(simple))) => {
                // skrifa sizes points from the last endpoint and contours from
                // the endpoint count, so a malformed glyph can claim few points
                // and many contours; budget both.
                points += simple.num_points();
                contours += simple.end_pts_of_contours().len();
                if points > MAX_GLYPH_POINTS || contours > MAX_GLYPH_SEGMENTS {
                    return Err(too_complex());
                }
            }
            Ok(Some(Outline::Composite(composite))) => {
                // skrifa sizes a composite at the depth limit before failing on
                // its children, so count its references; descend only below it.
                for (component, _) in composite.component_glyphs_and_flags() {
                    references += 1;
                    if references > MAX_COMPONENT_REFERENCES {
                        return Err(too_complex());
                    }
                    if depth < MAX_COMPOSITE_DEPTH {
                        pending.push((component.into(), depth + 1));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

const SLUG_BAND_EPSILON: f32 = 0.05;

/// Font-unit bounding box with y increasing upward.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x_min: i16,
    /// Bottom edge.
    pub y_min: i16,
    /// Right edge.
    pub x_max: i16,
    /// Top edge.
    pub y_max: i16,
}

impl Rect {
    /// Width in font units; i32 because a legal i16 box can span 65,535 units.
    pub fn width(&self) -> i32 { i32::from(self.x_max) - i32::from(self.x_min) }
    /// Height in font units; i32 for the same reason as [`Self::width`].
    pub fn height(&self) -> i32 { i32::from(self.y_max) - i32::from(self.y_min) }
}

fn draw_settings(location: LocationRef<'_>) -> DrawSettings<'_> {
    DrawSettings::unhinted(Size::unscaled(), location)
        .with_path_style(PathStyle::HarfBuzz)
}

// skrifa prefers VARC outlines when present. They bypass the glyf budget below
// and can panic inside skrifa, and swash shapes without them, so draw glyf, then
// CFF2, then CFF, the order skrifa uses without VARC.
fn outlines<'a>(face: &SkFontRef<'a>) -> Option<OutlineGlyphCollection<'a>> {
    [OutlineGlyphFormat::Glyf, OutlineGlyphFormat::Cff2, OutlineGlyphFormat::Cff]
        .into_iter()
        .find_map(|format| OutlineGlyphCollection::with_format(face, format))
}

fn draw_glyph(face: &SkFontRef<'_>, id: GlyphId, pen: &mut impl OutlinePen, location: LocationRef<'_>) -> Option<()> {
    outlines(face)?.get(id)?.draw(draw_settings(location), pen).ok()?;
    Some(())
}

#[cfg(any(test, feature = "unstable-internals"))]
fn glyph_bounding_box(face: &SkFontRef<'_>, id: GlyphId) -> Option<Rect> {
    glyph_bounding_box_at(face, id, LocationRef::default())
}

fn glyph_bounding_box_at(face: &SkFontRef<'_>, id: GlyphId, location: LocationRef<'_>) -> Option<Rect> {
    let mut pen = ControlBoundsPen::default();
    draw_glyph(face, id, &mut pen, location)?;
    let bounds = pen.bounding_box()?;
    // Bounds enclose control points, including CFF/CFF2 controls, and truncate
    // toward zero to preserve the cached integer font-unit metrics.
    let integer = |v: f32| {
        if v.is_finite() && v.trunc() >= i16::MIN as f32 && v.trunc() <= i16::MAX as f32 {
            Some(v as i16)
        } else {
            None
        }
    };
    Some(Rect {
        x_min: integer(bounds.x_min)?, y_min: integer(bounds.y_min)?,
        x_max: integer(bounds.x_max)?, y_max: integer(bounds.y_max)?,
    })
}

#[cfg(test)]
fn outline_glyph(face: &SkFontRef<'_>, id: GlyphId, pen: &mut impl OutlinePen) -> Option<Rect> {
    outline_glyph_at(face, id, pen, LocationRef::default())
}

fn outline_glyph_at(face: &SkFontRef<'_>, id: GlyphId, pen: &mut impl OutlinePen, location: LocationRef<'_>) -> Option<Rect> {
    draw_glyph(face, id, pen, location)?;
    glyph_bounding_box_at(face, id, location)
}

// Header: min x/y, band width/height, horizontal/vertical counts.
// Descriptors: float offset/length, exact high/low membership thresholds. Curve padding: original index, maximum key.
fn build_slug_bands(curves: &[f32]) -> Vec<f32> {
    let segments = curves.as_chunks::<8>().0;
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for s in segments {
        for axis in 0..2 {
            min[axis] = min[axis].min(s[axis]).min(s[axis + 2]).min(s[axis + 4]);
            max[axis] = max[axis].max(s[axis]).max(s[axis + 2]).max(s[axis + 4]);
        }
    }
    if segments.is_empty() {
        min = [0.; 2];
        max = [0.; 2];
    }
    let counts: [usize; 2] =
        std::array::from_fn(|axis| ((max[axis] - min[axis]) / 64.).floor().clamp(1., 16.) as usize);
    let sizes: [f32; 2] =
        std::array::from_fn(|axis| ((max[axis] - min[axis]) / counts[axis] as f32).max(1.));
    let mut data = vec![0.; 8 * (1 + counts[0] + counts[1])];
    data[..6].copy_from_slice(&[
        min[0],
        min[1],
        sizes[0],
        sizes[1],
        counts[1] as f32,
        counts[0] as f32,
    ]);
    let mut descriptor = 8;
    for axis in [1, 0] {
        for band in 0..counts[axis] {
            let low = min[axis] + band as f32 * sizes[axis] - SLUG_BAND_EPSILON;
            let high = min[axis] + (band + 1) as f32 * sizes[axis] + SLUG_BAND_EPSILON;
            let mut list: Vec<_> = segments
                .iter()
                .enumerate()
                .filter_map(|(index, s)| {
                    let lo = s[axis].min(s[axis + 2]).min(s[axis + 4]);
                    let hi = s[axis].max(s[axis + 2]).max(s[axis + 4]);
                    if lo > high || hi < low {
                        return None;
                    }
                    let other = 1 - axis;
                    let mut record = *s;
                    record[6] = index as f32;
                    record[7] = s[other].max(s[other + 2]).max(s[other + 4]);
                    Some(record)
                })
                .collect();
            list.sort_by(|a, b| b[7].total_cmp(&a[7]).then(a[6].total_cmp(&b[6])));
            data[descriptor] = data.len() as f32;
            data[descriptor + 1] = (list.len() * 8) as f32;
            data[descriptor + 2] = high;
            data[descriptor + 3] = low;
            data.extend(list.into_iter().flatten());
            descriptor += 8;
        }
    }
    data
}

#[cfg(test)]
#[path = "../tests/unit/slug_bands.rs"]
mod slug_band_tests;

// Flip font y-up coordinates once during encoding so shader UVs can be y-down.
// Lines use control=end; cubics are subdivided into quadratic records.
// Contours are closed; glyph winding is normalized from geometry for the shader.
// The shader and atlas keep the same eight-float segment format.
struct BezierBuilder {
    last_position: [f32; 2],
    contour_position: [f32; 2],
    contour_start: usize,
    contours: Vec<usize>,
    pub curves: Vec<f32>,
    total_height: f32,
    /// Set once the glyph exceeds `MAX_GLYPH_SEGMENTS`; further commands are dropped.
    over_budget: bool,
}

const CUBIC_TOLERANCE: f32 = 0.25;

impl BezierBuilder {
    pub fn new(total_height: f32) -> Self {
        Self {
            last_position: [0., 0.],
            contour_position: [0., 0.],
            contour_start: 0,
            contours: Vec::new(),
            curves: Vec::new(),
            total_height,
            over_budget: false,
        }
    }

    fn push_segment(&mut self, segment: [f32; 8]) {
        if self.curves.len() >= MAX_GLYPH_SEGMENTS * 8 {
            self.over_budget = true;
        } else {
            self.curves.extend_from_slice(&segment);
        }
    }

    fn signed_area(&self) -> f64 {
        self.curves
            .as_chunks::<8>()
            .0
            .iter()
            .map(|s| {
                let [ax, ay, bx, by, cx, cy, _, _] = s.map(f64::from);
                (ax * cy - ay * cx) / 6. + (ax * by - ay * bx + bx * cy - by * cx) / 3.
            })
            .sum()
    }

    fn finish(&mut self) {
        self.close();
        // Positive signed area in y-down coordinates is the shader's TrueType
        // convention. Decide from the entire glyph, independent of font tables.
        if self.signed_area() < 0. {
            for (i, &start) in self.contours.iter().enumerate() {
                let end = self
                    .contours
                    .get(i + 1)
                    .copied()
                    .unwrap_or(self.curves.len());
                let segments = self.curves[start..end].as_chunks_mut::<8>().0;
                segments.reverse();
                for segment in segments {
                    let line = segment[2..4] == segment[4..6];
                    segment.swap(0, 4);
                    segment.swap(1, 5);
                    if line {
                        // The shader's straight-line case requires control=end.
                        segment[2] = segment[4];
                        segment[3] = segment[5];
                    }
                }
            }
        }
    }

    // Accept unordered errors too: a NaN must not keep subdividing.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn cubic_to_quadratics(&mut self, p: [[f32; 2]; 4], depth: u8) {
        let control: [f32; 2] =
            std::array::from_fn(|i| (3. * p[1][i] + 3. * p[2][i] - p[0][i] - p[3][i]) / 4.);
        let d: [f32; 2] = std::array::from_fn(|i| -p[0][i] + 3. * p[1][i] - 3. * p[2][i] + p[3][i]);
        // Degree reduction has error D*t*(t-0.5)*(t-1), whose maximum
        // magnitude is |D|/(12*sqrt(3)). Use the conservative bound |D|/12.
        if self.over_budget {
            return;
        }
        if !(d[0].hypot(d[1]) > 12. * CUBIC_TOLERANCE) || depth >= 10 {
            self.push_segment([
                p[0][0], p[0][1], control[0], control[1], p[3][0], p[3][1], 0., 0.,
            ]);
        } else {
            let midpoint = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.];
            let a = midpoint(p[0], p[1]);
            let b = midpoint(p[1], p[2]);
            let c = midpoint(p[2], p[3]);
            let ab = midpoint(a, b);
            let bc = midpoint(b, c);
            let middle = midpoint(ab, bc);
            self.cubic_to_quadratics([p[0], a, ab, middle], depth + 1);
            self.cubic_to_quadratics([middle, bc, c, p[3]], depth + 1);
        }
    }
}

impl OutlinePen for BezierBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.contours.len() >= MAX_GLYPH_SEGMENTS {
            self.over_budget = true;
            return;
        }
        self.last_position = [x, self.total_height - y];
        self.contour_position = [x, y];
        self.contours.push(self.curves.len());
        self.contour_start = self.curves.len();
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let [x0, y0] = self.last_position;
        self.push_segment([
            x0,
            y0,
            x,
            self.total_height - y,
            x,
            self.total_height - y,
            0.,
            0.,
        ]);
        self.last_position = [x, self.total_height - y];
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let [x0, y0] = self.last_position;
        self.push_segment([
            x0,
            y0,
            x1,
            self.total_height - y1,
            x,
            self.total_height - y,
            0.,
            0.,
        ]);
        self.last_position = [x, self.total_height - y];
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.cubic_to_quadratics(
            [
                self.last_position,
                [x1, self.total_height - y1],
                [x2, self.total_height - y2],
                [x, self.total_height - y],
            ],
            0,
        );
        self.last_position = [x, self.total_height - y];
    }

    fn close(&mut self) {
        if self.contour_start < self.curves.len() {
            let [x, y] = self.contour_position;
            if self.last_position != [x, self.total_height - y] {
                // Use the original font-space start, avoiding a second y flip.
                self.line_to(x, y);
            }
            self.contour_start = self.curves.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubic_conversion_chains_and_stays_within_tolerance() {
        // An S curve exercises subdivision even though its midpoint lies on the chord.
        let p = [[10., 20.], [40., 200.], [160., -120.], [210., 30.]];
        let mut builder = BezierBuilder::new(0.);
        builder.move_to(p[0][0], -p[0][1]);
        builder.curve_to(p[1][0], -p[1][1], p[2][0], -p[2][1], p[3][0], -p[3][1]);
        let segments = builder.curves.as_chunks::<8>().0;
        assert!(segments.len() > 1);
        assert_eq!(segments[0][..2], p[0]);
        assert_eq!(segments.last().unwrap()[4..6], p[3]);
        for (i, segment) in segments.iter().enumerate() {
            if i > 0 {
                assert_eq!(segment[..2], segments[i - 1][4..6]);
            }
            assert_eq!(segment[6..], [0., 0.]);
            for sample in 0..=100 {
                let u = sample as f32 / 100.;
                // Midpoint subdivision scales D by 1/8 on either half, so this
                // cubic's accepted segments all cover equal parameter intervals.
                let t = (i as f32 + u) / segments.len() as f32;
                let mut error = [0.; 2];
                for axis in 0..2 {
                    let cubic = (1. - t).powi(3) * p[0][axis]
                        + 3. * (1. - t).powi(2) * t * p[1][axis]
                        + 3. * (1. - t) * t * t * p[2][axis]
                        + t.powi(3) * p[3][axis];
                    let quadratic = (1. - u).powi(2) * segment[axis]
                        + 2. * (1. - u) * u * segment[2 + axis]
                        + u * u * segment[4 + axis];
                    error[axis] = cubic - quadratic;
                }
                assert!(error[0].hypot(error[1]) <= CUBIC_TOLERANCE);
            }
        }
        assert_eq!(builder.last_position, p[3]);
    }

    #[test]
    fn winding_uses_geometry_even_for_implicitly_closed_outlines() {
        for reverse in [false, true] {
            let mut builder = BezierBuilder::new(10.);
            let points = if reverse {
                [[0., 0.], [4., 0.], [4., 4.], [0., 4.]]
            } else {
                [[0., 0.], [0., 4.], [4., 4.], [4., 0.]]
            };
            builder.move_to(points[0][0], points[0][1]);
            for p in &points[1..] {
                builder.line_to(p[0], p[1]);
            }
            builder.close();
            // The hole has opposite winding. It must stay a hole when the
            // decision is made for the entire glyph, not for each contour.
            let hole = if reverse {
                [[1., 1.], [1., 3.], [3., 3.], [3., 1.]]
            } else {
                [[1., 1.], [3., 1.], [3., 3.], [1., 3.]]
            };
            builder.move_to(hole[0][0], hole[0][1]);
            for p in &hole[1..] {
                builder.line_to(p[0], p[1]);
            }
            builder.close();
            let closed = builder.curves.clone();
            builder.finish();
            assert_eq!(builder.signed_area(), 12.);
            if !reverse {
                assert_eq!(builder.curves, closed);
            }
            let finished = builder.curves.clone();
            builder.finish();
            assert_eq!(builder.curves, finished, "must not double-flip");
        }
    }

    #[test]
    fn subdivision_stops_for_nan_and_at_depth_cap() {
        for c in [f32::NAN, 1e30] {
            let mut builder = BezierBuilder::new(0.);
            builder.move_to(0., 0.);
            builder.curve_to(c, 1., -c, 2., 3., 4.);
            assert!(!builder.curves.is_empty());
            assert!(builder.curves.len() / 8 <= 1024);
            if c.is_nan() {
                assert_eq!(builder.curves.len(), 8);
            }
        }
    }

    #[test]
    fn subdivision_accepts_at_depth_cap_despite_large_error() {
        let mut builder = BezierBuilder::new(0.);
        builder.cubic_to_quadratics([[0., 0.], [1e30, 1.], [-1e30, 2.], [3., 4.]], 10);
        assert_eq!(builder.curves.len(), 8);
    }

    #[test]
    fn closing_edge_uses_original_start_without_roundtrip() {
        let mut builder = BezierBuilder::new(1000.);
        // Exact closure is required even for fractional font coordinates.
        builder.move_to(2., 1000.1);
        builder.line_to(3., 4.);
        builder.close();
        assert_eq!(builder.curves[..2], builder.curves[12..14]);
        let curves = builder.curves.clone();
        builder.close();
        assert_eq!(builder.curves, curves);
    }

    #[test]
    fn roboto_bytes_produce_padded_y_down_outline() {
        let (face, _, _) = parse_font(include_bytes!("../fonts/Roboto-Regular.ttf"), 0)
            .expect("bundled font parses");

        let id = face.charmap().map('g').unwrap();
        let bbox = glyph_bounding_box(&face, id).unwrap();
        let height = bbox.y_max as f32;
        let mut builder = BezierBuilder::new(height);
        assert_eq!(outline_glyph(&face, id, &mut builder), Some(bbox));
        let bounds = face.glyph_metrics(Size::unscaled(), LocationRef::default()).bounds(id).unwrap();
        let bearing = face.hmtx().unwrap().side_bearing(id).unwrap() as f32;
        let shift = bearing - bounds.x_min;
        assert_eq!(
            [bearing, bounds.y_min, bounds.x_max + shift, bounds.y_max],
            [bbox.x_min, bbox.y_min, bbox.x_max, bbox.y_max].map(f32::from),
        );
        assert!(!builder.curves.is_empty());
        assert_eq!(builder.curves.len() % 8, 0);
        let bands = build_slug_bands(&builder.curves);
        assert!((0..bands[4] as usize).any(|band|
            (bands[9 + band * 8] as usize) < builder.curves.len()),
            "a typical glyph's band must contain fewer curves than its full list");
        // All three points must be in the flipped font bounding box; padding is zero.
        assert!(builder.curves.as_chunks::<8>().0.iter().all(|segment| {
            segment[6..] == [0., 0.]
                && segment[..6].as_chunks::<2>().0.iter().all(|p| {
                    p[0] >= bbox.x_min as f32
                        && p[0] <= bbox.x_max as f32
                        && p[1] >= 0.
                        && p[1] <= height - bbox.y_min as f32
                })
        }));
        assert!(builder
            .curves
            .as_chunks::<8>()
            .0
            .iter()
            .any(|s| s[2..4] != s[4..6]));
    }

    #[test]
    fn invalid_bytes_and_missing_face_return_loading_error() {
        for bytes in [
            &[][..],
            &b"not a font"[..],
            &include_bytes!("../fonts/Roboto-Regular.ttf")[..32],
        ] {
            assert!(matches!(
                parse_font(bytes, 0),
                Err(LoadingError::InvalidFont)
            ));
        }
        assert!(matches!(
            parse_font(include_bytes!("../fonts/Roboto-Regular.ttf"), 1),
            Err(LoadingError::InvalidFont)
        ));
    }

    #[test]
    fn line_and_quadratic_encoding_preserves_endpoints() {
        let mut builder = BezierBuilder::new(20.);
        builder.move_to(1., 2.);
        builder.line_to(3., 4.);
        builder.quad_to(5., 6., 7., 8.);
        assert_eq!(
            builder.curves,
            [1., 18., 3., 16., 3., 16., 0., 0., 3., 16., 5., 14., 7., 12., 0., 0.]
        );
    }
}

#[cfg(test)]
#[path = "../tests/unit/cff.rs"]
mod cff_tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "../tests/unit/analytic_regressions.rs"]
mod analytic_regressions;

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "../tests/unit/ray_review.rs"]
mod ray_review;

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "../tests/unit/font_loading.rs"]
mod font_loading_tests;

#[cfg(test)]
mod closure_counts {
    use super::*;

    #[test]
    fn bundled_cache_counts_before_and_after_closure() {
        let ascii: String = (' '..='~').collect();
        for (name, bytes, preset) in [
            (
                "Roboto ASCII",
                include_bytes!("../fonts/Roboto-Regular.ttf").as_slice(),
                ascii.as_str(),
            ),
            (
                "Cantarell ASCII",
                include_bytes!("../fonts/Cantarell-VF.otf").as_slice(),
                ascii.as_str(),
            ),
            (
                "CJK subset",
                include_bytes!("../fonts/NotoSerifSC-Subset.otf").as_slice(),
                "龍鬱齉直接从轮廓渲染字形文本",
            ),
        ] {
            let (face, offset, key) = parse_font(bytes, 0).unwrap();
            let font = FontRef {
                data: bytes,
                offset,
                key,
            };
            let before = glyph_ids(font, preset)
                .filter(|&id| glyph_bounding_box(&face, id).is_some())
                .count();
            let ids = closure_glyph_ids(&face, font, preset).unwrap();
            let outlined = ids
                .iter()
                .filter(|&&id| glyph_bounding_box(&face, id).is_some())
                .count();
            let after = ids.len();
            eprintln!("{name}: cached {before} -> {after} ({outlined} outlined after closure)");
            assert!(after >= before);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32"), feature = "unstable-internals"))]
#[path = "../tests/unit/coverage_continuity.rs"]
mod coverage_continuity;

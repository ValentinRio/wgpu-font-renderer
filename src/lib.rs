#![warn(missing_docs)]
//! Render scalable font outlines with wgpu on native platforms and WebGPU.
//!
//! [`FontStore`] loads a font and writes the selected glyph outlines into a float
//! atlas. For a well-formed font that shapes with GSUB, every glyph shaping can
//! produce from the preset characters is cached, including ligatures, contextual
//! forms and normalized accents; `morx` substitutions are not cached.
//! [`TypeWriter`] shapes text by script into paragraphs containing glyph IDs
//! and pixel advances and offsets, returning [`LayoutError`] on invalid input.
//! [`TypeWriter::layout_text`] shapes and wraps
//! hard-break-delimited paragraphs into a [`TextBlock`] using [`TextLayout`], Unicode break opportunities and font
//! metrics. [`TextRenderer`] prepares glyph instances and draws them
//! with shader-computed coverage, without rasterizing glyphs on the CPU.
//! Coverage is the clamped absolute signed winding integral over each pixel
//! box. For ordinary contours without overlap this is the ink area. It is not
//! the nonzero union area: same-direction contours that overlap partly within
//! a pixel can overcount before clamping. Opposite-direction contours subtract
//! their signed areas. This overlap limitation also existed in the previous
//! centre-ray method. Very small thin strokes can appear lighter with area
//! integration because their coverage reflects their actual ink area.
//!
//! Cubic outlines are approximated by quadratics within 0.25 font units, with a
//! depth cap of 10 for pathological inputs that may exceed this tolerance.
//! CFF2 outlines can be loaded and converted. [`FontStore::load_variation`] selects
//! axis overrides from [`FontStore::axes`], inheriting the base location and sharing bytes. One
//! normalized location drives outlines, placement, metrics and shaping. Default
//! loads select the default instance, including zero coordinates for FeatureVariations.
//! [`FontStore::metrics`] reports pixel line
//! metrics; the baseline is paragraph y plus [`FontMetrics::ascent`].
//! Contours are explicitly closed. Glyphs with negative total signed area in
//! y-down coordinates have all contours reversed to match the shader's winding.
//! CFF1 outlines use the same path, tested with fonts/NotoSerifSC-Subset.otf.
//! Coverage uses the nonzero winding rule, including overlapping
//! contours and opposite-direction holes.
//!
//! Screen coordinates are physical pixels, with the origin at the top left,
//! x increasing rightward and y downward. Font sizes are pixels per em. Outline
//! coordinates remain in font units until drawing. Paragraph positions anchor
//! both x and y. Shaping retains every glyph and its offsets in logical run order
//! without bidi reordering. Glyphs absent from the cache selected by `chars` are skipped during drawing.
//!
//! Glyph zero is cached, so missing letters and symbols draw `.notdef` if it has an outline.
//! Output glyph zero from controls (Cc) or default-ignorables is omitted without advance;
//! visible substitutions and ligatures are retained.
//! An empty `.notdef` advances without drawing. Loads that exhaust the atlas
//! return [`LoadingError::AtlasFull`] without adding the font or affecting earlier
//! fonts. Each outline or band stream must fit one logical layer (4,194,304
//! floats, or 16 MiB). Logical layers are append-only, up to the device array-layer
//! limit. Their used prefixes are packed together into a 512-texel-wide RGBA32Float
//! array. Rows are distributed across physical layers, leaving at most one 8 KiB
//! row unused per physical layer. The empty atlas uses one row, not full layers.
//! A small storage buffer maps stable logical addresses into packed texture rows.
//! A successful load replaces the texture and copies earlier payloads; growth
//! temporarily retains both textures. Failed loads discard staged writes and
//! leave earlier fonts, allocations and GPU resources intact.
//!
//! Outlines come from `glyf`, then `CFF2`, then `CFF`; `VARC` is ignored. Loading
//! enforces per-glyph work limits (65,535 points and 16,384 contours after
//! composite expansion, 4,096 component references, 16,384 encoded segments);
//! a glyph past them gives [`LoadingError::GlyphTooComplex`]. The limits are
//! loader policy, so a valid but very complex glyph can be refused. Malformed GSUB tables are rejected
//! at load or stopped by the closure cap ([`LoadingError::SubstitutionTooComplex`]).
//! The cache guarantee does not extend to crafted fonts. Known upstream limits:
//! swash's `cmap` and `ClassDef` arithmetic can panic in debug builds on crafted
//! data; swash misreads null FeatureVariations substitution offsets and can
//! compose an unexpected Hangul character, emitting glyphs the closure did not
//! cache (they draw nothing); skrifa does not bound total CFF work, so a crafted
//! CFF glyph can load slowly with bounded memory.
//! Glyph ink is placed at the pen plus its shaped offset plus the cached left
//! side bearing scaled to pixels. In 0.2, each glyph's ink was anchored at the
//! pen; text now shifts by each glyph's bearing. TrueType outlines apply
//! `hmtx.lsb - glyf.xMin`, so the cached box and bearing follow `hmtx`.
//! CFF outline coordinates are unchanged; their bearings also position the ink.
//! Both grayscale and LCD scale coverage by RGBA
//! alpha; alpha zero draws nothing. In 0.2, grayscale ignored alpha.
//!
//! Use `RenderMode::Cached` for static text drawn many times at the same pixel size.
//! Bitmap resources are created lazily on the first cached preparation.
//! Unchanged cached paragraphs reuse their prepared instances without uploads.
//! Set it with `Paragraph::set_render_mode` or `TextBlock::set_render_mode`; blocks
//! apply it to every line. The GPU rasterizes misses with the same analytic coverage
//! function into a padded R8 atlas. Cached pens snap x down to a quarter pixel.
//! Each line’s baseline (paragraph y plus font ascent) snaps to the nearest whole
//! pixel, with the same vertical correction applied to all its glyphs. Exact sizes
//! and font variations have separate entries.
//! The atlas grows by doubling and copying existing coverage, evicts glyphs unused
//! in the current preparation at its limit, and draws analytically when full.
//!
//! `RenderMode::Analytic` remains the default, preserving exact positions and existing
//! output. Prefer it for scaling, animation, large sizes and LCD. When LCD is active,
//! Cached requests draw analytically. Bitmap coverage uses unfiltered texel loads
//! and scales by the paragraph colour's alpha.
//!
//! Call [`TextRenderer::resize`] before the first render and on every viewport
//! change. Later font loads are picked up by [`TextRenderer::prepare`].
//!
//! ```no_run
//! use wgpu_font_renderer::{FontStore, TextRenderer, TypeWriter};
//!
//! # fn draw(device: &wgpu::Device, queue: &wgpu::Queue,
//! # config: &wgpu::SurfaceConfiguration, view: &wgpu::TextureView)
//! # -> Result<(), wgpu_font_renderer::LoadingError> {
//! let mut fonts = FontStore::new(device);
//! // On wasm, embed bytes with include_bytes! rather than reading a file.
//! let bytes = std::fs::read("fonts/Roboto-Regular.ttf").expect("read font");
//! let key = fonts.load_from_bytes(device, queue, &bytes, "Hello")?;
//! let metrics = fonts.metrics(key, 32.0).expect("valid font and size");
//! let baseline_y = 24.0 + metrics.ascent;
//! let axes = fonts.axes(key).expect("loaded font");
//! let bold = fonts.load_variation(device, queue, key, &[(*b"wght", 700.0)])?;
//! let paragraph = TypeWriter::new()
//!     .shape_text(&fonts, key, [24.0, 24.0], 32.0, [0.0, 0.0, 0.0, 1.0], "Hello")
//!     .expect("font is loaded");
//! let mut renderer = TextRenderer::new(device, config.format, &fonts);
//! renderer.prepare(device, queue, &[paragraph], &fonts);
//! renderer.resize(queue, [config.width, config.height]);
//! let mut encoder = device.create_command_encoder(&Default::default());
//! {
//!     let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
//!         color_attachments: &[Some(wgpu::RenderPassColorAttachment {
//!             view, depth_slice: None, resolve_target: None,
//!             ops: wgpu::Operations {
//!                 load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store,
//!             },
//!         })],
//!         ..Default::default()
//!     });
//!     renderer.render(&mut pass);
//! }
//! queue.submit(Some(encoder.finish()));
//! # Ok(())
//! # }
//! ```

//! Wrapped blocks can be passed directly to `prepare`; their lines are read-only.
//! ```no_run
//! use wgpu_font_renderer::{Align, FontId, FontStore, TextLayout, TextRenderer, TypeWriter};
//! # fn wrapped(device: &wgpu::Device, queue: &wgpu::Queue,
//! # fonts: &FontStore, font: FontId, renderer: &mut TextRenderer) {
//! let layout = TextLayout::default().with_max_width(240.).with_align(Align::Center);
//! let mut block = TypeWriter::new().layout_text(
//!     fonts, font, [24., 24.], 32., [0., 0., 0., 1.],
//!     "Text wraps at Unicode line-break opportunities.", &layout,
//! ).expect("valid font, size and layout");
//! block.set_position([40., 24.]);
//! renderer.prepare(device, queue, &block, fonts);
//! # }
//! ```
//!
//! Every stable public type can be named through this crate or wgpu alone.
//! ```
//! fn public_types(
//!     _: Option<wgpu_font_renderer::Axis>,
//!     _: Option<wgpu_font_renderer::FontMetrics>,
//!     _: Option<wgpu_font_renderer::FontId>,
//!     _: wgpu_font_renderer::GlyphId,
//!     _: Option<wgpu_font_renderer::ShapedGlyph>,
//!     _: Option<wgpu_font_renderer::FontStore>,
//!     _: Option<wgpu_font_renderer::LoadingError>,
//!     _: Option<wgpu_font_renderer::Paragraph>,
//!     _: Option<wgpu_font_renderer::TextBlock>,
//!     _: wgpu_font_renderer::TextLayout,
//!     _: wgpu_font_renderer::Align,
//!     _: wgpu_font_renderer::LineHeight,
//!     _: wgpu_font_renderer::TypeWriter,
//!     _: wgpu_font_renderer::RenderMode,
//!     _: wgpu_font_renderer::SubpixelOrder,
//!     _: Option<wgpu_font_renderer::TextRenderer>,
//!     _: Option<wgpu::TextureFormat>,
//! ) {}
//! public_types(None, None, None, wgpu_font_renderer::GlyphId(0), None, None, None, None,
//!     None, wgpu_font_renderer::TextLayout::default(),
//!     wgpu_font_renderer::Align::Left, wgpu_font_renderer::LineHeight::Font,
//!     wgpu_font_renderer::TypeWriter::default(),
//!     wgpu_font_renderer::RenderMode::default(),
//!     wgpu_font_renderer::SubpixelOrder::None, None, None);
//! ```

#[cfg(test)]
extern crate self as wgpu_font_renderer;

mod loader;
mod store;
mod atlas;
mod renderer;
mod typewriter;
mod gsub;
mod ortho;
pub use renderer::{RenderMode, SubpixelOrder, TextRenderer};
pub use store::{Axis, FontId, FontMetrics, FontStore};
pub use loader::LoadingError;
pub use typewriter::TypeWriter;
pub use typewriter::{Align, GlyphId, LayoutError, LineHeight, Paragraph, ShapedGlyph, TextBlock, TextLayout};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod atlas_tests;

/// Inspection helpers for tests and benchmarks; exempt from semver guarantees.
#[cfg(feature = "unstable-internals")]
#[doc(hidden)]
pub mod internals {
    pub use crate::loader::{Font, Glyph};

    /// Run the loader CPU path without uploading; returns how many outlined glyphs were prepared.
    pub fn check_font_bytes(data: &[u8], characters: &str) -> Result<usize, crate::LoadingError> {
        crate::loader::check_font_bytes(data, characters)
    }

    /// Shape a bundled test face without a GPU atlas.
    pub fn shape_font_bytes(writer: &mut crate::TypeWriter, data: &[u8], text: &str) {
        writer.shape_glyphs(swash::FontRef::from_index(data, 0).expect("valid test font"), 32., text);
    }
    pub use crate::atlas::allocation::Allocation;

    /// Reset the bitmap cache with a small initial and maximum size for tests.
    pub fn set_bitmap_cache_limits(renderer: &mut crate::TextRenderer, device: &wgpu::Device, initial: u32, max: u32) {
        renderer.set_bitmap_cache_limits(device, initial, max);
    }
    /// Current bitmap texture dimension (one byte per texel), or zero before initialization.
    pub fn bitmap_cache_size(renderer: &crate::TextRenderer) -> u32 { renderer.bitmap_cache_size() }
    /// Number of resident bitmap glyphs and cached instances in the frame.
    pub fn bitmap_cache_counts(renderer: &crate::TextRenderer) -> (usize, usize) { renderer.bitmap_cache_counts() }
    /// Inspect a loaded font.
    pub fn font(store: &crate::FontStore, id: crate::FontId) -> Option<&Font> {
        store.get(id)
    }
    /// Atlas texture and logical-address buffer for shader probes.
    pub fn atlas_resources(store: &crate::FontStore) -> (&wgpu::TextureView, &wgpu::Buffer) {
        store.atlas().resources()
    }
    /// Number of bytes of outline and band payload uploaded, excluding row padding.
    pub fn atlas_written_bytes(store: &crate::FontStore) -> u64 {
        store.atlas().written_bytes()
    }
    /// Bytes of outline and band payload uploaded by one font instance.
    pub fn font_written_bytes(store: &crate::FontStore, id: crate::FontId) -> Option<[u64; 2]> {
        store.get(id).map(|font| {
            font.glyph_cache.values().fold([0, 0], |mut sum, g| {
                sum[0] += u64::from(g.allocation.size()) * 4;
                sum[1] += u64::from(g.bands.size()) * 4;
                sum
            })
        })
    }
    /// Inspect the current atlas view.
    pub fn atlas_view(store: &crate::FontStore) -> &wgpu::TextureView {
        store.atlas().view()
    }
    /// Inspect the number of logical atlas layers.
    pub fn atlas_layer_count(store: &crate::FontStore) -> usize {
        store.atlas().layer_count()
    }
    /// Reserve test padding in curve and band layers to exercise atlas growth.
    pub fn pad_atlas(device: &wgpu::Device, queue: &wgpu::Queue, store: &mut crate::FontStore, floats: [u32; 2]) {
        store.pad_atlas(device, queue, floats);
    }
    /// Inspect the number of prepared glyph instances.
    pub fn instance_count(renderer: &crate::TextRenderer) -> usize {
        renderer.instance_count()
    }
    /// Inspect the buffer identity to check instance allocation reuse.
    pub fn instance_buffer(renderer: &crate::TextRenderer) -> Option<&wgpu::Buffer> {
        renderer.instance_buffer()
    }
}

// Compile the migration example without adding the changelog to public rustdoc.
#[cfg(doctest)]
#[doc = include_str!("../CHANGELOG.md")]
mod migration {}

#![warn(missing_docs)]
//! Render scalable font outlines with wgpu on native platforms and WebGPU.
//!
//! [`FontStore`] loads a font and writes the selected glyph outlines into a float
//! atlas. [`TypeWriter`] shapes text by script into paragraphs containing glyph IDs
//! and pixel advances and offsets. [`TextRenderer`] prepares glyph instances and draws them
//! with shader-computed coverage, without rasterizing glyphs on the CPU.
//! Cubic outlines are approximated by quadratics within 0.25 font units, with a
//! depth cap of 10 for pathological inputs that may exceed this tolerance.
//! CFF2 outlines can be loaded and converted. [`FontStore::load_variation`] selects
//! axis values from [`FontStore::axes`], sharing bytes with the base font. One
//! normalized location drives outlines, placement, metrics and shaping. Default
//! loads select the default instance. [`FontStore::metrics`] reports pixel line
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
//! Glyph zero is cached if it has an outline, so missing characters draw `.notdef`.
//! An empty `.notdef` advances without drawing. Loads that exhaust the atlas
//! return [`LoadingError::AtlasFull`] without adding the font or affecting earlier
//! fonts. An outline or its bands must fit a layer, and allocation must stay
//! within the device's `max_texture_array_layers`. The default atlas uses 16 MiB
//! per 512 × 2048 RGBA32Float layer, initially 32 MiB; 256 layers use 4 GiB.
//! Growth temporarily retains both old and new textures. A load that grows the
//! atlas and then fails restores allocations but keeps the larger texture.
//!
//! Outlines come from `glyf`, then `CFF2`, then `CFF`; `VARC` is ignored. Loading
//! enforces per-glyph work limits (65,535 points and 16,384 contours after
//! composite expansion, 4,096 component references, 16,384 encoded segments);
//! a glyph past them gives [`LoadingError::GlyphTooComplex`]. The limits are
//! loader policy, so a valid but very complex glyph can be refused. Upstream, swash's `cmap` lookup can
//! panic on crafted data in debug builds, and skrifa does not bound total CFF
//! work, so a crafted CFF glyph can load slowly with bounded memory.
//! Glyph ink is placed at the pen plus its shaped offset plus the cached left
//! side bearing scaled to pixels. In 0.2, each glyph's ink was anchored at the
//! pen; text now shifts by each glyph's bearing. TrueType outlines apply
//! `hmtx.lsb - glyf.xMin`, so the cached box and bearing follow `hmtx`.
//! CFF outline coordinates are unchanged; their bearings also position the ink.
//! Both grayscale and LCD scale coverage by RGBA
//! alpha; alpha zero draws nothing. In 0.2, grayscale ignored alpha.
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
//!     _: wgpu_font_renderer::TypeWriter,
//!     _: wgpu_font_renderer::SubpixelOrder,
//!     _: Option<wgpu_font_renderer::TextRenderer>,
//!     _: Option<wgpu::TextureFormat>,
//! ) {}
//! public_types(None, None, None, wgpu_font_renderer::GlyphId(0), None, None, None, None,
//!     wgpu_font_renderer::TypeWriter::default(),
//!     wgpu_font_renderer::SubpixelOrder::None, None, None);
//! ```

#[cfg(test)]
extern crate self as wgpu_font_renderer;

mod loader;
mod store;
mod atlas;
mod renderer;
mod typewriter;
mod ortho;
pub use renderer::{SubpixelOrder, TextRenderer};
pub use store::{Axis, FontId, FontMetrics, FontStore};
pub use loader::LoadingError;
pub use typewriter::TypeWriter;
pub use typewriter::{GlyphId, Paragraph, ShapedGlyph};

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

    /// Inspect a loaded font.
    pub fn font(store: &crate::FontStore, id: crate::FontId) -> Option<&Font> {
        store.get(id)
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

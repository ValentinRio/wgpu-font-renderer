#![warn(missing_docs)]
//! Render scalable font outlines with wgpu on native platforms and WebGPU.
//!
//! [`FontStore`] loads a font and writes the selected glyph outlines into a float
//! atlas. [`TypeWriter`] shapes Latin text into paragraphs containing glyph IDs
//! and pixel advances. [`TextRenderer`] prepares glyph instances and draws them
//! with shader-computed coverage, without rasterizing glyphs on the CPU.
//! Cubic outlines are approximated by quadratics within 0.25 font units, with a
//! depth cap of 10 for pathological inputs that may exceed this tolerance.
//! CFF2 outlines can be loaded and converted. Variable fonts use their default
//! variation instance; the loading API does not select variation axis values.
//! Contours are explicitly closed. Glyphs with negative total signed area in
//! y-down coordinates have all contours reversed to match the shader's winding.
//! CFF1 outlines use the same path, tested with fonts/NotoSerifSC-Subset.otf.
//! Coverage uses the nonzero winding rule, including overlapping
//! contours and opposite-direction holes.
//!
//! Screen coordinates are physical pixels, with the origin at the top left,
//! x increasing rightward and y downward. Font sizes are pixels per em. Outline
//! coordinates remain in font units until drawing. Paragraph positions anchor
//! both x and y. Shaping retains only the first glyph of each cluster,
//! and glyphs absent from the cache preset are skipped during drawing.
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
//!     _: Option<wgpu_font_renderer::FontId>,
//!     _: wgpu_font_renderer::GlyphId,
//!     _: Option<wgpu_font_renderer::FontStore>,
//!     _: Option<wgpu_font_renderer::LoadingError>,
//!     _: Option<wgpu_font_renderer::Paragraph>,
//!     _: wgpu_font_renderer::TypeWriter,
//!     _: wgpu_font_renderer::SubpixelOrder,
//!     _: Option<wgpu_font_renderer::TextRenderer>,
//!     _: Option<wgpu::TextureFormat>,
//! ) {}
//! public_types(None, wgpu_font_renderer::GlyphId(0), None, None, None,
//!     wgpu_font_renderer::TypeWriter::default(),
//!     wgpu_font_renderer::SubpixelOrder::None, None, None);
//! ```

mod loader;
mod store;
mod atlas;
mod renderer;
mod typewriter;
mod ortho;
pub use renderer::{SubpixelOrder, TextRenderer};
pub use store::{FontId, FontStore};
pub use loader::LoadingError;
pub use typewriter::TypeWriter;
pub use typewriter::{GlyphId, Paragraph};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod atlas_tests;

/// Inspection helpers for tests and benchmarks; exempt from semver guarantees.
#[cfg(feature = "unstable-internals")]
#[doc(hidden)]
pub mod internals {
    pub use crate::loader::{Font, Glyph};
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

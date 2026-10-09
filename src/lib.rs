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
//! CFF1 (.otf with a CFF table) uses the same path but has no fixture and is
//! untested. Overlapping contours (common in variable fonts) and self-intersections
//! may render as holes because the shader requires an exact winding count.
//!
//! Screen coordinates are physical pixels, with the origin at the top left,
//! x increasing rightward and y downward. Font sizes are pixels per em. Outline
//! coordinates remain in font units until drawing. Paragraph positions anchor
//! both x and y. Shaping retains only the first glyph of each cluster,
//! and glyphs absent from the cache preset are skipped during drawing.
//!
//! Load all required outlines before constructing the renderer: its atlas bind
//! group is not refreshed when subsequent loads grow the atlas. Use nonzero
//! viewport dimensions and update uniforms whenever the viewport changes.
//!
//! ```no_run
//! use wgpu_font_renderer::{FontStore, TextRenderer, TypeWriter};
//!
//! # fn draw(device: &wgpu::Device, queue: &wgpu::Queue,
//! # config: &wgpu::SurfaceConfiguration, view: &wgpu::TextureView)
//! # -> Result<(), wgpu_font_renderer::LoadingError> {
//! let mut fonts = FontStore::new(device, config);
//! // On wasm, embed bytes with include_bytes! rather than reading a file.
//! let bytes = std::fs::read("Roboto-Regular.ttf").expect("read font");
//! let key = fonts.load_from_bytes(device, queue, &bytes, "Hello")?;
//! let paragraph = TypeWriter::new()
//!     .shape_text(&fonts, key, [24.0, 24.0], 32, [0.0, 0.0, 0.0, 1.0], "Hello")
//!     .expect("font is loaded");
//! let mut renderer = TextRenderer::new(device, config, fonts.atlas());
//! renderer.prepare(device, &vec![paragraph], &fonts);
//! renderer.update_uniforms(device, [config.width, config.height]);
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
//!     renderer.render(&mut pass, [config.width, config.height]);
//! }
//! queue.submit(Some(encoder.finish()));
//! # Ok(())
//! # }
//! ```

mod loader;
mod store;
mod atlas;
mod renderer;
mod typewriter;
mod ortho;
pub use renderer::TextRenderer;
pub use store::FontStore;
pub use loader::LoadingError;
pub use typewriter::TypeWriter;
pub use typewriter::Paragraph;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod atlas_tests;

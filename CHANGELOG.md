# Changelog

All notable changes to this crate are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the version is
below 1.0, a minor bump (0.x) may break the API.

## [Unreleased]

### Breaking

- The library no longer enables any `wgpu` backend. Applications must enable the backends they want on their own `wgpu` dependency; a plain `wgpu = "30"` enables the defaults.
- Font handles returned by `load` and `load_from_bytes` are opaque `FontId` values.
- Glyph indices in paragraphs use the root-exported `GlyphId(pub u16)`.
- `FontStore::new` takes only a device.
- `FontStore::load` accepts any `AsRef<Path>` path and returns `FontId`.
- `FontStore::load_from_bytes` returns `FontId`.
- `LoadingError` is non-exhaustive, with `Io(std::io::Error)` and `InvalidFont`; it implements `Error` and preserves the I/O source.
- `Paragraph` fields are private; use getters, `set_position`, and `set_color`.
- `Paragraph::new` takes the font first, followed by position, size, and color.
- `Paragraph::append` is renamed to `push`.
- Font sizes in `Paragraph` and `TypeWriter::shape_text` are `f32` pixels per em.
- `SubpixelOrder` is non-exhaustive; include a fallback arm when matching.
- `TextRenderer::new` takes a texture format and `&FontStore` instead of a surface configuration and atlas.
- `TextRenderer::new_with_subpixel_order` becomes `with_subpixel_order`, with the same format/store change.
- `TextRenderer::update_uniforms` becomes `resize`, takes a queue, and must be called before the first render and on every viewport change.
- `TextRenderer::prepare` also takes a queue, skips unknown fonts and invalid sizes, refreshes the atlas binding, and reuses instance buffers. Call it once per queue submit with every paragraph of the frame: a second preparation before submit can change what earlier render calls draw. Use `render_range` for layered or multi-pass drawing.
- `TextRenderer::render` takes `&self` and a render pass without a size argument or lifetime tie to the renderer, and preserves the pass scissor rectangle.
- `Font`, `Glyph`, `Atlas`, `Allocation`, `Resolution`, `Params`, `FontStore::get`, and `FontStore::atlas` leave the stable API; limited inspection is available only through the hidden, semver-exempt `internals` module with `unstable-internals`.

### Migrating from 0.2

Before (0.2):

```rust,ignore
let mut store = FontStore::new(device, config);
let key = store.load(device, queue, "font.otf", "Hi")?; // external cache key
let memory_key = store.load_from_bytes(device, queue, bytes, "Hi")?;
let mut writer = TypeWriter::new(); // Default is also available
let mut paragraph = Paragraph::new([24., 24.], 32, [0., 0., 0., 1.], key);
paragraph.append(owned_ttf_parser::GlyphId(1), 12.);
paragraph.position = [40., 24.];
paragraph.color = [1.; 4];
let metrics = (paragraph.font_key, paragraph.position, paragraph.size,
    paragraph.color, paragraph.width, &paragraph.glyphs);
let shaped = writer.shape_text(&store, memory_key, [24., 24.], 32, [1.; 4], "Hi");
let mut gray = TextRenderer::new(device, config, store.atlas());
let mut renderer = TextRenderer::new_with_subpixel_order(
    device, config, store.atlas(), SubpixelOrder::Rgb); // None and Bgr also exist
let lcd = renderer.lcd_active();
renderer.update_uniforms(device, [config.width, config.height]);
renderer.prepare(device, &[paragraph], &store);
renderer.render(pass, [config.width, config.height]);
match error {
    LoadingError::FileNotFound => {},
    LoadingError::InvalidFile => {},
}
let font = store.get(key).unwrap(); // Font: face, offset, key, glyph_cache, as_ref()
let glyph = font.glyph_cache.values().next().unwrap();
let outlines = (&glyph.curves, &glyph.bands);
let glyph_metrics = (glyph.bbox, glyph.descent, glyph.y_offset, glyph.left_side_bearing);
let atlas = store.atlas(); // Atlas: view(), layer_count(), upload(), upload_bands(), texture_format
let allocation = &glyph.allocation; // Allocation: position(), size(), layer(), layer, region
// Font::from_file/from_bytes and Atlas::new supported direct internal loading.
// Resolution and Params described the renderer's GPU uniform layout.
```

After (compiled as a doctest):

```rust,no_run
use wgpu_font_renderer::{FontId, GlyphId, FontStore, LoadingError,
    Paragraph, SubpixelOrder, TextRenderer, TypeWriter};
fn migrate(
    device: &wgpu::Device, queue: &wgpu::Queue,
    config: &wgpu::SurfaceConfiguration, bytes: &[u8],
    error: LoadingError, pass: &mut wgpu::RenderPass<'_>,
) -> Result<(), LoadingError> {
let mut store = FontStore::new(device);
let font: FontId = store.load(device, queue, "font.otf", "Hi")?;
let memory_font = store.load_from_bytes(device, queue, bytes, "Hi")?;
let mut writer = TypeWriter::default(); // new() remains available
let mut paragraph = Paragraph::new(font, [24., 24.], 32., [0., 0., 0., 1.]);
paragraph.push(GlyphId(1), 12.);
paragraph.set_position([40., 24.]);
paragraph.set_color([1.; 4]);
let metrics = (paragraph.font(), paragraph.position(), paragraph.size(),
    paragraph.color(), paragraph.width(), paragraph.glyphs());
let shaped = writer.shape_text(&store, memory_font, [24., 24.], 32., [1.; 4], "Hi");
let mut gray = TextRenderer::new(device, config.format, &store);
let mut renderer = TextRenderer::with_subpixel_order(
    device, config.format, &store, SubpixelOrder::Rgb); // None and Bgr unchanged
let lcd = renderer.lcd_active();
renderer.resize(queue, [config.width, config.height]);
renderer.prepare(device, queue, &[paragraph], &store);
// Set the scissor rectangle yourself if needed.
renderer.render(pass);
match error {
    LoadingError::Io(error) => { /* std::error::Error::source preserves this */ },
    LoadingError::InvalidFont => {},
    _ => {},
}
// Tests/benchmarks only, with unstable-internals enabled:
# #[cfg(feature = "unstable-internals")]
# {
let font = wgpu_font_renderer::internals::font(&store, font).unwrap();
let glyph = font.glyph_cache.values().next().unwrap();
let curves = &glyph.curves;
let glyph_metrics = (glyph.bbox, glyph.descent, glyph.y_offset, glyph.left_side_bearing);
let allocation = &glyph.allocation; // position(), size(), layer() remain inspectable
let bands = &glyph.bands;
let face = font.face();
let view = wgpu_font_renderer::internals::atlas_view(&store);
let layers = wgpu_font_renderer::internals::atlas_layer_count(&store);
# }
// Direct Font/Atlas construction, uploading, parser references, allocation mutation,
// and Resolution/Params have no stable replacement: load through FontStore,
// use Paragraph for placement, and resize for the uniform viewport dimensions.
Ok(())
}
```

`atlas::SIZE`, `Allocator::{new, allocate}`, `Region::{position, size}`, and
`Layer::{Empty, Busy, is_empty}` have no stable replacement; atlas allocation
and layer bookkeeping are now private. `Resolution::{width, height}` also
leave the stable API; pass viewport dimensions to `TextRenderer::resize`.

`Font::{offset, key, as_ref}` and `Atlas::texture_format` have no stable
replacement: identify fonts with `FontId` and pass the render target format
to `TextRenderer::new`. `LoadingError` retains its `Display` implementation.

Run tests and benchmarks with `--all-features` to enable inspection targets.

### Added

- `TextRenderer::render_range` draws subsets of the prepared paragraphs with stable indices.
- CI checks for GPU tests on Mesa lavapipe, lint warnings, documentation, MSRV,
  wasm builds, packaged tests and semantic versioning.

### Changed

- Fonts are read with skrifa instead of ttf-parser. swash is at 0.2 without its scaler and rasteriser.
- Characters that map to glyph 0 (unmapped characters or an explicit mapping to `.notdef`) are no longer cached or drawn. In 0.2, they drew `.notdef` if it had an outline.
- Glyph IDs now use the shaper's cmap selection, which prioritizes Windows symbol subtables and prefers format 12. In 0.2, ttf-parser skipped Windows symbol subtables and used the first Unicode subtable in record order. Symbol-only fonts now draw, and fonts with conflicting format 4 and format 12 mappings now load the same glyphs the shaper selects, avoiding the wrong or missing glyphs possible in 0.2.
- `TextRenderer::prepare` accepts any iterator of borrowed paragraphs, including slices, borrowed vectors, and chained lists.
- `TypeWriter` implements `Default`, equivalent to `TypeWriter::new()`.

## [0.2.1] - 2026-10-13

No API or rendering changes.

### Added

- A CFF (CFF1) test font: a 14-character subset of Noto Serif SC. The coverage
  tests now cover dense CJK glyphs, with their own frozen error bounds.
- The `simple` example draws three fonts: Roboto (TrueType), Cantarell (CFF2)
  and Noto Serif SC (CFF).

### Changed

- Every font used by the examples, tests and benchmark now lives in `fonts/`,
  which ships in the package with each font's license.
- The README is shorter, with a new screenshot. The benchmarking guide moved to
  `benches/README.md`.

## [0.2.0] - 2026-10-13

### Breaking

- **wgpu 0.20 → 30.** wgpu types are part of this crate's public API
  (`Device`, `Queue`, `SurfaceConfiguration`, `RenderPass`, `TextureView`), so
  your application must use wgpu 30 as well.
- **Minimum supported Rust version is now 1.88** (`rust-version` in `Cargo.toml`).
- **Paragraphs are placed vertically from `position[1]`.** 0.1.0 read
  `position[0]` for both axes, so text whose x and y differ moves.
- **Rendering output changed.** Coverage is now computed analytically by root
  counting (the Slug method) instead of a distance estimate, with the nonzero
  winding rule. Glyph edges and overlapping contours render differently, and
  pixel-exact comparisons against 0.1.0 will not match.
- `Glyph`, reached through `FontStore::get`, has a new public field `bands`.

### Added

- `FontStore::load_from_bytes` loads a font from memory. Use it on wasm, where
  there is no filesystem.
- WebGPU in the browser (`wasm32-unknown-unknown`), with a web example.
- OpenType fonts with cubic outlines (CFF and CFF2). Cubics are split into
  quadratics within 0.25 font units. CFF1 has no test font yet.
- Variable fonts load at their default instance. Axis selection is not exposed.
- Opt-in subpixel (LCD) anti-aliasing: `SubpixelOrder`,
  `TextRenderer::new_with_subpixel_order` and `TextRenderer::lcd_active`. It
  needs `wgpu::Features::DUAL_SOURCE_BLENDING` and falls back to grayscale
  without it.
- `Paragraph` is re-exported from the crate root, so it can be named.
- API documentation on every public item, with a crate-level example.
- A CPU coverage reference and GPU tests, and a shader benchmark
  (`cargo bench --bench shader`).

### Changed

- Each glyph's curves are split into up to 16 horizontal and 16 vertical bands,
  sorted so the shader stops early. A pixel only tests the curves that can
  cross it.
- The outline atlas is `RGBA32F` with two texels per curve record.
- The adapter request in the example prefers the high-performance GPU.

### Fixed

- Large glyphs no longer render inverted.
- Glyphs that wrap an atlas row or sit on a later atlas layer render correctly.
- A failed atlas allocation no longer consumes the remaining capacity.
- Contours are explicitly closed, and winding is normalised per glyph.

## [0.1.0] - 2024-05-21

Initial release.

[Unreleased]: https://github.com/ValentinRio/wgpu-font-renderer/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/ValentinRio/wgpu-font-renderer/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ValentinRio/wgpu-font-renderer/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ValentinRio/wgpu-font-renderer/releases/tag/v0.1.0

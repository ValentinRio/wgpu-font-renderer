# Changelog

All notable changes to this crate are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). From 1.0, breaking
API changes come only with a major version.

## [Unreleased]

## [1.1.0] - Unreleased

### Changed

- Coverage is now the analytic box integral of signed winding over each pixel, replacing centre-ray weighting. Almost every anti-aliased pixel changes slightly.

### Fixed

- Coverage is continuous at curve endpoints and extrema, so moving text by whole pixels no longer changes how a glyph looks (within one readback byte in the grayscale and RGB tests). In 1.0, moving a glyph one pixel vertically could change a pixel by up to 17/255.
- LCD glyph quads now cover the full subpixel filter, so the outer colour fringe of a glyph whose edge falls less than a third of a pixel into its first column is no longer clipped.
- Improved mean accuracy against the independent nonzero reference: in the bundled 12 px corpus, worst Latin mean error fell from 0.014783 to about 0.004088 and Noto Serif SC from 0.026539 to about 0.004704; existing accuracy limits remain unchanged.
- Very small thin strokes are visibly lighter: their coverage now reflects their true ink area rather than the previous centre-ray approximation. Coverage remains a clamped signed winding integral, not nonzero union area; same-direction contours overlapping partly within a pixel can overcount before clamping.

## [1.0.0] - 2026-10-14

### Added

- Loading caches every glyph GSUB shaping can produce from the preset characters in a well-formed font, so ligatures, contextual forms and normalized accents draw; in 0.2 they drew nothing. Added glyphs pass the same outline checks. Malformed GSUB tables are rejected, and closure work past the caps returns the new `LoadingError::SubstitutionTooComplex`. `morx` substitutions are not cached.
- Public, non-exhaustive `LayoutError` identifies shaping and layout failures and implements `Debug`, `Display`, `Error`, `Clone` and `PartialEq`.

- `TextRenderer::render_range` draws subsets of the prepared paragraphs with stable indices.
- CI checks for GPU tests on Mesa lavapipe, lint warnings, documentation, MSRV,
  wasm builds, packaged tests and semantic versioning.

- `ShapedGlyph::source()` exposes cluster byte ranges in the original shaping or layout input for caret placement, hit testing and selection. Hand-pushed glyphs have no source.

- `TypeWriter::layout_text`, `TextLayout`, `Align`, `LineHeight` and `TextBlock` add greedy Unicode wrapping, hard breaks, hanging trailing spaces, alignment and block movement. Layout preserves each paragraph’s shaping and uses each font instance’s metrics; `shape_text` remains single-line.

- Metric table selection intentionally matches the renderer rather than skrifa’s `Metrics`: typo metrics require OS/2 v4 with USE_TYPO_METRICS; a zero hhea ascender falls back to nonzero typo, then Windows metrics. Instances are never unloaded; every instance uploads its own outlines, so per-frame creation eventually fills the atlas (`AtlasFull`).
- `FontStore::metrics` and non-exhaustive `FontMetrics` expose the anchor ascender (possibly zero or negative), nonnegative descent magnitude and signed line gap; ascent matches the renderer’s baseline anchor, including MVAR deltas.
- `FontStore::axes` borrows cached variable font ranges as `&[Axis]`; `Axis` is non-exhaustive. `load_variation` shares base bytes and characters, uploads instance outlines, inherits the base location, clamps overrides and ignores unknown tags. One skrifa normalized location drives outlines, placement, metrics and shaping.
- `LoadingError::UnknownFont` distinguishes a foreign variation base from invalid font data.

### Changed

- `TypeWriter::shape_text` and `layout_text` return `Result<_, LayoutError>` instead of `Option`; handle the error or use `?`. `shape_text` now rejects non-finite positions, which 0.2 accepted; `layout_text` also rejects them. Derived line heights must stay finite and positive after scaling, including underflow checks.

- Fonts are read with skrifa instead of ttf-parser. swash is at 0.2 without its scaler and rasteriser.
- `.notdef` is now drawn for every character a font lacks, if glyph zero has an outline. In 0.2 it was drawn only after a loaded character had been explicitly mapped to glyph zero. An empty `.notdef` still advances without drawing.
- Glyph ink is now positioned at the pen plus shaped offset plus its scaled left side bearing. In 0.2, each glyph's ink was anchored at the pen; text now shifts by each glyph's bearing. TrueType outlines apply `hmtx.lsb - glyf.xMin`, so the cached box and bearing follow `hmtx`, while 0.2 cached raw `glyf` coordinates. CFF outline coordinates are unchanged; their bearings also position the ink.
- Shaping keeps every glyph of each cluster with its advance and offsets, and shapes script runs under their own script. Runs remain in logical order; bidi reordering is not implemented, and `layout_text` provides wrapping.
- Repeated characters select and upload each glyph only once per load, including an empty `.notdef`.
- Text with no strong script retains Latin shaping; common, inherited and unknown-script characters otherwise join neighbouring script runs.
- Atlas exhaustion of either curves or bands fails the load with `LoadingError::AtlasFull(Vec<GlyphId>)`, naming every outlined glyph that did not fit. Earlier fonts remain usable and no half-loaded font is added. Exhaustion means an allocation cannot fit one layer or would exceed the device texture array layer limit: default layers use 16 MiB each (32 MiB initially; 4 GiB at 256 layers).
- Glyph IDs now follow the shaper's cmap selection: Windows symbol subtables take priority, and format 12 is preferred. In 0.2, Windows symbol subtables were skipped and each character used the first mapping found across Unicode subtables in record order; symbol-only fonts now draw, and conflicting format 4/12 mappings now agree with shaping, avoiding wrong or missing glyphs.
- `TextRenderer::prepare` accepts any iterator of borrowed paragraphs, including slices, borrowed vectors, and chained lists.
- `TypeWriter` implements `Default`, equivalent to `TypeWriter::new()`.

- `ShapedGlyph` stores source ranges in two `u32` fields, limiting its size to 24 bytes. Shaping and layout reject input longer than `u32::MAX` bytes. Swash caps clusters at 32 internal parser characters, counting decomposed components but not emoji variation selectors U+FE0E/U+FE0F that follow an emoji base. Shaping keeps swash’s cluster stream over the crate’s tokens, including its pending components and its repeated cap-boundary character (drawn twice, with overlapping source ranges); restarting at a source byte boundary would lose pending components.

- Text layout whitespace is close to CSS `white-space: pre-wrap`, except that leading spaces never wrap onto a line of their own: they count toward their line's width, so `"   abc"` overflows a maximum width that fits `"abc"`. Breakable spaces (line break class SP or BA, including tab) hang at every line end without contributing width or alignment; no-break spaces never hang.
- Hard breaks (LF, CR, CRLF as one break, VT, FF, NEL, U+2028 and U+2029) split text into independently shaped paragraphs, including independent script detection. For `"ع\n12"`, `12` is shaped with the Latin script tag (same font) rather than continuing the Arabic run. This preserves combining marks at paragraph starts. Reordered spans stay unbroken, block moves preserve line offsets, and layout rejects non-finite origins, line widths, derived line heights and block sizes.

- Variable fonts shape at their normalized location even when all coordinates are zero. Unlike 0.2, FeatureVariations whose condition ranges cover the default location now apply there.
- Output glyph zero (`.notdef`) is omitted from `Paragraph::glyphs()` only when its source is a Cc control or default-ignorable character, contributing no advance or ink. Nonzero output glyphs, including substitutions and ligatures with such a source, retain swash’s glyphs and positioning. Mapped Cc controls enter ordinary glyph shaping to retain their font glyphs; swash’s control handling would otherwise hide them before output. Missing letters and symbols still draw `.notdef` when it has an outline. Swash’s existing hiding of mapped default-ignorables (for example Roboto’s U+00AD), like HarfBuzz’s default, is retained from 0.2.

- Loading checks glyph dimensions and enforces per-glyph work limits, found by a seeded robustness test over random and corrupted fonts, rejecting a glyph past them with the new `LoadingError::GlyphTooComplex`; a valid but very complex glyph can be refused. A `VARC` table is ignored in favour of `glyf` or CFF outlines. Glyphs whose bounding box spans more than 32,767 font units, which OpenType allows, now load.
- Grayscale text now honours colour alpha, including the LCD fallback; in 0.2 it ignored alpha. LCD continues to scale coverage by alpha. Alpha one preserves existing output.

### Breaking

- The library no longer enables any `wgpu` backend. Applications must enable the backends they want on their own `wgpu` dependency; a plain `wgpu = "30"` enables the defaults.
- Font handles returned by `load` and `load_from_bytes` are opaque `FontId` values.
- Glyph indices in paragraphs use the root-exported `GlyphId(pub u16)`.
- `FontStore::new` takes only a device.
- `FontStore::load` accepts any `AsRef<Path>` path and returns `FontId`.
- `FontStore::load_from_bytes` returns `FontId`.
- `LoadingError` is non-exhaustive, with `Io(std::io::Error)`, `InvalidFont`, and `AtlasFull(Vec<GlyphId>)` and `GlyphTooComplex(GlyphId)`; it implements `Error` and preserves the I/O source.
- `Paragraph` fields are private; use getters, `set_position`, and `set_color`.
- `Paragraph::new` takes the font first, followed by position, size, and color.
- `Paragraph::append` is renamed to `push` (zero offset); `push_at` accepts an offset. `glyphs()` now returns `&[ShapedGlyph]`, a non-exhaustive, `Copy` struct with public `glyph`, `advance`, and `offset` fields in physical pixels, x rightward and y downward.
- Font sizes in `Paragraph` and `TypeWriter::shape_text` are `f32` pixels per em.
- `SubpixelOrder` is non-exhaustive; include a fallback arm when matching.
- `TextRenderer::new` takes a texture format and `&FontStore` instead of a surface configuration and atlas.
- `TextRenderer::new_with_subpixel_order` becomes `with_subpixel_order`, with the same format/store change.
- `TextRenderer::update_uniforms` becomes `resize`, takes a queue, and must be called before the first render and on every viewport change.
- `TextRenderer::prepare` also takes a queue, skips unknown fonts and invalid sizes, refreshes the atlas binding, and reuses instance buffers. Call it once per queue submit with every paragraph of the frame: a second preparation before submit can change what earlier render calls draw. Use `render_range` for layered or multi-pass drawing.
- `TextRenderer::render` takes `&self` and a render pass without a size argument or lifetime tie to the renderer, and preserves the pass scissor rectangle.
- `Font`, `Glyph`, `Atlas`, `Allocation`, `Resolution`, `Params`, `FontStore::get`, and `FontStore::atlas` leave the stable API; limited inspection is available only through the hidden, semver-exempt `internals` module with `unstable-internals`.

### Migrating from 0.2

`shape_text` and the new `layout_text` return `Result`, so replace `Some`/`None` matching with `Ok`/`Err`. Use `LayoutError` to inspect the cause. When propagating loading and layout errors together, return an error type that accepts both. Preset caches now include GSUB closure and may require more atlas space.

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
) -> Result<(), Box<dyn std::error::Error>> {
let mut store = FontStore::new(device);
let font: FontId = store.load(device, queue, "font.otf", "Hi")?;
let memory_font = store.load_from_bytes(device, queue, bytes, "Hi")?;
let mut writer = TypeWriter::default(); // new() remains available
let mut paragraph = Paragraph::new(font, [24., 24.], 32., [0., 0., 0., 1.]);
paragraph.push(GlyphId(1), 12.);
paragraph.push_at(GlyphId(2), 0., [-4., -2.]);
for glyph in paragraph.glyphs() {
    let placement = (glyph.glyph, glyph.advance, glyph.offset);
}
paragraph.set_position([40., 24.]);
paragraph.set_color([1.; 4]);
let metrics = (paragraph.font(), paragraph.position(), paragraph.size(),
    paragraph.color(), paragraph.width(), paragraph.glyphs());
let shaped = writer.shape_text(&store, memory_font, [24., 24.], 32., [1.; 4], "Hi")?;
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
    LoadingError::AtlasFull(glyphs) => { /* font was not loaded */ },
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

[Unreleased]: https://github.com/ValentinRio/wgpu-font-renderer/compare/v1.0.0...HEAD
[1.1.0]: https://github.com/ValentinRio/wgpu-font-renderer/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/ValentinRio/wgpu-font-renderer/compare/v0.2.1...v1.0.0
[0.2.1]: https://github.com/ValentinRio/wgpu-font-renderer/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ValentinRio/wgpu-font-renderer/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ValentinRio/wgpu-font-renderer/releases/tag/v0.1.0

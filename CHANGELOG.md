# Changelog

All notable changes to this crate are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the version is
below 1.0, a minor bump (0.x) may break the API.

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

[0.2.0]: https://github.com/ValentinRio/wgpu-font-renderer/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ValentinRio/wgpu-font-renderer/releases/tag/v0.1.0

# wgpu-font-renderer

[![crates.io](https://img.shields.io/crates/v/wgpu-font-renderer.svg?label=wgpu-font-renderer)](https://crates.io/crates/wgpu-font-renderer)
[![docs.rs](https://docs.rs/wgpu-font-renderer/badge.svg)](https://docs.rs/wgpu-font-renderer)
[![CI](https://github.com/ValentinRio/wgpu-font-renderer/actions/workflows/ci.yml/badge.svg)](https://github.com/ValentinRio/wgpu-font-renderer/actions/workflows/ci.yml)

GPU-centered font rendering for [wgpu](https://wgpu.rs/). Glyphs are drawn straight from their outlines in the fragment shader. No signed distance field cache of any sort.

![The simple example drawing Roboto, Cantarell and Noto Serif SC, with a large 龍 above a line of Chinese text](examples/screenshot.png)

## Features

- Reads outlines from TrueType and OpenType (CFF/CFF2) files, including variable-font instances.
- Analytic per-pixel coverage from the outline's quadratic curves, with the nonzero winding rule.
- Grayscale anti-aliasing by default, opt-in subpixel (LCD) anti-aliasing.
- Text shaping with [swash](https://github.com/dfrg/swash), line wrapping and alignment, font metrics, and per-glyph source ranges for carets and hit testing.
- Runs natively and in the browser through WebGPU.

## Installation

```toml
[dependencies]
wgpu-font-renderer = "0.2"
```

Enable the backends you want on your own `wgpu` dependency; a plain `wgpu = "30"` enables the defaults.

## Usage

These examples target the upcoming 1.0 API. For changes from 0.2, see [the migration guide](CHANGELOG.md#migrating-from-02).

```rust
use wgpu_font_renderer::{FontStore, TextRenderer, TypeWriter};

// Once, at startup. `config` is your wgpu::SurfaceConfiguration.
// The preset string lists the characters your app will draw.
let mut font_store = FontStore::new(&device);
let cache_preset = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,;:!ù*^$=)àç_è-('\"é&²<>+°§/.? ";
let font_key = font_store
    .load(&device, &queue, "fonts/Roboto-Regular.ttf", cache_preset)
    .expect("Couldn't load the font");
let mut text_renderer = TextRenderer::new(&device, config.format, &font_store);
// Before the first render and on every viewport change.
text_renderer.resize(&queue, [config.width, config.height]);

// Whenever the text changes: shape paragraphs.
// Arguments: font, position on screen, font size, RGBA color, text.
// The shader does no colour conversion; the target format decides encoding.
let mut type_writer = TypeWriter::new();
let mut paragraphs = Vec::new();
let paragraph = type_writer.shape_text(&font_store, font_key, [100., 100.], 72., [0.68, 0.5, 0.12, 1.], "Salut, c'est cool!")
    .expect("valid font, size and position");
paragraphs.push(paragraph);
// Once per queue submit, prepare every paragraph of the frame.
text_renderer.prepare(&device, &queue, &paragraphs, &font_store);

// Inside an existing render pass.
text_renderer.render(&mut pass);
```

For wrapped text, lay it out into a `TextBlock` and pass that to `prepare`:

```rust
use wgpu_font_renderer::{Align, TextLayout};

let layout = TextLayout::default().with_max_width(240.).with_align(Align::Center);
let block = type_writer.layout_text(
    &font_store, font_key, [24., 24.], 32., [0., 0., 0., 1.],
    "Text wraps at Unicode line-break opportunities.", &layout,
).expect("valid font, size and layout");
text_renderer.prepare(&device, &queue, &block, &font_store);
```

For a variable-font instance, derive it from a loaded font:

```rust
let bold = font_store.load_variation(&device, &queue, font_key, &[(*b"wght", 700.0)])?;
```

On wasm, or to embed a font in the binary, use `FontStore::load_from_bytes`. Layering with `render_range`, font metrics, source ranges, subpixel mode and the exact layout rules are documented on [docs.rs](https://docs.rs/wgpu-font-renderer). A complete program is in [`examples/simple.rs`](examples/simple.rs).

## Limits

- The pipeline targets one colour attachment, no depth/stencil attachment and sample count 1. It cannot draw inside a pass with depth/stencil, MSAA or several colour targets.
- Characters outside the preset string draw nothing unless their glyph is already cached. Loading includes canonical composition/decomposition seeds from swash’s Unicode data, Khmer parser vowel seeds, glyph zero, and GSUB substitutions reachable from those seeds (including ligatures only when all components are cached). Empty outlines draw no ink.
- AAT `morx` limit: when a font has no GSUB and swash shapes with `morx`, its ligatures and substitutions are not cached and can draw nothing.
- Malformed GSUB coverage indices, reversed ranges, truncated arrays and out-of-range lookup/feature references are rejected at load.
- Shaping and layout return `Result<_, LayoutError>` with the specific validation failure. GSUB closure is capped at 128 passes and 4,000,000 work units; exceeding either returns `LoadingError::SubstitutionTooComplex`.
- Fonts are never unloaded. Each load, and each variable-font instance, adds its outlines to the atlas for good, so creating instances per frame eventually returns `LoadingError::AtlasFull`.
- No bidi reordering and no font fallback: script runs are shaped in logical order, and characters the font lacks draw `.notdef`.
- Untrusted fonts: the loader caps per-glyph work and refuses glyphs past the caps with `LoadingError::GlyphTooComplex`. Two upstream limits remain: a crafted `cmap` panics swash in debug builds, and a crafted CFF glyph can load slowly. Details are in the crate docs.
- LCD mode has no filter yet, so small sizes show some colour fringing.

## Running the examples

Run the native example:

```sh
cargo run --example simple
```

To run it in a browser, install the `wasm32-unknown-unknown` target and the `wasm-bindgen-cli` version matching `Cargo.lock` (currently 0.2.129), then:

```sh
cargo build --release --example simple --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir examples/web/pkg target/wasm32-unknown-unknown/release/examples/simple.wasm
```

Serve `examples/web` on localhost with any static file server and open it in a WebGPU-capable browser. WebGPU needs a secure context, so a LAN IP over plain HTTP won't work.

## Testing and benchmarks

```sh
REQUIRE_GPU=1 cargo test --all-features --offline
cargo bench --all-features --offline --bench shader
```

Without `REQUIRE_GPU=1`, GPU tests skip when no adapter is available. A software adapter such as Mesa lavapipe is enough, and CI uses it. Benchmark options are in [`benches/README.md`](benches/README.md).

## How it works

Coverage is computed analytically, following Eric Lengyel's paper [GPU-Centered Font Rendering Directly from Glyph Outlines](https://jcgt.org/published/0006/02/02/) (JCGT, 2017), the method behind Slug. The shader was written from the paper, not from the reference implementation. The patent on the method (US 10,373,352) was dedicated to the public domain in March 2026.

For each pixel the shader casts a horizontal and a vertical ray through the pixel centre and sums the signed crossings with the glyph's quadratic curves, so overlapping contours fill. Curves are split into up to 16 bands per axis and sorted, so each ray tests only the curves that can reach it and stops early. There is no cubic solve and no trigonometry. Two centre rays approximate a box filter: they are exact on axis-aligned edges and can be off by a fraction of a pixel's coverage where a vertex or a very tight feature sits inside the pixel.

## Built with

- [swash](https://github.com/dfrg/swash) for text shaping
- [skrifa](https://github.com/googlefonts/fontations) to read TrueType and OpenType files
- [wgpu](https://wgpu.rs/) 30 as graphics API

## Contributing

Issues and pull requests are welcome. See the [open issues](https://github.com/ValentinRio/wgpu-font-renderer/issues) for proposed features and known issues.

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the [MIT license](LICENSE-MIT), at your option.

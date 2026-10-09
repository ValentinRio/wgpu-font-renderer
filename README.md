<a name="readme-top"></a>

<!-- PROJECT LOGO -->
<br />
<div align="center">

<h3 align="center">wgpu-font-renderer</h3>
  <div align="center">
    <a href="https://crates.io/crates/wgpu-font-renderer"><img src="https://img.shields.io/crates/v/wgpu-font-renderer.svg?label=wgpu-font-renderer" alt="crates.io"></a>
    <a href="https://docs.rs/wgpu-font-renderer"><img src="https://docs.rs/wgpu-font-renderer/badge.svg" alt="docs.rs"></a>
  </div>
  <p align="center">
    GPU-Centered Font Rendering crate
    <br />
    <a href="https://github.com/ValentinRio/wgpu-font-renderer/tree/main/examples">View Demo</a>
    ·
    <a href="https://github.com/ValentinRio/wgpu-font-renderer/issues">Report Bug</a>
  </p>
</div>



<!-- TABLE OF CONTENTS -->
<details>
  <summary>Table of Contents</summary>
  <ol>
    <li>
      <a href="#about-the-project">About The Project</a>
      <ul>
        <li><a href="#built-with">Built With</a></li>
      </ul>
    </li>
    <li>
      <a href="#getting-started">Getting Started</a>
      <ul>
        <li><a href="#installation">Installation</a></li>
      </ul>
    </li>
    <li><a href="#usage">Usage</a></li>
    <li><a href="#running-the-examples">Running the examples</a></li>
    <li><a href="#testing">Testing</a></li>
    <li><a href="#benchmarking">Benchmarking</a></li>
    <li><a href="#roadmap">Roadmap</a></li>
    <li><a href="#contributing">Contributing</a></li>
    <li><a href="#license">License</a></li>
    <li><a href="#contact">Contact</a></li>
  </ol>
</details>



<!-- ABOUT THE PROJECT -->
## About The Project

[![Product Name Screen Shot][product-screenshot]](https://example.com)

Render glyphs by extracting their outlines from TrueType or OpenType (CFF/CFF2) files and draw them directly from GPU. No signed distance field cache of any sort. This is based on Eric Lengyel's Slug algorithm.

Glyphs use up to 16 horizontal bands with a 64-font-unit margin on each side. Each band stores curves whose endpoint/control-point y-extent overlaps it, including straight edges. Pixels use their band when the AA window fits the margin (integer font sizes 0–557), otherwise the unchanged flat loop. Legacy row-crossing/later-layer lists and exact zero-distance or collinear-extension hits also use the full loop to keep pixels unchanged.

<p align="right">(<a href="#readme-top">back to top</a>)</p>



### Built With

* Swash for text shaping - https://github.com/dfrg/swash
* TTF Parser to read TrueType and OpenType files - https://github.com/RazrFalcon/ttf-parser
* WGPU 30.0.1 as graphic API - https://wgpu.rs/

<p align="right">(<a href="#readme-top">back to top</a>)</p>



<!-- GETTING STARTED -->
## Getting Started

This is an example of how you use this crate to generate paragraphs programatically and render them with WGPU.

### Installation

Add dependency to you cargo.toml

```toml
[dependencies]
wgpu-font-renderer = "0.1.0"
```

<p align="right">(<a href="#readme-top">back to top</a>)</p>



<!-- USAGE EXAMPLES -->
## Usage

First you need to load the TTF file in the FontStore by passing the file path and and preset string that will define the list of characters used by your app.

```rust
let mut font_store = FontStore::new(&device, &config);
let cache_preset = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,;:!ù*^$=)àç_è-('\"é&²<>+°§/.? ";
let font_key = font_store.load(&device, &queue, "examples/Roboto-Regular.ttf", cache_preset).expect("Couldn't load the font");
```

Then during the runtime you can create new paragraphs to be rendered. Those can be defined with:
- Specific font name
- Position on the screen
- Font size
- Linear RGBA color
- Text content

```rust
let mut paragraphs = Vec::new();
let mut type_writer = TypeWriter::new();
if let Some(paragraph) = type_writer.shape_text(&font_store, font_key, [100., 100.], 72, [0.68, 0.5, 0.12, 1.], "Salut, c'est cool!") {
    paragraphs.push(paragraph);
}
```

Then you can initialize the predefined font rendering middleware:

```rust
let mut text_renderer = TextRenderer::new(&device, &config, font_store.atlas());
```

Call prepare to pass the paragraphs you want to render to the middleware:

```rust
text_renderer.prepare(&device, &paragraphs, &font_store);
```

Call render with an existing render pass to build the command buffer necessary to render your paragraphs:

```rust
text_renderer.render(&mut pass, [config.width, config.height]);
```

_To see concrete example, please check [here](https://github.com/ValentinRio/wgpu-font-renderer/tree/main/examples)_

<p align="right">(<a href="#readme-top">back to top</a>)</p>



## Running the examples

Run the native example:

```sh
cargo run --example simple
```

To run the same example in a browser:

1. Install the target and the CLI version matching `wasm-bindgen` in `Cargo.lock`:

   ```sh
   rustup target add wasm32-unknown-unknown
   cargo install wasm-bindgen-cli --version 0.2.129
   ```

2. Build the example:

   ```sh
   cargo build --release --example simple --target wasm32-unknown-unknown
   ```

3. Generate the browser bindings:

   ```sh
   wasm-bindgen --target web --out-dir examples/web/pkg target/wasm32-unknown-unknown/release/examples/simple.wasm
   ```

4. Serve `examples/web` with any static server, for example:

   ```sh
   python3 -m http.server -d examples/web 8000
   ```

   Open `http://localhost:8000` in a WebGPU-capable browser. WebGPU requires a secure context, such as localhost or HTTPS. A LAN IP over plain HTTP will not expose WebGPU.

## Testing

Run `cargo test --offline`. Native GPU tests skip with a diagnostic when no adapter
is available. Set `REQUIRE_GPU=1` to fail instead, so CI cannot silently skip GPU
coverage:

```sh
REQUIRE_GPU=1 cargo test --offline
```

A software adapter such as Mesa lavapipe is sufficient. GPU tests are disabled on
wasm32. No tests are marked ignored.

## Benchmarking

The native shader benchmark renders four fixed scenes at 1920×1080 in sRGB,
using the public font loading, shaping and rendering API. Run it with:

```sh
cargo bench --offline --bench shader
BENCH_ROUNDS=7 BENCH_FRAMES=40 BENCH_WARMUP=10 cargo bench --offline --bench shader -- large_glyphs
```

Set the `WGPU_BACKEND` environment variable to `dx12` or `vulkan` to select the backend (POSIX shells: `WGPU_BACKEND=dx12 cargo bench ...`; PowerShell: `$env:WGPU_BACKEND = "dx12"`; cmd: `set WGPU_BACKEND=dx12`). Leave it unset for wgpu's defaults. Some Vulkan drivers return zero timestamps (seen on an AMD RX 9070 XT with the proprietary driver); DX12 works there.

Defaults are **5 rounds**, each with 10 discarded warm-up frames and 30 measured
frames per scene. Override them with `BENCH_ROUNDS`, `BENCH_WARMUP` and
`BENCH_FRAMES` (minimum 3 rounds and 10 measured frames). The scene order rotates
between rounds to distribute drift. Each round reports its **p25 frame time**;
the comparison statistic is the **median of those round p25 values**. This lower
quantile reduces the effect of one-sided contention tails without taking the
single fastest frame. Frame median and p95 are also printed; the final p95 pools
all measured frames. `BENCH_SCENE=large_glyphs` also filters scenes. Arguments
beginning with `-`, such as `--nocapture`, are ignored.

Each scene reports total curves across glyph instances, covered glyph-quad pixel
centres (clipped to the target, including overdraw), and ink pixels (any RGB
channel below white). Curves include quadratics produced by splitting the
Cantarell CFF2 fixture's cubic outlines.

The clock is `gpu_timestamp` when the adapter supports `TIMESTAMP_QUERY`, with
encoder queries when `TIMESTAMP_QUERY_INSIDE_ENCODERS` is supported, otherwise render-pass queries; the header and reports record `timestamp_source` (`encoder` or `pass`, null for CPU timing), and comparisons require it to match. Otherwise `cpu_submit_wait` measures submission
through a blocking device poll, excluding command encoding. Loading, shaping,
preparation and image readback are outside the measured interval. Each frame is
submitted and completed separately. The GPU interval includes attachment clear
and store operations as well as drawing.

Save a baseline on main and compare the branch on the same adapter and driver.
If main already contains the harness:

```sh
git switch main
BENCH_SAVE=main cargo bench --offline --bench shader
git switch shader-bench
BENCH_BASELINE=main cargo bench --offline --bench shader
```

If main predates the harness, use a separate checkout with the identical harness
and dependency declarations. From the branch checkout, after committing the
benchmark files:

```sh
git worktree add --detach /tmp/wgpu-font-renderer-main main
for file in Cargo.toml Cargo.lock benches/shader.rs; do
    mkdir -p "/tmp/wgpu-font-renderer-main/$(dirname "$file")"
    git show "HEAD:$file" > "/tmp/wgpu-font-renderer-main/$file"
done
(cd /tmp/wgpu-font-renderer-main && BENCH_SAVE=main cargo bench --offline --bench shader)
mkdir -p target/shader-bench/main
cp /tmp/wgpu-font-renderer-main/target/shader-bench/main.json target/shader-bench/main.json
cp -r /tmp/wgpu-font-renderer-main/target/shader-bench/main/. target/shader-bench/main/
BENCH_BASELINE=main cargo bench --offline --bench shader
```

Reports live in `target/shader-bench/<name>.json`. Schema 2 records the per-round
p25 statistics, **every measured frame sample**, frames and warm-up per round,
process IDs and per-round git revisions. It also records adapter/backend/driver,
clock, wgpu version, `LP_NUM_THREADS` (or null when unset), and the CPU core count
reported by `available_parallelism`. HEAD revisions do not represent uncommitted
edits. Comparisons refuse mismatches in adapter, driver, clock, target, statistic,
wgpu version, thread setting or CPU core count. The comparison prints the frames,
warm-up and round counts for both sides, including mixed configurations from
pooled baselines. Schema 1 baselines must be regenerated.

The effective detectable threshold is the larger of **3% of the baseline median**
and **3 × the largest spread estimate**, printed as **±X%** beside each verdict.
The estimates are the baseline's round-p25 MAD, the current run's round-p25 MAD,
and each side's half-range of per-process median p25 values. Each MAD uses its
own run's median, so a large tight baseline cannot mask a noisy candidate.
The process-median range retains drift sampled by a minority of appended
processes, even when the overall baseline MAD is zero. Process groups follow
round-number restarts in the saved records, so PID reuse does not merge them.
A shift exceeding that threshold is **faster** or **slower**; otherwise it is
**within noise**, which does not mean equal. This is a robust detection heuristic,
not a significance test. More frames stabilize each p25, while more rounds
capture drift; neither can recover drift between processes that was never sampled.

To include run-level drift, pool several separate invocations of the **same code**
into the baseline before comparing. For Mesa lavapipe, an explicit thread limit
can reduce contention; use the same setting on every run, for example:

```sh
export LP_NUM_THREADS=2
BENCH_SAVE=main cargo bench --offline --bench shader
BENCH_SAVE=main BENCH_APPEND=1 cargo bench --offline --bench shader
BENCH_SAVE=main BENCH_APPEND=1 cargo bench --offline --bench shader
# Switch to the candidate branch, retaining the baseline and RGBA directory.
BENCH_BASELINE=main cargo bench --offline --bench shader
```

`BENCH_APPEND=1` pools round records and recomputes summaries; it refuses changed
fingerprints or workloads. Without append, a full save replaces the file. A
**filtered save always merges its measured scenes into an existing baseline**,
preserves unselected scenes and their images, and prints that it merged them.
Per-scene round records are authoritative after merges or appends.

The exact FNV-1a checksum remains the strict output signal. Saves also write the
packed, unpadded RGBA8 readback to `target/shader-bench/<name>/<scene>.rgba`.
Keep that directory together with its JSON when copying baselines. Comparison
checks the saved RGBA checksum and reports **differing pixel count**, **maximum
absolute channel delta** over all RGBA channels, and ink count delta, separately
from timing. Output is **identical** when no pixels differ, **minor** when at most
32 pixels differ and max channel delta is at most 2, otherwise **OUTPUT CHANGED**.
Tune these limits with `BENCH_PIXEL_TOLERANCE` and `BENCH_CHANNEL_TOLERANCE`;
setting both to 0 gives strict classification. A minor result still prints a
changed exact checksum. Missing baseline scenes are reported explicitly.

Lavapipe is a software renderer: its numbers only rank changes relative to each
other on that setup. Real GPU performance conclusions require a real GPU.

<!-- ROADMAP -->
## Roadmap

- [x] Separate glyph outlines into bands, so each pixel only tests the curves that can cross it
- [ ] Sort curves inside each band, so the shader can stop early (depends on bands)
- [ ] Optimize the curve data layout: fetch each curve in one or two texel loads (RGBA32F or a storage buffer) instead of six sampled R32F reads, and drop the hardcoded 2048 atlas width
- [ ] Subpixel (LCD) anti-aliasing: the R, G and B coverage samples are computed, but only R reaches the output (grayscale anti-aliasing already works)
- [ ] Handle overlapping contours, which currently render as holes because the winding test expects exactly one crossing pair
- [ ] Add a CFF1 (`.otf`) test font; only CFF2 is covered today
- [ ] Let callers select variable font axes; only the default instance renders

See the [open issues](https://github.com/ValentinRio/wgpu-font-renderer/issues) for a full list of proposed features (and known issues).

<p align="right">(<a href="#readme-top">back to top</a>)</p>



<!-- CONTRIBUTING -->
## Contributing

Contributions are what make the open source community such an amazing place to learn, inspire, and create. Any contributions you make are **greatly appreciated**.

If you have a suggestion that would make this better, please fork the repo and create a pull request.
Don't forget to give the project a star! Thanks again!

1. Fork the Project
2. Create your Feature Branch (`git checkout -b feature/AmazingFeature`)
3. Commit your Changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to the Branch (`git push origin feature/AmazingFeature`)
5. Open a Pull Request

<p align="right">(<a href="#readme-top">back to top</a>)</p>



<!-- LICENSE -->
## License

Distributed under the MIT License.

<p align="right">(<a href="#readme-top">back to top</a>)</p>



<!-- CONTACT -->
## Contact

Project Link: [https://github.com/ValentinRio/wgpu-font-renderer](https://github.com/ValentinRio/wgpu-font-renderer)

<p align="right">(<a href="#readme-top">back to top</a>)</p>


<!-- MARKDOWN LINKS & IMAGES -->
<!-- https://www.markdownguide.org/basic-syntax/#reference-style-links -->
[issues-url]: https://github.com/ValentinRio/wgpu-font-renderer/issues
[license-url]: https://github.com/ValentinRio/wgpu-font-renderer/blob/main/LICENSE.txt
[product-screenshot]: examples/screenshot.png


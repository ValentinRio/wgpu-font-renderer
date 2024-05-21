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
    <li><a href="#roadmap">Roadmap</a></li>
    <li><a href="#contributing">Contributing</a></li>
    <li><a href="#license">License</a></li>
    <li><a href="#contact">Contact</a></li>
  </ol>
</details>



<!-- ABOUT THE PROJECT -->
## About The Project

[![Product Name Screen Shot][product-screenshot]](https://example.com)

Render glyphs by extracting their outlines from TTF files and draw them directly from GPU. No signed distance field cache of any sort. This is based on Eric Lengyel's Slug algorithm.

<p align="right">(<a href="#readme-top">back to top</a>)</p>



### Built With

* Swash for text shaping - https://github.com/dfrg/swash
* TTF Parser to read TTF files - https://github.com/RazrFalcon/ttf-parser
* WGPU as graphic API - https://wgpu.rs/

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



<!-- ROADMAP -->
## Roadmap

- [ ] Separate glyph outlines into bands
- [ ] Sort curves inside each band
- [ ] Optimize data-layout
- [ ] Add Anti-aliasing

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


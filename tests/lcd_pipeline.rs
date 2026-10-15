#![cfg(not(target_arch = "wasm32"))]

use skrifa::MetadataProvider;
use wgpu_font_renderer::{FontStore, SubpixelOrder, TextRenderer, TypeWriter};
mod common;
#[path = "common/draw.rs"]
mod draw;
#[path = "common/probe.rs"]
#[allow(dead_code, clippy::chunks_exact_to_as_chunks)]
mod probe;

fn lcd_gpu() -> Option<(wgpu::Device, wgpu::Queue, wgpu::SurfaceConfiguration)> {
    let gpu = common::gpu()?;
    if !gpu
        .0
        .features()
        .contains(wgpu::Features::DUAL_SOURCE_BLENDING)
    {
        assert!(
            std::env::var("REQUIRE_DUAL_SOURCE").as_deref() != Ok("1"),
            "REQUIRE_DUAL_SOURCE=1 but the adapter lacks DUAL_SOURCE_BLENDING"
        );
        eprintln!("Skipping LCD test: adapter lacks DUAL_SOURCE_BLENDING");
        return None;
    }
    Some(gpu)
}

fn replace_shader(source: &str, from: &str, to: &str) -> String {
    let changed = source.replace(from, to);
    assert_ne!(changed, source, "shader replacement did not match: {from}");
    changed
}

fn source(body: &str, output: &str, bgr: bool) -> String {
    let lcd = replace_shader(
        include_str!("../src/lcd.wgsl"),
        "@fragment\nfn fs_lcd",
        "fn fs_lcd",
    );
    let lcd = replace_shader(
        &lcd,
        "override LCD_BGR: bool = false;",
        if bgr {
            "const LCD_BGR: bool = true;"
        } else {
            "const LCD_BGR: bool = false;"
        },
    );
    format!(
        "enable dual_source_blending;\n{}\n{}\n{}\n{}",
        include_str!("../src/shader.wgsl"),
        lcd,
        r#"@vertex fn probe_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
            let p = array(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.));
            return vec4(p[index], 0., 1.);
        }"#,
        format_args!(
            "@fragment fn probe_fragment(@builtin(position) p: vec4<f32>) -> {output} {{ {body} }}"
        )
    )
}

// Clockwise rectangle in y-down font units: x=1000..right, y=0..2000.
fn rectangle(device: &wgpu::Device, queue: &wgpu::Queue, right: f32) -> wgpu::TextureView {
    let points = [[1000., 0.], [right, 0.], [right, 2000.], [1000., 2000.]];
    let mut data = Vec::<f32>::new();
    for i in 0..4 {
        let a = points[i];
        let b = points[(i + 1) % 4];
        data.extend([
            a[0],
            a[1],
            a[0] + (b[0] - a[0]) * 0.25,
            a[1] + (b[1] - a[1]) * 0.25,
            b[0],
            b[1],
            0.,
            0.,
        ]);
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("LCD synthetic outline"),
        size: wgpu::Extent3d {
            width: 8,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(&data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(128),
            rows_per_image: Some(1),
        },
        texture.size(),
    );
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

fn synthetic_input(x: &str) -> String {
    format!(
        r#"
    var input: VertexOutput;
    let sizes = array(12., 16., 24., 81., 400., 1024.);
    input.font_size = sizes[min(u32(p.y) / 64u, 5u)];
    input.units_per_em = 2048.;
    let offset = input.units_per_em / (3. * input.font_size);
    input.size = vec2(2048.) * input.font_size / input.units_per_em;
    input.uv = vec2(({x}) / 2048., 1000. / 2048.);
    input.atlas_size = 32;
    input.bands = vec3(0., 0., -1.);
    input.color = vec4(1.);
    input.pixel_scale = input.font_size / input.units_per_em;
    input.uv = input.uv * 2048.;
    "#
    )
}

#[test]
fn c2_lcd_offsets_are_one_third_pixel() {
    let Some((device, queue, _)) = lcd_gpu() else {
        return;
    };
    let atlas = rectangle(&device, &queue, 2000.);
    let shader = source(
        &(synthetic_input("1002.") + "return vec4(lcd_coverage(input), 1.);"),
        "@location(0) vec4<f32>",
        false,
    );
    let pixels = probe::probe_pixels(&device, &queue, &atlas, &shader);
    for (tile, size) in [12_f32, 16., 24., 81., 400., 1024.].into_iter().enumerate() {
        let rgb = &pixels[tile * 64 * 256 * 4..][..3];
        let expected =
            [-1. / 3., 0., 1. / 3.].map(|offset| (0.5 + 2. * size / 2048. + offset).clamp(0., 1.));
        for c in 0..3 {
            assert!(
                (rgb[c] as f32 / 255. - expected[c]).abs() <= 1. / 255.,
                "size {size}, {rgb:?}, {expected:?}"
            );
        }
        if size <= 81. {
            assert!(
                rgb[0] < rgb[1] && rgb[1] < rgb[2],
                "strict channel ordering: {rgb:?}"
            );
        }
    }
}

#[test]
fn c3_lcd_channels_use_their_own_winding_side() {
    let Some((device, queue, _)) = lcd_gpu() else {
        return;
    };
    let atlas = rectangle(&device, &queue, 2000.);
    let shader = source(
        &(synthetic_input("1000. + offset * .5") + "return vec4(lcd_coverage(input), 1.);"),
        "@location(0) vec4<f32>",
        false,
    );
    let pixels = probe::probe_pixels(&device, &queue, &atlas, &shader);
    for tile in 0..4 {
        let rgb = &pixels[tile * 64 * 256 * 4..][..3];
        assert!(
            rgb[0] < 127 && rgb[1] > 128,
            "R and G must straddle 0.5: {rgb:?}"
        );
    }
}

#[test]
fn lcd_each_channel_integrates_both_edges_of_a_thin_stem() {
    let Some((device, queue, _)) = lcd_gpu() else {
        return;
    };
    let atlas = rectangle(&device, &queue, 1060.);
    let shader = source(
        &(synthetic_input("1020.") + "return vec4(lcd_coverage(input), 1.);"),
        "@location(0) vec4<f32>",
        false,
    );
    let pixels = probe::probe_pixels(&device, &queue, &atlas, &shader);
    let scale = 12_f32 / 2048.;
    let center = 1020. + 1. / (3. * scale);
    let expected = ((1060. - center) * scale + 0.5).clamp(0., 1.)
        - ((1000. - center) * scale + 0.5).clamp(0., 1.);
    assert!((pixels[2] as f32 / 255. - expected).abs() <= 1. / 255.);
    assert!(pixels[2] > 10);
}

#[test]
fn c4_bgr_exactly_swaps_rgb_red_and_blue() {
    let Some((device, queue, _)) = lcd_gpu() else {
        return;
    };
    let atlas = rectangle(&device, &queue, 2000.);
    let body = synthetic_input("1000. + (p.x - 128.) * input.units_per_em / input.font_size / 64.")
        + "return vec4(lcd_coverage(input), 1.);";
    let rgb = probe::probe_pixels(
        &device,
        &queue,
        &atlas,
        &source(&body, "@location(0) vec4<f32>", false),
    );
    let bgr = probe::probe_pixels(
        &device,
        &queue,
        &atlas,
        &source(&body, "@location(0) vec4<f32>", true),
    );
    assert!(rgb.as_chunks::<4>().0.iter().any(|p| p[0] != p[2]));
    for (rgb, bgr) in rgb.as_chunks::<4>().0.iter().zip(bgr.as_chunks::<4>().0) {
        assert_eq!(*bgr, [rgb[2], rgb[1], rgb[0], rgb[3]]);
    }
}

#[test]
fn c5_dual_source_blend_and_color_alpha() {
    let Some((device, queue, _)) = lcd_gpu() else {
        return;
    };
    let atlas = rectangle(&device, &queue, 2000.);
    let input =
        synthetic_input("1000. + (p.x - 128.) * input.units_per_em / input.font_size / 64.");
    let bg = [51. / 255., 153. / 255., 102. / 255.];
    let blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Src1,
            dst_factor: wgpu::BlendFactor::OneMinusSrc1,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
    };
    for alpha in [1., 0.5, 0.] {
        let body =
            format!("{input} input.color = vec4(.8, .2, .6, {alpha}); return fs_lcd(input);");
        let pixels = probe::probe_pixels_with_blend(
            &device,
            &queue,
            &atlas,
            &source(&body, "LcdOutput", false),
            Some(blend),
            wgpu::Color {
                r: bg[0],
                g: bg[1],
                b: bg[2],
                a: 1.,
            },
        );
        // Compute the formula without attachment blending, retaining float
        // coverage until the final Rgba8Unorm conversion in both paths.
        let formula = format!("{input} let cov = lcd_coverage(input) * {alpha}; return vec4(vec3(.8, .2, .6) * cov + vec3(.2, .6, .4) * (vec3(1.) - cov), 1.);");
        let expected = probe::probe_pixels(
            &device,
            &queue,
            &atlas,
            &source(&formula, "@location(0) vec4<f32>", false),
        );
        for (actual, expected) in pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.as_chunks::<4>().0)
        {
            for c in 0..3 {
                assert!(
                    actual[c].abs_diff(expected[c]) <= 1,
                    "alpha {alpha}: {actual:?}, expected {expected:?}, channel {c}"
                );
            }
        }
    }
}

#[test]
fn c5_public_renderer_blends_per_channel() {
    let Some((device, queue, config)) = lcd_gpu() else {
        return;
    };
    let mut store = FontStore::new(&device);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "HiB@",
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    let mut renderer =
        TextRenderer::with_subpixel_order(&device, config.format, &store, SubpixelOrder::Rgb);
    let paragraph = writer
        .shape_text(&store, key, [12.25, 24.5], 24., [1.; 4], "HiB@")
        .unwrap();
    renderer.prepare(&device, &queue, &[paragraph], &store);
    let coverage = draw::draw_pixels(
        &device,
        &queue,
        &mut renderer,
        [128, 128],
        wgpu::Color::BLACK,
    );
    assert!(
        coverage.as_chunks::<4>().0.iter().any(|p| p[0] != p[2]),
        "public pipeline must blend distinct channel coverages"
    );
    for alpha in [1., 0.5, 0.] {
        let paragraph = writer
            .shape_text(
                &store,
                key,
                [12.25, 24.5],
                24.,
                [0.8, 0.2, 0.6, alpha],
                "HiB@",
            )
            .unwrap();
        renderer.prepare(&device, &queue, &[paragraph], &store);
        let pixels = draw::draw_pixels(
            &device,
            &queue,
            &mut renderer,
            [128, 128],
            wgpu::Color {
                r: 0.2,
                g: 0.6,
                b: 0.4,
                a: 1.,
            },
        );
        for (actual, coverage) in pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(coverage.as_chunks::<4>().0)
        {
            for c in 0..3 {
                let cov = f32::from(coverage[c]) / 255. * alpha;
                let expected = ([0.8, 0.2, 0.6][c] * cov + [0.2, 0.6, 0.4][c] * (1. - cov)) * 255.;
                assert!(
                    actual[c].abs_diff(expected.round() as u8) <= 1,
                    "public blend alpha {alpha}, channel {c}: {actual:?}, coverage {coverage:?}"
                );
            }
        }
    }
}

#[test]
fn c6_lcd_bands_match_full_list_and_are_taken() {
    let Some((device, queue, _config)) = lcd_gpu() else {
        return;
    };
    for bytes in [
        &include_bytes!("../fonts/Roboto-Regular.ttf")[..],
        &include_bytes!("../fonts/Cantarell-VF.otf")[..],
    ] {
        let mut store = FontStore::new(&device);
        let key = store
            .load_from_bytes(&device, &queue, bytes, "igB@é")
            .unwrap();
        let font = wgpu_font_renderer::internals::font(&store, key).unwrap();
        for c in "igB@é".chars() {
            let glyph = &font.glyph_cache[&font.face().charmap().map(c).unwrap().to_u32()];
            let [x, y] = glyph.allocation.packed_position();
            let bands = &glyph.bands;
            let [bx, by] = bands.packed_position();
            let body = format!(
                r#"
                var input: VertexOutput;
                let sizes = array(32., 72., 130., 163., 200., 400., 557., 558., 1024., 12., 16., 24.);
                input.font_size = sizes[u32(p.y) / 64u];
                input.units_per_em = 2048.;
                input.size = vec2({width}., {height}.) * input.font_size / 2048.;
                input.uv = vec2(p.x / 256., (p.y % 64.) / 64.);
                input.left_side_bearing = {bearing}.;
                input.atlas_pos = vec2({x}., {y}.);
                input.atlas_size = {count};
                input.layer = {layer}.;
                input.bands = vec3({bx}., {by}., {band_layer}.);
                input.pixel_scale = input.font_size / input.units_per_em;
                input.uv = input.uv * input.size / input.pixel_scale + vec2(input.left_side_bearing, 0.);
                return vec4(lcd_coverage(input), 1.);
            "#,
                width = glyph.bbox.width(),
                height = glyph.bbox.height(),
                bearing = glyph.left_side_bearing,
                count = glyph.allocation.size(),
                layer = glyph.allocation.layer(),
                band_layer = bands.layer()
            );
            let shader = source(&body, "@location(0) vec4<f32>", false);
            let full = replace_shader(
                &shader,
                "if input.bands.z >= 0. && input.atlas_size > 0 {",
                "if false {",
            );
            let actual = probe::probe_pixels(&device, &queue, wgpu_font_renderer::internals::atlas_resources(&store), &shader);
            let expected = probe::probe_pixels(&device, &queue, wgpu_font_renderer::internals::atlas_resources(&store), &full);
            assert!(
                expected.as_chunks::<4>().0.iter().any(|p| p[0] > 128),
                "reference must contain ink"
            );
            let differences: Vec<_> = actual
                .as_chunks::<4>()
                .0
                .iter()
                .zip(expected.as_chunks::<4>().0)
                .enumerate()
                .filter(|(_, (a, e))| a != e)
                .take(8)
                .collect();
            assert!(
                differences.is_empty(),
                "LCD band/full-list mismatch for {c}: {differences:?}"
            );
            let debug = replace_shader(
                &shader,
                "if input.bands.z >= 0. && input.atlas_size > 0 {",
                "if input.bands.z >= 0. && input.atlas_size > 0 { return vec3(1., 0., 0.);",
            );
            let paths = probe::probe_pixels(&device, &queue, wgpu_font_renderer::internals::atlas_resources(&store), &debug);
            // Includes the 12 px and 16 px tiles (indices 9 and 10).
            for tile in 0..12 {
                assert_eq!(
                    &paths[tile * 64 * 256 * 4..][..3],
                    &[255, 0, 0],
                    "must take band path for tile {tile}"
                );
            }
        }
    }
}

#[test]
fn c7_device_without_feature_falls_back_byte_identically() {
    let Some((device, queue, config)) = common::gpu_with_lcd(false) else {
        return;
    };
    assert!(!device
        .features()
        .contains(wgpu::Features::DUAL_SOURCE_BLENDING));
    let mut store = FontStore::new(&device);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            "HiB@",
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    let paragraphs: Vec<_> = [12, 16, 24, 72]
        .into_iter()
        .enumerate()
        .map(|(i, size)| {
            writer
                .shape_text(
                    &store,
                    key,
                    [12.25, 8.5 + i as f32 * 24.],
                    size as f32,
                    [0.3, 0.6, 0.9, 0.5],
                    "HiB@",
                )
                .unwrap()
        })
        .collect();
    let mut gray = TextRenderer::new(&device, config.format, &store);
    assert!(!gray.lcd_active());
    gray.prepare(&device, &queue, &paragraphs, &store);
    let expected = draw::draw_pixels(&device, &queue, &mut gray, [128, 128], wgpu::Color::WHITE);
    assert!(expected
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[..3] != [255; 3]));
    for order in [SubpixelOrder::Rgb, SubpixelOrder::Bgr] {
        let mut lcd = TextRenderer::with_subpixel_order(&device, config.format, &store, order);
        assert!(!lcd.lcd_active());
        lcd.prepare(&device, &queue, &paragraphs, &store);
        assert_eq!(
            draw::draw_pixels(&device, &queue, &mut lcd, [128, 128], wgpu::Color::WHITE),
            expected
        );
    }
}

#[test]
fn c9_write_roboto_lcd_comparison_png() {
    let Some((device, queue, mut config)) = lcd_gpu() else {
        return;
    };
    config.width = 768;
    config.height = 240;
    let text = "Roboto: Hamburgefontsiv 012345";
    let mut store = FontStore::new(&device);
    let key = store
        .load_from_bytes(
            &device,
            &queue,
            include_bytes!("../fonts/Roboto-Regular.ttf"),
            text,
        )
        .unwrap();
    let mut writer = TypeWriter::new();
    let mut image = vec![255; (config.width * config.height * 4) as usize];
    let mut renders = Vec::new();
    for (column, order) in [SubpixelOrder::None, SubpixelOrder::Rgb, SubpixelOrder::Bgr]
        .into_iter()
        .enumerate()
    {
        let mut renderer =
            TextRenderer::with_subpixel_order(&device, config.format, &store, order);
        assert_eq!(renderer.lcd_active(), order != SubpixelOrder::None);
        let paragraphs: Vec<_> = [12, 16, 24]
            .into_iter()
            .enumerate()
            .map(|(row, size)| {
                writer
                    .shape_text(
                        &store,
                        key,
                        [12.25, 20. + row as f32 * 72.],
                        size as f32,
                        [0., 0., 0., 1.],
                        text,
                    )
                    .unwrap()
            })
            .collect();
        renderer.prepare(&device, &queue, &paragraphs, &store);
        let pixels = draw::draw_pixels(
            &device,
            &queue,
            &mut renderer,
            [768, 240],
            wgpu::Color::WHITE,
        );
        if column < 2 {
            for y in 0..240usize {
                let src = &pixels[y * 768 * 4..][..384 * 4];
                image[(y * 768 + column * 384) * 4..][..384 * 4].copy_from_slice(src);
            }
        }
        renders.push(pixels);
    }
    assert!(
        renders[1].as_chunks::<4>().0.iter().any(|p| p[0] != p[2]),
        "LCD must produce distinct channel coverages"
    );
    assert_ne!(
        renders[0], renders[1],
        "LCD must change actual public renderer output"
    );
    for (rgb, bgr) in renders[1]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(renders[2].as_chunks::<4>().0)
    {
        assert_eq!(
            *bgr,
            [rgb[2], rgb[1], rgb[0], rgb[3]],
            "public BGR option must swap channels exactly"
        );
    }
    write_png("target/lcd-compare.png", 768, 240, &image);
}

// Minimal RGBA PNG writer using uncompressed DEFLATE, so this test helper
// needs no new image dependency. Left: grayscale, right: RGB; rows: 12/16/24 px.
fn write_png(path: &str, width: u32, height: u32, rgba: &[u8]) {
    fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
        png.extend((bytes.len() as u32).to_be_bytes());
        let start = png.len();
        png.extend(kind);
        png.extend(bytes);
        let mut crc = !0u32;
        for byte in &png[start..] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        png.extend((!crc).to_be_bytes());
    }
    let mut raw = Vec::new();
    for row in rgba.chunks_exact(width as usize * 4) {
        raw.push(0);
        raw.extend(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks = raw.chunks(65535);
    let count = blocks.len();
    for (i, block) in blocks.enumerate() {
        zlib.push(u8::from(i + 1 == count));
        let len = block.len() as u16;
        zlib.extend(len.to_le_bytes());
        zlib.extend((!len).to_le_bytes());
        zlib.extend(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend(((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    std::fs::create_dir_all("target").unwrap();
    std::fs::write(path, png).unwrap();
}

//! cosmic-text 0.19 src/swash.rs: hint(!DISABLE_HINTING), default Attrs has no
//! disable flag; Format::Alpha masks, quarter-pixel X bins. src/layout.rs truncs
//! physical Y; glyphon 0.12 src/text_render.rs adds round(run.line_y) separately.
//! Report both default hinted glyphon and explicitly unhinted glyphon, with
//! identical integer pen/baseline conventions for every row.
//! ab_glyph 0.2 uses analytic area accumulation on adaptively flattened curves:
//! independent and unhinted, but not an exact mathematical oracle for curves.
use super::*;
use ab_glyph::{point, Font, FontRef, Glyph, OutlinedGlyph, PxScaleFactor};

const LATIN_GLYPHS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789.,:;!?@&";
const CJK_GLYPHS: &str = "龍鬱直接字形";
const DEFAULTS: &str = "cosmic-text 0.19 swash.rs enables hinting unless DISABLE_HINTING (default Attrs leaves it enabled), rasterizes Format::Alpha with quarter-pixel X bins; LayoutGlyph::physical truncates Y, glyphon 0.12 adds round(run.line_y). Defaults preserved; integral quality pens/baselines eliminate bin/snap offset differences.";

fn errors(reference: &[f32], actual: &[f32]) -> Value {
    assert_eq!(reference.len(), actual.len());
    let mut total = 0_f64;
    let mut maximum = 0_f32;
    let mut over = 0_usize;
    for (&a, &b) in reference.iter().zip(actual) {
        let error = (a - b).abs();
        total += f64::from(error);
        maximum = maximum.max(error);
        over += usize::from(error > 0.1);
    }
    json!({"mean_absolute_coverage_error": total / reference.len() as f64,
        "max_coverage_error": maximum, "pixels_error_gt_0_1": over, "pixels_compared": reference.len()})
}
// Cell-local moments prevent background dilution and remove absolute-position
// floating-point loss. Centroids use pixel centres and unquantized reference
// coverage; shape/hinting can also change moments, so warnings flag differences
// for inspection rather than asserting that every difference is an offset bug.
fn placement(
    reference: &[f32],
    actual: &[f32],
    width: u32,
    cell: u32,
    cells: &[Value],
) -> Result<Value> {
    let moments = |image: &[f32], slot: usize| {
        let x0 = slot as u32 % 10 * cell;
        let y0 = slot as u32 / 10 * cell;
        let mut mass = 0.;
        let mut x_sum = 0.;
        let mut y_sum = 0.;
        for y in 0..cell {
            for x in 0..cell {
                let value = f64::from(image[((y0 + y) * width + x0 + x) as usize]);
                mass += value;
                x_sum += value * (f64::from(x) + 0.5);
                y_sum += value * (f64::from(y) + 0.5);
            }
        }
        (mass, x_sum, y_sum)
    };
    let mut glyphs = Vec::new();
    let mut max_x: f64 = 0.;
    let mut max_y: f64 = 0.;
    let mut max_norm: f64 = 0.;
    let mut min_ratio = f64::INFINITY;
    let mut max_ratio: f64 = 0.;
    let mut max_mass_deviation: f64 = 0.;
    for (slot, glyph) in cells.iter().enumerate() {
        let (ref_mass, ref_x, ref_y) = moments(reference, slot);
        let (mass, x, y) = moments(actual, slot);
        if ref_mass <= 0. || mass <= 0. {
            return Err(format!(
                "empty quality glyph {}: cannot compute coverage centroid",
                glyph["char"]
            )
            .into());
        }
        let dx = x / mass - ref_x / ref_mass;
        let dy = y / mass - ref_y / ref_mass;
        let norm = dx.hypot(dy);
        let ratio = mass / ref_mass;
        max_x = max_x.max(dx.abs());
        max_y = max_y.max(dy.abs());
        max_norm = max_norm.max(norm);
        min_ratio = min_ratio.min(ratio);
        max_ratio = max_ratio.max(ratio);
        max_mass_deviation = max_mass_deviation.max((ratio - 1.).abs());
        glyphs.push(
            json!({"char": glyph["char"], "font": glyph["font"], "centroid_offset_px": [dx, dy],
            "centroid_offset_norm_px": norm, "coverage_mass_ratio": ratio,
            "centroid_warning": norm > 0.05, "mass_warning": (ratio - 1.).abs() > 0.01}),
        );
    }
    Ok(
        json!({"max_abs_centroid_x_px": max_x, "max_abs_centroid_y_px": max_y,
        "max_centroid_offset_norm_px": max_norm, "min_coverage_mass_ratio": min_ratio,
        "max_coverage_mass_ratio": max_ratio, "max_mass_deviation_fraction": max_mass_deviation,
        "centroid_warning": max_norm > 0.05, "mass_warning": max_mass_deviation > 0.01, "glyphs": glyphs}),
    )
}
fn draw_reference(scene: &Scene, width: u32, height: u32, coverage: &mut [f32]) -> Result<()> {
    let font = FontRef::try_from_slice(scene.bytes)?;
    let units = font
        .units_per_em()
        .ok_or("reference font lacks units_per_em")?;
    for item in &scene.items {
        let ch = item.text.chars().next().ok_or("empty quality glyph")?;
        let id = font.glyph_id(ch);
        if id.0 == 0 {
            return Err(format!("reference lacks {ch}").into());
        }
        let outline = font
            .outline(id)
            .ok_or_else(|| format!("reference lacks outline for {ch}"))?;
        // ab_glyph PxScale normally denotes ascent-minus-descent height. Supply
        // scale factors explicitly so 96 means 96 pixels PER EM on all three.
        // ab_glyph has y-up outlines and y-down pixel coordinates internally;
        // Glyph.position is the baseline pen, with no half-pixel adjustment.
        let glyph = Glyph {
            id,
            scale: item.size.into(),
            position: point(item.position[0], (item.position[1] + item.ascent).round()),
        };
        let factor = item.size / units;
        let outlined = OutlinedGlyph::new(
            glyph,
            outline,
            PxScaleFactor {
                horizontal: factor,
                vertical: factor,
            },
        );
        let bounds = outlined.px_bounds();
        if bounds.min.x < 0.
            || bounds.min.y < 0.
            || bounds.max.x > width as f32
            || bounds.max.y > height as f32
        {
            return Err(format!("quality glyph {ch} is clipped in reference").into());
        }
        outlined.draw(|x, y, value| {
            let px = x + bounds.min.x as u32;
            let py = y + bounds.min.y as u32;
            coverage[(py * width + px) as usize] = value.clamp(0., 1.);
        });
    }
    Ok(())
}
fn render_grid(
    gpu: &Gpu,
    scenes: &[Scene],
    index: usize,
    width: u32,
    height: u32,
    unhinted: bool,
) -> Result<Vec<u8>> {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vs_glyphon linear quality grid"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut renderers = Vec::new();
    for scene in scenes {
        let (mut renderer, _, _) =
            Renderer::new_format(index, scene, gpu, wgpu::TextureFormat::Rgba8Unorm)?;
        renderer.shape(scene, &scene.texts(None))?;
        if unhinted {
            if let Renderer::Glyphon(state) = &mut renderer {
                for (buffer, item) in state.buffers.iter_mut().zip(&scene.items) {
                    let attrs = Attrs::new()
                        .family(Family::Name(&scene.family))
                        .cache_key_flags(glyphon::cosmic_text::CacheKeyFlags::DISABLE_HINTING);
                    buffer.set_text(&item.text, &attrs, Shaping::Advanced, None);
                    buffer.shape_until_scroll(&mut state.font_system, false);
                }
            }
        }
        if let Renderer::Ours(state) = &mut renderer {
            state.renderer.resize(&gpu.queue, [width, height]);
        }
        renderer.prepare_target(scene, gpu, [width, height], true)?;
        renderers.push(renderer);
    }
    let pixels = buffer(
        &gpu.device,
        u64::from(width) * u64::from(height) * 4,
        wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    );
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &attachments,
            ..Default::default()
        });
        for renderer in &renderers {
            renderer.render(&mut pass)?;
        }
    }
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &pixels,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    gpu.queue.submit(Some(encoder.finish()));
    gpu.wait()?;
    for renderer in &mut renderers {
        renderer.trim();
    }
    read_buffer(&gpu.device, &pixels)
}
pub(super) fn run(gpu: &Gpu, artifacts: &std::path::Path) -> Result<Value> {
    // A self-comparison alone cannot detect a broken placement checker.
    let cells = [json!({"char": "synthetic", "font": "test"})];
    let mut reference = [0.; 16];
    reference[5] = 1.;
    let mut translated = [0.; 16];
    translated[6] = 1.;
    let translated_check = placement(&reference, &translated, 4, 4, &cells)?;
    assert_eq!(translated_check["max_abs_centroid_x_px"], 1.);
    assert_eq!(translated_check["centroid_warning"], true);
    assert_eq!(translated_check["mass_warning"], false);
    let mut half_mass = reference;
    half_mass[5] = 0.5;
    let mass_check = placement(&reference, &half_mass, 4, 4, &cells)?;
    assert_eq!(mass_check["max_centroid_offset_norm_px"], 0.);
    assert_eq!(mass_check["mass_warning"], true);
    std::fs::create_dir_all(artifacts)?;
    println!("\nQuality: isolated glyphs, integer pens/baselines, linear Rgba8Unorm; independent unhinted ab_glyph coverage reference.");
    println!("glyphon defaults: {DEFAULTS}");
    println!("| Size px | Renderer | Mean absolute error | Max error | Pixels error > 0.1 | Max abs dx px | Max abs dy px | Mass ratio min/max | Max mass deviation % |");
    println!("|---:|---|---:|---:|---:|---:|---:|---|---:|");
    let mut results = Vec::new();
    for size in [12., 16., 24., 48., 96.] {
        let cell = (size * 1.75 + 32_f32).ceil() as u32;
        let count = LATIN_GLYPHS.chars().count() + CJK_GLYPHS.chars().count();
        let width = (cell * 10).div_ceil(64) * 64; // packed RGBA rows satisfy 256-byte copy alignment.
        let height = (count as u32).div_ceil(10) * cell;
        let mut scenes = Vec::new();
        let mut cells = Vec::new();
        let mut offset = 0;
        for (name, bytes, chars) in [
            ("quality_latin", ROBOTO, LATIN_GLYPHS),
            ("cjk_24px", NOTO, CJK_GLYPHS),
        ] {
            let items: Vec<_> = chars.chars().enumerate().map(|(i, ch)| {
                let slot = i + offset;
                let x = (slot as u32 % 10) * cell + 16;
                let baseline = (slot as u32 / 10) * cell + 16 + (size * 1.3).ceil() as u32;
                cells.push(json!({"char": ch.to_string(), "font": name, "pen_x": x, "baseline_y": baseline}));
                Item { text: ch.to_string(), size, position: [x as f32, baseline as f32], width: cell as f32 - 16., ascent: 0., line_height: 0. }
            }).collect();
            offset += items.len();
            let mut scene = Scene::from_items(name, bytes, items, gpu)?;
            for item in &mut scene.items {
                item.position[1] -= item.ascent;
            }
            scenes.push(scene);
        }
        let mut reference = vec![0.; (width * height) as usize];
        for scene in &scenes {
            draw_reference(scene, width, height, &mut reference)?;
        }
        let self_check = errors(&reference, &reference);
        assert_eq!(self_check["mean_absolute_coverage_error"], 0.);
        assert_eq!(self_check["max_coverage_error"], 0.);
        assert_eq!(self_check["pixels_error_gt_0_1"], 0);
        let self_placement = placement(&reference, &reference, width, cell, &cells)?;
        assert_eq!(self_placement["max_centroid_offset_norm_px"], 0.);
        assert_eq!(self_placement["max_mass_deviation_fraction"], 0.);
        println!("| {size:.0} | reference self-check | 0 | 0 | 0 | 0 | 0 | 1/1 | 0 |");
        let reference_file = format!("quality_reference_{size:.0}.rgba");
        let reference_bytes: Vec<u8> = reference
            .iter()
            .flat_map(|&coverage| {
                let v = ((1. - coverage) * 255.).round() as u8;
                [v, v, v, 255]
            })
            .collect();
        std::fs::write(artifacts.join(&reference_file), &reference_bytes)?;
        let mut renderers = Vec::new();
        for (index, name, file_name, unhinted) in [
            (0, "wgpu-font-renderer", "wgpu-font-renderer", false),
            (2, "wgpu-font-renderer (cached)", "wgpu-font-renderer_cached", false),
            (1, "glyphon (hinted, default)", "glyphon", false),
            (1, "glyphon (unhinted)", "glyphon_unhinted", true),
        ] {
            let rgba = render_grid(gpu, &scenes, index, width, height, unhinted)?;
            // Monochrome grids should have equal RGB and opaque alpha. Validate
            // this invariant before interpreting byte values as coverage.
            assert!(rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[0] == p[1] && p[1] == p[2] && p[3] == 255));
            let actual: Vec<_> = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| 1. - f32::from(p[0]) / 255.)
                .collect();
            let metrics = errors(&reference, &actual);
            let positions = placement(&reference, &actual, width, cell, &cells)?;
            let file = format!("quality_{file_name}_{size:.0}.rgba");
            std::fs::write(artifacts.join(&file), rgba)?;
            println!(
                "| {size:.0} | {name} | {:.6} | {:.6} | {} | {:.4} | {:.4} | {:.5}/{:.5} | {:.3} |",
                metrics["mean_absolute_coverage_error"].as_f64().unwrap(),
                metrics["max_coverage_error"].as_f64().unwrap(),
                metrics["pixels_error_gt_0_1"],
                positions["max_abs_centroid_x_px"].as_f64().unwrap(),
                positions["max_abs_centroid_y_px"].as_f64().unwrap(),
                positions["min_coverage_mass_ratio"].as_f64().unwrap(),
                positions["max_coverage_mass_ratio"].as_f64().unwrap(),
                positions["max_mass_deviation_fraction"].as_f64().unwrap() * 100.
            );
            if positions["centroid_warning"] == true || positions["mass_warning"] == true {
                println!("WARNING: quality {size:.0}px {name}: per-glyph centroid norm max={:.4}px (limit 0.05px), mass deviation max={:.3}% (limit 1%); inspect placement, hinting and rasterization differences.",
                    positions["max_centroid_offset_norm_px"].as_f64().unwrap(), positions["max_mass_deviation_fraction"].as_f64().unwrap() * 100.);
            }
            renderers.push(json!({"renderer": name, "hinting": if index != 1 { "none" } else if unhinted { "disabled via Attrs::cache_key_flags" } else { "default enabled" }, "errors": metrics, "placement": positions, "raw_file": file}));
        }
        results.push(json!({"size_px": size, "width": width, "height": height, "cell_size": cell,
            "cells": cells, "reference_raw_file": reference_file, "reference_self_check": self_check, "reference_self_placement": self_placement, "renderers": renderers}));
    }
    Ok(
        json!({"format": "Rgba8Unorm", "raw_encoding": "packed row-major RGBA8, white background, black text, opaque alpha",
        "artifact_directory": artifacts, "reference": "ab_glyph 0.2 unhinted outlines with explicit pixels-per-em scale factors; analytic area rasterization with curve flattening",
        "reference_limits": "independent approximation, not a mathematical oracle; raw reference is quantized to 8-bit but metrics use float coverage; MAE includes white cell background and row padding",
        "placement_thresholds": {"centroid_norm_px": 0.05, "mass_deviation_fraction": 0.01},
        "glyphon_defaults": DEFAULTS, "baseline_convention": "integer baseline pen; glyphon top = baseline - round(run.line_y), this crate top = baseline - FontStore ascent, ab_glyph Glyph.position = baseline",
        "grids": results}),
    )
}

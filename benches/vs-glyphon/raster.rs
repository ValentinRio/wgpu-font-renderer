//! Cold unique-key bitmap generation, with shaping and pipeline setup excluded.
use super::*;
use glyphon::cosmic_text::{CacheKey, CacheKeyFlags, SubpixelBin, SwashContent};
use std::collections::HashSet;
use wgpu_font_renderer::{internals, GlyphId, Paragraph, RenderMode};

fn stats(samples: &[f64], count: usize) -> Value {
    let mut value = summary(samples);
    value["median_us_per_glyph"] = json!(quantile(samples, 0.5) * 1000. / count as f64);
    value["p95_us_per_glyph"] = json!(quantile(samples, 0.95) * 1000. / count as f64);
    value
}
fn cell(value: &Value) -> String {
    if value.is_null() {
        return "unavailable".into();
    }
    format!(
        "{:.3}/{:.3}; {:.3}/{:.3}",
        value["median_ms"].as_f64().unwrap(),
        value["p95_ms"].as_f64().unwrap(),
        value["median_us_per_glyph"].as_f64().unwrap(),
        value["p95_us_per_glyph"].as_f64().unwrap()
    )
}

pub(super) fn run(gpu: &Gpu, scenes: &[Scene], rounds: usize) -> Result<Value> {
    println!("\nRaster: cold unique (glyph, size, subpixel bin) bitmaps; cells = median/p95 total ms; median/p95 µs/glyph over rounds.");
    println!("Glyphon CPU = hinted/default SwashCache::get_image_uncached, as prepare uses; upload = queue.write_texture calls only (packing/texture creation excluded). Cached CPU = prepare misses: lookup, allocation/growth, instance upload, raster encoding and submit; shaping and completion excluded. GPU = raster clear/bake/store only, growth copies excluded; unavailable without timestamps. Empty/no-image keys produce no bitmap and are excluded from both sides.");
    println!("| Scene | Glyphs (both) | Glyphon CPU raster | Glyphon CPU upload | Cached CPU encode | Cached GPU raster |");
    println!("|---|---:|---:|---:|---:|---:|");
    let mut results = Vec::new();
    // One renderer owns the same bitmap pipelines for every scene and round.
    let mut store = FontStore::new(&gpu.device);
    let mut renderer = TextRenderer::new(&gpu.device, FORMAT, &store);
    let start = Instant::now();
    internals::benchmark_raster(
        &mut renderer,
        &gpu.device,
        gpu.queries
            .as_ref()
            .map(|q| (q.clone(), gpu.encoder_timestamps)),
    );
    let pipeline_ms = ms(start);
    for scene in scenes {
        let (mut discovery, _, _) = Renderer::new(1, scene, gpu)?;
        discovery.shape(scene, &scene.texts(None))?;
        let Renderer::Glyphon(state) = &mut discovery else {
            unreachable!()
        };
        let mut seen = HashSet::new();
        let mut keys: Vec<CacheKey> = Vec::new();
        // Match glyphon 0.12 text_render.rs: visible RUNS, then physical keys;
        // per-glyph clipping occurs AFTER the raster/upload, so do not clip X/ink.
        for (buffer, item) in state.buffers.iter().zip(&scene.items) {
            let first = buffer
                .layout_runs()
                .next()
                .map_or(item.ascent, |r| r.line_y);
            let top = item.position[1] + item.ascent - first;
            let visible = |run: &glyphon::cosmic_text::LayoutRun<'_>| {
                let start = (top + run.line_top) as i32;
                let end = start + run.line_height as i32;
                start <= HEIGHT as i32 && 0 <= end
            };
            for run in buffer
                .layout_runs()
                .skip_while(|r| !visible(r))
                .take_while(visible)
            {
                for glyph in run.glyphs {
                    let key = glyph.physical((item.position[0], top), 1.).cache_key;
                    if seen.insert(key) {
                        keys.push(key);
                    }
                }
            }
        }
        let candidate_count = keys.len();
        let mut probe = SwashCache::new();
        keys.retain(|&key| {
            probe
                .get_image_uncached(&mut state.font_system, key)
                .is_some_and(|image| image.placement.width > 0 && image.placement.height > 0)
        });
        if keys.is_empty() {
            return Err(format!("{}: no raster keys", scene.name).into());
        }
        // Preserve first-use order. The public Paragraph API accepts glyph IDs,
        // so both rasterizers receive exactly the same keys without re-shaping.
        let font = store.load_from_bytes(&gpu.device, &gpu.queue, scene.bytes, &scene.chars)?;
        let paragraphs: Vec<_> = keys
            .iter()
            .map(|key| {
                assert_eq!(key.flags, CacheKeyFlags::empty());
                assert_eq!(key.font_weight, glyphon::fontdb::Weight::NORMAL);
                assert_eq!(key.y_bin, SubpixelBin::Zero);
                let size = f32::from_bits(key.font_size_bits);
                let ascent = store.metrics(font, size).unwrap().ascent;
                let mut p = Paragraph::new(
                    font,
                    [32. + key.x_bin.as_float(), 512. - ascent],
                    size,
                    [0., 0., 0., 1.],
                );
                p.push(GlyphId(key.glyph_id), 0.);
                p.set_render_mode(RenderMode::Cached);
                p
            })
            .collect();
        // Pack actual Swash image dimensions outside upload samples, into a real
        // R8 atlas. Upload format/stride and one write per image match glyphon.
        let images: Vec<_> = keys
            .iter()
            .map(|&key| {
                probe
                    .get_image_uncached(&mut state.font_system, key)
                    .unwrap()
            })
            .collect();
        let dimension = gpu.device.limits().max_texture_dimension_2d;
        let mut packer = etagere::BucketedAtlasAllocator::new(etagere::size2(
            dimension as i32,
            dimension as i32,
        ));
        let mut positions = Vec::new();
        for image in &images {
            if image.content != SwashContent::Mask {
                return Err("raster scene contains non-mask glyph".into());
            }
            let a = packer
                .allocate(etagere::size2(
                    image.placement.width as i32,
                    image.placement.height as i32,
                ))
                .ok_or("raster upload atlas full")?;
            positions.push(wgpu::Origin3d {
                x: a.rectangle.min.x as u32,
                y: a.rectangle.min.y as u32,
                z: 0,
            });
        }
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("raster glyphon upload atlas"),
            size: wgpu::Extent3d {
                width: dimension,
                height: dimension,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.submit([]);
        gpu.wait()?;
        let mut cpu = Vec::new();
        let mut upload = Vec::new();
        let mut encode = Vec::new();
        let mut execution = Vec::new();
        for round in 0..rounds {
            // Alternate renderer order to distribute drift. All cache resets and
            // draining of earlier GPU work happen outside measured intervals.
            for order in 0..2 {
                if (round + order) % 2 == 0 {
                    let mut swash = SwashCache::new();
                    let start = Instant::now();
                    let images: Vec<_> = keys
                        .iter()
                        .map(|&key| {
                            swash
                                .get_image_uncached(&mut state.font_system, key)
                                .unwrap()
                        })
                        .collect();
                    cpu.push(ms(start));
                    let start = Instant::now();
                    for (image, &origin) in images.iter().zip(&positions) {
                        gpu.queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &texture,
                                mip_level: 0,
                                origin,
                                aspect: wgpu::TextureAspect::All,
                            },
                            &image.data,
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(image.placement.width),
                                rows_per_image: None,
                            },
                            wgpu::Extent3d {
                                width: image.placement.width,
                                height: image.placement.height,
                                depth_or_array_layers: 1,
                            },
                        );
                    }
                    upload.push(ms(start));
                    gpu.queue.submit([]);
                    gpu.wait()?;
                } else {
                    internals::set_bitmap_cache_limits(&mut renderer, &gpu.device, 512, 4096);
                    gpu.queue.submit([]);
                    gpu.wait()?;
                    let start = Instant::now();
                    renderer.prepare(&gpu.device, &gpu.queue, &paragraphs, &store);
                    encode.push(ms(start));
                    let (resident, cached) = internals::bitmap_cache_counts(&renderer);
                    if resident != keys.len() || cached != keys.len() {
                        return Err(format!("{}: {} keys but {resident} resident/{cached} cached; missing outlines or fallback", scene.name, keys.len()).into());
                    }
                    if let Some(queries) = &gpu.queries {
                        let mut encoder = gpu.device.create_command_encoder(&Default::default());
                        encoder.resolve_query_set(queries, 0..2, &gpu.resolve, 0);
                        encoder.copy_buffer_to_buffer(&gpu.resolve, 0, &gpu.timing_readback, 0, 16);
                        gpu.queue.submit([encoder.finish()]);
                        let data = read_buffer(&gpu.device, &gpu.timing_readback)?;
                        let begin = u64::from_ne_bytes(data[..8].try_into()?);
                        let end = u64::from_ne_bytes(data[8..].try_into()?);
                        let elapsed = end.checked_sub(begin).ok_or("raster timestamp underflow")?
                            as f64
                            * f64::from(gpu.queue.get_timestamp_period())
                            / 1e6;
                        if !elapsed.is_finite() || elapsed <= 0. || elapsed > 60_000. {
                            return Err("invalid raster timestamp duration".into());
                        }
                        execution.push(elapsed);
                    }
                    gpu.wait()?;
                }
            }
        }
        let count = keys.len();
        let value = json!({"scene": scene.name, "glyph_count": count, "glyphon_glyph_count": count,
            "cached_glyph_count": count, "candidate_key_count": candidate_count,
            "excluded_empty_or_no_image_keys": candidate_count - count, "rounds": rounds,
            "keys": keys.iter().map(|key| json!({"glyph_id": key.glyph_id,
                "font_size_bits": key.font_size_bits, "x_bin": key.x_bin.as_float(),
                "y_bin": key.y_bin.as_float(), "flags": key.flags.bits(),
                "font_weight": key.font_weight.0})).collect::<Vec<_>>(),
            "glyphon_cpu_raster": stats(&cpu, count), "glyphon_cpu_upload": stats(&upload, count),
            "cached_cpu_encode": stats(&encode, count),
            "cached_gpu_raster": if execution.is_empty() { Value::Null } else { stats(&execution, count) }});
        println!(
            "| {} | {count} | {} | {} | {} | {} |",
            scene.name,
            cell(&value["glyphon_cpu_raster"]),
            cell(&value["glyphon_cpu_upload"]),
            cell(&value["cached_cpu_encode"]),
            cell(&value["cached_gpu_raster"])
        );
        println!("Raster keys {}: {candidate_count} candidates, {} empty/no-image excluded; both sides {count} bitmaps.", scene.name, candidate_count - count);
        results.push(value);
    }
    println!("Raster pipeline/shader creation (once, CPU; includes initial bitmap atlas/resources): {pipeline_ms:.3} ms");
    Ok(
        json!({"pipeline_creation_ms": pipeline_ms, "pipeline_creation_includes": "bitmap shader, draw/bake/clear pipelines and initial atlas/resources; base renderer initialization excluded", "clock": if gpu.queries.is_some() { "gpu_timestamp" } else { "unavailable" }, "timestamp_source": if gpu.queries.is_none() { None } else if gpu.encoder_timestamps { Some("encoder") } else { Some("pass") }, "intervals": {
        "glyphon_cpu_raster": "hinted/default get_image_uncached on unique bitmap keys; fresh SwashCache; image collection included",
        "glyphon_cpu_upload": "one R8 queue.write_texture per image; packing, allocation, growth and completion excluded",
        "cached_cpu_encode": "cold prepare: lookup, allocation/growth, raster encoding, uploads and submit; shaping, reset, pipeline creation and completion excluded",
        "cached_gpu_raster": "raster pass clear/bake/load/store only; growth copies excluded; unamortized",
        "workload": "glyphon prepare visible-run physical keys in first-use order; empty/no-image keys excluded from both sides"
    }, "scenes": results}),
    )
}

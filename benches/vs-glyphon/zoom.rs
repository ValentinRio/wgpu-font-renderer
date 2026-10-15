use super::*;

pub(super) fn run(gpu: &Gpu, memory_enabled: bool) -> Result<Value> {
    // 128 exact f32 sizes ascending, then the same sizes descending (no repeated
    // endpoint). The sequence is identical on both sides, and repeated intact.
    let ascending: Vec<f32> = (0..128)
        .map(|i| 8. * 32_f32.powf(i as f32 / 127.))
        .collect();
    assert!(ascending.windows(2).all(|pair| pair[0] < pair[1]));
    let sizes: Vec<_> = ascending
        .iter()
        .copied()
        .chain(ascending[..127].iter().rev().copied())
        .collect();
    let item = Item {
        text: "Zoom text".into(),
        size: 8.,
        position: [32., 24.],
        width: 1856.,
        ascent: 0.,
        line_height: 0.,
    };
    let mut scene = Scene::from_items("zoom", ROBOTO, vec![item], gpu)?;
    let unit_ascent = scene.items[0].ascent / 8.;
    let unit_height = scene.items[0].line_height / 8.;
    let texts = scene.texts(None);
    let mut results = Vec::new();
    println!("\nZoom stress: 128 sizes, 255 frames/pass, two passes in EACH order: ours→glyphon→cached and cached→glyphon→ours; wall interval includes relayout, prepare, encoding, upload, render and submit+wait; query readback excluded. GPU columns are unamortized single draws including clear/store, unlike static R-amortized times.");
    println!("| Order | Renderer | Pass | Wall median | Wall p95 | Wall max | GPU single-draw median (clear included) | GPU single-draw p95 | GPU single-draw max | Worst frames (index:size px) |");
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---|");
    // Each renderer has fresh caches at pass 1, and retains all its own state
    // into pass 2. No size prewarming; do not call prepare before the sweep.
    for order in 0..2 {
        let order_name = if order == 0 {
            "ours→glyphon→cached"
        } else {
            "cached→glyphon→ours"
        };
        for offset in 0..3 {
            let index = if order == 0 { offset } else { 2 - offset };
            let before = if memory_enabled {
                memory::snapshot(gpu)?
            } else {
                None
            };
            let (mut renderer, _, _) = Renderer::new(index, &scene, gpu)?;
            gpu.queue.submit([]);
            gpu.wait()?;
            for pass in 1..=2 {
                let mut wall = Vec::new();
                let mut timestamp = Vec::new();
                for (frame, &size) in sizes.iter().enumerate() {
                    let item = &mut scene.items[0];
                    item.size = size;
                    item.ascent = unit_ascent * size;
                    item.line_height = unit_height * size;
                    let start = Instant::now();
                    match &mut renderer {
                        Renderer::Glyphon(state) if !state.buffers.is_empty() => {
                            // In an app changing font size, keep text/buffers, invalidate
                            // layout with set_metrics, then shape at the new metrics.
                            for buffer in &mut state.buffers {
                                buffer.set_metrics(Metrics::new(size, item.line_height));
                                buffer.shape_until_scroll(&mut state.font_system, false);
                            }
                        }
                        _ => renderer.shape(&scene, &texts)?,
                    }
                    renderer.prepare(&scene, gpu)?;
                    let (draw_ms, completed) =
                        gpu.frame_details(&renderer, 1, true, false, false)?;
                    let wall_ms = completed.duration_since(start).as_secs_f64() * 1000.;
                    // Readback has completed now, outside wall_ms. Trim after every frame,
                    // including turn-around and pass boundaries, exactly as in glyphon docs.
                    renderer.trim();
                    wall.push(wall_ms);
                    if gpu.queries.is_some() {
                        timestamp.push(draw_ms);
                    }
                    assert!(wall_ms.is_finite() && wall_ms > 0., "zoom frame {frame}");
                }
                let mut worst: Vec<_> = (0..wall.len()).collect();
                worst.sort_by(|&a, &b| wall[b].total_cmp(&wall[a]));
                worst.truncate(5);
                let worst_json: Vec<_> = worst
                    .iter()
                    .map(|&i| json!({"frame": i, "size_px": sizes[i], "wall_ms": wall[i]}))
                    .collect();
                let worst_text = worst
                    .iter()
                    .map(|&i| format!("{i}:{:.2}", sizes[i]))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut timing = summary(&wall);
                timing["max_ms"] = json!(quantile(&wall, 1.));
                let gpu_timing = if timestamp.is_empty() {
                    Value::Null
                } else {
                    let mut value = summary(&timestamp);
                    value["max_ms"] = json!(quantile(&timestamp, 1.));
                    value
                };
                let gpu_cell = |q| {
                    if timestamp.is_empty() {
                        "unavailable".into()
                    } else {
                        format!("{:.3}", quantile(&timestamp, q))
                    }
                };
                println!(
                "| {order_name} | {} | {pass} | {:.3} | {:.3} | {:.3} | {} | {} | {} | {worst_text} |",
                renderer_name(index),
                quantile(&wall, 0.5),
                quantile(&wall, 0.95),
                quantile(&wall, 1.),
                gpu_cell(0.5),
                gpu_cell(0.95),
                gpu_cell(1.)
            );
                let held = if memory_enabled {
                    memory::capture(gpu, &renderer, before.as_ref())?
                } else {
                    Value::Null
                };
                results.push(
                json!({"order": order_name, "renderer": renderer_name(index), "pass": pass, "wall": timing,
                "gpu": gpu_timing, "worst_frames": worst_json, "memory": held}),
            );
            }
            drop(renderer);
            gpu.wait()?;
        }
    }
    if memory_enabled {
        memory::print_header();
        for result in &results {
            memory::print_row(
                &format!(
                    "zoom {} pass {}",
                    result["order"].as_str().unwrap(),
                    result["pass"]
                ),
                result["renderer"].as_str().unwrap(),
                &result["memory"],
            );
        }
    }
    Ok(
        json!({"sizes_px": sizes, "distinct_sizes": 128, "frame_index_base": 0,
        "wall_interval": "relayout + prepare + encoding + uploads + render + submit/wait; font/renderer initialization, query readback and post-frame trim excluded",
        "orders": ["ours→glyphon→cached", "cached→glyphon→ours"],
        "gpu_interval": "unamortized single draw including white clear/store; uploads precede timestamp",
        "passes": results}),
    )
}

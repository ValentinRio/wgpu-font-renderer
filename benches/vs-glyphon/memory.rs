//! Subtract isolated, settled allocator snapshots by exact resource labels.
//! Device totals and allocator reservations are never attributed to a renderer.
use super::*;

pub(super) fn snapshot(gpu: &Gpu) -> Result<Option<wgpu::AllocatorReport>> {
    gpu.queue.submit([]);
    gpu.wait()?;
    Ok(gpu.device.generate_allocator_report())
}
fn number(bytes: u64, source: &str) -> Value {
    json!({"bytes": bytes, "source": source})
}
fn unknown(reason: &str) -> Value {
    json!({"bytes": null, "source": "unknown", "reason": reason})
}
fn labelled(report: &wgpu::AllocatorReport, labels: &[&str]) -> u64 {
    report
        .allocations
        .iter()
        .filter(|a| labels.contains(&a.name.as_str()))
        .map(|a| a.size)
        .sum()
}
pub(super) fn capture(
    gpu: &Gpu,
    renderer: &Renderer,
    before: Option<&wgpu::AllocatorReport>,
) -> Result<Value> {
    let after = snapshot(gpu)?;
    let (outline_labels, bitmap_labels, buffer_labels): (&[&str], &[&str], &[&str]) = match renderer
    {
        Renderer::Ours(_) => (
            &["Atlas Texture"],
            &["Bitmap glyph atlas"],
            &[
                "Text uniforms buffer",
                "Bitmap uniforms buffer",
                "Text vertex buffer",
                "Text indice buffer",
                "Text instances buffer",
            ],
        ),
        Renderer::Glyphon(_) => (
            &[],
            &["glyphon atlas"],
            &["glyphon vertices", "glyphon params"],
        ),
    };
    let mut result = match renderer {
        Renderer::Ours(state) => {
            let view = wgpu_font_renderer::internals::atlas_resources(&state.store);
            let extent = view.texture().size();
            let outlines = u64::from(extent.width)
                * u64::from(extent.height)
                * u64::from(extent.depth_or_array_layers)
                * 16;
            let bitmap_size = u64::from(wgpu_font_renderer::internals::bitmap_cache_size(
                &state.renderer,
            ));
            let bitmap = bitmap_size * bitmap_size;
            let instances = wgpu_font_renderer::internals::instance_buffer(&state.renderer)
                .map_or(0, wgpu::Buffer::size);
            // src/renderer.rs: VERTICES = 4 × [f32; 3], INDICES = 6 × u16;
            // Params = Resolution (2 × u32), padding (2 × u32), transform (16 × f32).
            // src/renderer/bitmap.rs retains one extra Params when initialized.
            const GEOMETRY_BYTES: u64 = (size_of::<[[f32; 3]; 4]>() + size_of::<[u16; 6]>()) as u64;
            const UNIFORM_BYTES: u64 =
                (size_of::<[u32; 2]>() + size_of::<[u32; 2]>() + size_of::<[f32; 16]>()) as u64;
            let bitmap_uniforms = if bitmap == 0 { 0 } else { UNIFORM_BYTES };
            let buffers = instances + GEOMETRY_BYTES + UNIFORM_BYTES + bitmap_uniforms;
            let payload =
                wgpu_font_renderer::internals::font_written_bytes(&state.store, state.key)
                    .ok_or("missing loaded font")?;
            json!({"outlines": number(outlines, "computed"), "bitmap": number(bitmap, "computed"),
                "textures": number(outlines + bitmap, "computed"), "buffers": number(buffers, "computed"), "total": number(outlines + bitmap + buffers, "computed"),
                "font_payloads": [{"outlines_bytes": payload[0], "bands_bytes": payload[1], "written_bytes": payload[0] + payload[1]}],
                "atlas_written_bytes": wgpu_font_renderer::internals::atlas_written_bytes(&state.store),
                "components": {"instances": number(instances, "computed"),
                    "geometry": number(GEOMETRY_BYTES, "computed"),
                    "uniforms": number(UNIFORM_BYTES, "computed"),
                    "bitmap_uniforms": number(bitmap_uniforms, "computed")},
                "scope": "logical resource sizes; full texture extents; driver alignment/overhead excluded"})
        }
        Renderer::Glyphon(_) => json!({"outlines": number(0, "computed"),
            "bitmap": unknown("glyphon 0.12 mask/color atlas dimensions are private; growth and eviction prevent inferring capacity"),
            "textures": unknown("glyphon atlas capacities unavailable"),
            "buffers": unknown("glyphon vertex buffer capacity is private"), "total": unknown("atlas and buffer capacities unavailable"),
            "font_payloads": [], "scope": "GPU resources only; glyphon has no GPU outline atlas"}),
    };
    result["computed_total"] = result["total"].clone();
    result["computed_vs_report_warning"] = json!(false);
    if let (Some(before), Some(after)) = (before, after.as_ref()) {
        let delta = |labels| labelled(after, labels).checked_sub(labelled(before, labels));
        if let (Some(outlines), Some(bitmap), Some(buffers)) = (
            delta(outline_labels),
            delta(bitmap_labels),
            delta(buffer_labels),
        ) {
            let bitmap_expected = !matches!(renderer, Renderer::Ours(state) if wgpu_font_renderer::internals::bitmap_cache_size(&state.renderer) == 0);
            let outlines_expected = matches!(renderer, Renderer::Ours(_));
            // Zero is valid for an absent resource; missing expected labels are not zero.
            if (!outlines_expected || outlines > 0)
                && (!bitmap_expected || bitmap > 0)
                && buffers > 0
            {
                if let Some(computed) = result["computed_total"]["bytes"].as_u64() {
                    let reported = outlines + bitmap + buffers;
                    // Allow allocator alignment: the larger of 4 KiB or 1% of the report total.
                    result["computed_vs_report_warning"] =
                        json!(computed.abs_diff(reported) > 4096.max(reported / 100));
                }
                result["outlines"] = number(outlines, "allocator_report");
                result["bitmap"] = number(bitmap, "allocator_report");
                result["textures"] = number(outlines + bitmap, "allocator_report");
                result["buffers"] = number(buffers, "allocator_report");
                result["total"] = number(outlines + bitmap + buffers, "allocator_report");
                result["scope"] = json!(
                    "live resource allocations; settled isolated baseline subtracted by exact labels; allocator reservations excluded"
                );
            }
        }
    }
    result["allocator_report_available"] = json!(after.is_some());
    if let Some(after) = after {
        result["device_allocated"] = number(after.total_allocated_bytes, "allocator_report");
        result["device_reserved"] = number(after.total_reserved_bytes, "allocator_report");
    }
    result["allocation_labels"] =
        json!({"outlines": outline_labels, "bitmap": bitmap_labels, "buffers": buffer_labels});
    Ok(result)
}
pub(super) fn print_header() {
    println!(
        "\nGPU memory held (bytes; per-cell sources; isolated label deltas, reservations excluded)"
    );
    println!(
        "Outline atlas: packed payload, at most one 8 KiB row slack per physical layer. R8 bitmap: lazy 512-square atlas, grows on demand. Instance buffer: exact growth, retained high-water capacity. Each scene/zoom renderer loads one font instance; Font data is bytes of outlines + y bands actually written (padding excluded). Glyphon bitmap includes mask/color textures."
    );
    println!(
        "| After | Renderer | Outline atlas | Bitmap | Buffers | Total | Font data (outline + bands) |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|");
}
pub(super) fn print_row(after: &str, name: &str, value: &Value) {
    if value["computed_vs_report_warning"] == true {
        println!(
            "WARNING: memory {after} {name}: computed total {} B differs from allocator-report total {} B beyond the larger of 4 KiB or 1% of the report total.",
            value["computed_total"]["bytes"], value["total"]["bytes"]
        );
    }
    let cell = |v: &Value| match v["bytes"].as_u64() {
        Some(n) => format!("{n} ({})", v["source"].as_str().unwrap()),
        None => "unknown (private capacity)".into(),
    };
    let payload = value["font_payloads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| format!("{} + {}", p["outlines_bytes"], p["bands_bytes"]))
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "| {after} | {name} | {} | {} | {} | {} | {} |",
        cell(&value["outlines"]),
        cell(&value["bitmap"]),
        cell(&value["buffers"]),
        cell(&value["total"]),
        if payload.is_empty() { "—" } else { &payload }
    );
}

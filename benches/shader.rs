#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    if let Err(error) = native::run() {
        eprintln!("shader benchmark: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod native {
    use owned_ttf_parser::AsFaceRef;
    use serde_json::{json, Value};
    use std::{
        collections::HashMap,
        error::Error,
        path::{Path, PathBuf},
        time::Instant,
    };
    use wgpu_font_renderer::{FontStore, TextRenderer, TypeWriter};

    type Result<T> = std::result::Result<T, Box<dyn Error>>;
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const SCENES: [&str; 4] = ["small_text", "large_glyphs", "complex_glyphs", "cff_text"];

    fn count_env(name: &str, default: usize) -> Result<usize> {
        match std::env::var(name) {
            Ok(value) => Ok(value.parse()?),
            Err(std::env::VarError::NotPresent) => Ok(default),
            Err(error) => Err(error.into()),
        }
    }

    fn report_path(name: &str) -> Result<PathBuf> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        {
            return Err("baseline names must contain only letters, digits, '-' or '_'".into());
        }
        Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/shader-bench")
            .join(format!("{name}.json")))
    }

    fn read_buffer(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        device.poll(wgpu::PollType::wait_indefinitely())?;
        receiver.recv()??;
        let bytes = buffer.get_mapped_range(..)?.to_vec();
        buffer.unmap();
        Ok(bytes)
    }

    fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shader benchmark buffer"),
            size,
            usage,
            mapped_at_creation: false,
        })
    }

    fn quantile(values: &[f64], fraction: f64) -> f64 {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let index = (sorted.len() - 1) as f64 * fraction;
        let lower = index.floor() as usize;
        sorted[lower] + (sorted[index.ceil() as usize] - sorted[lower]) * index.fract()
    }

    fn round_stats(scene: &Value) -> Result<Vec<f64>> {
        let rounds = scene["rounds"].as_array().ok_or("missing round records")?;
        if rounds.len() < 3 {
            return Err("at least three round records are required".into());
        }
        rounds.iter().map(|round| number(round, "p25_ms")).collect()
    }

    fn mad(values: &[f64]) -> f64 {
        let median = quantile(values, 0.5);
        quantile(
            &values
                .iter()
                .map(|v| (v - median).abs())
                .collect::<Vec<_>>(),
            0.5,
        )
    }

    pub(crate) fn process_spread(scene: &Value) -> Result<f64> {
        let rounds = scene["rounds"].as_array().ok_or("missing round records")?;
        let mut medians = Vec::new();
        let mut group = Vec::new();
        for round in rounds {
            // Round numbering restarts per invocation, even when a PID is reused.
            let index = round["round"]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or("missing round index")?;
            if index == 1 && !group.is_empty() {
                medians.push(quantile(&group, 0.5));
                group.clear();
            }
            group.push(number(round, "p25_ms")?);
        }
        if group.is_empty() {
            return Err("missing process rounds".into());
        }
        medians.push(quantile(&group, 0.5));
        Ok((quantile(&medians, 1.) - quantile(&medians, 0.)) / 2.)
    }

    // Separate MADs prevent round-count imbalance from masking either run's noise.
    // Process-median half-range also retains minority drift that a MAD can miss.
    pub(crate) fn verdict(old: &[f64], new: &[f64], process_spread: f64) -> (&'static str, f64) {
        let before = quantile(old, 0.5);
        let after = quantile(new, 0.5);
        let spread = mad(old).max(mad(new)).max(process_spread);
        let threshold = (3. * spread).max(0.03 * before);
        let shift = after - before;
        (
            if shift.abs() <= threshold {
                "within noise"
            } else if shift < 0. {
                "faster"
            } else {
                "slower"
            },
            threshold / before * 100.,
        )
    }

    fn number(value: &Value, key: &str) -> Result<f64> {
        value[key]
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.)
            .ok_or_else(|| format!("invalid baseline field: {key}").into())
    }

    fn scene_arg(args: impl Iterator<Item = String>) -> Result<Option<String>> {
        let mut names = args.filter(|arg| !arg.starts_with('-'));
        let name = names.next();
        if names.next().is_some() || name.as_ref().is_some_and(|n| !SCENES.contains(&n.as_str())) {
            return Err(format!("choose one scene: {}", SCENES.join(", ")).into());
        }
        Ok(name)
    }

    fn timestamp_ms(begin: u64, end: u64, period: f32) -> Result<f64> {
        let ticks = end
            .checked_sub(begin)
            .ok_or("timestamp end precedes begin")?;
        let ms = ticks as f64 * f64::from(period) / 1e6;
        valid_ms(ms)?;
        Ok(ms)
    }

    fn valid_ms(ms: f64) -> Result<()> {
        if !ms.is_finite() || ms <= 0. || ms > 60_000. {
            return Err(format!("invalid or implausible frame timing: {ms} ms").into());
        }
        Ok(())
    }

    fn fingerprint(pixels: &[u8]) -> (String, usize) {
        let checksum = pixels.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
        let ink = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[..3] != [255; 3])
            .count();
        (format!("{checksum:016x}"), ink)
    }

    fn pixel_diff(old: &[u8], new: &[u8]) -> Result<(usize, u8)> {
        if old.len() != new.len() || !old.len().is_multiple_of(4) {
            return Err("RGBA size mismatch".into());
        }
        let mut count = 0;
        let mut max_delta = 0;
        for (a, b) in old.as_chunks::<4>().0.iter().zip(new.as_chunks::<4>().0) {
            count += usize::from(a != b);
            for (x, y) in a.iter().zip(b) {
                max_delta = max_delta.max(x.abs_diff(*y));
            }
        }
        Ok((count, max_delta))
    }

    fn diff_class(count: usize, delta: u8, limit: usize, channel_limit: u8) -> &'static str {
        if count == 0 {
            "identical"
        } else if count <= limit && delta <= channel_limit {
            "minor"
        } else {
            "OUTPUT CHANGED"
        }
    }

    fn check_compatible(baseline: &Value, current: &Value) -> Result<()> {
        for key in [
            "schema",
            "adapter",
            "clock",
            "target",
            "statistic",
            "lp_num_threads",
            "cpu_cores",
            "wgpu_version",
        ] {
            if baseline.get(key).is_none() || baseline[key] != current[key] {
                return Err(format!(
                    "refusing comparison: {key} differs (baseline {}, current {})",
                    baseline[key], current[key]
                )
                .into());
            }
        }
        Ok(())
    }

    fn summarize(scene: &mut Value) -> Result<()> {
        let stats = round_stats(scene)?;
        let samples: Vec<f64> = scene["rounds"]
            .as_array()
            .ok_or("missing rounds")?
            .iter()
            .flat_map(|r| r["samples_ms"].as_array().into_iter().flatten())
            .map(|v| v.as_f64().ok_or("invalid frame sample"))
            .collect::<std::result::Result<_, _>>()?;
        for sample in &samples {
            valid_ms(*sample)?;
        }
        if samples.is_empty() {
            return Err("missing frame samples".into());
        }
        scene["median_ms"] = json!(quantile(&stats, 0.5));
        scene["p95_ms"] = json!(quantile(&samples, 0.95));
        scene["frames"] = json!(samples.len());
        Ok(())
    }

    fn compare(
        baseline: &Value,
        current: &Value,
        baseline_path: &Path,
        images: &HashMap<String, Vec<u8>>,
        pixel_limit: usize,
        channel_limit: u8,
    ) -> Result<()> {
        check_compatible(baseline, current)?;
        println!("\nscene              baseline ms   current ms    delta    detectable     verdict / output");
        for new in current["scenes"].as_array().ok_or("missing scenes")? {
            let name = new["name"].as_str().ok_or("missing scene name")?;
            let Some(old) = baseline["scenes"]
                .as_array()
                .ok_or("missing baseline scenes")?
                .iter()
                .find(|s| s["name"] == name)
            else {
                println!("{name:<18} no baseline scene");
                continue;
            };
            let old_stats = round_stats(old)?;
            let new_stats = round_stats(new)?;
            let before = quantile(&old_stats, 0.5);
            let after = quantile(&new_stats, 0.5);
            let (verdict, threshold) = verdict(
                &old_stats,
                &new_stats,
                process_spread(old)?.max(process_spread(new)?),
            );
            let old_pixels = std::fs::read(
                baseline_path
                    .with_extension("")
                    .join(format!("{name}.rgba")),
            )?;
            if fingerprint(&old_pixels).0 != old["checksum"] {
                return Err(format!("baseline RGBA checksum mismatch: {name}").into());
            }
            let (count, delta) = pixel_diff(&old_pixels, &images[name])?;
            let class = diff_class(count, delta, pixel_limit, channel_limit);
            let ink_delta = new["ink_pixels"].as_i64().ok_or("invalid ink count")?
                - old["ink_pixels"]
                    .as_i64()
                    .ok_or("invalid baseline ink count")?;
            println!("{name:<18} {before:>11.3} {after:>12.3} {:+8.2}%  ±{threshold:>6.2}%  {verdict} / {class} (differing_pixels={count}, max_delta={delta}, ink_delta={ink_delta:+}, checksum={})",
                (after / before - 1.) * 100., if old["checksum"] == new["checksum"] { "identical" } else { "changed" });
            for (label, scene) in [("baseline", old), ("current", new)] {
                let rounds = scene["rounds"].as_array().ok_or("missing rounds")?;
                let configs: Vec<_> = rounds
                    .iter()
                    .map(|r| format!("{}/{}", r["frames"], r["warmup_frames"]))
                    .collect();
                println!(
                    "  {label}: rounds={}, frames/warmup per round=[{}], total_frames={}",
                    rounds.len(),
                    configs.join(","),
                    scene["frames"]
                );
            }
            if old["curves"] != new["curves"] || old["covered_pixels"] != new["covered_pixels"] {
                println!("  WORKLOAD CHANGED: curves or covered pixels differ");
            }
        }
        Ok(())
    }

    fn merge(existing: &mut Value, current: &Value, append: bool) -> Result<()> {
        check_compatible(existing, current)?;
        for new in current["scenes"].as_array().ok_or("missing scenes")? {
            let scenes = existing["scenes"]
                .as_array_mut()
                .ok_or("missing existing scenes")?;
            if let Some(old) = scenes.iter_mut().find(|s| s["name"] == new["name"]) {
                if append {
                    for key in ["checksum", "ink_pixels", "curves", "covered_pixels"] {
                        if old[key] != new[key] {
                            return Err(format!(
                                "cannot append rounds: {} {key} changed",
                                new["name"]
                            )
                            .into());
                        }
                    }
                    old["rounds"]
                        .as_array_mut()
                        .ok_or("missing existing rounds")?
                        .extend(
                            new["rounds"]
                                .as_array()
                                .ok_or("missing new rounds")?
                                .iter()
                                .cloned(),
                        );
                    summarize(old)?;
                } else {
                    *old = new.clone();
                }
            } else {
                scenes.push(new.clone());
            }
        }
        Ok(())
    }

    pub fn run() -> Result<()> {
        let frames = count_env("BENCH_FRAMES", 30)?;
        let rounds = count_env("BENCH_ROUNDS", 5)?;
        if rounds < 3 {
            return Err("BENCH_ROUNDS must be at least 3".into());
        }
        let pixel_limit = count_env("BENCH_PIXEL_TOLERANCE", 32)?;
        let channel_limit = u8::try_from(count_env("BENCH_CHANNEL_TOLERANCE", 2)?)?;
        let append = std::env::var("BENCH_APPEND").as_deref() == Ok("1");
        let warmup = count_env("BENCH_WARMUP", 10)?;
        if frames < 10 {
            return Err("BENCH_FRAMES must be at least 10 for spread estimates".into());
        }
        let filter = scene_arg(std::env::args().skip(1))?
            .or(scene_arg(std::env::var("BENCH_SCENE").ok().into_iter())?);
        let baseline_path = std::env::var("BENCH_BASELINE")
            .ok()
            .map(|name| report_path(&name))
            .transpose()?;
        let baseline: Option<Value> = baseline_path
            .as_ref()
            .map(|path| -> Result<Value> { Ok(serde_json::from_slice(&std::fs::read(path)?)?) })
            .transpose()?;
        let save = std::env::var("BENCH_SAVE")
            .ok()
            .map(|name| report_path(&name))
            .transpose()?;
        if append && save.is_none() {
            return Err("BENCH_APPEND requires BENCH_SAVE".into());
        }
        let existing: Option<Value> = save
            .as_ref()
            .filter(|p| p.exists())
            .map(|path| -> Result<Value> { Ok(serde_json::from_slice(&std::fs::read(path)?)?) })
            .transpose()?;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let info = adapter.get_info();
        let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        let clock = if features.is_empty() {
            "cpu_submit_wait"
        } else {
            "gpu_timestamp"
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: features,
                required_limits: wgpu::Limits::downlevel_defaults(),
                ..Default::default()
            }))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            color_space: wgpu::SurfaceColorSpace::Srgb,
            width: WIDTH,
            height: HEIGHT,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Shader benchmark target"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let timestamps = (!features.is_empty()).then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Frame timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let resolve = buffer(
            &device,
            16,
            wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        );
        let timing_readback = buffer(
            &device,
            16,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let mut report = json!({
            "schema": 2,
            "statistic": "median_of_round_p25",
            "lp_num_threads": std::env::var("LP_NUM_THREADS").ok(),
            "cpu_cores": std::thread::available_parallelism()?.get(),
            "wgpu_version": include_str!("../Cargo.lock").split("name = \"wgpu\"\nversion = \"").nth(1).ok_or("wgpu missing from Cargo.lock")?.split('"').next(),
            "adapter": { "name": info.name, "backend": format!("{:?}", info.backend),
                "driver": info.driver, "driver_info": info.driver_info, "vendor": info.vendor, "device": info.device },
            "clock": clock, "target": [WIDTH, HEIGHT, "Rgba8UnormSrgb"],
            "git_rev": std::process::Command::new("git").args(["rev-parse", "HEAD"])
                .current_dir(env!("CARGO_MANIFEST_DIR")).output().ok()
                .filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()),
            "frames_per_round": frames, "warmup_frames": warmup, "rounds": rounds, "scenes": []
        });
        // Fail before rendering if a baseline uses another adapter or clock.
        if let Some(old) = &baseline {
            check_compatible(old, &report)?;
        }
        if let Some(old) = existing.as_ref().filter(|_| append || filter.is_some()) {
            check_compatible(old, &report)?;
        }
        let mut images = HashMap::new();
        println!("Adapter: {} ({:?}), driver: {} {}\nClock: {clock}; {WIDTH}x{HEIGHT} sRGB; warmup={warmup}, frames/round={frames}, rounds={rounds}; statistic=median of round p25; LP_NUM_THREADS={:?}, CPU cores={}\nMinor output limits: ≤{pixel_limit} differing pixels, max channel Δ ≤{channel_limit}",
            info.name, info.backend, info.driver, info.driver_info, report["lp_num_threads"], report["cpu_cores"]);
        let selected: Vec<_> = SCENES
            .into_iter()
            .filter(|name| filter.as_ref().is_none_or(|f| f == name))
            .collect();
        for round in 0..rounds {
            println!("Round {}/{} (rotated scene order)", round + 1, rounds);
            for offset in 0..selected.len() {
                let name = selected[(offset + round) % selected.len()];
                let (text, size, lines, spacing) = match name {
                "small_text" => ("The quick brown fox jumps over the lazy dog. Shader benchmarks measure curves and pixels. 0123456789", 16, 45, 22.),
                "large_glyphs" => ("@&g", 400, 1, 0.),
                "complex_glyphs" => ("@&%gß§ @&%gß§", 96, 6, 145.),
                _ => ("Cantarell cubic outlines: @&%gß§ The quick brown fox 0123456789", 48, 12, 80.),
            };
                let bytes: &[u8] = if name == "cff_text" {
                    include_bytes!("../tests/fixtures/Cantarell-VF.otf")
                } else {
                    include_bytes!("../examples/Roboto-Regular.ttf")
                };
                let mut store = FontStore::new(&device, &config);
                let key = store
                    .load_from_bytes(&device, &queue, bytes, text)
                    .map_err(|error| error.to_string())?;
                let mut writer = TypeWriter::new();
                let paragraphs: Vec<_> = (0..lines)
                    .map(|line| {
                        writer
                            .shape_text(
                                &store,
                                key,
                                [32., 24. + line as f32 * spacing],
                                size,
                                [0., 0., 0., 1.],
                                text,
                            )
                            .unwrap()
                    })
                    .collect();
                let font = store.get(key).unwrap();
                let mut curves = 0;
                // Count rasterized glyph-quad pixel centres, including overdraw, not just ink.
                let mut covered_pixels = 0u64;
                let units = font.face.as_face_ref().units_per_em() as f32;
                for paragraph in &paragraphs {
                    let mut x = paragraph.position[0];
                    for (id, advance) in &paragraph.glyphs {
                        if let Some(glyph) = font.glyph_cache.get(id) {
                            curves += glyph.curves.len() / 8;
                            let scale = paragraph.size as f32 / units;
                            let y = paragraph.position[1]
                                + (glyph.y_offset as f32 + (glyph.descent as f32).abs()) * scale;
                            let extent = |start: f32, length: f32, limit: u32| {
                                ((start + length - 0.5).ceil().clamp(0., limit as f32)
                                    - (start - 0.5).ceil().clamp(0., limit as f32))
                                .max(0.) as u64
                            };
                            covered_pixels += extent(x, glyph.bbox.width() as f32 * scale, WIDTH)
                                * extent(y, glyph.bbox.height() as f32 * scale, HEIGHT);
                        }
                        x += advance;
                    }
                }
                let mut renderer = TextRenderer::new(&device, &config, store.atlas());
                renderer.prepare(&device, &paragraphs, &store);
                device.poll(wgpu::PollType::wait_indefinitely())?;
                let mut samples = Vec::with_capacity(frames);
                for frame in 0..warmup.checked_add(frames).ok_or("frame count overflow")? {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            timestamp_writes: timestamps.as_ref().map(|query_set| {
                                wgpu::RenderPassTimestampWrites {
                                    query_set,
                                    beginning_of_pass_write_index: Some(0),
                                    end_of_pass_write_index: Some(1),
                                }
                            }),
                            ..Default::default()
                        });
                        renderer.render(&mut pass, [WIDTH, HEIGHT]);
                    }
                    if let Some(queries) = &timestamps {
                        encoder.resolve_query_set(queries, 0..2, &resolve, 0);
                        encoder.copy_buffer_to_buffer(&resolve, 0, &timing_readback, 0, 16);
                    }
                    let commands = encoder.finish();
                    let start = Instant::now();
                    queue.submit(Some(commands));
                    device.poll(wgpu::PollType::wait_indefinitely())?;
                    let elapsed = start.elapsed().as_secs_f64() * 1000.;
                    let ms = if timestamps.is_some() {
                        let data = read_buffer(&device, &timing_readback)?;
                        let begin = u64::from_ne_bytes(data[..8].try_into()?);
                        let end = u64::from_ne_bytes(data[8..16].try_into()?);
                        timestamp_ms(begin, end, queue.get_timestamp_period())?
                    } else {
                        elapsed
                    };
                    valid_ms(ms)?;
                    if frame >= warmup {
                        samples.push(ms);
                    }
                }
                let stride = (WIDTH * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                    * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
                let pixels = buffer(
                    &device,
                    u64::from(stride) * u64::from(HEIGHT),
                    wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                );
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &pixels,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(stride),
                            rows_per_image: Some(HEIGHT),
                        },
                    },
                    texture.size(),
                );
                queue.submit(Some(encoder.finish()));
                let data = read_buffer(&device, &pixels)?;
                let rgba: Vec<u8> = data
                    .chunks_exact(stride as usize)
                    .flat_map(|row| row[..WIDTH as usize * 4].iter().copied())
                    .collect();
                let (checksum, ink_pixels) = fingerprint(&rgba);
                if ink_pixels == 0 || curves == 0 {
                    return Err(format!("empty scene: {name}").into());
                }
                let stat = quantile(&samples, 0.25);
                println!("{name:<18} round_p25={stat:.3} ms frame_median={:.3} ms p95={:.3} ms frames={frames} curves={curves} covered_pixels={covered_pixels} ink_pixels={ink_pixels} checksum={checksum}", quantile(&samples, 0.5), quantile(&samples, 0.95));
                let git_rev = report["git_rev"].clone();
                let scenes = report["scenes"].as_array_mut().unwrap();
                if !scenes.iter().any(|s| s["name"] == name) {
                    scenes.push(json!({"name": name, "rounds": [], "curves": curves,
                    "covered_pixels": covered_pixels, "ink_pixels": ink_pixels, "checksum": checksum}));
                }
                let scene = scenes.iter_mut().find(|s| s["name"] == name).unwrap();
                if scene["checksum"] != checksum {
                    return Err(format!("output changed between rounds: {name}").into());
                }
                scene["rounds"].as_array_mut().unwrap().push(json!({
                "p25_ms": stat, "samples_ms": samples, "frames": frames, "warmup_frames": warmup,
                "process_id": std::process::id(), "round": round + 1, "git_rev": git_rev
            }));
                images.insert(name.to_owned(), rgba);
            }
        }
        for scene in report["scenes"].as_array_mut().unwrap() {
            summarize(scene)?;
            println!(
                "{}: median_round_p25={:.3} ms frame_p95={:.3} ms total_frames={}",
                scene["name"],
                number(scene, "median_ms")?,
                number(scene, "p95_ms")?,
                scene["frames"]
            );
        }
        if let (Some(old), Some(path)) = (&baseline, &baseline_path) {
            compare(old, &report, path, &images, pixel_limit, channel_limit)?;
        }
        if let Some(path) = save {
            if let Some(mut old) = existing.filter(|_| append || filter.is_some()) {
                merge(&mut old, &report, append)?;
                report = old;
                println!(
                    "{} measured scenes into existing baseline; preserved unselected scenes",
                    if append {
                        "Appended rounds for"
                    } else {
                        "Merged"
                    }
                );
            }
            let image_dir = path.with_extension("");
            std::fs::create_dir_all(&image_dir)?;
            for (name, rgba) in images {
                std::fs::write(image_dir.join(format!("{name}.rgba")), rgba)?;
            }
            std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
            println!("Saved {}", path.display());
        }
        Ok(())
    }
    #[cfg(test)]
    mod tests {
        #[test]
        fn tight_rounds_resolve_fifteen_percent_but_drifting_rounds_do_not() {
            let tight = [99., 100., 100., 100., 101.];
            let shifted = tight.map(|v| v * 1.15);
            assert_eq!(super::verdict(&tight, &shifted, 0.), ("slower", 3.));
            let drift = [85., 92.5, 100., 107.5, 115.];
            assert_eq!(
                super::verdict(&drift, &drift.map(|v| v * 1.15), 0.).0,
                "within noise"
            );
            assert_eq!(super::quantile(&[1., 2., 3., 4.], 0.5), 2.5);
        }

        #[test]
        fn diff_counts_pixels_and_classifies_magnitude() {
            let old = [128; 12];
            let mut new = old;
            assert_eq!(super::pixel_diff(&old, &new).unwrap(), (0, 0));
            assert_eq!(super::diff_class(0, 0, 0, 0), "identical");
            new[0] ^= 1;
            new[1] ^= 1;
            assert_eq!(super::pixel_diff(&old, &new).unwrap(), (1, 1));
            assert_eq!(super::diff_class(1, 1, 32, 2), "minor");
            assert_eq!(super::diff_class(33, 1, 32, 2), "OUTPUT CHANGED");
            assert_eq!(super::diff_class(1, 3, 32, 2), "OUTPUT CHANGED");
            assert!(super::pixel_diff(&old, &new[..8]).is_err());
            assert_ne!(super::fingerprint(&old).0, super::fingerprint(&new).0);
        }

        #[test]
        fn arguments_ignore_flags_and_validate_scene() {
            let parse = |args: &[&str]| super::scene_arg(args.iter().map(|s| s.to_string()));
            assert_eq!(parse(&["--bench", "--nocapture"]).unwrap(), None);
            assert_eq!(
                parse(&["--nocapture", "large_glyphs", "-q"])
                    .unwrap()
                    .as_deref(),
                Some("large_glyphs")
            );
            assert!(parse(&["missing"]).is_err());
            assert!(parse(&["large_glyphs", "small_text"]).is_err());
        }

        #[test]
        fn saved_round_statistics_survive_json_round_trip_exactly() {
            let stat = 145.07993199999999;
            let text = serde_json::to_string(&serde_json::json!({"p25_ms": stat})).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed["p25_ms"].as_f64(), Some(stat));
        }

        #[test]
        fn filtered_merge_preserves_other_scenes_and_append_pools_rounds() {
            let scene = |name: &str| {
                serde_json::json!({"name": name, "checksum": "same",
                "ink_pixels": 1, "curves": 1, "covered_pixels": 1,
                "rounds": (0..5).map(|_| serde_json::json!({"p25_ms": 100., "samples_ms": [100.]})).collect::<Vec<_>>()})
            };
            let metadata = serde_json::json!({"schema": 2, "adapter": {}, "clock": "gpu_timestamp",
                "target": [], "statistic": "median_of_round_p25", "lp_num_threads": "2",
                "cpu_cores": 2, "wgpu_version": "30.0.1", "scenes": []});
            let mut full = metadata.clone();
            full["scenes"] = serde_json::json!([scene("small_text"), scene("large_glyphs")]);
            let mut filtered = metadata;
            filtered["scenes"] = serde_json::json!([scene("large_glyphs")]);
            super::merge(&mut full, &filtered, false).unwrap();
            assert_eq!(full["scenes"].as_array().unwrap().len(), 2);
            assert_eq!(full["scenes"][0]["name"], "small_text");
            super::merge(&mut full, &filtered, true).unwrap();
            assert_eq!(full["scenes"][1]["rounds"].as_array().unwrap().len(), 10);
            for key in ["clock", "lp_num_threads", "cpu_cores"] {
                let mut incompatible = filtered.clone();
                incompatible[key] = serde_json::json!("different");
                assert!(super::check_compatible(&full, &incompatible)
                    .unwrap_err()
                    .to_string()
                    .contains(key));
            }
            filtered["scenes"][0]["checksum"] = serde_json::json!("changed");
            assert!(super::merge(&mut full, &filtered, true).is_err());
        }

        #[test]
        fn timestamps_reject_underflow_and_implausible_duration() {
            assert!(super::timestamp_ms(20, 10, 1.).is_err());
            assert!(super::timestamp_ms(0, 60_000_000_001, 1.).is_err());
            assert_eq!(super::timestamp_ms(10, 1_000_010, 1.).unwrap(), 1.);
        }
    }
}

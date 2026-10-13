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

    // Several milliseconds of work reduce idle clock changes on fast GPUs.
    const TARGET_PASS_MS: f64 = 4.;
    const MAX_REPEAT: usize = 1024;
    const STATE_CENTRE_RATIO: f64 = 1.4;
    const STATE_MIN_SHARE: f64 = 0.10;
    const STATE_MIN_FRAMES: usize = 10;
    const STATE_MIN_SLOW_ROUNDS: usize = 2;
    // Smooth relative timing noise before looking for two density peaks.
    const STATE_KDE_BANDWIDTH: f64 = 0.12;
    const STATE_KDE_GRID_POINTS: usize = 192;
    const STATE_KDE_PADDING_BANDWIDTHS: f64 = 3.;
    const STATE_PEAK_MIN_RATIO: f64 = 0.2;
    const STATE_VALLEY_MAX_RATIO: f64 = 0.5;
    const UNRESOLVED_MARKER: &str = " [states=unresolved; weaker verdict, not reliable]";
    const CLOCK_STATE_MARKER: &str = " [possible whole-run clock-state difference; repeat run]";
    const UNRESOLVED_NOTE: &str =
        "Note: the samples look bimodal but cannot be resolved; the verdict is not reliable.";
    const CLOCK_STATE_NOTE: &str = "Note: a difference this large may be the machine running in a different clock state; repeat the run.";
    // At least half the centre separation must be empty to reject continuous tails.
    const STATE_GAP_FRACTION: f64 = 0.5;

    fn calibrated_repeat(draw_ms: f64, target_ms: f64, cap: usize) -> usize {
        (target_ms / draw_ms).ceil().max(1.).min(cap as f64) as usize
    }

    fn scene_repeat(scene: &Value) -> Result<usize> {
        match scene.get("repeat") {
            None => Ok(1),
            Some(value) => value
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .filter(|v| *v > 0 && *v <= MAX_REPEAT)
                .ok_or_else(|| format!("invalid scene repeat (expected 1..={MAX_REPEAT})").into()),
        }
    }

    fn selected_repeat(
        scene: Option<(&Value, &str)>,
        forced: Option<usize>,
    ) -> Result<Option<usize>> {
        if forced.is_some_and(|v| v == 0 || v > MAX_REPEAT) {
            return Err(format!("BENCH_REPEAT must be in 1..={MAX_REPEAT}").into());
        }
        let saved = scene.map(|(scene, _)| scene_repeat(scene)).transpose()?;
        if let (Some((_, source)), Some(saved), Some(forced)) = (scene, saved, forced) {
            if saved != forced {
                return Err(format!(
                    "BENCH_REPEAT={forced} differs from {} repeat={saved}",
                    source
                )
                .into());
            }
        }
        Ok(saved.or(forced))
    }

    struct States {
        fast: Vec<f64>,
        slow: Vec<f64>,
        unresolved: bool,
    }

    impl States {
        fn share(&self) -> f64 {
            self.fast.len() as f64 / (self.fast.len() + self.slow.len()) as f64
        }

        fn description(&self) -> String {
            if self.unresolved {
                "states=unresolved".to_owned()
            } else if self.slow.is_empty() {
                "states=1".to_owned()
            } else {
                format!(
                    "states=2 fast_median={:.3} ms slow_median={:.3} ms fast_share={:.1}%",
                    quantile(&self.fast, 0.5),
                    quantile(&self.slow, 0.5),
                    self.share() * 100.
                )
            }
        }
    }

    fn split_points(sorted: &[(f64, usize)], min_share: f64, require_gap: bool) -> Vec<usize> {
        let minimum = (sorted.len() as f64 * min_share)
            .ceil()
            .max(STATE_MIN_FRAMES as f64) as usize;
        let values: Vec<_> = sorted.iter().map(|s| s.0).collect();
        (minimum..=sorted.len().saturating_sub(minimum))
            .filter(|&i| i > 0 && i < sorted.len())
            .filter(|&i| {
                let fast = sorted_quantile(&values[..i], 0.5);
                let slow = sorted_quantile(&values[i..], 0.5);
                let rounds: std::collections::HashSet<_> =
                    sorted[i..].iter().map(|s| s.1).collect();
                slow / fast >= STATE_CENTRE_RATIO
                    && rounds.len() >= STATE_MIN_SLOW_ROUNDS
                    && (!require_gap
                        || sorted[i].0 - sorted[i - 1].0 >= STATE_GAP_FRACTION * (slow - fast))
            })
            .collect()
    }

    fn has_density_valley(sorted: &[(f64, usize)], min_share: f64) -> bool {
        let eligible = split_points(sorted, min_share, false);
        if eligible.is_empty() {
            return false;
        }
        let logs: Vec<_> = sorted.iter().map(|s| s.0.ln()).collect();
        let start = logs[0] - STATE_KDE_PADDING_BANDWIDTHS * STATE_KDE_BANDWIDTH;
        let end = logs[logs.len() - 1] + STATE_KDE_PADDING_BANDWIDTHS * STATE_KDE_BANDWIDTH;
        let step = (end - start) / (STATE_KDE_GRID_POINTS - 1) as f64;
        let density: Vec<f64> = (0..STATE_KDE_GRID_POINTS)
            .map(|i| {
                let x = start + i as f64 * step;
                logs.iter()
                    .map(|v| (-0.5 * ((x - v) / STATE_KDE_BANDWIDTH).powi(2)).exp())
                    .sum()
            })
            .collect();
        // Heavily overlapping modes (per-frame lognormal sigma >= 0.2) are
        // outside this rule's resolution; reporting one state is acceptable.
        (1..density.len() - 1).any(|i| {
            if density[i] >= density[i - 1] || density[i] >= density[i + 1] {
                return false;
            }
            let left = (0..i)
                .max_by(|&a, &b| density[a].total_cmp(&density[b]))
                .unwrap();
            let right = (i + 1..density.len())
                .max_by(|&a, &b| density[a].total_cmp(&density[b]))
                .unwrap();
            let split = logs.partition_point(|&v| v < start + i as f64 * step);
            (right - left) as f64 * step >= STATE_CENTRE_RATIO.ln()
                && density[left].min(density[right])
                    >= STATE_PEAK_MIN_RATIO * density[left].max(density[right])
                && density[i] <= STATE_VALLEY_MAX_RATIO * density[left].min(density[right])
                && eligible.contains(&split)
        })
    }

    fn detect_states(samples: &[(f64, usize)], min_share: f64) -> States {
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let splits = split_points(&sorted, min_share, true);
        let mut unresolved = splits.len() > 1;
        if let [i] = splits.as_slice() {
            unresolved |= !split_points(&sorted[..*i], min_share, true).is_empty()
                || !split_points(&sorted[*i..], min_share, true).is_empty();
            if !unresolved {
                return States {
                    fast: sorted[..*i].iter().map(|s| s.0).collect(),
                    slow: sorted[*i..].iter().map(|s| s.0).collect(),
                    unresolved: false,
                };
            }
        }
        let values: Vec<_> = sorted.iter().map(|s| s.0).collect();
        if splits.is_empty() {
            unresolved = has_density_valley(&sorted, min_share);
        }
        States {
            fast: values,
            slow: Vec::new(),
            unresolved,
        }
    }

    fn scene_states(scene: &Value) -> Result<States> {
        let samples = scene_samples(scene)?;
        let mut rounds = Vec::with_capacity(samples.len());
        for (index, round) in scene["rounds"]
            .as_array()
            .ok_or("missing rounds")?
            .iter()
            .enumerate()
        {
            rounds.extend(std::iter::repeat_n(
                index,
                round["samples_ms"].as_array().map_or(0, Vec::len),
            ));
        }
        Ok(detect_states(
            &samples.into_iter().zip(rounds).collect::<Vec<_>>(),
            STATE_MIN_SHARE,
        ))
    }

    fn check_append_repeat(existing: Option<&Value>, repeat: usize, source: &str) -> Result<()> {
        if let Some(scene) = existing {
            let saved = scene_repeat(scene)?;
            if saved != repeat {
                return Err(format!(
                    "cannot append rounds: repeat changed (existing save file repeat={saved}, {source} repeat={repeat})"
                )
                .into());
            }
        }
        Ok(())
    }

    fn scene_samples(scene: &Value) -> Result<Vec<f64>> {
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
        Ok(samples)
    }

    struct Comparison {
        before: f64,
        after: f64,
        verdict: &'static str,
        threshold: f64,
        marker: String,
        unresolved: bool,
        clock_caution: bool,
    }

    fn timing_comparison(old: &Value, new: &Value) -> Result<Comparison> {
        let old_states = scene_states(old)?;
        let new_states = scene_states(new)?;
        let two_states = !old_states.slow.is_empty() || !new_states.slow.is_empty();
        let unresolved = old_states.unresolved || new_states.unresolved;
        let changed_state = two_states && !unresolved;
        let old_stats = round_stats(old)?;
        let new_stats = round_stats(new)?;
        let mut before = quantile(
            if changed_state {
                &old_states.fast
            } else {
                &old_stats
            },
            0.5,
        );
        let mut after = quantile(
            if changed_state {
                &new_states.fast
            } else {
                &new_stats
            },
            0.5,
        );
        let mut matched_state = "fast-state";
        let mut unmatched = false;
        let old_two = !old_states.slow.is_empty();
        let new_two = !new_states.slow.is_empty();
        if changed_state && old_two != new_two {
            let (single, states) = if old_two {
                (after, &old_states)
            } else {
                (before, &new_states)
            };
            let fast = quantile(&states.fast, 0.5);
            let slow = quantile(&states.slow, 0.5);
            let single_log = single.ln();
            let matched = if (single_log - slow.ln()).abs() < (single_log - fast.ln()).abs() {
                matched_state = "slow-state";
                slow
            } else {
                fast
            };
            unmatched = (single / matched).max(matched / single) >= STATE_CENTRE_RATIO;
            if old_two {
                before = matched;
            } else {
                after = matched;
            }
        }
        // Retain the existing round/process noise estimates even for state comparisons.
        let spread = mad(&old_stats)
            .max(mad(&new_stats))
            .max(process_spread(old)?.max(process_spread(new)?));
        let (verdict, threshold) = if changed_state {
            shift_verdict(before, after, spread)
        } else {
            verdict(
                &old_stats,
                &new_stats,
                process_spread(old)?.max(process_spread(new)?),
            )
        };
        let factor = after.max(before) / after.min(before);
        let clock_caution =
            !unresolved && (unmatched || (!two_states && factor >= STATE_CENTRE_RATIO));
        Ok(Comparison {
            before,
            after,
            verdict,
            threshold,
            unresolved,
            clock_caution,
            marker: if unresolved {
                UNRESOLVED_MARKER.to_owned()
            } else if changed_state && old_two && new_two {
                format!(
                    " [weaker: fast-state medians; fast_share baseline={:.1}%, current={:.1}%]",
                    old_states.share() * 100.,
                    new_states.share() * 100.
                )
            } else if changed_state {
                let describe = |states: &States| {
                    if states.slow.is_empty() {
                        "states=1".to_owned()
                    } else {
                        format!(
                            "states=2 ({:.1}% at {:.3} ms, {:.1}% at {:.3} ms)",
                            states.share() * 100.,
                            quantile(&states.fast, 0.5),
                            (1. - states.share()) * 100.,
                            quantile(&states.slow, 0.5)
                        )
                    }
                };
                format!(
                    " [weaker: {matched_state} medians; baseline {}; current {}; repeat run]",
                    describe(&old_states),
                    describe(&new_states)
                )
            } else {
                String::new()
            } + if clock_caution {
                CLOCK_STATE_MARKER
            } else {
                ""
            },
        })
    }

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
        sorted_quantile(&sorted, fraction)
    }

    fn sorted_quantile(sorted: &[f64], fraction: f64) -> f64 {
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
        shift_verdict(before, after, spread)
    }

    fn shift_verdict(before: f64, after: f64, spread: f64) -> (&'static str, f64) {
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
        let legacy_timestamp_source = if baseline["clock"] == "gpu_timestamp" {
            json!("pass")
        } else {
            Value::Null
        };
        for key in [
            "schema",
            "adapter",
            "clock",
            "timestamp_source",
            "target",
            "statistic",
            "lp_num_threads",
            "cpu_cores",
            "wgpu_version",
        ] {
            let old = baseline
                .get(key)
                .or_else(|| (key == "timestamp_source").then_some(&legacy_timestamp_source));
            if old.is_none_or(|value| *value != current[key]) {
                return Err(format!(
                    "refusing comparison: {key} differs (baseline {}, current {})",
                    old.unwrap_or(&Value::Null),
                    current[key]
                )
                .into());
            }
        }
        Ok(())
    }

    fn summarize(scene: &mut Value) -> Result<()> {
        let stats = round_stats(scene)?;
        let samples = scene_samples(scene)?;
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
        println!("\nscene              R (old/new) baseline ms   current ms    delta    detectable     verdict / output");
        let mut state_changes = Vec::new();
        let mut unresolved_rows = false;
        let mut clock_cautions = false;
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
            let Comparison {
                before,
                after,
                verdict,
                threshold,
                marker,
                unresolved,
                clock_caution,
            } = timing_comparison(old, new)?;
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
            println!("{name:<18} {}/{} {before:>11.3} {after:>12.3} {:+8.2}%  ±{threshold:>6.2}%  {verdict} / {class} (differing_pixels={count}, max_delta={delta}, ink_delta={ink_delta:+}, checksum={}){marker}",
                scene_repeat(old)?, scene_repeat(new)?, (after / before - 1.) * 100., if old["checksum"] == new["checksum"] { "identical" } else { "changed" });
            unresolved_rows |= unresolved;
            clock_cautions |= clock_caution;
            if !unresolved && marker.contains("weaker") {
                state_changes.push(name);
            }
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
        if !state_changes.is_empty() {
            println!("Note: the samples show two machine states on at least one side; verdicts for {} are weaker (matched state medians compared).", state_changes.join(", "));
        }
        if unresolved_rows {
            println!("{UNRESOLVED_NOTE}");
        }
        if clock_cautions {
            println!("{CLOCK_STATE_NOTE}");
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
                    check_append_repeat(Some(old), scene_repeat(new)?, "current run")?;
                    old["repeat"] = json!(scene_repeat(new)?);
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
        let forced_repeat = std::env::var("BENCH_REPEAT")
            .ok()
            .map(|v| v.parse::<usize>())
            .transpose()?;
        selected_repeat(None, forced_repeat)?;
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
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let info = adapter.get_info();
        let mut features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        if !features.is_empty() {
            features |= adapter.features() & wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        }
        let encoder_timestamps = features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        let timestamp_source = if features.is_empty() {
            None
        } else if encoder_timestamps {
            Some("encoder")
        } else {
            Some("pass")
        };
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
            "wgpu_version": include_str!("../Cargo.lock").lines()
                .skip_while(|line| *line != "name = \"wgpu\"").nth(1)
                .and_then(|line| line.strip_prefix("version = \""))
                .and_then(|version| version.split_once('"'))
                .map(|(version, _)| version)
                .ok_or("wgpu missing from Cargo.lock")?,
            "adapter": { "name": info.name, "backend": format!("{:?}", info.backend),
                "driver": info.driver, "driver_info": info.driver_info, "vendor": info.vendor, "device": info.device },
            "clock": clock, "timestamp_source": timestamp_source,
            "target": [WIDTH, HEIGHT, "Rgba8UnormSrgb"],
            "git_rev": std::process::Command::new("git").args(["rev-parse", "HEAD"])
                .current_dir(env!("CARGO_MANIFEST_DIR")).output().ok()
                .filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()),
            "frames_per_round": frames, "warmup_frames": warmup, "rounds": rounds, "scenes": []
        });
        // Fail before rendering if a baseline uses another adapter, clock or timestamp source.
        if let Some(old) = &baseline {
            check_compatible(old, &report)?;
        }
        if let Some(old) = existing.as_ref().filter(|_| append || filter.is_some()) {
            check_compatible(old, &report)?;
        }
        let mut images = HashMap::new();
        println!("Adapter: {} ({:?}), driver: {} {}\nClock: {clock}; timestamp_source={}; {WIDTH}x{HEIGHT} sRGB; warmup={warmup}, frames/round={frames}, rounds={rounds}; statistic=median of round p25; LP_NUM_THREADS={:?}, CPU cores={}\nMinor output limits: ≤{pixel_limit} differing pixels, max channel Δ ≤{channel_limit}",
            info.name, info.backend, info.driver, info.driver_info, timestamp_source.unwrap_or("none"), report["lp_num_threads"], report["cpu_cores"]);
        let selected: Vec<_> = SCENES
            .into_iter()
            .filter(|name| filter.as_ref().is_none_or(|f| f == name))
            .collect();
        let mut repeats = HashMap::new();
        for name in &selected {
            let find = |report: &Value| {
                report["scenes"]
                    .as_array()
                    .and_then(|scenes| scenes.iter().find(|scene| scene["name"] == *name))
                    .cloned()
            };
            let baseline_scene = baseline.as_ref().and_then(find);
            let appended = existing.as_ref().filter(|_| append).and_then(find);
            let saved = baseline_scene
                .as_ref()
                .map(|scene| (scene, "baseline"))
                .or_else(|| appended.as_ref().map(|scene| (scene, "existing save file")));
            if let Some(repeat) = selected_repeat(saved, forced_repeat)? {
                check_append_repeat(
                    appended.as_ref(),
                    repeat,
                    saved.map_or("BENCH_REPEAT", |(_, source)| source),
                )?;
                repeats.insert(*name, repeat);
            }
        }
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
                    include_bytes!("../fonts/Cantarell-VF.otf")
                } else {
                    include_bytes!("../fonts/Roboto-Regular.ttf")
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
                let mut instances = 0u32;
                // Count rasterized glyph-quad pixel centres, including overdraw, not just ink.
                let mut covered_pixels = 0u64;
                let units = font.face.as_face_ref().units_per_em() as f32;
                for paragraph in &paragraphs {
                    let mut x = paragraph.position[0];
                    for (id, advance) in &paragraph.glyphs {
                        if let Some(glyph) = font.glyph_cache.get(id) {
                            instances += 1;
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
                let calibrate = !repeats.contains_key(name);
                let mut repeat = repeats.get(name).copied().unwrap_or(1);
                let warmup_count = warmup;
                let mut calibration_retries = 0;
                let total = warmup_count
                    .checked_add(frames)
                    .ok_or("frame count overflow")?;
                let mut frame = 0;
                while frame < total {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    if let Some(queries) = timestamps.as_ref().filter(|_| encoder_timestamps) {
                        encoder.write_timestamp(queries, 0);
                    }
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
                            timestamp_writes: timestamps
                                .as_ref()
                                .filter(|_| !encoder_timestamps)
                                .map(|query_set| wgpu::RenderPassTimestampWrites {
                                    query_set,
                                    beginning_of_pass_write_index: Some(0),
                                    end_of_pass_write_index: Some(1),
                                }),
                            ..Default::default()
                        });
                        renderer.render(&mut pass, [WIDTH, HEIGHT]);
                        // render borrows the renderer for the pass lifetime. Reuse its
                        // bindings and six-index glyph quads for the remaining draws.
                        for _ in 1..repeat {
                            pass.draw_indexed(0..6, 0, 0..instances);
                        }
                    }
                    if let Some(queries) = &timestamps {
                        if encoder_timestamps {
                            encoder.write_timestamp(queries, 1);
                        }
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
                        let period = queue.get_timestamp_period();
                        timestamp_ms(begin, end, period).map_err(|error| {
                            let hint = if begin == 0 && end == 0 {
                                "; driver returned no timestamps; try WGPU_BACKEND=dx12"
                            } else {
                                ""
                            };
                            format!(
                                "{error}; scene={name}, frame={frame} (zero-based), warmup={}, begin={begin}, end={end}, timestamp_period={period} ns/tick, wall_elapsed={elapsed} ms{hint}",
                                frame < warmup_count
                            )
                        })?
                    } else {
                        elapsed
                    };
                    valid_ms(ms)?;
                    if calibrate && (frame < warmup_count || (warmup_count == 0 && frame == 0)) {
                        let next =
                            calibrated_repeat(ms / repeat as f64, TARGET_PASS_MS, MAX_REPEAT)
                                .max(repeat);
                        if next > repeat {
                            repeat = next;
                            calibration_retries += 1;
                            continue;
                        }
                    }
                    if frame >= warmup_count {
                        samples.push(ms / repeat as f64);
                    }
                    frame += 1;
                }
                repeats.insert(name, repeat);
                let stride = (WIDTH * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                    * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
                let pixels = buffer(
                    &device,
                    u64::from(stride) * u64::from(HEIGHT),
                    wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                );
                let mut encoder = device.create_command_encoder(&Default::default());
                if repeat > 1 {
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
                        ..Default::default()
                    });
                    renderer.render(&mut pass, [WIDTH, HEIGHT]);
                }
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
                    scenes.push(json!({"name": name, "repeat": repeat, "rounds": [], "curves": curves,
                    "covered_pixels": covered_pixels, "ink_pixels": ink_pixels, "checksum": checksum}));
                }
                let scene = scenes.iter_mut().find(|s| s["name"] == name).unwrap();
                if scene["checksum"] != checksum {
                    return Err(format!("output changed between rounds: {name}").into());
                }
                scene["rounds"].as_array_mut().unwrap().push(json!({
                "p25_ms": stat, "samples_ms": samples, "frames": frames, "warmup_frames": warmup, "calibration_retries": calibration_retries,
                "process_id": std::process::id(), "round": round + 1, "git_rev": git_rev
            }));
                images.insert(name.to_owned(), rgba);
            }
        }
        for scene in report["scenes"].as_array_mut().unwrap() {
            summarize(scene)?;
            println!(
                "{}: median_round_p25={:.3} ms frame_p95={:.3} ms total_frames={} R={} {}",
                scene["name"],
                number(scene, "median_ms")?,
                number(scene, "p95_ms")?,
                scene["frames"],
                scene_repeat(scene)?,
                scene_states(scene)?.description()
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
    // The harness=false bench also compiles with cfg(test), without running tests.
    #[allow(dead_code)]
    mod tests {
        use super::*;

        fn cluster(centre: f64, count: usize) -> Vec<f64> {
            (0..count)
                .map(|i| centre * (1. + (i as f64 - (count - 1) as f64 / 2.) * 0.0001))
                .collect()
        }

        fn detect_states(samples: &[f64], min_share: f64) -> States {
            super::detect_states(
                &samples
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| (v, i % 5))
                    .collect::<Vec<_>>(),
                min_share,
            )
        }

        fn mixed(fast: f64, slow: f64, fast_count: usize) -> Vec<f64> {
            cluster(fast, fast_count)
                .into_iter()
                .chain(cluster(slow, 100 - fast_count))
                .collect()
        }

        fn sample_scene(samples: &[f64]) -> Value {
            json!({"rounds": (1..=5).map(|round| json!({
                "round": round, "p25_ms": quantile(samples, 0.25), "samples_ms": samples
            })).collect::<Vec<_>>()})
        }

        struct Generator(u64);

        impl Generator {
            fn uniform(&mut self) -> f64 {
                self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
                let mut value = self.0;
                value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
                value ^= value >> 31;
                ((value >> 11) as f64 + 0.5) / (1u64 << 53) as f64
            }

            fn lognormal(&mut self, centre: f64, sigma: f64) -> f64 {
                let normal = (-2. * self.uniform().ln()).sqrt()
                    * (std::f64::consts::TAU * self.uniform()).cos();
                centre * (sigma * normal).exp()
            }
        }

        fn generated(seed: u64, rounds: usize, fast_count: usize, sigma: f64) -> Vec<(f64, usize)> {
            let mut generator = Generator(seed);
            (0..rounds)
                .flat_map(|round| (0..30).map(move |frame| (round, frame)))
                .map(|(round, frame)| {
                    (
                        generator.lognormal(if frame < fast_count { 0.071 } else { 0.144 }, sigma),
                        round,
                    )
                })
                .collect()
        }

        fn state_counts(rounds: usize, fast_count: usize, sigma: f64) -> [usize; 3] {
            let mut counts = [0; 3];
            for seed in 0..200 {
                let states = super::detect_states(
                    &generated(seed, rounds, fast_count, sigma),
                    STATE_MIN_SHARE,
                );
                counts[if states.unresolved {
                    2
                } else if states.slow.is_empty() {
                    0
                } else {
                    1
                }] += 1;
            }
            counts
        }

        #[test]
        fn seeded_single_lognormal_distributions_are_one_state() {
            for rounds in [5, 1] {
                for sigma in [0.2, 0.3, 0.4, 0.5] {
                    let counts = state_counts(rounds, 30, sigma);
                    println!("single rounds={rounds} sigma={sigma}: {counts:?}");
                    assert_eq!(counts, [200, 0, 0], "rounds={rounds}, sigma={sigma}");
                }
            }
        }

        #[test]
        fn seeded_separated_lognormal_modes_are_never_one_state() {
            for fast_count in [21, 15] {
                for sigma in [0.03, 0.05, 0.07, 0.10] {
                    let counts = state_counts(5, fast_count, sigma);
                    println!("mixed fast/round={fast_count} sigma={sigma}: {counts:?}");
                    assert_eq!(counts[0], 0, "fast/round={fast_count}, sigma={sigma}");
                    if sigma == 0.03 {
                        assert_eq!(counts, [0, 200, 0]);
                    }
                }
            }
        }

        #[test]
        fn state_detection_requires_two_populated_separated_clusters() {
            assert!(detect_states(&cluster(1., 100), STATE_MIN_SHARE)
                .slow
                .is_empty());
            assert_eq!(
                detect_states(&mixed(1., 3., 95), STATE_MIN_SHARE).description(),
                "states=1"
            );
            // A continuous skewed distribution must not become two states.
            let tail: Vec<_> = (0..100)
                .map(|i| 1. + (i as f64 / 100.).powi(4) * 2.)
                .collect();
            assert_eq!(
                detect_states(&tail, STATE_MIN_SHARE).description(),
                "states=1"
            );
            let shoulder: Vec<_> = cluster(1., 85)
                .into_iter()
                .chain([1.2, 1.35, 1.5, 1.65, 1.7])
                .chain(cluster(2., 10))
                .collect();
            assert_eq!(
                detect_states(&shoulder, STATE_MIN_SHARE).description(),
                "states=1"
            );
            for fast_count in [50, 85, 15] {
                let states = detect_states(&mixed(1., 2., fast_count), STATE_MIN_SHARE);
                assert!((quantile(&states.fast, 0.5) - 1.).abs() < 1e-12);
                assert!((quantile(&states.slow, 0.5) - 2.).abs() < 1e-12);
                assert_eq!(states.share(), fast_count as f64 / 100.);
            }
            assert_eq!(
                detect_states(&mixed(1., 2., 85), 0.16).description(),
                "states=1"
            );
            assert_eq!(
                detect_states(&mixed(1., 2., 91), STATE_MIN_SHARE).description(),
                "states=1"
            );
            let boundary = detect_states(&mixed(1., 2., 90), STATE_MIN_SHARE);
            assert_eq!(boundary.share(), 0.9);
        }

        #[test]
        fn separated_submillisecond_samples_retain_fast_state_median() {
            let states = detect_states(&mixed(0.071, 0.158, 75), STATE_MIN_SHARE);
            assert_eq!(
                states.description(),
                "states=2 fast_median=0.071 ms slow_median=0.158 ms fast_share=75.0%"
            );
            assert!((quantile(&states.fast, 0.5) - 0.071).abs() < 1e-12);
            assert_eq!(
                detect_states(&cluster(0.071, 100), STATE_MIN_SHARE).description(),
                "states=1"
            );
        }

        #[test]
        fn comparison_marks_state_changes_without_a_false_regression() {
            let noisy_scene = |current: bool| {
                json!({"rounds": (0..10).map(|i| {
                let scale = 0.98 + (i % 5) as f64 * 0.01 + (i / 5) as f64 * 0.005;
                let samples = if current { mixed(0.072 * scale, 0.144 * scale, [10, 10, 10, 10, 35][i % 5]) }
                    else { cluster(0.071 * scale, 100) };
                json!({"round": i % 5 + 1, "p25_ms": quantile(&samples, 0.25), "samples_ms": samples})
            }).collect::<Vec<_>>()})
            };
            let old = noisy_scene(false);
            let new = noisy_scene(true);
            assert!(mad(&round_stats(&old).unwrap()) > 0.);
            let mixed_stats = round_stats(&new).unwrap();
            assert!(mad(&mixed_stats) > 0.);
            assert!(mixed_stats.iter().any(|&v| v < 0.1));
            assert!(mixed_stats.iter().any(|&v| v > 0.13));
            assert!(process_spread(&old).unwrap() > 0.);
            assert!(process_spread(&new).unwrap() > 0.);
            let result = timing_comparison(&old, &new).unwrap();
            assert_eq!(
                result.marker,
                " [weaker: fast-state medians; baseline states=1; current states=2 (15.0% at 0.073 ms, 85.0% at 0.144 ms); repeat run]"
            );
            assert!(!result.unresolved);
            assert_eq!(result.verdict, "within noise");
            let spread = mad(&round_stats(&old).unwrap())
                .max(mad(&round_stats(&new).unwrap()))
                .max(
                    process_spread(&old)
                        .unwrap()
                        .max(process_spread(&new).unwrap()),
                );
            assert_eq!(
                result.threshold,
                (3. * spread).max(0.03 * result.before) / result.before * 100.
            );
            assert!(result.threshold > 3.);
            assert!((result.after / result.before - 1.).abs() < 0.03);
            let reverse = timing_comparison(&new, &old).unwrap();
            assert_eq!(
                reverse.marker,
                " [weaker: fast-state medians; baseline states=2 (15.0% at 0.073 ms, 85.0% at 0.144 ms); current states=1; repeat run]"
            );
            assert!(!reverse.unresolved);
            let one_state = sample_scene(&cluster(0.072, 100));
            let result = timing_comparison(&old, &one_state).unwrap();
            let expected = verdict(
                &round_stats(&old).unwrap(),
                &round_stats(&one_state).unwrap(),
                process_spread(&old)
                    .unwrap()
                    .max(process_spread(&one_state).unwrap()),
            );
            assert!(result.marker.is_empty());
            assert_eq!((result.verdict, result.threshold), expected);
        }

        fn exact_modes(fast: f64, slow: f64, fast_count: usize) -> Value {
            sample_scene(
                &std::iter::repeat_n(fast, fast_count)
                    .chain(std::iter::repeat_n(slow, 100 - fast_count))
                    .collect::<Vec<_>>(),
            )
        }

        #[test]
        fn single_slow_state_matches_the_slow_state_without_a_false_speedup() {
            let old = sample_scene(&[0.144; 100]);
            let new = exact_modes(0.071, 0.144, 90);
            let result = timing_comparison(&old, &new).unwrap();
            assert_eq!((result.before, result.after), (0.144, 0.144));
            assert_eq!(result.verdict, "within noise");
            assert_eq!(result.marker, " [weaker: slow-state medians; baseline states=1; current states=2 (90.0% at 0.071 ms, 10.0% at 0.144 ms); repeat run]");
            assert!(!result.clock_caution);
        }

        #[test]
        fn single_slow_state_matches_the_slow_state_without_a_false_regression() {
            let old = exact_modes(0.071, 0.144, 15);
            let new = sample_scene(&[0.144; 100]);
            let result = timing_comparison(&old, &new).unwrap();
            assert_eq!((result.before, result.after), (0.144, 0.144));
            assert_eq!(result.verdict, "within noise");
            assert_eq!(result.marker, " [weaker: slow-state medians; baseline states=2 (15.0% at 0.071 ms, 85.0% at 0.144 ms); current states=1; repeat run]");
            assert!(!result.clock_caution);
        }

        #[test]
        fn single_state_twenty_percent_above_fast_is_compared_with_fast() {
            let old = exact_modes(0.071, 0.144, 70);
            let new = sample_scene(&[0.071 * 1.2; 100]);
            let result = timing_comparison(&old, &new).unwrap();
            assert_eq!((result.before, result.after), (0.071, 0.071 * 1.2));
            assert_eq!(result.verdict, "slower");
            assert_eq!(result.marker, " [weaker: fast-state medians; baseline states=2 (70.0% at 0.071 ms, 30.0% at 0.144 ms); current states=1; repeat run]");
            assert!(!result.clock_caution);
        }

        #[test]
        fn single_state_matching_neither_state_adds_a_clock_caution() {
            let two = exact_modes(0.071, 0.144, 70);
            let one = sample_scene(&[0.300; 100]);
            let result = timing_comparison(&two, &one).unwrap();
            assert_eq!((result.before, result.after), (0.144, 0.300));
            assert_eq!(result.verdict, "slower");
            assert_eq!(result.marker, " [weaker: slow-state medians; baseline states=2 (70.0% at 0.071 ms, 30.0% at 0.144 ms); current states=1; repeat run] [possible whole-run clock-state difference; repeat run]");
            assert!(result.clock_caution);
            let reverse = timing_comparison(&one, &two).unwrap();
            assert_eq!(reverse.marker, " [weaker: slow-state medians; baseline states=1; current states=2 (70.0% at 0.071 ms, 30.0% at 0.144 ms); repeat run] [possible whole-run clock-state difference; repeat run]");
            assert!(reverse.clock_caution);
            let below_fast = sample_scene(&[0.071 / 1.5; 100]);
            let lower = timing_comparison(&two, &below_fast).unwrap();
            assert!((lower.after / lower.before - 1.).abs() < 0.40);
            assert_eq!(lower.marker, " [weaker: fast-state medians; baseline states=2 (70.0% at 0.071 ms, 30.0% at 0.144 ms); current states=1; repeat run] [possible whole-run clock-state difference; repeat run]");
            assert!(lower.clock_caution);
        }

        #[test]
        fn both_two_state_runs_compare_fast_without_clock_caution_for_large_shifts() {
            let old = exact_modes(0.071, 0.144, 70);
            let new = exact_modes(0.071 * 1.5, 0.144 * 2., 70);
            let result = timing_comparison(&old, &new).unwrap();
            assert_eq!((result.before, result.after), (0.071, 0.071 * 1.5));
            let delta = result.after / result.before - 1.;
            assert!((delta - 0.5).abs() < 1e-12);
            assert!(result.after / result.before >= STATE_CENTRE_RATIO);
            assert_eq!(result.verdict, "slower");
            assert_eq!(
                result.marker,
                " [weaker: fast-state medians; fast_share baseline=70.0%, current=70.0%]"
            );
            assert!(!result.clock_caution);
        }

        #[test]
        fn scene_samples_reject_invalid_durations_and_small_sets_are_one_state() {
            // JSON encodes non-finite numbers as null; numeric validation also rejects them.
            for sample in [0., -1., f64::NAN, f64::INFINITY] {
                let scene = json!({"rounds": [{"samples_ms": [sample]}]});
                assert!(scene_samples(&scene).is_err());
                assert!(valid_ms(sample).is_err());
            }
            assert!(scene_samples(&json!({"rounds": [{"samples_ms": []}]})).is_err());
            for count in 0..10 {
                let samples: Vec<_> = (0..count)
                    .map(|i| if i % 2 == 0 { 1. } else { 2. })
                    .collect();
                assert_eq!(
                    detect_states(&samples, STATE_MIN_SHARE).description(),
                    "states=1"
                );
            }
        }

        #[test]
        fn scheduling_spikes_and_a_single_slow_round_are_one_state() {
            let spike: Vec<_> = [22.; 27].into_iter().chain([105.; 3]).collect();
            assert_eq!(
                detect_states(&spike, STATE_MIN_SHARE).description(),
                "states=1"
            );
            let samples: Vec<_> = (0..5)
                .flat_map(|round| {
                    (0..30).map(move |_| (if round == 2 { 105. } else { 22. }, round))
                })
                .collect();
            assert_eq!(
                super::detect_states(&samples, STATE_MIN_SHARE).description(),
                "states=1"
            );
        }

        #[test]
        fn a_unique_global_split_with_a_nested_split_is_unresolved() {
            let samples: Vec<_> = cluster(1., 33)
                .into_iter()
                .chain(cluster(2., 33))
                .chain(cluster(4., 34))
                .collect();
            let with_rounds: Vec<_> = samples
                .iter()
                .enumerate()
                .map(|(i, &value)| (value, i % 5))
                .collect();
            assert_eq!(split_points(&with_rounds, STATE_MIN_SHARE, true), vec![66]);
            assert_eq!(
                split_points(&with_rounds[..66], STATE_MIN_SHARE, true),
                vec![33]
            );
            let states = detect_states(&samples, STATE_MIN_SHARE);
            assert_eq!(states.description(), "states=unresolved");
            assert!(states.slow.is_empty());

            let another_nested: Vec<_> = cluster(1., 20)
                .into_iter()
                .chain(cluster(2., 30))
                .chain(cluster(10., 50))
                .enumerate()
                .map(|(i, value)| (value, i % 5))
                .collect();
            assert_eq!(
                split_points(&another_nested, STATE_MIN_SHARE, true),
                vec![50]
            );
            assert_eq!(
                split_points(&another_nested[..50], STATE_MIN_SHARE, true),
                vec![20]
            );
            assert_eq!(
                super::detect_states(&another_nested, STATE_MIN_SHARE).description(),
                "states=unresolved"
            );
        }

        #[test]
        fn multiple_global_splits_are_unresolved() {
            // Both gaps pass globally; this input exercises the multi-split branch.
            let samples: Vec<_> = cluster(1., 40)
                .into_iter()
                .chain(cluster(2., 40))
                .chain(cluster(10., 20))
                .collect();
            let with_rounds: Vec<_> = samples
                .iter()
                .enumerate()
                .map(|(i, &value)| (value, i % 5))
                .collect();
            assert_eq!(
                split_points(&with_rounds, STATE_MIN_SHARE, true),
                vec![40, 80]
            );
            assert_eq!(
                detect_states(&samples, STATE_MIN_SHARE).description(),
                "states=unresolved"
            );
        }

        fn noisy_bimodal() -> Vec<f64> {
            (0..50)
                .map(|i| 0.7 + i as f64 * 0.6 / 49.)
                .chain((0..50).map(|i| 1.7 + i as f64 * 0.6 / 49.))
                .collect()
        }

        #[test]
        fn noisy_bimodality_is_visible_and_uses_the_existing_statistic() {
            let samples = noisy_bimodal();
            assert_eq!(
                detect_states(&samples, STATE_MIN_SHARE).description(),
                "states=unresolved"
            );
            // Broad uniform samples have no density valley and remain one state.
            let broad: Vec<_> = (0..50)
                .map(|i| 0.4 + i as f64 * 1.1 / 49.)
                .chain((0..50).map(|i| 1.5 + i as f64 * 1.1 / 49.))
                .collect();
            assert_eq!(
                detect_states(&broad, STATE_MIN_SHARE).description(),
                "states=1"
            );
            let old = sample_scene(&cluster(1., 100));
            let new = sample_scene(&samples);
            let two = sample_scene(&mixed(1., 2., 50));
            let combined = timing_comparison(&two, &new).unwrap();
            assert!(combined.unresolved);
            assert_eq!(combined.marker, UNRESOLVED_MARKER);
            assert_eq!(combined.before, quantile(&round_stats(&two).unwrap(), 0.5));
            assert!(!combined.clock_caution);
            for (old, new) in [(&old, &new), (&new, &old)] {
                let result = timing_comparison(old, new).unwrap();
                let old_stats = round_stats(old).unwrap();
                let new_stats = round_stats(new).unwrap();
                assert_eq!(result.before, quantile(&old_stats, 0.5));
                assert_eq!(result.after, quantile(&new_stats, 0.5));
                assert_eq!(
                    (result.verdict, result.threshold),
                    verdict(
                        &old_stats,
                        &new_stats,
                        process_spread(old)
                            .unwrap()
                            .max(process_spread(new).unwrap())
                    )
                );
                assert!(result.marker.contains(UNRESOLVED_MARKER));
                assert!(result.unresolved);
            }
        }

        #[test]
        fn unresolved_comparison_has_only_one_marker_even_for_a_large_shift() {
            let samples = noisy_bimodal();
            let old = sample_scene(&samples);
            let new = sample_scene(&samples.iter().map(|v| v * 1.5).collect::<Vec<_>>());
            assert!(scene_states(&old).unwrap().unresolved);
            assert!(scene_states(&new).unwrap().unresolved);
            let result = timing_comparison(&old, &new).unwrap();
            assert!(result.after / result.before >= 1.5 - 1e-12);
            assert_eq!(result.marker, UNRESOLVED_MARKER);
            assert!(!result.clock_caution);
        }

        #[test]
        fn whole_run_clock_state_differences_keep_the_verdict_and_add_caution() {
            let old = sample_scene(&[0.071; 100]);
            let new = sample_scene(&[0.144; 100]);
            let result = timing_comparison(&old, &new).unwrap();
            assert_eq!(result.verdict, "slower");
            assert_eq!(result.marker, CLOCK_STATE_MARKER);
            assert!(result.clock_caution);
            let reverse = timing_comparison(&new, &old).unwrap();
            assert_eq!(reverse.verdict, "faster");
            assert_eq!(reverse.marker, CLOCK_STATE_MARKER);
            let small = timing_comparison(&old, &sample_scene(&[0.072; 100])).unwrap();
            assert!(!small.clock_caution);
            assert!(small.marker.is_empty());
            // Boundaries are multiplicative, including the reciprocal boundary.
            for value in [0.6, STATE_CENTRE_RATIO, 1. / STATE_CENTRE_RATIO] {
                assert!(
                    timing_comparison(&sample_scene(&[1.; 100]), &sample_scene(&[value; 100]))
                        .unwrap()
                        .clock_caution
                );
            }
        }

        #[test]
        fn one_state_clock_caution_is_symmetric_and_uses_the_centre_factor() {
            for (a, b, caution) in [(0.100, 0.071, true), (0.100, 0.072, false)] {
                let a = sample_scene(&[a; 100]);
                let b = sample_scene(&[b; 100]);
                for (old, new) in [(&a, &b), (&b, &a)] {
                    let result = timing_comparison(old, new).unwrap();
                    assert_eq!(result.clock_caution, caution);
                    assert_eq!(result.marker, if caution { CLOCK_STATE_MARKER } else { "" });
                }
            }
        }

        #[test]
        fn append_repeat_preflight_rejects_baseline_conflicts() {
            let baseline = json!({"repeat": 2});
            let existing = json!({"repeat": 1});
            let repeat = selected_repeat(Some((&baseline, "baseline")), None)
                .unwrap()
                .unwrap();
            assert!(check_append_repeat(Some(&existing), repeat, "baseline")
                .unwrap_err()
                .to_string()
                .contains("existing save file repeat=1, baseline repeat=2"));
            assert!(check_append_repeat(Some(&json!({})), repeat, "baseline").is_err());
            check_append_repeat(Some(&baseline), repeat, "baseline").unwrap();
        }

        #[test]
        fn repeat_reuses_baselines_and_rejects_forced_mismatch() {
            let scene = json!({"repeat": 80});
            assert_eq!(
                selected_repeat(Some((&scene, "baseline")), None).unwrap(),
                Some(80)
            );
            assert_eq!(
                selected_repeat(Some((&scene, "baseline")), Some(80)).unwrap(),
                Some(80)
            );
            assert_eq!(
                selected_repeat(Some((&json!({}), "baseline")), None).unwrap(),
                Some(1)
            );
            assert_eq!(selected_repeat(None, None).unwrap(), None);
            assert_eq!(selected_repeat(None, Some(4)).unwrap(), Some(4));
            let error = selected_repeat(Some((&scene, "baseline")), Some(1))
                .unwrap_err()
                .to_string();
            assert!(error.contains("BENCH_REPEAT=1 differs from baseline repeat=80"));
            assert!(selected_repeat(Some((&json!({}), "baseline")), Some(2)).is_err());
            for invalid in [
                json!({"repeat": 0}),
                json!({"repeat": -1}),
                json!({"repeat": 1.5}),
            ] {
                assert!(scene_repeat(&invalid).is_err());
            }
            let error = selected_repeat(Some((&scene, "existing save file")), Some(1))
                .unwrap_err()
                .to_string();
            assert_eq!(
                error,
                "BENCH_REPEAT=1 differs from existing save file repeat=80"
            );
            assert!(selected_repeat(None, Some(0)).is_err());
            assert!(selected_repeat(None, Some(MAX_REPEAT + 1)).is_err());
        }

        #[test]
        fn calibration_targets_pass_duration_with_a_repeat_cap() {
            assert_eq!(calibrated_repeat(20., TARGET_PASS_MS, MAX_REPEAT), 1);
            assert_eq!(calibrated_repeat(0.05, TARGET_PASS_MS, MAX_REPEAT), 80);
            assert_eq!(calibrated_repeat(0.05, TARGET_PASS_MS, 64), 64);
            assert_eq!(
                calibrated_repeat(0.0001, TARGET_PASS_MS, MAX_REPEAT),
                MAX_REPEAT
            );
        }

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
                "timestamp_source": "encoder",
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
            for key in ["clock", "timestamp_source", "lp_num_threads", "cpu_cores"] {
                let mut incompatible = filtered.clone();
                incompatible[key] = serde_json::json!("different");
                assert!(super::check_compatible(&full, &incompatible)
                    .unwrap_err()
                    .to_string()
                    .contains(key));
            }
            let mut pass_timestamps = filtered.clone();
            pass_timestamps["timestamp_source"] = serde_json::json!("pass");
            assert!(super::check_compatible(&full, &pass_timestamps).is_err());
            pass_timestamps
                .as_object_mut()
                .unwrap()
                .remove("timestamp_source");
            assert!(super::check_compatible(&pass_timestamps, &full).is_err());
            let mut different_repeat = filtered.clone();
            different_repeat["scenes"][0]["repeat"] = json!(2);
            assert!(super::merge(&mut full, &different_repeat, true)
                .unwrap_err()
                .to_string()
                .contains("repeat changed"));
            filtered["scenes"][0]["checksum"] = serde_json::json!("changed");
            assert!(super::merge(&mut full, &filtered, true).is_err());
        }

        #[test]
        fn legacy_timestamp_sources_match_pass_and_cpu_but_not_encoder() {
            let mut baseline = serde_json::json!({"schema": 2, "adapter": {},
                "clock": "gpu_timestamp", "target": [], "statistic": "median_of_round_p25",
                "lp_num_threads": null, "cpu_cores": 2, "wgpu_version": "30.0.1"});
            let mut current = baseline.clone();
            current["timestamp_source"] = serde_json::json!("encoder");
            assert!(super::check_compatible(&baseline, &current)
                .unwrap_err()
                .to_string()
                .contains("timestamp_source"));
            current["timestamp_source"] = serde_json::json!("pass");
            super::check_compatible(&baseline, &current).unwrap();
            baseline["clock"] = serde_json::json!("cpu_submit_wait");
            current["clock"] = serde_json::json!("cpu_submit_wait");
            current["timestamp_source"] = serde_json::Value::Null;
            super::check_compatible(&baseline, &current).unwrap();
        }

        #[test]
        fn timestamps_reject_underflow_and_implausible_duration() {
            assert!(super::timestamp_ms(20, 10, 1.).is_err());
            assert!(super::timestamp_ms(0, 60_000_000_001, 1.).is_err());
            assert_eq!(super::timestamp_ms(10, 1_000_010, 1.).unwrap(), 1.);
        }
    }
}

#[path = "../adapter/mod.rs"]
mod adapter;
#[path = "memory.rs"]
mod memory;
#[path = "quality.rs"]
mod quality;
#[path = "raster.rs"]
mod raster;
#[path = "zoom.rs"]
mod zoom;

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, Viewport, Wrap,
};
use serde_json::{json, Value};
use std::{error::Error, path::PathBuf, time::Instant};
use wgpu_font_renderer::{
    FontId, FontStore, LineHeight, TextBlock, TextLayout, TextRenderer, TypeWriter,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const ROBOTO: &[u8] = include_bytes!("../../fonts/Roboto-Regular.ttf");
const NOTO: &[u8] = include_bytes!("../../fonts/NotoSerifSC-Subset.otf");
const CJK: &str = "龍鬱齉直接从轮廓渲染字形文本";
const NAMES: [&str; 4] = ["body_16px", "ui_mixed", "large_200px", "cjk_24px"];
const LATIN: &str = "The morning light falls across the quiet library. Readers turn pages and share ideas about art, science, and the world outside. Clear text makes a long paragraph comfortable to read. A good benchmark uses the same words, fonts, positions, and colours for every renderer. Numbers 0123456789 and ordinary punctuation belong here too.";

#[derive(Clone)]
struct Item {
    text: String,
    size: f32,
    position: [f32; 2],
    width: f32,
    ascent: f32,
    line_height: f32,
}
struct Scene {
    name: &'static str,
    bytes: &'static [u8],
    family: String,
    chars: String,
    items: Vec<Item>,
}
impl Scene {
    fn new(name: &'static str, gpu: &Gpu) -> Result<Self> {
        let mut items = Vec::new();
        let mut add = |text: String, size, position, width| {
            items.push(Item {
                text,
                size,
                position,
                width,
                ascent: 0.,
                line_height: 0.,
            })
        };
        match name {
            "body_16px" => {
                // Three columns of nine wrapped paragraphs fill the screen.
                let text = std::iter::repeat_n(LATIN, 9)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                for column in 0..3 {
                    add(text.clone(), 16., [32. + column as f32 * 624., 24.], 584.);
                }
            }
            "ui_mixed" => {
                for (row, size) in [12., 14., 18., 24., 32., 48.].into_iter().enumerate() {
                    for column in 0..4 {
                        add(
                            format!("Settings {row}{column}: ready"),
                            size,
                            [32. + column as f32 * 472., 32. + row as f32 * 160.],
                            440.,
                        );
                    }
                }
            }
            "large_200px" => {
                for (row, text) in ["Read clearly", "Shape and draw", "Hello world"]
                    .into_iter()
                    .enumerate()
                {
                    add(text.into(), 200., [32., 24. + row as f32 * 320.], 1856.);
                }
            }
            "cjk_24px" => {
                let text = std::iter::repeat_n(CJK.repeat(4), 22)
                    .collect::<Vec<_>>()
                    .join("\n");
                add(text, 24., [32., 24.], 1856.);
            }
            _ => unreachable!(),
        }
        let bytes = if name == "cjk_24px" { NOTO } else { ROBOTO };
        Self::from_items(name, bytes, items, gpu)
    }
    fn from_items(
        name: &'static str,
        bytes: &'static [u8],
        mut items: Vec<Item>,
        gpu: &Gpu,
    ) -> Result<Self> {
        let mut db = glyphon::fontdb::Database::new();
        db.load_font_data(bytes.to_vec());
        let family = db.faces().next().ok_or("missing font face")?.families[0]
            .0
            .clone();
        // Validate every drawable character, especially the intentionally tiny CJK subset.
        let face = skrifa::FontRef::new(bytes)?;
        use skrifa::MetadataProvider;
        for ch in items
            .iter()
            .flat_map(|item| item.text.chars())
            .filter(|ch| !ch.is_whitespace())
        {
            if face.charmap().map(ch).is_none_or(|id| id.to_u32() == 0) {
                return Err(format!("{name}: font lacks {ch}").into());
            }
        }
        let chars = items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<String>()
            + if name == "cjk_24px" {
                CJK
            } else {
                "0123456789"
            };
        // Read the public API's actual metrics once, outside every measured interval.
        let mut store = FontStore::new(&gpu.device);
        let key = store.load_from_bytes(&gpu.device, &gpu.queue, bytes, &chars)?;
        for item in &mut items {
            let metrics = store.metrics(key, item.size).ok_or("missing metrics")?;
            item.ascent = metrics.ascent;
            item.line_height = metrics.ascent + metrics.descent + metrics.line_gap;
        }
        gpu.wait()?;
        Ok(Self {
            name,
            bytes,
            family,
            chars,
            items,
        })
    }
    fn texts(&self, revision: Option<usize>) -> Vec<String> {
        self.items
            .iter()
            .map(|item| {
                let mut text = item.text.clone();
                if let Some(revision) = revision {
                    // Unique content on every reshape sample defeats shape-run memoization.
                    // Only characters already loaded into both font databases are used.
                    if self.name == "cjk_24px" {
                        let alphabet: Vec<_> = CJK.chars().collect();
                        let mut n = revision;
                        for _ in 0..6 {
                            text.push(alphabet[n % alphabet.len()]);
                            n /= alphabet.len();
                        }
                    } else {
                        text.push_str(&format!(" {revision:06}"));
                    }
                }
                text
            })
            .collect()
    }
}

struct Ours {
    store: FontStore,
    key: FontId,
    renderer: TextRenderer,
    writer: TypeWriter,
    blocks: Vec<TextBlock>,
    cached: bool,
}
struct Glyphon {
    font_system: FontSystem,
    // Cache, viewport, atlas and renderer live for the entire warm/reshape run.
    _cache: Cache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: glyphon::TextRenderer,
    swash: SwashCache,
    buffers: Vec<Buffer>,
}
enum Renderer {
    Ours(Box<Ours>),
    Glyphon(Box<Glyphon>),
}
impl Renderer {
    fn new(index: usize, scene: &Scene, gpu: &Gpu) -> Result<(Self, f64, f64)> {
        Self::new_format(index, scene, gpu, FORMAT)
    }
    fn new_format(
        index: usize,
        scene: &Scene,
        gpu: &Gpu,
        format: wgpu::TextureFormat,
    ) -> Result<(Self, f64, f64)> {
        let start = Instant::now();
        if index != 1 {
            let mut store = FontStore::new(&gpu.device);
            let key = store.load_from_bytes(&gpu.device, &gpu.queue, scene.bytes, &scene.chars)?;
            let loading = ms(start);
            let start = Instant::now();
            let mut renderer = TextRenderer::new(&gpu.device, format, &store);
            renderer.resize(&gpu.queue, [WIDTH, HEIGHT]);
            Ok((
                Self::Ours(Box::new(Ours {
                    store,
                    key,
                    renderer,
                    writer: TypeWriter::new(),
                    blocks: Vec::new(),
                    cached: index == 2,
                })),
                loading,
                ms(start),
            ))
        } else {
            // Never call FontSystem::new(): it scans installed system fonts.
            // Only bench font bytes enter this database; no filesystem fallback.
            let mut db = glyphon::fontdb::Database::new();
            db.load_font_data(scene.bytes.to_vec());
            let font_system = FontSystem::new_with_locale_and_db("en-US".into(), db);
            let loading = ms(start);
            let start = Instant::now();
            let cache = Cache::new(&gpu.device);
            let viewport = Viewport::new(&gpu.device, &cache);
            let mut atlas = TextAtlas::new(&gpu.device, &gpu.queue, &cache, format);
            let renderer = glyphon::TextRenderer::new(
                &mut atlas,
                &gpu.device,
                wgpu::MultisampleState::default(),
                None,
            );
            Ok((
                Self::Glyphon(Box::new(Glyphon {
                    font_system,
                    _cache: cache,
                    viewport,
                    atlas,
                    renderer,
                    swash: SwashCache::new(),
                    buffers: Vec::new(),
                })),
                loading,
                ms(start),
            ))
        }
    }
    fn shape(&mut self, scene: &Scene, texts: &[String]) -> Result<()> {
        match self {
            Self::Ours(state) => {
                state.blocks = scene
                    .items
                    .iter()
                    .zip(texts)
                    .map(|(item, text)| {
                        state.writer.layout_text(
                            &state.store,
                            state.key,
                            item.position,
                            item.size,
                            [0., 0., 0., 1.],
                            text,
                            &TextLayout::default()
                                .with_max_width(item.width)
                                .with_line_height(LineHeight::Pixels(item.line_height)),
                        )
                    })
                    .collect::<std::result::Result<_, _>>()?;
                if state.cached {
                    for block in &mut state.blocks { block.set_render_mode(wgpu_font_renderer::RenderMode::Cached); }
                }
            }
            Self::Glyphon(state) => {
                if state.buffers.is_empty() {
                    state.buffers = scene
                        .items
                        .iter()
                        .map(|item| {
                            let mut buffer = Buffer::new(
                                &mut state.font_system,
                                Metrics::new(item.size, item.line_height),
                            );
                            // Shape the full block on both sides. Target clipping is done by
                            // the renderers, never by an asymmetric layout-height limit.
                            buffer.set_size(Some(item.width), None);
                            buffer.set_wrap(Wrap::Word);
                            buffer
                        })
                        .collect();
                }
                for (buffer, text) in state.buffers.iter_mut().zip(texts) {
                    buffer.set_text(
                        text,
                        &Attrs::new().family(Family::Name(&scene.family)),
                        Shaping::Advanced,
                        None,
                    );
                    buffer.shape_until_scroll(&mut state.font_system, false);
                }
            }
        }
        Ok(())
    }
    fn prepare(&mut self, scene: &Scene, gpu: &Gpu) -> Result<()> {
        self.prepare_target(scene, gpu, [WIDTH, HEIGHT], false)
    }
    fn prepare_target(
        &mut self,
        scene: &Scene,
        gpu: &Gpu,
        [width, height]: [u32; 2],
        integer_baselines: bool,
    ) -> Result<()> {
        match self {
            Self::Ours(state) => state.renderer.prepare(
                &gpu.device,
                &gpu.queue,
                state.blocks.iter().flat_map(TextBlock::lines),
                &state.store,
            ),
            Self::Glyphon(state) => {
                state
                    .viewport
                    .update(&gpu.queue, Resolution { width, height });
                state.renderer.prepare(
                    &gpu.device,
                    &gpu.queue,
                    &mut state.font_system,
                    &mut state.atlas,
                    &state.viewport,
                    state
                        .buffers
                        .iter()
                        .zip(&scene.items)
                        .map(|(buffer, item)| {
                            // Cosmic-text centres glyph metrics in its line box; this crate
                            // anchors at ascent. Align the first baseline as closely as allowed,
                            // then use identical line heights. glyphon rounds line_y and
                            // truncates top separately, so later baselines can differ ~1.5px.
                            // Shaping and wrapping can differ at break boundaries;
                            // ink and layout-count checks flag large mismatches.
                            let first_baseline = buffer
                                .layout_runs()
                                .next()
                                .map_or(item.ascent, |run| run.line_y);
                            TextArea {
                                buffer,
                                left: item.position[0],
                                // glyphon adds round(run.line_y) to trunc(top) separately.
                                // In the quality grid, baseline - round(line_y) is integral;
                                // baseline - line_y would accidentally shift some glyphs 1px.
                                top: if integer_baselines {
                                    (item.position[1] + item.ascent).round()
                                        - first_baseline.round()
                                } else {
                                    item.position[1] + item.ascent - first_baseline
                                },
                                scale: 1.,
                                bounds: TextBounds {
                                    left: 0,
                                    top: 0,
                                    right: width as i32,
                                    bottom: height as i32,
                                },
                                default_color: Color::rgb(0, 0, 0),
                                custom_glyphs: &[],
                            }
                        }),
                    &mut state.swash,
                )?;
            }
        }
        Ok(())
    }
    fn render(&self, pass: &mut wgpu::RenderPass<'_>) -> Result<()> {
        match self {
            Self::Ours(state) => state.renderer.render(pass),
            Self::Glyphon(state) => state.renderer.render(&state.atlas, &state.viewport, pass)?,
        }
        Ok(())
    }
    fn layout_stats(&self) -> Value {
        let (lines, glyphs) = match self {
            Self::Ours(state) => (
                state
                    .blocks
                    .iter()
                    .map(TextBlock::line_count)
                    .sum::<usize>(),
                state
                    .blocks
                    .iter()
                    .flat_map(TextBlock::lines)
                    .map(|line| line.glyphs().len())
                    .sum::<usize>(),
            ),
            Self::Glyphon(state) => (
                state
                    .buffers
                    .iter()
                    .map(|buffer| buffer.layout_runs().count())
                    .sum(),
                state
                    .buffers
                    .iter()
                    .flat_map(|buffer| buffer.layout_runs())
                    .map(|run| run.glyphs.len())
                    .sum(),
            ),
        };
        json!({"lines": lines, "glyphs_including_spaces": glyphs})
    }
    fn trim(&mut self) {
        if let Self::Glyphon(state) = self {
            state.atlas.trim();
        }
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    queries: Option<wgpu::QuerySet>,
    encoder_timestamps: bool,
    resolve: wgpu::Buffer,
    timing_readback: wgpu::Buffer,
    pixels: wgpu::Buffer,
}
impl Gpu {
    fn wait(&self) -> Result<()> {
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }
    fn frame(&self, renderer: &Renderer, repeat: usize, timed: bool, pixels: bool) -> Result<f64> {
        Ok(self.frame_details(renderer, repeat, timed, pixels, true)?.0)
    }
    fn frame_details(
        &self,
        renderer: &Renderer,
        repeat: usize,
        timed: bool,
        pixels: bool,
        drain_uploads: bool,
    ) -> Result<(f64, Instant)> {
        // Queue::write_buffer/texture copies run before the encoder. Drain them
        // outside the interval so the CPU submit+wait fallback excludes prepare
        // uploads just like timestamp queries do. Apply this to both clocks.
        if timed && drain_uploads {
            self.queue.submit([]);
            self.wait()?;
        }
        let queries = self.queries.as_ref().filter(|_| timed);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        if let Some(queries) = queries.filter(|_| self.encoder_timestamps) {
            encoder.write_timestamp(queries, 0);
        }
        {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &attachments,
                timestamp_writes: queries
                    .filter(|_| !self.encoder_timestamps)
                    .map(|query_set| wgpu::RenderPassTimestampWrites {
                        query_set,
                        beginning_of_pass_write_index: Some(0),
                        end_of_pass_write_index: Some(1),
                    }),
                ..Default::default()
            });
            // Both use their public render API R times, including identical rebinding
            // overhead. No crate-specific indexed-draw shortcut is used on either side.
            for _ in 0..repeat {
                renderer.render(&mut pass)?;
            }
        }
        if let Some(queries) = queries {
            if self.encoder_timestamps {
                encoder.write_timestamp(queries, 1);
            }
            encoder.resolve_query_set(queries, 0..2, &self.resolve, 0);
            encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.timing_readback, 0, 16);
        }
        if pixels {
            // 1920 * 4 is already a multiple of COPY_BYTES_PER_ROW_ALIGNMENT.
            encoder.copy_texture_to_buffer(
                self.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.pixels,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(WIDTH * 4),
                        rows_per_image: Some(HEIGHT),
                    },
                },
                self.texture.size(),
            );
        }
        let commands = encoder.finish();
        let start = Instant::now();
        self.queue.submit(Some(commands));
        self.wait()?;
        let completed = Instant::now();
        let elapsed = completed.duration_since(start).as_secs_f64() * 1000.;
        if queries.is_some() {
            let data = read_buffer(&self.device, &self.timing_readback)?;
            let begin = u64::from_ne_bytes(data[..8].try_into()?);
            let end = u64::from_ne_bytes(data[8..].try_into()?);
            let ticks = end.checked_sub(begin).ok_or("timestamp underflow")?;
            let elapsed = ticks as f64 * f64::from(self.queue.get_timestamp_period()) / 1e6;
            if !elapsed.is_finite() || elapsed <= 0. || elapsed > 60_000. {
                return Err(
                    format!("invalid timestamp duration: {elapsed} ms ({begin}..{end})").into(),
                );
            }
            Ok((elapsed / repeat as f64, completed))
        } else {
            Ok((elapsed / repeat as f64, completed))
        }
    }
    fn ink(&self, renderer: &Renderer) -> Result<usize> {
        self.frame(renderer, 1, false, true)?;
        Ok(read_buffer(&self.device, &self.pixels)?
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[..3].iter().any(|&channel| channel < 255))
            .count())
    }
}
fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vs_glyphon readback"),
        size,
        usage,
        mapped_at_creation: false,
    })
}
fn read_buffer(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    device.poll(wgpu::PollType::wait_indefinitely())?;
    receiver.recv()??;
    let data = buffer.get_mapped_range(..)?.to_vec();
    buffer.unmap();
    Ok(data)
}
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}
fn quantile(values: &[f64], fraction: f64) -> f64 {
    assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = (sorted.len() - 1) as f64 * fraction;
    sorted[index.floor() as usize]
        + (sorted[index.ceil() as usize] - sorted[index.floor() as usize]) * index.fract()
}
fn summary(values: &[f64]) -> Value {
    json!({"median_ms": quantile(values, 0.5), "p95_ms": quantile(values, 0.95), "samples_ms": values})
}
fn count_env(name: &str, default: usize, minimum: usize) -> Result<usize> {
    let value = match std::env::var(name) {
        Ok(value) => value.parse()?,
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => return Err(error.into()),
    };
    if value < minimum {
        return Err(format!("{name} must be at least {minimum}").into());
    }
    Ok(value)
}

fn cpu(
    renderer: &mut Renderer,
    scene: &Scene,
    gpu: &Gpu,
    mode: &str,
    iterations: usize,
    index: usize,
) -> Result<Value> {
    let mut samples = Vec::new();
    let mut loading = Vec::new();
    let mut initialization = Vec::new();
    let warmup = if mode == "reshape" { 10 } else { 0 };
    for iteration in 0..warmup {
        let texts = scene.texts(Some(iterations + iteration));
        renderer.shape(scene, &texts)?;
        renderer.prepare(scene, gpu)?;
        gpu.frame(renderer, 1, false, false)?;
        renderer.trim();
    }
    for iteration in 0..iterations {
        // Input generation and allocation of the changing strings are not shaping.
        let texts = scene.texts((mode == "reshape").then_some(iteration));
        if mode == "cold" {
            let (fresh, font_ms, init_ms) = Renderer::new(index, scene, gpu)?;
            *renderer = fresh;
            loading.push(font_ms);
            initialization.push(init_ms);
            // Flush font-load GPU work before timing the first shape + prepare.
            gpu.queue.submit([]);
            gpu.wait()?;
        }
        let start = Instant::now();
        if mode != "warm" {
            renderer.shape(scene, &texts)?;
        }
        renderer.prepare(scene, gpu)?;
        samples.push(ms(start));
        gpu.frame(renderer, 1, false, false)?;
        renderer.trim();
    }
    let mut result = summary(&samples);
    result["untimed_warmup_iterations"] = json!(warmup);
    if mode == "cold" {
        let totals: Vec<_> = samples
            .iter()
            .zip(&loading)
            .zip(&initialization)
            .map(|((&prepare, &load), &init)| prepare + load + init)
            .collect();
        result["total_cpu_startup"] = summary(&totals);
        result["font_loading"] = summary(&loading);
        result["renderer_initialization"] = summary(&initialization);
    }
    Ok(result)
}

fn lock_version(package: &str) -> Result<&'static str> {
    let name = format!("name = \"{package}\"");
    include_str!("../../Cargo.lock")
        .lines()
        .skip_while(|line| *line != name)
        .nth(1)
        .and_then(|line| line.strip_prefix("version = \""))
        .and_then(|version| version.split_once('"'))
        .map(|(version, _)| version)
        .ok_or_else(|| format!("{package} missing from Cargo.lock").into())
}
fn renderer_name(index: usize) -> &'static str {
    match index { 0 => "wgpu-font-renderer", 1 => "glyphon", _ => "wgpu-font-renderer (cached)" }
}
fn enabled(name: &str) -> Result<bool> {
    match std::env::var(name).as_deref() {
        Ok("0") => Ok(false),
        Ok("1") | Err(std::env::VarError::NotPresent) => Ok(true),
        _ => Err(format!("{name} must be 0 or 1").into()),
    }
}
pub fn run() -> Result<()> {
    let scene_enabled = enabled("BENCH_SCENES")?;
    let zoom_enabled = enabled("BENCH_ZOOM")?;
    let memory_enabled = enabled("BENCH_MEMORY")?;
    let quality_enabled = enabled("BENCH_QUALITY")?;
    let rounds = count_env("BENCH_ROUNDS", 5, 3)?;
    let frames = count_env("BENCH_FRAMES", 30, 10)?;
    let warmup = count_env("BENCH_WARMUP", 10, 0)?;
    let cpu_iterations = count_env("BENCH_CPU_ITERS", 100.max(rounds * frames), 10)?;
    let cold_iterations = count_env("BENCH_COLD_ITERS", 30, 10)?;
    let filter = std::env::var("BENCH_SCENE").ok();
    if filter
        .as_ref()
        .is_some_and(|name| !NAMES.contains(&name.as_str()))
    {
        return Err(
            "BENCH_SCENE must name one of body_16px, ui_mixed, large_200px, cjk_24px".into(),
        );
    }
    let save = std::env::var("BENCH_SAVE").unwrap_or_else(|_| "latest".into());
    if save.is_empty()
        || !save
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err("BENCH_SAVE must contain only letters, digits, '-' or '_'".into());
    }
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = adapter::select(&instance)?;
    let info = adapter.get_info();
    let mut features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
    if !features.is_empty() {
        features |= adapter.features() & wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    }
    let encoder_timestamps = features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
    let clock = if features.is_empty() {
        "cpu_submit_wait"
    } else {
        "gpu_timestamp"
    };
    let source = if features.is_empty() {
        None
    } else if encoder_timestamps {
        Some("encoder")
    } else {
        Some("pass")
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vs_glyphon white sRGB target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let gpu = Gpu {
        view: texture.create_view(&Default::default()),
        texture,
        queries: (!features.is_empty()).then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("vs_glyphon timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        }),
        encoder_timestamps,
        resolve: buffer(
            &device,
            16,
            wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        ),
        timing_readback: buffer(
            &device,
            16,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        ),
        pixels: buffer(
            &device,
            u64::from(WIDTH) * u64::from(HEIGHT) * 4,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        ),
        device,
        queue,
    };
    let software = info.device_type == wgpu::DeviceType::Cpu
        || ["llvmpipe", "lavapipe", "swiftshader", "software"]
            .iter()
            .any(|name| info.name.to_lowercase().contains(name));
    println!(
        "Adapter: {} ({:?}); driver: {} {}",
        info.name, info.backend, info.driver, info.driver_info
    );
    println!("Clock: {clock}; timestamp_source={}; {WIDTH}x{HEIGHT} {FORMAT:?}; rounds={rounds}, frames={frames}, warmup={warmup}", source.unwrap_or("none"));
    if software {
        println!("CPU/software adapter: these numbers only check the harness and say nothing about GPU performance.");
    }
    let mut report = json!({"schema": 1, "adapter": {"name": info.name,
        "backend": format!("{:?}", info.backend), "driver": info.driver,
        "driver_info": info.driver_info, "device_type": format!("{:?}", info.device_type),
        "vendor": info.vendor, "device": info.device}, "software_adapter": software,
        "wgpu_version": lock_version("wgpu")?,
        "glyphon_version": lock_version("glyphon")?, "clock": clock, "timestamp_source": source,
        "target": [WIDTH, HEIGHT, "Rgba8UnormSrgb"], "clear": "white", "text_color": "black",
        "statistic": "median_of_round_p25", "frames_per_round": frames, "rounds": rounds,
        "warmup_frames": warmup, "cpu_iterations": cpu_iterations, "cold_iterations": cold_iterations,
        "cpu_clock": "wall_clock", "cpu_interval": "shape_if_changed_and_prepare; upload_enqueue_included; completion_excluded",
        "font_loading_interval": "database_or_store_creation_and_font_load; completion_excluded",
        "lp_num_threads": std::env::var("LP_NUM_THREADS").ok(),
        "cpu_cores": std::thread::available_parallelism()?.get(), "scenes": []});
    let scenes: Vec<_> = NAMES
        .into_iter()
        .filter(|name| scene_enabled && filter.as_ref().is_none_or(|f| f == name))
        .map(|name| Scene::new(name, &gpu))
        .collect::<Result<_>>()?;
    report["raster"] = if scene_enabled { raster::run(&gpu, &scenes, rounds)? } else { Value::Null };
    let mut states = Vec::new();
    let mut repeats = Vec::new();
    let mut gpu_samples = Vec::new();
    for scene in &scenes {
        let texts = scene.texts(None);
        let mut renderers = Vec::new();
        let mut single = Vec::new();
        for index in 0..3 {
            let (mut renderer, _, _) = Renderer::new(index, scene, &gpu)?;
            renderer.shape(scene, &texts)?;
            renderer.prepare(scene, &gpu)?;
            gpu.queue.submit([]);
            gpu.wait()?;
            let mut samples = Vec::new();
            for _ in 0..warmup.max(1) {
                renderer.prepare(scene, &gpu)?;
                samples.push(gpu.frame(&renderer, 1, true, false)?);
                renderer.trim();
            }
            single.push(quantile(&samples, 0.5));
            renderers.push(renderer);
        }
        // Shared R targets ~4 ms for the slower renderer. Using the faster one
        // would force unnecessarily long passes on the slower implementation.
        let mut repeat = (4. / single.iter().copied().fold(0., f64::max)).ceil().clamp(1., 1024.) as usize;
        // Refine at the repeated workload: one-draw clear/fixed overhead is
        // not multiplied by R, so a single-draw estimate alone can undershoot.
        for _ in 0..warmup.max(1) {
            let mut slower_pass_ms: f64 = 0.;
            for renderer in &mut renderers {
                renderer.prepare(scene, &gpu)?;
                slower_pass_ms =
                    slower_pass_ms.max(gpu.frame(renderer, repeat, true, false)? * repeat as f64);
                renderer.trim();
            }
            let next = (repeat as f64 * 4. / slower_pass_ms)
                .round()
                .clamp(1., 1024.) as usize;
            if next == repeat || (3.2..=4.8).contains(&slower_pass_ms) {
                break;
            }
            repeat = next;
        }
        println!(
            "Calibrated {}: R={repeat} (single-draw analytic/glyphon/cached {:.3}/{:.3}/{:.3} ms)",
            scene.name, single[0], single[1], single[2]
        );
        repeats.push(repeat);
        states.push(renderers);
        gpu_samples.push([Vec::<Vec<f64>>::new(), Vec::<Vec<f64>>::new(), Vec::<Vec<f64>>::new()]);
    }
    // Rotate both scene order and renderer order to distribute drift across rounds.
    for round in 0..rounds {
        for offset in 0..scenes.len() {
            let s = (offset + round) % scenes.len();
            for order in 0..3 {
                let index = (order + round + offset) % 3;
                let renderer = &mut states[s][index];
                for _ in 0..warmup {
                    renderer.prepare(&scenes[s], &gpu)?;
                    gpu.frame(renderer, repeats[s], true, false)?;
                    renderer.trim();
                }
                let mut samples = Vec::new();
                for _ in 0..frames {
                    renderer.prepare(&scenes[s], &gpu)?;
                    samples.push(gpu.frame(renderer, repeats[s], true, false)?);
                    renderer.trim();
                }
                gpu_samples[s][index].push(samples);
            }
        }
    }
    println!(
        "\nAll times in ms per frame; GPU statistic = median of round p25. CPU cells = median/p95."
    );
    println!("| Scene | Renderer | R | GPU p25 | GPU median | GPU p95 | Warm | Reshape | cold first prepare only | Cold total | Font load | Init | Ink |");
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for (s, scene) in scenes.iter().enumerate() {
        let mut results = Vec::new();
        let mut inks = Vec::new();
        for index in 0..3 {
            let renderer = &mut states[s][index];
            renderer.prepare(scene, &gpu)?;
            let layout = renderer.layout_stats();
            let ink = gpu.ink(renderer)?;
            if ink == 0 {
                return Err(format!("{}: renderer {index} drew no ink", scene.name).into());
            }
            renderer.trim();
            inks.push(ink);
            let warm = cpu(renderer, scene, &gpu, "warm", cpu_iterations, index)?;
            let reshape = cpu(renderer, scene, &gpu, "reshape", cpu_iterations, index)?;
            let cold = cpu(renderer, scene, &gpu, "cold", cold_iterations, index)?;
            let samples: Vec<_> = gpu_samples[s][index].iter().flatten().copied().collect();
            let p25s: Vec<_> = gpu_samples[s][index]
                .iter()
                .map(|round| quantile(round, 0.25))
                .collect();
            let mut timing = summary(&samples);
            timing["median_of_round_p25_ms"] = json!(quantile(&p25s, 0.5));
            timing["rounds"] = json!(gpu_samples[s][index].iter().enumerate().map(|(r, samples)|
                json!({"round": r + 1, "p25_ms": quantile(samples, 0.25), "samples_ms": samples})).collect::<Vec<_>>());
            let name = renderer_name(index);
            let cell = |value: &Value| {
                format!(
                    "{:.3}/{:.3}",
                    value["median_ms"].as_f64().unwrap(),
                    value["p95_ms"].as_f64().unwrap()
                )
            };
            println!(
                "| {} | {name} | {} | {:.3} | {:.3} | {:.3} | {} | {} | {} | {} | {} | {} | {ink} |",
                scene.name,
                repeats[s],
                quantile(&p25s, 0.5),
                quantile(&samples, 0.5),
                quantile(&samples, 0.95),
                cell(&warm),
                cell(&reshape),
                cell(&cold),
                cell(&cold["total_cpu_startup"]),
                cell(&cold["font_loading"]),
                cell(&cold["renderer_initialization"])
            );
            results.push(json!({"renderer": name, "gpu": timing, "layout": layout,
                "cpu": {"warm": warm, "reshape": reshape, "cold": cold}, "ink_pixels": ink}));
        }
        let a = &results[0]["layout"];
        let b = &results[1]["layout"];
        let lines_differ = a["lines"] != b["lines"];
        let glyph_a = a["glyphs_including_spaces"].as_u64().unwrap();
        let glyph_b = b["glyphs_including_spaces"].as_u64().unwrap();
        let glyph_difference =
            glyph_a.abs_diff(glyph_b) as f64 / glyph_a.min(glyph_b).max(1) as f64;
        println!("Layout {}: ours lines={} glyphs={glyph_a}; glyphon lines={} glyphs={glyph_b}; glyph difference={:.2}% (includes trailing break spaces)", scene.name, a["lines"], b["lines"], glyph_difference * 100.);
        if lines_differ || glyph_difference > 0.02 {
            println!("WARNING: {} layout differs (lines unequal or glyph count >2%); check scene conformity.", scene.name);
        }
        // Relative to the smaller count: either renderer exceeding the other
        // by more than 10% triggers the warning.
        let difference = inks[0].abs_diff(inks[1]) as f64 / inks[0].min(inks[1]).max(1) as f64;
        let warning = difference > 0.10;
        if warning {
            println!(
                "WARNING: {} ink differs by {:.1}% (>10%); scenes may not match.",
                scene.name,
                difference * 100.
            );
        }
        report["scenes"].as_array_mut().unwrap().push(json!({"name": scene.name,
            "font": if scene.name == "cjk_24px" { "NotoSerifSC-Subset.otf" } else { "Roboto-Regular.ttf" },
            "items": scene.items.iter().map(|item| json!({"text": item.text, "size": item.size,
                "position": item.position, "width": item.width, "ascent": item.ascent,
                "line_height": item.line_height})).collect::<Vec<_>>(),
            "repeat": repeats[s], "ink_difference_fraction": difference,
            "ink_warning": warning, "layout_check": {"lines_differ": lines_differ, "glyph_difference_fraction": glyph_difference, "warning": lines_differ || glyph_difference > 0.02}, "renderers": results}));
    }
    drop(states);
    gpu.wait()?;
    report["sections"] = json!({"scenes": scene_enabled, "zoom": zoom_enabled, "memory": memory_enabled, "quality": quality_enabled});
    report["memory"] = json!([]);
    if memory_enabled {
        memory::print_header();
        for scene in &scenes {
            for index in 0..3 {
                let before = memory::snapshot(&gpu)?;
                let (mut renderer, _, _) = Renderer::new(index, scene, &gpu)?;
                renderer.shape(scene, &scene.texts(None))?;
                renderer.prepare(scene, &gpu)?;
                gpu.frame(&renderer, 1, false, false)?;
                renderer.trim();
                let value = memory::capture(&gpu, &renderer, before.as_ref())?;
                memory::print_row(scene.name, renderer_name(index), &value);
                report["scenes"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|s| s["name"] == scene.name)
                    .unwrap()["renderers"][index]["memory"] = value.clone();
                report["memory"].as_array_mut().unwrap().push(
                    json!({"after": scene.name, "renderer": renderer_name(index), "held": value}),
                );
                drop(renderer);
                gpu.wait()?;
            }
        }
    }
    report["zoom"] = if zoom_enabled {
        zoom::run(&gpu, memory_enabled)?
    } else {
        Value::Null
    };
    let artifacts = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/vs-glyphon")
        .join(&save);
    report["quality"] = if quality_enabled {
        quality::run(&gpu, &artifacts)?
    } else {
        Value::Null
    };
    if quality_enabled {
        report["quality"]["adapter"] = report["adapter"].clone();
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/vs-glyphon")
        .join(format!("{save}.json"));
    std::fs::create_dir_all(path.parent().ok_or("missing report directory")?)?;
    std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    println!("Saved {}", path.display());
    Ok(())
}

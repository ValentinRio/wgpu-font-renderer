//! Head-to-head, single-device benchmark. Run with --offline; no system fonts.
//! GPU samples exclude prepare, shaping and readback, and include a white clear.
//! CPU samples are wall-clock API costs (including enqueueing uploads, not GPU
//! completion). A single-draw submit/wait and atlas.trim follow each CPU sample,
//! outside its interval, so uploads never accumulate across artificial frames.
//! BENCH_ROUNDS/FRAMES/WARMUP default to 5/30/10; BENCH_SCENE selects one scene,
//! WGPU_BACKEND selects the backend and BENCH_SAVE names target/vs-glyphon/*.json.
//! BENCH_CPU_ITERS defaults to max(100, rounds*frames), BENCH_COLD_ITERS to 30.
//! FontStore eagerly caches/uploads outlines during font loading; glyphon lazily
//! parses/rasterizes glyphs in prepare. Cold reports print first-prepare-only
//! alongside font load + init + first-prepare total. Reshape gets 10 untimed
//! warm-up frames. Quality reports both hinted/default and unhinted glyphon,
//! with per-glyph centroid and mass checks against the independent reference.
//! BENCH_SCENES, BENCH_ZOOM, BENCH_MEMORY, BENCH_QUALITY: 1 (default) enables,
//! 0 skips each section. For example BENCH_SCENES=0 BENCH_MEMORY=0 BENCH_QUALITY=0
//! runs zoom only. BENCH_SCENE filters static scenes, not zoom or quality.
//! Zoom uses two 255-frame sweeps (128 sizes) in EACH renderer order; no size knob.
//! Quality raw grids: target/vs-glyphon/<BENCH_SAVE>/quality_<renderer>_<size>.rgba.
//! Raster isolates cold unique-key bitmap generation with pipeline setup separate.
//! Memory requires unstable-internals; sources are allocator_report/computed/unknown.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    if let Err(error) = native::run() {
        eprintln!("vs_glyphon: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[path = "harness.rs"]
mod native;

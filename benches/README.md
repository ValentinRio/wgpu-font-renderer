# Shader benchmark

The native shader benchmark renders four fixed scenes at 1920×1080 in sRGB,
using the public font loading, shaping and rendering API. Run it with:

```sh
cargo bench --all-features --offline --bench shader
BENCH_ROUNDS=7 BENCH_FRAMES=40 BENCH_WARMUP=10 cargo bench --all-features --offline --bench shader -- large_glyphs
```

Set the `WGPU_BACKEND` environment variable to `dx12` or `vulkan` to select the backend (POSIX shells: `WGPU_BACKEND=dx12 cargo bench --all-features ...`; PowerShell: `$env:WGPU_BACKEND = "dx12"`; cmd: `set WGPU_BACKEND=dx12`). Leave it unset for wgpu's defaults. Some Vulkan drivers return zero timestamps (seen on an AMD RX 9070 XT with the proprietary driver); DX12 works there.

Both benchmarks default to `PowerPreference::HighPerformance`, preferring the
discrete GPU when available. `WGPU_POWER_PREF=low|high|none` overrides this using
wgpu's parser (`none` removes the power preference; unset or unrecognized values
use the high-performance default). `WGPU_ADAPTER_NAME` takes priority over the
power preference and selects the first adapter whose name contains that value,
case-insensitively, following wgpu's substring matching. An unmatched name exits
with a clear error. Each benchmark prints every enumerated adapter's name,
backend and device type, then the chosen adapter, before running measurements.

Defaults are **5 rounds**, each with 10 discarded warm-up frames and 30 measured
frames per scene. Override them with `BENCH_ROUNDS`, `BENCH_WARMUP` and
`BENCH_FRAMES` (minimum 3 rounds and 10 measured frames). The scene order rotates
between rounds to distribute drift. Each round reports its **p25 frame time**;
the comparison statistic is the **median of those round p25 values**. This lower
quantile reduces the effect of one-sided contention tails without taking the
single fastest frame. Frame median and p95 are also printed; the final p95 pools
all measured frames. `BENCH_SCENE=large_glyphs` also filters scenes. Arguments
beginning with `-`, such as `--nocapture`, are ignored.

Each scene reports total curves across glyph instances, covered glyph-quad pixel
centres (clipped to the target, including overdraw), and ink pixels (any RGB
channel below white). Curves include quadratics produced by splitting the
Cantarell CFF2 fixture's cubic outlines.

The clock is `gpu_timestamp` when the adapter supports `TIMESTAMP_QUERY`, with
encoder queries when `TIMESTAMP_QUERY_INSIDE_ENCODERS` is supported, otherwise render-pass queries; the header and reports record `timestamp_source` (`encoder` or `pass`, null for CPU timing), and comparisons require it to match. Otherwise `cpu_submit_wait` measures submission
through a blocking device poll, excluding command encoding. Loading, shaping,
preparation and image readback are outside the measured interval. Each frame is
submitted and completed separately. The GPU interval includes attachment clear
and store operations as well as drawing.

A draw that finishes in a fraction of a millisecond leaves the GPU idle most of
the time, and an idle GPU may drop to a lower clock. So each timed pass draws the
scene **R** times, and every reported timing is the pass time divided by R
(clear and store costs are amortized over the R draws). On the first round,
warm-up picks R per scene so that a pass takes about 4 ms, up to 1024 draws;
a scene that already takes longer keeps `R=1`. Calibration may add discarded
warm-up passes, saved as `calibration_retries`. `BENCH_REPEAT` (1 to 1024) forces
R. Pixel readback and checksums always use a single-draw frame.

Save a baseline on main and compare the branch on the same adapter and driver.
If main already contains the harness:

```sh
git switch main
BENCH_SAVE=main cargo bench --all-features --offline --bench shader
git switch shader-bench
BENCH_BASELINE=main cargo bench --all-features --offline --bench shader
```

If main predates the harness, use a separate checkout with the identical harness
and dependency declarations. From the branch checkout, after committing the
benchmark files:

```sh
git worktree add --detach /tmp/wgpu-font-renderer-main main
for file in Cargo.toml Cargo.lock benches/shader.rs; do
    mkdir -p "/tmp/wgpu-font-renderer-main/$(dirname "$file")"
    git show "HEAD:$file" > "/tmp/wgpu-font-renderer-main/$file"
done
(cd /tmp/wgpu-font-renderer-main && BENCH_SAVE=main cargo bench --all-features --offline --bench shader)
mkdir -p target/shader-bench/main
cp /tmp/wgpu-font-renderer-main/target/shader-bench/main.json target/shader-bench/main.json
cp -r /tmp/wgpu-font-renderer-main/target/shader-bench/main/. target/shader-bench/main/
BENCH_BASELINE=main cargo bench --all-features --offline --bench shader
```

Reports live in `target/shader-bench/<name>.json`. Schema 2 records the per-round
p25 statistics, **every measured frame sample**, frames and warm-up per round,
process IDs and per-round git revisions. It also records adapter/backend/driver,
clock, wgpu version, `LP_NUM_THREADS` (or null when unset), and the CPU core count
reported by `available_parallelism`. HEAD revisions do not represent uncommitted
edits. Comparisons refuse mismatches in adapter, driver, clock, target, statistic,
wgpu version, thread setting or CPU core count. The comparison prints the frames,
warm-up and round counts for both sides, including mixed configurations from
pooled baselines. Schema 1 baselines must be regenerated.

The effective detectable threshold is the larger of **3% of the baseline median**
and **3 × the largest spread estimate**, printed as **±X%** beside each verdict.
The estimates are the baseline's round-p25 MAD, the current run's round-p25 MAD,
and each side's half-range of per-process median p25 values. Each MAD uses its
own run's median, so a large tight baseline cannot mask a noisy candidate.
The process-median range retains drift sampled by a minority of appended
processes, even when the overall baseline MAD is zero. Process groups follow
round-number restarts in the saved records, so PID reuse does not merge them.
A shift exceeding that threshold is **faster** or **slower**; otherwise it is
**within noise**, which does not mean equal. This is a robust detection heuristic,
not a significance test. More frames stabilize each p25, while more rounds
capture drift; neither can recover drift between processes that was never sampled.

Some machines run a benchmark in one of two **clock states** about 2× apart, and
may switch between them mid-run. The harness pools the frame samples of all
rounds and classifies each scene, printed in its summary line with R:

- **`states=1`**: one mode. Isolated spikes, a single slow round, a smooth tail
  and a shoulder all count as one state.
- **`states=2`**: one clean split into two clusters. Each holds at least 10% of
  the frames and at least 10 frames, the slow one appears in at least 2 rounds,
  the medians are at least 1.4× apart, and an empty gap spans at least half
  that separation. The summary adds both medians and the fast-state share.
- **`states=unresolved`**: the samples have two density peaks with a clear
  valley between them but no clean gap, or more than one split qualifies.

A comparison then compares like with like, and marks the row **weaker**
whenever states were involved:

- Both sides two states: the delta and verdict use both **fast-state medians**.
- One side two states, the other one: the single side is matched to the nearer
  of the two states, and its median is compared with that state's median. The
  row says which state was matched, prints both medians of the other side
  with their frame shares, and ends with `repeat run`. The two runs saw a
  different mix of states, so a **within noise** here
  cannot rule out a real change in the unmatched state.
- A single side that is 1.4× or more from both states matches neither. The row
  compares against the nearer one and adds a clock-state caution.
- Either side unresolved: the row says so and the verdict is not reliable.

When both sides have one state but their medians are 1.4× or more apart, in
either direction, the row carries the
same caution: each run may have sat wholly in a different state, which no single
run can detect. The harness does not know whether the rendering changed, so the
caution appears either way. Repeat the run before believing it.

Comparisons reuse the baseline's R per scene. A forced `BENCH_REPEAT` that
differs from the baseline's, or an append onto a file saved with a different R,
is refused before rendering. A saved scene without a `repeat` field means
`R=1`, so older schema 2 baselines stay usable.

To include run-level drift, pool several separate invocations of the **same code**
into the baseline before comparing. For Mesa lavapipe, an explicit thread limit
can reduce contention; use the same setting on every run, for example:

```sh
export LP_NUM_THREADS=2
BENCH_SAVE=main cargo bench --all-features --offline --bench shader
BENCH_SAVE=main BENCH_APPEND=1 cargo bench --all-features --offline --bench shader
BENCH_SAVE=main BENCH_APPEND=1 cargo bench --all-features --offline --bench shader
# Switch to the candidate branch, retaining the baseline and RGBA directory.
BENCH_BASELINE=main cargo bench --all-features --offline --bench shader
```

`BENCH_APPEND=1` pools round records and recomputes summaries; it refuses changed
fingerprints or workloads. Without append, a full save replaces the file. A
**filtered save always merges its measured scenes into an existing baseline**,
preserves unselected scenes and their images, and prints that it merged them.
Per-scene round records are authoritative after merges or appends.

The exact FNV-1a checksum remains the strict output signal. Saves also write the
packed, unpadded RGBA8 readback to `target/shader-bench/<name>/<scene>.rgba`.
Keep that directory together with its JSON when copying baselines. Comparison
checks the saved RGBA checksum and reports **differing pixel count**, **maximum
absolute channel delta** over all RGBA channels, and ink count delta, separately
from timing. Output is **identical** when no pixels differ, **minor** when at most
32 pixels differ and max channel delta is at most 2, otherwise **OUTPUT CHANGED**.
Tune these limits with `BENCH_PIXEL_TOLERANCE` and `BENCH_CHANNEL_TOLERANCE`;
setting both to 0 gives strict classification. A minor result still prints a
changed exact checksum. Missing baseline scenes are reported explicitly.

Lavapipe is a software renderer: its numbers only rank changes relative to each
other on that setup. Real GPU performance conclusions require a real GPU.

# vs_glyphon

Every section includes `wgpu-font-renderer (cached)` with the same scenes and
measurement intervals. Cold prepare includes GPU cache rasterization enqueueing;
GPU completion remains outside the CPU interval, as for glyphon uploads. Memory
includes the full physical R8 atlas and its raster uniform buffer. Cached zoom
uses exact size keys without prewarming. Quality retains both glyphon hinting
variants and adds cached coverage with its documented pen snapping.

This native harness compares wgpu-font-renderer (analytic and cached) with glyphon 0.12 on one wgpu
30 device. It lives in the unpublished `vs-glyphon` workspace crate, which enables
`unstable-internals` on the library for memory inspection and requires Rust 1.89
or newer (the library keeps its Rust 1.88 MSRV). Run from the repository root;
all dependencies and fonts must already be available locally:

```sh
BENCH_ROUNDS=3 BENCH_FRAMES=10 BENCH_WARMUP=3 cargo bench -p vs-glyphon --all-features --offline
BENCH_SAVE=my-run cargo bench -p vs-glyphon --all-features --offline
# Zoom only, including its memory checkpoints:
BENCH_SCENES=0 BENCH_QUALITY=0 cargo bench -p vs-glyphon --all-features --offline
# Quality only:
BENCH_SCENES=0 BENCH_ZOOM=0 BENCH_MEMORY=0 cargo bench -p vs-glyphon --all-features --offline
```

Every section is enabled by default. Reports replace
`target/vs-glyphon/<BENCH_SAVE>.json`; the default name is `latest`. Quality grids
are saved under `target/vs-glyphon/<BENCH_SAVE>/`. Unlike the shader benchmark,
this harness does not append, merge filtered runs, or compare baselines.

| Environment variable | Default | Meaning |
|---|---|---|
| `BENCH_SCENES` | `1` | Static scenes and their timing/ink table; `0` skips them. |
| `BENCH_ZOOM` | `1` | Two zoom passes; `0` skips them. |
| `BENCH_MEMORY` | `1` | Memory after static scenes and zoom passes; `0` skips it. No checkpoints are produced for a skipped scene/zoom section. |
| `BENCH_QUALITY` | `1` | Isolated-glyph quality grids, self-checks and raw files; `0` skips them. |
| `BENCH_SCENE` | all four | Filter static scenes to `body_16px`, `ui_mixed`, `large_200px`, or `cjk_24px`. Does not filter zoom or quality. |
| `BENCH_ROUNDS` | `5` | Static GPU rounds; minimum 3. |
| `BENCH_FRAMES` | `30` | Measured GPU frames per static round; minimum 10. |
| `BENCH_WARMUP` | `10` | Discarded frames before each static round; minimum 0. Calibration still performs at least one warm-up sample and a repeated-pass refinement. |
| `BENCH_CPU_ITERS` | `max(100, rounds × frames)` | Static CPU iterations in each warm/reshape mode; minimum 10. |
| `BENCH_COLD_ITERS` | `30` | Fresh-cache static CPU iterations; minimum 10. |
| `BENCH_SAVE` | `latest` | Report/artifact name; letters, digits, `-` and `_` only. |
| `WGPU_BACKEND` | wgpu defaults | Backend selection through wgpu's environment handling, e.g. `vulkan` or `dx12`. |
| `LP_NUM_THREADS` | driver default | Mesa software-renderer thread setting, recorded in JSON. Keep it consistent when comparing runs. |

The section switches accept only `0` or `1`. Round/frame/CPU knobs affect the
static section; zoom always has 128 distinct geometric sizes and two fixed
255-frame passes in each of two renderer orders, and quality always uses sizes 12, 16, 24, 48 and 96 px.

## Static timing and ink

The original table and `scenes[].renderers[]` JSON remain intact. GPU p25 is the
median of the per-round p25 values; GPU median and p95 pool all measured samples.
Times are milliseconds per draw. Both renderers use the same calibrated repeat
count R for a scene, targeting a roughly 4 ms pass for the slower renderer,
limited to 1–1024. A slower single draw keeps R=1. Clear/store overhead is
amortized over R. Scene and renderer order rotate between rounds.

GPU timestamps surround the white clear and rendering, using encoder queries
when supported, otherwise pass queries. The fallback is CPU submit+wait.
Shaping, prepare, queued prepare uploads and readback are outside static GPU
intervals. Every GPU sample is individually submitted and completed.

CPU cells are median/p95 wall-clock milliseconds for API calls, including
upload enqueueing but excluding GPU completion. `warm` prepares already-shaped,
unchanged text. `reshape` uses unique new text each iteration and includes
shaping/layout plus prepare. Both paths perform 10 untimed reshape frames first,
with distinct revisions outside the measured set, to warm newly introduced digits
and CJK glyphs. The count is recorded in JSON. `cold` uses fresh font and
renderer caches for each iteration and prints `cold first prepare only` (shaping/layout plus prepare),
`Cold total` (font load + init + first shape/prepare), font loading and setup
separately. Cold total is summed per iteration before computing median/p95,
not assembled from the three medians. JSON retains `total_cpu_startup`.
This crate front-loads parsing and atlas upload into font load; glyphon
parses/rasterizes lazily in prepare. On a CPU adapter, this crate's init is
mostly pipeline compilation, so first-prepare-only ordering is not startup
ordering. These are CPU API costs: completion and the following draw are still
excluded, even in the total.
A single-draw frame, submit/wait, and glyphon atlas trim follow every CPU sample
outside its timed interval.

Ink counts come from one single-draw frame and count pixels with any RGB channel
below white. A difference greater than 10% relative to the smaller count emits a
warning. Matching ink is a coarse scene-conformity check, not a quality verdict.
Both counts are recorded per renderer in JSON and printed per scene. The harness
warns if line counts differ or glyph counts differ by more than 2% relative to the
smaller count. Counts include spaces: this crate preserves trailing break spaces
that cosmic-text drops. For the body scene, 8,937 versus 8,829 glyphs differ by
about 1.22%; those spaces have no ink, and the line counts both equal 159. This
known whitespace difference does not by itself mean different visible workloads.

## Raster only

The `raster` section runs all four selected static scenes (`BENCH_SCENE` still
filters them) for `BENCH_ROUNDS` cold samples per renderer, alternating renderer
order each round. Cells contain median/p95 total milliseconds, then median/p95
microseconds per unique bitmap glyph. This section is part of `BENCH_SCENES`.

The workload is derived from glyphon 0.12's prepare traversal: visible layout
runs using the same TextArea offsets/bounds, then `LayoutGlyph::physical` keys.
Glyphon clips individual glyphs only after raster/upload, so horizontal and ink
clipping do not remove keys here. Keys retain first-use order and default flags
(hinted), font size and quarter-pixel bins. Empty/no-image keys (including
spaces) are counted separately and excluded from both bitmap workloads. Repeated
empty probes and atlas-growth re-rasterization from a complete glyphon prepare
are not part of this unique-key measurement. The same font bytes and glyph IDs,
sizes and X bins are fed through public `Paragraph` construction to our cached
renderer; there is no independent shaping or layout in either timed interval.
Current scenes have zero Y bins, checked at runtime. Both bitmap counts must
match the shared key count, with no fallback, or the benchmark exits with error.

- **Glyphon CPU raster** calls the same
  `SwashCache::get_image_uncached` as glyphon's atlas-miss path, with a fresh
  SwashCache per round. Font loading/shape/physical-key discovery are excluded.
  Images are retained until upload and raster timing includes their collection.
- **Glyphon CPU upload** measures one `Queue::write_texture` per bitmap with
  glyphon's R8 mask format, stride and image dimensions. A real atlas is packed
  ahead of time; packing, texture creation/growth, cache bookkeeping and GPU
  completion are excluded. This is the CPU write cost, not GPU upload time or
  full glyphon prepare. Each sample's uploads are submitted and drained outside
  the interval. Non-mask content is rejected for these bundled-font scenes.
- **Cached CPU encode** measures cold `prepare` on the unique-key paragraphs:
  lookup, allocation/growth, temporary raster instance upload, raster encoding,
  submission and prepared-instance upload. It excludes shaping, outline loading,
  bitmap pipeline creation, cache reset and GPU completion. The bitmap atlas is
  empty and starts at its normal 512 dimension each round.
- **Cached GPU raster** timestamps only the actual bitmap pass (per-glyph
  clear/bake and attachment load/store), excluding atlas growth copies and
  submission/readback. As in other GPU sections, encoder timestamps are used
  when supported, otherwise pass timestamps. Without timestamp queries this
  cell is `unavailable` and JSON is null; CPU timings still exist. These cold
  samples are unamortized, one pass per round, with no R repeats or round-p25
  aggregation.

A separate once-only CPU line measures bitmap shader, draw/bake/clear pipeline
creation and initial atlas/resources. Base renderer initialization is outside
that line. Driver pipeline caches may persist across runs; this line measures CPU creation
API cost. The pipelines survive all raster
scenes/rounds; no creation enters a raster sample. The `unstable-internals` hook
only initializes those resources and attaches optional raster timestamps.

JSON `raster` retains pipeline creation time, interval descriptions, clock/source,
per-scene shared and renderer bitmap counts, excluded key counts, rounds, raw
samples and both units' median/p95. CPU and GPU columns describe different
intervals and should be compared with their stated inclusions in mind. Software
adapter numbers only validate the harness.

## Zoom stress

`Zoom text` uses Roboto and moves through 128 distinct sizes geometrically from
8 to 256 pixels per em, then back through the same sizes without duplicating the
turnaround endpoint. Each pass has 255 frames. Each renderer starts with fresh
caches for pass 1 and reuses them unchanged into pass 2. No sizes are prewarmed.
The complete pair of passes is run in both orders: ours→glyphon→cached, then
cached→glyphon→ours, with fresh library caches for each order. Every row records its
order in the table and JSON. All eight rows are reported independently; device
pipeline caches can persist across orders.
Each frame relayouts at the new size, prepares, renders exactly once, submits and
waits. Glyphon keeps its buffers/text, changes metrics and reshapes layout, then
calls `Viewport::update` and `TextRenderer::prepare`; it trims after every frame.
Zoom drives glyphon through `Buffer::set_metrics` and this crate through a full
TypeWriter relayout per frame, each API's natural path for a size change. This
crate has no public Paragraph/TextBlock size setter or cheaper pure-size relayout
API. This distinction is part of the measured application workload.
The second pass exposes whatever atlas contents survive glyphon's normal LRU
policy, rather than assuming that all sizes fit simultaneously.

The zoom table reports median, p95 and maximum total frame wall time, plus GPU
timestamp median/p95/max when supported. Wall time includes relayout, prepare,
encoding, uploads, drawing and submit/wait; font loading/renderer setup, timestamp
readback and post-frame trim are excluded. Zoom GPU columns are labelled
**single-draw, unamortized, clear included**. Unlike static scenes' R-amortized
per-draw numbers, they include the entire clear/draw/store for one draw; do not
compare the columns as equivalent measures. Upload commands precede timestamps. With no timestamps,
GPU columns are unavailable; wall times still exist. The five worst wall-time
frames include zero-based indices and sizes. JSON stores the size sequence,
every sample, worst-frame timings, both passes in each order and their memory
checkpoints. Both orders reduce fixed-order bias, but machine drift can still
affect sequential sweeps; repeat runs before drawing conclusions.

## GPU memory held

Memory cells are bytes, each labelled `allocator_report`, `computed`, or
`unknown`. This counts retained GPU textures and buffers, including renderer
uniforms and fixed geometry, but excludes the benchmark target, readback/query
buffers, CPU caches, pipeline internals and general driver overhead.

The output separates this crate's outline atlas, lazy R8 bitmap atlas and buffers.
The outline atlas packs only written outline and y-band prefixes into 512-wide
RGBA32Float rows, with at most one 8 KiB row of slack per physical layer. Packed
atlas addresses are carried in instance data; there are no storage buffers. Each
scene loads one font instance; `Font data (outline + bands)` reports its actual
payload bytes, excluding row padding. JSON records both components and their sum. Instances grow exactly
to the requested byte count and retain that high-water capacity; fixed geometry
and uniforms use 140 bytes: four `[f32; 3]` vertices (48), six `u16` indices (12),
and viewport/padding/transform uniforms (80 = 8 + 8 + 64). The lazy bitmap cache
retains another 80-byte uniform buffer. Temporary bitmap raster instances are
released before the settled snapshot.
Glyphon's mask/color atlases start at 256×256 and grow on demand. These numbers
measure the current formats and allocation policies; matching glyphon's footprint
is not assumed.

When `Device::generate_allocator_report` returns usable labelled allocations,
the harness settles GPU work, takes a baseline with no measured renderer, then
subtracts the baseline's labelled texture/buffer allocations from a report with
that renderer alive. Other renderers are absent at each checkpoint. Static
checkpoints reproduce one complete frame of the same scene with fresh state
after timing; zoom checkpoints inspect the actual retained state after each
pass. Device-wide allocated/reserved totals are recorded separately in JSON and
are never attributed to the renderer. Allocator byte sizes can include alignment
and differ from logical sizes. Shared device block reservations are not renderer
memory usage. JSON preserves `computed_total` before using allocator values;
when both totals exist, `computed_vs_report_warning` flags an absolute difference
greater than the larger of 4 KiB or 1% of the report total, allowing allocator
alignment padding. The memory table prints the warning with the scene, renderer
and tolerance. Glyphon's total stays unknown without usable allocator labels, so
no comparison is possible there.

If reports are unavailable or lack usable labels, this crate's full atlas texture
extent and instance buffer size come from `internals`; fixed geometry/uniform
sizes are documented from `src/renderer.rs`. These are logical computed sizes,
excluding driver padding. Glyphon 0.12 hides its atlas dimensions and vertex
capacity. Its atlas dimensions double from 256×256 and its vertex buffer grows
from 4096 bytes to the next power of two, but growth/eviction prevents inferring
current capacity through its public API. Those totals are `unknown` with reasons,
not estimated from glyph counts. Its known 16-byte viewport uniform is recorded
separately. Any available device allocator totals retain their own source.

## Output quality

Each grid isolates Roboto's Latin letters, digits and punctuation, plus six CJK
glyphs from Noto Serif SC, in separate cells. Pens and baselines are integer pixel
positions. Both renderers use a white, black-text `Rgba8Unorm` target, so stored
RGB is 255 × (1 − coverage), without an sRGB transfer function. One glyph per
paragraph/buffer excludes kerning, wrapping and text-layout differences.

The independent reference uses `ab_glyph` 0.2 to parse and rasterize the same font
outlines without hinting. Explicit scale factors use pixels per em: ab_glyph's
normal `PxScale` instead denotes font height. Glyph.position is the baseline;
this crate's top is baseline minus FontStore ascent. Cached cosmic-text 0.19 and
glyphon 0.12 sources show that glyphon truncates physical Y and separately rounds
`run.line_y`; its quality TextArea top is therefore baseline minus
`round(run.line_y)`. This explicitly avoids a one-pixel convention mismatch.
At integer pens, X subpixel bins are zero.

The quality table includes `wgpu-font-renderer (cached)`, `glyphon (hinted, default)` and `glyphon (unhinted)` rows.
The default row preserves swash hinting, alpha masks, quarter-pixel X bins and
vertical snapping. The unhinted row uses
`Attrs::cache_key_flags(glyphon::cosmic_text::CacheKeyFlags::DISABLE_HINTING)`;
all other settings and positions are identical. The source finding and correction
are recorded in JSON. Compare the unhinted row with the unhinted reference when
investigating placement; the default row also measures hinting's intentional shape
changes. Larger errors do not automatically mean worse readability.

For every renderer and size, errors are in coverage units (0–1): mean absolute
error across the entire grid including white background/padding, maximum error,
and the count of pixels whose absolute error exceeds 0.1. Float reference
coverage is used for metrics; GPU coverage is recovered from 8-bit RGB. The
reference compared with itself must report exactly zero for all three measures.
Every row also checks each isolated glyph's coverage centroid and mass against
the reference, including all pixels in that cell. Signed per-glyph (dx, dy)
offsets and mass ratios are stored in JSON. The table reports maxima of absolute
x/y offsets, mass ratio min/max and maximum absolute mass deviation from 1.
Warnings fire for a centroid offset norm above 0.05 px or any mass ratio outside
0.99–1.01. A small-background MAE cannot hide these checks. The reference
self-check also verifies zero centroid shift and unit mass ratio. Synthetic
one-pixel translation and half-mass checks validate that the placement checker
can detect those errors. Centroid changes can also result from rasterization or
hinted shape changes, so inspect the per-glyph diagnostics instead of treating a
warning alone as proof of a misplaced pen.

On the recorded llvmpipe run, this crate's real maximum coverage errors were
0.341, 0.334, 0.261, 0.337 and 0.262 at 12, 16, 24, 48 and 96 px respectively;
the low background-averaged means do not replace these maxima. Quality figures
carry the adapter/backend/driver they came from, both in the report header and
explicitly under `quality.adapter`. Re-run on the target adapter.

`ab_glyph` uses analytic area accumulation for line segments with adaptive curve
flattening; it is independent, not an exact mathematical oracle for curves.
Against this approximated, float reference, even an analytic GPU renderer has a
nonzero error floor from curve flattening, filtering and 8-bit quantization. A
reference compared with itself being zero does not establish a zero-error floor
for an independent renderer. GPU
and saved reference quantization, curve approximation, hinting and rasterizer
filtering remain legitimate sources of differences. This grid does not evaluate
complex shaping, compositing on other backgrounds, LCD text or perceived quality.

The four raw files for each size are packed row-major RGBA8 with opaque alpha:
`quality_reference_<size>.rgba`, `quality_wgpu-font-renderer_<size>.rgba` and
`quality_glyphon_<size>.rgba` (hinted default), plus
`quality_glyphon_unhinted_<size>.rgba` and `quality_wgpu-font-renderer_cached_<size>.rgba`. JSON records their directory, width, height,
cell positions and filenames. The saved reference is quantized to 8-bit for
human diffing; it does not replace the float reference used for metrics.

## Fairness and interpretation

Both paths use identical bundled font bytes, strings, positions, sizes, black
text, target dimensions/format and white clears. Cosmic-text's database contains
only the font bytes for that scene; no system-font discovery/fallback can select
an unrelated face. Static line heights come from FontStore metrics, and the
first baselines are aligned as closely as the APIs allow. Glyphon's actual
baseline is `round(run.line_y) + trunc(top)` separately for each line; later
lines in timing scenes can differ by up to about 1.5 px from this crate's float
baseline. Timing is unaffected by that baseline convention. Quality instead
places each glyph at an explicitly corrected integer baseline. Shaping and
word-wrap implementations can differ at break boundaries; ink, line-count and
2% glyph-count warnings check conformity, with trailing break spaces explained
above.
Glyphon uses `Shaping::Advanced`, reuses its Cache/TextAtlas/Viewport/TextRenderer
across frames, updates its viewport per frame and trims its atlas after each
frame, following its examples. This crate uses its public rendering API;
`internals` supplies fallback memory inspection and the raster section’s
bitmap initialization, cache reset and timestamp hook.

On llvmpipe, lavapipe, SwiftShader or another CPU/software adapter, all these
numbers only check the harness and say nothing about GPU performance. The
adapter, backend, driver, software flag, clock source and wgpu version remain in
the report. Compare performance only on the same hardware/backend/driver with
consistent thread settings, and obtain real GPU runs before making GPU claims.

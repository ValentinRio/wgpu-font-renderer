# Shader benchmark

The native shader benchmark renders four fixed scenes at 1920×1080 in sRGB,
using the public font loading, shaping and rendering API. Run it with:

```sh
cargo bench --all-features --offline --bench shader
BENCH_ROUNDS=7 BENCH_FRAMES=40 BENCH_WARMUP=10 cargo bench --all-features --offline --bench shader -- large_glyphs
```

Set the `WGPU_BACKEND` environment variable to `dx12` or `vulkan` to select the backend (POSIX shells: `WGPU_BACKEND=dx12 cargo bench --all-features ...`; PowerShell: `$env:WGPU_BACKEND = "dx12"`; cmd: `set WGPU_BACKEND=dx12`). Leave it unset for wgpu's defaults. Some Vulkan drivers return zero timestamps (seen on an AMD RX 9070 XT with the proprietary driver); DX12 works there.

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

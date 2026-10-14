# Test notes

The Cantarell font uses CFF2 outlines. Tests check every outlined glyph's segment
chaining and padded bounds. The GPU regression test renders "Hi" and requires
ink in the expected box, a dark H stem, a white H gap, and a bounded ink count.
Contours are closed and winding is selected from each glyph's total signed area.
Variable fonts are loaded at their default variation instance.

The shader has two CPU oracles in `tests/common`, and no frozen shader files.
`reference.rs` computes box-filtered nonzero coverage by supersampling.
`centre.rs` is a separate CPU model of the shader's own method: two centre rays
over the glyph's full, unbanded curve list.

`tests/analytic_coverage.rs` renders every outlined preset glyph of Roboto (TrueType),
Cantarell (CFF2) and the Noto Serif SC subset (CFF) at 12, 16, 24, 48 and 200 px.
Each pixel must equal the centre-ray model within readback precision, which also catches band and
early-exit mistakes. Against the box-filter reference, the mean and maximum
error must stay under the constants in `tests/common/coverage_limits.rs`. Those
bounds describe how far two centre rays sit from a box filter. They were set
from the first green measurement and are not slack to be widened.

The Noto glyphs use their own mean bounds, `C3_DENSE_MEAN_ERRORS`. A dense CJK
glyph has more edge pixels per area than a Latin letter, and edges are
where two centre rays differ from a box filter, so its mean error is higher. The
centre-ray match and the maximum error bound are the same for all three fonts.

`tests/unit/analytic_regressions.rs` covers overlapping and nested contours,
rays through curve endpoints and tangent to extrema, and thin features. Pixels
away from any outline vertex must match the box-filter reference tightly, so an
error that runs along a row or column fails.

The band regression test builds an all-curves variant from the shipped shader
source and compares its RGBA8 output with the banded path for TrueType and CFF2
glyphs, with test-only instrumentation asserting the band path was taken. A
load of over 1000 outlined characters forces band-layer growth mid-load; probes
include glyphs near an old layer's tail and in a new layer.

Placement regressions upload Roboto `g` and `B` and Cantarell CFF2 `@` at layer 0,
layer 2, and row starts 2040 and 2045 (including segments split across rows).
Another regression forces their band allocations to start at column 2045 and
wrap a row. They compare banded and forced full-list RGBA8 output at all twelve
sampled sizes and instrument band use at size 400. Every reference size tile
must contain at least 64 black pixels with alpha >= 32 before pixel comparison.

`tests/text_pipeline.rs` shapes "office fluffy" in Roboto and Cantarell for
both default and variation loads. Every output must be cached, and GPU readback
must contain ink strictly inside each ffi/ffl box. Loading only "f" must omit
those ligatures because i/l are outside the preset. Removing GSUB closure makes
both cache membership and ink checks fail. Empty cached glyphs allocate no atlas
storage and produce no coverage samples.

`src/gsub.rs` tests every output-producing lookup type, extension lookups,
context/chained-context targets, and ligatures' all-components rule. Crafted
single-substitution cycles and large coverage tables exercise the iteration and
work caps, including through the GPU loader. `LayoutError` tests assert each
validation cause.

Run the full suite with `REQUIRE_GPU=1 cargo test --offline --all-features`.
The `unstable-internals` feature enables integration targets that inspect fonts
and the atlas; its hidden inspection module is exempt from semver.

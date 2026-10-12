# Font fixture

Cantarell-VF.otf is the Cantarell variable font, version 0.303, distributed under the
SIL Open Font License 1.1. Its embedded name table includes the copyright,
license notice, and license URL (http://scripts.sil.org/OFL). The full license
text is in [OFL.txt](OFL.txt).
Copyright 2019 The Cantarell Project Authors
(https://gitlab.gnome.org/GNOME/cantarell-fonts).

Copied unchanged from `owned_ttf_parser` 0.21.0's `fonts/Cantarell-VF.otf`
in the Cargo registry. Upstream source:
https://github.com/alexheretic/owned-ttf-parser/tree/v0.21.0/fonts

This fixture uses CFF2 outlines. Tests check every outlined glyph's segment
chaining and padded bounds. The GPU regression test renders "Hi" and requires
ink in the expected box, a dark H stem, a white H gap, and a bounded ink count.
Contours are closed and winding is selected from each glyph's total signed area.
Variable fonts are loaded at their default variation instance.

The shader has two CPU oracles in `tests/common`, and no frozen shader files.
`reference.rs` computes box-filtered nonzero coverage by supersampling.
`centre.rs` is a separate CPU model of the shader's own method: two centre rays
over the glyph's full, unbanded curve list.

`tests/analytic_coverage.rs` renders every cached glyph of Roboto (TrueType) and
this fixture (CFF2) at 12, 16, 24, 48 and 200 px. Each pixel must equal the
centre-ray model within readback precision, which also catches band and
early-exit mistakes. Against the box-filter reference, the mean and maximum
error must stay under the constants in `tests/common/coverage_limits.rs`. Those
bounds describe how far two centre rays sit from a box filter. They were set
from the first green measurement and are not slack to be widened.

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

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

`full-list.wgsl` is the pre-band reference shader with corrected linear atlas reads on
the glyph’s actual layer. Its legacy row restart and sampler reads were removed.
The GPU band regression test compares its RGBA8 output with the current shader for TrueType
and CFF2 glyphs, including disconnected contours, row wrapping, and font sizes
on both sides of the AA-window fallback threshold. Test-only instrumentation
verifies the band path at size 400 and the 557/558 cutoff. A load of over 1000
outlined characters with consecutive repeats forces band-layer growth mid-load;
probes include glyphs near an old layer's tail and in a new layer. Every growth
probe must pass the instrumented GPU assertion that the band path completed at
sizes 400 and 557, so matching full-list fallbacks cannot satisfy the regression.

`original-main.wgsl` is copied byte-for-byte from
`a1bef57:tests/fixtures/full-list.wgsl`. It retains the original sampler reads and
curve loop as an independent oracle. Tests compare several unwrapped layer-0
TrueType and CFF2 glyphs at twelve sizes; test source adapts only its varying
interface and the common coverage probe.

Placement regressions upload Roboto `g` and `B` and Cantarell CFF2 `@` at layer 0,
layer 2, and row starts 2040 and 2045 (including segments split across rows).
Another regression forces their band allocations to start at column 2045 and
wrap a row. They compare banded and forced full-list RGBA8 output at all twelve
sampled sizes and instrument band use at size 400. Every reference size tile
must contain at least 64 black pixels with alpha >= 32 before pixel comparison.
